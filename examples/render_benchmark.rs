//! Dependency-free timing of the existing renderer and the M1 pipeline.
//!
//! Informational in M1; it becomes a regression gate when `Renderer` is
//! reimplemented on the pipeline. Run in release mode:
//!
//! ```text
//! cargo run --release --example render_benchmark -- [iterations]
//! ```

use std::hint::black_box;
use std::time::{Duration, Instant};

use trend::{
    AnsiPresenter, CellAspect, CellEncoder, CellFrame, FitMode, HalfBlock, ImageSource,
    PixelBuffer, PixelFormat, PixelFrameView, Quadrant, RasterSpec, RenderOptions, RenderRequest,
    Renderer, SamplingFilter, TerminalColorMode, Viewport, dither,
};

fn source_pixels(width: u32, height: u32) -> Vec<u8> {
    let mut pixels = Vec::with_capacity(width as usize * height as usize * 4);
    for y in 0..height {
        for x in 0..width {
            pixels.extend_from_slice(&[
                (x * 255 / width) as u8,
                (y * 255 / height) as u8,
                ((x ^ y) & 0xFF) as u8,
                255,
            ]);
        }
    }
    pixels
}

fn time(iterations: u32, mut run: impl FnMut()) -> Duration {
    run();
    let started = Instant::now();
    for _ in 0..iterations {
        run();
    }
    started.elapsed() / iterations
}

fn main() {
    let iterations: u32 = std::env::args()
        .nth(1)
        .and_then(|value| value.parse().ok())
        .unwrap_or(50);
    let (width, height) = (1920, 1080);
    let pixels = source_pixels(width, height);
    let source = PixelFrameView::new(
        &pixels,
        width,
        height,
        width as usize * 4,
        PixelFormat::Rgba8,
        std::time::Duration::ZERO,
    )
    .unwrap();
    let renderer = Renderer::new();

    println!("source {width}x{height}, {iterations} iterations, times per frame");
    println!(
        "{:<9} {:>9} {:>12} {:>12} {:>7} {:>12} {:>14}",
        "filter", "viewport", "Renderer", "HalfBlock", "ratio", "Quadrant", "ANSI bytes"
    );
    for viewport in [
        Viewport {
            width: 80,
            height: 24,
        },
        Viewport {
            width: 200,
            height: 60,
        },
    ] {
        for filter in [
            SamplingFilter::Nearest,
            SamplingFilter::Bilinear,
            SamplingFilter::Area,
        ] {
            let request = RenderRequest {
                viewport,
                fit: FitMode::Contain,
                generation: 1,
            };
            let options = RenderOptions {
                sampling: filter,
                dither_strength: 8,
                ..RenderOptions::default()
            };
            let image = ImageSource::new(source)
                .with_fit(FitMode::Contain)
                .with_sampling(filter);
            let mut cells = CellFrame::default();
            let mut buffer = PixelBuffer::new(1, 1).unwrap();

            let old = time(iterations, || {
                renderer
                    .render_with_options_into(source, request, options, &mut cells)
                    .unwrap();
                black_box(&cells);
            });

            let mut pipeline = |encoder: &dyn CellEncoder, cells: &mut CellFrame| {
                let spec =
                    RasterSpec::new(viewport, encoder.grid(), CellAspect::default()).unwrap();
                image.render_into(&spec, &mut buffer).unwrap();
                dither::ordered_4x4_in_place(&mut buffer, options.dither_strength);
                cells.resize(viewport.width, viewport.height).unwrap();
                encoder.encode(buffer.view(), cells.view_mut()).unwrap();
                black_box(&*cells);
            };
            let half = time(iterations, || pipeline(&HalfBlock, &mut cells));
            let quadrant = time(iterations, || pipeline(&Quadrant, &mut cells));

            pipeline(&HalfBlock, &mut cells);
            let ansi_bytes = AnsiPresenter::new(TerminalColorMode::TrueColor)
                .present(&cells)
                .len();

            println!(
                "{:<9} {:>9} {:>12.2?} {:>12.2?} {:>6.2}x {:>12.2?} {:>14}",
                format!("{filter:?}"),
                format!("{}x{}", viewport.width, viewport.height),
                old,
                half,
                half.as_secs_f64() / old.as_secs_f64(),
                quadrant,
                ansi_bytes
            );
        }
    }
}
