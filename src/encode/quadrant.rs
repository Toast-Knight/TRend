use crate::cell::CellViewMut;
use crate::framebuffer::PixelFrameView;
use crate::geometry::SubcellGrid;

use super::fit::{encode_blocks, fit_exact};
use super::{CellEncoder, ColorUse, EncodeError, EncoderRequirements, GlyphRepertoire, check_size};

/// Glyph for each 4-bit foreground mask: bit 0 top-left, bit 1 top-right,
/// bit 2 bottom-left, bit 3 bottom-right.
const GLYPHS: [char; 16] = [
    ' ', '▘', '▝', '▀', '▖', '▌', '▞', '▛', '▗', '▚', '▐', '▜', '▄', '▙', '▟', '█',
];

/// Two columns by two rows per cell, drawn with Unicode quadrant block
/// elements (U+2580–U+259F) using two colors per cell.
///
/// - All four pixels transparent: the default cell.
/// - Some transparent: visible pixels form the foreground (their mean color)
///   over the terminal default.
/// - All visible: the two-color split with the lowest squared RGB error; the
///   first minimum wins, so a uniform cell becomes a space with a colored
///   background.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct Quadrant;

impl CellEncoder for Quadrant {
    fn grid(&self) -> SubcellGrid {
        SubcellGrid::QUADRANT
    }

    fn requirements(&self) -> EncoderRequirements {
        EncoderRequirements::new(
            GlyphRepertoire::BlockElements,
            ColorUse::ForegroundAndBackground,
        )
    }

    fn encode(
        &self,
        pixels: PixelFrameView<'_>,
        mut cells: CellViewMut<'_>,
    ) -> Result<(), EncodeError> {
        check_size(self.grid(), &pixels, &cells)?;
        encode_blocks::<4, 2>(&pixels, &mut cells, |mask| GLYPHS[mask as usize], fit_exact);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cell::{Cell, CellFrame, Color};
    use crate::pixel_buffer::PixelBuffer;

    /// The M1–M4 implementation, kept as the reference the fast path must
    /// match exactly.
    fn encode_cell(samples: [Option<Color>; 4]) -> Cell {
        let visible_mask = samples
            .iter()
            .enumerate()
            .filter(|(_, sample)| sample.is_some())
            .fold(0_usize, |mask, (bit, _)| mask | 1 << bit);
        if visible_mask == 0 {
            return Cell::default();
        }
        if visible_mask != 0b1111 {
            let foreground = legacy_mean(&samples, visible_mask).0;
            return Cell::with_glyph(GLYPHS[visible_mask], foreground, Color::DEFAULT);
        }
        let (uniform, uniform_error) = legacy_mean(&samples, 0b1111);
        let mut best_error = uniform_error;
        let mut best = Cell::with_glyph(' ', uniform, uniform);
        for (mask, &glyph) in GLYPHS.iter().enumerate().take(8).skip(1) {
            let (foreground, foreground_error) = legacy_mean(&samples, mask);
            let (background, background_error) = legacy_mean(&samples, !mask & 0b1111);
            if foreground_error + background_error < best_error {
                best_error = foreground_error + background_error;
                best = Cell::with_glyph(glyph, foreground, background);
            }
        }
        best
    }

    fn legacy_mean(samples: &[Option<Color>; 4], mask: usize) -> (Color, u32) {
        let selected = || {
            samples
                .iter()
                .enumerate()
                .filter(move |(bit, _)| mask & (1 << bit) != 0)
                .filter_map(|(_, sample)| *sample)
        };
        let mut sums = [0_u32; 3];
        let mut count = 0_u32;
        for color in selected() {
            sums[0] += u32::from(color.red);
            sums[1] += u32::from(color.green);
            sums[2] += u32::from(color.blue);
            count += 1;
        }
        let channel = |sum: u32| ((sum + count / 2) / count) as u8;
        let average = Color::rgb(channel(sums[0]), channel(sums[1]), channel(sums[2]));
        let error = selected()
            .map(|color| {
                let delta =
                    |left: u8, right: u8| (i32::from(left) - i32::from(right)).unsigned_abs();
                delta(color.red, average.red).pow(2)
                    + delta(color.green, average.green).pow(2)
                    + delta(color.blue, average.blue).pow(2)
            })
            .sum();
        (average, error)
    }

    /// Encodes one cell through the real encoder.
    fn encode_one(samples: [Option<Color>; 4]) -> Cell {
        let mut pixels = PixelBuffer::new(2, 2).unwrap();
        for (index, sample) in samples.iter().enumerate() {
            let rgba = match sample {
                Some(color) => [color.red, color.green, color.blue, 255],
                None => [0; 4],
            };
            pixels.pixels_mut()[index * 4..index * 4 + 4].copy_from_slice(&rgba);
        }
        let mut cells = CellFrame::default();
        cells.resize(1, 1).unwrap();
        Quadrant.encode(pixels.view(), cells.view_mut()).unwrap();
        cells.cells()[0]
    }

    #[test]
    fn fast_path_matches_the_original_implementation() {
        let mut state = 0x1234_5678_u32;
        let mut next = move || {
            state ^= state << 13;
            state ^= state >> 17;
            state ^= state << 5;
            state
        };
        // Few distinct colors (ties everywhere), then arbitrary colors.
        for palette_size in [1_u32, 2, 3, 4, 256, u32::MAX] {
            for _ in 0..50_000 {
                let samples: [Option<Color>; 4] = core::array::from_fn(|_| {
                    let value = next() % palette_size.max(1);
                    let value = if palette_size == u32::MAX {
                        next()
                    } else {
                        value.wrapping_mul(0x9E37_79B9)
                    };
                    (value % 11 != 0 || palette_size < 4)
                        .then(|| Color::rgb(value as u8, (value >> 8) as u8, (value >> 16) as u8))
                });
                assert_eq!(encode_one(samples), encode_cell(samples), "{samples:?}");
            }
        }
        // Every combination of two channel levels in every pixel.
        for bits in 0..1_u32 << 12 {
            let samples: [Option<Color>; 4] = core::array::from_fn(|pixel| {
                let level = |channel: u32| {
                    if bits >> (pixel as u32 * 3 + channel) & 1 == 1 {
                        200
                    } else {
                        10
                    }
                };
                Some(Color::rgb(level(0), level(1), level(2)))
            });
            assert_eq!(encode_one(samples), encode_cell(samples));
        }
    }

    const RED: Color = Color::rgb(255, 0, 0);
    const BLUE: Color = Color::rgb(0, 0, 255);

    #[test]
    fn glyph_table_matches_unicode_quadrant_semantics() {
        // Each glyph's filled quadrants must equal its mask.
        let expected = [
            (0b0001, '▘'),
            (0b0010, '▝'),
            (0b0100, '▖'),
            (0b1000, '▗'),
            (0b0011, '▀'),
            (0b1100, '▄'),
            (0b0101, '▌'),
            (0b1010, '▐'),
            (0b0110, '▞'),
            (0b1001, '▚'),
            (0b0111, '▛'),
            (0b1011, '▜'),
            (0b1101, '▙'),
            (0b1110, '▟'),
            (0b1111, '█'),
        ];
        for (mask, glyph) in expected {
            assert_eq!(GLYPHS[mask], glyph, "mask {mask:04b}");
        }
    }

    #[test]
    fn diagonal_split_uses_both_colors() {
        let cell = encode_cell([Some(RED), Some(BLUE), Some(BLUE), Some(RED)]);
        assert_eq!(cell, Cell::with_glyph('▞', BLUE, RED));
    }

    #[test]
    fn single_odd_pixel_is_the_foreground_of_its_complement_mask() {
        let cell = encode_cell([Some(RED), Some(RED), Some(RED), Some(BLUE)]);
        assert_eq!(cell, Cell::with_glyph('▛', RED, BLUE));
    }

    #[test]
    fn uniform_cell_is_a_colored_space() {
        let cell = encode_cell([Some(BLUE); 4]);
        assert_eq!(cell, Cell::with_glyph(' ', BLUE, BLUE));
    }

    #[test]
    fn transparent_pixels_become_terminal_default_background() {
        assert_eq!(encode_cell([None; 4]), Cell::default());
        assert_eq!(
            encode_cell([None, Some(RED), None, Some(Color::rgb(255, 0, 2))]),
            Cell::with_glyph('▐', Color::rgb(255, 0, 1), Color::DEFAULT)
        );
    }
}
