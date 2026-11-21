//! M1 proof that the pipeline is not built around 1×2 half-block pixels.
//!
//! `Quadrant` resolves 2×2 pixels per cell, so its pixels are half as wide as
//! they are tall on a 1:2 cell. Nothing in these tests special-cases either
//! encoder: the grid comes from `CellEncoder::grid`, the physical shape from
//! `CellAspect`, and the rest follows from `RasterSpec`.

use core::time::Duration;

use trend::{
    AnsiPresenter, Cell, CellAspect, CellEncoder, CellFrame, Color, EncodeError, FitMode,
    HalfBlock, ImageSource, PixelBuffer, PixelFormat, PixelFrameView, Quadrant, RasterSpec,
    RenderGeometry, RenderTransform, SamplingFilter, TerminalCapabilities, TerminalColorMode,
    Viewport,
};

const RED: Color = Color::rgb(255, 0, 0);
const BLUE: Color = Color::rgb(0, 0, 255);

/// A square opaque source with a disc of red on blue.
fn disc(size: u32) -> Vec<u8> {
    let mut pixels = Vec::with_capacity(size as usize * size as usize * 4);
    let center = f64::from(size) / 2.0;
    for y in 0..size {
        for x in 0..size {
            let dx = f64::from(x) + 0.5 - center;
            let dy = f64::from(y) + 0.5 - center;
            let inside = dx * dx + dy * dy <= center * center;
            pixels.extend_from_slice(if inside {
                &[255, 0, 0, 255]
            } else {
                &[0, 0, 255, 255]
            });
        }
    }
    pixels
}

fn view(pixels: &[u8], width: u32, height: u32) -> PixelFrameView<'_> {
    PixelFrameView::new(
        pixels,
        width,
        height,
        width as usize * 4,
        PixelFormat::Rgba8,
        Duration::ZERO,
    )
    .unwrap()
}

/// Renders through the generic pipeline for any encoder.
fn render(
    encoder: &dyn CellEncoder,
    source: PixelFrameView<'_>,
    viewport: Viewport,
    cell: CellAspect,
    fit: FitMode,
) -> (PixelBuffer, CellFrame) {
    let spec = RasterSpec::new(viewport, encoder.grid(), cell).unwrap();
    let mut pixels = PixelBuffer::new(1, 1).unwrap();
    ImageSource::new(source)
        .with_fit(fit)
        .with_sampling(SamplingFilter::Area)
        .render_into(&spec, &mut pixels)
        .unwrap();
    let mut cells = CellFrame::default();
    cells.resize(viewport.width, viewport.height).unwrap();
    encoder.encode(pixels.view(), cells.view_mut()).unwrap();
    (pixels, cells)
}

/// (min_x, max_x_exclusive, min_y, max_y_exclusive) of pixels with alpha > 0.
fn visible_bounds(pixels: &PixelBuffer) -> (u32, u32, u32, u32) {
    let mut bounds = (u32::MAX, 0, u32::MAX, 0);
    for (index, pixel) in pixels.pixels().chunks_exact(4).enumerate() {
        if pixel[3] != 0 {
            let x = index as u32 % pixels.width();
            let y = index as u32 / pixels.width();
            bounds = (
                bounds.0.min(x),
                bounds.1.max(x + 1),
                bounds.2.min(y),
                bounds.3.max(y + 1),
            );
        }
    }
    bounds
}

/// (min_x, max_x_exclusive) of cell columns that are not the default cell.
fn covered_columns(cells: &CellFrame) -> (u16, u16) {
    let mut columns = (u16::MAX, 0);
    for (index, cell) in cells.cells().iter().enumerate() {
        if *cell != Cell::default() {
            let x = (index % usize::from(cells.width())) as u16;
            columns = (columns.0.min(x), columns.1.max(x + 1));
        }
    }
    columns
}

#[test]
fn same_source_covers_the_same_physical_area_under_both_encoders() {
    let source = disc(100);
    let viewport = Viewport {
        width: 60,
        height: 20,
    };

    let (half_pixels, half_cells) = render(
        &HalfBlock,
        view(&source, 100, 100),
        viewport,
        CellAspect::default(),
        FitMode::Contain,
    );
    let (quad_pixels, quad_cells) = render(
        &Quadrant,
        view(&source, 100, 100),
        viewport,
        CellAspect::default(),
        FitMode::Contain,
    );

    // Different pixel surfaces for the same viewport...
    assert_eq!((half_pixels.width(), half_pixels.height()), (60, 40));
    assert_eq!((quad_pixels.width(), quad_pixels.height()), (120, 40));
    // ...but the square source stays physically square in both: 40x40 square
    // pixels versus 80x40 pixels that are each 1 wide and 2 tall.
    assert_eq!(visible_bounds(&half_pixels), (10, 50, 0, 40));
    assert_eq!(visible_bounds(&quad_pixels), (20, 100, 0, 40));
    // So both land on exactly the same terminal cells.
    assert_eq!(covered_columns(&half_cells), (10, 50));
    assert_eq!(covered_columns(&quad_cells), (10, 50));
}

#[test]
fn cell_aspect_is_host_data_not_a_constant() {
    let source = disc(100);
    let viewport = Viewport {
        width: 60,
        height: 20,
    };
    // A terminal with square cells: the 60x20 viewport is physically 3:1, so a
    // square image is 20 cells wide, for either encoder.
    let square_cells = CellAspect {
        width: 1,
        height: 1,
    };
    for encoder in [&HalfBlock as &dyn CellEncoder, &Quadrant] {
        let (_, cells) = render(
            encoder,
            view(&source, 100, 100),
            viewport,
            square_cells,
            FitMode::Contain,
        );
        assert_eq!(covered_columns(&cells), (20, 40));
    }
}

#[test]
fn quadrant_resolves_horizontal_detail_that_half_blocks_cannot() {
    // Two cells wide, one cell tall: each cell is red on the left, blue on the right.
    let mut source = Vec::new();
    for _ in 0..2 {
        for color in [RED, BLUE, RED, BLUE] {
            source.extend_from_slice(&[color.red, color.green, color.blue, 255]);
        }
    }
    let viewport = Viewport {
        width: 2,
        height: 1,
    };
    let render_nearest = |encoder: &dyn CellEncoder| {
        let spec = RasterSpec::new(viewport, encoder.grid(), CellAspect::default()).unwrap();
        let mut pixels = PixelBuffer::new(1, 1).unwrap();
        ImageSource::new(view(&source, 4, 2))
            .with_fit(FitMode::Stretch)
            .render_into(&spec, &mut pixels)
            .unwrap();
        let mut cells = CellFrame::default();
        cells.resize(2, 1).unwrap();
        encoder.encode(pixels.view(), cells.view_mut()).unwrap();
        cells
    };

    let quadrant = render_nearest(&Quadrant);
    assert_eq!(quadrant.cells(), &[Cell::with_glyph('▌', RED, BLUE); 2]);

    // Half-block cells have one column of pixels, so a left/right split is lost.
    let half = render_nearest(&HalfBlock);
    assert!(
        half.cells()
            .iter()
            .all(|cell| cell.foreground == cell.background)
    );
}

#[test]
fn pan_is_specified_in_cells_and_scaled_by_each_grid() {
    let viewport = Viewport {
        width: 20,
        height: 10,
    };
    let transform = RenderTransform {
        zoom_percent: 300,
        pan_x_cells: 3,
        pan_y_cells: 2,
        ..RenderTransform::default()
    };
    let geometry = |encoder: &dyn CellEncoder| {
        let spec = RasterSpec::new(viewport, encoder.grid(), CellAspect::default()).unwrap();
        RenderGeometry::calculate_for_spec(100, 100, &spec, FitMode::Contain, transform).unwrap()
    };
    let half = geometry(&HalfBlock);
    let quad = geometry(&Quadrant);

    // Same physical placement: quadrant pixels are half as wide, so every
    // horizontal pixel quantity doubles while vertical ones match.
    assert_eq!((half.offset_x, half.offset_y), (-23, -24));
    assert_eq!(quad.sample_width, half.sample_width * 2);
    assert_eq!(quad.sample_height, half.sample_height);
    assert_eq!(quad.offset_x, half.offset_x * 2);
    assert_eq!(quad.offset_y, half.offset_y);
}

#[test]
fn quadrant_cells_go_through_the_existing_presenter_unchanged() {
    let source = disc(64);
    let (_, cells) = render(
        &Quadrant,
        view(&source, 64, 64),
        Viewport {
            width: 16,
            height: 8,
        },
        CellAspect::default(),
        FitMode::Contain,
    );
    let glyphs: Vec<char> = cells
        .cells()
        .iter()
        .filter_map(|cell| char::from_u32(cell.glyph))
        .collect();
    // The disc edge needs glyphs half-blocks do not have.
    assert!(glyphs.iter().any(|glyph| "▘▝▖▗▞▚▛▜▙▟▌▐".contains(*glyph)));

    let mut presenter = AnsiPresenter::new(TerminalColorMode::TrueColor);
    let output = String::from_utf8(presenter.present(&cells).to_vec()).unwrap();
    assert!(output.contains("\x1b[38;2;255;0;0m"));
    assert!(
        glyphs
            .iter()
            .filter(|glyph| **glyph != ' ')
            .all(|glyph| output.contains(*glyph))
    );

    // The backend's existing ASCII fallback still applies to new glyphs.
    let mut ascii = AnsiPresenter::with_capabilities(TerminalCapabilities {
        color: TerminalColorMode::Ansi256,
        unicode: false,
    });
    assert!(ascii.present(&cells).is_ascii());
}

#[test]
fn encoders_reject_surfaces_sized_for_a_different_grid() {
    let viewport = Viewport {
        width: 5,
        height: 3,
    };
    let half_spec = RasterSpec::new(viewport, HalfBlock.grid(), CellAspect::default()).unwrap();
    let pixels = PixelBuffer::new(half_spec.width(), half_spec.height()).unwrap();
    let mut cells = CellFrame::default();
    cells.resize(5, 3).unwrap();

    assert_eq!(
        Quadrant.encode(pixels.view(), cells.view_mut()),
        Err(EncodeError::SizeMismatch {
            expected: (10, 6),
            actual: (5, 6)
        })
    );
}
