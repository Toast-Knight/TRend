use core::fmt;
use core::time::Duration;

use crate::geometry::{PixelRect, RegionError};

/// Channel order of 32-bit pixels in memory.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub enum PixelFormat {
    /// Bytes are red, green, blue, alpha.
    Rgba8,
    /// Bytes are blue, green, red, alpha (common for Windows, DirectX, Vulkan
    /// swapchains, and browser off-screen buffers).
    Bgra8,
}

impl PixelFormat {
    pub const fn bytes_per_pixel(self) -> usize {
        match self {
            Self::Rgba8 | Self::Bgra8 => 4,
        }
    }
}

/// Which image row comes first in memory.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum RowOrder {
    /// The first row in memory is the top of the image.
    #[default]
    TopDown,
    /// The first row in memory is the bottom of the image, as returned by
    /// OpenGL `glReadPixels` and bottom-up bitmaps.
    BottomUp,
}

/// What the fourth byte of each pixel means.
///
/// There is deliberately no default: the same bytes mean different things in
/// different APIs, and guessing wrong renders a frame fully transparent or
/// with wrong colors.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub enum AlphaMode {
    /// Color channels are independent of alpha. Alpha 0 is "no content".
    Straight,
    /// Color channels are already multiplied by alpha, as most GPU output and
    /// compositors produce. TRend divides them out when reading. Channels
    /// larger than alpha are invalid and are clamped to alpha first.
    Premultiplied,
    /// The fourth byte is padding or undefined (for example BGRX/RGBX
    /// framebuffers and many screen captures). Every pixel is opaque.
    Ignore,
}

/// Describes how a borrowed frame's bytes are laid out, independent of where
/// they came from (a decoder, OpenGL, Vulkan, wgpu, DirectX, a browser, or
/// the application itself).
///
/// Coordinates are always logical: `(0, 0)` is the top-left of the image,
/// whatever the memory row order.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub struct PixelLayout {
    pub width: u32,
    pub height: u32,
    /// Bytes from the start of one row in memory to the start of the next.
    /// At least `width * bytes_per_pixel`; larger for padded rows such as
    /// 256-byte-aligned GPU readbacks.
    pub stride: usize,
    pub format: PixelFormat,
    pub row_order: RowOrder,
    pub alpha: AlphaMode,
}

impl PixelLayout {
    /// A tightly packed, top-down layout. Format and alpha meaning must be
    /// stated explicitly.
    pub const fn new(width: u32, height: u32, format: PixelFormat, alpha: AlphaMode) -> Self {
        Self {
            width,
            height,
            stride: (width as usize).saturating_mul(format.bytes_per_pixel()),
            format,
            row_order: RowOrder::TopDown,
            alpha,
        }
    }

    /// Sets the byte distance between rows.
    pub const fn with_stride(mut self, stride: usize) -> Self {
        self.stride = stride;
        self
    }

    pub const fn with_row_order(mut self, row_order: RowOrder) -> Self {
        self.row_order = row_order;
        self
    }

    /// Validates the layout against a buffer length and returns the number of
    /// bytes one row of pixels occupies.
    fn validate(&self, buffer_len: usize) -> Result<usize, FrameError> {
        if self.width == 0 || self.height == 0 {
            return Err(FrameError::ZeroDimension);
        }
        let row_bytes = usize::try_from(self.width)
            .ok()
            .and_then(|value| value.checked_mul(self.format.bytes_per_pixel()))
            .ok_or(FrameError::SizeOverflow)?;
        if self.stride < row_bytes {
            return Err(FrameError::StrideTooSmall);
        }
        let final_row = usize::try_from(self.height - 1)
            .ok()
            .and_then(|value| value.checked_mul(self.stride))
            .ok_or(FrameError::SizeOverflow)?;
        let required = final_row
            .checked_add(row_bytes)
            .ok_or(FrameError::SizeOverflow)?;
        if buffer_len < required {
            return Err(FrameError::BufferTooSmall);
        }
        Ok(row_bytes)
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub enum FrameError {
    ZeroDimension,
    StrideTooSmall,
    BufferTooSmall,
    SizeOverflow,
}

impl fmt::Display for FrameError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::ZeroDimension => "frame dimensions must be non-zero",
            Self::StrideTooSmall => "frame stride is smaller than one pixel row",
            Self::BufferTooSmall => "pixel buffer is smaller than the declared frame",
            Self::SizeOverflow => "frame dimensions overflow addressable memory",
        })
    }
}

impl std::error::Error for FrameError {}

/// A validated, borrowed view of one pixel frame.
///
/// The bytes stay owned by the caller for the lifetime of the view; TRend
/// never copies them. The [`PixelLayout`] says how to interpret them, and
/// [`Self::rgba`] reads any layout as straight, top-down RGBA.
#[derive(Clone, Copy, Debug)]
pub struct PixelFrameView<'a> {
    pixels: &'a [u8],
    width: u32,
    height: u32,
    stride: usize,
    format: PixelFormat,
    row_order: RowOrder,
    alpha: AlphaMode,
    timestamp: Duration,
}

impl<'a> PixelFrameView<'a> {
    /// A top-down, straight-alpha view. This is the original constructor;
    /// use [`Self::from_layout`] to describe any other layout.
    pub fn new(
        pixels: &'a [u8],
        width: u32,
        height: u32,
        stride: usize,
        format: PixelFormat,
        timestamp: Duration,
    ) -> Result<Self, FrameError> {
        let layout =
            PixelLayout::new(width, height, format, AlphaMode::Straight).with_stride(stride);
        Ok(Self::from_layout(pixels, layout)?.with_timestamp(timestamp))
    }

    /// Borrows `pixels` as a frame described by `layout`, with a zero timestamp.
    ///
    /// `pixels` must hold `(height - 1) * stride + width * bytes_per_pixel`
    /// bytes; the final row needs no padding.
    pub fn from_layout(pixels: &'a [u8], layout: PixelLayout) -> Result<Self, FrameError> {
        layout.validate(pixels.len())?;
        Ok(Self {
            pixels,
            width: layout.width,
            height: layout.height,
            stride: layout.stride,
            format: layout.format,
            row_order: layout.row_order,
            alpha: layout.alpha,
            timestamp: Duration::ZERO,
        })
    }

    /// Builds a view from a layout whose invariants the caller already
    /// upholds, such as an owned [`crate::PixelBuffer`].
    pub(crate) fn from_valid_layout(pixels: &'a [u8], layout: PixelLayout) -> Self {
        debug_assert!(layout.validate(pixels.len()).is_ok());
        Self {
            pixels,
            width: layout.width,
            height: layout.height,
            stride: layout.stride,
            format: layout.format,
            row_order: layout.row_order,
            alpha: layout.alpha,
            timestamp: Duration::ZERO,
        }
    }

    /// Returns the same view carrying a media timestamp.
    pub const fn with_timestamp(mut self, timestamp: Duration) -> Self {
        self.timestamp = timestamp;
        self
    }

    /// The borrowed bytes, in memory order starting at the view's first
    /// memory row. Interpret them with [`Self::layout`], or use
    /// [`Self::rgba`], which handles every layout.
    pub const fn pixels(&self) -> &'a [u8] {
        self.pixels
    }

    pub const fn width(&self) -> u32 {
        self.width
    }

    pub const fn height(&self) -> u32 {
        self.height
    }

    pub const fn stride(&self) -> usize {
        self.stride
    }

    pub const fn format(&self) -> PixelFormat {
        self.format
    }

    pub const fn row_order(&self) -> RowOrder {
        self.row_order
    }

    pub const fn alpha_mode(&self) -> AlphaMode {
        self.alpha
    }

    pub const fn timestamp(&self) -> Duration {
        self.timestamp
    }

    pub const fn layout(&self) -> PixelLayout {
        PixelLayout {
            width: self.width,
            height: self.height,
            stride: self.stride,
            format: self.format,
            row_order: self.row_order,
            alpha: self.alpha,
        }
    }

    /// Raw bytes of logical row `y` (0 is the top), `width * bytes_per_pixel`
    /// long, in this view's format. `None` if `y` is out of range.
    pub fn row(&self, y: u32) -> Option<&'a [u8]> {
        if y >= self.height {
            return None;
        }
        let start = self.memory_row(y) * self.stride;
        let length = self.width as usize * self.format.bytes_per_pixel();
        Some(&self.pixels[start..start + length])
    }

    /// The pixel at logical `(x, y)` as straight (non-premultiplied) RGBA,
    /// whatever the layout. `None` if out of range.
    ///
    /// - [`AlphaMode::Straight`]: channels as stored (reordered for BGRA).
    /// - [`AlphaMode::Premultiplied`]: channels divided by alpha, rounded;
    ///   alpha 0 reads as `[0, 0, 0, 0]`.
    /// - [`AlphaMode::Ignore`]: channels as stored, alpha 255.
    #[inline]
    pub fn rgba(&self, x: u32, y: u32) -> Option<[u8; 4]> {
        if x >= self.width || y >= self.height {
            return None;
        }
        let bytes_per_pixel = self.format.bytes_per_pixel();
        let offset = self.memory_row(y) * self.stride + x as usize * bytes_per_pixel;
        let bytes = &self.pixels[offset..offset + bytes_per_pixel];
        let [red, green, blue, alpha] = match self.format {
            PixelFormat::Rgba8 => [bytes[0], bytes[1], bytes[2], bytes[3]],
            PixelFormat::Bgra8 => [bytes[2], bytes[1], bytes[0], bytes[3]],
        };
        Some(match self.alpha {
            AlphaMode::Straight => [red, green, blue, alpha],
            AlphaMode::Ignore => [red, green, blue, 255],
            AlphaMode::Premultiplied if alpha == 0 => [0; 4],
            AlphaMode::Premultiplied => [
                unpremultiply(red, alpha),
                unpremultiply(green, alpha),
                unpremultiply(blue, alpha),
                alpha,
            ],
        })
    }

    /// A view of `rect` (logical coordinates) sharing the same bytes and
    /// layout. No pixels are copied.
    pub fn sub(&self, rect: PixelRect) -> Result<Self, RegionError> {
        rect.check_within(self.width, self.height)?;
        let first_memory_row = match self.row_order {
            RowOrder::TopDown => rect.y,
            RowOrder::BottomUp => self.height - rect.y - rect.height,
        };
        let bytes_per_pixel = self.format.bytes_per_pixel();
        let start = first_memory_row as usize * self.stride + rect.x as usize * bytes_per_pixel;
        let length =
            (rect.height as usize - 1) * self.stride + rect.width as usize * bytes_per_pixel;
        Ok(Self {
            pixels: &self.pixels[start..start + length],
            width: rect.width,
            height: rect.height,
            ..*self
        })
    }

    /// True for the canonical layout, whose bytes need no normalization.
    #[inline]
    pub(crate) fn is_straight_rgba_top_down(&self) -> bool {
        matches!(
            (self.format, self.row_order, self.alpha),
            (PixelFormat::Rgba8, RowOrder::TopDown, AlphaMode::Straight)
        )
    }

    /// Memory row index holding logical row `y`.
    #[inline]
    fn memory_row(&self, y: u32) -> usize {
        match self.row_order {
            RowOrder::TopDown => y as usize,
            RowOrder::BottomUp => (self.height - 1 - y) as usize,
        }
    }
}

/// Per-layout pixel decoding, chosen once per call instead of per pixel.
///
/// Encoders borrow whole rows with [`PixelFrameView::row`] (which handles
/// row order) and convert each pixel with [`decode`], monomorphized for the
/// view's format and alpha mode. The results equal [`PixelFrameView::rgba`]
/// for every layout (see the tests).
pub(crate) mod decoders {
    pub(crate) const RGBA_STRAIGHT: u8 = 0;
    pub(crate) const BGRA_STRAIGHT: u8 = 1;
    pub(crate) const RGBA_IGNORE: u8 = 2;
    pub(crate) const BGRA_IGNORE: u8 = 3;
    pub(crate) const RGBA_PREMULTIPLIED: u8 = 4;
    pub(crate) const BGRA_PREMULTIPLIED: u8 = 5;
}

/// Runs `$body` with `$decoder` bound to a `const u8` naming the view's
/// decoder, so `decode::<$decoder>` compiles to straight-line code.
macro_rules! with_decoder {
    ($view:expr, $decoder:ident => $body:block) => {{
        use $crate::framebuffer::decoders::*;
        match $view.decoder() {
            RGBA_STRAIGHT => {
                const $decoder: u8 = RGBA_STRAIGHT;
                $body
            }
            BGRA_STRAIGHT => {
                const $decoder: u8 = BGRA_STRAIGHT;
                $body
            }
            RGBA_IGNORE => {
                const $decoder: u8 = RGBA_IGNORE;
                $body
            }
            BGRA_IGNORE => {
                const $decoder: u8 = BGRA_IGNORE;
                $body
            }
            RGBA_PREMULTIPLIED => {
                const $decoder: u8 = RGBA_PREMULTIPLIED;
                $body
            }
            _ => {
                const $decoder: u8 = BGRA_PREMULTIPLIED;
                $body
            }
        }
    }};
}
pub(crate) use with_decoder;

/// Straight RGBA from the 4 bytes of one pixel, for decoder `D`.
#[inline(always)]
pub(crate) fn decode<const D: u8>(bytes: &[u8]) -> [u8; 4] {
    use decoders::*;
    let (red, green, blue, alpha) = match D {
        RGBA_STRAIGHT | RGBA_IGNORE | RGBA_PREMULTIPLIED => {
            (bytes[0], bytes[1], bytes[2], bytes[3])
        }
        _ => (bytes[2], bytes[1], bytes[0], bytes[3]),
    };
    match D {
        RGBA_STRAIGHT | BGRA_STRAIGHT => [red, green, blue, alpha],
        RGBA_IGNORE | BGRA_IGNORE => [red, green, blue, 255],
        _ if alpha == 0 => [0; 4],
        _ => [
            unpremultiply(red, alpha),
            unpremultiply(green, alpha),
            unpremultiply(blue, alpha),
            alpha,
        ],
    }
}

impl PixelFrameView<'_> {
    /// The decoder for this view's layout (see [`decoders`]).
    #[inline]
    pub(crate) fn decoder(&self) -> u8 {
        use decoders::*;
        match (self.format, self.alpha) {
            (PixelFormat::Rgba8, AlphaMode::Straight) => RGBA_STRAIGHT,
            (PixelFormat::Bgra8, AlphaMode::Straight) => BGRA_STRAIGHT,
            (PixelFormat::Rgba8, AlphaMode::Ignore) => RGBA_IGNORE,
            (PixelFormat::Bgra8, AlphaMode::Ignore) => BGRA_IGNORE,
            (PixelFormat::Rgba8, AlphaMode::Premultiplied) => RGBA_PREMULTIPLIED,
            (PixelFormat::Bgra8, AlphaMode::Premultiplied) => BGRA_PREMULTIPLIED,
        }
    }
}

/// `channel * 255 / alpha`, rounded, after clamping invalid channels to alpha.
/// Opaque pixels, the vast majority of rendered frames, return unchanged:
/// `(c·255 + 127) / 255 == c`, so no division is needed.
#[inline]
fn unpremultiply(channel: u8, alpha: u8) -> u8 {
    if alpha == 255 {
        return channel;
    }
    let channel = u16::from(channel.min(alpha));
    let alpha = u16::from(alpha);
    ((channel * 255 + alpha / 2) / alpha) as u8
}

/// A borrowed, writable RGBA8 surface that a renderer draws into.
///
/// Always straight alpha, top-down, with a stride, so every writer sees one
/// canonical layout. Foreign layouts (BGRA, bottom-up, premultiplied) are
/// read-only inputs described by [`PixelLayout`]. Obtain one from
/// [`crate::PixelBuffer::view_mut`] or wrap your own memory with
/// [`Self::new`]; take sub-rectangles with [`Self::sub`] and split into row
/// bands with [`Self::into_row_bands`].
#[derive(Debug)]
pub struct PixelViewMut<'a> {
    pixels: &'a mut [u8],
    width: u32,
    height: u32,
    stride: usize,
}

impl<'a> PixelViewMut<'a> {
    /// Wraps caller-owned RGBA8 memory. Same length rules as
    /// [`PixelFrameView::from_layout`].
    pub fn new(
        pixels: &'a mut [u8],
        width: u32,
        height: u32,
        stride: usize,
    ) -> Result<Self, FrameError> {
        PixelLayout::new(width, height, PixelFormat::Rgba8, AlphaMode::Straight)
            .with_stride(stride)
            .validate(pixels.len())?;
        Ok(Self {
            pixels,
            width,
            height,
            stride,
        })
    }

    pub const fn width(&self) -> u32 {
        self.width
    }

    pub const fn height(&self) -> u32 {
        self.height
    }

    pub const fn stride(&self) -> usize {
        self.stride
    }

    /// RGBA bytes of row `y`, `width * 4` long. Panics if `y >= height`.
    pub fn row_mut(&mut self, y: u32) -> &mut [u8] {
        assert!(y < self.height, "pixel row out of range");
        let start = y as usize * self.stride;
        &mut self.pixels[start..start + self.width as usize * 4]
    }

    /// Reads the surface back as a straight, top-down RGBA view.
    pub fn as_view(&self) -> PixelFrameView<'_> {
        PixelFrameView::from_valid_layout(self.pixels, self.layout())
    }

    /// A shorter-lived view of the same pixels, so the view can be passed to
    /// a function that takes it by value (such as
    /// [`crate::ImageSource::render_region`]) and used again afterwards.
    pub fn reborrow(&mut self) -> PixelViewMut<'_> {
        PixelViewMut {
            pixels: self.pixels,
            width: self.width,
            height: self.height,
            stride: self.stride,
        }
    }

    /// A writable view of `rect` within this one. No pixels are copied.
    pub fn sub(&mut self, rect: PixelRect) -> Result<PixelViewMut<'_>, RegionError> {
        rect.check_within(self.width, self.height)?;
        let start = rect.y as usize * self.stride + rect.x as usize * 4;
        let length = (rect.height as usize - 1) * self.stride + rect.width as usize * 4;
        Ok(PixelViewMut {
            pixels: &mut self.pixels[start..start + length],
            width: rect.width,
            height: rect.height,
            stride: self.stride,
        })
    }

    /// Splits the surface into disjoint bands of `rows_per_band` rows (the
    /// last may be shorter), each yielded with its first row. Bands can be
    /// filled independently: in sequence with a cancellation check between
    /// them, or on separate threads. Panics if `rows_per_band` is 0.
    pub fn into_row_bands(self, rows_per_band: u32) -> PixelRowBands<'a> {
        assert!(rows_per_band > 0, "rows_per_band must be non-zero");
        PixelRowBands {
            rest: Some(self),
            next_row: 0,
            rows_per_band,
        }
    }

    fn layout(&self) -> PixelLayout {
        PixelLayout::new(
            self.width,
            self.height,
            PixelFormat::Rgba8,
            AlphaMode::Straight,
        )
        .with_stride(self.stride)
    }
}

/// Iterator returned by [`PixelViewMut::into_row_bands`].
#[derive(Debug)]
pub struct PixelRowBands<'a> {
    rest: Option<PixelViewMut<'a>>,
    next_row: u32,
    rows_per_band: u32,
}

impl<'a> Iterator for PixelRowBands<'a> {
    /// The band's first row within the original view, and the band.
    type Item = (u32, PixelViewMut<'a>);

    fn next(&mut self) -> Option<Self::Item> {
        let view = self.rest.take()?;
        let first_row = self.next_row;
        if view.height <= self.rows_per_band {
            return Some((first_row, view));
        }
        let split = self.rows_per_band as usize * view.stride;
        let (band, rest) = view.pixels.split_at_mut(split);
        self.next_row += self.rows_per_band;
        self.rest = Some(PixelViewMut {
            pixels: rest,
            width: view.width,
            height: view.height - self.rows_per_band,
            stride: view.stride,
        });
        Some((
            first_row,
            PixelViewMut {
                pixels: band,
                width: view.width,
                height: self.rows_per_band,
                stride: view.stride,
            },
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unpremultiply_shortcut_is_exact() {
        for alpha in 1..=255_u16 {
            for channel in 0..=255_u16 {
                let clamped = channel.min(alpha);
                let expected = ((clamped * 255 + alpha / 2) / alpha) as u8;
                assert_eq!(unpremultiply(channel as u8, alpha as u8), expected);
            }
        }
    }

    #[test]
    fn row_decoders_match_rgba_for_every_layout() {
        let values = [0_u8, 1, 2, 7, 64, 127, 128, 200, 254, 255];
        for format in [PixelFormat::Rgba8, PixelFormat::Bgra8] {
            for alpha_mode in [
                AlphaMode::Straight,
                AlphaMode::Ignore,
                AlphaMode::Premultiplied,
            ] {
                let mut bytes = Vec::new();
                for alpha in 0..=255_u8 {
                    for &a in &values {
                        for &b in &values {
                            bytes.extend_from_slice(&[a, b, a ^ b, alpha]);
                        }
                    }
                }
                let width = (bytes.len() / 4) as u32;
                let layout = PixelLayout::new(width, 1, format, alpha_mode);
                let view = PixelFrameView::from_layout(&bytes, layout).unwrap();
                let row = view.row(0).unwrap();
                let decoded: Vec<[u8; 4]> = with_decoder!(view, D => {
                    row.chunks_exact(4).map(decode::<D>).collect()
                });
                for (x, pixel) in decoded.iter().enumerate() {
                    assert_eq!(
                        Some(*pixel),
                        view.rgba(x as u32, 0),
                        "{format:?} {alpha_mode:?} at {x}"
                    );
                }
            }
        }
    }

    #[test]
    fn accepts_borrowed_rgba_with_padding() {
        let pixels = [0_u8; 24];
        let frame =
            PixelFrameView::new(&pixels, 2, 2, 16, PixelFormat::Rgba8, Duration::ZERO).unwrap();

        assert_eq!(frame.pixels().as_ptr(), pixels.as_ptr());
        assert_eq!(frame.stride(), 16);
    }

    #[test]
    fn rejects_short_buffer() {
        let pixels = [0_u8; 15];
        assert_eq!(
            PixelFrameView::new(&pixels, 2, 2, 8, PixelFormat::Rgba8, Duration::ZERO).unwrap_err(),
            FrameError::BufferTooSmall
        );
    }
}
