//! M3: encoder requirements and the `Density` encoder.
//!
//! Density is the first encoder that works without color. It's a real
//! `CellEncoder`: one pixel per cell, brightness mapped to glyph density, so
//! monochrome and ASCII-only terminals get a faithful image through the same
//! pipeline as every other encoder.

use core::time::Duration;

use trend::{
    AnsiPresenter, Cell, CellAspect, CellEncoder, CellFrame, Color, ColorUse, Density, DensityRamp,
    EncodeError, EncoderRequirements, FitMode, GlyphRepertoire, HalfBlock, ImageSource,
    PixelBuffer, PixelFormat, PixelFrameView, Quadrant, RasterSpec, TerminalCapabilities,
    TerminalColorMode, Viewport,
};

const MONO_ASCII: TerminalCapabilities = TerminalCapabilities::CONSERVATIVE;
const MONO_UNICODE: TerminalCapabilities = TerminalCapabilities {
    color: TerminalColorMode::Monochrome,
    unicode: true,
};
const COLOR_ASCII: TerminalCapabilities = TerminalCapabilities {
    color: TerminalColorMode::Ansi256,
    unicode: false,
};
const COLOR_UNICODE: TerminalCapabilities = TerminalCapabilities {
    color: TerminalColorMode::TrueColor,
    unicode: true,
};

fn gray_row(levels: &[u8]) -> PixelBuffer {
    let mut buffer = PixelBuffer::new(levels.len() as u32, 1).unwrap();
    for (pixel, &level) in buffer.pixels_mut().chunks_exact_mut(4).zip(levels) {
        pixel.copy_from_slice(&[level, level, level, 255]);
    }
    buffer
}

fn encode(encoder: &dyn CellEncoder, pixels: &PixelBuffer) -> CellFrame {
    let mut cells = CellFrame::default();
    cells
        .resize(pixels.width() as u16, pixels.height() as u16)
        .unwrap();
    encoder.encode(pixels.view(), cells.view_mut()).unwrap();
    cells
}

fn glyphs(cells: &CellFrame) -> String {
    cells
        .cells()
        .iter()
        .map(|cell| char::from_u32(cell.glyph).unwrap())
        .collect()
}

#[test]
fn requirements_describe_each_encoder() {
    assert_eq!(HalfBlock.grid().columns * HalfBlock.grid().rows, 2);
    assert_eq!(
        Density::ASCII.grid().columns * Density::ASCII.grid().rows,
        1
    );
    assert_eq!(
        HalfBlock.requirements(),
        EncoderRequirements::new(
            GlyphRepertoire::BlockElements,
            ColorUse::ForegroundAndBackground
        )
    );
    assert_eq!(Quadrant.requirements(), HalfBlock.requirements());
    assert_eq!(
        Density::ASCII.requirements(),
        EncoderRequirements::new(GlyphRepertoire::Ascii, ColorUse::None)
    );
    assert_eq!(
        Density::new(DensityRamp::Shades, true).requirements(),
        EncoderRequirements::new(GlyphRepertoire::BlockElements, ColorUse::Foreground)
    );
    assert_eq!(Density::default(), Density::ASCII);
}

#[test]
fn requirements_decide_which_terminals_show_an_encoder_faithfully() {
    let cases: [(&dyn CellEncoder, [bool; 4]); 5] = [
        //                                     mono/ascii mono/uni color/ascii color/uni
        (&HalfBlock, [false, false, false, true]),
        (&Quadrant, [false, false, false, true]),
        (&Density::ASCII, [true, true, true, true]),
        (
            &Density::new(DensityRamp::Shades, false),
            [false, true, false, true],
        ),
        (
            &Density::new(DensityRamp::Ascii, true),
            [false, false, true, true],
        ),
    ];
    for (encoder, expected) in cases {
        let actual = [MONO_ASCII, MONO_UNICODE, COLOR_ASCII, COLOR_UNICODE]
            .map(|capabilities| encoder.requirements().supported_by(capabilities));
        assert_eq!(actual, expected, "{:?}", encoder.requirements());
    }
}

#[test]
fn hosts_can_choose_an_encoder_by_capability() {
    // A host policy in a few lines: the first preferred encoder the terminal
    // shows faithfully. TRend supplies the facts, the host decides.
    let preference: [&dyn CellEncoder; 4] = [
        &HalfBlock,
        &Density::new(DensityRamp::Shades, true),
        &Density::new(DensityRamp::Shades, false),
        &Density::ASCII,
    ];
    let choose = |capabilities| {
        preference
            .iter()
            .find(|encoder| encoder.requirements().supported_by(capabilities))
            .map(|encoder| encoder.requirements())
    };
    assert_eq!(choose(COLOR_UNICODE), Some(HalfBlock.requirements()));
    assert_eq!(
        choose(MONO_UNICODE),
        Some(Density::new(DensityRamp::Shades, false).requirements())
    );
    assert_eq!(choose(MONO_ASCII), Some(Density::ASCII.requirements()));
    assert_eq!(choose(COLOR_ASCII), Some(Density::ASCII.requirements()));
}

#[test]
fn density_maps_brightness_to_ramp_steps() {
    let pixels = gray_row(&[0, 30, 60, 90, 120, 150, 180, 210, 240, 255]);
    assert_eq!(glyphs(&encode(&Density::ASCII, &pixels)), " .:-=+*#%@");
    assert_eq!(
        glyphs(&encode(&Density::new(DensityRamp::Shades, false), &pixels)),
        "   ░░▒▒▓▓█"
    );

    // Colorless output carries no color; colored output keeps the pixel's.
    let cells = encode(&Density::ASCII, &pixels);
    assert!(
        cells
            .cells()
            .iter()
            .all(|cell| cell.foreground == Color::DEFAULT && cell.background == Color::DEFAULT)
    );
    let colored = encode(&Density::new(DensityRamp::Ascii, true), &gray_row(&[200]));
    assert_eq!(
        colored.cells()[0],
        Cell::with_glyph('#', Color::rgb(200, 200, 200), Color::DEFAULT)
    );
}

#[test]
fn density_uses_brightness_not_just_one_channel() {
    // Pure green is far brighter than pure blue at the same channel value.
    let mut pixels = PixelBuffer::new(2, 1).unwrap();
    pixels
        .pixels_mut()
        .copy_from_slice(&[0, 255, 0, 255, 0, 0, 255, 255]);
    assert_eq!(glyphs(&encode(&Density::ASCII, &pixels)), "* ");
}

#[test]
fn transparent_pixels_become_default_cells() {
    let mut pixels = PixelBuffer::new(3, 1).unwrap();
    pixels
        .pixels_mut()
        .copy_from_slice(&[255, 255, 255, 0, 255, 255, 255, 1, 0, 0, 0, 255]);
    let cells = encode(&Density::new(DensityRamp::Ascii, true), &pixels);
    assert_eq!(cells.cells()[0], Cell::default());
    assert_eq!(cells.cells()[1].glyph, '@' as u32);
    // Opaque black is the empty ramp step, but keeps its explicit color.
    assert_eq!(
        cells.cells()[2],
        Cell::with_glyph(' ', Color::rgb(0, 0, 0), Color::DEFAULT)
    );
}

#[test]
fn density_matches_the_presenters_monochrome_shading_for_flat_cells() {
    // For a flat cell of brightness g, the presenter's monochrome mode and
    // Density choose the same step of the same ramp.
    for (ramp, unicode) in [(DensityRamp::Ascii, false), (DensityRamp::Shades, true)] {
        for level in 0..=255_u8 {
            let density = encode(&Density::new(ramp, false), &gray_row(&[level]));
            let mut flat = CellFrame::default();
            flat.resize(1, 1).unwrap();
            flat.cells_mut()[0] =
                Cell::with_glyph(' ', Color::DEFAULT, Color::rgb(level, level, level));
            let mut presenter = AnsiPresenter::with_capabilities(TerminalCapabilities {
                color: TerminalColorMode::Monochrome,
                unicode,
            });
            let shaded = String::from_utf8(presenter.present(&flat).to_vec()).unwrap();
            assert_eq!(
                shaded.chars().last(),
                char::from_u32(density.cells()[0].glyph),
                "{ramp:?} level {level}"
            );
        }
    }
}

#[test]
fn monochrome_presenter_writes_colorless_glyphs_as_is() {
    let pixels = gray_row(&[0, 60, 120, 180, 255]);
    let cells = encode(&Density::ASCII, &pixels);
    let mut presenter = AnsiPresenter::with_capabilities(MONO_ASCII);
    let output = String::from_utf8(presenter.present(&cells).to_vec()).unwrap();
    // Before M3, every colorless cell presented as a blank in monochrome.
    assert!(output.ends_with(" :=*@"), "{output:?}");

    // Colored cells are still shaded by brightness, exactly as before.
    let mut colored = CellFrame::default();
    colored.resize(1, 1).unwrap();
    colored.cells_mut()[0] = Cell::with_glyph('x', Color::rgb(255, 255, 255), Color::DEFAULT);
    let mut presenter = AnsiPresenter::with_capabilities(MONO_ASCII);
    assert!(presenter.present(&colored).ends_with(b"@"));

    // Non-ASCII glyphs keep the presenter's existing ASCII fallback.
    let shades = encode(&Density::new(DensityRamp::Shades, false), &gray_row(&[255]));
    let mut presenter = AnsiPresenter::with_capabilities(MONO_ASCII);
    assert!(presenter.present(&shades).ends_with(b"#"));
}

#[test]
fn density_through_the_pipeline_keeps_physical_shape() {
    // A square image in a 60×20 viewport of 1:2 cells covers 40 columns under
    // Density (pixels 1 wide, 2 tall) exactly as under HalfBlock.
    let size = 64_u32;
    let source: Vec<u8> = (0..size * size)
        .flat_map(|_| [220, 220, 220, 255])
        .collect();
    let view = PixelFrameView::new(
        &source,
        size,
        size,
        size as usize * 4,
        PixelFormat::Rgba8,
        Duration::ZERO,
    )
    .unwrap();
    let viewport = Viewport {
        width: 60,
        height: 20,
    };
    let covered = |encoder: &dyn CellEncoder| {
        let spec = RasterSpec::new(viewport, encoder.grid(), CellAspect::default()).unwrap();
        let mut pixels = PixelBuffer::new(1, 1).unwrap();
        ImageSource::new(view)
            .with_fit(FitMode::Contain)
            .render_into(&spec, &mut pixels)
            .unwrap();
        let mut cells = CellFrame::default();
        cells.resize(viewport.width, viewport.height).unwrap();
        encoder.encode(pixels.view(), cells.view_mut()).unwrap();
        let columns: Vec<usize> = cells
            .cells()
            .iter()
            .enumerate()
            .filter(|(_, cell)| **cell != Cell::default())
            .map(|(index, _)| index % 60)
            .collect();
        (
            *columns.iter().min().unwrap(),
            *columns.iter().max().unwrap() + 1,
        )
    };
    let density_spec =
        RasterSpec::new(viewport, Density::ASCII.grid(), CellAspect::default()).unwrap();
    assert_eq!(
        (
            density_spec.pixel_aspect().width,
            density_spec.pixel_aspect().height
        ),
        (1, 2)
    );
    assert_eq!(covered(&Density::ASCII), (10, 50));
    assert_eq!(covered(&HalfBlock), (10, 50));
}

#[test]
fn density_rejects_surfaces_sized_for_another_grid() {
    let pixels = PixelBuffer::new(4, 2).unwrap();
    let mut cells = CellFrame::default();
    cells.resize(4, 1).unwrap();
    assert_eq!(
        Density::ASCII.encode(pixels.view(), cells.view_mut()),
        Err(EncodeError::SizeMismatch {
            expected: (4, 1),
            actual: (4, 2)
        })
    );
}
