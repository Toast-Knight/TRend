use core::fmt;
use core::num::NonZeroU64;
use core::time::Duration;
#[cfg(feature = "webp")]
use std::io::BufReader;
use std::io::Cursor;

use gif::{ColorOutput, DecodeOptions, DisposalMethod, MemoryLimit, Repeat};
#[cfg(feature = "webp")]
use image_webp::{LoopCount, WebPDecoder};
use trend::{
    AnimationPosition, AnimationTimeline, FrameError, PixelFormat, PixelFrameView, TimelineError,
};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct DecodeLimits {
    pub max_width: u32,
    pub max_height: u32,
    pub max_pixels: u64,
    pub max_frames: usize,
    pub max_decoded_bytes: usize,
    pub max_decoder_bytes: u64,
    pub minimum_frame_duration: Duration,
}

impl Default for DecodeLimits {
    fn default() -> Self {
        Self {
            max_width: 16_384,
            max_height: 16_384,
            max_pixels: 64 * 1024 * 1024,
            max_frames: 1_024,
            max_decoded_bytes: 256 * 1024 * 1024,
            max_decoder_bytes: 16 * 1024 * 1024,
            minimum_frame_duration: Duration::from_millis(10),
        }
    }
}

#[derive(Debug)]
pub enum DecodeError {
    Gif(gif::DecodingError),
    #[cfg(feature = "webp")]
    WebP(image_webp::DecodingError),
    DimensionLimit,
    FrameLimit,
    ByteLimit,
    SizeOverflow,
    InvalidFrame,
    Timeline(TimelineError),
    Frame(FrameError),
}

impl fmt::Display for DecodeError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Gif(error) => write!(formatter, "GIF decode failed: {error}"),
            #[cfg(feature = "webp")]
            Self::WebP(error) => write!(formatter, "WebP decode failed: {error}"),
            Self::DimensionLimit => {
                formatter.write_str("animation dimensions exceed decode limits")
            }
            Self::FrameLimit => formatter.write_str("animation exceeds frame count limit"),
            Self::ByteLimit => formatter.write_str("animation exceeds decoded byte limit"),
            Self::SizeOverflow => formatter.write_str("decoded animation size overflowed"),
            Self::InvalidFrame => formatter.write_str("GIF decoder returned an invalid frame"),
            Self::Timeline(error) => error.fmt(formatter),
            Self::Frame(error) => error.fmt(formatter),
        }
    }
}

impl std::error::Error for DecodeError {}

impl From<gif::DecodingError> for DecodeError {
    fn from(value: gif::DecodingError) -> Self {
        Self::Gif(value)
    }
}

#[cfg(feature = "webp")]
impl From<image_webp::DecodingError> for DecodeError {
    fn from(value: image_webp::DecodingError) -> Self {
        Self::WebP(value)
    }
}

impl From<TimelineError> for DecodeError {
    fn from(value: TimelineError) -> Self {
        Self::Timeline(value)
    }
}

impl From<FrameError> for DecodeError {
    fn from(value: FrameError) -> Self {
        Self::Frame(value)
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct DecodedFrame {
    pixels: Vec<u8>,
    timestamp: Duration,
}

#[derive(Clone, Debug)]
pub struct DecodedAnimation {
    frames: Vec<DecodedFrame>,
    width: u32,
    height: u32,
    decoded_bytes: usize,
    timeline: AnimationTimeline,
}

impl DecodedAnimation {
    pub const fn width(&self) -> u32 {
        self.width
    }

    pub const fn height(&self) -> u32 {
        self.height
    }

    pub fn frame_count(&self) -> usize {
        self.frames.len()
    }

    pub const fn decoded_bytes(&self) -> usize {
        self.decoded_bytes
    }

    pub const fn duration(&self) -> Duration {
        self.timeline.duration()
    }

    pub fn position(&self, wall_now: Duration) -> AnimationPosition {
        self.timeline.position(wall_now)
    }

    pub fn frame_at(&self, frame_index: usize) -> Result<PixelFrameView<'_>, DecodeError> {
        let frame = self
            .frames
            .get(frame_index)
            .ok_or(DecodeError::InvalidFrame)?;
        let stride = usize::try_from(self.width)
            .ok()
            .and_then(|width| width.checked_mul(4))
            .ok_or(DecodeError::SizeOverflow)?;
        Ok(PixelFrameView::new(
            &frame.pixels,
            self.width,
            self.height,
            stride,
            PixelFormat::Rgba8,
            frame.timestamp,
        )?)
    }

    pub fn frame_timestamp(&self, frame_index: usize) -> Result<Duration, DecodeError> {
        self.frames
            .get(frame_index)
            .map(|frame| frame.timestamp)
            .ok_or(DecodeError::InvalidFrame)
    }

    pub fn frame_duration(&self, frame_index: usize) -> Result<Duration, DecodeError> {
        let start = self.frame_timestamp(frame_index)?;
        let end = self
            .frames
            .get(frame_index + 1)
            .map_or(self.timeline.duration(), |frame| frame.timestamp);
        Ok(end.saturating_sub(start))
    }

    pub fn current_frame(&self, wall_now: Duration) -> Result<PixelFrameView<'_>, DecodeError> {
        self.frame_at(self.position(wall_now).frame_index)
    }

    pub fn pause(&mut self, wall_now: Duration) {
        self.timeline.pause(wall_now);
    }

    pub fn play(&mut self, wall_now: Duration) {
        self.timeline.play(wall_now);
    }

    pub fn restart(&mut self, wall_now: Duration) {
        self.timeline.restart(wall_now);
    }

    pub fn seek_time(&mut self, media_time: Duration, wall_now: Duration) {
        self.timeline.seek_time(media_time, wall_now);
    }

    pub fn seek_frame(
        &mut self,
        frame_index: usize,
        wall_now: Duration,
    ) -> Result<(), TimelineError> {
        self.timeline.seek_frame(frame_index, wall_now)
    }
}

pub fn decode_gif(
    bytes: &[u8],
    limits: DecodeLimits,
    wall_now: Duration,
) -> Result<DecodedAnimation, DecodeError> {
    let decoder_limit = NonZeroU64::new(limits.max_decoder_bytes).ok_or(DecodeError::ByteLimit)?;
    let mut options = DecodeOptions::new();
    options.set_color_output(ColorOutput::RGBA);
    options.set_memory_limit(MemoryLimit::Bytes(decoder_limit));
    options.check_frame_consistency(true);
    let mut decoder = options.read_info(Cursor::new(bytes))?;

    let width = u32::from(decoder.width());
    let height = u32::from(decoder.height());
    let pixel_count = u64::from(width) * u64::from(height);
    if width == 0
        || height == 0
        || width > limits.max_width
        || height > limits.max_height
        || pixel_count > limits.max_pixels
    {
        return Err(DecodeError::DimensionLimit);
    }
    let canvas_len = usize::try_from(pixel_count)
        .ok()
        .and_then(|pixels| pixels.checked_mul(4))
        .ok_or(DecodeError::SizeOverflow)?;
    if canvas_len > limits.max_decoded_bytes {
        return Err(DecodeError::ByteLimit);
    }

    let total_plays = match decoder.repeat() {
        Repeat::Infinite => None,
        Repeat::Finite(repetitions) => Some(u64::from(repetitions) + 1),
    };
    let canvas_width = usize::try_from(width).map_err(|_| DecodeError::SizeOverflow)?;
    let mut canvas = vec![0; canvas_len];
    let mut frames = Vec::new();
    let mut durations = Vec::new();
    let mut timestamp = Duration::ZERO;
    let mut decoded_bytes = 0_usize;

    while let Some(frame) = decoder.read_next_frame()? {
        if frames.len() >= limits.max_frames {
            return Err(DecodeError::FrameLimit);
        }
        decoded_bytes = decoded_bytes
            .checked_add(canvas_len)
            .ok_or(DecodeError::SizeOverflow)?;
        if decoded_bytes > limits.max_decoded_bytes {
            return Err(DecodeError::ByteLimit);
        }

        let previous = (frame.dispose == DisposalMethod::Previous).then(|| canvas.clone());
        composite_frame(&mut canvas, canvas_width, frame)?;
        frames.push(DecodedFrame {
            pixels: canvas.clone(),
            timestamp,
        });

        let encoded_duration = Duration::from_millis(u64::from(frame.delay) * 10);
        let frame_duration = encoded_duration.max(limits.minimum_frame_duration);
        if frame_duration.is_zero() {
            return Err(DecodeError::Timeline(TimelineError::ZeroDuration));
        }
        timestamp = timestamp
            .checked_add(frame_duration)
            .ok_or(DecodeError::SizeOverflow)?;
        durations.push(frame_duration);

        match frame.dispose {
            DisposalMethod::Background => clear_frame(&mut canvas, canvas_width, frame),
            DisposalMethod::Previous => canvas = previous.ok_or(DecodeError::InvalidFrame)?,
            DisposalMethod::Any | DisposalMethod::Keep => {}
        }
    }

    let timeline = AnimationTimeline::new(wall_now, durations, total_plays)?;
    Ok(DecodedAnimation {
        frames,
        width,
        height,
        decoded_bytes,
        timeline,
    })
}

#[cfg(feature = "webp")]
pub fn decode_webp(
    bytes: &[u8],
    limits: DecodeLimits,
    wall_now: Duration,
) -> Result<DecodedAnimation, DecodeError> {
    if bytes.len() > usize::try_from(limits.max_decoder_bytes).unwrap_or(usize::MAX) {
        return Err(DecodeError::ByteLimit);
    }

    let mut decoder = WebPDecoder::new(BufReader::new(Cursor::new(bytes)))?;
    decoder.set_memory_limit(usize::try_from(limits.max_decoder_bytes).unwrap_or(usize::MAX));
    if !decoder.is_animated() {
        return Err(DecodeError::InvalidFrame);
    }

    let (width, height) = decoder.dimensions();
    let pixel_count = u64::from(width) * u64::from(height);
    if width == 0
        || height == 0
        || width > limits.max_width
        || height > limits.max_height
        || pixel_count > limits.max_pixels
    {
        return Err(DecodeError::DimensionLimit);
    }

    let frame_count = usize::try_from(decoder.num_frames()).map_err(|_| DecodeError::FrameLimit)?;
    if frame_count == 0 || frame_count > limits.max_frames {
        return Err(DecodeError::FrameLimit);
    }

    let pixel_count = usize::try_from(pixel_count).map_err(|_| DecodeError::SizeOverflow)?;
    let canvas_len = pixel_count
        .checked_mul(4)
        .ok_or(DecodeError::SizeOverflow)?;
    let decoded_bytes = canvas_len
        .checked_mul(frame_count)
        .ok_or(DecodeError::SizeOverflow)?;
    if decoded_bytes > limits.max_decoded_bytes {
        return Err(DecodeError::ByteLimit);
    }

    let channels = if decoder.has_alpha() { 4 } else { 3 };
    let decoder_frame_len = pixel_count
        .checked_mul(channels)
        .ok_or(DecodeError::SizeOverflow)?;
    if decoder.output_buffer_size() != Some(decoder_frame_len) {
        return Err(DecodeError::InvalidFrame);
    }

    let total_plays = match decoder.loop_count() {
        LoopCount::Forever => None,
        LoopCount::Times(count) => Some(u64::from(count.get())),
    };
    let mut decoder_frame = vec![0; decoder_frame_len];
    let mut frames = Vec::with_capacity(frame_count);
    let mut durations = Vec::with_capacity(frame_count);
    let mut timestamp = Duration::ZERO;

    for _ in 0..frame_count {
        let encoded_duration =
            Duration::from_millis(u64::from(decoder.read_frame(&mut decoder_frame)?));
        let frame_duration = encoded_duration.max(limits.minimum_frame_duration);
        if frame_duration.is_zero() {
            return Err(DecodeError::Timeline(TimelineError::ZeroDuration));
        }

        let pixels = if channels == 4 {
            decoder_frame.clone()
        } else {
            let mut rgba = vec![0; canvas_len];
            let (rgb_pixels, _) = decoder_frame.as_chunks::<3>();
            let (rgba_pixels, _) = rgba.as_chunks_mut::<4>();
            for (rgb, output) in rgb_pixels.iter().zip(rgba_pixels) {
                output.copy_from_slice(&[rgb[0], rgb[1], rgb[2], 255]);
            }
            rgba
        };
        frames.push(DecodedFrame { pixels, timestamp });
        durations.push(frame_duration);
        timestamp = timestamp
            .checked_add(frame_duration)
            .ok_or(DecodeError::SizeOverflow)?;
    }

    let timeline = AnimationTimeline::new(wall_now, durations, total_plays)?;
    Ok(DecodedAnimation {
        frames,
        width,
        height,
        decoded_bytes,
        timeline,
    })
}

fn composite_frame(
    canvas: &mut [u8],
    canvas_width: usize,
    frame: &gif::Frame<'_>,
) -> Result<(), DecodeError> {
    let frame_width = usize::from(frame.width);
    let frame_height = usize::from(frame.height);
    let expected = frame_width
        .checked_mul(frame_height)
        .and_then(|pixels| pixels.checked_mul(4))
        .ok_or(DecodeError::SizeOverflow)?;
    if frame.buffer.len() != expected {
        return Err(DecodeError::InvalidFrame);
    }

    for row in 0..frame_height {
        for column in 0..frame_width {
            let source = (row * frame_width + column) * 4;
            if frame.buffer[source + 3] == 0 {
                continue;
            }
            let destination =
                ((usize::from(frame.top) + row) * canvas_width + usize::from(frame.left) + column)
                    * 4;
            canvas[destination..destination + 4].copy_from_slice(&frame.buffer[source..source + 4]);
        }
    }
    Ok(())
}

fn clear_frame(canvas: &mut [u8], canvas_width: usize, frame: &gif::Frame<'_>) {
    for row in 0..usize::from(frame.height) {
        let start = ((usize::from(frame.top) + row) * canvas_width + usize::from(frame.left)) * 4;
        let end = start + usize::from(frame.width) * 4;
        canvas[start..end].fill(0);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use trend::{CellFrame, FitMode, RenderRequest, Renderer, Viewport};

    #[cfg(feature = "webp")]
    fn animated_webp_fixture() -> std::path::PathBuf {
        std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../..")
            .join("Test runs Envs/Materials/WebP Spike/WebPAnimatedRejected.webp")
    }

    fn encode_gif(frames: &[gif::Frame<'_>], repeat: Repeat) -> Vec<u8> {
        let mut bytes = Vec::new();
        {
            let palette = [255, 0, 0, 0, 255, 0, 0, 0, 255, 0, 0, 0];
            let mut encoder = gif::Encoder::new(&mut bytes, 2, 1, &palette).unwrap();
            encoder.set_repeat(repeat).unwrap();
            for frame in frames {
                encoder.write_frame(frame).unwrap();
            }
        }
        bytes
    }

    fn frame<'a>(pixels: &'a [u8], delay: u16, dispose: DisposalMethod) -> gif::Frame<'a> {
        gif::Frame {
            width: 2,
            height: 1,
            delay,
            dispose,
            buffer: pixels.into(),
            ..gif::Frame::default()
        }
    }

    #[test]
    fn decodes_variable_delays_into_borrowed_rgba_frames() {
        let bytes = encode_gif(
            &[
                frame(&[0, 1], 1, DisposalMethod::Keep),
                frame(&[2, 1], 5, DisposalMethod::Keep),
            ],
            Repeat::Infinite,
        );
        let animation = decode_gif(&bytes, DecodeLimits::default(), Duration::ZERO).unwrap();

        assert_eq!(animation.frame_count(), 2);
        assert_eq!(animation.position(Duration::from_millis(9)).frame_index, 0);
        assert_eq!(animation.position(Duration::from_millis(10)).frame_index, 1);
        assert_eq!(animation.position(Duration::from_millis(60)).frame_index, 0);
        let view = animation.frame_at(1).unwrap();
        assert_eq!(view.pixels().as_ptr(), animation.frames[1].pixels.as_ptr());
    }

    #[cfg(feature = "webp")]
    #[test]
    fn decodes_and_renders_real_animated_webp_with_timestamps() {
        let bytes = std::fs::read(animated_webp_fixture()).unwrap();
        let animation = decode_webp(&bytes, DecodeLimits::default(), Duration::ZERO).unwrap();

        assert!(animation.frame_count() > 1);
        assert!(animation.width() > 0 && animation.height() > 0);
        assert!(!animation.duration().is_zero());
        for frame_index in 1..animation.frame_count() {
            assert!(
                animation.frame_timestamp(frame_index).unwrap()
                    > animation.frame_timestamp(frame_index - 1).unwrap()
            );
            assert!(!animation.frame_duration(frame_index).unwrap().is_zero());
        }

        let mut cells = CellFrame::default();
        for frame_index in [0, animation.frame_count() - 1] {
            Renderer::new()
                .render_into(
                    animation.frame_at(frame_index).unwrap(),
                    RenderRequest {
                        viewport: Viewport {
                            width: 4,
                            height: 2,
                        },
                        fit: FitMode::Contain,
                        generation: frame_index as u64,
                    },
                    &mut cells,
                )
                .unwrap();
            assert!(cells.cells().iter().any(|cell| cell.glyph != ' ' as u32));
        }
    }

    #[cfg(feature = "webp")]
    #[test]
    fn animated_webp_rejects_invalid_data_and_enforces_limits() {
        assert!(matches!(
            decode_webp(b"not webp", DecodeLimits::default(), Duration::ZERO),
            Err(DecodeError::WebP(_))
        ));

        let bytes = std::fs::read(animated_webp_fixture()).unwrap();
        let frame_limits = DecodeLimits {
            max_frames: 1,
            ..DecodeLimits::default()
        };
        let byte_limits = DecodeLimits {
            max_decoded_bytes: 4,
            ..DecodeLimits::default()
        };
        assert!(matches!(
            decode_webp(&bytes, frame_limits, Duration::ZERO),
            Err(DecodeError::FrameLimit)
        ));
        assert!(matches!(
            decode_webp(&bytes, byte_limits, Duration::ZERO),
            Err(DecodeError::ByteLimit)
        ));
    }

    #[test]
    fn background_disposal_clears_before_the_next_frame() {
        let mut first = frame(&[0, 1], 1, DisposalMethod::Background);
        first.transparent = Some(3);
        let mut second = frame(&[3, 2], 1, DisposalMethod::Keep);
        second.transparent = Some(3);
        let bytes = encode_gif(&[first, second], Repeat::Finite(0));
        let animation = decode_gif(&bytes, DecodeLimits::default(), Duration::ZERO).unwrap();

        assert_eq!(
            animation.frame_at(1).unwrap().pixels(),
            &[0, 0, 0, 0, 0, 0, 255, 255]
        );
    }

    #[test]
    fn previous_disposal_restores_the_canvas_before_the_next_frame() {
        let first = frame(&[0, 1], 1, DisposalMethod::Keep);
        let mut second = frame(&[2, 3], 1, DisposalMethod::Previous);
        second.transparent = Some(3);
        let mut third = frame(&[3, 2], 1, DisposalMethod::Keep);
        third.transparent = Some(3);
        let bytes = encode_gif(&[first, second, third], Repeat::Finite(0));
        let animation = decode_gif(&bytes, DecodeLimits::default(), Duration::ZERO).unwrap();

        assert_eq!(
            animation.frame_at(2).unwrap().pixels(),
            &[255, 0, 0, 255, 0, 0, 255, 255]
        );
    }

    #[test]
    fn resize_rendering_preserves_decoded_storage_and_playback_clock() {
        let bytes = encode_gif(
            &[
                frame(&[0, 1], 5, DisposalMethod::Keep),
                frame(&[2, 1], 5, DisposalMethod::Keep),
            ],
            Repeat::Infinite,
        );
        let mut animation =
            decode_gif(&bytes, DecodeLimits::default(), Duration::from_secs(1)).unwrap();
        animation.pause(Duration::from_millis(1_075));
        let source_address = animation
            .current_frame(Duration::from_secs(2))
            .unwrap()
            .pixels()
            .as_ptr();
        let renderer = Renderer::new();
        let mut cells = CellFrame::default();

        for (generation, viewport) in [(1, (1, 1)), (2, (8, 4)), (3, (2, 3))] {
            renderer
                .render_into(
                    animation.current_frame(Duration::from_secs(2)).unwrap(),
                    RenderRequest {
                        viewport: Viewport {
                            width: viewport.0,
                            height: viewport.1,
                        },
                        fit: FitMode::Contain,
                        generation,
                    },
                    &mut cells,
                )
                .unwrap();
        }

        assert_eq!(animation.position(Duration::from_secs(20)).frame_index, 1);
        assert_eq!(
            animation
                .current_frame(Duration::from_secs(20))
                .unwrap()
                .pixels()
                .as_ptr(),
            source_address
        );
        animation.restart(Duration::from_secs(20));
        assert_eq!(animation.position(Duration::from_secs(30)).frame_index, 0);
    }

    #[test]
    fn decoded_storage_obeys_count_and_byte_limits() {
        let frames = [
            frame(&[0, 1], 1, DisposalMethod::Keep),
            frame(&[1, 0], 1, DisposalMethod::Keep),
        ];
        let bytes = encode_gif(&frames, Repeat::Finite(0));

        assert!(matches!(
            decode_gif(
                &bytes,
                DecodeLimits {
                    max_frames: 1,
                    ..DecodeLimits::default()
                },
                Duration::ZERO
            ),
            Err(DecodeError::FrameLimit)
        ));
        assert!(matches!(
            decode_gif(
                &bytes,
                DecodeLimits {
                    max_decoded_bytes: 8,
                    ..DecodeLimits::default()
                },
                Duration::ZERO
            ),
            Err(DecodeError::ByteLimit)
        ));
    }
}
