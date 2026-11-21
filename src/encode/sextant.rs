use crate::cell::CellViewMut;
use crate::framebuffer::PixelFrameView;
use crate::geometry::SubcellGrid;

use super::fit::{encode_blocks, fit_projected};
use super::{CellEncoder, ColorUse, EncodeError, EncoderRequirements, GlyphRepertoire, check_size};

/// Two columns by three rows per cell, drawn with Unicode sextant block
/// mosaics (U+1FB00–U+1FB3B, plus `▌`, `▐`, and `█` from the block
/// elements) using two colors per cell.
///
/// It resolves 1.5× the vertical detail of [`crate::Quadrant`]. Sextants are
/// newer than block elements (Unicode 13, 2020): check that the terminal's
/// font draws them, as most current terminals do natively.
///
/// Transparency follows [`crate::Quadrant`]. Fully visible cells use a fast
/// two-color split that is within a fraction of a percent of the best
/// possible (see `docs/benchmarks.md`); the bottom-right pixel is always in
/// the background, and a uniform cell becomes a space with a colored
/// background.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct Sextant;

/// Glyph for each 6-bit foreground mask; bit `i` is sextant position `i + 1`
/// (row-major: 1 2 / 3 4 / 5 6).
pub(crate) fn sextant_glyph(mask: u32) -> char {
    match mask {
        0 => ' ',
        0b010101 => '▌',
        0b101010 => '▐',
        0b111111 => '█',
        _ => {
            // U+1FB00 onward lists every other mask in order.
            let skipped = u32::from(mask > 0b010101) + u32::from(mask > 0b101010);
            char::from_u32(0x1FB00 + mask - 1 - skipped).unwrap_or(' ')
        }
    }
}

impl CellEncoder for Sextant {
    fn grid(&self) -> SubcellGrid {
        SubcellGrid::SEXTANT
    }

    fn requirements(&self) -> EncoderRequirements {
        EncoderRequirements::new(GlyphRepertoire::Sextants, ColorUse::ForegroundAndBackground)
    }

    fn encode(
        &self,
        pixels: PixelFrameView<'_>,
        mut cells: CellViewMut<'_>,
    ) -> Result<(), EncodeError> {
        check_size(self.grid(), &pixels, &cells)?;
        encode_blocks::<6, 2>(&pixels, &mut cells, sextant_glyph, fit_projected::<6>);
        Ok(())
    }
}
