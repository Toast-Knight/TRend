//! External renderer → framebuffer → TRend → terminal, end to end.
//!
//! `tiny-raster` is an independent crate with no TRend dependency, like any
//! engine or GPU readback. This host is the only code that knows both sides:
//!
//! 1. It sizes the renderer to the TRend surface's pixel grid and tells it
//!    the physical pixel aspect, so nothing is resampled or stretched.
//! 2. It translates the renderer's own [`FrameInfo`] into a [`PixelLayout`].
//!    That is the entire integration.
//! 3. `Surface::encode_from` reads the borrowed BGRA, bottom-up, premultiplied
//!    frame in place (zero copies), then a status line is drawn as text and
//!    the frame is presented.
//!
//! Every stage is timed, including the real write and flush to the
//! terminal, which is where the rest of a 30-FPS frame goes. At exit it
//! prints a per-stage summary against the 33.3 ms budget.
//!
//! ```text
//! cargo run --release -p trend-external-renderer -- \
//!     [--encoder half|quadrant|sextant|braille] [--size COLSxROWS] \
//!     [--frames N] [--fps N] [--colors truecolor|256|16] [--compact]
//! ```
//!
//! Without `--size`, the size comes from `COLUMNS`/`LINES` or defaults to
//! 100×30. Detecting the real terminal size belongs to a terminal-session
//! crate, not to TRend's core.

use std::alloc::{GlobalAlloc, Layout, System};
use std::env;
use std::error::Error;
use std::io::{self, Write};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::thread;
use std::time::{Duration, Instant};

use tiny_raster::{AlphaConvention, ChannelOrder, FrameInfo, Origin, Renderer};
use trend::text::{TextBackground, TextStyle, put_str};
use trend::{
    AlphaMode, Braille, Cell, CellEncoder, Color, HalfBlock, OutputOptions, PixelFormat,
    PixelFrameView, PixelLayout, Quadrant, RowOrder, Sextant, Surface, TerminalCapabilities,
    TerminalColorMode, Viewport,
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

/// The whole integration: the renderer's description of its memory, in
/// TRend's terms.
fn layout_of(info: FrameInfo) -> PixelLayout {
    let format = match info.order {
        ChannelOrder::Bgra => PixelFormat::Bgra8,
    };
    let alpha = match info.alpha {
        AlphaConvention::Premultiplied => AlphaMode::Premultiplied,
    };
    let row_order = match info.origin {
        Origin::BottomLeft => RowOrder::BottomUp,
    };
    PixelLayout::new(info.width, info.height, format, alpha)
        .with_stride(info.stride)
        .with_row_order(row_order)
}

struct Options {
    encoder: Box<dyn CellEncoder>,
    encoder_name: &'static str,
    viewport: Viewport,
    frames: u32,
    fps: u32,
    colors: TerminalColorMode,
    compact: bool,
}

fn parse() -> Result<Options, Box<dyn Error>> {
    let from_env = |name| {
        env::var(name)
            .ok()
            .and_then(|value| value.parse::<u16>().ok())
    };
    let mut options = Options {
        encoder: Box::new(Sextant),
        encoder_name: "sextant",
        viewport: Viewport {
            width: from_env("COLUMNS").unwrap_or(100),
            height: from_env("LINES").map_or(30, |lines| lines.saturating_sub(1).max(1)),
        },
        frames: 300,
        fps: 30,
        colors: TerminalColorMode::TrueColor,
        compact: false,
    };
    let mut arguments = env::args().skip(1);
    while let Some(argument) = arguments.next() {
        let mut value = || arguments.next().ok_or(format!("{argument} needs a value"));
        match argument.as_str() {
            "--encoder" => {
                let chosen: (Box<dyn CellEncoder>, &'static str) = match value()?.as_str() {
                    "half" => (Box::new(HalfBlock), "half"),
                    "quadrant" => (Box::new(Quadrant), "quadrant"),
                    "sextant" => (Box::new(Sextant), "sextant"),
                    "braille" => (Box::new(Braille), "braille"),
                    other => return Err(format!("unknown encoder {other}").into()),
                };
                (options.encoder, options.encoder_name) = chosen;
            }
            "--size" => {
                let size = value()?;
                let (columns, rows) = size.split_once('x').ok_or("--size needs COLSxROWS")?;
                options.viewport = Viewport {
                    width: columns.parse()?,
                    height: rows.parse()?,
                };
            }
            "--frames" => options.frames = value()?.parse()?,
            "--fps" => options.fps = value()?.parse::<u32>()?.max(1),
            "--colors" => {
                options.colors = match value()?.as_str() {
                    "256" => TerminalColorMode::Ansi256,
                    "16" => TerminalColorMode::Ansi16,
                    _ => TerminalColorMode::TrueColor,
                }
            }
            "--compact" => options.compact = true,
            other => return Err(format!("unknown argument {other}").into()),
        }
    }
    Ok(options)
}

#[derive(Default)]
struct Samples {
    render: Vec<Duration>,
    encode: Vec<Duration>,
    present: Vec<Duration>,
    write: Vec<Duration>,
    bytes: Vec<usize>,
    trend_allocations: usize,
}

fn median(values: &mut [Duration]) -> Duration {
    values.sort();
    values.get(values.len() / 2).copied().unwrap_or_default()
}

fn milliseconds(duration: Duration) -> f64 {
    duration.as_secs_f64() * 1e3
}

fn main() -> Result<(), Box<dyn Error>> {
    let options = parse()?;
    let capabilities = TerminalCapabilities {
        color: options.colors,
        unicode: true,
    };
    let mut surface = Surface::new(options.encoder, options.viewport, capabilities)?;
    surface.set_output_options(
        OutputOptions::new()
            .with_synchronized_updates(true)
            .with_combined_colors(true)
            .with_compact_indexed_colors(options.compact),
    );

    // The renderer draws exactly the surface's pixel grid, in the surface's
    // physical pixel shape: no resampling, no stretching.
    let spec = *surface.spec();
    let aspect = spec.pixel_aspect();
    let mut renderer = Renderer::new();
    renderer.resize(spec.width(), spec.height());

    let mut stdout = io::stdout().lock();
    write!(stdout, "\x1b[2J\x1b[?25l")?;
    let frame_time = Duration::from_secs(1) / options.fps;
    let started = Instant::now();
    let mut samples = Samples::default();
    let mut status = String::with_capacity(160);
    let label = TextStyle::new(Color::rgb(255, 230, 140))
        .with_background(TextBackground::Color(Color::rgb(15, 18, 26)))
        .with_attributes(Cell::BOLD);

    for frame in 0..options.frames {
        let time = frame as f32 / options.fps as f32;

        // 1. The external renderer, in its own memory and conventions.
        let render_started = Instant::now();
        renderer.render(time, aspect.width as f32 / aspect.height as f32);
        let render = render_started.elapsed();

        // 2. TRend borrows that memory. Nothing is copied or converted.
        let allocations_before = ALLOCATIONS.load(Ordering::Relaxed);
        let encode_started = Instant::now();
        let frame_view =
            PixelFrameView::from_layout(renderer.pixels(), layout_of(renderer.info()))?;
        surface.encode_from(frame_view)?;
        let encode = encode_started.elapsed();

        // 3. A status line as text over the cells, then the ANSI bytes.
        let present_started = Instant::now();
        {
            use std::fmt::Write as _;
            status.clear();
            let last = |values: &[Duration]| values.last().copied().map_or(0.0, milliseconds);
            let _ = write!(
                status,
                " {} | render {:.1} ms | encode {:.1} | present {:.1} | write {:.1} | {} KB ",
                options.encoder_name,
                last(&samples.render),
                last(&samples.encode),
                last(&samples.present),
                last(&samples.write),
                samples.bytes.last().copied().unwrap_or(0) / 1024,
            );
        }
        put_str(&mut surface.cells_mut(), 0, 0, &status, label);
        let bytes = surface.present_bytes();
        let byte_count = bytes.len();
        let present = present_started.elapsed();
        let trend_allocations = ALLOCATIONS.load(Ordering::Relaxed) - allocations_before;

        // 4. The terminal: writing and flushing is where it pushes back.
        let write_started = Instant::now();
        stdout.write_all(bytes)?;
        stdout.flush()?;
        let write = write_started.elapsed();

        if frame >= 2 {
            samples.render.push(render);
            samples.encode.push(encode);
            samples.present.push(present);
            samples.write.push(write);
            samples.bytes.push(byte_count);
            samples.trend_allocations += trend_allocations;
        }
        if let Some(wait) = (frame_time * (frame + 1)).checked_sub(started.elapsed()) {
            thread::sleep(wait);
        }
    }
    let elapsed = started.elapsed();
    write!(
        stdout,
        "\x1b[{};1H\x1b[0m\x1b[?25h",
        options.viewport.height + 1
    )?;
    stdout.flush()?;
    drop(stdout);

    let frames = samples.render.len().max(1);
    let total_bytes: usize = samples.bytes.iter().sum();
    let (render, encode, present, write) = (
        median(&mut samples.render),
        median(&mut samples.encode),
        median(&mut samples.present),
        median(&mut samples.write),
    );
    let budget = Duration::from_secs(1) / options.fps;
    let share = |duration: Duration| duration.as_secs_f64() * 100.0 / budget.as_secs_f64();
    println!(
        "{} {}x{} cells ({}x{} px), {:?}{}, {} frames in {:.1} s ({:.1} fps)",
        options.encoder_name,
        options.viewport.width,
        options.viewport.height,
        spec.width(),
        spec.height(),
        options.colors,
        if options.compact { " compact" } else { "" },
        options.frames,
        elapsed.as_secs_f64(),
        f64::from(options.frames) / elapsed.as_secs_f64(),
    );
    println!(
        "median per frame against a {:.1} ms budget:",
        milliseconds(budget)
    );
    for (name, value) in [
        ("renderer (external)", render),
        ("TRend encode (zero-copy)", encode),
        ("TRend text + present", present),
        ("terminal write + flush", write),
    ] {
        println!(
            "  {name:<26} {:>7.2} ms  {:>5.1}%",
            milliseconds(value),
            share(value)
        );
    }
    println!(
        "  output {:.1} KB/frame, {:.1} MB/s; TRend allocations after warm-up: {}",
        total_bytes as f64 / frames as f64 / 1024.0,
        total_bytes as f64 / elapsed.as_secs_f64() / 1e6,
        samples.trend_allocations,
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The borrowed frame encodes exactly like its straight-RGBA equivalent.
    /// (That this path allocates nothing is tested in the core's
    /// `tests/zero_copy.rs` and measured by `main` at run time.)
    #[test]
    fn the_renderer_frame_is_encoded_in_place_and_correctly() {
        let viewport = Viewport {
            width: 40,
            height: 12,
        };
        for encoder in [
            &HalfBlock as &dyn CellEncoder,
            &Quadrant,
            &Sextant,
            &Braille,
        ] {
            let mut surface =
                Surface::new(encoder, viewport, TerminalCapabilities::CONSERVATIVE).unwrap();
            let spec = *surface.spec();
            let mut renderer = Renderer::new();
            renderer.resize(spec.width(), spec.height());
            renderer.render(0.8, 1.0);
            let view =
                PixelFrameView::from_layout(renderer.pixels(), layout_of(renderer.info())).unwrap();
            assert!(spec.matches(&view));

            surface.encode_from(view).unwrap();

            // Reference: normalize to straight RGBA, then encode.
            let mut normalized = trend::PixelBuffer::new(spec.width(), spec.height()).unwrap();
            for y in 0..spec.height() {
                for x in 0..spec.width() {
                    let offset = (y * spec.width() + x) as usize * 4;
                    normalized.pixels_mut()[offset..offset + 4]
                        .copy_from_slice(&view.rgba(x, y).unwrap());
                }
            }
            let mut expected = trend::CellFrame::default();
            expected.resize(viewport.width, viewport.height).unwrap();
            encoder
                .encode(normalized.view(), expected.view_mut())
                .unwrap();
            assert_eq!(surface.cells(), &expected);
            // The transparent background stays the terminal's own.
            assert!(
                surface
                    .cells()
                    .cells()
                    .iter()
                    .any(|cell| *cell == Cell::default())
            );
        }
    }
}
