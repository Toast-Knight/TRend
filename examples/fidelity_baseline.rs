use std::alloc::{GlobalAlloc, Layout, System};
use std::hint::black_box;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Instant;

use trend::{
    AnsiPresenter, Cell, CellFrame, Color, FitMode, PixelFormat, PixelFrameView, RenderOptions,
    RenderRequest, Renderer, SamplingFilter, TerminalColorMode, Viewport,
    experimental::{
        ExperimentalGlyphOptions, ExperimentalGlyphRenderer, ExperimentalGlyphSet, glyph_coverage,
    },
};

const SOURCE_WIDTH: u32 = 640;
const SOURCE_HEIGHT: u32 = 192;
const VIEWPORT: Viewport = Viewport {
    width: 80,
    height: 24,
};
const ITERATIONS: u32 = 500;
const MATCHER_ITERATIONS: u32 = 50;

struct CountingAllocator;

static ALLOCATION_CALLS: AtomicUsize = AtomicUsize::new(0);

// The benchmark delegates storage to the system allocator and only counts calls.
unsafe impl GlobalAlloc for CountingAllocator {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        ALLOCATION_CALLS.fetch_add(1, Ordering::Relaxed);
        unsafe { System.alloc(layout) }
    }

    unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
        ALLOCATION_CALLS.fetch_add(1, Ordering::Relaxed);
        unsafe { System.alloc_zeroed(layout) }
    }

    unsafe fn dealloc(&self, pointer: *mut u8, layout: Layout) {
        unsafe { System.dealloc(pointer, layout) }
    }

    unsafe fn realloc(&self, pointer: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        ALLOCATION_CALLS.fetch_add(1, Ordering::Relaxed);
        unsafe { System.realloc(pointer, layout, new_size) }
    }
}

#[global_allocator]
static ALLOCATOR: CountingAllocator = CountingAllocator;

fn main() {
    let pixels = deterministic_fixture();
    let source = PixelFrameView::new(
        &pixels,
        SOURCE_WIDTH,
        SOURCE_HEIGHT,
        SOURCE_WIDTH as usize * 4,
        PixelFormat::Rgba8,
        std::time::Duration::ZERO,
    )
    .expect("valid deterministic fixture");
    let request = RenderRequest {
        viewport: VIEWPORT,
        fit: FitMode::Stretch,
        generation: 1,
    };
    let renderer = Renderer::new();
    let experimental = ExperimentalGlyphRenderer::new();
    let save_directory = save_directory();
    if let Some(directory) = &save_directory {
        std::fs::create_dir_all(directory).expect("create ANSI output directory");
    }

    println!("fixture=procedural-detail-v1");
    println!("source={}x{}", SOURCE_WIDTH, SOURCE_HEIGHT);
    println!("viewport={}x{}", VIEWPORT.width, VIEWPORT.height);
    println!("iterations={ITERATIONS}");
    measure(
        "nearest",
        SamplingFilter::Nearest,
        &pixels,
        source,
        request,
        &renderer,
        save_directory.as_deref(),
    );
    measure(
        "bilinear",
        SamplingFilter::Bilinear,
        &pixels,
        source,
        request,
        &renderer,
        save_directory.as_deref(),
    );
    measure(
        "area",
        SamplingFilter::Area,
        &pixels,
        source,
        request,
        &renderer,
        save_directory.as_deref(),
    );
    measure_experimental(
        "experimental-small",
        ExperimentalGlyphSet::Small,
        &pixels,
        source,
        request,
        &experimental,
        save_directory.as_deref(),
    );
    measure_experimental(
        "experimental-expanded",
        ExperimentalGlyphSet::Expanded,
        &pixels,
        source,
        request,
        &experimental,
        save_directory.as_deref(),
    );
    measure_experimental(
        "phase6b-reliable",
        ExperimentalGlyphSet::Reliable,
        &pixels,
        source,
        request,
        &experimental,
        save_directory.as_deref(),
    );
    measure_experimental(
        "phase6b-unique-expanded",
        ExperimentalGlyphSet::UniqueExpanded,
        &pixels,
        source,
        request,
        &experimental,
        save_directory.as_deref(),
    );
    measure_experimental(
        "phase6b-pruned",
        ExperimentalGlyphSet::Pruned,
        &pixels,
        source,
        request,
        &experimental,
        save_directory.as_deref(),
    );

    let pattern_pixels = pattern_suite_fixture();
    measure_pattern_suite(
        &pattern_pixels,
        request,
        &renderer,
        &experimental,
        save_directory.as_deref(),
    );
}

fn measure_pattern_suite(
    pixels: &[u8],
    request: RenderRequest,
    renderer: &Renderer,
    experimental: &ExperimentalGlyphRenderer,
    save_directory: Option<&Path>,
) {
    let source = PixelFrameView::new(
        pixels,
        SOURCE_WIDTH,
        SOURCE_HEIGHT,
        SOURCE_WIDTH as usize * 4,
        PixelFormat::Rgba8,
        std::time::Duration::ZERO,
    )
    .expect("valid pattern fixture");
    println!("fixture=synthetic-pattern-suite-v1");

    for (name, sampling) in [
        ("patterns-nearest", SamplingFilter::Nearest),
        ("patterns-bilinear", SamplingFilter::Bilinear),
        ("patterns-area", SamplingFilter::Area),
    ] {
        let mut frame = CellFrame::default();
        renderer
            .render_with_options_into(
                source,
                request,
                RenderOptions {
                    sampling,
                    ..RenderOptions::default()
                },
                &mut frame,
            )
            .expect("render pattern baseline");
        report_fixture_quality(name, pixels, &frame, save_directory);
    }

    for (name, glyph_set) in [
        ("patterns-phase6a-small", ExperimentalGlyphSet::Small),
        ("patterns-phase6a-expanded", ExperimentalGlyphSet::Expanded),
        ("patterns-phase6b-reliable", ExperimentalGlyphSet::Reliable),
        (
            "patterns-phase6b-unique-expanded",
            ExperimentalGlyphSet::UniqueExpanded,
        ),
        ("patterns-phase6b-pruned", ExperimentalGlyphSet::Pruned),
    ] {
        let mut frame = CellFrame::default();
        experimental
            .render_into(
                source,
                request,
                ExperimentalGlyphOptions {
                    sampling: SamplingFilter::Nearest,
                    glyph_set,
                    ..ExperimentalGlyphOptions::default()
                },
                &mut frame,
            )
            .expect("render experimental pattern fixture");
        report_fixture_quality(name, pixels, &frame, save_directory);
    }
}

fn report_fixture_quality(
    name: &str,
    pixels: &[u8],
    frame: &CellFrame,
    save_directory: Option<&Path>,
) {
    let mse = reconstruction_mse(pixels, frame);
    let psnr = 10.0 * (255.0_f64 * 255.0 / mse).log10();
    let mut presenter = AnsiPresenter::new(TerminalColorMode::TrueColor);
    let ansi_bytes = present_and_save(name, &mut presenter, frame, save_directory);
    println!("quality_path={name}");
    println!("first_frame_ansi_bytes={ansi_bytes}");
    println!("rgb_mse_8x8={mse:.3}");
    println!("rgb_psnr_8x8_db={psnr:.3}");
    println!("glyph_usage={}", glyph_usage(frame));
}

fn measure(
    name: &str,
    sampling: SamplingFilter,
    pixels: &[u8],
    source: PixelFrameView<'_>,
    request: RenderRequest,
    renderer: &Renderer,
    save_directory: Option<&Path>,
) {
    let options = RenderOptions {
        sampling,
        ..RenderOptions::default()
    };
    let mut frame = CellFrame::default();
    renderer
        .render_with_options_into(source, request, options, &mut frame)
        .expect("warm render");
    let allocations_before = ALLOCATION_CALLS.load(Ordering::Relaxed);
    let started = Instant::now();
    for _ in 0..ITERATIONS {
        renderer
            .render_with_options_into(source, request, options, &mut frame)
            .expect("measured render");
        black_box(frame.cells());
    }
    let elapsed = started.elapsed();
    let allocation_calls = ALLOCATION_CALLS
        .load(Ordering::Relaxed)
        .saturating_sub(allocations_before);
    let mut presenter = AnsiPresenter::new(TerminalColorMode::TrueColor);
    let ansi_bytes = present_and_save(name, &mut presenter, &frame, save_directory);
    let frame_bytes = frame.allocated_bytes();
    let estimated_retained_bytes = pixels
        .len()
        .saturating_add(frame_bytes.saturating_mul(2))
        .saturating_add(presenter.output_capacity());
    let mse = reconstruction_mse(pixels, &frame);
    let psnr = 10.0 * (255.0_f64 * 255.0 / mse).log10();

    println!("sampling={name}");
    println!(
        "mean_render_us={:.3}",
        elapsed.as_secs_f64() * 1_000_000.0 / f64::from(ITERATIONS)
    );
    println!("first_frame_ansi_bytes={ansi_bytes}");
    println!("source_bytes={}", pixels.len());
    println!("cell_frame_bytes={frame_bytes}");
    println!("estimated_retained_bytes={estimated_retained_bytes}");
    println!("render_allocation_calls={allocation_calls}");
    println!("rgb_mse_8x8={mse:.3}");
    println!("rgb_psnr_8x8_db={psnr:.3}");
    println!("glyph_usage={}", glyph_usage(&frame));
}

fn measure_experimental(
    name: &str,
    glyph_set: ExperimentalGlyphSet,
    pixels: &[u8],
    source: PixelFrameView<'_>,
    request: RenderRequest,
    renderer: &ExperimentalGlyphRenderer,
    save_directory: Option<&Path>,
) {
    let options = ExperimentalGlyphOptions {
        sampling: SamplingFilter::Nearest,
        glyph_set,
        ..ExperimentalGlyphOptions::default()
    };
    let mut frame = CellFrame::default();
    renderer
        .render_into(source, request, options, &mut frame)
        .expect("warm experimental render");
    let allocations_before = ALLOCATION_CALLS.load(Ordering::Relaxed);
    let started = Instant::now();
    for _ in 0..MATCHER_ITERATIONS {
        renderer
            .render_into(source, request, options, &mut frame)
            .expect("measured experimental render");
        black_box(frame.cells());
    }
    let elapsed = started.elapsed();
    let allocation_calls = ALLOCATION_CALLS
        .load(Ordering::Relaxed)
        .saturating_sub(allocations_before);
    let mut presenter = AnsiPresenter::new(TerminalColorMode::TrueColor);
    let ansi_bytes = present_and_save(name, &mut presenter, &frame, save_directory);
    let frame_bytes = frame.allocated_bytes();
    let estimated_retained_bytes = pixels
        .len()
        .saturating_add(frame_bytes.saturating_mul(2))
        .saturating_add(presenter.output_capacity());
    let mse = reconstruction_mse(pixels, &frame);
    let psnr = 10.0 * (255.0_f64 * 255.0 / mse).log10();

    println!("sampling={name}");
    println!("work_sampling=nearest-1to1");
    println!("iterations={MATCHER_ITERATIONS}");
    println!(
        "mean_render_us={:.3}",
        elapsed.as_secs_f64() * 1_000_000.0 / f64::from(MATCHER_ITERATIONS)
    );
    println!("first_frame_ansi_bytes={ansi_bytes}");
    println!("source_bytes={}", pixels.len());
    println!("cell_frame_bytes={frame_bytes}");
    println!("estimated_retained_bytes={estimated_retained_bytes}");
    println!("render_allocation_calls={allocation_calls}");
    println!(
        "scratch_bytes_per_cell={}",
        ExperimentalGlyphRenderer::scratch_bytes_for(glyph_set)
    );
    println!("rgb_mse_8x8={mse:.3}");
    println!("rgb_psnr_8x8_db={psnr:.3}");
    println!("glyph_usage={}", glyph_usage(&frame));
}

fn save_directory() -> Option<PathBuf> {
    let mut arguments = std::env::args().skip(1);
    match arguments.next().as_deref() {
        None => None,
        Some("--save-ansi") => Some(
            arguments
                .next()
                .map_or_else(|| PathBuf::from("target/fidelity-ansi"), PathBuf::from),
        ),
        Some(argument) => panic!("unknown argument: {argument}"),
    }
}

fn present_and_save(
    name: &str,
    presenter: &mut AnsiPresenter,
    frame: &CellFrame,
    save_directory: Option<&Path>,
) -> usize {
    let ansi = presenter.present(frame);
    if let Some(directory) = save_directory {
        let path = directory.join(format!("{name}.ansi"));
        std::fs::write(&path, ansi).expect("write ANSI output");
        println!("ansi_file={}", path.display());
    }
    ansi.len()
}

fn glyph_usage(frame: &CellFrame) -> String {
    let mut counts = std::collections::BTreeMap::new();
    for cell in frame.cells() {
        *counts
            .entry(char::from_u32(cell.glyph).unwrap_or(' '))
            .or_insert(0_usize) += 1;
    }
    counts
        .into_iter()
        .map(|(glyph, count)| format!("{glyph}:{count}"))
        .collect::<Vec<_>>()
        .join(",")
}

fn deterministic_fixture() -> Vec<u8> {
    let mut pixels = Vec::with_capacity(SOURCE_WIDTH as usize * SOURCE_HEIGHT as usize * 4);
    for y in 0..SOURCE_HEIGHT {
        for x in 0..SOURCE_WIDTH {
            let mut red = (x * 255 / (SOURCE_WIDTH - 1)) as u8;
            let mut green = (y * 255 / (SOURCE_HEIGHT - 1)) as u8;
            let mut blue = ((x + y * 3) * 255 / (SOURCE_WIDTH + SOURCE_HEIGHT * 3 - 4)) as u8;

            if y < 32 {
                let bar = x * 8 / SOURCE_WIDTH;
                red = if bar & 1 != 0 { 255 } else { 24 };
                green = if bar & 2 != 0 { 255 } else { 24 };
                blue = if bar & 4 != 0 { 255 } else { 24 };
            } else if x < 160 && y >= 96 {
                let light = ((x / 4) + (y / 4)) & 1 == 0;
                let value = if light { 240 } else { 16 };
                red = value;
                green = value;
                blue = value;
            }

            let delta_x = i64::from(x) - 430;
            let delta_y = i64::from(y) - 120;
            let radius_squared = delta_x * delta_x + delta_y * delta_y;
            if (2_400..=2_700).contains(&radius_squared) {
                red = 255;
                green = 255;
                blue = 255;
            }
            if (x + y * 2) % 97 < 2 {
                red = 255;
                green = 48;
                blue = 32;
            }
            if (x * 3 + (SOURCE_HEIGHT - 1 - y) * 5) % 211 < 3 {
                red = 24;
                green = 240;
                blue = 255;
            }

            pixels.extend_from_slice(&[red, green, blue, 255]);
        }
    }
    pixels
}

fn pattern_suite_fixture() -> Vec<u8> {
    const PANEL_WIDTH: u32 = SOURCE_WIDTH / 4;
    const PANEL_HEIGHT: u32 = SOURCE_HEIGHT / 2;
    let mut pixels = Vec::with_capacity(SOURCE_WIDTH as usize * SOURCE_HEIGHT as usize * 4);
    for y in 0..SOURCE_HEIGHT {
        for x in 0..SOURCE_WIDTH {
            let panel = x / PANEL_WIDTH + (y / PANEL_HEIGHT) * 4;
            let local_x = x % PANEL_WIDTH;
            let local_y = y % PANEL_HEIGHT;
            let value = match panel {
                0 => u8::from(local_y % 12 == 5) * 255,
                1 => u8::from(local_x % 16 == 7) * 255,
                2 => {
                    let diagonal = local_x * PANEL_HEIGHT / PANEL_WIDTH;
                    let reverse = (PANEL_WIDTH - 1 - local_x) * PANEL_HEIGHT / PANEL_WIDTH;
                    u8::from(local_y.abs_diff(diagonal) <= 1 || local_y.abs_diff(reverse) <= 1)
                        * 255
                }
                3 => {
                    let delta_x = i64::from(local_x) - i64::from(PANEL_WIDTH / 2);
                    let delta_y = i64::from(local_y) - i64::from(PANEL_HEIGHT / 2);
                    let radius_squared = delta_x * delta_x + delta_y * delta_y;
                    u8::from((1_500..=1_700).contains(&radius_squared)) * 255
                }
                4 => u8::from(((local_x / 4) + (local_y / 4)) & 1 == 0) * 255,
                5 => ((local_x * 73 + local_y * 151 + local_x * local_y * 3) & 255) as u8,
                6 => u8::from(local_x >= PANEL_WIDTH / 2) * 255,
                7 => (local_x * 255 / (PANEL_WIDTH - 1)) as u8,
                _ => unreachable!(),
            };
            let (red, green, blue) = if panel == 5 {
                (value, value.rotate_left(2), value.rotate_left(4))
            } else if panel == 7 {
                (value, value, 255 - value)
            } else {
                (value, value, value)
            };
            pixels.extend_from_slice(&[red, green, blue, 255]);
        }
    }
    pixels
}

fn reconstruction_mse(pixels: &[u8], frame: &CellFrame) -> f64 {
    let mut squared_error = 0_u64;
    for cell_y in 0..u32::from(VIEWPORT.height) {
        for cell_x in 0..u32::from(VIEWPORT.width) {
            let index = cell_y as usize * usize::from(VIEWPORT.width) + cell_x as usize;
            let cell = frame.cells()[index];
            for pixel_y in 0..8 {
                for pixel_x in 0..8 {
                    let rendered = rendered_color(cell, pixel_x, pixel_y);
                    let source_x = cell_x * 8 + pixel_x;
                    let source_y = cell_y * 8 + pixel_y;
                    let source_index =
                        (source_y as usize * SOURCE_WIDTH as usize + source_x as usize) * 4;
                    for (actual, expected) in [rendered.red, rendered.green, rendered.blue]
                        .into_iter()
                        .zip(&pixels[source_index..source_index + 3])
                    {
                        let delta = i64::from(actual) - i64::from(*expected);
                        squared_error += (delta * delta) as u64;
                    }
                }
            }
        }
    }
    squared_error as f64 / f64::from(SOURCE_WIDTH * SOURCE_HEIGHT * 3)
}

fn rendered_color(cell: Cell, pixel_x: u32, pixel_y: u32) -> Color {
    let glyph = char::from_u32(cell.glyph).unwrap_or(' ');
    let coverage = glyph_coverage(glyph).unwrap_or(u64::MAX);
    let index = pixel_y * 8 + pixel_x;
    if coverage & (1_u64 << index) != 0 {
        cell.foreground
    } else {
        cell.background
    }
}
