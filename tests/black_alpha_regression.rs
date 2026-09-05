use core::time::Duration;

use trend::{
    AnsiPresenter, CellFrame, Color, FitMode, PixelFormat, PixelFrameView, RenderOptions,
    RenderRequest, Renderer, SamplingFilter, TerminalColorMode, Viewport,
};

fn black_alpha_fixture() -> Vec<u8> {
    let mut pixels = Vec::with_capacity(16 * 16 * 4);
    for y in 0..16_u8 {
        for x in 0..16_u8 {
            let rgba = match x {
                0..=3 => [0, 0, 0, 255],
                4..=7 => {
                    let level = (x - 4) * 3 + y / 6;
                    [level, level, level, 255]
                }
                8..=11 => {
                    if (x + y) % 2 == 0 {
                        [0, 0, 0, 255]
                    } else {
                        [255, 255, 255, 255]
                    }
                }
                12 => [255, 0, 0, 255],
                13 => [0, 255, 0, y.saturating_mul(17)],
                14 => [0, 0, 255, 0],
                _ => [0, 0, 0, 0],
            };
            pixels.extend_from_slice(&rgba);
        }
    }
    pixels
}

fn render_fixture(filter: SamplingFilter, viewport: Viewport) -> CellFrame {
    let pixels = black_alpha_fixture();
    let source =
        PixelFrameView::new(&pixels, 16, 16, 64, PixelFormat::Rgba8, Duration::ZERO).unwrap();
    let mut output = CellFrame::default();
    Renderer::new()
        .render_with_options_into(
            source,
            RenderRequest {
                viewport,
                fit: FitMode::Stretch,
                generation: 1,
            },
            RenderOptions {
                sampling: filter,
                ..RenderOptions::default()
            },
            &mut output,
        )
        .unwrap();
    output
}

fn render_pixels(
    pixels: &[u8],
    width: u32,
    height: u32,
    filter: SamplingFilter,
    alpha_background: Option<Color>,
) -> CellFrame {
    let source = PixelFrameView::new(
        pixels,
        width,
        height,
        usize::try_from(width).unwrap() * 4,
        PixelFormat::Rgba8,
        Duration::ZERO,
    )
    .unwrap();
    let mut output = CellFrame::default();
    Renderer::new()
        .render_with_options_into(
            source,
            RenderRequest {
                viewport: Viewport {
                    width: u16::try_from(width).unwrap(),
                    height: u16::try_from(height / 2).unwrap(),
                },
                fit: FitMode::Stretch,
                generation: 1,
            },
            RenderOptions {
                sampling: filter,
                alpha_background,
                ..RenderOptions::default()
            },
            &mut output,
        )
        .unwrap();
    output
}

#[test]
fn opaque_black_near_black_edges_and_saturated_colors_remain_exact() {
    let row = [
        0, 0, 0, 255, 1, 2, 3, 255, 255, 255, 255, 255, 255, 0, 0, 255,
    ];
    let pixels = [row, row].concat();

    for filter in [
        SamplingFilter::Nearest,
        SamplingFilter::Bilinear,
        SamplingFilter::Area,
    ] {
        let frame = render_pixels(&pixels, 4, 2, filter, None);
        let expected = [
            Color::rgb(0, 0, 0),
            Color::rgb(1, 2, 3),
            Color::rgb(255, 255, 255),
            Color::rgb(255, 0, 0),
        ];
        for (cell, color) in frame.cells().iter().zip(expected) {
            assert_eq!(cell.foreground, color);
            assert_eq!(cell.background, color);
        }
    }
}

#[test]
fn transparent_black_and_transparent_colored_pixels_are_equivalent_but_not_opaque_black() {
    let transparent_black = render_pixels(
        &[0, 0, 0, 0, 0, 0, 0, 0],
        1,
        2,
        SamplingFilter::Nearest,
        None,
    );
    let transparent_red = render_pixels(
        &[255, 0, 0, 0, 255, 0, 0, 0],
        1,
        2,
        SamplingFilter::Nearest,
        None,
    );
    let opaque_black = render_pixels(
        &[0, 0, 0, 255, 0, 0, 0, 255],
        1,
        2,
        SamplingFilter::Nearest,
        None,
    );

    assert_eq!(transparent_black, transparent_red);
    assert_eq!(transparent_black.cells()[0], trend::Cell::default());
    assert_ne!(transparent_black, opaque_black);
    assert_eq!(opaque_black.cells()[0].foreground, Color::rgb(0, 0, 0));
    assert_eq!(opaque_black.cells()[0].background, Color::rgb(0, 0, 0));
}

#[test]
fn partial_alpha_preserves_straight_color_or_composites_over_explicit_matte() {
    let pixels = [200, 100, 50, 128, 200, 100, 50, 128];
    let straight = render_pixels(&pixels, 1, 2, SamplingFilter::Nearest, None);
    let composited = render_pixels(
        &pixels,
        1,
        2,
        SamplingFilter::Nearest,
        Some(Color::rgb(20, 40, 60)),
    );

    assert_eq!(straight.cells()[0].foreground, Color::rgb(200, 100, 50));
    assert_eq!(straight.cells()[0].background, Color::rgb(200, 100, 50));
    assert_eq!(composited.cells()[0].foreground, Color::rgb(110, 70, 55));
    assert_eq!(composited.cells()[0].background, Color::rgb(110, 70, 55));
}

#[test]
fn alpha_gradient_composites_deterministically_over_caller_matte() {
    let row = [255, 0, 0, 0, 255, 0, 0, 85, 255, 0, 0, 170, 255, 0, 0, 255];
    let pixels = [row, row].concat();
    let first = render_pixels(
        &pixels,
        4,
        2,
        SamplingFilter::Nearest,
        Some(Color::rgb(0, 0, 0)),
    );
    let second = render_pixels(
        &pixels,
        4,
        2,
        SamplingFilter::Nearest,
        Some(Color::rgb(0, 0, 0)),
    );

    assert_eq!(first, second);
    let expected_red = [0, 85, 170, 255];
    for (cell, red) in first.cells().iter().zip(expected_red) {
        assert_eq!(cell.foreground, Color::rgb(red, 0, 0));
        assert_eq!(cell.background, Color::rgb(red, 0, 0));
    }
}

#[test]
fn transparent_regions_surround_opaque_content_without_black_fill() {
    let row = [0, 0, 0, 0, 255, 0, 0, 255, 0, 0, 255, 0];
    let pixels = [row, row].concat();
    let frame = render_pixels(&pixels, 3, 2, SamplingFilter::Nearest, None);

    assert_eq!(frame.cells()[0], trend::Cell::default());
    assert_eq!(frame.cells()[1].foreground, Color::rgb(255, 0, 0));
    assert_eq!(frame.cells()[1].background, Color::rgb(255, 0, 0));
    assert_eq!(frame.cells()[2], trend::Cell::default());
}

#[test]
fn ansi_distinguishes_terminal_default_from_explicit_black_even_when_fg_equals_bg() {
    let transparent = render_pixels(
        &[0, 0, 0, 0, 0, 0, 0, 0],
        1,
        2,
        SamplingFilter::Nearest,
        None,
    );
    let opaque = render_pixels(
        &[0, 0, 0, 255, 0, 0, 0, 255],
        1,
        2,
        SamplingFilter::Nearest,
        None,
    );
    let mut transparent_presenter = AnsiPresenter::new(TerminalColorMode::TrueColor);
    let mut opaque_presenter = AnsiPresenter::new(TerminalColorMode::TrueColor);
    let transparent_ansi = transparent_presenter.present(&transparent);
    let opaque_ansi = opaque_presenter.present(&opaque);

    assert_ne!(transparent_ansi, opaque_ansi);
    assert!(
        opaque_ansi
            .windows(13)
            .any(|bytes| bytes == b"\x1b[38;2;0;0;0m")
    );
    assert!(
        opaque_ansi
            .windows(13)
            .any(|bytes| bytes == b"\x1b[48;2;0;0;0m")
    );
    assert!(opaque_ansi.ends_with("▀".as_bytes()));
}

#[test]
fn black_heavy_alpha_fixture_is_deterministic_across_filters_and_sizes() {
    for filter in [
        SamplingFilter::Nearest,
        SamplingFilter::Bilinear,
        SamplingFilter::Area,
    ] {
        for viewport in [
            Viewport {
                width: 8,
                height: 4,
            },
            Viewport {
                width: 16,
                height: 8,
            },
            Viewport {
                width: 24,
                height: 12,
            },
        ] {
            let first = render_fixture(filter, viewport);
            let second = render_fixture(filter, viewport);
            assert_eq!(first, second);
            assert!(first.cells().iter().any(|cell| {
                cell.foreground == Color::rgb(0, 0, 0) || cell.background == Color::rgb(0, 0, 0)
            }));
            assert!(first.cells().iter().any(|cell| {
                cell.foreground == Color::DEFAULT || cell.background == Color::DEFAULT
            }));

            let mut first_presenter = AnsiPresenter::new(TerminalColorMode::TrueColor);
            let mut second_presenter = AnsiPresenter::new(TerminalColorMode::TrueColor);
            assert_eq!(
                first_presenter.present(&first),
                second_presenter.present(&second)
            );
        }
    }
}

#[test]
fn explicit_matte_compositing_handles_alpha_gradient_without_allocating_a_frame() {
    let pixels = black_alpha_fixture();
    let source_address = pixels.as_ptr();
    let source =
        PixelFrameView::new(&pixels, 16, 16, 64, PixelFormat::Rgba8, Duration::ZERO).unwrap();
    let mut output = CellFrame::default();
    Renderer::new()
        .render_with_options_into(
            source,
            RenderRequest {
                viewport: Viewport {
                    width: 16,
                    height: 8,
                },
                fit: FitMode::Stretch,
                generation: 1,
            },
            RenderOptions {
                sampling: SamplingFilter::Area,
                alpha_background: Some(Color::rgb(24, 32, 40)),
                ..RenderOptions::default()
            },
            &mut output,
        )
        .unwrap();

    assert_eq!(source.pixels().as_ptr(), source_address);
    assert!(
        output
            .cells()
            .iter()
            .all(|cell| cell.foreground != Color::DEFAULT && cell.background != Color::DEFAULT)
    );
}
