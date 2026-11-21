//! Where a 30-FPS frame goes: every encoder, realistic workloads, per stage.
//!
//! For each workload, terminal size, and encoder this prints the median time
//! per frame of each stage, the output bytes, the allocations per frame, and
//! TRend's share of a 33.3 ms (30 FPS) frame:
//!
//! - `app`: the application's own work (drawing a 3D-like scene, or the
//!   stand-in for a decoder). Not TRend's time; shown for the full budget.
//! - `resample`: `ImageSource` fitting a decoded frame to the surface.
//! - `encode`: the cell encoder.
//! - `present`: diffing and ANSI serialization.
//!
//! Workloads, all changing every frame except `photo`:
//!
//! - `video`: a 1920×1080 decoded frame, slowly panned and zoomed, resampled
//!   with Bilinear (what a video player does every frame).
//! - `photo`: a static 1920×1080 image resampled with Area once; later frames
//!   present nothing (a viewer, or a paused video).
//! - `scene3d`: a perspective floor and moving shaded spheres drawn by the app
//!   straight into the surface at encoder resolution.
//! - `readback`: an exact-size BGRA, bottom-up, premultiplied frame (a GPU
//!   readback) encoded in place with `encode_from`.
//!
//! ```text
//! cargo run --release --example frame_budget -- [frames] [--colors truecolor|256|16] [--compact]
//! ```

use std::alloc::{GlobalAlloc, Layout, System};
use std::hint::black_box;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, Instant};

use trend::{
    AlphaMode, Braille, CellEncoder, Density, FitMode, HalfBlock, ImageSource, OutputOptions,
    PixelBuffer, PixelFormat, PixelFrameView, PixelLayout, PixelViewMut, Quadrant, RenderTransform,
    RowOrder, SamplingFilter, Sextant, Surface, TerminalCapabilities, TerminalColorMode, Viewport,
};

struct CountingAllocator;
static ALLOCATIONS: AtomicUsize = AtomicUsize::new(0);

unsafe impl GlobalAlloc for CountingAllocator {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        ALLOCATIONS.fetch_add(1, Ordering::Relaxed);
        unsafe { System.alloc(layout) }
    }
    unsafe fn dealloc(&self, pointer: *mut u8, layout: Layout) {
        unsafe { System.dealloc(pointer, layout) }
    }
    unsafe fn realloc(&self, pointer: *mut u8, layout: Layout, size: usize) -> *mut u8 {
        ALLOCATIONS.fetch_add(1, Ordering::Relaxed);
        unsafe { System.realloc(pointer, layout, size) }
    }
}

#[global_allocator]
static ALLOCATOR: CountingAllocator = CountingAllocator;

const BUDGET: Duration = Duration::from_micros(33_333);

fn hash(x: u32, y: u32, seed: u32) -> u32 {
    let mut value = x.wrapping_mul(0x9E37_79B1) ^ y.wrapping_mul(0x85EB_CA77) ^ seed;
    value ^= value >> 15;
    value = value.wrapping_mul(0x2C1B_3C6D);
    value ^ (value >> 12)
}

/// A 1920×1080 photographic stand-in: smooth gradients, soft shapes, grain.
fn photo() -> (Vec<u8>, u32, u32) {
    let (width, height) = (1920_u32, 1080_u32);
    let mut pixels = Vec::with_capacity((width * height * 4) as usize);
    for y in 0..height {
        for x in 0..width {
            let grain = (hash(x, y, 1) % 17) as i32 - 8;
            let (fx, fy) = (x as f32 / width as f32, y as f32 / height as f32);
            let sky = fy < 0.55;
            let dx = fx - 0.7;
            let dy = fy - 0.3;
            let sun = dx * dx + dy * dy < 0.01;
            let base = if sun {
                [250, 235, 190]
            } else if sky {
                [(60.0 + 120.0 * fy) as i32, (110.0 + 90.0 * fy) as i32, 210]
            } else {
                let hill = ((fx * 9.0).sin() * 0.05 + 0.6 - fy).abs() < 0.01;
                if hill {
                    [40, 60, 40]
                } else {
                    [
                        (50.0 + 60.0 * fx) as i32,
                        (120.0 - 40.0 * fy) as i32,
                        (40.0 + 30.0 * (fx * 20.0).sin()) as i32,
                    ]
                }
            };
            for channel in base {
                pixels.push((channel + grain).clamp(0, 255) as u8);
            }
            pixels.push(255);
        }
    }
    (pixels, width, height)
}

/// A 3D-like scene drawn by the app: a perspective checker floor, a sky
/// gradient, and three shaded spheres that move every frame.
fn scene3d(mut pixels: PixelViewMut<'_>, aspect_width: f32, time: f32) {
    let (width, height) = (pixels.width(), pixels.height());
    let horizon = height as f32 * 0.45;
    let spheres = [0.0_f32, 2.1, 4.2].map(|phase| {
        let x = width as f32 * (0.5 + 0.3 * (time * 0.8 + phase).sin());
        let y = height as f32 * (0.62 + 0.1 * (time * 1.3 + phase).cos());
        (x, y, height as f32 * 0.14)
    });
    for y in 0..height {
        let row = pixels.row_mut(y);
        let fy = y as f32 + 0.5;
        for (x, pixel) in row.chunks_exact_mut(4).enumerate() {
            let fx = x as f32 + 0.5;
            let mut color = if fy < horizon {
                let t = fy / horizon;
                [
                    (20.0 + 80.0 * t) as u8,
                    (40.0 + 90.0 * t) as u8,
                    (90.0 + 110.0 * t) as u8,
                ]
            } else {
                let depth = height as f32 / (fy - horizon + 1.0);
                let u = ((fx - width as f32 / 2.0) * aspect_width * depth * 0.02 + time * 2.0)
                    .floor() as i32;
                let v = (depth * 2.0 + time * 4.0).floor() as i32;
                let fade = (1.0 / depth).min(1.0);
                let shade = if (u + v) & 1 == 0 { 190.0 } else { 60.0 };
                let value = (shade * fade + 30.0 * (1.0 - fade)) as u8;
                [value, value, (value as f32 * 0.9) as u8]
            };
            for (sx, sy, radius) in spheres {
                let dx = (fx - sx) * aspect_width;
                let dy = fy - sy;
                let distance = dx * dx + dy * dy;
                if distance < radius * radius {
                    let z = (1.0 - distance / (radius * radius)).sqrt();
                    let light = (0.3 + 0.7 * (z * 0.8 - dx / radius * 0.3 - dy / radius * 0.4))
                        .clamp(0.0, 1.0);
                    color = [
                        (230.0 * light) as u8,
                        (120.0 * light) as u8,
                        (60.0 * light) as u8,
                    ];
                }
            }
            pixel.copy_from_slice(&[color[0], color[1], color[2], 255]);
        }
    }
}

#[derive(Default)]
struct Row {
    app: Vec<Duration>,
    resample: Vec<Duration>,
    encode: Vec<Duration>,
    present: Vec<Duration>,
    bytes: Vec<usize>,
    allocations: Vec<usize>,
}

fn median(values: &mut [Duration]) -> Duration {
    if values.is_empty() {
        return Duration::ZERO;
    }
    values.sort();
    values[values.len() / 2]
}

impl Row {
    fn print(mut self, workload: &str, size: (u16, u16), encoder: &str) {
        let microseconds = |duration: Duration| duration.as_secs_f64() * 1e6;
        let (app, resample, encode, present) = (
            median(&mut self.app),
            median(&mut self.resample),
            median(&mut self.encode),
            median(&mut self.present),
        );
        let trend = resample + encode + present;
        self.bytes.sort_unstable();
        let bytes = self.bytes[self.bytes.len() / 2];
        let allocations = self.allocations.iter().copied().max().unwrap_or(0);
        println!(
            "{workload:<9} {:>7} {encoder:<8} {:>8.0} {:>8.0} {:>8.0} {:>8.0} {:>8.0} {:>6.1}% {:>9} {:>6.1} {:>6}",
            format!("{}x{}", size.0, size.1),
            microseconds(app),
            microseconds(resample),
            microseconds(encode),
            microseconds(present),
            microseconds(trend),
            trend.as_secs_f64() * 100.0 / BUDGET.as_secs_f64(),
            bytes,
            bytes as f64 * 30.0 / 1e6,
            allocations,
        );
    }
}

fn timed<T>(work: impl FnOnce() -> T) -> (T, Duration) {
    let started = Instant::now();
    let value = work();
    (value, started.elapsed())
}

fn main() {
    let arguments: Vec<String> = std::env::args().skip(1).collect();
    let frames: u32 = arguments
        .iter()
        .find_map(|argument| argument.parse().ok())
        .unwrap_or(30);
    let color = match arguments
        .iter()
        .position(|argument| argument == "--colors")
        .and_then(|index| arguments.get(index + 1))
        .map(String::as_str)
    {
        Some("256") => TerminalColorMode::Ansi256,
        Some("16") => TerminalColorMode::Ansi16,
        _ => TerminalColorMode::TrueColor,
    };
    let options = OutputOptions::new()
        .with_compact_indexed_colors(arguments.iter().any(|argument| argument == "--compact"));
    let capabilities = TerminalCapabilities {
        color,
        unicode: true,
    };

    let (photo_pixels, photo_width, photo_height) = photo();
    let photo_view = PixelFrameView::new(
        &photo_pixels,
        photo_width,
        photo_height,
        photo_width as usize * 4,
        PixelFormat::Rgba8,
        Duration::ZERO,
    )
    .expect("valid photo");
    let encoders: [(&str, &dyn CellEncoder); 5] = [
        ("half", &HalfBlock),
        ("quad", &Quadrant),
        ("sextant", &Sextant),
        ("braille", &Braille),
        ("density", &Density::ASCII),
    ];

    println!(
        "{frames} frames; medians in µs per frame; {color:?}{}; TRend = resample + encode + present; budget 33.3 ms",
        if options.compact_indexed_colors {
            ", compact indexed colors"
        } else {
            ""
        }
    );
    println!(
        "{:<9} {:>7} {:<8} {:>8} {:>8} {:>8} {:>8} {:>8} {:>7} {:>9} {:>6} {:>6}",
        "workload",
        "cells",
        "encoder",
        "app",
        "resample",
        "encode",
        "present",
        "TRend",
        "budget",
        "bytes",
        "MB/s",
        "allocs"
    );

    for size in [(200_u16, 60_u16), (320, 90)] {
        let viewport = Viewport {
            width: size.0,
            height: size.1,
        };
        for (name, encoder) in encoders {
            let mut surface = Surface::new(encoder, viewport, capabilities).expect("surface");
            surface.set_output_options(options);
            let mut sink = std::io::sink();

            // video: pan and zoom a decoded frame, resampled every frame.
            let mut row = Row::default();
            for frame in 0..frames + 2 {
                let source = ImageSource::new(photo_view)
                    .with_fit(FitMode::Cover)
                    .with_sampling(SamplingFilter::Bilinear)
                    .with_transform(RenderTransform {
                        zoom_percent: 115,
                        pan_x_cells: frame as i32 % 40 - 20,
                        ..RenderTransform::default()
                    });
                let before = ALLOCATIONS.load(Ordering::Relaxed);
                let spec = *surface.spec();
                let (_, resample) = timed(|| {
                    source
                        .render_region(&spec, 0, 0, surface.pixels_mut())
                        .expect("resample")
                });
                let (_, encode) = timed(|| surface.encode().expect("encode"));
                let (stats, present) = timed(|| surface.present(&mut sink).expect("present"));
                let allocations = ALLOCATIONS.load(Ordering::Relaxed) - before;
                if frame >= 2 {
                    row.resample.push(resample);
                    row.encode.push(encode);
                    row.present.push(present);
                    row.bytes.push(stats.bytes);
                    row.allocations.push(allocations);
                }
            }
            row.print("video", size, name);

            // photo: resample once, then nothing changes.
            surface.invalidate();
            let mut row = Row::default();
            let source = ImageSource::new(photo_view).with_sampling(SamplingFilter::Area);
            for frame in 0..frames.min(10) + 2 {
                let before = ALLOCATIONS.load(Ordering::Relaxed);
                let (_, resample) = if frame == 0 {
                    timed(|| surface.encode_image(&source).expect("image"))
                } else {
                    ((), Duration::ZERO)
                };
                let (stats, present) = timed(|| surface.present(&mut sink).expect("present"));
                let allocations = ALLOCATIONS.load(Ordering::Relaxed) - before;
                if frame == 0 {
                    // Report the first (only real) frame on its own line.
                    let mut first = Row::default();
                    first.resample.push(resample);
                    first.present.push(present);
                    first.bytes.push(stats.bytes);
                    first.allocations.push(0);
                    first.print("photo 1st", size, name);
                } else if frame >= 2 {
                    row.present.push(present);
                    row.bytes.push(stats.bytes);
                    row.allocations.push(allocations);
                }
            }
            row.print("photo", size, name);

            // scene3d: the app draws at encoder resolution; TRend encodes.
            surface.invalidate();
            let aspect = surface.spec().pixel_aspect();
            let aspect_width = aspect.width as f32 / aspect.height as f32;
            let mut row = Row::default();
            for frame in 0..frames + 2 {
                let time = frame as f32 / 30.0;
                let before = ALLOCATIONS.load(Ordering::Relaxed);
                let (_, app) = timed(|| scene3d(surface.pixels_mut(), aspect_width, time));
                let (_, encode) = timed(|| surface.encode().expect("encode"));
                let (stats, present) = timed(|| surface.present(&mut sink).expect("present"));
                let allocations = ALLOCATIONS.load(Ordering::Relaxed) - before;
                if frame >= 2 {
                    row.app.push(app);
                    row.encode.push(encode);
                    row.present.push(present);
                    row.bytes.push(stats.bytes);
                    row.allocations.push(allocations);
                }
            }
            row.print("scene3d", size, name);

            // readback: a GPU-style frame the app owns, encoded in place.
            surface.invalidate();
            let (width, height) = (surface.spec().width(), surface.spec().height());
            let stride = (width as usize * 4).next_multiple_of(256);
            let mut app_scene = PixelBuffer::new(width, height).expect("scene");
            let mut readback = vec![0_u8; stride * height as usize];
            let layout =
                PixelLayout::new(width, height, PixelFormat::Bgra8, AlphaMode::Premultiplied)
                    .with_stride(stride)
                    .with_row_order(RowOrder::BottomUp);
            let mut row = Row::default();
            for frame in 0..frames + 2 {
                let time = frame as f32 / 30.0;
                // The "GPU": render, then write a BGRA bottom-up readback.
                let (_, app) = timed(|| {
                    scene3d(app_scene.view_mut(), aspect_width, time);
                    for y in 0..height {
                        let target = (height - 1 - y) as usize * stride;
                        let source = app_scene.view().row(y).expect("row");
                        for (x, pixel) in source.chunks_exact(4).enumerate() {
                            readback[target + x * 4..target + x * 4 + 4]
                                .copy_from_slice(&[pixel[2], pixel[1], pixel[0], pixel[3]]);
                        }
                    }
                });
                let before = ALLOCATIONS.load(Ordering::Relaxed);
                let frame_view = PixelFrameView::from_layout(&readback, layout).expect("layout");
                let (_, encode) = timed(|| surface.encode_from(frame_view).expect("encode"));
                let (stats, present) = timed(|| surface.present(&mut sink).expect("present"));
                let allocations = ALLOCATIONS.load(Ordering::Relaxed) - before;
                if frame >= 2 {
                    row.app.push(app);
                    row.encode.push(encode);
                    row.present.push(present);
                    row.bytes.push(stats.bytes);
                    row.allocations.push(allocations);
                }
            }
            row.print("readback", size, name);
            black_box(&surface);
        }
        println!();
    }
}
