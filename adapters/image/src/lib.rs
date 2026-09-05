use core::fmt;
use core::time::Duration;
#[cfg(feature = "webp")]
use std::io::BufReader;
use std::io::Cursor;

use trend::{FrameError, PixelFormat, PixelFrameView};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct DecodeLimits {
    pub max_width: u32,
    pub max_height: u32,
    pub max_pixels: u64,
    pub max_decoded_bytes: usize,
    pub max_decoder_bytes: usize,
}

impl Default for DecodeLimits {
    fn default() -> Self {
        Self {
            max_width: 16_384,
            max_height: 16_384,
            max_pixels: 64 * 1024 * 1024,
            max_decoded_bytes: 256 * 1024 * 1024,
            max_decoder_bytes: 8 * 1024 * 1024,
        }
    }
}

#[derive(Debug)]
pub enum DecodeError {
    Png(png::DecodingError),
    Jpeg(jpeg_decoder::Error),
    #[cfg(feature = "webp")]
    WebP(image_webp::DecodingError),
    DimensionLimit,
    ByteLimit,
    SizeOverflow,
    UnsupportedColorType,
    #[cfg(feature = "webp")]
    UnsupportedAnimation,
    InvalidDecodedLength,
    Frame(FrameError),
}

impl fmt::Display for DecodeError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Png(error) => write!(formatter, "PNG decode failed: {error}"),
            Self::Jpeg(error) => write!(formatter, "JPEG decode failed: {error}"),
            #[cfg(feature = "webp")]
            Self::WebP(error) => write!(formatter, "WebP decode failed: {error}"),
            Self::DimensionLimit => formatter.write_str("image dimensions exceed decode limits"),
            Self::ByteLimit => formatter.write_str("decoded image exceeds byte limit"),
            Self::SizeOverflow => formatter.write_str("decoded image size overflowed"),
            Self::UnsupportedColorType => {
                formatter.write_str("unsupported decoded image color type")
            }
            #[cfg(feature = "webp")]
            Self::UnsupportedAnimation => {
                formatter.write_str("animated image requires an animation adapter")
            }
            Self::InvalidDecodedLength => {
                formatter.write_str("image decoder returned an invalid length")
            }
            Self::Frame(error) => error.fmt(formatter),
        }
    }
}

impl std::error::Error for DecodeError {}

impl From<png::DecodingError> for DecodeError {
    fn from(value: png::DecodingError) -> Self {
        Self::Png(value)
    }
}

impl From<jpeg_decoder::Error> for DecodeError {
    fn from(value: jpeg_decoder::Error) -> Self {
        Self::Jpeg(value)
    }
}

#[cfg(feature = "webp")]
impl From<image_webp::DecodingError> for DecodeError {
    fn from(value: image_webp::DecodingError) -> Self {
        Self::WebP(value)
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DecodedImage {
    pixels: Vec<u8>,
    width: u32,
    height: u32,
}

impl DecodedImage {
    pub fn pixels(&self) -> &[u8] {
        &self.pixels
    }

    pub const fn width(&self) -> u32 {
        self.width
    }

    pub const fn height(&self) -> u32 {
        self.height
    }

    pub fn view(&self, timestamp: Duration) -> Result<PixelFrameView<'_>, FrameError> {
        let stride = usize::try_from(self.width)
            .ok()
            .and_then(|width| width.checked_mul(4))
            .ok_or(FrameError::SizeOverflow)?;
        PixelFrameView::new(
            &self.pixels,
            self.width,
            self.height,
            stride,
            PixelFormat::Rgba8,
            timestamp,
        )
    }
}

pub fn decode_png(bytes: &[u8], limits: DecodeLimits) -> Result<DecodedImage, DecodeError> {
    let mut decoder = png::Decoder::new_with_limits(
        Cursor::new(bytes),
        png::Limits {
            bytes: limits.max_decoder_bytes,
        },
    );
    decoder.set_transformations(png::Transformations::EXPAND | png::Transformations::STRIP_16);
    let mut reader = decoder.read_info()?;
    let width = reader.info().width;
    let height = reader.info().height;
    if width == 0
        || height == 0
        || width > limits.max_width
        || height > limits.max_height
        || u64::from(width) * u64::from(height) > limits.max_pixels
    {
        return Err(DecodeError::DimensionLimit);
    }

    let pixel_count = usize::try_from(u64::from(width) * u64::from(height))
        .map_err(|_| DecodeError::SizeOverflow)?;
    let rgba_len = pixel_count
        .checked_mul(4)
        .ok_or(DecodeError::SizeOverflow)?;
    if rgba_len > limits.max_decoded_bytes {
        return Err(DecodeError::ByteLimit);
    }

    let mut pixels = vec![0; rgba_len];
    let frame = reader.next_frame(&mut pixels)?;
    expand_to_rgba_in_place(
        &mut pixels,
        pixel_count,
        frame.color_type,
        frame.buffer_size(),
    )?;

    Ok(DecodedImage {
        pixels,
        width,
        height,
    })
}

pub fn decode_jpeg(bytes: &[u8], limits: DecodeLimits) -> Result<DecodedImage, DecodeError> {
    let mut decoder = jpeg_decoder::Decoder::new(Cursor::new(bytes));
    decoder.set_max_decoding_buffer_size(limits.max_decoder_bytes);
    decoder.read_info()?;
    let info = decoder.info().ok_or(DecodeError::InvalidDecodedLength)?;
    let width = u32::from(info.width);
    let height = u32::from(info.height);
    validate_dimensions(width, height, limits)?;

    let pixel_count = checked_pixel_count(width, height)?;
    let rgba_len = checked_rgba_len(pixel_count, limits.max_decoded_bytes)?;
    let channels = match info.pixel_format {
        jpeg_decoder::PixelFormat::L8 => 1,
        jpeg_decoder::PixelFormat::RGB24 => 3,
        jpeg_decoder::PixelFormat::L16 | jpeg_decoder::PixelFormat::CMYK32 => {
            return Err(DecodeError::UnsupportedColorType);
        }
    };
    let decoded_len = pixel_count
        .checked_mul(channels)
        .ok_or(DecodeError::SizeOverflow)?;
    if decoded_len > limits.max_decoded_bytes {
        return Err(DecodeError::ByteLimit);
    }

    let mut pixels = decoder.decode()?;
    if pixels.len() != decoded_len {
        return Err(DecodeError::InvalidDecodedLength);
    }
    pixels
        .try_reserve_exact(rgba_len - decoded_len)
        .map_err(|_| DecodeError::ByteLimit)?;
    pixels.resize(rgba_len, 0);
    expand_jpeg_to_rgba_in_place(&mut pixels, pixel_count, channels)?;

    Ok(DecodedImage {
        pixels,
        width,
        height,
    })
}

#[cfg(feature = "webp")]
pub fn decode_webp(bytes: &[u8], limits: DecodeLimits) -> Result<DecodedImage, DecodeError> {
    if bytes.len() > limits.max_decoder_bytes {
        return Err(DecodeError::ByteLimit);
    }
    let mut decoder = image_webp::WebPDecoder::new(BufReader::new(Cursor::new(bytes)))?;
    decoder.set_memory_limit(limits.max_decoder_bytes);
    let (width, height) = decoder.dimensions();
    validate_dimensions(width, height, limits)?;
    if decoder.is_animated() {
        return Err(DecodeError::UnsupportedAnimation);
    }

    let pixel_count = checked_pixel_count(width, height)?;
    let rgba_len = checked_rgba_len(pixel_count, limits.max_decoded_bytes)?;
    let decoded_len = decoder
        .output_buffer_size()
        .ok_or(DecodeError::SizeOverflow)?;
    let channels = if decoder.has_alpha() { 4 } else { 3 };
    if decoded_len != pixel_count * channels {
        return Err(DecodeError::InvalidDecodedLength);
    }

    let mut pixels = vec![0; rgba_len];
    decoder.read_image(&mut pixels[..decoded_len])?;
    if channels == 3 {
        expand_jpeg_to_rgba_in_place(&mut pixels, pixel_count, channels)?;
    }

    Ok(DecodedImage {
        pixels,
        width,
        height,
    })
}

#[cfg(feature = "webp")]
pub fn decode_webp_spike(bytes: &[u8], limits: DecodeLimits) -> Result<DecodedImage, DecodeError> {
    decode_webp(bytes, limits)
}

fn validate_dimensions(width: u32, height: u32, limits: DecodeLimits) -> Result<(), DecodeError> {
    if width == 0
        || height == 0
        || width > limits.max_width
        || height > limits.max_height
        || u64::from(width) * u64::from(height) > limits.max_pixels
    {
        return Err(DecodeError::DimensionLimit);
    }
    Ok(())
}

fn checked_pixel_count(width: u32, height: u32) -> Result<usize, DecodeError> {
    usize::try_from(u64::from(width) * u64::from(height)).map_err(|_| DecodeError::SizeOverflow)
}

fn checked_rgba_len(pixel_count: usize, max_bytes: usize) -> Result<usize, DecodeError> {
    let rgba_len = pixel_count
        .checked_mul(4)
        .ok_or(DecodeError::SizeOverflow)?;
    if rgba_len > max_bytes {
        return Err(DecodeError::ByteLimit);
    }
    Ok(rgba_len)
}

fn expand_jpeg_to_rgba_in_place(
    pixels: &mut [u8],
    pixel_count: usize,
    channels: usize,
) -> Result<(), DecodeError> {
    if !matches!(channels, 1 | 3) || pixels.len() != pixel_count * 4 {
        return Err(DecodeError::InvalidDecodedLength);
    }
    for index in (0..pixel_count).rev() {
        let source = index * channels;
        let destination = index * 4;
        if channels == 1 {
            let gray = pixels[source];
            pixels[destination..destination + 4].copy_from_slice(&[gray, gray, gray, 255]);
        } else {
            let red = pixels[source];
            let green = pixels[source + 1];
            let blue = pixels[source + 2];
            pixels[destination..destination + 4].copy_from_slice(&[red, green, blue, 255]);
        }
    }
    Ok(())
}

fn expand_to_rgba_in_place(
    pixels: &mut [u8],
    pixel_count: usize,
    color_type: png::ColorType,
    decoded_len: usize,
) -> Result<(), DecodeError> {
    let channels = match color_type {
        png::ColorType::Grayscale => 1,
        png::ColorType::GrayscaleAlpha => 2,
        png::ColorType::Rgb => 3,
        png::ColorType::Rgba => 4,
        png::ColorType::Indexed => return Err(DecodeError::UnsupportedColorType),
    };
    if decoded_len != pixel_count * channels {
        return Err(DecodeError::InvalidDecodedLength);
    }
    if channels == 4 {
        return Ok(());
    }

    for index in (0..pixel_count).rev() {
        let source = index * channels;
        let destination = index * 4;
        match color_type {
            png::ColorType::Grayscale => {
                let gray = pixels[source];
                pixels[destination..destination + 4].copy_from_slice(&[gray, gray, gray, 255]);
            }
            png::ColorType::GrayscaleAlpha => {
                let gray = pixels[source];
                let alpha = pixels[source + 1];
                pixels[destination..destination + 4].copy_from_slice(&[gray, gray, gray, alpha]);
            }
            png::ColorType::Rgb => {
                let red = pixels[source];
                let green = pixels[source + 1];
                let blue = pixels[source + 2];
                pixels[destination..destination + 4].copy_from_slice(&[red, green, blue, 255]);
            }
            png::ColorType::Rgba | png::ColorType::Indexed => unreachable!(),
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[cfg(feature = "webp")]
    use image_webp::{ColorType as WebPColorType, WebPEncoder};
    use jpeg_encoder::{ColorType as JpegColorType, Encoder as JpegEncoder};
    use trend::{CellFrame, FitMode, RenderRequest, Renderer, Viewport};

    #[cfg(feature = "webp")]
    fn webp_fixture(name: &str) -> std::path::PathBuf {
        std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../..")
            .join("Test runs Envs/Materials/WebP Spike")
            .join(name)
    }

    fn encode_test_png(width: u32, height: u32, pixels: &[u8]) -> Vec<u8> {
        let mut bytes = Vec::new();
        {
            let mut encoder = png::Encoder::new(&mut bytes, width, height);
            encoder.set_color(png::ColorType::Rgba);
            encoder.set_depth(png::BitDepth::Eight);
            let mut writer = encoder.write_header().unwrap();
            writer.write_image_data(pixels).unwrap();
        }
        bytes
    }

    fn encode_test_jpeg(
        width: u16,
        height: u16,
        color_type: JpegColorType,
        pixels: &[u8],
    ) -> Vec<u8> {
        let mut bytes = Vec::new();
        JpegEncoder::new(&mut bytes, 100)
            .encode(pixels, width, height, color_type)
            .unwrap();
        bytes
    }

    #[cfg(feature = "webp")]
    fn encode_test_webp(width: u32, height: u32, pixels: &[u8]) -> Vec<u8> {
        let mut bytes = Vec::new();
        WebPEncoder::new(&mut bytes)
            .encode(pixels, width, height, WebPColorType::Rgba8)
            .unwrap();
        bytes
    }

    #[test]
    fn decodes_png_to_borrowable_rgba_frame() {
        let expected = [255, 0, 0, 255, 0, 0, 255, 128];
        let bytes = encode_test_png(2, 1, &expected);
        let image = decode_png(&bytes, DecodeLimits::default()).unwrap();
        let frame = image.view(Duration::ZERO).unwrap();

        assert_eq!((frame.width(), frame.height()), (2, 1));
        assert_eq!(frame.pixels(), expected);
        assert_eq!(frame.pixels().as_ptr(), image.pixels().as_ptr());
    }

    #[test]
    fn rejects_dimensions_before_allocating_output() {
        let bytes = encode_test_png(2, 1, &[0; 8]);
        let limits = DecodeLimits {
            max_width: 1,
            ..DecodeLimits::default()
        };

        assert!(matches!(
            decode_png(&bytes, limits),
            Err(DecodeError::DimensionLimit)
        ));
    }

    #[test]
    fn decoded_png_renders_through_core_without_another_pixel_copy() {
        let bytes = encode_test_png(
            2,
            2,
            &[
                255, 0, 0, 255, 0, 255, 0, 255, 0, 0, 255, 255, 255, 255, 255, 255,
            ],
        );
        let image = decode_png(&bytes, DecodeLimits::default()).unwrap();
        let view = image.view(Duration::ZERO).unwrap();
        let pixel_address = view.pixels().as_ptr();
        let mut cells = CellFrame::default();

        Renderer::new()
            .render_into(
                view,
                RenderRequest {
                    viewport: Viewport {
                        width: 1,
                        height: 1,
                    },
                    fit: FitMode::Contain,
                    generation: 1,
                },
                &mut cells,
            )
            .unwrap();

        assert_eq!(pixel_address, image.pixels().as_ptr());
        assert_eq!((cells.width(), cells.height()), (1, 1));
        assert_ne!(cells.cells()[0].glyph, ' ' as u32);
    }

    #[test]
    fn decodes_rgb_jpeg_to_opaque_borrowable_rgba_frame() {
        let bytes = encode_test_jpeg(2, 1, JpegColorType::Rgb, &[240, 20, 10, 10, 30, 230]);
        let image = decode_jpeg(&bytes, DecodeLimits::default()).unwrap();
        let frame = image.view(Duration::ZERO).unwrap();

        assert_eq!((frame.width(), frame.height()), (2, 1));
        assert_eq!(frame.pixels().len(), 8);
        assert_eq!(frame.pixels()[3], 255);
        assert_eq!(frame.pixels()[7], 255);
        assert_eq!(frame.pixels().as_ptr(), image.pixels().as_ptr());
    }

    #[test]
    fn decodes_grayscale_jpeg_to_opaque_rgba() {
        let bytes = encode_test_jpeg(2, 1, JpegColorType::Luma, &[32, 224]);
        let image = decode_jpeg(&bytes, DecodeLimits::default()).unwrap();

        for pixel in image.pixels().as_chunks::<4>().0 {
            assert_eq!(pixel[0], pixel[1]);
            assert_eq!(pixel[1], pixel[2]);
            assert_eq!(pixel[3], 255);
        }
    }

    #[test]
    fn jpeg_limits_are_checked_before_decoding_pixels() {
        let bytes = encode_test_jpeg(2, 1, JpegColorType::Rgb, &[0; 6]);
        let dimension_limits = DecodeLimits {
            max_width: 1,
            ..DecodeLimits::default()
        };
        let byte_limits = DecodeLimits {
            max_decoded_bytes: 7,
            ..DecodeLimits::default()
        };

        assert!(matches!(
            decode_jpeg(&bytes, dimension_limits),
            Err(DecodeError::DimensionLimit)
        ));
        assert!(matches!(
            decode_jpeg(&bytes, byte_limits),
            Err(DecodeError::ByteLimit)
        ));
    }

    #[test]
    fn jpeg_decoder_buffer_limit_is_enforced() {
        let bytes = encode_test_jpeg(8, 8, JpegColorType::Rgb, &[127; 8 * 8 * 3]);
        let limits = DecodeLimits {
            max_decoder_bytes: 8,
            ..DecodeLimits::default()
        };

        assert!(matches!(
            decode_jpeg(&bytes, limits),
            Err(DecodeError::Jpeg(_))
        ));
    }

    #[test]
    fn rejects_malformed_and_truncated_jpeg_data() {
        assert!(matches!(
            decode_jpeg(b"not a jpeg", DecodeLimits::default()),
            Err(DecodeError::Jpeg(_))
        ));

        let mut truncated = encode_test_jpeg(8, 8, JpegColorType::Rgb, &[127; 8 * 8 * 3]);
        truncated.truncate(truncated.len() / 2);
        assert!(matches!(
            decode_jpeg(&truncated, DecodeLimits::default()),
            Err(DecodeError::Jpeg(_))
        ));
    }

    #[test]
    fn decoded_jpeg_renders_through_core_without_another_pixel_copy() {
        let bytes = encode_test_jpeg(
            2,
            2,
            JpegColorType::Rgb,
            &[255, 0, 0, 0, 255, 0, 0, 0, 255, 255, 255, 255],
        );
        let image = decode_jpeg(&bytes, DecodeLimits::default()).unwrap();
        let view = image.view(Duration::ZERO).unwrap();
        let pixel_address = view.pixels().as_ptr();
        let mut cells = CellFrame::default();

        Renderer::new()
            .render_into(
                view,
                RenderRequest {
                    viewport: Viewport {
                        width: 1,
                        height: 1,
                    },
                    fit: FitMode::Contain,
                    generation: 1,
                },
                &mut cells,
            )
            .unwrap();

        assert_eq!(pixel_address, image.pixels().as_ptr());
        assert_eq!((cells.width(), cells.height()), (1, 1));
        assert_ne!(cells.cells()[0].glyph, ' ' as u32);
    }

    #[cfg(feature = "webp")]
    #[test]
    fn webp_spike_decodes_lossless_alpha_to_exact_rgba() {
        let expected = [255, 0, 0, 0, 0, 255, 0, 127, 0, 0, 255, 255, 8, 16, 24, 64];
        let bytes = encode_test_webp(2, 2, &expected);
        let image = decode_webp_spike(&bytes, DecodeLimits::default()).unwrap();

        assert_eq!((image.width(), image.height()), (2, 2));
        assert_eq!(image.pixels(), expected);
    }

    #[cfg(feature = "webp")]
    #[test]
    fn decodes_real_lossy_lossless_alpha_and_large_webp_fixtures() {
        for name in [
            "WebPTinyLossless.webp",
            "WebPLossyScene.webp",
            "WebPLosslessScene.webp",
            "WebPAlpha.webp",
            "WebPLargeLossy.webp",
        ] {
            let bytes = std::fs::read(webp_fixture(name)).unwrap();
            let image = decode_webp(&bytes, DecodeLimits::default()).unwrap();
            let view = image.view(Duration::ZERO).unwrap();
            assert!(view.width() > 0 && view.height() > 0, "{name}");

            let mut cells = CellFrame::default();
            Renderer::new()
                .render_into(
                    view,
                    RenderRequest {
                        viewport: Viewport {
                            width: 4,
                            height: 2,
                        },
                        fit: FitMode::Contain,
                        generation: 1,
                    },
                    &mut cells,
                )
                .unwrap();
            assert!(
                cells.cells().iter().any(|cell| cell.glyph != ' ' as u32),
                "{name}"
            );
        }

        let alpha = std::fs::read(webp_fixture("WebPAlpha.webp")).unwrap();
        let image = decode_webp(&alpha, DecodeLimits::default()).unwrap();
        assert!(
            image
                .pixels()
                .as_chunks::<4>()
                .0
                .iter()
                .any(|pixel| pixel[3] < 255)
        );
    }

    #[cfg(feature = "webp")]
    #[test]
    fn real_large_webp_obeys_dimension_and_byte_limits() {
        let bytes = std::fs::read(webp_fixture("WebPLargeLossy.webp")).unwrap();
        let dimension_limits = DecodeLimits {
            max_width: 1,
            ..DecodeLimits::default()
        };
        let byte_limits = DecodeLimits {
            max_decoded_bytes: 4,
            ..DecodeLimits::default()
        };

        assert!(matches!(
            decode_webp(&bytes, dimension_limits),
            Err(DecodeError::DimensionLimit)
        ));
        assert!(matches!(
            decode_webp(&bytes, byte_limits),
            Err(DecodeError::ByteLimit)
        ));
    }

    #[cfg(feature = "webp")]
    #[test]
    fn webp_spike_checks_limits_before_output_allocation() {
        let bytes = encode_test_webp(4, 2, &[127; 4 * 2 * 4]);
        let dimension_limits = DecodeLimits {
            max_width: 3,
            ..DecodeLimits::default()
        };
        let byte_limits = DecodeLimits {
            max_decoded_bytes: 31,
            ..DecodeLimits::default()
        };
        let input_limits = DecodeLimits {
            max_decoder_bytes: bytes.len() - 1,
            ..DecodeLimits::default()
        };

        assert!(matches!(
            decode_webp_spike(&bytes, dimension_limits),
            Err(DecodeError::DimensionLimit)
        ));
        assert!(matches!(
            decode_webp_spike(&bytes, byte_limits),
            Err(DecodeError::ByteLimit)
        ));
        assert!(matches!(
            decode_webp_spike(&bytes, input_limits),
            Err(DecodeError::ByteLimit)
        ));
    }

    #[cfg(feature = "webp")]
    #[test]
    fn webp_spike_rejects_malformed_and_truncated_data() {
        assert!(matches!(
            decode_webp_spike(b"not webp", DecodeLimits::default()),
            Err(DecodeError::WebP(_))
        ));

        let mut truncated = encode_test_webp(8, 8, &[127; 8 * 8 * 4]);
        truncated.truncate(truncated.len() / 2);
        assert!(matches!(
            decode_webp_spike(&truncated, DecodeLimits::default()),
            Err(DecodeError::WebP(_))
        ));
    }

    #[cfg(feature = "webp")]
    #[test]
    fn webp_spike_renders_through_core_without_another_pixel_copy() {
        let bytes = encode_test_webp(
            2,
            2,
            &[
                255, 0, 0, 255, 0, 255, 0, 128, 0, 0, 255, 64, 255, 255, 255, 0,
            ],
        );
        let image = decode_webp_spike(&bytes, DecodeLimits::default()).unwrap();
        let view = image.view(Duration::ZERO).unwrap();
        let pixel_address = view.pixels().as_ptr();
        let mut cells = CellFrame::default();

        Renderer::new()
            .render_into(
                view,
                RenderRequest {
                    viewport: Viewport {
                        width: 1,
                        height: 1,
                    },
                    fit: FitMode::Contain,
                    generation: 1,
                },
                &mut cells,
            )
            .unwrap();

        assert_eq!(pixel_address, image.pixels().as_ptr());
        assert_eq!((cells.width(), cells.height()), (1, 1));
    }
}
