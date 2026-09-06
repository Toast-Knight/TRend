use std::path::Path;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum MediaCategory {
    Image,
    Animation,
    Video,
    Audio,
    Document,
    Archive,
    Model,
    Unknown,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum MediaFormat {
    Png,
    Jpeg,
    Gif,
    WebP,
    Bmp,
    Tiff,
    Svg,
    Mp4,
    Mkv,
    Avi,
    Mov,
    WebM,
    Wav,
    Mp3,
    Pdf,
    Zip,
    Gltf,
    Glb,
    Unknown,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DetectionEvidence {
    Magic,
    Mime,
    Extension,
    None,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct MediaDetection {
    pub category: MediaCategory,
    pub format: MediaFormat,
    pub evidence: DetectionEvidence,
}

impl MediaDetection {
    const fn new(
        category: MediaCategory,
        format: MediaFormat,
        evidence: DetectionEvidence,
    ) -> Self {
        Self {
            category,
            format,
            evidence,
        }
    }

    const fn unknown() -> Self {
        Self::new(
            MediaCategory::Unknown,
            MediaFormat::Unknown,
            DetectionEvidence::None,
        )
    }
}

pub fn detect(path: &Path, mime: Option<&str>, bytes: &[u8]) -> MediaDetection {
    detect_magic(bytes)
        .or_else(|| mime.and_then(detect_mime))
        .or_else(|| detect_extension(path))
        .unwrap_or_else(MediaDetection::unknown)
}

fn detected(category: MediaCategory, format: MediaFormat) -> MediaDetection {
    MediaDetection::new(category, format, DetectionEvidence::Magic)
}

fn detect_magic(bytes: &[u8]) -> Option<MediaDetection> {
    if bytes.starts_with(b"\x89PNG\r\n\x1a\n") {
        return Some(detected(MediaCategory::Image, MediaFormat::Png));
    }
    if bytes.starts_with(&[0xff, 0xd8, 0xff]) {
        return Some(detected(MediaCategory::Image, MediaFormat::Jpeg));
    }
    if bytes.starts_with(b"GIF87a") || bytes.starts_with(b"GIF89a") {
        return Some(detected(MediaCategory::Animation, MediaFormat::Gif));
    }
    if bytes.len() >= 12 && bytes.starts_with(b"RIFF") {
        match &bytes[8..12] {
            b"WEBP" => {
                let category =
                    if bytes.len() >= 21 && &bytes[12..16] == b"VP8X" && bytes[20] & 0x02 != 0 {
                        MediaCategory::Animation
                    } else {
                        MediaCategory::Image
                    };
                return Some(detected(category, MediaFormat::WebP));
            }
            b"AVI " => return Some(detected(MediaCategory::Video, MediaFormat::Avi)),
            b"WAVE" => return Some(detected(MediaCategory::Audio, MediaFormat::Wav)),
            _ => {}
        }
    }
    if bytes.starts_with(b"BM") {
        return Some(detected(MediaCategory::Image, MediaFormat::Bmp));
    }
    if bytes.starts_with(b"II*\0") || bytes.starts_with(b"MM\0*") {
        return Some(detected(MediaCategory::Image, MediaFormat::Tiff));
    }
    if bytes.starts_with(b"%PDF-") {
        return Some(detected(MediaCategory::Document, MediaFormat::Pdf));
    }
    let text = bytes
        .strip_prefix(&[0xef, 0xbb, 0xbf])
        .unwrap_or(bytes)
        .iter()
        .copied()
        .skip_while(u8::is_ascii_whitespace)
        .take(512)
        .collect::<Vec<_>>();
    if text.starts_with(b"<svg")
        || (text.starts_with(b"<?xml") && text.windows(4).any(|window| window == b"<svg"))
    {
        return Some(detected(MediaCategory::Image, MediaFormat::Svg));
    }
    if bytes.starts_with(b"PK\x03\x04")
        || bytes.starts_with(b"PK\x05\x06")
        || bytes.starts_with(b"PK\x07\x08")
    {
        return Some(detected(MediaCategory::Archive, MediaFormat::Zip));
    }
    if bytes.starts_with(b"glTF") {
        return Some(detected(MediaCategory::Model, MediaFormat::Glb));
    }
    if bytes.starts_with(b"ID3")
        || (bytes.len() >= 2 && bytes[0] == 0xff && bytes[1] & 0xe0 == 0xe0)
    {
        return Some(detected(MediaCategory::Audio, MediaFormat::Mp3));
    }
    if bytes.len() >= 12 && &bytes[4..8] == b"ftyp" {
        let format = match &bytes[8..12] {
            b"qt  " => MediaFormat::Mov,
            _ => MediaFormat::Mp4,
        };
        return Some(detected(MediaCategory::Video, format));
    }
    if bytes.starts_with(&[0x1a, 0x45, 0xdf, 0xa3]) {
        let format = if bytes
            .windows(4)
            .any(|value| value.eq_ignore_ascii_case(b"webm"))
        {
            MediaFormat::WebM
        } else {
            MediaFormat::Mkv
        };
        return Some(detected(MediaCategory::Video, format));
    }
    None
}

fn detect_mime(mime: &str) -> Option<MediaDetection> {
    let mime = mime.split(';').next()?.trim().to_ascii_lowercase();
    let (category, format) = match mime.as_str() {
        "image/png" => (MediaCategory::Image, MediaFormat::Png),
        "image/jpeg" => (MediaCategory::Image, MediaFormat::Jpeg),
        "image/gif" => (MediaCategory::Animation, MediaFormat::Gif),
        "image/webp" => (MediaCategory::Image, MediaFormat::WebP),
        "image/bmp" => (MediaCategory::Image, MediaFormat::Bmp),
        "image/tiff" => (MediaCategory::Image, MediaFormat::Tiff),
        "image/svg+xml" => (MediaCategory::Image, MediaFormat::Svg),
        "video/mp4" => (MediaCategory::Video, MediaFormat::Mp4),
        "video/x-matroska" => (MediaCategory::Video, MediaFormat::Mkv),
        "video/x-msvideo" => (MediaCategory::Video, MediaFormat::Avi),
        "video/quicktime" => (MediaCategory::Video, MediaFormat::Mov),
        "video/webm" => (MediaCategory::Video, MediaFormat::WebM),
        "audio/wav" | "audio/x-wav" => (MediaCategory::Audio, MediaFormat::Wav),
        "audio/mpeg" => (MediaCategory::Audio, MediaFormat::Mp3),
        "application/pdf" => (MediaCategory::Document, MediaFormat::Pdf),
        "application/zip" => (MediaCategory::Archive, MediaFormat::Zip),
        "model/gltf+json" => (MediaCategory::Model, MediaFormat::Gltf),
        "model/gltf-binary" => (MediaCategory::Model, MediaFormat::Glb),
        _ => return None,
    };
    Some(MediaDetection::new(
        category,
        format,
        DetectionEvidence::Mime,
    ))
}

fn detect_extension(path: &Path) -> Option<MediaDetection> {
    let extension = path.extension()?.to_str()?;
    let (category, format) = match extension.to_ascii_lowercase().as_str() {
        "png" => (MediaCategory::Image, MediaFormat::Png),
        "jpg" | "jpeg" => (MediaCategory::Image, MediaFormat::Jpeg),
        "gif" => (MediaCategory::Animation, MediaFormat::Gif),
        "webp" => (MediaCategory::Image, MediaFormat::WebP),
        "bmp" => (MediaCategory::Image, MediaFormat::Bmp),
        "tif" | "tiff" => (MediaCategory::Image, MediaFormat::Tiff),
        "svg" => (MediaCategory::Image, MediaFormat::Svg),
        "mp4" => (MediaCategory::Video, MediaFormat::Mp4),
        "mkv" => (MediaCategory::Video, MediaFormat::Mkv),
        "avi" => (MediaCategory::Video, MediaFormat::Avi),
        "mov" => (MediaCategory::Video, MediaFormat::Mov),
        "webm" => (MediaCategory::Video, MediaFormat::WebM),
        "wav" => (MediaCategory::Audio, MediaFormat::Wav),
        "mp3" => (MediaCategory::Audio, MediaFormat::Mp3),
        "pdf" => (MediaCategory::Document, MediaFormat::Pdf),
        "zip" => (MediaCategory::Archive, MediaFormat::Zip),
        "gltf" => (MediaCategory::Model, MediaFormat::Gltf),
        "glb" => (MediaCategory::Model, MediaFormat::Glb),
        _ => return None,
    };
    Some(MediaDetection::new(
        category,
        format,
        DetectionEvidence::Extension,
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn magic_overrides_mime_and_extension() {
        let result = detect(
            Path::new("wrong.jpg"),
            Some("image/gif"),
            b"\x89PNG\r\n\x1a\n",
        );
        assert_eq!(result.format, MediaFormat::Png);
        assert_eq!(result.evidence, DetectionEvidence::Magic);
    }

    #[test]
    fn detects_animated_webp_from_vp8x_flags() {
        let mut bytes = b"RIFF\0\0\0\0WEBPVP8X\x0a\0\0\0\0".to_vec();
        bytes[20] = 0x02;
        let result = detect(Path::new("media"), None, &bytes);
        assert_eq!(result.category, MediaCategory::Animation);
        assert_eq!(result.format, MediaFormat::WebP);
    }

    #[test]
    fn uses_mime_then_extension_when_magic_is_unknown() {
        assert_eq!(
            detect(Path::new("media.bin"), Some("video/webm"), b"unknown").format,
            MediaFormat::WebM
        );
        assert_eq!(
            detect(Path::new("media.jpeg"), None, b"unknown").format,
            MediaFormat::Jpeg
        );
    }

    #[test]
    fn classifies_non_renderable_media_without_claiming_a_decoder() {
        assert_eq!(
            detect(Path::new("archive.zip"), None, b"PK\x03\x04").category,
            MediaCategory::Archive
        );
        assert_eq!(
            detect(Path::new("sound.mp3"), None, b"ID3").category,
            MediaCategory::Audio
        );
        assert_eq!(
            detect(Path::new("drawing"), None, b"<?xml?><svg></svg>").format,
            MediaFormat::Svg
        );
    }
}
