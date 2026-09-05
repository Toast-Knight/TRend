use core::fmt;
use core::time::Duration;

use trend::{FrameError, PixelFormat, PixelFrameView};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct DecodeLimits {
    pub max_width: u32,
    pub max_height: u32,
    pub max_pixels: u64,
    pub max_buffered_frames: usize,
    pub max_decoded_bytes: usize,
}

impl Default for DecodeLimits {
    fn default() -> Self {
        Self {
            max_width: 8_192,
            max_height: 8_192,
            max_pixels: 16 * 1024 * 1024,
            max_buffered_frames: 2,
            max_decoded_bytes: 128 * 1024 * 1024,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SeekAccuracy {
    KeyframeThenForward,
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum Container {
    WebM,
    Mkv,
    Mp4,
    Avi,
    Mov,
}

impl Container {
    pub fn from_extension(extension: &str) -> Option<Self> {
        match extension.trim_start_matches('.') {
            value if value.eq_ignore_ascii_case("webm") => Some(Self::WebM),
            value if value.eq_ignore_ascii_case("mkv") => Some(Self::Mkv),
            value if value.eq_ignore_ascii_case("mp4") => Some(Self::Mp4),
            value if value.eq_ignore_ascii_case("avi") => Some(Self::Avi),
            value if value.eq_ignore_ascii_case("mov") => Some(Self::Mov),
            _ => None,
        }
    }

    pub fn from_path(path: &std::path::Path) -> Option<Self> {
        path.extension()
            .and_then(|extension| extension.to_str())
            .and_then(Self::from_extension)
    }
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum VideoCodec {
    Vp8,
    Vp9,
    Mjpeg,
    H264,
    Hevc,
    Av1,
}

impl VideoCodec {
    #[cfg(all(
        target_os = "linux",
        any(feature = "linux-webm", feature = "linux-mkv", feature = "linux-h264")
    ))]
    pub(crate) fn from_matroska_id(codec_id: &str) -> Option<Self> {
        match codec_id {
            "V_VP8" => Some(Self::Vp8),
            "V_VP9" => Some(Self::Vp9),
            "V_MPEG4/ISO/AVC" => Some(Self::H264),
            "V_MPEGH/ISO/HEVC" => Some(Self::Hevc),
            "V_AV1" => Some(Self::Av1),
            _ => None,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ContainerSupport {
    container: Container,
    codecs: &'static [VideoCodec],
}

impl ContainerSupport {
    pub const fn new(container: Container, codecs: &'static [VideoCodec]) -> Self {
        Self { container, codecs }
    }

    pub const fn container(self) -> Container {
        self.container
    }

    pub const fn codecs(self) -> &'static [VideoCodec] {
        self.codecs
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct BackendCapabilities {
    backend: &'static str,
    containers: &'static [ContainerSupport],
}

impl BackendCapabilities {
    pub const fn new(backend: &'static str, containers: &'static [ContainerSupport]) -> Self {
        Self {
            backend,
            containers,
        }
    }

    pub const fn backend(self) -> &'static str {
        self.backend
    }

    pub const fn containers(self) -> &'static [ContainerSupport] {
        self.containers
    }

    pub fn codecs_for(self, container: Container) -> &'static [VideoCodec] {
        self.containers
            .iter()
            .find(|support| support.container == container)
            .map_or(&[], |support| support.codecs)
    }

    pub fn supports(self, container: Container, codec: VideoCodec) -> bool {
        self.codecs_for(container).contains(&codec)
    }

    pub fn supports_container(self, container: Container) -> bool {
        !self.codecs_for(container).is_empty()
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct VideoInfo {
    pub width: u32,
    pub height: u32,
    pub duration: Duration,
    pub nominal_frame_rate: Option<(u32, u32)>,
    pub audio_present: bool,
    pub decoder: &'static str,
    pub seek_accuracy: SeekAccuracy,
}

#[derive(Debug)]
pub enum VideoError {
    UnsupportedPlatform,
    UnsupportedContainer {
        container: Container,
    },
    UnsupportedCombination {
        container: Container,
        codec: VideoCodec,
    },
    InvalidLimits,
    DimensionLimit,
    ByteLimit,
    SizeOverflow,
    DynamicFormatUnsupported,
    InvalidDecodedBuffer,
    Platform(String),
    Frame(FrameError),
}

impl fmt::Display for VideoError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::UnsupportedPlatform => {
                formatter.write_str("no video backend is available on this platform")
            }
            Self::UnsupportedContainer { container } => {
                write!(formatter, "unsupported video container: {container:?}")
            }
            Self::UnsupportedCombination { container, codec } => {
                write!(
                    formatter,
                    "unsupported video combination: {container:?} / {codec:?}"
                )
            }
            Self::InvalidLimits => formatter.write_str("video decode limits are invalid"),
            Self::DimensionLimit => formatter.write_str("video dimensions exceed decode limits"),
            Self::ByteLimit => formatter.write_str("video frame buffer exceeds decode limits"),
            Self::SizeOverflow => formatter.write_str("video frame dimensions overflowed"),
            Self::DynamicFormatUnsupported => {
                formatter.write_str("mid-stream video format changes are not supported")
            }
            Self::InvalidDecodedBuffer => {
                formatter.write_str("decoder returned an invalid RGB32 frame")
            }
            Self::Platform(message) => formatter.write_str(message),
            Self::Frame(error) => error.fmt(formatter),
        }
    }
}

impl std::error::Error for VideoError {}

impl From<FrameError> for VideoError {
    fn from(value: FrameError) -> Self {
        Self::Frame(value)
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct VideoFrame {
    pixels: Vec<u8>,
    width: u32,
    height: u32,
    timestamp: Duration,
    duration: Duration,
    epoch: u64,
}

impl VideoFrame {
    pub const fn timestamp(&self) -> Duration {
        self.timestamp
    }

    pub const fn duration(&self) -> Duration {
        self.duration
    }

    pub const fn epoch(&self) -> u64 {
        self.epoch
    }

    pub fn view(&self) -> Result<PixelFrameView<'_>, FrameError> {
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
            self.timestamp,
        )
    }
}

#[cfg(windows)]
mod windows_backend;

#[cfg(windows)]
pub use windows_backend::VideoDecoder;

#[cfg(all(target_os = "linux", feature = "linux-h264"))]
mod linux_h264_decoder;

#[cfg(all(
    target_os = "linux",
    any(feature = "linux-webm", feature = "linux-mkv", feature = "linux-h264")
))]
#[allow(unsafe_code)]
mod linux_webm_backend;

#[cfg(all(
    target_os = "linux",
    any(feature = "linux-webm", feature = "linux-mkv", feature = "linux-h264")
))]
pub use linux_webm_backend::VideoDecoder;

#[cfg(not(any(
    windows,
    all(
        target_os = "linux",
        any(feature = "linux-webm", feature = "linux-mkv", feature = "linux-h264")
    )
)))]
pub struct VideoDecoder {
    info: VideoInfo,
}

#[cfg(not(any(
    windows,
    all(
        target_os = "linux",
        any(feature = "linux-webm", feature = "linux-mkv", feature = "linux-h264")
    )
)))]
impl VideoDecoder {
    pub const fn capabilities() -> BackendCapabilities {
        BackendCapabilities::new("Unsupported platform", &[])
    }

    pub fn open(_path: &std::path::Path, _limits: DecodeLimits) -> Result<Self, VideoError> {
        Err(VideoError::UnsupportedPlatform)
    }

    pub const fn info(&self) -> &VideoInfo {
        &self.info
    }

    pub const fn buffered_frames(&self) -> usize {
        0
    }

    pub const fn dropped_frames(&self) -> u64 {
        0
    }

    pub const fn decoded_frames(&self) -> u64 {
        0
    }

    pub const fn seek_discarded_frames(&self) -> u64 {
        0
    }

    pub const fn retained_bytes(&self) -> usize {
        0
    }

    pub const fn seek_frame_timestamp(&self) -> Option<Duration> {
        None
    }

    pub fn frame_for_time(&mut self, _target: Duration) -> Result<Option<&VideoFrame>, VideoError> {
        Err(VideoError::UnsupportedPlatform)
    }

    pub fn seek(&mut self, _target: Duration) -> Result<(), VideoError> {
        Err(VideoError::UnsupportedPlatform)
    }

    pub fn restart(&mut self) -> Result<(), VideoError> {
        Err(VideoError::UnsupportedPlatform)
    }
}

#[cfg(test)]
mod capability_tests {
    use super::*;

    #[test]
    fn recognizes_all_planned_container_extensions() {
        assert_eq!(Container::from_extension("webm"), Some(Container::WebM));
        assert_eq!(Container::from_extension(".MKV"), Some(Container::Mkv));
        assert_eq!(Container::from_extension("mp4"), Some(Container::Mp4));
        assert_eq!(Container::from_extension("AVI"), Some(Container::Avi));
        assert_eq!(Container::from_extension("mov"), Some(Container::Mov));
        assert_eq!(Container::from_extension("gif"), None);
    }
}
