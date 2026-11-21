//! Recorded baseline of the existing `AnsiPresenter` byte output.
//!
//! Recorded before the M1 pipeline work. Presentation must stay
//! byte-identical for every color mode, Unicode setting, and placement.
//!
//! To record an intentional change, run with `TREND_RECORD_GOLDEN=1` and
//! `--nocapture`, review the diff, and paste the printed table.

mod common;

use common::Fnv;
use trend::{
    AnsiPresenter, CellFrame, FitMode, PositionedRenderRequest, RenderOptions, RenderRequest,
    Renderer, SamplingFilter, TerminalCapabilities, TerminalColorMode, Viewport,
};

const MODES: [TerminalColorMode; 4] = [
    TerminalColorMode::TrueColor,
    TerminalColorMode::Ansi256,
    TerminalColorMode::Ansi16,
    TerminalColorMode::Monochrome,
];

/// (mode, unicode, positioned) => FNV-1a of the bytes of a five-frame sequence.
const ANSI_GOLDEN: [((&str, bool, bool), u64); 16] = [
    (("TrueColor", true, false), 0x93762CCE2495FBE0),
    (("TrueColor", true, true), 0x067A69DF18188DD2),
    (("TrueColor", false, false), 0x537E0E7E36D7A29B),
    (("TrueColor", false, true), 0x50FB606A3AAC8503),
    (("Ansi256", true, false), 0xC480A239A255C7DC),
    (("Ansi256", true, true), 0x6DEDDB593F384850),
    (("Ansi256", false, false), 0xA9D34A19BB4487B7),
    (("Ansi256", false, true), 0x8F4C6421AC865E49),
    (("Ansi16", true, false), 0x1D3779C845AC94AA),
    (("Ansi16", true, true), 0xFB5C1AB23DDF4924),
    (("Ansi16", false, false), 0x19B93D398661E353),
    (("Ansi16", false, true), 0x59FA0A7BC26B40BF),
    (("Monochrome", true, false), 0xC50DDD6EE240265C),
    (("Monochrome", true, true), 0x57E9145251CAF5AB),
    (("Monochrome", false, false), 0xF005084223A06689),
    (("Monochrome", false, true), 0x4836F66F3EC5D5FD),
];

/// FNV-1a of `render_at` output for each color mode (Unicode on).
const RENDER_AT_GOLDEN: [(&str, u64); 4] = [
    ("TrueColor", 0x74AD9F4EB7D034E9),
    ("Ansi256", 0x81076F8C31AE5AA5),
    ("Ansi16", 0x95104CF4E68BA88B),
    ("Monochrome", 0xCB414B894BF76C35),
];

fn render(pattern: usize, viewport: Viewport, fit: FitMode, options: RenderOptions) -> CellFrame {
    let patterns = common::patterns();
    let mut frame = CellFrame::default();
    Renderer::new()
        .render_with_options_into(
            patterns[pattern].view(),
            RenderRequest {
                viewport,
                fit,
                generation: 1,
            },
            options,
            &mut frame,
        )
        .unwrap();
    frame
}

/// Initial draw, full change, no change, geometry change, geometry change back.
fn frame_sequence() -> Vec<CellFrame> {
    let area = RenderOptions {
        sampling: SamplingFilter::Area,
        ..RenderOptions::default()
    };
    let dithered = RenderOptions {
        sampling: SamplingFilter::Bilinear,
        dither_strength: 16,
        ..RenderOptions::default()
    };
    let large = Viewport {
        width: 17,
        height: 9,
    };
    let first = render(0, large, FitMode::Contain, area);
    let second = render(4, large, FitMode::Cover, dithered);
    let small = render(
        1,
        Viewport {
            width: 3,
            height: 2,
        },
        FitMode::Stretch,
        RenderOptions::default(),
    );
    vec![first.clone(), second.clone(), second, small, first]
}

fn check<T: std::fmt::Debug + PartialEq>(label: &str, actual: &[(T, u64)], expected: &[(T, u64)]) {
    if common::recording() {
        println!("{label}:");
        for (key, hash) in actual {
            println!("    ({key:?}, 0x{hash:016X}),");
        }
        return;
    }
    assert_eq!(actual.len(), expected.len());
    let mismatches: Vec<_> = actual
        .iter()
        .zip(expected)
        .filter(|(a, e)| a != e)
        .map(|(a, e)| format!("{:?}: got 0x{:016X}, recorded 0x{:016X}", a.0, a.1, e.1))
        .collect();
    assert!(
        mismatches.is_empty(),
        "{label} diverged from the recorded baseline:\n{}",
        mismatches.join("\n")
    );
}

#[test]
fn presenter_bytes_match_recorded_baseline() {
    let frames = frame_sequence();
    let mut actual = Vec::new();
    for mode in MODES {
        for unicode in [true, false] {
            for positioned in [false, true] {
                let mut presenter = AnsiPresenter::with_capabilities(TerminalCapabilities {
                    color: mode,
                    unicode,
                });
                let mut hash = Fnv::default();
                for frame in &frames {
                    let bytes = if positioned {
                        presenter.present_at(frame, 3, 5)
                    } else {
                        presenter.present(frame)
                    };
                    hash.write_u64(bytes.len() as u64);
                    hash.write(bytes);
                }
                let mode_name = format!("{mode:?}");
                actual.push(((mode_name, unicode, positioned), hash.finish()));
            }
        }
    }
    let expected: Vec<_> = ANSI_GOLDEN
        .iter()
        .map(|&((mode, unicode, positioned), hash)| ((mode.to_owned(), unicode, positioned), hash))
        .collect();
    check("ANSI_GOLDEN", &actual, &expected);
}

#[test]
fn render_at_bytes_match_recorded_baseline() {
    let patterns = common::patterns();
    let renderer = Renderer::new();
    let mut actual = Vec::new();
    for mode in MODES {
        let mut presenter = AnsiPresenter::new(mode);
        let mut cells = CellFrame::default();
        let mut hash = Fnv::default();
        for (pattern, origin) in [(0, (1, 1)), (4, (1, 1)), (1, (4, 9))] {
            let output = presenter
                .render_at(
                    &renderer,
                    patterns[pattern].view(),
                    PositionedRenderRequest {
                        render: RenderRequest {
                            viewport: Viewport {
                                width: 15,
                                height: 15,
                            },
                            fit: FitMode::Contain,
                            generation: pattern as u64,
                        },
                        origin_row: origin.0,
                        origin_column: origin.1,
                    },
                    RenderOptions {
                        sampling: SamplingFilter::Area,
                        ..RenderOptions::default()
                    },
                    &mut cells,
                )
                .unwrap();
            hash.write_u64(output.bytes.len() as u64);
            hash.write(output.bytes);
        }
        actual.push((format!("{mode:?}"), hash.finish()));
    }
    let expected: Vec<_> = RENDER_AT_GOLDEN
        .iter()
        .map(|&(mode, hash)| (mode.to_owned(), hash))
        .collect();
    check("RENDER_AT_GOLDEN", &actual, &expected);
}

/// (mode, unicode, positioned) => FNV-1a of a sequence of partial updates.
/// Recorded from the M2b presenter, before M3 rewrote its serialization and
/// diff scan.
const PARTIAL_GOLDEN: [((&str, bool, bool), u64); 16] = [
    (("TrueColor", true, false), 0xA3D1BE9BD53B7447),
    (("TrueColor", true, true), 0xC0511FD106EBC22E),
    (("TrueColor", false, false), 0xEC3ECF89FE17C559),
    (("TrueColor", false, true), 0xF8A7EEB34028FA88),
    (("Ansi256", true, false), 0x9B0042164776602F),
    (("Ansi256", true, true), 0x24776F709E8843FA),
    (("Ansi256", false, false), 0xE49A6F02F737E65C),
    (("Ansi256", false, true), 0x2A360B4E2438EEEB),
    (("Ansi16", true, false), 0x55F999DABDC01B17),
    (("Ansi16", true, true), 0x73619CEA27F2D4C0),
    (("Ansi16", false, false), 0x1C6272E8F95B94DC),
    (("Ansi16", false, true), 0x45C131F6C005EF9D),
    (("Monochrome", true, false), 0xDF28F7F6FA592520),
    (("Monochrome", true, true), 0xE04874092CE75894),
    (("Monochrome", false, false), 0xA9E632998C311B9C),
    (("Monochrome", false, true), 0x9350A85AFC3C6C68),
];

/// Frames where only some cells change: single cells, runs at the start,
/// middle, and end of rows, several runs in one row, and changes back.
fn partial_update_frames() -> Vec<CellFrame> {
    use trend::{Cell, Color};
    let base = render(
        0,
        Viewport {
            width: 23,
            height: 7,
        },
        FitMode::Stretch,
        RenderOptions::default(),
    );
    let edits: [&[(usize, usize)]; 6] = [
        &[(0, 0)],
        &[(3, 4), (3, 5), (3, 6), (3, 14), (3, 15), (3, 22)],
        &[(6, 0), (6, 1), (6, 2), (6, 21), (6, 22)],
        &[(1, 10)],
        &[(2, 3), (2, 7), (2, 11), (2, 19)],
        &[],
    ];
    let mut frames = vec![base.clone()];
    let mut current = base.clone();
    for (step, edit) in edits.iter().enumerate() {
        for &(row, column) in *edit {
            let index = row * 23 + column;
            let shade = (step * 40 + column * 7) as u8;
            current.cells_mut()[index] = match step % 3 {
                0 => Cell::half_block(Color::rgb(shade, 255 - shade, 9), Color::rgb(3, shade, 200)),
                1 => Cell::with_glyph('x', Color::rgb(shade, shade, shade), Color::DEFAULT),
                _ => Cell::default(),
            };
        }
        frames.push(current.clone());
    }
    // Revert everything to the base in one frame.
    frames.push(base);
    frames
}

#[test]
fn partial_updates_match_recorded_baseline() {
    let frames = partial_update_frames();
    let mut actual = Vec::new();
    for mode in MODES {
        for unicode in [true, false] {
            for positioned in [false, true] {
                let mut presenter = AnsiPresenter::with_capabilities(TerminalCapabilities {
                    color: mode,
                    unicode,
                });
                let mut hash = Fnv::default();
                for frame in &frames {
                    let bytes = if positioned {
                        presenter.present_at(frame, 2, 9)
                    } else {
                        presenter.present(frame)
                    };
                    hash.write_u64(bytes.len() as u64);
                    hash.write(bytes);
                }
                actual.push(((format!("{mode:?}"), unicode, positioned), hash.finish()));
            }
        }
    }
    let expected: Vec<_> = PARTIAL_GOLDEN
        .iter()
        .map(|&((mode, unicode, positioned), hash)| ((mode.to_owned(), unicode, positioned), hash))
        .collect();
    check("PARTIAL_GOLDEN", &actual, &expected);
}
