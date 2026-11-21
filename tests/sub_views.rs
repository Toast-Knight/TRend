//! M2a contracts for sub-views: pixel and cell regions, row bands, multiple
//! regions on one terminal, row-band and parallel encoding, dirty
//! rectangles, and banded source work with cancellation.

use trend::{
    AnsiPresenter, Cell, CellAspect, CellEncoder, CellFrame, CellRect, CellView, CellViewMut,
    Color, FrameError, HalfBlock, PixelBuffer, PixelFrameView, PixelRect, PixelViewMut, Quadrant,
    RasterSpec, RegionError, TerminalColorMode, Viewport,
};

fn assert_send<T: Send>() {}
fn assert_sync<T: Sync>() {}

#[test]
fn views_and_encoders_can_cross_threads() {
    assert_send::<PixelViewMut<'static>>();
    assert_send::<CellViewMut<'static>>();
    assert_send::<PixelFrameView<'static>>();
    assert_sync::<PixelFrameView<'static>>();
    assert_sync::<HalfBlock>();
    assert_sync::<Quadrant>();
}

/// An opaque test image whose pixels encode their own coordinates.
fn coordinate_image(width: u32, height: u32) -> PixelBuffer {
    let mut buffer = PixelBuffer::new(width, height).unwrap();
    let mut view = buffer.view_mut();
    for y in 0..height {
        for (x, pixel) in view.row_mut(y).chunks_exact_mut(4).enumerate() {
            pixel.copy_from_slice(&[
                (x as u32 * 11) as u8,
                (y * 7) as u8,
                ((x as u32) ^ y) as u8,
                255,
            ]);
        }
    }
    buffer
}

fn encoded(encoder: &dyn CellEncoder, pixels: PixelFrameView<'_>, viewport: Viewport) -> CellFrame {
    let mut cells = CellFrame::default();
    cells.resize(viewport.width, viewport.height).unwrap();
    encoder.encode(pixels, cells.view_mut()).unwrap();
    cells
}

// ---------------------------------------------------------------------------
// Pixel views
// ---------------------------------------------------------------------------

#[test]
fn pixel_view_mut_wraps_host_memory_with_stride() {
    let mut memory = vec![0_u8; 3 * 20 + 8];
    assert_eq!(
        PixelViewMut::new(&mut memory, 3, 4, 11).unwrap_err(),
        FrameError::StrideTooSmall
    );
    assert_eq!(
        PixelViewMut::new(&mut memory[..67], 2, 4, 20).unwrap_err(),
        FrameError::BufferTooSmall
    );

    let mut view = PixelViewMut::new(&mut memory, 2, 4, 20).unwrap();
    view.row_mut(3).copy_from_slice(&[1, 2, 3, 4, 5, 6, 7, 8]);
    assert_eq!(view.as_view().rgba(1, 3), Some([5, 6, 7, 8]));
    assert_eq!(&memory[60..68], &[1, 2, 3, 4, 5, 6, 7, 8]);
}

#[test]
fn pixel_sub_views_write_into_the_parent() {
    let mut buffer = PixelBuffer::new(6, 5).unwrap();
    let mut view = buffer.view_mut();
    {
        let mut region = view.sub(PixelRect::new(2, 1, 3, 3)).unwrap();
        let mut nested = region.sub(PixelRect::new(1, 2, 2, 1)).unwrap();
        nested.row_mut(0).copy_from_slice(&[9; 8]);
        assert_eq!(region.stride(), 24);
        assert_eq!(
            region.sub(PixelRect::new(2, 0, 2, 1)).unwrap_err(),
            RegionError::OutOfBounds
        );
    }
    // Nested (1, 2) inside (2, 1) is (3, 3) and (4, 3) in the parent.
    for y in 0..5 {
        for x in 0..6 {
            let expected = if y == 3 && (x == 3 || x == 4) {
                [9; 4]
            } else {
                [0; 4]
            };
            assert_eq!(buffer.view().rgba(x, y), Some(expected), "({x},{y})");
        }
    }
}

#[test]
fn pixel_row_bands_cover_every_row_exactly_once() {
    let mut buffer = PixelBuffer::new(3, 10).unwrap();
    let bands: Vec<_> = buffer
        .view_mut()
        .into_row_bands(3)
        .map(|(first_row, mut band)| {
            let height = band.height();
            for y in 0..height {
                band.row_mut(y).fill(first_row as u8 + 1);
            }
            (first_row, height)
        })
        .collect();

    assert_eq!(bands, [(0, 3), (3, 3), (6, 3), (9, 1)]);
    for y in 0..10 {
        assert_eq!(buffer.view().row(y).unwrap(), &[(y / 3 * 3) as u8 + 1; 12]);
    }
    assert_eq!(buffer.view_mut().into_row_bands(50).count(), 1);
}

#[test]
#[should_panic(expected = "rows_per_band must be non-zero")]
fn pixel_row_bands_reject_zero_height() {
    let mut buffer = PixelBuffer::new(1, 1).unwrap();
    let _ = buffer.view_mut().into_row_bands(0);
}

#[test]
fn source_work_can_be_parallel_or_cancelled_between_bands() {
    let draw = |first_row: u32, mut band: PixelViewMut<'_>| {
        for y in 0..band.height() {
            let row = first_row + y;
            for (x, pixel) in band.row_mut(y).chunks_exact_mut(4).enumerate() {
                pixel.copy_from_slice(&[x as u8, row as u8, 1, 255]);
            }
        }
    };

    let mut sequential = PixelBuffer::new(16, 37).unwrap();
    for (first_row, band) in sequential.view_mut().into_row_bands(5) {
        draw(first_row, band);
    }

    let mut parallel = PixelBuffer::new(16, 37).unwrap();
    std::thread::scope(|scope| {
        for (first_row, band) in parallel.view_mut().into_row_bands(5) {
            scope.spawn(move || draw(first_row, band));
        }
    });
    assert_eq!(parallel, sequential);

    // Cancellation of expensive source work: the host checks its token
    // between bands; here it fires after two bands.
    let mut cancelled = PixelBuffer::new(16, 37).unwrap();
    let mut checks = 0;
    let mut should_cancel = || {
        checks += 1;
        checks > 2
    };
    for (first_row, band) in cancelled.view_mut().into_row_bands(5) {
        if should_cancel() {
            break;
        }
        draw(first_row, band);
    }
    let view = cancelled.view();
    assert_eq!(view.rgba(3, 9), Some([3, 9, 1, 255]));
    assert_eq!(view.rgba(3, 10), Some([0, 0, 0, 0]));
}

// ---------------------------------------------------------------------------
// Cell views
// ---------------------------------------------------------------------------

#[test]
fn cell_sub_views_write_into_the_parent() {
    let marker = Cell::with_glyph('x', Color::rgb(1, 2, 3), Color::DEFAULT);
    let mut frame = CellFrame::default();
    frame.resize(8, 4).unwrap();
    {
        let mut screen = frame.view_mut();
        let mut pane = screen.sub(CellRect::new(3, 1, 4, 3)).unwrap();
        let mut nested = pane.sub(CellRect::new(1, 1, 2, 2)).unwrap();
        nested.row_mut(1)[1] = marker;
        assert_eq!(nested.as_view().row(1)[1], marker);
        assert_eq!(
            pane.sub(CellRect::new(0, 0, 5, 1)).unwrap_err(),
            RegionError::OutOfBounds
        );
        assert_eq!(
            pane.sub(CellRect::new(0, 0, 1, 0)).unwrap_err(),
            RegionError::Empty
        );
    }
    // (1, 1) + (1, 1) inside (3, 1) is (5, 3).
    assert_eq!(frame.cells()[3 * 8 + 5], marker);
    assert_eq!(
        frame.cells().iter().filter(|cell| **cell == marker).count(),
        1
    );

    let view: CellView<'_> = frame.view();
    let region = view.sub(CellRect::new(4, 2, 3, 2)).unwrap();
    assert_eq!(region.row(1)[1], marker);
}

#[test]
fn cell_row_bands_cover_every_row_and_tolerate_empty_views() {
    let mut frame = CellFrame::default();
    frame.resize(4, 7).unwrap();
    let bands: Vec<_> = frame
        .view_mut()
        .into_row_bands(2)
        .map(|(first_row, band)| (first_row, band.height()))
        .collect();
    assert_eq!(bands, [(0, 2), (2, 2), (4, 2), (6, 1)]);

    let mut empty = CellFrame::default();
    assert_eq!(empty.view_mut().into_row_bands(3).count(), 0);
}

// ---------------------------------------------------------------------------
// Regions, bands, and dirty rectangles with real encoders
// ---------------------------------------------------------------------------

#[test]
fn several_regions_with_different_encoders_share_one_terminal_frame() {
    let screen_size = Viewport {
        width: 40,
        height: 10,
    };
    let left_rect = CellRect::new(0, 0, 18, 10);
    let right_rect = CellRect::new(20, 2, 20, 6);
    let pane = |rect: CellRect| Viewport {
        width: rect.width,
        height: rect.height,
    };

    let left_spec =
        RasterSpec::new(pane(left_rect), HalfBlock.grid(), CellAspect::default()).unwrap();
    let right_spec =
        RasterSpec::new(pane(right_rect), Quadrant.grid(), CellAspect::default()).unwrap();
    let left_pixels = coordinate_image(left_spec.width(), left_spec.height());
    let right_pixels = coordinate_image(right_spec.width(), right_spec.height());

    let mut screen = CellFrame::default();
    screen
        .resize(screen_size.width, screen_size.height)
        .unwrap();
    {
        let mut view = screen.view_mut();
        HalfBlock
            .encode(left_pixels.view(), view.sub(left_rect).unwrap())
            .unwrap();
        Quadrant
            .encode(right_pixels.view(), view.sub(right_rect).unwrap())
            .unwrap();
    }

    let left_alone = encoded(&HalfBlock, left_pixels.view(), pane(left_rect));
    let right_alone = encoded(&Quadrant, right_pixels.view(), pane(right_rect));
    for y in 0..screen_size.height {
        for x in 0..screen_size.width {
            let cell = screen.cells()[usize::from(y) * 40 + usize::from(x)];
            let inside = |rect: CellRect| {
                (rect.x..rect.x + rect.width).contains(&x)
                    && (rect.y..rect.y + rect.height).contains(&y)
            };
            let expected = if inside(left_rect) {
                left_alone.view().row(y - left_rect.y)[usize::from(x - left_rect.x)]
            } else if inside(right_rect) {
                right_alone.view().row(y - right_rect.y)[usize::from(x - right_rect.x)]
            } else {
                Cell::default()
            };
            assert_eq!(cell, expected, "({x},{y})");
        }
    }

    // One presenter draws the whole screen.
    let mut presenter = AnsiPresenter::new(TerminalColorMode::TrueColor);
    assert!(!presenter.present(&screen).is_empty());
    assert!(presenter.present(&screen).is_empty());
}

#[test]
fn row_band_encoding_matches_whole_frame_encoding() {
    let viewport = Viewport {
        width: 31,
        height: 13,
    };
    for encoder in [&HalfBlock as &(dyn CellEncoder + Sync), &Quadrant] {
        let spec = RasterSpec::new(viewport, encoder.grid(), CellAspect::default()).unwrap();
        let pixels = coordinate_image(spec.width(), spec.height());
        let whole = encoded(encoder, pixels.view(), viewport);

        for rows_per_band in [1, 3, 5, 13, 100] {
            let mut banded = CellFrame::default();
            banded.resize(viewport.width, viewport.height).unwrap();
            for (first_row, band) in banded.view_mut().into_row_bands(rows_per_band) {
                let rect = CellRect::new(0, first_row, band.width(), band.height());
                let source = pixels.view().sub(spec.pixel_rect(rect).unwrap()).unwrap();
                encoder.encode(source, band).unwrap();
            }
            assert_eq!(banded, whole, "bands of {rows_per_band}");
        }

        // The same bands encoded on separate threads.
        let mut parallel = CellFrame::default();
        parallel.resize(viewport.width, viewport.height).unwrap();
        let frame = pixels.view();
        std::thread::scope(|scope| {
            for (first_row, band) in parallel.view_mut().into_row_bands(4) {
                let rect = CellRect::new(0, first_row, band.width(), band.height());
                let source = frame.sub(spec.pixel_rect(rect).unwrap()).unwrap();
                scope.spawn(move || encoder.encode(source, band).unwrap());
            }
        });
        assert_eq!(parallel, whole);
    }
}

#[test]
fn dirty_rectangles_reencode_only_the_covering_cells() {
    let viewport = Viewport {
        width: 20,
        height: 8,
    };
    let spec = RasterSpec::new(viewport, Quadrant.grid(), CellAspect::default()).unwrap();
    let mut pixels = coordinate_image(spec.width(), spec.height());
    let mut cells = encoded(&Quadrant, pixels.view(), viewport);

    // The application reports a damaged, unaligned pixel rectangle.
    let damage = PixelRect::new(3, 5, 4, 3);
    {
        let mut view = pixels.view_mut();
        let mut region = view.sub(damage).unwrap();
        for y in 0..damage.height {
            region.row_mut(y).fill(200);
        }
    }
    let covering = spec.covering_cells(damage).unwrap();
    assert_eq!(covering, CellRect::new(1, 2, 3, 2));
    let pixel_region = spec.pixel_rect(covering).unwrap();
    assert_eq!(pixel_region, PixelRect::new(2, 4, 6, 4));

    let before = cells.clone();
    Quadrant
        .encode(
            pixels.view().sub(pixel_region).unwrap(),
            cells.view_mut().sub(covering).unwrap(),
        )
        .unwrap();

    assert_eq!(cells, encoded(&Quadrant, pixels.view(), viewport));
    let changed = cells
        .cells()
        .iter()
        .zip(before.cells())
        .filter(|(after, before)| after != before)
        .count();
    assert!(changed > 0 && changed <= 6);
}

#[test]
fn region_mapping_validates_and_rounds_outward() {
    let spec = RasterSpec::new(
        Viewport {
            width: 10,
            height: 4,
        },
        Quadrant.grid(),
        CellAspect::default(),
    )
    .unwrap();

    assert_eq!(
        spec.pixel_rect(CellRect::new(9, 3, 1, 1)),
        Ok(PixelRect::new(18, 6, 2, 2))
    );
    assert_eq!(
        spec.pixel_rect(CellRect::new(9, 3, 2, 1)),
        Err(RegionError::OutOfBounds)
    );
    assert_eq!(
        spec.covering_cells(PixelRect::new(19, 7, 1, 1)),
        Ok(CellRect::new(9, 3, 1, 1))
    );
    assert_eq!(
        spec.covering_cells(PixelRect::new(0, 0, 20, 8)),
        Ok(CellRect::new(0, 0, 10, 4))
    );
    assert_eq!(
        spec.covering_cells(PixelRect::new(0, 0, 21, 1)),
        Err(RegionError::OutOfBounds)
    );
    assert_eq!(
        spec.covering_cells(PixelRect::default()),
        Err(RegionError::Empty)
    );

    // Clipping a damage rectangle to the surface before mapping it.
    let surface = PixelRect::new(0, 0, spec.width(), spec.height());
    let damage = PixelRect::new(15, 6, 100, 100);
    assert_eq!(
        damage.intersection(&surface),
        Some(PixelRect::new(15, 6, 5, 2))
    );
    assert_eq!(
        PixelRect::new(0, 0, 2, 2).intersection(&PixelRect::new(2, 0, 1, 1)),
        None
    );
    assert_eq!(
        CellRect::new(0, 0, 5, 5).intersection(&CellRect::new(3, 4, 5, 5)),
        Some(CellRect::new(3, 4, 2, 1))
    );
}

#[test]
fn cell_views_can_wrap_caller_owned_slices() {
    let marker = Cell::with_glyph('#', Color::rgb(9, 9, 9), Color::DEFAULT);
    let mut storage = vec![Cell::default(); 14];

    assert_eq!(
        CellViewMut::from_slice(&mut storage, 4, 4).unwrap_err(),
        RegionError::OutOfBounds
    );
    {
        // A longer slice is fine; only the first width * height cells are used.
        let mut view = CellViewMut::from_slice(&mut storage, 4, 3).unwrap();
        assert_eq!((view.width(), view.height()), (4, 3));
        view.sub(CellRect::new(1, 2, 2, 1)).unwrap().row_mut(0)[1] = marker;
        for (first_row, mut band) in view.into_row_bands(2) {
            band.row_mut(0)[0] = Cell::with_glyph('b', Color::DEFAULT, Color::DEFAULT);
            assert!(first_row == 0 || first_row == 2);
        }
    }
    assert_eq!(storage[2 * 4 + 2], marker);
    assert_eq!(storage[0].glyph, 'b' as u32);
    assert_eq!(storage[8].glyph, 'b' as u32);
    assert_eq!(storage[12..], [Cell::default(); 2]);

    let empty = CellViewMut::from_slice(&mut [], 0, 7).unwrap();
    assert_eq!(empty.into_row_bands(1).count(), 0);
}
