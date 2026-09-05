use core::fmt;
use core::time::Duration;
use std::fs;
use std::path::Path;

use trend::{FrameError, PixelFrameView, PlaybackClock};
#[cfg(feature = "webp")]
use trend_animation::decode_webp as decode_animated_webp;
use trend_animation::{
    DecodeError as AnimationError, DecodeLimits as AnimationLimits, DecodedAnimation, decode_gif,
};
#[cfg(feature = "webp")]
use trend_image::decode_webp as decode_static_webp;
use trend_image::{
    DecodeError as ImageError, DecodeLimits as ImageLimits, DecodedImage, decode_jpeg, decode_png,
};
pub use trend_video::SeekAccuracy;
use trend_video::{Container, DecodeLimits as VideoLimits, VideoDecoder, VideoError};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum MediaKind {
    Image,
    Animation,
    Video,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct MediaInfo {
    pub kind: MediaKind,
    pub width: u32,
    pub height: u32,
    pub duration: Option<Duration>,
    pub nominal_frame_rate: Option<(u32, u32)>,
    pub audio_present: bool,
    pub seek_accuracy: Option<SeekAccuracy>,
}

#[derive(Clone, Copy, Debug)]
pub struct MediaFrame<'a> {
    pub view: PixelFrameView<'a>,
    pub requested_timestamp: Duration,
    pub presentation_timestamp: Duration,
    pub duration: Option<Duration>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct SeekResult {
    pub requested_timestamp: Duration,
    pub first_frame_timestamp: Option<Duration>,
    pub accuracy: Option<SeekAccuracy>,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct MediaLimits {
    pub image: ImageLimits,
    pub animation: AnimationLimits,
    pub video: VideoLimits,
}

#[derive(Debug)]
pub enum MediaError {
    Io(std::io::Error),
    Image(ImageError),
    Animation(AnimationError),
    Video(VideoError),
    Frame(FrameError),
    UnsupportedExtension(String),
    StaticSource,
    InvalidRange,
}

impl fmt::Display for MediaError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io(error) => error.fmt(formatter),
            Self::Image(error) => error.fmt(formatter),
            Self::Animation(error) => error.fmt(formatter),
            Self::Video(error) => error.fmt(formatter),
            Self::Frame(error) => error.fmt(formatter),
            Self::UnsupportedExtension(extension) => {
                write!(formatter, "unsupported media extension: {extension}")
            }
            Self::StaticSource => formatter.write_str("static media has no playback timeline"),
            Self::InvalidRange => {
                formatter.write_str("preview range is empty or outside the media")
            }
        }
    }
}

impl std::error::Error for MediaError {}

impl From<std::io::Error> for MediaError {
    fn from(value: std::io::Error) -> Self {
        Self::Io(value)
    }
}

impl From<ImageError> for MediaError {
    fn from(value: ImageError) -> Self {
        Self::Image(value)
    }
}

impl From<AnimationError> for MediaError {
    fn from(value: AnimationError) -> Self {
        Self::Animation(value)
    }
}

impl From<VideoError> for MediaError {
    fn from(value: VideoError) -> Self {
        Self::Video(value)
    }
}

impl From<FrameError> for MediaError {
    fn from(value: FrameError) -> Self {
        Self::Frame(value)
    }
}

enum DecodedMedia {
    Image(DecodedImage),
    Animation(DecodedAnimation),
    Video {
        decoder: VideoDecoder,
        last_target: Option<Duration>,
    },
}

pub struct MediaSource {
    media: DecodedMedia,
    info: MediaInfo,
}

impl MediaSource {
    pub fn open(path: &Path, limits: MediaLimits) -> Result<Self, MediaError> {
        let extension = path
            .extension()
            .and_then(|value| value.to_str())
            .unwrap_or("")
            .to_ascii_lowercase();
        match extension.as_str() {
            "png" => Self::from_image(decode_png(&fs::read(path)?, limits.image)?),
            "jpg" | "jpeg" => Self::from_image(decode_jpeg(&fs::read(path)?, limits.image)?),
            "gif" => Self::from_animation(decode_gif(
                &fs::read(path)?,
                limits.animation,
                Duration::ZERO,
            )?),
            #[cfg(feature = "webp")]
            "webp" => {
                let bytes = fs::read(path)?;
                match decode_static_webp(&bytes, limits.image) {
                    Ok(image) => Self::from_image(image),
                    Err(ImageError::UnsupportedAnimation) => Self::from_animation(
                        decode_animated_webp(&bytes, limits.animation, Duration::ZERO)?,
                    ),
                    Err(error) => Err(error.into()),
                }
            }
            _ => {
                let Some(container) = Container::from_extension(&extension) else {
                    return Err(MediaError::UnsupportedExtension(extension));
                };
                if !video_container_enabled(container) {
                    return Err(MediaError::UnsupportedExtension(extension));
                }
                if !VideoDecoder::capabilities().supports_container(container) {
                    return Err(VideoError::UnsupportedContainer { container }.into());
                }
                Self::from_video(VideoDecoder::open(path, limits.video)?)
            }
        }
    }

    pub const fn info(&self) -> MediaInfo {
        self.info
    }

    pub fn representative_frame(&mut self) -> Result<Option<PixelFrameView<'_>>, MediaError> {
        self.frame_at(Duration::ZERO)
    }

    pub fn frame_at(
        &mut self,
        media_time: Duration,
    ) -> Result<Option<PixelFrameView<'_>>, MediaError> {
        Ok(self
            .frame_at_with_timing(media_time)?
            .map(|frame| frame.view))
    }

    pub fn frame_at_with_timing(
        &mut self,
        media_time: Duration,
    ) -> Result<Option<MediaFrame<'_>>, MediaError> {
        match &mut self.media {
            DecodedMedia::Image(image) => {
                let view = image.view(Duration::ZERO)?;
                Ok(Some(MediaFrame {
                    view,
                    requested_timestamp: media_time,
                    presentation_timestamp: view.timestamp(),
                    duration: None,
                }))
            }
            DecodedMedia::Animation(animation) => {
                let position = animation.position(media_time);
                let duration = animation.frame_duration(position.frame_index)?;
                let view = animation.frame_at(position.frame_index)?;
                Ok(Some(MediaFrame {
                    view,
                    requested_timestamp: media_time,
                    presentation_timestamp: view.timestamp(),
                    duration: Some(duration),
                }))
            }
            DecodedMedia::Video {
                decoder,
                last_target,
            } => {
                if last_target.is_some_and(|previous| media_time < previous) {
                    decoder.seek(media_time)?;
                }
                let frame = decoder.frame_for_time(media_time)?;
                *last_target = Some(media_time);
                match frame {
                    Some(frame) => {
                        let duration = frame.duration();
                        let view = frame.view()?;
                        Ok(Some(MediaFrame {
                            view,
                            requested_timestamp: media_time,
                            presentation_timestamp: view.timestamp(),
                            duration: Some(duration),
                        }))
                    }
                    None => Ok(None),
                }
            }
        }
    }

    pub fn seek(&mut self, media_time: Duration) -> Result<(), MediaError> {
        self.seek_with_timing(media_time).map(|_| ())
    }

    pub fn seek_with_timing(&mut self, media_time: Duration) -> Result<SeekResult, MediaError> {
        let mut first_frame_timestamp = None;
        if let DecodedMedia::Video {
            decoder,
            last_target,
        } = &mut self.media
        {
            decoder.seek(media_time)?;
            *last_target = Some(media_time);
            first_frame_timestamp = decoder.seek_frame_timestamp();
        } else if let DecodedMedia::Animation(animation) = &self.media {
            let position = animation.position(media_time);
            first_frame_timestamp = Some(animation.frame_timestamp(position.frame_index)?);
        } else if matches!(self.media, DecodedMedia::Image(_)) {
            first_frame_timestamp = Some(Duration::ZERO);
        }
        Ok(SeekResult {
            requested_timestamp: media_time,
            first_frame_timestamp,
            accuracy: self.info.seek_accuracy,
        })
    }

    pub fn retained_bytes(&self) -> usize {
        match &self.media {
            DecodedMedia::Image(image) => image.pixels().len(),
            DecodedMedia::Animation(animation) => animation.decoded_bytes(),
            DecodedMedia::Video { decoder, .. } => decoder.retained_bytes(),
        }
    }

    pub fn decoded_frames(&self) -> u64 {
        match &self.media {
            DecodedMedia::Image(_) => 1,
            DecodedMedia::Animation(animation) => animation.frame_count() as u64,
            DecodedMedia::Video { decoder, .. } => decoder.decoded_frames(),
        }
    }

    pub fn dropped_frames(&self) -> u64 {
        match &self.media {
            DecodedMedia::Video { decoder, .. } => decoder.dropped_frames(),
            DecodedMedia::Image(_) | DecodedMedia::Animation(_) => 0,
        }
    }

    pub fn seek_discarded_frames(&self) -> u64 {
        match &self.media {
            DecodedMedia::Video { decoder, .. } => decoder.seek_discarded_frames(),
            DecodedMedia::Image(_) | DecodedMedia::Animation(_) => 0,
        }
    }

    pub fn buffered_frames(&self) -> usize {
        match &self.media {
            DecodedMedia::Image(_) => 1,
            DecodedMedia::Animation(animation) => animation.frame_count(),
            DecodedMedia::Video { decoder, .. } => decoder.buffered_frames(),
        }
    }

    fn from_image(image: DecodedImage) -> Result<Self, MediaError> {
        let info = MediaInfo {
            kind: MediaKind::Image,
            width: image.width(),
            height: image.height(),
            duration: None,
            nominal_frame_rate: None,
            audio_present: false,
            seek_accuracy: None,
        };
        Ok(Self {
            media: DecodedMedia::Image(image),
            info,
        })
    }

    fn from_animation(animation: DecodedAnimation) -> Result<Self, MediaError> {
        let info = MediaInfo {
            kind: MediaKind::Animation,
            width: animation.width(),
            height: animation.height(),
            duration: Some(animation.duration()),
            nominal_frame_rate: None,
            audio_present: false,
            seek_accuracy: None,
        };
        Ok(Self {
            media: DecodedMedia::Animation(animation),
            info,
        })
    }

    fn from_video(decoder: VideoDecoder) -> Result<Self, MediaError> {
        let info = MediaInfo {
            kind: MediaKind::Video,
            width: decoder.info().width,
            height: decoder.info().height,
            duration: Some(decoder.info().duration),
            nominal_frame_rate: decoder.info().nominal_frame_rate,
            audio_present: decoder.info().audio_present,
            seek_accuracy: Some(decoder.info().seek_accuracy),
        };
        Ok(Self {
            media: DecodedMedia::Video {
                decoder,
                last_target: None,
            },
            info,
        })
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PreviewRange {
    pub start: Duration,
    pub duration: Duration,
    pub looping: bool,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SessionState {
    Stopped,
    Playing,
    Paused,
    Completed,
    Cancelled,
}

pub struct PreviewSession {
    source: MediaSource,
    clock: PlaybackClock,
    range: Option<PreviewRange>,
    state: SessionState,
}

impl PreviewSession {
    pub fn new(source: MediaSource, wall_now: Duration) -> Self {
        Self {
            source,
            clock: PlaybackClock::new(wall_now),
            range: None,
            state: SessionState::Stopped,
        }
    }

    pub const fn source(&self) -> &MediaSource {
        &self.source
    }

    pub const fn state(&self) -> SessionState {
        self.state
    }

    pub const fn info(&self) -> MediaInfo {
        self.source.info()
    }

    pub fn representative_frame(&mut self) -> Result<Option<PixelFrameView<'_>>, MediaError> {
        self.source.representative_frame()
    }

    pub fn frame_at_media_time(
        &mut self,
        media_time: Duration,
    ) -> Result<Option<MediaFrame<'_>>, MediaError> {
        self.source.frame_at_with_timing(media_time)
    }

    pub fn seek_media_time(&mut self, media_time: Duration) -> Result<SeekResult, MediaError> {
        self.source.seek_with_timing(media_time)
    }

    pub fn start(&mut self, range: PreviewRange, wall_now: Duration) -> Result<(), MediaError> {
        let source_duration = self.source.info.duration.ok_or(MediaError::StaticSource)?;
        if range.duration.is_zero() || range.start >= source_duration {
            return Err(MediaError::InvalidRange);
        }
        let duration = range.duration.min(source_duration - range.start);
        self.range = Some(PreviewRange { duration, ..range });
        self.clock.seek(Duration::ZERO, wall_now);
        self.clock.play(wall_now);
        self.source.seek(range.start)?;
        self.state = SessionState::Playing;
        Ok(())
    }

    pub fn frame_for_time(
        &mut self,
        wall_now: Duration,
    ) -> Result<Option<PixelFrameView<'_>>, MediaError> {
        if matches!(
            self.state,
            SessionState::Stopped | SessionState::Completed | SessionState::Cancelled
        ) {
            return Ok(None);
        }
        let range = self.range.ok_or(MediaError::InvalidRange)?;
        let elapsed = self.clock.media_time(wall_now);
        if !range.looping && elapsed >= range.duration {
            self.state = SessionState::Completed;
            return Ok(None);
        }
        let offset = if range.looping {
            duration_from_nanos(elapsed.as_nanos() % range.duration.as_nanos())
        } else {
            elapsed
        };
        self.source.frame_at(range.start.saturating_add(offset))
    }

    pub fn pause(&mut self, wall_now: Duration) {
        if self.state == SessionState::Playing {
            self.clock.pause(wall_now);
            self.state = SessionState::Paused;
        }
    }

    pub fn resume(&mut self, wall_now: Duration) {
        if self.state == SessionState::Paused {
            self.clock.play(wall_now);
            self.state = SessionState::Playing;
        }
    }

    pub fn seek(&mut self, offset: Duration, wall_now: Duration) -> Result<(), MediaError> {
        let range = self.range.ok_or(MediaError::InvalidRange)?;
        if offset >= range.duration {
            return Err(MediaError::InvalidRange);
        }
        self.clock.seek(offset, wall_now);
        self.source.seek(range.start.saturating_add(offset))?;
        Ok(())
    }

    pub fn restart(&mut self, wall_now: Duration) -> Result<(), MediaError> {
        let range = self.range.ok_or(MediaError::InvalidRange)?;
        self.start(range, wall_now)
    }

    pub fn stop(&mut self) {
        self.state = SessionState::Stopped;
    }

    pub fn cancel(&mut self) {
        self.state = SessionState::Cancelled;
    }

    pub fn into_source(self) -> MediaSource {
        self.source
    }
}

fn duration_from_nanos(value: u128) -> Duration {
    Duration::new(
        u64::try_from(value / 1_000_000_000).unwrap_or(u64::MAX),
        (value % 1_000_000_000) as u32,
    )
}

fn video_container_enabled(container: Container) -> bool {
    match container {
        Container::Mp4 => true,
        Container::WebM => cfg!(feature = "webm"),
        Container::Mkv => cfg!(feature = "mkv"),
        Container::Avi => cfg!(feature = "avi"),
        Container::Mov => cfg!(feature = "mov"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use trend::{CellFrame, FitMode, RenderRequest, Renderer, Viewport};

    fn fixture(name: &str) -> std::path::PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../..")
            .join("Test runs Envs/Materials")
            .join(name)
    }

    #[cfg(feature = "webp")]
    #[test]
    fn opens_static_and_animated_webp_through_unified_media_api() {
        let mut image = MediaSource::open(
            &fixture("WebP Spike/WebPAlpha.webp"),
            MediaLimits::default(),
        )
        .unwrap();
        assert_eq!(image.info().kind, MediaKind::Image);
        assert!(image.representative_frame().unwrap().is_some());

        let mut animation = MediaSource::open(
            &fixture("WebP Spike/WebPAnimatedRejected.webp"),
            MediaLimits::default(),
        )
        .unwrap();
        assert_eq!(animation.info().kind, MediaKind::Animation);
        let frame = animation
            .frame_at_with_timing(Duration::from_millis(1))
            .unwrap()
            .unwrap();
        assert_eq!(frame.requested_timestamp, Duration::from_millis(1));
        assert!(frame.duration.is_some_and(|duration| !duration.is_zero()));
    }

    #[cfg(any(
        all(windows, feature = "webm"),
        all(target_os = "linux", feature = "linux-webm")
    ))]
    #[test]
    fn opens_and_renders_webm_with_existing_media_timing_api() {
        let mut source = MediaSource::open(
            &fixture("WebM Experimental/vp8-normal.webm"),
            MediaLimits::default(),
        )
        .unwrap();
        assert_eq!(source.info().kind, MediaKind::Video);
        let requested = Duration::from_millis(100);
        let frame = source.frame_at_with_timing(requested).unwrap().unwrap();
        assert_eq!(frame.requested_timestamp, requested);
        assert_eq!(frame.presentation_timestamp, frame.view.timestamp());
        assert!(frame.duration.is_some_and(|duration| !duration.is_zero()));

        let mut cells = CellFrame::default();
        Renderer::new()
            .render_into(
                frame.view,
                RenderRequest {
                    viewport: Viewport {
                        width: 20,
                        height: 10,
                    },
                    fit: FitMode::Contain,
                    generation: 1,
                },
                &mut cells,
            )
            .unwrap();
        assert!(cells.cells().iter().any(|cell| cell.glyph != ' ' as u32));
    }

    #[cfg(any(
        all(windows, feature = "mkv"),
        all(target_os = "linux", feature = "linux-mkv")
    ))]
    #[test]
    fn opens_and_renders_mkv_with_existing_media_timing_api() {
        let mut source = MediaSource::open(
            &fixture("MKV Experimental/vp9-large.mkv"),
            MediaLimits::default(),
        )
        .unwrap();
        assert_eq!(source.info().kind, MediaKind::Video);
        let requested = Duration::from_millis(100);
        let frame = source.frame_at_with_timing(requested).unwrap().unwrap();
        assert_eq!(frame.requested_timestamp, requested);
        assert_eq!(frame.presentation_timestamp, frame.view.timestamp());
        assert!(frame.duration.is_some_and(|duration| !duration.is_zero()));

        let mut cells = CellFrame::default();
        Renderer::new()
            .render_into(
                frame.view,
                RenderRequest {
                    viewport: Viewport {
                        width: 20,
                        height: 10,
                    },
                    fit: FitMode::Contain,
                    generation: 1,
                },
                &mut cells,
            )
            .unwrap();
        assert!(cells.cells().iter().any(|cell| cell.glyph != ' ' as u32));
    }

    #[cfg(all(target_os = "linux", feature = "linux-h264"))]
    #[test]
    fn opens_and_renders_h264_mkv_with_existing_media_timing_api() {
        let mut source = MediaSource::open(
            &fixture("MKV Experimental/h264-linux-normal.mkv"),
            MediaLimits::default(),
        )
        .unwrap();
        assert_eq!(source.info().kind, MediaKind::Video);
        assert_eq!((source.info().width, source.info().height), (320, 192));

        let requested = Duration::from_millis(250);
        let frame = source.frame_at_with_timing(requested).unwrap().unwrap();
        assert_eq!(frame.requested_timestamp, requested);
        assert_eq!(frame.presentation_timestamp, frame.view.timestamp());
        assert!(frame.duration.is_some_and(|duration| !duration.is_zero()));

        let mut cells = CellFrame::default();
        Renderer::new()
            .render_into(
                frame.view,
                RenderRequest {
                    viewport: Viewport {
                        width: 20,
                        height: 10,
                    },
                    fit: FitMode::Contain,
                    generation: 1,
                },
                &mut cells,
            )
            .unwrap();
        assert!(cells.cells().iter().any(|cell| cell.glyph != ' ' as u32));
    }

    #[cfg(all(windows, feature = "avi"))]
    #[test]
    fn opens_and_renders_avi_with_existing_media_timing_api() {
        let mut source = MediaSource::open(
            &fixture("AVI Experimental/mjpeg-normal.avi"),
            MediaLimits::default(),
        )
        .unwrap();
        assert_eq!(source.info().kind, MediaKind::Video);
        assert_eq!((source.info().width, source.info().height), (320, 180));

        let requested = Duration::from_millis(250);
        let frame = source.frame_at_with_timing(requested).unwrap().unwrap();
        assert_eq!(frame.requested_timestamp, requested);
        assert_eq!(frame.presentation_timestamp, frame.view.timestamp());
        assert!(frame.duration.is_some_and(|duration| !duration.is_zero()));

        let mut cells = CellFrame::default();
        Renderer::new()
            .render_into(
                frame.view,
                RenderRequest {
                    viewport: Viewport {
                        width: 20,
                        height: 10,
                    },
                    fit: FitMode::Contain,
                    generation: 1,
                },
                &mut cells,
            )
            .unwrap();
        assert!(cells.cells().iter().any(|cell| cell.glyph != ' ' as u32));
    }

    #[cfg(all(windows, feature = "mov"))]
    #[test]
    fn opens_and_renders_mov_with_existing_media_timing_api() {
        let mut source = MediaSource::open(
            &fixture("MOV Experimental/h264-normal.mov"),
            MediaLimits::default(),
        )
        .unwrap();
        assert_eq!(source.info().kind, MediaKind::Video);
        assert_eq!((source.info().width, source.info().height), (320, 192));

        let requested = Duration::from_millis(250);
        let frame = source.frame_at_with_timing(requested).unwrap().unwrap();
        assert_eq!(frame.requested_timestamp, requested);
        assert_eq!(frame.presentation_timestamp, frame.view.timestamp());
        assert!(frame.duration.is_some_and(|duration| !duration.is_zero()));

        let mut cells = CellFrame::default();
        Renderer::new()
            .render_into(
                frame.view,
                RenderRequest {
                    viewport: Viewport {
                        width: 20,
                        height: 10,
                    },
                    fit: FitMode::Contain,
                    generation: 1,
                },
                &mut cells,
            )
            .unwrap();
        assert!(cells.cells().iter().any(|cell| cell.glyph != ' ' as u32));
    }

    #[cfg(all(
        target_os = "linux",
        feature = "avi",
        any(feature = "linux-webm", feature = "linux-mkv", feature = "linux-h264")
    ))]
    #[test]
    fn reports_avi_as_unsupported_by_the_linux_backend() {
        let error = MediaSource::open(
            &fixture("AVI Experimental/mjpeg-normal.avi"),
            MediaLimits::default(),
        )
        .err()
        .unwrap();
        assert!(matches!(
            error,
            MediaError::Video(VideoError::UnsupportedContainer {
                container: Container::Avi
            })
        ));
    }

    #[cfg(all(
        target_os = "linux",
        feature = "mov",
        any(feature = "linux-webm", feature = "linux-mkv", feature = "linux-h264")
    ))]
    #[test]
    fn reports_mov_as_unsupported_by_the_linux_backend() {
        let error = MediaSource::open(
            &fixture("MOV Experimental/h264-normal.mov"),
            MediaLimits::default(),
        )
        .err()
        .unwrap();
        assert!(matches!(
            error,
            MediaError::Video(VideoError::UnsupportedContainer {
                container: Container::Mov
            })
        ));
    }

    #[test]
    fn representative_static_frame_is_borrowed_and_reused() {
        let mut source =
            MediaSource::open(&fixture("ColorTest.png"), MediaLimits::default()).unwrap();
        let first = source
            .representative_frame()
            .unwrap()
            .unwrap()
            .pixels()
            .as_ptr();
        let second = source
            .representative_frame()
            .unwrap()
            .unwrap()
            .pixels()
            .as_ptr();

        assert_eq!(first, second);
        assert_eq!(source.info.kind, MediaKind::Image);
    }

    #[test]
    fn representative_gif_frame_and_resize_reuse_decoded_storage() {
        let mut source =
            MediaSource::open(&fixture("ColorTest.gif"), MediaLimits::default()).unwrap();
        let retained = source.retained_bytes();
        let first = source
            .representative_frame()
            .unwrap()
            .unwrap()
            .pixels()
            .as_ptr();
        let repeated = source
            .representative_frame()
            .unwrap()
            .unwrap()
            .pixels()
            .as_ptr();
        assert_eq!(first, repeated);

        let renderer = Renderer::new();
        let mut cells = CellFrame::default();
        for (generation, viewport) in [(1, (20, 8)), (2, (80, 24)), (3, (40, 12))] {
            renderer
                .render_into(
                    source.representative_frame().unwrap().unwrap(),
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
        assert_eq!(source.retained_bytes(), retained);
        assert_eq!(
            source
                .representative_frame()
                .unwrap()
                .unwrap()
                .pixels()
                .as_ptr(),
            first
        );
    }

    #[test]
    fn gif_session_supports_pause_resume_seek_loop_completion_and_cancel() {
        let source = MediaSource::open(&fixture("ColorTest.gif"), MediaLimits::default()).unwrap();
        let mut session = PreviewSession::new(source, Duration::ZERO);
        session
            .start(
                PreviewRange {
                    start: Duration::ZERO,
                    duration: Duration::from_millis(100),
                    looping: true,
                },
                Duration::ZERO,
            )
            .unwrap();
        let looped = session
            .frame_for_time(Duration::from_millis(150))
            .unwrap()
            .unwrap()
            .pixels()
            .as_ptr();
        session.restart(Duration::ZERO).unwrap();
        let direct = session
            .frame_for_time(Duration::from_millis(50))
            .unwrap()
            .unwrap()
            .pixels()
            .as_ptr();
        assert_eq!(looped, direct);
        session.pause(Duration::from_millis(160));
        assert_eq!(session.state(), SessionState::Paused);
        let paused_at_anchor = session
            .frame_for_time(Duration::from_millis(160))
            .unwrap()
            .unwrap()
            .pixels()
            .as_ptr();
        let paused = session
            .frame_for_time(Duration::from_secs(20))
            .unwrap()
            .unwrap()
            .pixels()
            .as_ptr();
        assert_eq!(paused, paused_at_anchor);
        session.resume(Duration::from_millis(300));
        session
            .seek(Duration::from_millis(50), Duration::from_millis(310))
            .unwrap();
        session.restart(Duration::from_millis(320)).unwrap();
        session.stop();
        assert!(
            session
                .frame_for_time(Duration::from_secs(1))
                .unwrap()
                .is_none()
        );
        session.cancel();
        assert_eq!(session.state(), SessionState::Cancelled);
        assert!(
            session
                .frame_for_time(Duration::from_secs(2))
                .unwrap()
                .is_none()
        );

        session
            .start(
                PreviewRange {
                    start: Duration::ZERO,
                    duration: Duration::from_millis(50),
                    looping: false,
                },
                Duration::ZERO,
            )
            .unwrap();
        assert!(
            session
                .frame_for_time(Duration::from_millis(49))
                .unwrap()
                .is_some()
        );
        assert!(
            session
                .frame_for_time(Duration::from_millis(50))
                .unwrap()
                .is_none()
        );
        assert_eq!(session.state(), SessionState::Completed);
    }

    #[test]
    fn direct_media_time_reports_requested_and_actual_gif_frame_timing() {
        let source = MediaSource::open(&fixture("ColorTest.gif"), MediaLimits::default()).unwrap();
        let mut session = PreviewSession::new(source, Duration::ZERO);
        let requested = Duration::from_millis(55);

        let seek = session.seek_media_time(requested).unwrap();
        let frame = session.frame_at_media_time(requested).unwrap().unwrap();

        assert_eq!(seek.requested_timestamp, requested);
        assert_eq!(
            seek.first_frame_timestamp,
            Some(frame.presentation_timestamp)
        );
        assert_eq!(frame.requested_timestamp, requested);
        assert_eq!(frame.presentation_timestamp, frame.view.timestamp());
        assert!(frame.presentation_timestamp <= requested);
        assert!(frame.duration.is_some_and(|duration| !duration.is_zero()));
    }

    #[cfg(windows)]
    #[test]
    fn video_reuses_decoder_from_thumbnail_through_preview_and_full_range() {
        let source = MediaSource::open(&fixture("ColorTest.mp4"), MediaLimits::default()).unwrap();
        let mut session = PreviewSession::new(source, Duration::ZERO);
        let thumbnail = session
            .representative_frame()
            .unwrap()
            .unwrap()
            .pixels()
            .as_ptr();
        let repeated_thumbnail = session
            .representative_frame()
            .unwrap()
            .unwrap()
            .pixels()
            .as_ptr();
        assert_eq!(thumbnail, repeated_thumbnail);
        session
            .start(
                PreviewRange {
                    start: Duration::ZERO,
                    duration: Duration::from_millis(200),
                    looping: true,
                },
                Duration::ZERO,
            )
            .unwrap();
        assert!(
            session
                .frame_for_time(Duration::from_millis(250))
                .unwrap()
                .is_some()
        );
        let decoded_after_preview = session.source().decoded_frames();
        let duration = session.source().info().duration.unwrap();
        session
            .start(
                PreviewRange {
                    start: Duration::ZERO,
                    duration,
                    looping: false,
                },
                Duration::from_secs(1),
            )
            .unwrap();
        assert!(
            session
                .frame_for_time(Duration::from_secs(1))
                .unwrap()
                .is_some()
        );

        assert!(session.source().decoded_frames() >= decoded_after_preview);
        assert!(session.source().buffered_frames() <= 2);
        assert!(
            session.source().retained_bytes() <= MediaLimits::default().video.max_decoded_bytes
        );
    }
}
