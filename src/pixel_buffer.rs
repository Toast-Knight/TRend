use crate::framebuffer::{
    AlphaMode, FrameError, PixelFormat, PixelFrameView, PixelLayout, PixelViewMut,
};

/// An owned RGBA8 framebuffer: the surface any renderer writes for TRend.
///
/// Pixels are straight (non-premultiplied) RGBA8, rows are tightly packed
/// (`stride == width * 4`), and dimensions are always non-zero. Alpha 0 means
/// "no content"; encoders present it as the terminal default color. Reuse one
/// buffer across frames to keep its allocation.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PixelBuffer {
    pixels: Vec<u8>,
    width: u32,
    height: u32,
}

impl PixelBuffer {
    /// Creates a transparent black buffer.
    pub fn new(width: u32, height: u32) -> Result<Self, FrameError> {
        let mut buffer = Self {
            pixels: Vec::new(),
            width: 0,
            height: 0,
        };
        buffer.resize(width, height)?;
        Ok(buffer)
    }

    /// Changes the dimensions and resets every pixel to transparent black.
    ///
    /// Existing capacity is reused when it is large enough. On error the
    /// buffer is left unchanged.
    pub fn resize(&mut self, width: u32, height: u32) -> Result<(), FrameError> {
        if width == 0 || height == 0 {
            return Err(FrameError::ZeroDimension);
        }
        let length = usize::try_from(width)
            .ok()
            .and_then(|width| width.checked_mul(usize::try_from(height).ok()?))
            .and_then(|pixels| pixels.checked_mul(PixelFormat::Rgba8.bytes_per_pixel()))
            .ok_or(FrameError::SizeOverflow)?;
        self.pixels.clear();
        self.pixels.resize(length, 0);
        self.width = width;
        self.height = height;
        Ok(())
    }

    pub const fn width(&self) -> u32 {
        self.width
    }

    pub const fn height(&self) -> u32 {
        self.height
    }

    /// Row-major RGBA8 bytes, `width * 4` bytes per row.
    pub fn pixels(&self) -> &[u8] {
        &self.pixels
    }

    /// Row-major RGBA8 bytes, `width * 4` bytes per row.
    pub fn pixels_mut(&mut self) -> &mut [u8] {
        &mut self.pixels
    }

    /// Borrows the buffer as a straight, top-down RGBA view with a zero
    /// timestamp.
    pub fn view(&self) -> PixelFrameView<'_> {
        PixelFrameView::from_valid_layout(&self.pixels, self.layout())
    }

    /// Borrows the buffer as a writable surface, for drawing into all of it
    /// or into sub-rectangles and row bands.
    pub fn view_mut(&mut self) -> PixelViewMut<'_> {
        let (width, height) = (self.width, self.height);
        PixelViewMut::new(&mut self.pixels, width, height, width as usize * 4)
            .expect("PixelBuffer upholds its layout")
    }

    fn layout(&self) -> PixelLayout {
        PixelLayout::new(
            self.width,
            self.height,
            PixelFormat::Rgba8,
            AlphaMode::Straight,
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn new_buffer_is_transparent_and_tightly_packed() {
        let buffer = PixelBuffer::new(3, 2).unwrap();
        let view = buffer.view();

        assert_eq!(buffer.pixels().len(), 24);
        assert!(buffer.pixels().iter().all(|&byte| byte == 0));
        assert_eq!((view.width(), view.height(), view.stride()), (3, 2, 12));
        assert_eq!(view.pixels().as_ptr(), buffer.pixels().as_ptr());
    }

    #[test]
    fn resize_clears_and_reuses_capacity() {
        let mut buffer = PixelBuffer::new(8, 8).unwrap();
        buffer.pixels_mut().fill(255);
        let capacity = buffer.pixels.capacity();

        buffer.resize(4, 4).unwrap();

        assert_eq!(buffer.pixels().len(), 64);
        assert!(buffer.pixels().iter().all(|&byte| byte == 0));
        assert_eq!(buffer.pixels.capacity(), capacity);
    }

    #[test]
    fn invalid_dimensions_leave_buffer_unchanged() {
        let mut buffer = PixelBuffer::new(2, 2).unwrap();
        buffer.pixels_mut()[0] = 9;

        assert_eq!(buffer.resize(0, 5), Err(FrameError::ZeroDimension));
        assert_eq!(PixelBuffer::new(4, 0), Err(FrameError::ZeroDimension));
        assert_eq!((buffer.width(), buffer.height()), (2, 2));
        assert_eq!(buffer.pixels()[0], 9);
    }
}
