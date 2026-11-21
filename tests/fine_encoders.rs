//! M5: the `Sextant` and `Braille` encoders.
//!
//! Glyph tables are checked against tables generated from the Unicode
//! character names, the geometry against the same physical-area proofs as
//! `Quadrant`, and the fitting against its documented rules.

use core::time::Duration;

use trend::{
    Braille, Cell, CellAspect, CellEncoder, CellFrame, CellRect, Color, ColorUse, EncodeError,
    EncoderRequirements, FitMode, GlyphRepertoire, HalfBlock, ImageSource, PixelBuffer,
    PixelFormat, PixelFrameView, PixelRect, Quadrant, RasterSpec, SamplingFilter, Sextant,
    SubcellGrid, TerminalCapabilities, TerminalColorMode, Viewport,
};

/// Glyph for each sextant mask (bit i = position i + 1, row-major), built
/// from the Unicode names "BLOCK SEXTANT-…" plus the half and full blocks.
const SEXTANTS: &str = " 🬀🬁🬂🬃🬄🬅🬆🬇🬈🬉🬊🬋🬌🬍🬎🬏🬐🬑🬒🬓▌🬔🬕🬖🬗🬘🬙🬚🬛🬜🬝🬞🬟🬠🬡🬢🬣🬤🬥🬦🬧▐🬨🬩🬪🬫🬬🬭🬮🬯🬰🬱🬲🬳🬴🬵🬶🬷🬸🬹🬺🬻█";
/// Glyph for each mask over the 2×4 block in row-major order, built from
/// the Unicode names "BRAILLE PATTERN DOTS-…" (blank is a space).
const BRAILLE: &str = " ⠁⠈⠉⠂⠃⠊⠋⠐⠑⠘⠙⠒⠓⠚⠛⠄⠅⠌⠍⠆⠇⠎⠏⠔⠕⠜⠝⠖⠗⠞⠟⠠⠡⠨⠩⠢⠣⠪⠫⠰⠱⠸⠹⠲⠳⠺⠻⠤⠥⠬⠭⠦⠧⠮⠯⠴⠵⠼⠽⠶⠷⠾⠿⡀⡁⡈⡉⡂⡃⡊⡋⡐⡑⡘⡙⡒⡓⡚⡛⡄⡅⡌⡍⡆⡇⡎⡏⡔⡕⡜⡝⡖⡗⡞⡟⡠⡡⡨⡩⡢⡣⡪⡫⡰⡱⡸⡹⡲⡳⡺⡻⡤⡥⡬⡭⡦⡧⡮⡯⡴⡵⡼⡽⡶⡷⡾⡿⢀⢁⢈⢉⢂⢃⢊⢋⢐⢑⢘⢙⢒⢓⢚⢛⢄⢅⢌⢍⢆⢇⢎⢏⢔⢕⢜⢝⢖⢗⢞⢟⢠⢡⢨⢩⢢⢣⢪⢫⢰⢱⢸⢹⢲⢳⢺⢻⢤⢥⢬⢭⢦⢧⢮⢯⢴⢵⢼⢽⢶⢷⢾⢿⣀⣁⣈⣉⣂⣃⣊⣋⣐⣑⣘⣙⣒⣓⣚⣛⣄⣅⣌⣍⣆⣇⣎⣏⣔⣕⣜⣝⣖⣗⣞⣟⣠⣡⣨⣩⣢⣣⣪⣫⣰⣱⣸⣹⣲⣳⣺⣻⣤⣥⣬⣭⣦⣧⣮⣯⣴⣵⣼⣽⣶⣷⣾⣿";

const RED: Color = Color::rgb(220, 20, 40);
const BLUE: Color = Color::rgb(10, 40, 200);

fn glyph(table: &str, mask: u32) -> char {
    table.chars().nth(mask as usize).unwrap()
}

/// One cell from a block whose pixels are `color(index)` (`None` = transparent).
fn encode_block(encoder: &dyn CellEncoder, color: impl Fn(usize) -> Option<Color>) -> Cell {
    let grid = encoder.grid();
    let (width, height) = (u32::from(grid.columns), u32::from(grid.rows));
    let mut pixels = PixelBuffer::new(width, height).unwrap();
    for (index, pixel) in pixels.pixels_mut().chunks_exact_mut(4).enumerate() {
        pixel.copy_from_slice(&match color(index) {
            Some(color) => [color.red, color.green, color.blue, 255],
            None => [0; 4],
        });
    }
    let mut cells = CellFrame::default();
    cells.resize(1, 1).unwrap();
    encoder.encode(pixels.view(), cells.view_mut()).unwrap();
    cells.cells()[0]
}

#[test]
fn grids_and_requirements() {
    assert_eq!(
        Sextant.grid(),
        SubcellGrid {
            columns: 2,
            rows: 3
        }
    );
    assert_eq!(
        Braille.grid(),
        SubcellGrid {
            columns: 2,
            rows: 4
        }
    );
    assert_eq!(
        Sextant.requirements(),
        EncoderRequirements::new(GlyphRepertoire::Sextants, ColorUse::ForegroundAndBackground)
    );
    assert_eq!(
        Braille.requirements(),
        EncoderRequirements::new(GlyphRepertoire::Braille, ColorUse::ForegroundAndBackground)
    );
    for encoder in [&Sextant as &dyn CellEncoder, &Braille] {
        let support = |color, unicode| {
            encoder
                .requirements()
                .supported_by(TerminalCapabilities { color, unicode })
        };
        assert!(support(TerminalColorMode::TrueColor, true));
        assert!(support(TerminalColorMode::Ansi16, true));
        assert!(!support(TerminalColorMode::TrueColor, false));
        assert!(!support(TerminalColorMode::Monochrome, true));
    }
}

#[test]
fn partly_transparent_cells_use_every_glyph_of_the_unicode_tables() {
    // The visible pixels' mask selects the glyph directly, so this reaches
    // every mask except the fully visible one.
    for mask in 1..63_u32 {
        let cell = encode_block(&Sextant, |index| (mask >> index & 1 == 1).then_some(RED));
        assert_eq!(
            cell,
            Cell::with_glyph(glyph(SEXTANTS, mask), RED, Color::DEFAULT)
        );
    }
    for mask in 1..255_u32 {
        let cell = encode_block(&Braille, |index| (mask >> index & 1 == 1).then_some(RED));
        assert_eq!(
            cell,
            Cell::with_glyph(glyph(BRAILLE, mask), RED, Color::DEFAULT)
        );
    }
    for encoder in [&Sextant as &dyn CellEncoder, &Braille] {
        assert_eq!(encode_block(encoder, |_| None), Cell::default());
    }
}

#[test]
fn two_color_cells_are_reproduced_exactly() {
    // Sextant: the last pixel is always background.
    for mask in 1..63_u32 {
        let cell = encode_block(&Sextant, |index| {
            Some(if mask >> index & 1 == 1 { RED } else { BLUE })
        });
        let expected = if mask >> 5 & 1 == 0 {
            Cell::with_glyph(glyph(SEXTANTS, mask), RED, BLUE)
        } else {
            Cell::with_glyph(glyph(SEXTANTS, mask ^ 63), BLUE, RED)
        };
        assert_eq!(cell, expected, "mask {mask:06b}");
    }
    // Braille: the smaller part is the dots; with four each, the last pixel's
    // part is the background.
    for mask in 1..255_u32 {
        let cell = encode_block(&Braille, |index| {
            Some(if mask >> index & 1 == 1 { RED } else { BLUE })
        });
        let dots = mask.count_ones();
        let red_dots = dots < 4 || (dots == 4 && mask >> 7 & 1 == 0);
        let expected = if red_dots {
            Cell::with_glyph(glyph(BRAILLE, mask), RED, BLUE)
        } else {
            Cell::with_glyph(glyph(BRAILLE, mask ^ 255), BLUE, RED)
        };
        assert_eq!(cell, expected, "mask {mask:08b}");
    }
}

#[test]
fn uniform_cells_are_colored_spaces() {
    for encoder in [&Sextant as &dyn CellEncoder, &Braille] {
        assert_eq!(
            encode_block(encoder, |_| Some(BLUE)),
            Cell::with_glyph(' ', BLUE, BLUE)
        );
    }
}

fn disc(size: u32) -> Vec<u8> {
    let center = f64::from(size) / 2.0;
    (0..size * size)
        .flat_map(|index| {
            let (x, y) = (f64::from(index % size) + 0.5, f64::from(index / size) + 0.5);
            let inside = (x - center).powi(2) + (y - center).powi(2) <= center * center;
            if inside {
                [255, 0, 0, 255]
            } else {
                [0, 0, 255, 255]
            }
        })
        .collect()
}

fn render(
    encoder: &dyn CellEncoder,
    source: &[u8],
    size: u32,
    viewport: Viewport,
    cell: CellAspect,
) -> (PixelBuffer, CellFrame) {
    let view = PixelFrameView::new(
        source,
        size,
        size,
        size as usize * 4,
        PixelFormat::Rgba8,
        Duration::ZERO,
    )
    .unwrap();
    let spec = RasterSpec::new(viewport, encoder.grid(), cell).unwrap();
    let mut pixels = PixelBuffer::new(1, 1).unwrap();
    ImageSource::new(view)
        .with_fit(FitMode::Contain)
        .with_sampling(SamplingFilter::Area)
        .render_into(&spec, &mut pixels)
        .unwrap();
    let mut cells = CellFrame::default();
    cells.resize(viewport.width, viewport.height).unwrap();
    encoder.encode(pixels.view(), cells.view_mut()).unwrap();
    (pixels, cells)
}

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
fn every_encoder_covers_the_same_physical_area() {
    let source = disc(120);
    let viewport = Viewport {
        width: 60,
        height: 20,
    };
    for (cell, expected) in [
        (CellAspect::default(), (10, 50)),
        (
            CellAspect {
                width: 1,
                height: 1,
            },
            (20, 40),
        ),
    ] {
        for encoder in [
            &HalfBlock as &dyn CellEncoder,
            &Quadrant,
            &Sextant,
            &Braille,
        ] {
            let (pixels, cells) = render(encoder, &source, 120, viewport, cell);
            let grid = encoder.grid();
            assert_eq!(
                (pixels.width(), pixels.height()),
                (60 * u32::from(grid.columns), 20 * u32::from(grid.rows))
            );
            assert_eq!(covered_columns(&cells), expected, "{grid:?} {cell:?}");
        }
    }
    // Pixel shapes on 1:2 cells: sextant pixels are 3:4, Braille's square.
    let aspect = |grid| {
        let spec = RasterSpec::new(viewport, grid, CellAspect::default()).unwrap();
        (spec.pixel_aspect().width, spec.pixel_aspect().height)
    };
    assert_eq!(aspect(SubcellGrid::SEXTANT), (3, 4));
    assert_eq!(aspect(SubcellGrid::BRAILLE), (1, 1));
}

#[test]
fn finer_grids_resolve_vertical_stripes() {
    // Pixel rows alternate red and blue, which a 2×2 grid cannot show.
    let stripes = |index: usize| {
        Some(if (index / 2).is_multiple_of(2) {
            RED
        } else {
            BLUE
        })
    };
    // Sextant: rows 0 and 2 red, row 1 blue. The last pixel is red, so the
    // blue middle row is the foreground.
    assert_eq!(
        encode_block(&Sextant, stripes),
        Cell::with_glyph(glyph(SEXTANTS, 0b001100), BLUE, RED)
    );
    // Braille: four red and four blue pixels; the last pixel is blue, so
    // the red rows 0 and 2 are the dots.
    assert_eq!(
        encode_block(&Braille, stripes),
        Cell::with_glyph(glyph(BRAILLE, 0b0011_0011), RED, BLUE)
    );
}

/// Encoding any cell region from its own pixels gives the same cells as
/// encoding everything: the per-cell contract that row bands rely on.
#[test]
fn regions_encode_like_the_whole_frame() {
    let source = disc(97);
    let viewport = Viewport {
        width: 23,
        height: 11,
    };
    for encoder in [&Sextant as &dyn CellEncoder, &Braille] {
        let (pixels, whole) = render(encoder, &source, 97, viewport, CellAspect::default());
        let spec = RasterSpec::new(viewport, encoder.grid(), CellAspect::default()).unwrap();
        let mut cells = CellFrame::default();
        cells.resize(viewport.width, viewport.height).unwrap();
        for region in [
            CellRect::new(0, 0, 23, 4),
            CellRect::new(0, 4, 9, 7),
            CellRect::new(9, 4, 14, 7),
        ] {
            let pixel_rect: PixelRect = spec.pixel_rect(region).unwrap();
            encoder
                .encode(
                    pixels.view().sub(pixel_rect).unwrap(),
                    cells.view_mut().sub(region).unwrap(),
                )
                .unwrap();
        }
        assert_eq!(cells, whole);
    }
}

#[test]
fn encoders_reject_surfaces_sized_for_another_grid() {
    let pixels = PixelBuffer::new(4, 4).unwrap();
    let mut cells = CellFrame::default();
    cells.resize(2, 2).unwrap();
    assert_eq!(
        Sextant.encode(pixels.view(), cells.view_mut()),
        Err(EncodeError::SizeMismatch {
            expected: (4, 6),
            actual: (4, 4)
        })
    );
    assert_eq!(
        Braille.encode(pixels.view(), cells.view_mut()),
        Err(EncodeError::SizeMismatch {
            expected: (4, 8),
            actual: (4, 4)
        })
    );
}
