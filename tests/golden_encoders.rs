//! Recorded baselines of the cell encoders' output.
//!
//! `Quadrant` and `Density` were recorded from M4, before the M5 fitting
//! work, so encoder optimizations must keep their cells identical.
//! `Sextant` and `Braille` were recorded when introduced in M5, to guard
//! them from then on.
//!
//! To record an intentional change, run with `TREND_RECORD_GOLDEN=1` and
//! `--nocapture`, review the diff, and paste the printed table.

mod common;

use common::{FILTERS, FITS, Fnv, MATTES, VIEWPORTS};
use trend::{
    Braille, CellAspect, CellEncoder, CellFrame, Density, DensityRamp, ImageSource, PixelBuffer,
    Quadrant, RasterSpec, Sextant,
};

/// Every pattern, filter, viewport, fit, and matte through `ImageSource`,
/// then the encoder; plus cells built from a few colors, which exercise ties.
fn encoder_hash(encoder: &dyn CellEncoder) -> u64 {
    let mut hash = Fnv::default();
    let mut pixels = PixelBuffer::new(1, 1).unwrap();
    let mut cells = CellFrame::default();
    for pattern in common::patterns() {
        for filter in FILTERS {
            for viewport in VIEWPORTS {
                for fit in FITS {
                    for matte in MATTES {
                        let spec = RasterSpec::new(viewport, encoder.grid(), CellAspect::default())
                            .unwrap();
                        ImageSource::new(pattern.view())
                            .with_sampling(filter)
                            .with_fit(fit)
                            .with_alpha_background(matte)
                            .render_into(&spec, &mut pixels)
                            .unwrap();
                        cells.resize(viewport.width, viewport.height).unwrap();
                        encoder.encode(pixels.view(), cells.view_mut()).unwrap();
                        hash.write_cells(cells.cells());
                    }
                }
            }
        }
    }

    // Few-color cells: every pixel drawn from a small palette, including
    // transparency, so equal errors and equal colors are common.
    let palette = [
        [0, 0, 0, 255],
        [255, 255, 255, 255],
        [200, 30, 90, 255],
        [201, 30, 90, 255],
        [12, 180, 64, 255],
        [0, 0, 0, 0],
        [90, 90, 90, 128],
    ];
    let spec = RasterSpec::new(
        trend::Viewport {
            width: 64,
            height: 32,
        },
        encoder.grid(),
        CellAspect::default(),
    )
    .unwrap();
    let mut pixels = PixelBuffer::new(spec.width(), spec.height()).unwrap();
    let mut state = 0x9E37_79B9_u32;
    for round in 0..4 {
        let colors = 2 + round;
        for pixel in pixels.pixels_mut().chunks_exact_mut(4) {
            state ^= state << 13;
            state ^= state >> 17;
            state ^= state << 5;
            pixel.copy_from_slice(&palette[state as usize % colors.min(palette.len())]);
        }
        cells.resize(64, 32).unwrap();
        encoder.encode(pixels.view(), cells.view_mut()).unwrap();
        hash.write_cells(cells.cells());
    }
    hash.finish()
}

const ENCODER_GOLDEN: [(&str, u64); 5] = [
    ("Quadrant", 0x14355D35F1CCB293),
    ("Density ascii", 0xD238A7E541573E10),
    ("Density shades colored", 0x79045CEA64672BB4),
    ("Sextant", 0x21A66AE504082163),
    ("Braille", 0x03DFBD5204FAE3A1),
];

#[test]
fn encoder_output_matches_recorded_baseline() {
    let encoders: [(&str, &dyn CellEncoder); 5] = [
        ("Quadrant", &Quadrant),
        ("Density ascii", &Density::ASCII),
        (
            "Density shades colored",
            &Density::new(DensityRamp::Shades, true),
        ),
        ("Sextant", &Sextant),
        ("Braille", &Braille),
    ];
    let actual: Vec<(&str, u64)> = encoders
        .iter()
        .map(|(name, encoder)| (*name, encoder_hash(*encoder)))
        .collect();
    if common::recording() {
        println!("ENCODER_GOLDEN:");
        for (name, hash) in &actual {
            println!("    ({name:?}, 0x{hash:016X}),");
        }
        return;
    }
    assert_eq!(actual, ENCODER_GOLDEN);
}
