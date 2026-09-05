use std::collections::VecDeque;
use std::path::Path;
use std::ptr;
use std::slice;
use std::time::Duration;

use windows::Win32::Media::MediaFoundation::{
    IMFMediaBuffer, IMFSourceReader, MF_MT_DEFAULT_STRIDE, MF_MT_FRAME_RATE, MF_MT_FRAME_SIZE,
    MF_MT_MAJOR_TYPE, MF_MT_SUBTYPE, MF_PD_DURATION, MF_SOURCE_READER_ALL_STREAMS,
    MF_SOURCE_READER_ENABLE_VIDEO_PROCESSING, MF_SOURCE_READER_FIRST_AUDIO_STREAM,
    MF_SOURCE_READER_FIRST_VIDEO_STREAM, MF_SOURCE_READER_MEDIASOURCE,
    MF_SOURCE_READERF_CURRENTMEDIATYPECHANGED, MF_SOURCE_READERF_ENDOFSTREAM,
    MF_SOURCE_READERF_NATIVEMEDIATYPECHANGED, MF_VERSION, MFCreateAttributes, MFCreateMediaType,
    MFCreateSourceReaderFromURL, MFMediaType_Video, MFSTARTUP_FULL, MFShutdown, MFStartup,
    MFVideoFormat_RGB32,
};
use windows::Win32::System::Com::StructuredStorage::PROPVARIANT;
use windows::Win32::System::Com::{COINIT_MULTITHREADED, CoInitializeEx, CoUninitialize};
use windows::core::{GUID, HSTRING};

use crate::{
    BackendCapabilities, Container, ContainerSupport, DecodeLimits, SeekAccuracy, VideoCodec,
    VideoError, VideoFrame, VideoInfo,
};

const CAPABILITIES: BackendCapabilities = BackendCapabilities::new(
    "Windows Media Foundation",
    &[
        ContainerSupport::new(
            Container::Mp4,
            &[VideoCodec::H264, VideoCodec::Hevc, VideoCodec::Vp9],
        ),
        ContainerSupport::new(Container::Mkv, &[VideoCodec::H264, VideoCodec::Vp9]),
        ContainerSupport::new(Container::WebM, &[VideoCodec::Vp8, VideoCodec::Vp9]),
        ContainerSupport::new(Container::Avi, &[VideoCodec::Mjpeg]),
        ContainerSupport::new(Container::Mov, &[VideoCodec::H264, VideoCodec::Hevc]),
    ],
);

struct MediaFoundationSession {
    com_initialized: bool,
    media_foundation_started: bool,
}

impl MediaFoundationSession {
    fn start() -> Result<Self, VideoError> {
        unsafe {
            CoInitializeEx(None, COINIT_MULTITHREADED)
                .ok()
                .map_err(platform_error)?;
            if let Err(error) = MFStartup(MF_VERSION, MFSTARTUP_FULL) {
                CoUninitialize();
                return Err(platform_error(error));
            }
        }
        Ok(Self {
            com_initialized: true,
            media_foundation_started: true,
        })
    }
}

impl Drop for MediaFoundationSession {
    fn drop(&mut self) {
        unsafe {
            if self.media_foundation_started {
                let _ = MFShutdown();
            }
            if self.com_initialized {
                CoUninitialize();
            }
        }
    }
}

pub struct VideoDecoder {
    reader: IMFSourceReader,
    info: VideoInfo,
    limits: DecodeLimits,
    frames: VecDeque<VideoFrame>,
    spare_pixels: Vec<u8>,
    source_stride: i32,
    source_height: u32,
    frame_bytes: usize,
    epoch: u64,
    decoded_frames: u64,
    dropped_frames: u64,
    seek_discarded_frames: u64,
    last_returned_timestamp: Option<Duration>,
    end_of_stream: bool,
    _session: MediaFoundationSession,
}

impl VideoDecoder {
    pub const fn capabilities() -> BackendCapabilities {
        CAPABILITIES
    }

    pub fn open(path: &Path, limits: DecodeLimits) -> Result<Self, VideoError> {
        if limits.max_buffered_frames == 0 || limits.max_decoded_bytes == 0 {
            return Err(VideoError::InvalidLimits);
        }
        let session = MediaFoundationSession::start()?;
        let path = HSTRING::from(path.to_string_lossy().as_ref());
        let mut attributes = None;
        unsafe {
            MFCreateAttributes(&mut attributes, 1).map_err(platform_error)?;
        }
        let attributes = attributes.ok_or_else(|| {
            VideoError::Platform("Media Foundation returned no attribute store".into())
        })?;
        unsafe {
            attributes
                .SetUINT32(&MF_SOURCE_READER_ENABLE_VIDEO_PROCESSING, 1)
                .map_err(platform_error)?;
        }
        let reader =
            unsafe { MFCreateSourceReaderFromURL(&path, &attributes) }.map_err(platform_error)?;
        let video_stream = MF_SOURCE_READER_FIRST_VIDEO_STREAM.0 as u32;
        unsafe {
            reader
                .SetStreamSelection(MF_SOURCE_READER_ALL_STREAMS.0 as u32, false)
                .map_err(platform_error)?;
            reader
                .SetStreamSelection(video_stream, true)
                .map_err(platform_error)?;
        }

        let audio_present = unsafe {
            reader
                .GetNativeMediaType(MF_SOURCE_READER_FIRST_AUDIO_STREAM.0 as u32, 0)
                .is_ok()
        };
        let native_type =
            unsafe { reader.GetNativeMediaType(video_stream, 0) }.map_err(platform_error)?;
        let native_frame_size =
            unsafe { native_type.GetUINT64(&MF_MT_FRAME_SIZE) }.map_err(platform_error)?;
        let width = (native_frame_size >> 32) as u32;
        let height = native_frame_size as u32;
        let output_type = unsafe { MFCreateMediaType() }.map_err(platform_error)?;
        unsafe {
            output_type
                .SetGUID(&MF_MT_MAJOR_TYPE, &MFMediaType_Video)
                .map_err(platform_error)?;
            output_type
                .SetGUID(&MF_MT_SUBTYPE, &MFVideoFormat_RGB32)
                .map_err(platform_error)?;
            reader
                .SetCurrentMediaType(video_stream, None, &output_type)
                .map_err(platform_error)?;
        }
        let decoded_type =
            unsafe { reader.GetCurrentMediaType(video_stream) }.map_err(platform_error)?;
        let decoded_frame_size =
            unsafe { decoded_type.GetUINT64(&MF_MT_FRAME_SIZE) }.map_err(platform_error)?;
        let source_height = decoded_frame_size as u32;
        let pixels = u64::from(width) * u64::from(height);
        if width == 0
            || height == 0
            || width > limits.max_width
            || height > limits.max_height
            || pixels > limits.max_pixels
        {
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

        let source_stride = unsafe { decoded_type.GetUINT32(&MF_MT_DEFAULT_STRIDE) }
            .map(|stride| stride as i32)
            .unwrap_or_else(|_| i32::try_from(width.saturating_mul(4)).unwrap_or(i32::MAX));
        let nominal_frame_rate = unsafe { decoded_type.GetUINT64(&MF_MT_FRAME_RATE) }
            .ok()
            .and_then(|packed| {
                let numerator = (packed >> 32) as u32;
                let denominator = packed as u32;
                (numerator != 0 && denominator != 0).then_some((numerator, denominator))
            });
        let duration = unsafe {
            reader.GetPresentationAttribute(MF_SOURCE_READER_MEDIASOURCE.0 as u32, &MF_PD_DURATION)
        }
        .map_err(platform_error)
        .and_then(|value| i64::try_from(&value).map_err(platform_error))
        .map(duration_from_hns)?;

        Ok(Self {
            reader,
            info: VideoInfo {
                width,
                height,
                duration,
                nominal_frame_rate,
                audio_present,
                decoder: "Windows Media Foundation",
                seek_accuracy: SeekAccuracy::KeyframeThenForward,
            },
            limits,
            frames: VecDeque::with_capacity(limits.max_buffered_frames),
            spare_pixels: Vec::new(),
            source_stride,
            source_height,
            frame_bytes,
            epoch: 0,
            decoded_frames: 0,
            dropped_frames: 0,
            seek_discarded_frames: 0,
            last_returned_timestamp: None,
            end_of_stream: false,
            _session: session,
        })
    }

    pub const fn info(&self) -> &VideoInfo {
        &self.info
    }

    pub fn buffered_frames(&self) -> usize {
        self.frames.len()
    }

    pub const fn dropped_frames(&self) -> u64 {
        self.dropped_frames
    }

    pub const fn decoded_frames(&self) -> u64 {
        self.decoded_frames
    }

    pub const fn seek_discarded_frames(&self) -> u64 {
        self.seek_discarded_frames
    }

    pub fn retained_bytes(&self) -> usize {
        self.frames
            .iter()
            .map(|frame| frame.pixels.capacity())
            .sum::<usize>()
            .saturating_add(self.spare_pixels.capacity())
    }

    pub fn seek_frame_timestamp(&self) -> Option<Duration> {
        self.frames.front().map(|frame| frame.timestamp)
    }

    pub fn frame_for_time(&mut self, target: Duration) -> Result<Option<&VideoFrame>, VideoError> {
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

    pub fn seek(&mut self, target: Duration) -> Result<(), VideoError> {
        let target_hns = duration_to_hns(target)?;
        let position = PROPVARIANT::from(target_hns);
        unsafe {
            self.reader
                .SetCurrentPosition(&GUID::zeroed(), &position)
                .map_err(platform_error)?;
        }
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

    pub fn restart(&mut self) -> Result<(), VideoError> {
        self.seek(Duration::ZERO)
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
        let video_stream = MF_SOURCE_READER_FIRST_VIDEO_STREAM.0 as u32;
        loop {
            let mut flags = 0_u32;
            let mut timestamp_hns = 0_i64;
            let mut sample = None;
            unsafe {
                self.reader
                    .ReadSample(
                        video_stream,
                        0,
                        None,
                        Some(&mut flags),
                        Some(&mut timestamp_hns),
                        Some(&mut sample),
                    )
                    .map_err(platform_error)?;
            }
            let format_change = (MF_SOURCE_READERF_CURRENTMEDIATYPECHANGED.0
                | MF_SOURCE_READERF_NATIVEMEDIATYPECHANGED.0)
                as u32;
            if flags & format_change != 0 {
                self.refresh_output_layout()?;
            }
            if flags & MF_SOURCE_READERF_ENDOFSTREAM.0 as u32 != 0 {
                return Ok(None);
            }
            let Some(sample) = sample else {
                continue;
            };
            let sample_duration = unsafe { sample.GetSampleDuration() }
                .ok()
                .map(duration_from_hns)
                .unwrap_or_else(|| nominal_duration(self.info.nominal_frame_rate));
            let buffer = unsafe { sample.ConvertToContiguousBuffer() }.map_err(platform_error)?;
            self.copy_rgb32(&buffer)?;
            self.decoded_frames = self.decoded_frames.saturating_add(1);
            let pixels = std::mem::take(&mut self.spare_pixels);
            return Ok(Some(VideoFrame {
                pixels,
                width: self.info.width,
                height: self.info.height,
                timestamp: duration_from_hns(timestamp_hns),
                duration: sample_duration,
                epoch: self.epoch,
            }));
        }
    }

    fn copy_rgb32(&mut self, buffer: &IMFMediaBuffer) -> Result<(), VideoError> {
        self.spare_pixels.resize(self.frame_bytes, 0);
        let mut source = ptr::null_mut();
        let mut source_len = 0_u32;
        unsafe {
            buffer
                .Lock(&mut source, None, Some(&mut source_len))
                .map_err(platform_error)?;
        }
        let result = self.copy_locked_rgb32(source, source_len);
        let unlock_result = unsafe { buffer.Unlock() }.map_err(platform_error);
        result.and(unlock_result)
    }

    fn refresh_output_layout(&mut self) -> Result<(), VideoError> {
        let media_type = unsafe {
            self.reader
                .GetCurrentMediaType(MF_SOURCE_READER_FIRST_VIDEO_STREAM.0 as u32)
        }
        .map_err(platform_error)?;
        let frame_size =
            unsafe { media_type.GetUINT64(&MF_MT_FRAME_SIZE) }.map_err(platform_error)?;
        let width = (frame_size >> 32) as u32;
        let height = frame_size as u32;
        let pixels = u64::from(width) * u64::from(height);
        if width < self.info.width
            || height < self.info.height
            || width > self.limits.max_width
            || height > self.limits.max_height
            || pixels > self.limits.max_pixels
        {
            return Err(VideoError::DimensionLimit);
        }
        let frame_bytes = usize::try_from(pixels)
            .ok()
            .and_then(|value| value.checked_mul(4))
            .ok_or(VideoError::SizeOverflow)?;
        let retained_bytes = frame_bytes
            .checked_mul(self.limits.max_buffered_frames)
            .ok_or(VideoError::SizeOverflow)?;
        if retained_bytes > self.limits.max_decoded_bytes {
            return Err(VideoError::ByteLimit);
        }
        self.source_height = height;
        self.source_stride = unsafe { media_type.GetUINT32(&MF_MT_DEFAULT_STRIDE) }
            .map(|stride| stride as i32)
            .unwrap_or(self.source_stride);
        Ok(())
    }

    fn copy_locked_rgb32(&mut self, source: *const u8, source_len: u32) -> Result<(), VideoError> {
        let source_stride = self.source_stride.unsigned_abs() as usize;
        let height = usize::try_from(self.info.height).map_err(|_| VideoError::SizeOverflow)?;
        let source_height =
            usize::try_from(self.source_height).map_err(|_| VideoError::SizeOverflow)?;
        let width = usize::try_from(self.info.width).map_err(|_| VideoError::SizeOverflow)?;
        let required = source_stride
            .checked_mul(source_height)
            .ok_or(VideoError::SizeOverflow)?;
        if source.is_null() || source_stride < width * 4 || required > source_len as usize {
            return Err(VideoError::InvalidDecodedBuffer);
        }
        let source = unsafe { slice::from_raw_parts(source, required) };
        for output_row in 0..height {
            let source_row = if self.source_stride < 0 {
                source_height - output_row - 1
            } else {
                output_row
            };
            let source_row = &source[source_row * source_stride..][..width * 4];
            let output_row = &mut self.spare_pixels[output_row * width * 4..][..width * 4];
            let (source_pixels, _) = source_row.as_chunks::<4>();
            let (output_pixels, _) = output_row.as_chunks_mut::<4>();
            for (bgra, rgba) in source_pixels.iter().zip(output_pixels) {
                rgba.copy_from_slice(&[bgra[2], bgra[1], bgra[0], 255]);
            }
        }
        Ok(())
    }
}

fn nominal_duration(frame_rate: Option<(u32, u32)>) -> Duration {
    frame_rate.map_or(Duration::ZERO, |(numerator, denominator)| {
        Duration::from_secs_f64(f64::from(denominator) / f64::from(numerator))
    })
}

fn duration_from_hns(value: i64) -> Duration {
    Duration::from_nanos(value.max(0) as u64 * 100)
}

fn duration_to_hns(value: Duration) -> Result<i64, VideoError> {
    let units = value.as_nanos() / 100;
    i64::try_from(units).map_err(|_| VideoError::SizeOverflow)
}

fn platform_error(error: windows::core::Error) -> VideoError {
    VideoError::Platform(error.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeSet;
    use trend::{CellFrame, FitMode, RenderRequest, Renderer, Viewport};

    fn fixture() -> std::path::PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../..")
            .join("Test runs Envs/Materials/ColorTest.mp4")
    }

    #[test]
    fn declares_media_foundation_container_codec_capabilities() {
        let capabilities = VideoDecoder::capabilities();
        assert_eq!(capabilities.backend(), "Windows Media Foundation");
        assert_eq!(
            capabilities.codecs_for(Container::Mp4),
            &[VideoCodec::H264, VideoCodec::Hevc, VideoCodec::Vp9]
        );
        assert_eq!(
            capabilities.codecs_for(Container::Mkv),
            &[VideoCodec::H264, VideoCodec::Vp9]
        );
        assert_eq!(
            capabilities.codecs_for(Container::WebM),
            &[VideoCodec::Vp8, VideoCodec::Vp9]
        );
        assert_eq!(
            capabilities.codecs_for(Container::Avi),
            &[VideoCodec::Mjpeg]
        );
        assert_eq!(
            capabilities.codecs_for(Container::Mov),
            &[VideoCodec::H264, VideoCodec::Hevc]
        );
    }

    #[cfg(feature = "webm")]
    fn webm_fixture(name: &str) -> std::path::PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../..")
            .join("Test runs Envs/Materials/WebM Experimental")
            .join(name)
    }

    #[cfg(feature = "mkv")]
    fn mkv_fixture(name: &str) -> std::path::PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../..")
            .join("Test runs Envs/Materials/MKV Experimental")
            .join(name)
    }

    #[cfg(feature = "avi")]
    fn avi_fixture(name: &str) -> std::path::PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../..")
            .join("Test runs Envs/Materials/AVI Experimental")
            .join(name)
    }

    #[cfg(feature = "mov")]
    fn mov_fixture(name: &str) -> std::path::PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../..")
            .join("Test runs Envs/Materials/MOV Experimental")
            .join(name)
    }

    #[test]
    fn decodes_mp4_to_borrowed_bounded_rgba_frames() {
        let mut decoder = VideoDecoder::open(&fixture(), DecodeLimits::default()).unwrap();
        assert_eq!((decoder.info().width, decoder.info().height), (500, 500));
        assert_eq!(decoder.info().nominal_frame_rate, Some((20, 1)));
        assert!(!decoder.info().audio_present);

        let frame = decoder
            .frame_for_time(Duration::from_millis(200))
            .unwrap()
            .unwrap();
        assert_eq!(frame.view().unwrap().pixels().len(), 500 * 500 * 4);
        assert!(frame.timestamp() <= Duration::from_millis(250));
        assert!(decoder.buffered_frames() <= 2);
        assert!(decoder.retained_bytes() <= DecodeLimits::default().max_decoded_bytes);
    }

    #[cfg(feature = "webm")]
    #[test]
    fn experimental_webm_decodes_timestamped_vp8_vp9_and_renders() {
        for (name, dimensions) in [
            ("vp8-tiny.webm", (16, 16)),
            ("vp8-normal.webm", (320, 180)),
            ("vp9-large.webm", (1280, 720)),
        ] {
            let mut decoder =
                VideoDecoder::open(&webm_fixture(name), DecodeLimits::default()).unwrap();
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
            assert!(!frame.duration().is_zero(), "{name}");
            assert_eq!(
                frame.view().unwrap().timestamp(),
                frame.timestamp(),
                "{name}"
            );

            let mut cells = CellFrame::default();
            Renderer::new()
                .render_into(
                    frame.view().unwrap(),
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

    #[cfg(feature = "webm")]
    #[test]
    fn experimental_webm_reports_audio_without_decoding_it_and_seeks() {
        let mut decoder = VideoDecoder::open(
            &webm_fixture("vp9-with-audio.webm"),
            DecodeLimits::default(),
        )
        .unwrap();
        assert!(decoder.info().audio_present);
        decoder.seek(Duration::from_millis(200)).unwrap();
        let frame = decoder
            .frame_for_time(Duration::from_millis(200))
            .unwrap()
            .unwrap();
        assert!(frame.timestamp() >= Duration::from_millis(200));
        assert_eq!(frame.epoch(), 1);
    }

    #[cfg(feature = "webm")]
    #[test]
    fn experimental_webm_rejects_corruption_and_large_frames_over_limit() {
        assert!(
            VideoDecoder::open(&webm_fixture("corrupt.webm"), DecodeLimits::default()).is_err()
        );
        let error = VideoDecoder::open(
            &webm_fixture("vp9-large.webm"),
            DecodeLimits {
                max_width: 640,
                ..DecodeLimits::default()
            },
        )
        .err()
        .unwrap();
        assert!(matches!(error, VideoError::DimensionLimit));

        let error = VideoDecoder::open(
            &mkv_fixture("vp9-large.mkv"),
            DecodeLimits {
                max_decoded_bytes: 1,
                ..DecodeLimits::default()
            },
        )
        .err()
        .unwrap();
        assert!(matches!(error, VideoError::ByteLimit));
    }

    #[cfg(feature = "mkv")]
    #[test]
    fn experimental_mkv_decodes_h264_vp9_timestamps_and_renders() {
        for (name, dimensions) in [
            ("h264-normal.mkv", (500, 500)),
            ("vp9-large.mkv", (1280, 720)),
        ] {
            let mut decoder =
                VideoDecoder::open(&mkv_fixture(name), DecodeLimits::default()).unwrap();
            assert_eq!((decoder.info().width, decoder.info().height), dimensions);
            assert!(decoder.info().duration > Duration::ZERO);
            let frame = decoder
                .frame_for_time(Duration::from_millis(100))
                .unwrap()
                .unwrap();
            assert_eq!(frame.view().unwrap().timestamp(), frame.timestamp());
            assert!(!frame.duration().is_zero());

            let mut cells = CellFrame::default();
            Renderer::new()
                .render_into(
                    frame.view().unwrap(),
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
    }

    #[cfg(feature = "mkv")]
    #[test]
    fn experimental_mkv_reports_audio_and_seeks() {
        let mut decoder =
            VideoDecoder::open(&mkv_fixture("vp9-with-audio.mkv"), DecodeLimits::default())
                .unwrap();
        assert!(decoder.info().audio_present);
        decoder.seek(Duration::from_millis(100)).unwrap();
        let frame = decoder
            .frame_for_time(Duration::from_millis(100))
            .unwrap()
            .unwrap();
        assert!(frame.timestamp() >= Duration::from_millis(100));
        assert_eq!(frame.epoch(), 1);
    }

    #[cfg(feature = "mkv")]
    #[test]
    fn experimental_mkv_rejects_corruption_and_large_frames_over_limit() {
        assert!(VideoDecoder::open(&mkv_fixture("corrupt.mkv"), DecodeLimits::default()).is_err());
        let error = VideoDecoder::open(
            &mkv_fixture("vp9-large.mkv"),
            DecodeLimits {
                max_width: 640,
                ..DecodeLimits::default()
            },
        )
        .err()
        .unwrap();
        assert!(matches!(error, VideoError::DimensionLimit));
    }

    #[test]
    fn steady_state_reuses_bounded_buffers() {
        let mut decoder = VideoDecoder::open(&fixture(), DecodeLimits::default()).unwrap();
        let mut addresses = BTreeSet::new();
        for frame_number in 0..13 {
            let frame = decoder
                .frame_for_time(Duration::from_millis(frame_number * 50))
                .unwrap()
                .unwrap();
            addresses.insert(frame.view().unwrap().pixels().as_ptr() as usize);
        }

        assert!(addresses.len() <= DecodeLimits::default().max_buffered_frames);
        assert!(decoder.retained_bytes() <= DecodeLimits::default().max_decoded_bytes);
    }

    #[test]
    fn resize_rerenders_the_borrowed_frame_without_reopening_video() {
        let mut decoder = VideoDecoder::open(&fixture(), DecodeLimits::default()).unwrap();
        let frame = decoder
            .frame_for_time(Duration::from_millis(200))
            .unwrap()
            .unwrap();
        let source_address = frame.view().unwrap().pixels().as_ptr();
        let renderer = Renderer::new();
        let mut cells = CellFrame::default();

        for (generation, (width, height)) in [(1, (20, 10)), (2, (80, 24)), (3, (40, 15))] {
            renderer
                .render_into(
                    frame.view().unwrap(),
                    RenderRequest {
                        viewport: Viewport { width, height },
                        fit: FitMode::Contain,
                        generation,
                    },
                    &mut cells,
                )
                .unwrap();
            assert_eq!(frame.view().unwrap().pixels().as_ptr(), source_address);
        }
    }

    #[test]
    fn seek_advances_epoch_and_decodes_forward_from_keyframe() {
        let mut decoder = VideoDecoder::open(&fixture(), DecodeLimits::default()).unwrap();
        decoder.frame_for_time(Duration::ZERO).unwrap();
        decoder.seek(Duration::from_millis(400)).unwrap();
        let frame = decoder
            .frame_for_time(Duration::from_millis(400))
            .unwrap()
            .unwrap();

        assert_eq!(frame.epoch(), 1);
        assert!(frame.timestamp() >= Duration::from_millis(400));
        assert!(decoder.buffered_frames() <= 2);
    }

    #[test]
    fn rejects_frame_storage_over_budget() {
        let error = VideoDecoder::open(
            &fixture(),
            DecodeLimits {
                max_decoded_bytes: 1,
                ..DecodeLimits::default()
            },
        )
        .err()
        .unwrap();
        assert!(matches!(error, VideoError::ByteLimit));
    }

    #[cfg(feature = "avi")]
    #[test]
    fn experimental_mjpeg_avi_decodes_timestamps_and_renders() {
        for (name, dimensions, frame_rate) in [
            ("mjpeg-normal.avi", (320, 180), (10, 1)),
            ("mjpeg-large.avi", (1280, 720), (8, 1)),
        ] {
            let mut decoder =
                VideoDecoder::open(&avi_fixture(name), DecodeLimits::default()).unwrap();
            assert_eq!((decoder.info().width, decoder.info().height), dimensions);
            assert_eq!(decoder.info().nominal_frame_rate, Some(frame_rate));
            assert_eq!(decoder.info().duration, Duration::from_secs(1));

            let requested = Duration::from_millis(250);
            let frame = decoder.frame_for_time(requested).unwrap().unwrap();
            assert!(frame.timestamp() <= requested);
            assert_eq!(frame.view().unwrap().timestamp(), frame.timestamp());
            assert!(!frame.duration().is_zero());

            let mut cells = CellFrame::default();
            Renderer::new()
                .render_into(
                    frame.view().unwrap(),
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
    }

    #[cfg(feature = "avi")]
    #[test]
    fn experimental_mjpeg_avi_reports_audio_and_seeks() {
        let mut decoder = VideoDecoder::open(
            &avi_fixture("mjpeg-with-audio.avi"),
            DecodeLimits::default(),
        )
        .unwrap();
        assert!(decoder.info().audio_present);

        let requested = Duration::from_millis(500);
        decoder.seek(requested).unwrap();
        let frame = decoder.frame_for_time(requested).unwrap().unwrap();
        assert!(frame.timestamp() >= requested);
        assert_eq!(frame.epoch(), 1);
        assert_eq!(frame.timestamp(), decoder.seek_frame_timestamp().unwrap());
    }

    #[cfg(feature = "avi")]
    #[test]
    fn experimental_mjpeg_avi_rejects_corruption_and_limits() {
        assert!(VideoDecoder::open(&avi_fixture("corrupt.avi"), DecodeLimits::default()).is_err());

        let dimension_error = VideoDecoder::open(
            &avi_fixture("mjpeg-large.avi"),
            DecodeLimits {
                max_width: 640,
                ..DecodeLimits::default()
            },
        )
        .err()
        .unwrap();
        assert!(matches!(dimension_error, VideoError::DimensionLimit));

        let byte_error = VideoDecoder::open(
            &avi_fixture("mjpeg-normal.avi"),
            DecodeLimits {
                max_decoded_bytes: 1,
                ..DecodeLimits::default()
            },
        )
        .err()
        .unwrap();
        assert!(matches!(byte_error, VideoError::ByteLimit));
    }

    #[cfg(feature = "mov")]
    #[test]
    fn experimental_mov_decodes_verified_codecs_timestamps_and_renders() {
        for (name, dimensions, frame_rate) in [
            ("h264-normal.mov", (320, 192), (10, 1)),
            ("hevc-normal.mov", (320, 192), (10, 1)),
            ("hevc-large.mov", (1280, 720), (8, 1)),
        ] {
            let mut decoder =
                VideoDecoder::open(&mov_fixture(name), DecodeLimits::default()).unwrap();
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

            let requested = Duration::from_millis(250);
            let frame = decoder.frame_for_time(requested).unwrap().unwrap();
            assert!(frame.timestamp() <= requested, "{name}");
            assert_eq!(
                frame.view().unwrap().timestamp(),
                frame.timestamp(),
                "{name}"
            );
            assert!(!frame.duration().is_zero(), "{name}");

            let mut cells = CellFrame::default();
            Renderer::new()
                .render_into(
                    frame.view().unwrap(),
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

    #[cfg(feature = "mov")]
    #[test]
    fn experimental_mov_reports_audio_and_seeks() {
        let mut decoder =
            VideoDecoder::open(&mov_fixture("h264-with-audio.mov"), DecodeLimits::default())
                .unwrap();
        assert!(decoder.info().audio_present);

        let requested = Duration::from_millis(500);
        decoder.seek(requested).unwrap();
        let frame = decoder.frame_for_time(requested).unwrap().unwrap();
        assert!(frame.timestamp() >= requested);
        assert_eq!(frame.epoch(), 1);
        assert_eq!(frame.timestamp(), decoder.seek_frame_timestamp().unwrap());
    }

    #[cfg(feature = "mov")]
    #[test]
    fn experimental_mov_rejects_corruption_and_limits() {
        assert!(VideoDecoder::open(&mov_fixture("corrupt.mov"), DecodeLimits::default()).is_err());

        let dimension_error = VideoDecoder::open(
            &mov_fixture("hevc-large.mov"),
            DecodeLimits {
                max_width: 640,
                ..DecodeLimits::default()
            },
        )
        .err()
        .unwrap();
        assert!(matches!(dimension_error, VideoError::DimensionLimit));

        let byte_error = VideoDecoder::open(
            &mov_fixture("h264-normal.mov"),
            DecodeLimits {
                max_decoded_bytes: 1,
                ..DecodeLimits::default()
            },
        )
        .err()
        .unwrap();
        assert!(matches!(byte_error, VideoError::ByteLimit));
    }
}
