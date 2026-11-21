//! M5: composition and text overlays.
//!
//! - `compose::blend_over` places pixel overlays before encoding.
//! - `text::put_str` draws styled text into cells after encoding.
//! - The presenter writes cell attributes (bold, underline, …), and output
//!   for cells without attributes is unchanged (see the golden tests).

mod common;

use common::screen::{Pen, Screen};
use trend::compose::blend_over;
use trend::text::{TextBackground, TextStyle, put_str};
use trend::{
    AlphaMode, AnsiPresenter, Cell, CellEncoder, CellFrame, CellRect, Color, HalfBlock,
    OutputOptions, PixelBuffer, PixelFormat, PixelFrameView, PixelLayout, PixelRect, Quadrant,
    RowOrder, Sextant, Surface, TerminalCapabilities, TerminalColorMode, Viewport,
};

const TRUECOLOR: TerminalCapabilities = TerminalCapabilities {
    color: TerminalColorMode::TrueColor,
    unicode: true,
};
const YELLOW: Color = Color::rgb(250, 220, 40);
const NAVY: Color = Color::rgb(10, 20, 80);

fn frame(width: u16, height: u16, cell: Cell) -> CellFrame {
    let mut frame = CellFrame::default();
    frame.resize(width, height).unwrap();
    frame.cells_mut().fill(cell);
    frame
}

fn text(frame: &CellFrame, row: u16) -> String {
    frame
        .view()
        .row(row)
        .iter()
        .map(|cell| char::from_u32(cell.glyph).unwrap())
        .collect()
}

// ---------------------------------------------------------------- text

#[test]
fn text_is_written_one_character_per_cell_and_clipped() {
    let background = Cell::half_block(Color::rgb(1, 2, 3), NAVY);
    let mut cells = frame(10, 2, background);
    let style = TextStyle::new(YELLOW);
    assert_eq!(put_str(&mut cells.view_mut(), 2, 0, "hello", style), 5);
    assert_eq!(text(&cells, 0), "▀▀hello▀▀▀");
    // Keep: the cell's own background stays behind the text.
    assert_eq!(cells.cells()[2], Cell::with_glyph('h', YELLOW, NAVY));

    // Clipped at the right edge, never wrapped.
    assert_eq!(put_str(&mut cells.view_mut(), 7, 1, "clipped", style), 3);
    assert_eq!(text(&cells, 1), "▀▀▀▀▀▀▀cli");
    // Out of range writes nothing.
    assert_eq!(put_str(&mut cells.view_mut(), 10, 0, "x", style), 0);
    assert_eq!(put_str(&mut cells.view_mut(), 0, 2, "x", style), 0);
    assert_eq!(put_str(&mut cells.view_mut(), 0, 0, "", style), 0);
}

#[test]
fn text_backgrounds_and_attributes() {
    let mut cells = frame(6, 1, Cell::half_block(YELLOW, NAVY));
    put_str(
        &mut cells.view_mut(),
        0,
        0,
        "ab",
        TextStyle::new(YELLOW).with_background(TextBackground::Default),
    );
    put_str(
        &mut cells.view_mut(),
        2,
        0,
        "cd",
        TextStyle::new(NAVY)
            .with_background(TextBackground::Color(YELLOW))
            .with_attributes(Cell::BOLD | Cell::UNDERLINE | 0x80),
    );
    let row = cells.view().row(0);
    assert_eq!(row[0], Cell::with_glyph('a', YELLOW, Color::DEFAULT));
    let mut styled = Cell::with_glyph('c', NAVY, YELLOW);
    // The reserved top bit is never set.
    styled.attributes = Cell::BOLD | Cell::UNDERLINE;
    assert_eq!(row[2], styled);
    assert_eq!(row[4], Cell::half_block(YELLOW, NAVY), "untouched");
}

#[test]
fn characters_that_would_shift_the_row_become_question_marks() {
    let mut cells = frame(12, 1, Cell::default());
    let written = put_str(
        &mut cells.view_mut(),
        0,
        0,
        "a\tb漢c🙂e\u{301}f",
        TextStyle::default(),
    );
    assert_eq!(written, 9);
    assert_eq!(text(&cells, 0), "a?b?c?e?f   ");
}

#[test]
fn text_confined_to_a_sub_view_panel() {
    let mut cells = frame(12, 4, Cell::half_block(NAVY, NAVY));
    let panel = CellRect::new(3, 1, 5, 2);
    let mut view = cells.view_mut();
    let mut panel_view = view.sub(panel).unwrap();
    panel_view.fill(Cell::with_glyph(' ', Color::DEFAULT, Color::rgb(0, 0, 0)));
    assert_eq!(
        put_str(&mut panel_view, 1, 1, "status line", TextStyle::new(YELLOW)),
        4
    );
    assert_eq!(text(&cells, 2), "▀▀▀ stat▀▀▀▀");
    assert_eq!(text(&cells, 1), "▀▀▀     ▀▀▀▀");
    assert_eq!(cells.cells()[2 * 12 + 4].background, Color::rgb(0, 0, 0));
}

// ------------------------------------------------------------ attributes

fn present(frames: &[CellFrame]) -> Vec<Vec<u8>> {
    let mut presenter = AnsiPresenter::with_capabilities(TRUECOLOR);
    frames
        .iter()
        .map(|frame| presenter.present_at(frame, 1, 1).to_vec())
        .collect()
}

fn styled(glyph: char, attributes: u8) -> Cell {
    let mut cell = Cell::with_glyph(glyph, YELLOW, NAVY);
    cell.attributes = attributes;
    cell
}

#[test]
fn attributes_change_with_minimal_sequences() {
    let cells = [
        styled('a', 0),
        styled('b', Cell::BOLD),
        styled('c', Cell::BOLD | Cell::UNDERLINE),
        styled('d', Cell::DIM | Cell::UNDERLINE),
        styled('e', Cell::DIM),
        styled(
            'f',
            Cell::REVERSE | Cell::ITALIC | Cell::STRIKETHROUGH | Cell::BLINK,
        ),
        styled('g', 0),
        styled('h', Cell::BOLD | Cell::DIM),
        // Bold off while dim stays: `22` clears both, so dim is re-applied.
        styled('i', Cell::DIM),
        styled('j', Cell::BOLD),
    ];
    let mut row = CellFrame::default();
    row.resize(cells.len() as u16, 1).unwrap();
    row.cells_mut().copy_from_slice(&cells);
    let output = String::from_utf8(present(&[row])[0].clone()).unwrap();
    assert_eq!(
        output,
        "\x1b[1;1H\x1b[38;2;250;220;40m\x1b[48;2;10;20;80ma\
         \x1b[1mb\
         \x1b[4mc\
         \x1b[22;2md\
         \x1b[24me\
         \x1b[22;3;5;7;9mf\
         \x1b[23;25;27;29mg\
         \x1b[1;2mh\
         \x1b[22;2mi\
         \x1b[22;1mj\
         \x1b[0m"
    );
}

#[test]
fn attributes_render_as_expected_and_are_reset_after_positioned_output() {
    let mut first = frame(8, 2, Cell::with_glyph('.', YELLOW, NAVY));
    put_str(
        &mut first.view_mut(),
        1,
        0,
        "BOLD",
        TextStyle::new(YELLOW).with_attributes(Cell::BOLD),
    );
    let mut second = first.clone();
    put_str(
        &mut second.view_mut(),
        1,
        1,
        "und",
        TextStyle::new(YELLOW).with_attributes(Cell::UNDERLINE | Cell::ITALIC),
    );
    let outputs = present(&[first, second.clone()]);
    let mut screen = Screen::new(10, 3);
    for output in &outputs {
        screen.apply(output);
        assert_eq!(screen.pen_attributes(), 0, "reset after positioned output");
    }
    for (column, cell) in second.cells().iter().enumerate() {
        let (row, column) = (column / 8 + 1, column % 8 + 1);
        let shown = screen.at(row, column);
        assert_eq!(shown.attributes, cell.attributes, "({row}, {column})");
        assert_eq!(u32::from(shown.glyph), cell.glyph);
        assert_eq!(shown.foreground, Pen::Rgb(250, 220, 40));
    }
}

#[test]
fn attributes_survive_every_mode_and_option() {
    let mut cells = frame(6, 1, Cell::half_block(YELLOW, NAVY));
    put_str(
        &mut cells.view_mut(),
        0,
        0,
        "hi",
        TextStyle::new(YELLOW).with_attributes(Cell::BOLD),
    );
    for color in [
        TerminalColorMode::TrueColor,
        TerminalColorMode::Ansi256,
        TerminalColorMode::Ansi16,
        TerminalColorMode::Monochrome,
    ] {
        for options in [
            OutputOptions::new(),
            OutputOptions::new()
                .with_combined_colors(true)
                .with_synchronized_updates(true),
        ] {
            let mut presenter = AnsiPresenter::with_options(
                TerminalCapabilities {
                    color,
                    unicode: true,
                },
                options,
            );
            let mut screen = Screen::new(6, 1);
            screen.apply(presenter.present_at(&cells, 1, 1));
            assert_eq!(screen.at(1, 1).attributes, Cell::BOLD, "{color:?}");
            assert_eq!(screen.at(1, 3).attributes, 0, "{color:?}");
            let stats = presenter.last_stats();
            assert_eq!(stats.attribute_sequences, 2, "on, then off");
        }
    }
}

// ---------------------------------------------------------------- pixels

fn solid(width: u32, height: u32, rgba: [u8; 4]) -> PixelBuffer {
    let mut buffer = PixelBuffer::new(width, height).unwrap();
    for pixel in buffer.pixels_mut().chunks_exact_mut(4) {
        pixel.copy_from_slice(&rgba);
    }
    buffer
}

#[test]
fn overlays_are_clipped_and_report_the_touched_rectangle() {
    let sprite = solid(4, 3, [255, 0, 0, 255]);
    for (x, y, expected) in [
        (1, 1, Some(PixelRect::new(1, 1, 4, 3))),
        (-2, -1, Some(PixelRect::new(0, 0, 2, 2))),
        (8, 4, Some(PixelRect::new(8, 4, 2, 1))),
        (10, 0, None),
        (0, -3, None),
        (i32::MIN, i32::MAX, None),
    ] {
        let mut target = solid(10, 5, [0, 0, 255, 255]);
        let touched = blend_over(sprite.view(), &mut target.view_mut(), x, y);
        assert_eq!(touched, expected, "({x}, {y})");
        for py in 0..5 {
            for px in 0..10 {
                let inside = touched.is_some_and(|rect| {
                    (rect.x..rect.x + rect.width).contains(&px)
                        && (rect.y..rect.y + rect.height).contains(&py)
                });
                let want = if inside {
                    [255, 0, 0, 255]
                } else {
                    [0, 0, 255, 255]
                };
                assert_eq!(target.view().rgba(px, py), Some(want));
            }
        }
    }
}

#[test]
fn any_source_layout_blends_like_its_straight_rgba_equivalent() {
    // A translucent gradient, stored straight RGBA and as a premultiplied,
    // bottom-up BGRA readback with padded rows.
    let (width, height) = (7_u32, 5_u32);
    let straight = {
        let mut buffer = PixelBuffer::new(width, height).unwrap();
        for (index, pixel) in buffer.pixels_mut().chunks_exact_mut(4).enumerate() {
            let alpha = (index * 37 % 256) as u8;
            pixel.copy_from_slice(&[200, (index * 11) as u8, 40, alpha]);
        }
        buffer
    };
    let stride = width as usize * 4 + 12;
    let mut readback = vec![0_u8; stride * height as usize];
    let layout = PixelLayout::new(width, height, PixelFormat::Bgra8, AlphaMode::Premultiplied)
        .with_stride(stride)
        .with_row_order(RowOrder::BottomUp);
    for y in 0..height {
        for x in 0..width {
            let [r, g, b, a] = straight.view().rgba(x, y).unwrap();
            let premultiply = |c: u8| ((u32::from(c) * u32::from(a) + 127) / 255) as u8;
            let offset = (height - 1 - y) as usize * stride + x as usize * 4;
            readback[offset..offset + 4].copy_from_slice(&[
                premultiply(b),
                premultiply(g),
                premultiply(r),
                a,
            ]);
        }
    }
    let foreign = PixelFrameView::from_layout(&readback, layout).unwrap();
    // The reference: blending exactly what `rgba()` reads from the readback.
    let mut normalized = PixelBuffer::new(width, height).unwrap();
    for y in 0..height {
        for x in 0..width {
            let offset = (y * width + x) as usize * 4;
            normalized.pixels_mut()[offset..offset + 4]
                .copy_from_slice(&foreign.rgba(x, y).unwrap());
        }
    }
    let mut from_foreign = solid(12, 9, [10, 90, 30, 255]);
    let mut from_normalized = from_foreign.clone();
    blend_over(foreign, &mut from_foreign.view_mut(), 3, 2);
    blend_over(normalized.view(), &mut from_normalized.view_mut(), 3, 2);
    assert_eq!(from_foreign, from_normalized);
}

// ------------------------------------------------------ with the surface

#[test]
fn a_hud_over_a_surface_updates_only_what_changed() {
    let viewport = Viewport {
        width: 24,
        height: 8,
    };
    for encoder in [&HalfBlock as &dyn CellEncoder, &Quadrant, &Sextant] {
        let mut surface = Surface::new(encoder, viewport, TRUECOLOR).unwrap();
        surface
            .draw(|mut pixels, spec| {
                for y in 0..spec.height() {
                    for (x, pixel) in pixels.row_mut(y).chunks_exact_mut(4).enumerate() {
                        pixel.copy_from_slice(&[(x * 9) as u8, (y * 20) as u8, 120, 255]);
                    }
                }
            })
            .unwrap();
        let mut sink = Vec::new();
        surface.present(&mut sink).unwrap();

        // A translucent sprite: blend, then re-encode only its damage.
        let sprite = solid(5, 4, [255, 255, 255, 140]);
        let damage = blend_over(sprite.view(), &mut surface.pixels_mut(), 6, 3).unwrap();
        let region = surface.encode_region(damage).unwrap().unwrap();
        let mut full = CellFrame::default();
        full.resize(viewport.width, viewport.height).unwrap();
        encoder
            .encode(surface.pixels().view(), full.view_mut())
            .unwrap();
        assert_eq!(surface.cells(), &full, "{:?}", encoder.grid());

        // Text over the encoded cells, then present both changes.
        let style = TextStyle::new(YELLOW).with_attributes(Cell::BOLD);
        assert_eq!(put_str(&mut surface.cells_mut(), 0, 0, "HP 97", style), 5);
        surface.present(&mut sink).unwrap();
        let stats = surface.last_stats();
        let changed_region = usize::from(region.width) * usize::from(region.height);
        assert!(stats.cells <= changed_region + 5, "{stats:?}");
        assert!(stats.cells >= 5);

        // The screen shows the text with its attribute, over the image.
        let mut screen = Screen::new(30, 10);
        screen.apply(&sink);
        let label: String = (1..=5).map(|column| screen.at(1, column).glyph).collect();
        assert_eq!(label, "HP 97");
        assert_eq!(screen.at(1, 1).attributes, Cell::BOLD);
        assert_eq!(screen.at(1, 6).attributes, 0);
    }
}

#[test]
fn surface_presentation_of_attribute_free_frames_is_unchanged() {
    // Guard: a frame without attributes produces no attribute sequences.
    let viewport = Viewport {
        width: 16,
        height: 4,
    };
    let mut surface = Surface::new(Quadrant, viewport, TRUECOLOR).unwrap();
    surface
        .draw(|mut pixels, _| {
            for y in 0..pixels.height() {
                pixels.row_mut(y).fill(y as u8 * 30);
            }
        })
        .unwrap();
    let bytes = surface.present_bytes().to_vec();
    assert_eq!(surface.last_stats().attribute_sequences, 0);
    assert!(!String::from_utf8(bytes).unwrap().contains("\x1b[1m"));
}
