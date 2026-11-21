//! M4: opt-in presenter output (combined colors, synchronized updates),
//! statistics, invalidation, and region presentation.
//!
//! Default output is pinned byte for byte by the golden ANSI tests. These
//! tests check the new behavior by its effect on a screen model: every
//! option and every region schedule must leave the terminal showing the same
//! thing as the default full presentation.

mod common;

use common::screen::{Pen, Screen};
use trend::{
    AnsiPresenter, Cell, CellFrame, CellRect, Color, OutputOptions, PresentStats, RegionError,
    TerminalCapabilities, TerminalColorMode,
};

const MODES: [TerminalColorMode; 4] = [
    TerminalColorMode::TrueColor,
    TerminalColorMode::Ansi256,
    TerminalColorMode::Ansi16,
    TerminalColorMode::Monochrome,
];

fn capabilities(color: TerminalColorMode) -> TerminalCapabilities {
    TerminalCapabilities {
        color,
        unicode: true,
    }
}

fn all_options() -> [OutputOptions; 6] {
    [
        OutputOptions::new(),
        OutputOptions::new().with_combined_colors(true),
        OutputOptions::new().with_synchronized_updates(true),
        OutputOptions::new()
            .with_combined_colors(true)
            .with_synchronized_updates(true),
        OutputOptions::new().with_compact_indexed_colors(true),
        OutputOptions::new()
            .with_compact_indexed_colors(true)
            .with_combined_colors(true)
            .with_synchronized_updates(true),
    ]
}

/// Deterministic frames that change partially from one to the next, with
/// default colors, repeated colors, and several glyphs mixed in.
fn frames(width: u16, height: u16, count: u32) -> Vec<CellFrame> {
    let mut state = 0x2545_F491_u32;
    let mut next = move || {
        state ^= state << 13;
        state ^= state >> 17;
        state ^= state << 5;
        state
    };
    let palette = [
        Color::DEFAULT,
        Color::rgb(0, 0, 0),
        Color::rgb(255, 255, 255),
        Color::rgb(200, 30, 90),
        Color::rgb(12, 180, 64),
    ];
    let mut frame = CellFrame::default();
    frame.resize(width, height).unwrap();
    let mut frames = Vec::new();
    for index in 0..count {
        for cell in frame.cells_mut() {
            // The first frame fills everything; later frames change a third.
            if index > 0 && next() % 3 != 0 {
                continue;
            }
            let random = next();
            let color = |bits: u32| {
                if bits.is_multiple_of(4) {
                    palette[(bits as usize / 4) % palette.len()]
                } else {
                    Color::rgb(bits as u8, (bits >> 8) as u8, (bits >> 16) as u8)
                }
            };
            let glyph = ['▀', '▄', ' ', 'x', '█'][(random % 5) as usize];
            *cell = Cell::with_glyph(glyph, color(next()), color(next()));
        }
        frames.push(frame.clone());
    }
    frames
}

fn screen_after(outputs: &[Vec<u8>]) -> Screen {
    let mut screen = Screen::new(40, 16);
    for output in outputs {
        screen.apply(output);
        assert!(!screen.synchronized, "every frame closes its update");
    }
    screen
}

#[test]
fn every_option_shows_the_same_screen_as_the_default() {
    for mode in MODES {
        let frames = frames(23, 9, 6);
        for positioned in [false, true] {
            let run = |options: OutputOptions| {
                let mut presenter = AnsiPresenter::with_options(capabilities(mode), options);
                frames
                    .iter()
                    .map(|frame| {
                        if positioned {
                            presenter.present_at(frame, 4, 7).to_vec()
                        } else {
                            presenter.present(frame).to_vec()
                        }
                    })
                    .collect::<Vec<_>>()
            };
            let expected = run(OutputOptions::default());
            for options in all_options() {
                let outputs = run(options);
                assert_eq!(
                    screen_after(&outputs),
                    screen_after(&expected),
                    "{mode:?} {options:?} positioned {positioned}"
                );
                if options == OutputOptions::default() {
                    assert_eq!(outputs, expected);
                }
            }
        }
    }
}

#[test]
fn default_options_are_the_original_output() {
    assert_eq!(OutputOptions::default(), OutputOptions::new());
    let frames = frames(11, 5, 3);
    let mut original = AnsiPresenter::with_capabilities(capabilities(TerminalColorMode::TrueColor));
    let mut explicit = AnsiPresenter::with_options(
        capabilities(TerminalColorMode::TrueColor),
        OutputOptions::default(),
    );
    for frame in &frames {
        assert_eq!(original.present(frame), explicit.present(frame));
    }
}

#[test]
fn combined_colors_merge_both_changes_into_one_sequence() {
    let mut frame = CellFrame::default();
    frame.resize(2, 1).unwrap();
    frame.cells_mut()[0] = Cell::half_block(Color::rgb(1, 2, 3), Color::rgb(4, 5, 6));
    // Only the foreground changes for the second cell.
    frame.cells_mut()[1] = Cell::half_block(Color::DEFAULT, Color::rgb(4, 5, 6));

    let present = |options| {
        let mut presenter =
            AnsiPresenter::with_options(capabilities(TerminalColorMode::TrueColor), options);
        let bytes = presenter.present_at(&frame, 1, 1).to_vec();
        (String::from_utf8(bytes).unwrap(), presenter.last_stats())
    };
    let (separate, separate_stats) = present(OutputOptions::default());
    let (combined, combined_stats) = present(OutputOptions::new().with_combined_colors(true));
    assert_eq!(
        separate,
        "\x1b[1;1H\x1b[38;2;1;2;3m\x1b[48;2;4;5;6m▀\x1b[39m▀\x1b[0m"
    );
    assert_eq!(
        combined,
        "\x1b[1;1H\x1b[38;2;1;2;3;48;2;4;5;6m▀\x1b[39m▀\x1b[0m"
    );
    assert_eq!(separate_stats.color_sequences, 3);
    assert_eq!(combined_stats.color_sequences, 2);

    // Every mode, including defaults on one side.
    let mut both = CellFrame::default();
    both.resize(1, 1).unwrap();
    for (mode, cell, expected) in [
        (
            TerminalColorMode::Ansi256,
            Cell::half_block(Color::rgb(255, 0, 0), Color::DEFAULT),
            "\x1b[38;5;196;49m",
        ),
        (
            TerminalColorMode::Ansi16,
            Cell::half_block(Color::rgb(255, 255, 255), Color::rgb(0, 0, 0)),
            "\x1b[97;40m",
        ),
    ] {
        both.cells_mut()[0] = cell;
        let mut presenter = AnsiPresenter::with_options(
            capabilities(mode),
            OutputOptions::new().with_combined_colors(true),
        );
        let output = String::from_utf8(presenter.present_at(&both, 1, 1).to_vec()).unwrap();
        assert!(output.contains(expected), "{mode:?}: {output:?}");
    }
}

#[test]
fn synchronized_updates_wrap_output_and_skip_empty_frames() {
    let frames = frames(8, 3, 2);
    let options = OutputOptions::new().with_synchronized_updates(true);
    let mut plain = AnsiPresenter::with_capabilities(capabilities(TerminalColorMode::TrueColor));
    let mut synchronized =
        AnsiPresenter::with_options(capabilities(TerminalColorMode::TrueColor), options);
    for frame in &frames {
        let expected = plain.present_at(frame, 2, 2).to_vec();
        let wrapped = synchronized.present_at(frame, 2, 2).to_vec();
        assert_eq!(
            wrapped,
            [b"\x1b[?2026h", &expected[..], b"\x1b[?2026l"].concat()
        );
        assert_eq!(synchronized.last_stats().bytes, wrapped.len());
    }
    // An unchanged frame writes nothing at all, not an empty update.
    assert!(synchronized.present_at(&frames[1], 2, 2).is_empty());
    assert!(
        !synchronized.present(&frames[1]).is_empty(),
        "moved: redraws"
    );
    assert!(synchronized.present(&frames[1]).is_empty());
    assert_eq!(synchronized.last_stats(), PresentStats::default());
}

#[test]
fn options_can_change_between_frames() {
    let frames = frames(9, 4, 12);
    let mut reference = AnsiPresenter::with_capabilities(capabilities(TerminalColorMode::Ansi256));
    let mut switching = AnsiPresenter::with_capabilities(capabilities(TerminalColorMode::Ansi256));
    let mut expected = Vec::new();
    let mut outputs = Vec::new();
    for (index, frame) in frames.iter().enumerate() {
        switching.set_options(all_options()[index % 6]);
        assert_eq!(switching.options(), all_options()[index % 6]);
        expected.push(reference.present(frame).to_vec());
        outputs.push(switching.present(frame).to_vec());
    }
    assert_eq!(screen_after(&outputs), screen_after(&expected));
    assert_eq!(
        switching.capabilities(),
        capabilities(TerminalColorMode::Ansi256)
    );
}

#[test]
fn stats_describe_the_output() {
    for mode in MODES {
        for options in all_options() {
            let mut presenter = AnsiPresenter::with_options(capabilities(mode), options);
            for frame in frames(17, 6, 4) {
                let output =
                    String::from_utf8(presenter.present_at(&frame, 1, 1).to_vec()).unwrap();
                let stats = presenter.last_stats();
                assert_eq!(stats.bytes, output.len());
                assert_eq!(stats.runs, output.matches('H').count());
                // Each written cell is exactly one glyph outside escapes.
                let mut glyphs = 0;
                let mut in_escape = false;
                for ch in output.chars() {
                    match (in_escape, ch) {
                        (false, '\x1b') => in_escape = true,
                        (false, _) => glyphs += 1,
                        (true, c) if c.is_ascii_alphabetic() => in_escape = false,
                        _ => {}
                    }
                }
                assert_eq!(stats.cells, glyphs, "{mode:?} {options:?}");
                // Color sequences are all SGRs but the trailing reset.
                let resets = usize::from(!output.is_empty());
                assert_eq!(
                    stats.color_sequences,
                    output.matches('m').count() - resets,
                    "{mode:?} {options:?}"
                );
            }
        }
    }
}

#[test]
fn invalidate_redraws_everything_the_presenter_covers() {
    let frames = frames(12, 5, 2);
    let mut presenter =
        AnsiPresenter::with_capabilities(capabilities(TerminalColorMode::TrueColor));
    presenter.present_at(&frames[0], 3, 3);
    assert!(presenter.present_at(&frames[0], 3, 3).is_empty());
    presenter.invalidate();
    let redrawn = presenter.present_at(&frames[0], 3, 3).to_vec();
    let mut fresh = AnsiPresenter::with_capabilities(capabilities(TerminalColorMode::TrueColor));
    assert_eq!(redrawn, fresh.present_at(&frames[0], 3, 3));
    assert_eq!(presenter.last_stats().cells, 12 * 5);

    // `present` redraws without clearing the screen: the size didn't change.
    let mut presenter =
        AnsiPresenter::with_capabilities(capabilities(TerminalColorMode::TrueColor));
    let first = presenter.present(&frames[1]).to_vec();
    presenter.invalidate();
    assert_eq!(presenter.present(&frames[1]), &first[b"\x1b[2J".len()..]);
}

#[test]
fn regions_tiling_a_frame_show_the_same_screen_as_presenting_it_whole() {
    let tilings: [&[CellRect]; 3] = [
        &[CellRect::new(0, 0, 13, 7)],
        &[CellRect::new(0, 0, 13, 3), CellRect::new(0, 3, 13, 4)],
        &[
            CellRect::new(0, 0, 5, 7),
            CellRect::new(5, 0, 8, 2),
            CellRect::new(5, 2, 8, 5),
        ],
    ];
    for mode in MODES {
        for options in all_options() {
            let frames = frames(13, 7, 5);
            let mut whole = AnsiPresenter::with_options(capabilities(mode), options);
            let expected: Vec<_> = frames
                .iter()
                .map(|frame| whole.present_at(frame, 5, 9).to_vec())
                .collect();
            for tiling in tilings {
                let mut presenter = AnsiPresenter::with_options(capabilities(mode), options);
                let mut outputs = Vec::new();
                for frame in &frames {
                    for &region in tiling {
                        outputs.push(
                            presenter
                                .present_region_at(frame.view(), region, 5, 9)
                                .unwrap()
                                .to_vec(),
                        );
                        let (foreground, background) = screen_after(&outputs).pen();
                        assert_eq!((foreground, background), (Pen::Default, Pen::Default));
                    }
                }
                assert_eq!(
                    screen_after(&outputs),
                    screen_after(&expected),
                    "{mode:?} {options:?} {tiling:?}"
                );
                // Everything has been presented: nothing is left to write.
                let last = frames.last().unwrap();
                assert!(presenter.present_at(last, 5, 9).is_empty());
            }
        }
    }
}

#[test]
fn a_region_writes_only_its_own_cells() {
    let frames = frames(10, 6, 2);
    let mut presenter =
        AnsiPresenter::with_capabilities(capabilities(TerminalColorMode::TrueColor));
    presenter.present_at(&frames[0], 1, 1);

    // Present only rows 2..4 of the next frame.
    let region = CellRect::new(0, 2, 10, 2);
    let output = presenter
        .present_region_at(frames[1].view(), region, 1, 1)
        .unwrap()
        .to_vec();
    let mut screen = Screen::new(10, 6);
    screen.apply(&output);
    for row in 1..=6 {
        let written = screen.text(row) != " ".repeat(10);
        if !(3..=4).contains(&row) {
            assert!(!written, "row {row} was written");
        }
    }
    let changed_in_region = (2..4)
        .flat_map(|row| (0..10).map(move |column| row * 10 + column))
        .filter(|&index| frames[0].cells()[index] != frames[1].cells()[index])
        .count();
    assert_eq!(presenter.last_stats().cells, changed_in_region);

    // The rest of the frame is still pending, and only the rest.
    presenter.present_at(&frames[1], 1, 1);
    let changed_outside = frames[0]
        .cells()
        .iter()
        .zip(frames[1].cells())
        .enumerate()
        .filter(|(index, (before, after))| before != after && !(20..40).contains(index))
        .count();
    assert_eq!(presenter.last_stats().cells, changed_outside);
}

#[test]
fn region_presentation_forgets_the_screen_when_the_frame_moves_or_resizes() {
    let frames = frames(8, 4, 1);
    let mut presenter =
        AnsiPresenter::with_capabilities(capabilities(TerminalColorMode::TrueColor));
    let region = CellRect::new(1, 1, 3, 2);
    presenter
        .present_region_at(frames[0].view(), region, 1, 1)
        .unwrap();
    assert!(
        presenter
            .present_region_at(frames[0].view(), region, 1, 1)
            .unwrap()
            .is_empty()
    );
    // Moved: the region is redrawn.
    presenter
        .present_region_at(frames[0].view(), region, 2, 1)
        .unwrap();
    assert_eq!(presenter.last_stats().cells, 6);
    // A different frame size: redrawn too.
    presenter
        .present_region_at(
            frames[0].view().sub(CellRect::new(0, 0, 8, 3)).unwrap(),
            region,
            2,
            1,
        )
        .unwrap();
    assert_eq!(presenter.last_stats().cells, 6);
}

#[test]
fn region_presentation_rejects_regions_outside_the_frame() {
    let frames = frames(8, 4, 1);
    let mut presenter =
        AnsiPresenter::with_capabilities(capabilities(TerminalColorMode::TrueColor));
    assert!(matches!(
        presenter.present_region_at(frames[0].view(), CellRect::new(6, 0, 3, 1), 1, 1),
        Err(RegionError::OutOfBounds)
    ));
    // Nothing was remembered, so a later full presentation draws everything.
    presenter.present_at(&frames[0], 1, 1);
    assert_eq!(presenter.last_stats().cells, 32);
}

#[test]
fn views_present_like_the_frames_they_show() {
    let frames = frames(12, 6, 3);
    let region = CellRect::new(2, 1, 7, 4);
    let mut from_view = AnsiPresenter::with_capabilities(capabilities(TerminalColorMode::Ansi16));
    let mut from_copy = AnsiPresenter::with_capabilities(capabilities(TerminalColorMode::Ansi16));
    for frame in &frames {
        let view = frame.view().sub(region).unwrap();
        let mut copy = CellFrame::default();
        copy.resize(region.width, region.height).unwrap();
        for row in 0..region.height {
            copy.view_mut().row_mut(row).copy_from_slice(view.row(row));
        }
        assert_eq!(
            from_view.present_view_at(view, 3, 4),
            from_copy.present_at(&copy, 3, 4)
        );
    }
}

/// Smooth content: neighboring colors share palette entries.
fn gradient(width: u16, height: u16, shift: u32) -> CellFrame {
    let mut frame = CellFrame::default();
    frame.resize(width, height).unwrap();
    let row_width = usize::from(width);
    for (index, cell) in frame.cells_mut().iter_mut().enumerate() {
        let (x, y) = ((index % row_width) as u32, (index / row_width) as u32);
        let color = |y: u32| Color::rgb((x * 3 + shift) as u8, (y * 5) as u8, 90);
        *cell = Cell::half_block(color(2 * y), color(2 * y + 1));
    }
    frame
}

#[test]
fn compact_indexed_colors_shrink_indexed_output_only() {
    let frames = [gradient(80, 24, 0), gradient(80, 24, 7)];
    for mode in MODES {
        let run = |options: OutputOptions| {
            let mut presenter = AnsiPresenter::with_options(capabilities(mode), options);
            frames
                .iter()
                .map(|frame| presenter.present_at(frame, 1, 1).to_vec())
                .collect::<Vec<_>>()
        };
        let plain = run(OutputOptions::new());
        let compact = run(OutputOptions::new().with_compact_indexed_colors(true));
        assert_eq!(screen_after(&compact), screen_after(&plain), "{mode:?}");
        let (plain_bytes, compact_bytes) = (
            plain.iter().map(Vec::len).sum::<usize>(),
            compact.iter().map(Vec::len).sum::<usize>(),
        );
        match mode {
            TerminalColorMode::Ansi256 | TerminalColorMode::Ansi16 => assert!(
                compact_bytes * 2 < plain_bytes,
                "{mode:?}: {compact_bytes} vs {plain_bytes}"
            ),
            _ => assert_eq!(compact, plain, "{mode:?}"),
        }
    }
}
