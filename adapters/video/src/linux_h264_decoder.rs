use std::path::Path;

use openh264::OpenH264API;
use openh264::decoder::{Decoder, DecoderConfig};
use openh264::formats::YUVSource;

use crate::VideoError;

pub(crate) struct H264Decoder {
    decoder: Decoder,
    nal_length_size: usize,
    annex_b: Vec<u8>,
}

impl H264Decoder {
    pub(crate) fn new(codec_private: &[u8]) -> Result<Self, VideoError> {
        let (nal_length_size, headers) = parse_avc_decoder_configuration(codec_private)?;
        let api = load_openh264()?;
        let mut decoder = Decoder::with_api_config(api, DecoderConfig::new())
            .map_err(|error| platform_error("initialize OpenH264", error))?;
        decoder
            .decode(&headers)
            .map_err(|error| platform_error("decode H.264 headers", error))?;
        Ok(Self {
            decoder,
            nal_length_size,
            annex_b: Vec::new(),
        })
    }

    pub(crate) fn decode_into(
        &mut self,
        packet: &[u8],
        expected_width: u32,
        expected_height: u32,
        output: &mut Vec<u8>,
    ) -> Result<bool, VideoError> {
        avc_packet_to_annex_b(packet, self.nal_length_size, &mut self.annex_b)?;
        let Some(image) = self
            .decoder
            .decode(&self.annex_b)
            .map_err(|error| platform_error("decode H.264 frame", error))?
        else {
            return Ok(false);
        };
        let dimensions = image.dimensions();
        let expected = (
            usize::try_from(expected_width).map_err(|_| VideoError::SizeOverflow)?,
            usize::try_from(expected_height).map_err(|_| VideoError::SizeOverflow)?,
        );
        if dimensions != expected {
            return Err(VideoError::DynamicFormatUnsupported);
        }
        let output_len = expected
            .0
            .checked_mul(expected.1)
            .and_then(|pixels| pixels.checked_mul(4))
            .ok_or(VideoError::SizeOverflow)?;
        output.resize(output_len, 0);
        image.write_rgba8(output);
        Ok(true)
    }
}

fn load_openh264() -> Result<OpenH264API, VideoError> {
    let configured = std::env::var_os("TREND_OPENH264_LIBRARY");
    let candidates = configured.as_deref().map(Path::new).into_iter().chain([
        Path::new("libopenh264.so.8"),
        Path::new("libopenh264.so.7"),
        Path::new("libopenh264.so.6"),
        Path::new("libopenh264.so"),
    ]);
    let mut last_error = None;
    for path in candidates {
        let result = unsafe { OpenH264API::from_blob_path_unchecked(path) };
        match result {
            Ok(api) => return Ok(api),
            Err(error) => last_error = Some(error.to_string()),
        }
    }
    Err(VideoError::Platform(format!(
        "could not load the OpenH264 runtime; install libopenh264-8 or set TREND_OPENH264_LIBRARY{}",
        last_error.map_or_else(String::new, |error| format!(": {error}"))
    )))
}

fn parse_avc_decoder_configuration(data: &[u8]) -> Result<(usize, Vec<u8>), VideoError> {
    if data.len() < 7 || data[0] != 1 {
        return Err(invalid_avc("missing AVCDecoderConfigurationRecord"));
    }
    let nal_length_size = usize::from((data[4] & 0x03) + 1);
    if nal_length_size == 3 {
        return Err(invalid_avc("invalid NAL length size"));
    }

    let mut offset = 6;
    let mut headers = Vec::new();
    let sequence_count = usize::from(data[5] & 0x1f);
    for _ in 0..sequence_count {
        append_configuration_nal(data, &mut offset, &mut headers)?;
    }
    let picture_count = *data
        .get(offset)
        .ok_or_else(|| invalid_avc("missing picture parameter set count"))?;
    offset += 1;
    for _ in 0..picture_count {
        append_configuration_nal(data, &mut offset, &mut headers)?;
    }
    if headers.is_empty() {
        return Err(invalid_avc("missing sequence and picture parameter sets"));
    }
    Ok((nal_length_size, headers))
}

fn append_configuration_nal(
    data: &[u8],
    offset: &mut usize,
    output: &mut Vec<u8>,
) -> Result<(), VideoError> {
    let length_bytes = data
        .get(*offset..offset.saturating_add(2))
        .ok_or_else(|| invalid_avc("truncated parameter-set length"))?;
    let length = usize::from(u16::from_be_bytes([length_bytes[0], length_bytes[1]]));
    *offset = offset.saturating_add(2);
    let nal = data
        .get(*offset..offset.saturating_add(length))
        .ok_or_else(|| invalid_avc("truncated parameter set"))?;
    if nal.is_empty() {
        return Err(invalid_avc("empty parameter set"));
    }
    output.extend_from_slice(&[0, 0, 0, 1]);
    output.extend_from_slice(nal);
    *offset = offset.saturating_add(length);
    Ok(())
}

fn avc_packet_to_annex_b(
    packet: &[u8],
    nal_length_size: usize,
    output: &mut Vec<u8>,
) -> Result<(), VideoError> {
    output.clear();
    let mut offset = 0;
    while offset < packet.len() {
        let length_bytes = packet
            .get(offset..offset.saturating_add(nal_length_size))
            .ok_or_else(|| invalid_avc("truncated NAL length"))?;
        let mut length = 0_usize;
        for byte in length_bytes {
            length = length
                .checked_shl(8)
                .and_then(|value| value.checked_add(usize::from(*byte)))
                .ok_or(VideoError::SizeOverflow)?;
        }
        offset = offset.saturating_add(nal_length_size);
        let nal = packet
            .get(offset..offset.saturating_add(length))
            .ok_or_else(|| invalid_avc("truncated NAL unit"))?;
        if nal.is_empty() {
            return Err(invalid_avc("empty NAL unit"));
        }
        output.extend_from_slice(&[0, 0, 0, 1]);
        output.extend_from_slice(nal);
        offset = offset.saturating_add(length);
    }
    if output.is_empty() {
        return Err(invalid_avc("empty H.264 packet"));
    }
    Ok(())
}

fn invalid_avc(message: &str) -> VideoError {
    VideoError::Platform(format!("invalid H.264 AVC data: {message}"))
}

fn platform_error(operation: &str, error: impl std::fmt::Display) -> VideoError {
    VideoError::Platform(format!("failed to {operation}: {error}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn converts_avc_configuration_and_packets_to_annex_b() {
        let config = [
            1, 100, 0, 31, 0xff, 0xe1, 0, 2, 0x67, 0x64, 1, 0, 2, 0x68, 0xee,
        ];
        let (length_size, headers) = parse_avc_decoder_configuration(&config).unwrap();
        assert_eq!(length_size, 4);
        assert_eq!(headers, [0, 0, 0, 1, 0x67, 0x64, 0, 0, 0, 1, 0x68, 0xee]);

        let mut packet = Vec::new();
        avc_packet_to_annex_b(&[0, 0, 0, 2, 0x65, 0x88], length_size, &mut packet).unwrap();
        assert_eq!(packet, [0, 0, 0, 1, 0x65, 0x88]);
    }

    #[test]
    fn rejects_truncated_avc_data() {
        assert!(parse_avc_decoder_configuration(&[1, 2, 3]).is_err());
        let mut output = Vec::new();
        assert!(avc_packet_to_annex_b(&[0, 0, 0, 4, 1], 4, &mut output).is_err());
    }
}
