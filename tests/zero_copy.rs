//! M2a contract: a borrowed frame that already has the encoder's exact size
//! is encoded in place. No pixel copy, no allocation, whatever its layout.
//!
//! A counting global allocator records allocations per thread, so tests in
//! this binary can run in parallel without disturbing each other.

use std::alloc::{GlobalAlloc, Layout, System};
use std::cell::Cell as StdCell;

use trend::{
    AlphaMode, AnsiPresenter, Braille, Cell, CellAspect, CellEncoder, CellFrame, CellRect, Density,
    DensityRamp, FitMode, HalfBlock, ImageSource, OutputOptions, PixelBuffer, PixelFormat,
    PixelFrameView, PixelLayout, PixelRect, Quadrant, RasterSpec, RenderOptions, RenderRequest,
    RenderStatus, Renderer, RowOrder, SamplingFilter, Sextant, Surface, TerminalCapabilities,
    TerminalColorMode, Viewport,
};

thread_local! {
    static ALLOCATIONS: StdCell<usize> = const { StdCell::new(0) };
}

struct CountingAllocator;

// SAFETY: forwards every call to the system allocator unchanged.
unsafe impl GlobalAlloc for CountingAllocator {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        ALLOCATIONS.with(|count| count.set(count.get() + 1));
        unsafe { System.alloc(layout) }
    }

    unsafe fn dealloc(&self, pointer: *mut u8, layout: Layout) {
        unsafe { System.dealloc(pointer, layout) }
    }

    unsafe fn realloc(&self, pointer: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        ALLOCATIONS.with(|count| count.set(count.get() + 1));
        unsafe { System.realloc(pointer, layout, new_size) }
    }
}

#[global_allocator]
static ALLOCATOR: CountingAllocator = CountingAllocator;

fn allocations_during(work: impl FnOnce()) -> usize {
    let before = ALLOCATIONS.with(StdCell::get);
    work();
    ALLOCATIONS.with(StdCell::get) - before
}

/// Stand-in for mapped GPU memory: BGRA, bottom-up, premultiplied, with rows
/// padded to 256 bytes like a wgpu or D3D12 readback.
fn readback(width: u32, height: u32) -> (Vec<u8>, PixelLayout) {
    let stride = (width as usize * 4).next_multiple_of(256);
    let mut bytes = vec![0_u8; stride * height as usize];
    for (index, byte) in bytes.iter_mut().enumerate() {
        *byte = (index * 37 % 251) as u8;
    }
    let layout = PixelLayout::new(width, height, PixelFormat::Bgra8, AlphaMode::Premultiplied)
        .with_stride(stride)
        .with_row_order(RowOrder::BottomUp);
    (bytes, layout)
}

#[test]
fn exact_size_readbacks_encode_without_allocating() {
    let viewport = Viewport {
        width: 120,
        height: 40,
    };
    for encoder in [
        &HalfBlock as &dyn CellEncoder,
        &Quadrant,
        &Sextant,
        &Braille,
        &Density::ASCII,
        &Density::new(DensityRamp::Shades, true),
    ] {
        let spec = RasterSpec::new(viewport, encoder.grid(), CellAspect::default()).unwrap();
        let (bytes, layout) = readback(spec.width(), spec.height());
        let mut cells = CellFrame::default();
        cells.resize(viewport.width, viewport.height).unwrap();

        let allocations = allocations_during(|| {
            let frame = PixelFrameView::from_layout(&bytes, layout).unwrap();
            assert!(spec.matches(&frame));
            encoder.encode(frame, cells.view_mut()).unwrap();
        });

        assert_eq!(allocations, 0);
        // Control: the counter does see allocations on this thread.
        assert!(allocations_during(|| drop(std::hint::black_box(vec![0_u8; 16]))) > 0);
        assert!(
            cells
                .cells()
                .iter()
                .any(|cell| *cell != trend::Cell::default())
        );
    }
}

#[test]
fn region_encoding_from_a_readback_does_not_allocate() {
    let viewport = Viewport {
        width: 64,
        height: 20,
    };
    let spec = RasterSpec::new(viewport, HalfBlock.grid(), CellAspect::default()).unwrap();
    let (bytes, layout) = readback(spec.width(), spec.height());
    let frame = PixelFrameView::from_layout(&bytes, layout).unwrap();
    let mut cells = CellFrame::default();
    cells.resize(viewport.width, viewport.height).unwrap();

    let allocations = allocations_during(|| {
        let dirty = CellRect::new(10, 3, 20, 7);
        let pixels = frame.sub(spec.pixel_rect(dirty).unwrap()).unwrap();
        let mut screen = cells.view_mut();
        HalfBlock
            .encode(pixels, screen.sub(dirty).unwrap())
            .unwrap();
        for (first_row, band) in cells.view_mut().into_row_bands(6) {
            let rect = CellRect::new(0, first_row, band.width(), band.height());
            let pixels = frame.sub(spec.pixel_rect(rect).unwrap()).unwrap();
            HalfBlock.encode(pixels, band).unwrap();
        }
    });

    assert_eq!(allocations, 0);
    // The sub-view points into the caller's memory; nothing was copied.
    let sub = frame
        .sub(spec.pixel_rect(CellRect::new(1, 1, 1, 1)).unwrap())
        .unwrap();
    assert!(bytes.as_ptr_range().contains(&sub.pixels().as_ptr()));
}

/// The legacy renderer keeps its allocation profile after becoming a facade
/// over the new geometry and sampling: rendering into caller-owned cells, or
/// into a `CellFrame` that is already large enough, allocates nothing.
#[test]
fn legacy_renderer_stays_allocation_free() {
    let (bytes, layout) = readback(333, 187);
    let frame = PixelFrameView::from_layout(&bytes, layout).unwrap();
    let request = RenderRequest {
        viewport: Viewport {
            width: 80,
            height: 24,
        },
        fit: FitMode::Cover,
        generation: 3,
    };
    let renderer = Renderer::new();
    let mut output = vec![Cell::default(); 80 * 24];
    let mut cells = CellFrame::default();
    cells.resize(80, 24).unwrap();

    for sampling in [
        SamplingFilter::Nearest,
        SamplingFilter::Bilinear,
        SamplingFilter::Area,
    ] {
        let options = RenderOptions {
            sampling,
            dither_strength: 12,
            ..RenderOptions::default()
        };
        let allocations = allocations_during(|| {
            renderer
                .render_to_slice_with_options(frame, request, options, &mut output)
                .unwrap();
            let status = renderer
                .render_to_slice_cancellable(frame, request, options, &mut output, || false)
                .unwrap();
            assert_eq!(status, RenderStatus::Complete);
            renderer
                .render_with_options_into(frame, request, options, &mut cells)
                .unwrap();
        });
        assert_eq!(allocations, 0, "{sampling:?}");
    }
}

/// Rendering a source into row bands of an existing buffer allocates nothing.
#[test]
fn banded_image_source_rendering_does_not_allocate() {
    let (bytes, layout) = readback(333, 187);
    let frame = PixelFrameView::from_layout(&bytes, layout).unwrap();
    let spec = RasterSpec::new(
        Viewport {
            width: 90,
            height: 30,
        },
        Quadrant.grid(),
        CellAspect::default(),
    )
    .unwrap();
    let source = ImageSource::new(frame).with_sampling(SamplingFilter::Area);
    let mut pixels = PixelBuffer::new(spec.width(), spec.height()).unwrap();

    let allocations = allocations_during(|| {
        for (first_row, band) in pixels.view_mut().into_row_bands(8) {
            source.render_region(&spec, 0, first_row, band).unwrap();
        }
    });
    assert_eq!(allocations, 0);
}

/// The presenter reuses its byte buffer: once it has grown to a frame's
/// size, presenting further frames of that size allocates nothing, in every
/// color mode.
#[test]
fn presenting_same_size_frames_does_not_allocate() {
    let (bytes, layout) = readback(160, 96);
    let frame = PixelFrameView::from_layout(&bytes, layout).unwrap();
    let viewport = Viewport {
        width: 160,
        height: 48,
    };
    let mut frames = Vec::new();
    for shift in 0..3_i32 {
        let mut cells = CellFrame::default();
        let request = RenderRequest {
            viewport,
            fit: FitMode::Cover,
            generation: 0,
        };
        let options = RenderOptions {
            transform: trend::RenderTransform {
                zoom_percent: 150,
                pan_x_cells: shift * 3,
                ..trend::RenderTransform::default()
            },
            ..RenderOptions::default()
        };
        Renderer::new()
            .render_with_options_into(frame, request, options, &mut cells)
            .unwrap();
        frames.push(cells);
    }
    for color in [
        TerminalColorMode::TrueColor,
        TerminalColorMode::Ansi256,
        TerminalColorMode::Ansi16,
        TerminalColorMode::Monochrome,
    ] {
        let mut presenter = AnsiPresenter::with_capabilities(TerminalCapabilities {
            color,
            unicode: true,
        });
        // Warm up: the first frames size the output and previous-frame buffers.
        for cells in &frames {
            presenter.present(cells);
        }
        let allocations = allocations_during(|| {
            for cells in frames.iter().chain(&frames) {
                std::hint::black_box(presenter.present(cells).len());
            }
        });
        assert_eq!(allocations, 0, "{color:?}");
    }
}

/// M4: once warmed up, a `Surface` draws, encodes (every way), and presents
/// (whole or by region, with every output option) without allocating.
#[test]
fn surface_steady_state_does_not_allocate() {
    let viewport = Viewport {
        width: 120,
        height: 40,
    };
    let (bytes, _) = readback(240, 80);
    let image_layout = PixelLayout::new(240, 80, PixelFormat::Bgra8, AlphaMode::Premultiplied)
        .with_stride((240_usize * 4).next_multiple_of(256))
        .with_row_order(RowOrder::BottomUp);
    let image = PixelFrameView::from_layout(&bytes, image_layout).unwrap();
    let options = OutputOptions::new()
        .with_combined_colors(true)
        .with_synchronized_updates(true);

    for encoder in [
        &HalfBlock as &dyn CellEncoder,
        &Quadrant,
        &Sextant,
        &Braille,
        &Density::ASCII,
    ] {
        let mut surface = Surface::new(
            encoder,
            viewport,
            TerminalCapabilities {
                color: TerminalColorMode::TrueColor,
                unicode: true,
            },
        )
        .unwrap();
        surface.set_output_options(options);
        let spec = *surface.spec();
        let (readback_bytes, readback_layout) = readback(spec.width(), spec.height());
        let exact = PixelFrameView::from_layout(&readback_bytes, readback_layout).unwrap();
        let mut sink = std::io::sink();

        let mut frame = |surface: &mut Surface<&dyn CellEncoder>, t: u8| {
            surface
                .draw(|mut pixels, spec| {
                    for y in 0..spec.height() {
                        for (x, pixel) in pixels.row_mut(y).chunks_exact_mut(4).enumerate() {
                            pixel.copy_from_slice(&[x as u8 ^ t, y as u8, t, 255]);
                        }
                    }
                })
                .unwrap();
            surface.present(&mut sink).unwrap();
            surface.encode_from(exact).unwrap();
            surface.present(&mut sink).unwrap();
            let dirty = PixelRect::new(u32::from(t), 2, 13, 5);
            let cells = surface.encode_from_region(exact, dirty).unwrap().unwrap();
            surface.present_region(cells, &mut sink).unwrap();
            let source = ImageSource::new(image).with_sampling(SamplingFilter::Bilinear);
            surface.encode_image(&source).unwrap();
            surface.present(&mut sink).unwrap();
            let damage = PixelRect::new(u32::from(t) * 3, 5, 17, 9);
            let cells = surface
                .encode_image_region(&source, damage)
                .unwrap()
                .unwrap();
            surface.present_region(cells, &mut sink).unwrap();
            surface.pixels_mut().row_mut(3)[..8].fill(t);
            let cells = surface
                .encode_region(PixelRect::new(0, 3, 2, 1))
                .unwrap()
                .unwrap();
            surface.present_region(cells, &mut sink).unwrap();
            let mapping = *surface.source_mapping().unwrap();
            std::hint::black_box(mapping.cell_to_source(4, 4));
            std::hint::black_box(mapping.source_damage(PixelRect::new(1, 1, 3, 3)));
        };
        // Warm up: the first frames size the presenter's buffers.
        for t in 0..3 {
            frame(&mut surface, t);
        }
        let allocations = allocations_during(|| {
            for t in 3..8 {
                frame(&mut surface, t);
            }
        });
        assert_eq!(allocations, 0, "{:?}", encoder.grid());
    }
}

/// M5: overlays allocate nothing: pixel blending, text, panel fills, and
/// presenting attributes.
#[test]
fn overlays_do_not_allocate() {
    use trend::compose::blend_over;
    use trend::text::{TextBackground, TextStyle, put_str};

    let viewport = Viewport {
        width: 80,
        height: 24,
    };
    let mut surface = Surface::new(
        Sextant,
        viewport,
        TerminalCapabilities {
            color: TerminalColorMode::TrueColor,
            unicode: true,
        },
    )
    .unwrap();
    let mut sprite = PixelBuffer::new(9, 7).unwrap();
    for (index, pixel) in sprite.pixels_mut().chunks_exact_mut(4).enumerate() {
        pixel.copy_from_slice(&[255, index as u8, 0, (index * 13) as u8]);
    }
    let (readback_bytes, readback_layout) = readback(9, 7);
    let readback = PixelFrameView::from_layout(&readback_bytes, readback_layout).unwrap();
    let mut sink = std::io::sink();
    let mut frame = |surface: &mut Surface<Sextant>, t: i32| {
        surface
            .draw(|mut pixels, _| {
                for y in 0..pixels.height() {
                    pixels.row_mut(y).fill((y as i32 + t) as u8);
                }
            })
            .unwrap();
        let damage = blend_over(sprite.view(), &mut surface.pixels_mut(), t * 3 - 4, 5).unwrap();
        surface.encode_region(damage).unwrap();
        blend_over(readback, &mut surface.pixels_mut(), 20, t).unwrap();
        surface.encode().unwrap();
        let mut cells = surface.cells_mut();
        let mut panel = cells.sub(CellRect::new(50, 1, 20, 3)).unwrap();
        panel.fill(trend::Cell::default());
        let style = TextStyle::new(trend::Color::rgb(255, 255, 0))
            .with_background(TextBackground::Keep)
            .with_attributes(trend::Cell::BOLD | trend::Cell::UNDERLINE);
        put_str(&mut panel, 1, 1, "frame ✓ 漢", style);
        put_str(&mut cells, 0, 0, "status: ok", TextStyle::default());
        surface.present(&mut sink).unwrap();
    };
    for t in 0..3 {
        frame(&mut surface, t);
    }
    let allocations = allocations_during(|| {
        for t in 3..8 {
            frame(&mut surface, t);
        }
    });
    assert_eq!(allocations, 0);
}
