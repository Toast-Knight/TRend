//! Recorded baseline of the existing `Renderer` output.
//!
//! The constants below were recorded from the renderer as it existed before
//! the M1 pipeline work. They guard the old path against any behavior change
//! while the new pipeline is introduced alongside it and, later, while
//! `Renderer` is reimplemented on top of it.
//!
//! To record an intentional change, run with `TREND_RECORD_GOLDEN=1` and
//! `--nocapture`, review the diff, and paste the printed table.

mod common;

use common::{FILTERS, Fnv, VIEWPORTS};
use trend::{
    Cell, CellFrame, Color, FitMode, RenderOptions, RenderRequest, RenderStatus, Renderer,
    SamplingFilter,
};

/// FNV-1a of every case for one (pattern, filter) pair, in `common::cases()` order.
const RENDER_GOLDEN: [((&str, &str), u64); 15] = [
    (("gradient", "Nearest"), 0x10AA7CFF8A20C057),
    (("gradient", "Bilinear"), 0x76C5D28A4D1F4C28),
    (("gradient", "Area"), 0x164DD0A6B43349E4),
    (("alpha-checker", "Nearest"), 0x49EC079A535CBBDE),
    (("alpha-checker", "Bilinear"), 0x03B75144E2658ADE),
    (("alpha-checker", "Area"), 0x382BAC4E6149D6A1),
    (("tall", "Nearest"), 0x4C178C24B86EB7F0),
    (("tall", "Bilinear"), 0xEB891EAA8E91DBE8),
    (("tall", "Area"), 0x4A2A6619C5644D9D),
    (("wide", "Nearest"), 0xB97CD409B7B9E24E),
    (("wide", "Bilinear"), 0x08C6DD10AA7E2DD8),
    (("wide", "Area"), 0x53D1ECF83840932C),
    (("noise", "Nearest"), 0x4BA1107B6668DEA8),
    (("noise", "Bilinear"), 0x35B3F7F208AACBDB),
    (("noise", "Area"), 0x7388FED815A249BD),
];

/// FNV-1a of cancellable renders stopped after 1, 2, 5, and 100 checks.
const CANCEL_GOLDEN: [(&str, u64); 5] = [
    ("gradient", 0x98AF9B6DE3BFAFC8),
    ("alpha-checker", 0xB36A3A662AAA9A4C),
    ("tall", 0xA9FA5CFCE5BEDB3A),
    ("wide", 0x4BED7DCAE47602F3),
    ("noise", 0x565230EDFACF5111),
];

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
fn renderer_output_matches_recorded_baseline() {
    let patterns = common::patterns();
    let renderer = Renderer::new();
    let mut frame = CellFrame::default();
    let mut hashes = vec![Fnv::default(); patterns.len() * FILTERS.len()];

    for case in common::cases() {
        renderer
            .render_with_options_into(
                patterns[case.pattern].view(),
                case.request(),
                case.options(),
                &mut frame,
            )
            .unwrap();
        let hash = &mut hashes[case.pattern * FILTERS.len() + case.filter];
        hash.write_u64(frame.generation());
        hash.write(&frame.width().to_le_bytes());
        hash.write(&frame.height().to_le_bytes());
        hash.write_cells(frame.cells());
    }

    let actual: Vec<_> = hashes
        .iter()
        .enumerate()
        .map(|(index, hash)| {
            (
                (
                    patterns[index / FILTERS.len()].name,
                    format!("{:?}", FILTERS[index % FILTERS.len()]),
                ),
                hash.finish(),
            )
        })
        .collect();
    let expected: Vec<_> = RENDER_GOLDEN
        .iter()
        .map(|&((pattern, filter), hash)| ((pattern, filter.to_owned()), hash))
        .collect();
    check("RENDER_GOLDEN", &actual, &expected);
}

#[test]
fn cancellable_renderer_matches_recorded_baseline() {
    let patterns = common::patterns();
    let sentinel = Cell::half_block(Color::rgb(1, 2, 3), Color::rgb(4, 5, 6));
    let viewport = VIEWPORTS[2];
    let request = RenderRequest {
        viewport,
        fit: FitMode::Cover,
        generation: 9,
    };
    let options = RenderOptions {
        sampling: SamplingFilter::Area,
        dither_strength: 8,
        ..RenderOptions::default()
    };

    let actual: Vec<_> = patterns
        .iter()
        .map(|pattern| {
            let mut hash = Fnv::default();
            for stop_after in [1_u32, 2, 5, 100] {
                let mut output =
                    vec![sentinel; usize::from(viewport.width) * usize::from(viewport.height)];
                let mut checks = 0;
                let status = Renderer::new()
                    .render_to_slice_cancellable(
                        pattern.view(),
                        request,
                        options,
                        &mut output,
                        || {
                            checks += 1;
                            checks > stop_after
                        },
                    )
                    .unwrap();
                hash.write(&[u8::from(status == RenderStatus::Complete)]);
                hash.write_cells(&output);
            }
            (pattern.name, hash.finish())
        })
        .collect();
    check("CANCEL_GOLDEN", &actual, &CANCEL_GOLDEN);
}
