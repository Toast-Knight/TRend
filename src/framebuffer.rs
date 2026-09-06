use core::fmt;
use core::time::Duration;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PixelFormat {
    Rgba8,
}

impl PixelFormat {
    pub const fn bytes_per_pixel(self) -> usize {
        match self {
            Self::Rgba8 => 4,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
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

/// A validated, borrowed view of one decoded pixel frame.
///
/// Rows may include padding through `stride`; pixels are currently RGBA8 and
/// remain owned by the caller for the lifetime of this view.
#[derive(Clone, Copy, Debug)]
pub struct PixelFrameView<'a> {
    pixels: &'a [u8],
    width: u32,
    height: u32,
    stride: usize,
    format: PixelFormat,
    timestamp: Duration,
}

impl<'a> PixelFrameView<'a> {
    pub fn new(
        pixels: &'a [u8],
        width: u32,
        height: u32,
        stride: usize,
        format: PixelFormat,
        timestamp: Duration,
    ) -> Result<Self, FrameError> {
        if width == 0 || height == 0 {
            return Err(FrameError::ZeroDimension);
        }

        let row_bytes = usize::try_from(width)
            .ok()
            .and_then(|value| value.checked_mul(format.bytes_per_pixel()))
            .ok_or(FrameError::SizeOverflow)?;
        if stride < row_bytes {
            return Err(FrameError::StrideTooSmall);
        }

        let final_row = usize::try_from(height - 1)
            .ok()
            .and_then(|value| value.checked_mul(stride))
            .ok_or(FrameError::SizeOverflow)?;
        let required = final_row
            .checked_add(row_bytes)
            .ok_or(FrameError::SizeOverflow)?;
        if pixels.len() < required {
            return Err(FrameError::BufferTooSmall);
        }

        Ok(Self {
            pixels,
            width,
            height,
            stride,
            format,
            timestamp,
        })
    }

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

    pub const fn timestamp(&self) -> Duration {
        self.timestamp
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
