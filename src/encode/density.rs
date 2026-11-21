use crate::cell::{Cell, CellViewMut, Color};
use crate::color::{ASCII_RAMP, SHADE_RAMP, luminance, ramp_glyph};
use crate::framebuffer::{PixelFrameView, decode, with_decoder};
use crate::geometry::SubcellGrid;

use super::{
    CellEncoder, ColorUse, EncodeError, EncoderRequirements, GlyphRepertoire, check_size,
    visible_color,
};

/// Characters a [`Density`] encoder steps through, from empty to dense.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
#[non_exhaustive]
pub enum DensityRamp {
    /// ` .:-=+*#%@`: printable ASCII, shown by every terminal.
    #[default]
    Ascii,
    /// ` ░▒▓█`: Unicode shades.
    Shades,
}

impl DensityRamp {
    fn glyphs(self) -> &'static [char] {
        match self {
            Self::Ascii => &ASCII_RAMP,
            Self::Shades => &SHADE_RAMP,
        }
    }
}

/// One pixel per cell, drawn as a character whose ink density follows the
/// pixel's brightness.
///
/// This is the encoder for terminals without color, and for text-only
/// output. Without color, the glyph alone carries the image, so the output
/// is faithful on monochrome terminals. With `colored`, the foreground also
/// takes the pixel's color. Density assumes light glyphs on a dark
/// background: brighter pixels get denser characters. Transparent pixels
/// (alpha 0) become the default cell.
///
/// The ramps and brightness weights are the ones `AnsiPresenter` uses to
/// shade colored cells in monochrome mode, so a flat region looks the same
/// either way; `Density` makes that choice per pixel instead of per cell.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct Density {
    ramp: DensityRamp,
    colored: bool,
}

impl Density {
    /// ASCII ramp without color: shown faithfully by every terminal.
    pub const ASCII: Self = Self::new(DensityRamp::Ascii, false);

    pub const fn new(ramp: DensityRamp, colored: bool) -> Self {
        Self { ramp, colored }
    }

    pub const fn ramp(&self) -> DensityRamp {
        self.ramp
    }

    pub const fn is_colored(&self) -> bool {
        self.colored
    }
}

impl CellEncoder for Density {
    fn grid(&self) -> SubcellGrid {
        SubcellGrid {
            columns: 1,
            rows: 1,
        }
    }

    fn requirements(&self) -> EncoderRequirements {
        let glyphs = match self.ramp {
            DensityRamp::Ascii => GlyphRepertoire::Ascii,
            DensityRamp::Shades => GlyphRepertoire::BlockElements,
        };
        let colors = if self.colored {
            ColorUse::Foreground
        } else {
            ColorUse::None
        };
        EncoderRequirements::new(glyphs, colors)
    }

    fn encode(
        &self,
        pixels: PixelFrameView<'_>,
        mut cells: CellViewMut<'_>,
    ) -> Result<(), EncodeError> {
        check_size(self.grid(), &pixels, &cells)?;
        let ramp = self.ramp.glyphs();
        with_decoder!(pixels, D => {
            for cell_y in 0..cells.height() {
                let Some(line) = pixels.row(u32::from(cell_y)) else {
                    continue;
                };
                let row = cells.row_mut(cell_y);
                for (cell, pixel) in row.iter_mut().zip(line.chunks_exact(4)) {
                    *cell = match visible_color(decode::<D>(pixel)) {
                        None => Cell::default(),
                        Some(color) => {
                            let glyph = ramp_glyph(ramp, luminance(color));
                            let foreground = if self.colored { color } else { Color::DEFAULT };
                            Cell::with_glyph(glyph, foreground, Color::DEFAULT)
                        }
                    };
                }
            }
        });
        Ok(())
    }
}
