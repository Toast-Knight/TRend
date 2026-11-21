//! Pixel-to-cell encoders.
//!
//! An encoder turns a [`PixelFrameView`] sized `cells × grid` into terminal
//! cells. It chooses glyphs and 24-bit colors only; it knows nothing about
//! escape codes, color depth, terminal capabilities, or screen position.

use core::fmt;

use crate::cell::{CellViewMut, Color};
use crate::framebuffer::PixelFrameView;
use crate::geometry::SubcellGrid;
use crate::terminal::{TerminalCapabilities, TerminalColorMode};

mod braille;
mod density;
mod fit;
mod half_block;
mod quadrant;
mod sextant;

pub use braille::Braille;
pub use density::{Density, DensityRamp};
pub use half_block::HalfBlock;
pub(crate) use half_block::half_block_cell;
pub use quadrant::Quadrant;
pub use sextant::Sextant;

/// Converts pixels into terminal cells at a fixed subcell resolution.
pub trait CellEncoder {
    /// Pixels this encoder resolves per cell.
    fn grid(&self) -> SubcellGrid;

    /// What the encoder's output needs from the terminal to look right.
    ///
    /// Hosts use it to pick an encoder for a terminal (see
    /// [`EncoderRequirements::supported_by`]). The default is the most
    /// demanding combination, so an encoder that doesn't override it is only
    /// chosen for Unicode color terminals.
    fn requirements(&self) -> EncoderRequirements {
        EncoderRequirements::new(
            GlyphRepertoire::BlockElements,
            ColorUse::ForegroundAndBackground,
        )
    }

    /// Encodes `pixels` into every cell of `cells`.
    ///
    /// `pixels` must be exactly `cells.width() * grid.columns` by
    /// `cells.height() * grid.rows`. Any [`crate::PixelLayout`] is accepted:
    /// read pixels with [`PixelFrameView::rgba`], which normalizes format,
    /// row order, and alpha mode. After normalization, alpha 0 is "no
    /// content" and becomes [`Color::DEFAULT`]; any other alpha is visible
    /// straight RGB.
    ///
    /// Each cell must depend only on the pixels it covers, so encoding a
    /// sub-view of the cells from the matching sub-view of the pixels (see
    /// [`crate::RasterSpec::pixel_rect`]) gives the same cells as encoding
    /// everything. Row bands and [`crate::Surface::encode_region`] rely on it.
    ///
    /// Because the view is borrowed, a frame that already has the spec's
    /// exact size (see [`crate::RasterSpec::matches`]) is encoded in place
    /// with no copy.
    fn encode(&self, pixels: PixelFrameView<'_>, cells: CellViewMut<'_>)
    -> Result<(), EncodeError>;
}

/// Borrowed encoders work anywhere an encoder is expected.
impl<T: CellEncoder + ?Sized> CellEncoder for &T {
    fn grid(&self) -> SubcellGrid {
        (**self).grid()
    }

    fn requirements(&self) -> EncoderRequirements {
        (**self).requirements()
    }

    fn encode(
        &self,
        pixels: PixelFrameView<'_>,
        cells: CellViewMut<'_>,
    ) -> Result<(), EncodeError> {
        (**self).encode(pixels, cells)
    }
}

/// Boxed encoders, such as `Box<dyn CellEncoder>`, let a host choose an
/// encoder at run time, for example by terminal capability.
impl<T: CellEncoder + ?Sized> CellEncoder for Box<T> {
    fn grid(&self) -> SubcellGrid {
        (**self).grid()
    }

    fn requirements(&self) -> EncoderRequirements {
        (**self).requirements()
    }

    fn encode(
        &self,
        pixels: PixelFrameView<'_>,
        cells: CellViewMut<'_>,
    ) -> Result<(), EncodeError> {
        (**self).encode(pixels, cells)
    }
}

/// The characters an encoder emits.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub enum GlyphRepertoire {
    /// Printable ASCII only: every terminal can show it.
    Ascii,
    /// Unicode block elements and shades (U+2580–U+259F), available in
    /// practically every Unicode terminal font.
    BlockElements,
    /// Block elements plus the sextant mosaics of Unicode 13 (U+1FB00–
    /// U+1FB3B). Most current terminals draw them natively; older fonts may
    /// lack them, which [`crate::TerminalCapabilities`] can't tell.
    Sextants,
    /// Braille patterns (U+2800–U+28FF), in nearly every Unicode font.
    Braille,
}

/// Which cell colors carry image information.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub enum ColorUse {
    /// None: the glyph alone encodes the image, so monochrome output is
    /// faithful.
    None,
    /// The foreground color; the background stays the terminal default.
    Foreground,
    /// Both colors, one per part of the cell (half-block and quadrant
    /// encoders).
    ForegroundAndBackground,
}

/// What an encoder's output needs from the terminal: the policy input for
/// choosing an encoder. Subcell structure is [`CellEncoder::grid`]; the
/// physical cell shape is the host's [`crate::CellAspect`].
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub struct EncoderRequirements {
    pub glyphs: GlyphRepertoire,
    pub colors: ColorUse,
}

impl EncoderRequirements {
    pub const fn new(glyphs: GlyphRepertoire, colors: ColorUse) -> Self {
        Self { glyphs, colors }
    }

    /// Whether a terminal with `capabilities` shows this output faithfully:
    /// Unicode glyphs need `unicode`, and colors need a color mode other than
    /// monochrome. (The presenter still degrades unsupported output, e.g.
    /// shading colored cells in monochrome, but that loses information.)
    pub fn supported_by(&self, capabilities: TerminalCapabilities) -> bool {
        let glyphs = match self.glyphs {
            GlyphRepertoire::Ascii => true,
            GlyphRepertoire::BlockElements
            | GlyphRepertoire::Sextants
            | GlyphRepertoire::Braille => capabilities.unicode,
        };
        let colors = match self.colors {
            ColorUse::None => true,
            ColorUse::Foreground | ColorUse::ForegroundAndBackground => {
                capabilities.color != TerminalColorMode::Monochrome
            }
        };
        glyphs && colors
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub enum EncodeError {
    /// Pixel dimensions do not equal cell dimensions times the encoder grid.
    SizeMismatch {
        expected: (u32, u32),
        actual: (u32, u32),
    },
}

impl fmt::Display for EncodeError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::SizeMismatch { expected, actual } => write!(
                formatter,
                "encoder expected {}x{} pixels but received {}x{}",
                expected.0, expected.1, actual.0, actual.1
            ),
        }
    }
}

impl std::error::Error for EncodeError {}

fn check_size(
    grid: SubcellGrid,
    pixels: &PixelFrameView<'_>,
    cells: &CellViewMut<'_>,
) -> Result<(), EncodeError> {
    let expected = (
        u32::from(cells.width()) * u32::from(grid.columns),
        u32::from(cells.height()) * u32::from(grid.rows),
    );
    let actual = (pixels.width(), pixels.height());
    if expected == actual {
        Ok(())
    } else {
        Err(EncodeError::SizeMismatch { expected, actual })
    }
}

/// A decoded straight RGBA pixel as a color; `None` for alpha 0.
#[inline(always)]
fn visible_color([red, green, blue, alpha]: [u8; 4]) -> Option<Color> {
    (alpha != 0).then_some(Color::rgb(red, green, blue))
}
