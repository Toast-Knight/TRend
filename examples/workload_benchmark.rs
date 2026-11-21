//! Realistic TRend workloads, timed stage by stage.
//!
//! Complements `render_benchmark` (a synthetic single-image comparison) with
//! the kinds of frames TRend is meant to serve, so optimization targets come
//! from measurement:
//!
//! - `photo`: a static 1920×1080 photographic image with grain, resampled.
//! - `pan`: the same photo slowly panned and zoomed, like video; most cells
//!   change every frame.
//! - `plasma`: a procedural effect drawn natively at the encoder's resolution;
//!   every cell changes every frame.
//! - `ui`: a mostly static procedural interface with one small moving sprite.
//! - `readback`: a BGRA, bottom-up, premultiplied, 256-byte-aligned frame at
//!   exact size, encoded zero-copy versus resampled through `ImageSource`.
//! - `bands`: the photo resampled whole versus in row bands (sequential and on
//!   four threads).
//! - `output`: output options (combined colors, synchronized updates,
//!   compact indexed colors) on full-change frames.
//! - `surface`: `Surface` against the manual pipeline, and damage-driven
//!   region encoding and presentation against full frames (`ui`).
//!
//! Stages, in microseconds per frame:
//!
//! - `source`: `ImageSource` resampling, or the app's own drawing (native);
//! - `encode`: the cell encoder;
//! - `diff`: presenting an unchanged frame (the cost of the diff scan alone);
//! - `present`: presenting the new frame (diff plus ANSI serialization).
//!
//! ```text
//! cargo run --release --example workload_benchmark -- [frames]
//! ```

use std::hint::black_box;
use std::time::{Duration, Instant};

use trend::{
    AlphaMode, AnsiPresenter, Braille, CellAspect, CellEncoder, CellFrame, Density, FitMode,
    HalfBlock, ImageSource, OutputOptions, PixelBuffer, PixelFormat, PixelFrameView, PixelLayout,
    PixelRect, PixelViewMut, Quadrant, RasterSpec, RenderOptions, RenderRequest, RenderTransform,
    Renderer, RowOrder, SamplingFilter, Sextant, Surface, TerminalCapabilities, TerminalColorMode,
    Viewport,
};

const SIZES: [(u16, u16); 4] = [(80, 24), (120, 40), (200, 60), (320, 90)];

fn hash(x: u32, y: u32, seed: u32) -> u32 {
    let mut value = x.wrapping_mul(0x8DA6_B343) ^ y.wrapping_mul(0xD816_3841) ^ seed;
    value ^= value >> 13;
    value = value.wrapping_mul(0x5bd1_e995);
    value ^ (value >> 15)
}

/// Smooth value noise in 0..=255 at a given cell size.
fn value_noise(x: u32, y: u32, cell: u32, seed: u32) -> u32 {
    let (cx, cy) = (x / cell, y / cell);
    let (fx, fy) = (x % cell, y % cell);
    let corner = |dx, dy| hash(cx + dx, cy + dy, seed) & 0xFF;
    let top = corner(0, 0) * (cell - fx) + corner(1, 0) * fx;
    let bottom = corner(0, 1) * (cell - fx) + corner(1, 1) * fx;
    (top * (cell - fy) + bottom * fy) / (cell * cell)
}

/// A photograph-like image: layered noise, a sky gradient, and sensor grain.
fn photo(width: u32, height: u32) -> Vec<u8> {
    let mut pixels = Vec::with_capacity(width as usize * height as usize * 4);
    for y in 0..height {
        for x in 0..width {
            let terrain = (value_noise(x, y, 160, 1) * 5
                + value_noise(x, y, 40, 2) * 2
                + value_noise(x, y, 9, 3))
                / 8;
            let sky = y * 255 / height;
            let grain = (hash(x, y, 99) & 0x1F) as i32 - 16;
            let channel = |base: u32| (base as i32 + grain).clamp(0, 255) as u8;
            pixels.extend_from_slice(&[
                channel((terrain + sky / 3) * 3 / 4),
                channel((terrain * 5 / 6 + 40).min(255)),
                channel((255 - sky) / 2 + terrain / 3),
                255,
            ]);
        }
    }
    pixels
}

/// An animated full-screen effect drawn at the target resolution.
fn plasma(target: &mut PixelBuffer, time: u32) {
    fill(target.view_mut(), |x, y| plasma_at(x, y, time));
}

fn plasma_at(x: u32, y: u32, time: u32) -> [u8; 4] {
    let a = value_noise(x * 4 + time * 3, y * 4, 64, 7);
    let b = value_noise(x * 4, y * 4 + time * 5, 48, 8);
    [a as u8, ((a + b) / 2) as u8, b as u8, 255]
}

fn ui(target: &mut PixelBuffer, time: u32) {
    let (width, height) = (target.width(), target.height());
    fill(target.view_mut(), |x, y| ui_at(x, y, width, height, time));
}

/// Where the `ui` sprite is centered at `time`.
fn sprite_center(width: u32, height: u32, time: u32) -> (u32, u32) {
    (8 + time % (width.saturating_sub(16).max(1)), height / 2)
}

/// A mostly static interface: panels, rules, bar charts, and a moving sprite.
fn ui_at(x: u32, y: u32, width: u32, height: u32, time: u32) -> [u8; 4] {
    let sprite = sprite_center(width, height, time);
    if x.abs_diff(sprite.0) < 6 && y.abs_diff(sprite.1) < 6 {
        [240, 80, 60, 255]
    } else if y < 4 || x < 2 || x == width / 3 {
        [60, 64, 72, 255]
    } else if x < width / 3 {
        if y.is_multiple_of(6) {
            [90, 96, 110, 255]
        } else {
            [28, 30, 36, 255]
        }
    } else {
        let bar = (x - width / 3) / 6;
        let bar_height = (hash(bar, 0, 5) % (height / 2)) + 4;
        if !(x - width / 3).is_multiple_of(6) && y > height - bar_height {
            [70, 150, 210, 255]
        } else {
            [20, 22, 26, 255]
        }
    }
}

/// The pixels the `ui` sprite touches when it moves from `time - 1` to `time`.
fn sprite_damage(width: u32, height: u32, time: u32) -> PixelRect {
    let now = sprite_center(width, height, time);
    let before = sprite_center(width, height, time.saturating_sub(1));
    let left = now.0.min(before.0).saturating_sub(5);
    let right = now.0.max(before.0) + 6;
    let top = now.1.saturating_sub(5);
    PixelRect::new(left, top, right - left, 11)
}

fn fill(mut target: PixelViewMut<'_>, color: impl Fn(u32, u32) -> [u8; 4]) {
    for y in 0..target.height() {
        for (x, pixel) in target.row_mut(y).chunks_exact_mut(4).enumerate() {
            pixel.copy_from_slice(&color(x as u32, y));
        }
    }
}

#[derive(Default)]
struct Stages {
    source: Duration,
    encode: Duration,
    diff: Duration,
    present: Duration,
    bytes: usize,
    frames: u32,
}

impl Stages {
    fn row(&self, scene: &str, size: (u16, u16), variant: &str) {
        let per = |d: Duration| d.as_secs_f64() * 1e6 / f64::from(self.frames);
        let bytes = self.bytes / self.frames as usize;
        println!(
            "{scene:<9} {:>7} {variant:<22} {:>9.1} {:>9.1} {:>8.1} {:>9.1} {:>9} {:>8.1}",
            format!("{}x{}", size.0, size.1),
            per(self.source),
            per(self.encode),
            per(self.diff),
            per(self.present),
            bytes,
            bytes as f64 * 60.0 / 1e6,
        );
    }
}

fn timed<T>(total: &mut Duration, work: impl FnOnce() -> T) -> T {
    let started = Instant::now();
    let value = work();
    *total += started.elapsed();
    value
}

/// Encodes and presents one prepared frame, measuring each stage.
fn encode_and_present(
    encoder: &dyn CellEncoder,
    pixels: PixelFrameView<'_>,
    cells: &mut CellFrame,
    presenter: &mut AnsiPresenter,
    stages: &mut Stages,
) {
    timed(&mut stages.encode, || {
        encoder.encode(pixels, cells.view_mut()).unwrap()
    });
    let bytes = timed(&mut stages.present, || presenter.present(cells).len());
    stages.bytes += bytes;
    timed(&mut stages.diff, || {
        black_box(presenter.present(cells).len())
    });
    stages.frames += 1;
}

fn cells_for(viewport: Viewport) -> CellFrame {
    let mut cells = CellFrame::default();
    cells.resize(viewport.width, viewport.height).unwrap();
    cells
}

fn main() {
    let frames: u32 = std::env::args()
        .nth(1)
        .and_then(|value| value.parse().ok())
        .unwrap_or(20);
    let (photo_width, photo_height) = (1920, 1080);
    let photo_pixels = photo(photo_width, photo_height);
    let photo_view = PixelFrameView::new(
        &photo_pixels,
        photo_width,
        photo_height,
        photo_width as usize * 4,
        PixelFormat::Rgba8,
        Duration::ZERO,
    )
    .unwrap();
    let encoders: [(&str, &dyn CellEncoder); 5] = [
        ("half", &HalfBlock),
        ("quad", &Quadrant),
        ("sextant", &Sextant),
        ("braille", &Braille),
        ("density", &Density::ASCII),
    ];

    println!("{frames} frames per row; times in µs per frame; truecolor output");
    println!(
        "{:<9} {:>7} {:<22} {:>9} {:>9} {:>8} {:>9} {:>9} {:>8}",
        "scene", "cells", "variant", "source", "encode", "diff", "present", "bytes", "MB/s@60"
    );

    for size in SIZES {
        let viewport = Viewport {
            width: size.0,
            height: size.1,
        };

        // Static photo: resample once per frame (as a preview would), present
        // an unchanged image after the first frame.
        for filter in [
            SamplingFilter::Nearest,
            SamplingFilter::Bilinear,
            SamplingFilter::Area,
        ] {
            for (name, encoder) in encoders {
                let spec =
                    RasterSpec::new(viewport, encoder.grid(), CellAspect::default()).unwrap();
                let source = ImageSource::new(photo_view).with_sampling(filter);
                let mut pixels = PixelBuffer::new(1, 1).unwrap();
                let mut cells = cells_for(viewport);
                let mut presenter = AnsiPresenter::new(TerminalColorMode::TrueColor);
                let mut stages = Stages::default();
                for _ in 0..frames {
                    timed(&mut stages.source, || {
                        source.render_into(&spec, &mut pixels).unwrap()
                    });
                    encode_and_present(
                        encoder,
                        pixels.view(),
                        &mut cells,
                        &mut presenter,
                        &mut stages,
                    );
                }
                stages.row("photo", size, &format!("{name} {filter:?}"));
            }
        }

        // Legacy renderer on the same photo (sampling + encoding fused).
        for filter in [SamplingFilter::Nearest, SamplingFilter::Area] {
            let mut cells = cells_for(viewport);
            let mut stages = Stages::default();
            let request = RenderRequest {
                viewport,
                fit: FitMode::Contain,
                generation: 0,
            };
            let options = RenderOptions {
                sampling: filter,
                ..RenderOptions::default()
            };
            for _ in 0..frames {
                timed(&mut stages.source, || {
                    Renderer::new()
                        .render_with_options_into(photo_view, request, options, &mut cells)
                        .unwrap()
                });
                stages.frames += 1;
            }
            stages.row("photo", size, &format!("legacy Renderer {filter:?}"));
        }

        // Panning photo, like video: most cells change every frame.
        for (name, encoder) in encoders {
            let spec = RasterSpec::new(viewport, encoder.grid(), CellAspect::default()).unwrap();
            let mut pixels = PixelBuffer::new(1, 1).unwrap();
            let mut cells = cells_for(viewport);
            let mut presenter = AnsiPresenter::new(TerminalColorMode::TrueColor);
            let mut stages = Stages::default();
            for frame in 0..frames {
                let source = ImageSource::new(photo_view)
                    .with_fit(FitMode::Cover)
                    .with_sampling(SamplingFilter::Bilinear)
                    .with_transform(RenderTransform {
                        zoom_percent: 130,
                        pan_x_cells: frame as i32,
                        ..RenderTransform::default()
                    });
                timed(&mut stages.source, || {
                    source.render_into(&spec, &mut pixels).unwrap()
                });
                encode_and_present(
                    encoder,
                    pixels.view(),
                    &mut cells,
                    &mut presenter,
                    &mut stages,
                );
            }
            stages.row("pan", size, &format!("{name} Bilinear"));
        }

        // Native procedural scenes drawn at the encoder's resolution.
        for (scene, draw) in [
            ("plasma", plasma as fn(&mut PixelBuffer, u32)),
            ("ui", ui as fn(&mut PixelBuffer, u32)),
        ] {
            for (name, encoder) in encoders {
                let spec =
                    RasterSpec::new(viewport, encoder.grid(), CellAspect::default()).unwrap();
                let mut pixels = PixelBuffer::new(spec.width(), spec.height()).unwrap();
                let mut cells = cells_for(viewport);
                for mode in [TerminalColorMode::TrueColor, TerminalColorMode::Ansi256] {
                    let mut presenter = AnsiPresenter::new(mode);
                    let mut stages = Stages::default();
                    for frame in 0..frames {
                        timed(&mut stages.source, || draw(&mut pixels, frame));
                        encode_and_present(
                            encoder,
                            pixels.view(),
                            &mut cells,
                            &mut presenter,
                            &mut stages,
                        );
                    }
                    let mode = if mode == TerminalColorMode::TrueColor {
                        ""
                    } else {
                        " 256"
                    };
                    stages.row(scene, size, &format!("{name} native{mode}"));
                }
            }
        }

        // Exact-size readback: zero-copy encode vs resampling at 1:1.
        let spec = RasterSpec::new(viewport, HalfBlock.grid(), CellAspect::default()).unwrap();
        let stride = (spec.width() as usize * 4).next_multiple_of(256);
        let readback: Vec<u8> = (0..stride * spec.height() as usize)
            .map(|index| (index * 37 % 251) as u8)
            .collect();
        let layout = PixelLayout::new(
            spec.width(),
            spec.height(),
            PixelFormat::Bgra8,
            AlphaMode::Premultiplied,
        )
        .with_stride(stride)
        .with_row_order(RowOrder::BottomUp);
        let frame = PixelFrameView::from_layout(&readback, layout).unwrap();
        let mut cells = cells_for(viewport);
        let mut stages = Stages::default();
        for _ in 0..frames {
            timed(&mut stages.encode, || {
                HalfBlock.encode(frame, cells.view_mut()).unwrap()
            });
            stages.frames += 1;
        }
        stages.row("readback", size, "half zero-copy");
        let mut pixels = PixelBuffer::new(1, 1).unwrap();
        let mut stages = Stages::default();
        for _ in 0..frames {
            timed(&mut stages.source, || {
                ImageSource::new(frame)
                    .with_fit(FitMode::Stretch)
                    .render_into(&spec, &mut pixels)
                    .unwrap()
            });
            timed(&mut stages.encode, || {
                HalfBlock.encode(pixels.view(), cells.view_mut()).unwrap()
            });
            stages.frames += 1;
        }
        stages.row("readback", size, "half resampled 1:1");

        // Row bands: whole vs sequential bands vs four host threads.
        let spec = RasterSpec::new(viewport, HalfBlock.grid(), CellAspect::default()).unwrap();
        let source = ImageSource::new(photo_view).with_sampling(SamplingFilter::Area);
        let mut pixels = PixelBuffer::new(spec.width(), spec.height()).unwrap();
        let rows = spec.height().div_ceil(4);
        for (variant, threads) in [
            ("area whole", 0),
            ("area 8-row bands", 1),
            ("area 4 threads", 4),
        ] {
            let mut stages = Stages::default();
            for _ in 0..frames {
                timed(&mut stages.source, || match threads {
                    0 => source
                        .render_region(&spec, 0, 0, pixels.view_mut())
                        .unwrap(),
                    1 => {
                        for (first_row, band) in pixels.view_mut().into_row_bands(8) {
                            source.render_region(&spec, 0, first_row, band).unwrap();
                        }
                    }
                    _ => std::thread::scope(|scope| {
                        for (first_row, band) in pixels.view_mut().into_row_bands(rows) {
                            let spec = &spec;
                            scope.spawn(move || {
                                source.render_region(spec, 0, first_row, band).unwrap()
                            });
                        }
                    }),
                });
                stages.frames += 1;
            }
            stages.row("bands", size, variant);
        }

        output_and_surface(size, frames);
        println!();
    }
}

const TRUECOLOR: TerminalCapabilities = TerminalCapabilities {
    color: TerminalColorMode::TrueColor,
    unicode: true,
};

/// M4: output options, `Surface` overhead, and damage-driven regions.
fn output_and_surface(size: (u16, u16), frames: u32) {
    let viewport = Viewport {
        width: size.0,
        height: size.1,
    };
    let spec = RasterSpec::new(viewport, HalfBlock.grid(), CellAspect::default()).unwrap();

    // Output options on full-change frames (plasma: every cell changes).
    let mut plasma_frames = Vec::new();
    let mut pixels = PixelBuffer::new(spec.width(), spec.height()).unwrap();
    for frame in 0..frames {
        plasma(&mut pixels, frame);
        let mut cells = cells_for(viewport);
        HalfBlock.encode(pixels.view(), cells.view_mut()).unwrap();
        plasma_frames.push(cells);
    }
    for mode in [
        TerminalColorMode::TrueColor,
        TerminalColorMode::Ansi256,
        TerminalColorMode::Ansi16,
    ] {
        for (name, options) in [
            ("default", OutputOptions::new()),
            ("combined", OutputOptions::new().with_combined_colors(true)),
            ("sync", OutputOptions::new().with_synchronized_updates(true)),
            (
                "compact",
                OutputOptions::new().with_compact_indexed_colors(true),
            ),
        ] {
            if mode == TerminalColorMode::TrueColor && name == "compact" {
                continue; // No effect in truecolor.
            }
            let capabilities = TerminalCapabilities {
                color: mode,
                unicode: true,
            };
            let mut presenter = AnsiPresenter::with_options(capabilities, options);
            let mut stages = Stages::default();
            for cells in &plasma_frames {
                stages.bytes += timed(&mut stages.present, || presenter.present(cells).len());
                stages.frames += 1;
            }
            let mode = match mode {
                TerminalColorMode::TrueColor => "",
                TerminalColorMode::Ansi256 => " 256",
                _ => " 16",
            };
            stages.row("output", size, &format!("plasma {name}{mode}"));
        }
    }

    // Surface against the same steps by hand (plasma, draw + present).
    let mut surface = Surface::new(HalfBlock, viewport, TRUECOLOR).unwrap();
    let mut sink = std::io::sink();
    let mut stages = Stages::default();
    for frame in 0..frames {
        timed(&mut stages.source, || {
            surface
                .draw(|pixels, _| fill(pixels, |x, y| plasma_at(x, y, frame)))
                .unwrap()
        });
        stages.bytes += timed(&mut stages.present, || surface.present(&mut sink).unwrap()).bytes;
        stages.frames += 1;
    }
    stages.row("surface", size, "plasma draw+encode");
    let mut cells = cells_for(viewport);
    let mut presenter = AnsiPresenter::new(TerminalColorMode::TrueColor);
    let mut stages = Stages::default();
    for frame in 0..frames {
        timed(&mut stages.source, || {
            fill(pixels.view_mut(), |x, y| plasma_at(x, y, frame));
            HalfBlock.encode(pixels.view(), cells.view_mut()).unwrap();
        });
        stages.bytes += timed(&mut stages.present, || {
            presenter.present_at(&cells, 1, 1).len()
        });
        stages.frames += 1;
    }
    stages.row("surface", size, "plasma by hand");

    // A mostly static UI: re-encode and present everything, or only the
    // sprite's damage.
    let (width, height) = (spec.width(), spec.height());
    for damage_only in [false, true] {
        let mut surface = Surface::new(HalfBlock, viewport, TRUECOLOR).unwrap();
        surface
            .draw(|pixels, _| fill(pixels, |x, y| ui_at(x, y, width, height, 0)))
            .unwrap();
        surface.present(&mut sink).unwrap();
        let mut stages = Stages::default();
        for frame in 1..=frames {
            let damage = sprite_damage(width, height, frame);
            timed(&mut stages.source, || {
                let mut pixels = surface.pixels_mut();
                if damage_only {
                    let mut region = pixels.sub(damage).unwrap();
                    for y in 0..damage.height {
                        for (x, pixel) in region.row_mut(y).chunks_exact_mut(4).enumerate() {
                            let (x, y) = (damage.x + x as u32, damage.y + y);
                            pixel.copy_from_slice(&ui_at(x, y, width, height, frame));
                        }
                    }
                } else {
                    fill(pixels, |x, y| ui_at(x, y, width, height, frame));
                }
            });
            let region = timed(&mut stages.encode, || {
                if damage_only {
                    surface.encode_region(damage).unwrap()
                } else {
                    surface.encode().unwrap();
                    None
                }
            });
            let written = timed(&mut stages.present, || match region {
                Some(cells) => surface.present_region(cells, &mut sink).unwrap(),
                None => surface.present(&mut sink).unwrap(),
            });
            stages.bytes += written.bytes;
            stages.frames += 1;
        }
        let variant = if damage_only {
            "ui damage regions"
        } else {
            "ui full frames"
        };
        stages.row("surface", size, variant);
    }
}
