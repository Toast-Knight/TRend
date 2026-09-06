use core::time::Duration;
use std::collections::VecDeque;
use std::fs::File;
use std::path::{Path, PathBuf};

#[cfg(any(feature = "linux-webm", feature = "linux-mkv"))]
use core::ptr;
#[cfg(any(feature = "linux-webm", feature = "linux-mkv"))]
use std::slice;

use matroska_demuxer::{Frame, MatroskaFile, TrackType};
#[cfg(any(feature = "linux-webm", feature = "linux-mkv"))]
use vpx_sys::{
    VPX_DECODER_ABI_VERSION, vpx_codec_ctx, vpx_codec_dec_cfg, vpx_codec_dec_init_ver,
    vpx_codec_decode, vpx_codec_destroy, vpx_codec_err_t, vpx_codec_get_frame, vpx_codec_iter_t,
    vpx_codec_vp8_dx, vpx_codec_vp9_dx, vpx_color_range, vpx_color_space, vpx_image, vpx_img_fmt,
};

#[cfg(feature = "linux-h264")]
use crate::linux_h264_decoder::H264Decoder;
use crate::{
    BackendCapabilities, Container, ContainerSupport, DecodeLimits, SeekAccuracy, VideoCodec,
    VideoError, VideoFrame, VideoInfo,
};

const CAPABILITIES: BackendCapabilities = BackendCapabilities::new(
    "Linux Matroska codecs",
    &[
        #[cfg(any(feature = "linux-webm", feature = "linux-mkv"))]
        ContainerSupport::new(Container::WebM, &[VideoCodec::Vp8, VideoCodec::Vp9]),
        #[cfg(all(
            any(feature = "linux-webm", feature = "linux-mkv"),
            not(feature = "linux-h264")
        ))]
        ContainerSupport::new(Container::Mkv, &[VideoCodec::Vp8, VideoCodec::Vp9]),
        #[cfg(all(
            any(feature = "linux-webm", feature = "linux-mkv"),
            feature = "linux-h264"
        ))]
        ContainerSupport::new(
            Container::Mkv,
            &[VideoCodec::Vp8, VideoCodec::Vp9, VideoCodec::H264],
        ),
        #[cfg(all(
            not(any(feature = "linux-webm", feature = "linux-mkv")),
            feature = "linux-h264"
        ))]
        ContainerSupport::new(Container::Mkv, &[VideoCodec::H264]),
    ],
);

#[cfg(any(feature = "linux-webm", feature = "linux-mkv"))]
#[derive(Clone, Copy)]
enum VpxCodec {
    Vp8,
    Vp9,
}

#[cfg(any(feature = "linux-webm", feature = "linux-mkv"))]
impl VpxCodec {
    fn from_codec(codec: VideoCodec) -> Result<Self, VideoError> {
        match codec {
            VideoCodec::Vp8 => Ok(Self::Vp8),
            VideoCodec::Vp9 => Ok(Self::Vp9),
            _ => Err(VideoError::Platform(
                "capability matrix admitted a codec unsupported by libvpx".into(),
            )),
        }
    }
}

#[cfg(any(feature = "linux-webm", feature = "linux-mkv"))]
struct VpxDecoder {
    context: vpx_codec_ctx,
}

#[cfg(any(feature = "linux-webm", feature = "linux-mkv"))]
impl VpxDecoder {
    fn new(codec: VpxCodec, width: u32, height: u32) -> Result<Self, VideoError> {
        let interface = unsafe {
            match codec {
                VpxCodec::Vp8 => vpx_codec_vp8_dx(),
                VpxCodec::Vp9 => vpx_codec_vp9_dx(),
            }
        };
        let mut context = vpx_codec_ctx::default();
        let config = vpx_codec_dec_cfg {
            threads: 1,
            w: width,
            h: height,
        };
        let result = unsafe {
            vpx_codec_dec_init_ver(
                &mut context,
                interface,
                &config,
                0,
                VPX_DECODER_ABI_VERSION as i32,
            )
        };
        check_vpx(result, "initialize decoder")?;
        Ok(Self { context })
    }

    fn decode(&mut self, packet: &[u8]) -> Result<Option<&vpx_image>, VideoError> {
        let packet_len = u32::try_from(packet.len()).map_err(|_| VideoError::ByteLimit)?;
        let result = unsafe {
            vpx_codec_decode(
                &mut self.context,
                packet.as_ptr(),
                packet_len,
                ptr::null_mut(),
                0,
            )
        };
        check_vpx(result, "decode frame")?;

        let mut iterator: vpx_codec_iter_t = ptr::null();
        let image = unsafe { vpx_codec_get_frame(&mut self.context, &mut iterator) };
        Ok(unsafe { image.as_ref() })
    }
}

#[cfg(any(feature = "linux-webm", feature = "linux-mkv"))]
impl Drop for VpxDecoder {
    fn drop(&mut self) {
        unsafe {
            vpx_codec_destroy(&mut self.context);
        }
    }
}

#[derive(Clone)]
enum CodecConfiguration {
    #[cfg(any(feature = "linux-webm", feature = "linux-mkv"))]
    Vpx(VpxCodec),
    #[cfg(feature = "linux-h264")]
    H264 { codec_private: Vec<u8> },
}

impl CodecConfiguration {
    fn from_track(codec: VideoCodec, codec_private: Option<&[u8]>) -> Result<Self, VideoError> {
        match codec {
            #[cfg(any(feature = "linux-webm", feature = "linux-mkv"))]
            VideoCodec::Vp8 | VideoCodec::Vp9 => Ok(Self::Vpx(VpxCodec::from_codec(codec)?)),
            #[cfg(feature = "linux-h264")]
            VideoCodec::H264 => Ok(Self::H264 {
                codec_private: codec_private
                    .ok_or_else(|| {
                        VideoError::Platform("H.264 track has no codec private data".into())
                    })?
                    .to_vec(),
            }),
            _ => Err(VideoError::Platform(
                "capability matrix admitted a codec without a Linux decoder".into(),
            )),
        }
    }

    const fn decoder_name(&self) -> &'static str {
        match self {
            #[cfg(any(feature = "linux-webm", feature = "linux-mkv"))]
            Self::Vpx(_) => "Linux Matroska/libvpx",
            #[cfg(feature = "linux-h264")]
            Self::H264 { .. } => "Linux Matroska/OpenH264",
        }
    }
}

enum CodecDecoder {
    #[cfg(any(feature = "linux-webm", feature = "linux-mkv"))]
    Vpx(VpxDecoder),
    #[cfg(feature = "linux-h264")]
    H264(Box<H264Decoder>),
}

impl CodecDecoder {
    fn new(
        configuration: &CodecConfiguration,
        _width: u32,
        _height: u32,
    ) -> Result<Self, VideoError> {
        match configuration {
            #[cfg(any(feature = "linux-webm", feature = "linux-mkv"))]
            CodecConfiguration::Vpx(codec) => {
                Ok(Self::Vpx(VpxDecoder::new(*codec, _width, _height)?))
            }
            #[cfg(feature = "linux-h264")]
            CodecConfiguration::H264 { codec_private } => {
                Ok(Self::H264(Box::new(H264Decoder::new(codec_private)?)))
            }
        }
    }

    fn decode_into(
        &mut self,
        packet: &[u8],
        width: u32,
        height: u32,
        output: &mut Vec<u8>,
    ) -> Result<bool, VideoError> {
        match self {
            #[cfg(any(feature = "linux-webm", feature = "linux-mkv"))]
            Self::Vpx(decoder) => {
                let Some(image) = decoder.decode(packet)? else {
                    return Ok(false);
                };
                if image.d_w != width || image.d_h != height {
                    return Err(VideoError::DynamicFormatUnsupported);
                }
                convert_to_rgba(image, output)?;
                Ok(true)
            }
            #[cfg(feature = "linux-h264")]
            Self::H264(decoder) => decoder.decode_into(packet, width, height, output),
        }
    }
}

pub struct VideoDecoder {
    inner: Box<VideoDecoderInner>,
}

struct VideoDecoderInner {
    path: PathBuf,
    demuxer: MatroskaFile<File>,
    decoder: CodecDecoder,
    info: VideoInfo,
    limits: DecodeLimits,
    codec: CodecConfiguration,
    video_track: u64,
    timestamp_scale: u64,
    codec_delay: Duration,
    default_frame_duration: Duration,
    frames: VecDeque<VideoFrame>,
    spare_pixels: Vec<u8>,
    demux_frame: Frame,
    epoch: u64,
    decoded_frames: u64,
    dropped_frames: u64,
    seek_discarded_frames: u64,
    last_returned_timestamp: Option<Duration>,
    end_of_stream: bool,
}

impl VideoDecoder {
    pub const fn capabilities() -> BackendCapabilities {
        CAPABILITIES
    }

    pub fn open(path: &Path, limits: DecodeLimits) -> Result<Self, VideoError> {
        if limits.max_buffered_frames == 0 || limits.max_decoded_bytes == 0 {
            return Err(VideoError::InvalidLimits);
        }
        if let Some(container) = Container::from_path(path)
            && !Self::capabilities().supports_container(container)
        {
            return Err(VideoError::UnsupportedContainer { container });
        }
        let file = File::open(path).map_err(platform_error)?;
        let demuxer = MatroskaFile::open(file).map_err(platform_error)?;
        let container = match demuxer.ebml_header().doc_type() {
            "webm" => Container::WebM,
            "matroska" => Container::Mkv,
            value => {
                return Err(VideoError::Platform(format!(
                    "unsupported Matroska document type: {value}"
                )));
            }
        };

        let track = demuxer
            .tracks()
            .iter()
            .find(|track| track.track_type() == TrackType::Video && track.flag_enabled())
            .ok_or_else(|| VideoError::Platform("container has no enabled video track".into()))?;
        let video = track
            .video()
            .ok_or_else(|| VideoError::Platform("video track has no dimensions".into()))?;
        let width =
            u32::try_from(video.pixel_width().get()).map_err(|_| VideoError::DimensionLimit)?;
        let height =
            u32::try_from(video.pixel_height().get()).map_err(|_| VideoError::DimensionLimit)?;
        let pixels = u64::from(width) * u64::from(height);
        if width > limits.max_width || height > limits.max_height || pixels > limits.max_pixels {
            return Err(VideoError::DimensionLimit);
        }
        let frame_bytes = usize::try_from(pixels)
            .ok()
            .and_then(|value| value.checked_mul(4))
            .ok_or(VideoError::SizeOverflow)?;
        let retained_bytes = frame_bytes
            .checked_mul(limits.max_buffered_frames)
            .ok_or(VideoError::SizeOverflow)?;
        if retained_bytes > limits.max_decoded_bytes {
            return Err(VideoError::ByteLimit);
        }

        let timestamp_scale = demuxer.info().timestamp_scale().get();
        let duration = demuxer
            .info()
            .duration()
            .map(|value| duration_from_scaled_float(value, timestamp_scale))
            .transpose()?
            .unwrap_or(Duration::ZERO);
        let default_frame_duration = track
            .default_duration()
            .map(|value| duration_from_nanos(u128::from(value.get())))
            .unwrap_or(Duration::ZERO);
        let nominal_frame_rate = track.default_duration().and_then(|value| {
            let nanos = value.get();
            let divisor = gcd(1_000_000_000, nanos);
            Some((
                u32::try_from(1_000_000_000 / divisor).ok()?,
                u32::try_from(nanos / divisor).ok()?,
            ))
        });
        let video_track = track.track_number().get();
        let codec_delay = duration_from_nanos(u128::from(track.codec_delay().unwrap_or(0)));
        let video_codec = VideoCodec::from_matroska_id(track.codec_id()).ok_or_else(|| {
            VideoError::Platform(format!(
                "unrecognized Matroska video codec ID: {}",
                track.codec_id()
            ))
        })?;
        if !Self::capabilities().supports(container, video_codec) {
            return Err(VideoError::UnsupportedCombination {
                container,
                codec: video_codec,
            });
        }
        let codec = CodecConfiguration::from_track(video_codec, track.codec_private())?;
        let decoder = CodecDecoder::new(&codec, width, height)?;
        let audio_present = demuxer
            .tracks()
            .iter()
            .any(|track| track.track_type() == TrackType::Audio && track.flag_enabled());

        Ok(Self {
            inner: Box::new(VideoDecoderInner {
                path: path.to_path_buf(),
                demuxer,
                decoder,
                info: VideoInfo {
                    width,
                    height,
                    duration,
                    nominal_frame_rate,
                    audio_present,
                    decoder: codec.decoder_name(),
                    seek_accuracy: SeekAccuracy::KeyframeThenForward,
                },
                limits,
                codec,
                video_track,
                timestamp_scale,
                codec_delay,
                default_frame_duration,
                frames: VecDeque::with_capacity(limits.max_buffered_frames),
                spare_pixels: Vec::new(),
                demux_frame: Frame::default(),
                epoch: 0,
                decoded_frames: 0,
                dropped_frames: 0,
                seek_discarded_frames: 0,
                last_returned_timestamp: None,
                end_of_stream: false,
            }),
        })
    }

    pub fn info(&self) -> &VideoInfo {
        &self.inner.info
    }

    pub fn buffered_frames(&self) -> usize {
        self.inner.frames.len()
    }

    pub fn dropped_frames(&self) -> u64 {
        self.inner.dropped_frames
    }

    pub fn decoded_frames(&self) -> u64 {
        self.inner.decoded_frames
    }

    pub fn seek_discarded_frames(&self) -> u64 {
        self.inner.seek_discarded_frames
    }

    pub fn retained_bytes(&self) -> usize {
        self.inner
            .frames
            .iter()
            .map(|frame| frame.pixels.capacity())
            .sum::<usize>()
            .saturating_add(self.inner.spare_pixels.capacity())
    }

    pub fn seek_frame_timestamp(&self) -> Option<Duration> {
        self.inner.frames.front().map(|frame| frame.timestamp)
    }

    pub fn frame_for_time(&mut self, target: Duration) -> Result<Option<&VideoFrame>, VideoError> {
        self.inner.frame_for_time(target)
    }

    pub fn seek(&mut self, target: Duration) -> Result<(), VideoError> {
        self.inner.seek(target)
    }

    pub fn restart(&mut self) -> Result<(), VideoError> {
        self.inner.seek(Duration::ZERO)
    }
}

impl VideoDecoderInner {
    fn frame_for_time(&mut self, target: Duration) -> Result<Option<&VideoFrame>, VideoError> {
        while !self.end_of_stream
            && self
                .frames
                .back()
                .is_none_or(|frame| frame.timestamp <= target)
        {
            if self.frames.len() == self.limits.max_buffered_frames {
                self.recycle_oldest(true);
            }
            match self.decode_one()? {
                Some(frame) => self.frames.push_back(frame),
                None => self.end_of_stream = true,
            }
        }
        while self.frames.len() > 1 && self.frames[1].timestamp <= target {
            self.recycle_oldest(true);
        }
        self.last_returned_timestamp = self.frames.front().map(|frame| frame.timestamp);
        Ok(self.frames.front())
    }

    fn seek(&mut self, target: Duration) -> Result<(), VideoError> {
        let file = File::open(&self.path).map_err(platform_error)?;
        self.demuxer = MatroskaFile::open(file).map_err(platform_error)?;
        self.decoder = CodecDecoder::new(&self.codec, self.info.width, self.info.height)?;
        self.epoch = self.epoch.wrapping_add(1);
        while let Some(frame) = self.frames.pop_front() {
            self.spare_pixels = frame.pixels;
        }
        self.last_returned_timestamp = None;
        self.end_of_stream = false;
        while let Some(frame) = self.decode_one()? {
            if frame.timestamp >= target {
                self.frames.push_back(frame);
                break;
            }
            self.seek_discarded_frames = self.seek_discarded_frames.saturating_add(1);
            self.spare_pixels = frame.pixels;
        }
        Ok(())
    }

    fn recycle_oldest(&mut self, dropped: bool) {
        if let Some(frame) = self.frames.pop_front() {
            if dropped && self.last_returned_timestamp != Some(frame.timestamp) {
                self.dropped_frames = self.dropped_frames.saturating_add(1);
            }
            self.spare_pixels = frame.pixels;
        }
    }

    fn decode_one(&mut self) -> Result<Option<VideoFrame>, VideoError> {
        while self
            .demuxer
            .next_frame(&mut self.demux_frame)
            .map_err(platform_error)?
        {
            if self.demux_frame.track != self.video_track {
                continue;
            }
            if self.demux_frame.data.len() > self.limits.max_decoded_bytes {
                return Err(VideoError::ByteLimit);
            }
            let timestamp = duration_from_scaled(self.demux_frame.timestamp, self.timestamp_scale)?
                .saturating_sub(self.codec_delay);
            let duration = self
                .demux_frame
                .duration
                .map(|value| duration_from_scaled(value, self.timestamp_scale))
                .transpose()?
                .filter(|value| !value.is_zero())
                .unwrap_or(self.default_frame_duration);
            if !self.decoder.decode_into(
                &self.demux_frame.data,
                self.info.width,
                self.info.height,
                &mut self.spare_pixels,
            )? {
                continue;
            }
            self.decoded_frames = self.decoded_frames.saturating_add(1);
            return Ok(Some(VideoFrame {
                pixels: std::mem::take(&mut self.spare_pixels),
                width: self.info.width,
                height: self.info.height,
                timestamp,
                duration,
                epoch: self.epoch,
            }));
        }
        Ok(None)
    }
}

#[cfg(any(feature = "linux-webm", feature = "linux-mkv"))]
fn convert_to_rgba(image: &vpx_image, output: &mut Vec<u8>) -> Result<(), VideoError> {
    if !matches!(
        image.fmt,
        vpx_img_fmt::VPX_IMG_FMT_I420 | vpx_img_fmt::VPX_IMG_FMT_YV12
    ) || image.bit_depth != 8
        || image.x_chroma_shift != 1
        || image.y_chroma_shift != 1
    {
        return Err(VideoError::InvalidDecodedBuffer);
    }
    let width = usize::try_from(image.d_w).map_err(|_| VideoError::SizeOverflow)?;
    let height = usize::try_from(image.d_h).map_err(|_| VideoError::SizeOverflow)?;
    let rgba_len = width
        .checked_mul(height)
        .and_then(|pixels| pixels.checked_mul(4))
        .ok_or(VideoError::SizeOverflow)?;
    output.resize(rgba_len, 0);

    let y_stride =
        usize::try_from(image.stride[0]).map_err(|_| VideoError::InvalidDecodedBuffer)?;
    let u_stride =
        usize::try_from(image.stride[1]).map_err(|_| VideoError::InvalidDecodedBuffer)?;
    let v_stride =
        usize::try_from(image.stride[2]).map_err(|_| VideoError::InvalidDecodedBuffer)?;
    if image.planes[0].is_null() || image.planes[1].is_null() || image.planes[2].is_null() {
        return Err(VideoError::InvalidDecodedBuffer);
    }
    let chroma_width = width.div_ceil(2);
    let chroma_height = height.div_ceil(2);
    if y_stride < width || u_stride < chroma_width || v_stride < chroma_width {
        return Err(VideoError::InvalidDecodedBuffer);
    }
    let y_len = y_stride
        .checked_mul(height)
        .ok_or(VideoError::SizeOverflow)?;
    let u_len = u_stride
        .checked_mul(chroma_height)
        .ok_or(VideoError::SizeOverflow)?;
    let v_len = v_stride
        .checked_mul(chroma_height)
        .ok_or(VideoError::SizeOverflow)?;
    let y_plane = unsafe { slice::from_raw_parts(image.planes[0], y_len) };
    let u_plane = unsafe { slice::from_raw_parts(image.planes[1], u_len) };
    let v_plane = unsafe { slice::from_raw_parts(image.planes[2], v_len) };

    for row in 0..height {
        for column in 0..width {
            let y = i32::from(y_plane[row * y_stride + column]);
            let u = i32::from(u_plane[(row / 2) * u_stride + column / 2]) - 128;
            let v = i32::from(v_plane[(row / 2) * v_stride + column / 2]) - 128;
            let (red, green, blue) = yuv_to_rgb(y, u, v, image.cs, image.range);
            let offset = (row * width + column) * 4;
            output[offset..offset + 4].copy_from_slice(&[red, green, blue, 255]);
        }
    }
    Ok(())
}

#[cfg(any(feature = "linux-webm", feature = "linux-mkv"))]
fn yuv_to_rgb(
    y: i32,
    u: i32,
    v: i32,
    color_space: vpx_color_space,
    color_range: vpx_color_range,
) -> (u8, u8, u8) {
    let (red, green, blue) = if color_range == vpx_color_range::VPX_CR_FULL_RANGE {
        if color_space == vpx_color_space::VPX_CS_BT_709 {
            (
                256 * y + 403 * v,
                256 * y - 48 * u - 120 * v,
                256 * y + 475 * u,
            )
        } else {
            (
                256 * y + 359 * v,
                256 * y - 88 * u - 183 * v,
                256 * y + 454 * u,
            )
        }
    } else {
        let y = (y - 16).max(0);
        if color_space == vpx_color_space::VPX_CS_BT_709 {
            (
                298 * y + 459 * v,
                298 * y - 55 * u - 136 * v,
                298 * y + 541 * u,
            )
        } else {
            (
                298 * y + 409 * v,
                298 * y - 100 * u - 208 * v,
                298 * y + 516 * u,
            )
        }
    };
    (
        clamp_channel(red),
        clamp_channel(green),
        clamp_channel(blue),
    )
}

#[cfg(any(feature = "linux-webm", feature = "linux-mkv"))]
fn clamp_channel(value: i32) -> u8 {
    ((value + 128) >> 8).clamp(0, 255) as u8
}

fn duration_from_scaled(value: u64, scale: u64) -> Result<Duration, VideoError> {
    let nanos = u128::from(value)
        .checked_mul(u128::from(scale))
        .ok_or(VideoError::SizeOverflow)?;
    Ok(duration_from_nanos(nanos))
}

fn duration_from_scaled_float(value: f64, scale: u64) -> Result<Duration, VideoError> {
    let nanos = value * scale as f64;
    if !nanos.is_finite() || nanos < 0.0 || nanos > u64::MAX as f64 {
        return Err(VideoError::SizeOverflow);
    }
    Ok(duration_from_nanos(nanos.round() as u128))
}

fn duration_from_nanos(value: u128) -> Duration {
    Duration::new(
        u64::try_from(value / 1_000_000_000).unwrap_or(u64::MAX),
        (value % 1_000_000_000) as u32,
    )
}

fn gcd(mut left: u64, mut right: u64) -> u64 {
    while right != 0 {
        (left, right) = (right, left % right);
    }
    left
}

#[cfg(any(feature = "linux-webm", feature = "linux-mkv"))]
fn check_vpx(result: vpx_codec_err_t, operation: &str) -> Result<(), VideoError> {
    if result == vpx_codec_err_t::VPX_CODEC_OK {
        Ok(())
    } else {
        Err(VideoError::Platform(format!(
            "libvpx failed to {operation}: {result:?}"
        )))
    }
}

fn platform_error(error: impl std::fmt::Display) -> VideoError {
    VideoError::Platform(error.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use trend::{CellFrame, FitMode, PixelFormat, RenderRequest, Renderer, Viewport};

    #[cfg(any(feature = "linux-webm", feature = "linux-mkv"))]
    fn fixture(name: &str) -> std::path::PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../..")
            .join("tests/fixtures/media/WebM Experimental")
            .join(name)
    }

    #[test]
    fn declares_linux_matroska_codec_capabilities() {
        let capabilities = VideoDecoder::capabilities();
        assert_eq!(capabilities.backend(), "Linux Matroska codecs");
        #[cfg(any(feature = "linux-webm", feature = "linux-mkv"))]
        assert_eq!(
            capabilities.codecs_for(Container::WebM),
            &[VideoCodec::Vp8, VideoCodec::Vp9]
        );
        #[cfg(all(
            any(feature = "linux-webm", feature = "linux-mkv"),
            not(feature = "linux-h264")
        ))]
        assert_eq!(
            capabilities.codecs_for(Container::Mkv),
            &[VideoCodec::Vp8, VideoCodec::Vp9]
        );
        #[cfg(all(
            any(feature = "linux-webm", feature = "linux-mkv"),
            feature = "linux-h264"
        ))]
        assert_eq!(
            capabilities.codecs_for(Container::Mkv),
            &[VideoCodec::Vp8, VideoCodec::Vp9, VideoCodec::H264]
        );
        #[cfg(all(
            not(any(feature = "linux-webm", feature = "linux-mkv")),
            feature = "linux-h264"
        ))]
        assert_eq!(capabilities.codecs_for(Container::Mkv), &[VideoCodec::H264]);
        for container in [Container::Mp4, Container::Avi, Container::Mov] {
            assert_eq!(capabilities.codecs_for(container), &[], "{container:?}");
        }
    }

    #[cfg(feature = "avi")]
    #[test]
    fn rejects_avi_as_an_unsupported_container_before_demuxing() {
        let error = VideoDecoder::open(
            &Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("../..")
                .join("tests/fixtures/media/AVI Experimental/mjpeg-normal.avi"),
            DecodeLimits::default(),
        )
        .err()
        .unwrap();
        assert!(matches!(
            error,
            VideoError::UnsupportedContainer {
                container: Container::Avi
            }
        ));
    }

    #[cfg(feature = "mov")]
    #[test]
    fn rejects_mov_as_an_unsupported_container_before_demuxing() {
        let error = VideoDecoder::open(
            &Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("../..")
                .join("tests/fixtures/media/MOV Experimental/h264-normal.mov"),
            DecodeLimits::default(),
        )
        .err()
        .unwrap();
        assert!(matches!(
            error,
            VideoError::UnsupportedContainer {
                container: Container::Mov
            }
        ));
    }

    #[cfg(any(feature = "linux-mkv", feature = "linux-h264"))]
    fn mkv_fixture(name: &str) -> std::path::PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../..")
            .join("tests/fixtures/media/MKV Experimental")
            .join(name)
    }

    #[cfg(feature = "linux-h264")]
    #[test]
    fn decodes_h264_mkv_timestamps_and_renders_shared_rgba() {
        for (name, dimensions, frame_rate, frame_duration) in [
            (
                "h264-linux-normal.mkv",
                (320, 192),
                (10, 1),
                Duration::from_millis(100),
            ),
            (
                "h264-linux-large.mkv",
                (1280, 720),
                (8, 1),
                Duration::from_millis(125),
            ),
        ] {
            let mut decoder =
                VideoDecoder::open(&mkv_fixture(name), DecodeLimits::default()).unwrap();
            assert_eq!(
                (decoder.info().width, decoder.info().height),
                dimensions,
                "{name}"
            );
            assert_eq!(
                decoder.info().nominal_frame_rate,
                Some(frame_rate),
                "{name}"
            );
            assert_eq!(decoder.info().duration, Duration::from_secs(1), "{name}");

            let frame = decoder
                .frame_for_time(Duration::from_millis(250))
                .unwrap()
                .unwrap();
            let view = frame.view().unwrap();
            assert_eq!(view.format(), PixelFormat::Rgba8, "{name}");
            assert_eq!(view.timestamp(), frame.timestamp(), "{name}");
            assert_eq!(frame.duration(), frame_duration, "{name}");

            let mut cells = CellFrame::default();
            Renderer::new()
                .render_into(
                    view,
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
            assert!(
                cells.cells().iter().any(|cell| cell.glyph != ' ' as u32),
                "{name}"
            );
            assert!(decoder.buffered_frames() <= DecodeLimits::default().max_buffered_frames);
            assert!(decoder.retained_bytes() <= DecodeLimits::default().max_decoded_bytes);
        }
    }

    #[cfg(feature = "linux-h264")]
    #[test]
    fn h264_mkv_reports_audio_and_preserves_seek_timing() {
        let mut decoder = VideoDecoder::open(
            &mkv_fixture("h264-linux-with-audio.mkv"),
            DecodeLimits::default(),
        )
        .unwrap();
        assert!(decoder.info().audio_present);

        let requested = Duration::from_millis(500);
        decoder.seek(requested).unwrap();
        let frame = decoder.frame_for_time(requested).unwrap().unwrap();
        let timestamp = frame.timestamp();
        let epoch = frame.epoch();
        assert!(timestamp >= requested);
        assert_eq!(timestamp, decoder.seek_frame_timestamp().unwrap());
        assert_eq!(epoch, 1);
        assert!(decoder.seek_discarded_frames() > 0);
    }

    #[cfg(feature = "linux-h264")]
    #[test]
    fn h264_mkv_rejects_corruption_and_limits() {
        assert!(
            VideoDecoder::open(
                &mkv_fixture("h264-linux-corrupt.mkv"),
                DecodeLimits::default()
            )
            .is_err()
        );

        let dimension_error = VideoDecoder::open(
            &mkv_fixture("h264-linux-large.mkv"),
            DecodeLimits {
                max_width: 640,
                ..DecodeLimits::default()
            },
        )
        .err()
        .unwrap();
        assert!(matches!(dimension_error, VideoError::DimensionLimit));

        let byte_error = VideoDecoder::open(
            &mkv_fixture("h264-linux-normal.mkv"),
            DecodeLimits {
                max_decoded_bytes: 1,
                ..DecodeLimits::default()
            },
        )
        .err()
        .unwrap();
        assert!(matches!(byte_error, VideoError::ByteLimit));
    }

    #[cfg(any(feature = "linux-webm", feature = "linux-mkv"))]
    #[test]
    fn decodes_timestamped_vp8_and_vp9_into_shared_rgba_frames() {
        for (name, dimensions) in [
            ("vp8-tiny.webm", (16, 16)),
            ("vp8-normal.webm", (320, 180)),
            ("vp9-large.webm", (1280, 720)),
        ] {
            let mut decoder = VideoDecoder::open(&fixture(name), DecodeLimits::default()).unwrap();
            assert_eq!(
                (decoder.info().width, decoder.info().height),
                dimensions,
                "{name}"
            );
            assert!(decoder.info().duration > Duration::ZERO, "{name}");

            let frame = decoder
                .frame_for_time(Duration::from_millis(100))
                .unwrap()
                .unwrap();
            let view = frame.view().unwrap();
            assert_eq!(view.format(), PixelFormat::Rgba8, "{name}");
            assert_eq!(view.timestamp(), frame.timestamp(), "{name}");
            assert_eq!(
                view.stride(),
                usize::try_from(dimensions.0).unwrap() * 4,
                "{name}"
            );
            assert!(!frame.duration().is_zero(), "{name}");

            let mut cells = CellFrame::default();
            Renderer::new()
                .render_into(
                    view,
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
            assert!(
                cells.cells().iter().any(|cell| cell.glyph != ' ' as u32),
                "{name}"
            );
        }
    }

    #[cfg(any(feature = "linux-webm", feature = "linux-mkv"))]
    #[test]
    fn reports_audio_without_decoding_it_and_preserves_seek_timing() {
        let mut decoder =
            VideoDecoder::open(&fixture("vp9-with-audio.webm"), DecodeLimits::default()).unwrap();
        assert!(decoder.info().audio_present);

        let requested = Duration::from_millis(200);
        decoder.seek(requested).unwrap();
        let frame = decoder.frame_for_time(requested).unwrap().unwrap();
        let timestamp = frame.timestamp();
        let epoch = frame.epoch();
        let mut cells = CellFrame::default();
        Renderer::new()
            .render_into(
                frame.view().unwrap(),
                RenderRequest {
                    viewport: Viewport {
                        width: 8,
                        height: 4,
                    },
                    fit: FitMode::Contain,
                    generation: epoch,
                },
                &mut cells,
            )
            .unwrap();
        assert!(timestamp >= requested);
        assert_eq!(timestamp, decoder.seek_frame_timestamp().unwrap());
        assert_eq!(epoch, 1);
        assert!(decoder.seek_discarded_frames() > 0);
        assert!(cells.cells().iter().any(|cell| cell.glyph != ' ' as u32));
    }

    #[cfg(any(feature = "linux-webm", feature = "linux-mkv"))]
    #[test]
    fn rejects_corrupt_input_and_frames_over_limits() {
        assert!(VideoDecoder::open(&fixture("corrupt.webm"), DecodeLimits::default()).is_err());

        let error = VideoDecoder::open(
            &fixture("vp9-large.webm"),
            DecodeLimits {
                max_width: 640,
                ..DecodeLimits::default()
            },
        )
        .err()
        .unwrap();
        assert!(matches!(error, VideoError::DimensionLimit));
    }

    #[cfg(feature = "linux-mkv")]
    #[test]
    fn decodes_timestamped_vp9_mkv_and_renders_shared_rgba() {
        let mut decoder =
            VideoDecoder::open(&mkv_fixture("vp9-large.mkv"), DecodeLimits::default()).unwrap();
        assert_eq!((decoder.info().width, decoder.info().height), (1280, 720));
        assert!(decoder.info().duration > Duration::ZERO);

        let frame = decoder
            .frame_for_time(Duration::from_millis(100))
            .unwrap()
            .unwrap();
        let view = frame.view().unwrap();
        assert_eq!(view.format(), PixelFormat::Rgba8);
        assert_eq!(view.stride(), 1280 * 4);
        assert_eq!(view.timestamp(), frame.timestamp());
        assert!(!frame.duration().is_zero());

        let mut cells = CellFrame::default();
        Renderer::new()
            .render_into(
                view,
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

    #[cfg(feature = "linux-mkv")]
    #[test]
    fn mkv_reports_audio_and_preserves_seek_timing() {
        let mut decoder =
            VideoDecoder::open(&mkv_fixture("vp9-with-audio.mkv"), DecodeLimits::default())
                .unwrap();
        assert!(decoder.info().audio_present);

        let requested = Duration::from_millis(100);
        decoder.seek(requested).unwrap();
        let frame = decoder.frame_for_time(requested).unwrap().unwrap();
        let timestamp = frame.timestamp();
        let epoch = frame.epoch();
        assert!(timestamp >= requested);
        assert_eq!(timestamp, decoder.seek_frame_timestamp().unwrap());
        assert_eq!(epoch, 1);
        assert!(decoder.seek_discarded_frames() > 0);
    }

    #[cfg(feature = "linux-mkv")]
    #[test]
    fn mkv_rejects_corruption_and_bounds() {
        assert!(VideoDecoder::open(&mkv_fixture("corrupt.mkv"), DecodeLimits::default()).is_err());

        let bounds_error = VideoDecoder::open(
            &mkv_fixture("vp9-large.mkv"),
            DecodeLimits {
                max_width: 640,
                ..DecodeLimits::default()
            },
        )
        .err()
        .unwrap();
        assert!(matches!(bounds_error, VideoError::DimensionLimit));

        let byte_error = VideoDecoder::open(
            &mkv_fixture("vp9-large.mkv"),
            DecodeLimits {
                max_decoded_bytes: 1,
                ..DecodeLimits::default()
            },
        )
        .err()
        .unwrap();
        assert!(matches!(byte_error, VideoError::ByteLimit));
    }

    #[cfg(all(feature = "linux-mkv", not(feature = "linux-h264")))]
    #[test]
    fn mkv_rejects_h264_without_its_decoder_feature() {
        let codec_error =
            VideoDecoder::open(&mkv_fixture("h264-normal.mkv"), DecodeLimits::default())
                .err()
                .unwrap();
        assert!(matches!(
            codec_error,
            VideoError::UnsupportedCombination {
                container: Container::Mkv,
                codec: VideoCodec::H264
            }
        ));
    }
}
