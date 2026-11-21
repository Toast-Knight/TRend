use crate::cell::CellViewMut;
use crate::framebuffer::PixelFrameView;
use crate::geometry::SubcellGrid;

use super::fit::{POPCOUNT, Split, encode_blocks, fit_projected};
use super::{CellEncoder, ColorUse, EncodeError, EncoderRequirements, GlyphRepertoire, check_size};

/// Two columns by four rows per cell, drawn with Unicode Braille patterns
/// (U+2800–U+28FF): raised dots in the foreground color over the background
/// color.
///
/// Braille has the finest grid of any encoder, but its dots cover only a
/// small part of the cell, so it suits line art, plots, wireframes, and
/// text-like detail better than photographs, where [`crate::Sextant`] or
/// [`crate::Quadrant`] reproduce color more faithfully.
///
/// Fully visible cells use the same fast two-color split as
/// [`crate::Sextant`]. Because dots are small, the part with fewer pixels
/// becomes the dots and the larger part fills the cell; with four of each,
/// the bottom-right pixel's part is the background. Visible pixels of a
/// partly transparent cell become dots in their mean color. A uniform cell
/// is a space with a colored background.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct Braille;

/// Braille dot bit for each pixel of the 2×4 block, row-major. Dots 1–3 run
/// down the left column, 4–6 down the right, and 7 and 8 are the bottom row.
const DOT_BITS: [u32; 8] = [0, 3, 1, 4, 2, 5, 6, 7];

/// Glyph for an 8-bit foreground mask over the row-major block.
pub(crate) fn braille_glyph(mask: u32) -> char {
    if mask == 0 {
        return ' ';
    }
    let dots = DOT_BITS
        .iter()
        .enumerate()
        .fold(0, |dots, (pixel, bit)| dots | (mask >> pixel & 1) << bit);
    char::from_u32(0x2800 + dots).unwrap_or(' ')
}

/// The projected split, with the smaller part as the dots.
fn fit_dots(block: &[[u8; 3]; 8]) -> Split {
    let split = fit_projected::<8>(block);
    if POPCOUNT[split.mask as usize] > 4 {
        Split {
            mask: split.mask ^ 0xFF,
            foreground: split.background,
            background: split.foreground,
        }
    } else {
        split
    }
}

impl CellEncoder for Braille {
    fn grid(&self) -> SubcellGrid {
        SubcellGrid::BRAILLE
    }

    fn requirements(&self) -> EncoderRequirements {
        EncoderRequirements::new(GlyphRepertoire::Braille, ColorUse::ForegroundAndBackground)
    }

    fn encode(
        &self,
        pixels: PixelFrameView<'_>,
        mut cells: CellViewMut<'_>,
    ) -> Result<(), EncodeError> {
        check_size(self.grid(), &pixels, &cells)?;
        encode_blocks::<8, 2>(&pixels, &mut cells, braille_glyph, fit_dots);
        Ok(())
    }
}
