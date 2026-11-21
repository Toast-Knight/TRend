use crate::cell::{Cell, CellViewMut, Color};
use crate::framebuffer::PixelFrameView;
use crate::geometry::SubcellGrid;

use super::{
    CellEncoder, ColorUse, EncodeError, EncoderRequirements, GlyphRepertoire, check_size,
    visible_color,
};
use crate::framebuffer::{decode, with_decoder};

/// One column by two rows per cell, drawn with `▀` and `▄`.
///
/// Produces exactly the cells of the existing [`crate::Renderer`] for the
/// same pixels: both halves visible → `▀` with top foreground and bottom
/// background; one half visible → `▀` or `▄` over the terminal default;
/// neither → the default cell.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct HalfBlock;

impl CellEncoder for HalfBlock {
    fn grid(&self) -> SubcellGrid {
        SubcellGrid::HALF_BLOCK
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
        with_decoder!(pixels, D => {
            for cell_y in 0..cells.height() {
                let y = u32::from(cell_y) * 2;
                let (Some(upper), Some(lower)) = (pixels.row(y), pixels.row(y + 1)) else {
                    continue;
                };
                let row = cells.row_mut(cell_y);
                for ((cell, upper), lower) in row
                    .iter_mut()
                    .zip(upper.chunks_exact(4))
                    .zip(lower.chunks_exact(4))
                {
                    *cell = half_block_cell(
                        visible_color(decode::<D>(upper)),
                        visible_color(decode::<D>(lower)),
                    );
                }
            }
        });
        Ok(())
    }
}

/// The half-block cell for an upper and a lower pixel (`None` = no content).
/// Shared with the legacy [`crate::Renderer`], so both always agree.
#[inline]
pub(crate) fn half_block_cell(upper: Option<Color>, lower: Option<Color>) -> Cell {
    match (upper, lower) {
        (Some(foreground), Some(background)) => Cell::half_block(foreground, background),
        (Some(foreground), None) => Cell::upper_half(foreground),
        (None, Some(foreground)) => Cell::lower_half(foreground),
        (None, None) => Cell::default(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cell::CellFrame;
    use crate::pixel_buffer::PixelBuffer;

    #[test]
    fn encodes_each_visibility_combination() {
        let mut pixels = PixelBuffer::new(4, 2).unwrap();
        pixels.pixels_mut().copy_from_slice(&[
            255, 0, 0, 255, 0, 255, 0, 255, 0, 0, 0, 0, 0, 0, 0, 0, //
            0, 0, 255, 255, 0, 0, 0, 0, 9, 8, 7, 1, 0, 0, 0, 0,
        ]);
        let mut cells = CellFrame::default();
        cells.resize(4, 1).unwrap();

        HalfBlock.encode(pixels.view(), cells.view_mut()).unwrap();

        assert_eq!(
            cells.cells(),
            &[
                Cell::half_block(Color::rgb(255, 0, 0), Color::rgb(0, 0, 255)),
                Cell::upper_half(Color::rgb(0, 255, 0)),
                Cell::lower_half(Color::rgb(9, 8, 7)),
                Cell::default(),
            ]
        );
    }

    #[test]
    fn rejects_pixels_that_do_not_match_the_grid() {
        let pixels = PixelBuffer::new(2, 3).unwrap();
        let mut cells = CellFrame::default();
        cells.resize(2, 2).unwrap();

        assert_eq!(
            HalfBlock.encode(pixels.view(), cells.view_mut()),
            Err(EncodeError::SizeMismatch {
                expected: (2, 4),
                actual: (2, 3)
            })
        );
    }
}
