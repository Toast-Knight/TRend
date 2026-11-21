//! M2b contracts for `ImageSource::render_region`: any rectangle of the
//! spec's surface, rendered into any writable view, is identical to the same
//! pixels of a whole-surface `render_into`. That makes row bands, cancellation
//! between bands, host threads, dirty rectangles, and rendering straight into
//! a region of a larger buffer all equivalent to rendering the whole frame.

mod common;

use trend::{
    CellAspect, CellEncoder, CellFrame, CellRect, FitMode, GeometryError, HalfBlock, ImageSource,
    PixelBuffer, PixelRect, Quadrant, RasterSpec, RegionError, RenderError, RenderTransform,
    Viewport,
};

const VIEWPORT: Viewport = Viewport {
    width: 17,
    height: 9,
};

/// Every source configuration exercised below: 5 patterns × 3 filters ×
/// 2 fits × 4 transforms × 2 mattes.
fn for_each_source(mut check: impl FnMut(&str, ImageSource<'_>)) {
    for pattern in common::patterns() {
        for filter in common::FILTERS {
            for fit in [FitMode::Contain, FitMode::Cover] {
                for transform in common::transforms() {
                    for matte in common::MATTES {
                        let source = ImageSource::new(pattern.view())
                            .with_fit(fit)
                            .with_sampling(filter)
                            .with_transform(transform)
                            .with_alpha_background(matte);
                        check(pattern.name, source);
                    }
                }
            }
        }
    }
}

fn whole(source: &ImageSource<'_>, spec: &RasterSpec) -> PixelBuffer {
    let mut buffer = PixelBuffer::new(1, 1).unwrap();
    source.render_into(spec, &mut buffer).unwrap();
    buffer
}

#[test]
fn row_bands_reproduce_the_whole_surface() {
    let spec = RasterSpec::new(VIEWPORT, Quadrant.grid(), CellAspect::default()).unwrap();
    for_each_source(|name, source| {
        let expected = whole(&source, &spec);
        for rows_per_band in [1, 4, 7, 18, 100] {
            let mut banded = PixelBuffer::new(spec.width(), spec.height()).unwrap();
            for (first_row, band) in banded.view_mut().into_row_bands(rows_per_band) {
                source.render_region(&spec, 0, first_row, band).unwrap();
            }
            assert_eq!(banded, expected, "{name} bands of {rows_per_band}");
        }
    });
}

#[test]
fn bands_can_render_on_host_threads() {
    let spec = RasterSpec::new(VIEWPORT, HalfBlock.grid(), CellAspect::default()).unwrap();
    for_each_source(|name, source| {
        let mut parallel = PixelBuffer::new(spec.width(), spec.height()).unwrap();
        std::thread::scope(|scope| {
            for (first_row, band) in parallel.view_mut().into_row_bands(5) {
                let spec = &spec;
                scope.spawn(move || source.render_region(spec, 0, first_row, band).unwrap());
            }
        });
        assert_eq!(parallel, whole(&source, &spec), "{name}");
    });
}

#[test]
fn arbitrary_rectangles_match_the_same_pixels_of_the_whole_surface() {
    let spec = RasterSpec::new(VIEWPORT, Quadrant.grid(), CellAspect::default()).unwrap();
    let rects = [
        PixelRect::new(0, 0, spec.width(), spec.height()),
        PixelRect::new(5, 3, 9, 7),
        PixelRect::new(spec.width() - 1, spec.height() - 1, 1, 1),
        PixelRect::new(20, 0, 3, spec.height()),
    ];
    for_each_source(|name, source| {
        let expected = whole(&source, &spec);
        for rect in rects {
            // Render into the middle of a larger, pre-filled buffer.
            let mut canvas = PixelBuffer::new(rect.width + 4, rect.height + 3).unwrap();
            canvas.pixels_mut().fill(7);
            let target = PixelRect::new(2, 1, rect.width, rect.height);
            {
                let mut view = canvas.view_mut();
                source
                    .render_region(&spec, rect.x, rect.y, view.sub(target).unwrap())
                    .unwrap();
            }
            let region = expected.view().sub(rect).unwrap();
            for y in 0..canvas.height() {
                for x in 0..canvas.width() {
                    let inside =
                        (2..2 + rect.width).contains(&x) && (1..1 + rect.height).contains(&y);
                    let pixel = canvas.view().rgba(x, y).unwrap();
                    if inside {
                        assert_eq!(Some(pixel), region.rgba(x - 2, y - 1), "{name} {rect:?}");
                    } else {
                        assert_eq!(pixel, [7; 4], "{name} {rect:?} outside at ({x},{y})");
                    }
                }
            }
        }
    });
}

#[test]
fn cancelling_between_bands_leaves_later_bands_untouched() {
    let patterns = common::patterns();
    let spec = RasterSpec::new(VIEWPORT, HalfBlock.grid(), CellAspect::default()).unwrap();
    let source = ImageSource::new(patterns[0].view()).with_fit(FitMode::Stretch);
    let expected = whole(&source, &spec);

    let mut buffer = PixelBuffer::new(spec.width(), spec.height()).unwrap();
    let mut checks = 0;
    let mut cancelled = || {
        checks += 1;
        checks > 2
    };
    for (first_row, band) in buffer.view_mut().into_row_bands(4) {
        if cancelled() {
            break;
        }
        source.render_region(&spec, 0, first_row, band).unwrap();
    }

    let done = PixelRect::new(0, 0, spec.width(), 8);
    let rest = PixelRect::new(0, 8, spec.width(), spec.height() - 8);
    for y in 0..8 {
        assert_eq!(
            buffer.view().row(y),
            expected.view().sub(done).unwrap().row(y)
        );
    }
    let untouched = buffer.view().sub(rest).unwrap();
    for y in 0..rest.height {
        assert!(untouched.row(y).unwrap().iter().all(|&byte| byte == 0));
    }
}

#[test]
fn a_region_of_a_shared_buffer_can_hold_its_own_source() {
    // Two panes of one pixel canvas, each with its own spec and source, then
    // encoded into the matching regions of one cell frame.
    let patterns = common::patterns();
    let left_cells = CellRect::new(0, 0, 10, 6);
    let right_cells = CellRect::new(10, 0, 8, 6);
    let pane = |rect: CellRect| Viewport {
        width: rect.width,
        height: rect.height,
    };
    let screen_spec = RasterSpec::new(
        Viewport {
            width: 18,
            height: 6,
        },
        HalfBlock.grid(),
        CellAspect::default(),
    )
    .unwrap();
    let left_spec =
        RasterSpec::new(pane(left_cells), HalfBlock.grid(), CellAspect::default()).unwrap();
    let right_spec =
        RasterSpec::new(pane(right_cells), HalfBlock.grid(), CellAspect::default()).unwrap();
    let left = ImageSource::new(patterns[0].view());
    let right = ImageSource::new(patterns[4].view()).with_fit(FitMode::Cover);

    let mut canvas = PixelBuffer::new(screen_spec.width(), screen_spec.height()).unwrap();
    {
        let mut view = canvas.view_mut();
        let left_rect = screen_spec.pixel_rect(left_cells).unwrap();
        let right_rect = screen_spec.pixel_rect(right_cells).unwrap();
        left.render_region(&left_spec, 0, 0, view.sub(left_rect).unwrap())
            .unwrap();
        right
            .render_region(&right_spec, 0, 0, view.sub(right_rect).unwrap())
            .unwrap();
    }
    let mut screen = CellFrame::default();
    screen.resize(18, 6).unwrap();
    HalfBlock.encode(canvas.view(), screen.view_mut()).unwrap();

    let expect_pane = |source: &ImageSource<'_>, spec: &RasterSpec, rect: CellRect| {
        let mut cells = CellFrame::default();
        cells.resize(rect.width, rect.height).unwrap();
        HalfBlock
            .encode(whole(source, spec).view(), cells.view_mut())
            .unwrap();
        for y in 0..rect.height {
            assert_eq!(screen.view().sub(rect).unwrap().row(y), cells.view().row(y));
        }
    };
    expect_pane(&left, &left_spec, left_cells);
    expect_pane(&right, &right_spec, right_cells);
}

#[test]
fn regions_outside_the_surface_are_rejected() {
    let patterns = common::patterns();
    let spec = RasterSpec::new(VIEWPORT, HalfBlock.grid(), CellAspect::default()).unwrap();
    let source = ImageSource::new(patterns[0].view());
    let mut buffer = PixelBuffer::new(spec.width(), 2).unwrap();

    assert_eq!(
        source.render_region(&spec, 0, spec.height() - 1, buffer.view_mut()),
        Err(RenderError::Region(RegionError::OutOfBounds))
    );
    assert_eq!(
        source.render_region(&spec, 1, 0, buffer.view_mut()),
        Err(RenderError::Region(RegionError::OutOfBounds))
    );
    assert_eq!(
        source.render_region(&spec, u32::MAX, 0, buffer.view_mut()),
        Err(RenderError::Region(RegionError::OutOfBounds))
    );
    // Geometry errors are reported before region errors, as in render_into.
    let zero_zoom = source.with_transform(RenderTransform {
        zoom_percent: 0,
        ..RenderTransform::default()
    });
    assert_eq!(
        zero_zoom.render_region(&spec, u32::MAX, 0, buffer.view_mut()),
        Err(RenderError::Geometry(GeometryError::ZeroZoom))
    );
    // A rejected call leaves the target untouched.
    assert!(buffer.pixels().iter().all(|&byte| byte == 0));
}
