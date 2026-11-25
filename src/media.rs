use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::ffi::{OsStr, OsString};
use std::io::BufReader;
use std::path::Path;
use std::process::Command;
use std::sync::OnceLock;

static FFPROBE_EXECUTABLE: OnceLock<Option<OsString>> = OnceLock::new();

const VIDEO_EXTENSIONS: &[&str] = &[
    "asf", "avi", "divx", "f4v", "flv", "m1v", "m2v", "m4v", "mkv", "mov",
    "mp4", "mpe", "mpeg", "mpg", "ogm", "ogv", "rm", "rmvb", "ts", "vob",
    "webm", "wmv", "xvid",
];
const AUDIO_EXTENSIONS: &[&str] = &[
    "aac", "aif", "aiff", "alac", "flac", "m4a", "mp3", "ogg", "opus", "wav", "wma",
];
const IMAGE_EXTENSIONS: &[&str] = &[
    "avif", "bmp", "gif", "heic", "heif", "jpeg", "jpg", "png", "svg", "tif", "tiff", "webp",
];

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(default)]
pub struct MediaMetadata {
    pub duration_ms: Option<u64>,
    pub width: Option<u32>,
    pub height: Option<u32>,
    pub video_codec: Option<String>,
    pub audio_codec: Option<String>,
    /// Frame rate in thousandths of a frame per second (29970 = 29.97 fps).
    pub frame_rate_milli: Option<u32>,
    pub container: Option<String>,
    pub bit_rate: Option<u64>,
    pub sample_rate: Option<u32>,
    pub channels: Option<u32>,
    pub image_format: Option<String>,
    pub camera_make: Option<String>,
    pub camera_model: Option<String>,
    pub captured_at: Option<String>,
    pub orientation: Option<String>,
}

pub fn is_supported_media_extension(extension: Option<&str>) -> bool {
    let Some(extension) = extension else { return false; };
    let extension = extension.trim_start_matches('.').to_ascii_lowercase();
    VIDEO_EXTENSIONS.contains(&extension.as_str())
        || AUDIO_EXTENSIONS.contains(&extension.as_str())
        || IMAGE_EXTENSIONS.contains(&extension.as_str())
}

pub fn is_supported_image_extension(extension: Option<&str>) -> bool {
    let Some(extension) = extension else { return false; };
    IMAGE_EXTENSIONS.contains(&extension.trim_start_matches('.').to_ascii_lowercase().as_str())
}

/// Read image dimensions and basic EXIF fields without decoding image pixels.
pub fn extract_image_metadata(path: &Path, extension: Option<&str>) -> Option<MediaMetadata> {
    if !is_supported_image_extension(extension) {
        return None;
    }

    if extension.is_some_and(|value| value.eq_ignore_ascii_case("svg")) {
        if let Some(metadata) = extract_svg_metadata(path) {
            return Some(metadata);
        }
    }

    let reader = image::ImageReader::open(path).ok()?.with_guessed_format().ok()?;
    let format = reader.format().map(|value| format!("{value:?}").to_ascii_lowercase());
    let (width, height) = reader.into_dimensions().ok()?;
    let exif = exif::Reader::new()
        .read_from_container(&mut BufReader::new(std::fs::File::open(path).ok()?))
        .ok();
    let exif_value = |tag| {
        exif.as_ref()
            .and_then(|reader| reader.get_field(tag, exif::In::PRIMARY))
            .map(|field| field.display_value().to_string())
    };

    Some(MediaMetadata {
        width: Some(width),
        height: Some(height),
        image_format: format.or_else(|| extension.map(|value| value.trim_start_matches('.').to_ascii_lowercase())),
        camera_make: exif_value(exif::Tag::Make),
        camera_model: exif_value(exif::Tag::Model),
        captured_at: exif_value(exif::Tag::DateTimeOriginal).or_else(|| exif_value(exif::Tag::DateTime)),
        orientation: exif_value(exif::Tag::Orientation),
        ..Default::default()
    })
}

fn extract_svg_metadata(path: &Path) -> Option<MediaMetadata> {
    let text = std::fs::read_to_string(path).ok()?;
    if text.len() > 16 * 1024 * 1024 { return None; }
    let document = roxmltree::Document::parse(&text).ok()?;
    let root = document.root_element();
    if root.tag_name().name() != "svg" { return None; }
    let parse_dimension = |value: Option<&str>| {
        value.and_then(|value| {
            let numeric = value.trim().trim_end_matches("px").trim_end_matches("pt");
            numeric.parse::<f64>().ok().filter(|number| number.is_finite() && *number > 0.0 && *number <= u32::MAX as f64)
                .map(|number| number.round() as u32)
        })
    };
    let view_box = root.attribute("viewBox").or_else(|| root.attribute("viewbox"))
        .and_then(|value| {
            let values = value.split(|ch: char| ch.is_ascii_whitespace() || ch == ',')
                .filter(|part| !part.is_empty()).collect::<Vec<_>>();
            if values.len() == 4 {
                Some((values[2].parse::<f64>().ok()?, values[3].parse::<f64>().ok()?))
            } else { None }
        });
    let width = parse_dimension(root.attribute("width")).or_else(|| view_box.map(|value| value.0.round() as u32));
    let height = parse_dimension(root.attribute("height")).or_else(|| view_box.map(|value| value.1.round() as u32));
    Some(MediaMetadata {
        width,
        height,
        image_format: Some("svg".into()),
        ..Default::default()
    })
}

/// Uses ffprobe when it is installed. Missing ffprobe or malformed media never
/// prevents normal file indexing; it only leaves the optional metadata empty.
pub fn extract_media_metadata(path: &Path, extension: Option<&str>) -> Option<MediaMetadata> {
    if !is_supported_media_extension(extension) {
        return None;
    }

    let executable = ffprobe_executable()?;
    let output = Command::new(executable)
        .args([
            "-v", "error",
            "-probesize", "5000000",
            "-analyzeduration", "5000000",
            "-show_entries",
            "format=duration,format_name,bit_rate:stream=codec_type,codec_name,width,height,r_frame_rate,sample_rate,channels",
            "-of", "json",
        ])
        .arg(path)
        .output()
        .ok()?;

    if !output.status.success() {
        return None;
    }

    parse_ffprobe_json(&output.stdout)
}

fn ffprobe_executable() -> Option<&'static OsStr> {
    FFPROBE_EXECUTABLE.get_or_init(|| {
        let candidate = std::env::var_os("DEEP_SEARCH_FFPROBE").unwrap_or_else(|| "ffprobe".into());
        Command::new(&candidate)
            .arg("-version")
            .output()
            .ok()
            .filter(|output| output.status.success())
            .map(|_| candidate)
    }).as_deref()
}

fn parse_ffprobe_json(bytes: &[u8]) -> Option<MediaMetadata> {
    let root: Value = serde_json::from_slice(bytes).ok()?;
    let format = root.get("format");
    let streams = root.get("streams").and_then(Value::as_array);

    let duration_ms = format
        .and_then(|value| value.get("duration"))
        .and_then(parse_number)
        .filter(|value| value.is_finite() && *value >= 0.0)
        .map(|value| (value * 1000.0).round().min(u64::MAX as f64) as u64);
    let container = format
        .and_then(|value| value.get("format_name"))
        .and_then(Value::as_str)
        .map(str::to_owned);
    let bit_rate = format
        .and_then(|value| value.get("bit_rate"))
        .and_then(parse_u64);

    let video = streams.and_then(|items| {
        items.iter().find(|stream| stream.get("codec_type").and_then(Value::as_str) == Some("video"))
    });
    let audio = streams.and_then(|items| {
        items.iter().find(|stream| stream.get("codec_type").and_then(Value::as_str) == Some("audio"))
    });

    let metadata = MediaMetadata {
        duration_ms,
        width: video.and_then(|stream| stream.get("width")).and_then(parse_u32),
        height: video.and_then(|stream| stream.get("height")).and_then(parse_u32),
        video_codec: video
            .and_then(|stream| stream.get("codec_name"))
            .and_then(Value::as_str)
            .map(str::to_owned),
        audio_codec: audio
            .and_then(|stream| stream.get("codec_name"))
            .and_then(Value::as_str)
            .map(str::to_owned),
        frame_rate_milli: video
            .and_then(|stream| stream.get("r_frame_rate"))
            .and_then(Value::as_str)
            .and_then(parse_frame_rate),
        container,
        bit_rate,
        sample_rate: audio.and_then(|stream| stream.get("sample_rate")).and_then(parse_u32),
        channels: audio.and_then(|stream| stream.get("channels")).and_then(parse_u32),
    };

    let has_metadata = metadata.duration_ms.is_some()
        || metadata.width.is_some()
        || metadata.height.is_some()
        || metadata.video_codec.is_some()
        || metadata.frame_rate_milli.is_some()
        || metadata.audio_codec.is_some()
        || metadata.container.is_some()
        || metadata.bit_rate.is_some()
        || metadata.sample_rate.is_some()
        || metadata.channels.is_some();
    has_metadata.then_some(metadata)
}

fn parse_number(value: &Value) -> Option<f64> {
    value.as_f64().or_else(|| value.as_str()?.parse().ok())
}

fn parse_u64(value: &Value) -> Option<u64> {
    value.as_u64().or_else(|| value.as_str()?.parse().ok())
}

fn parse_u32(value: &Value) -> Option<u32> {
    value.as_u64().and_then(|number| u32::try_from(number).ok())
        .or_else(|| value.as_str()?.parse().ok())
}

fn parse_frame_rate(value: &str) -> Option<u32> {
    let (numerator, denominator) = if let Some((left, right)) = value.split_once('/') {
        (left.parse::<f64>().ok()?, right.parse::<f64>().ok()?)
    } else {
        (value.parse::<f64>().ok()?, 1.0)
    };
    if !numerator.is_finite() || !denominator.is_finite() || denominator <= 0.0 || numerator <= 0.0 {
        return None;
    }
    let milli = (numerator / denominator * 1000.0).round();
    (milli > 0.0 && milli <= u32::MAX as f64).then_some(milli as u32)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{SystemTime, UNIX_EPOCH};

    #[test]
    fn extracts_dimensions_and_format_from_png_without_decoding_pixels() {
        let unique = SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_nanos();
        let path = std::env::temp_dir().join(format!("deep-search-image-{unique}.png"));
        let image = image::RgbaImage::from_pixel(7, 5, image::Rgba([1, 2, 3, 255]));
        image.save(&path).unwrap();

        let metadata = extract_image_metadata(&path, Some("png")).unwrap();
        assert_eq!(metadata.width, Some(7));
        assert_eq!(metadata.height, Some(5));
        assert_eq!(metadata.image_format.as_deref(), Some("png"));
        std::fs::remove_file(path).unwrap();
    }

    #[test]
    fn extracts_svg_dimensions_from_viewbox() {
        let unique = SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_nanos();
        let path = std::env::temp_dir().join(format!("deep-search-image-{unique}.svg"));
        std::fs::write(&path, r#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 640 480"></svg>"#).unwrap();

        let metadata = extract_image_metadata(&path, Some("svg")).unwrap();
        assert_eq!(metadata.width, Some(640));
        assert_eq!(metadata.height, Some(480));
        assert_eq!(metadata.image_format.as_deref(), Some("svg"));
        std::fs::remove_file(path).unwrap();
    }

    #[test]
    fn legacy_media_metadata_without_image_fields_still_deserializes() {
        let legacy = r#"{"duration_ms":1200,"width":1920,"height":1080,"video_codec":"h264"}"#;
        let metadata: MediaMetadata = serde_json::from_str(legacy).unwrap();
        assert_eq!(metadata.duration_ms, Some(1200));
        assert_eq!(metadata.video_codec.as_deref(), Some("h264"));
        assert_eq!(metadata.camera_model, None);
    }
    use super::*;

    #[test]
    fn parses_ffprobe_video_and_audio_stream_metadata() {
        let json = br#"{
            "streams": [
                {"codec_type":"video","codec_name":"h264","width":1920,"height":1080,"r_frame_rate":"30000/1001"},
                {"codec_type":"audio","codec_name":"aac","sample_rate":"48000","channels":2}
            ],
            "format": {"duration":"123.456","format_name":"mov,mp4,m4a","bit_rate":"2500000"}
        }"#;
        let metadata = parse_ffprobe_json(json).unwrap();
        assert_eq!(metadata.duration_ms, Some(123_456));
        assert_eq!(metadata.width, Some(1920));
        assert_eq!(metadata.height, Some(1080));
        assert_eq!(metadata.video_codec.as_deref(), Some("h264"));
        assert_eq!(metadata.audio_codec.as_deref(), Some("aac"));
        assert_eq!(metadata.frame_rate_milli, Some(29_970));
        assert_eq!(metadata.container.as_deref(), Some("mov,mp4,m4a"));
        assert_eq!(metadata.bit_rate, Some(2_500_000));
        assert_eq!(metadata.sample_rate, Some(48_000));
        assert_eq!(metadata.channels, Some(2));
    }

    #[test]
    fn ignores_camera_and_unknown_extensions() {
        assert!(is_supported_media_extension(Some("mkv")));
        assert!(is_supported_media_extension(Some("flac")));
        assert!(!is_supported_media_extension(Some("m2ts")));
        assert!(!is_supported_media_extension(Some("mxf")));
        assert!(!is_supported_media_extension(Some("txt")));
    }

    #[test]
    fn keeps_metadata_when_only_frame_rate_is_available() {
        let json = br#"{"streams":[{"codec_type":"video","r_frame_rate":"24/1"}]}"#;
        let metadata = parse_ffprobe_json(json).unwrap();
        assert_eq!(metadata.frame_rate_milli, Some(24_000));
    }

    #[test]
    fn malformed_probe_output_is_ignored() {
        assert!(parse_ffprobe_json(b"not json").is_none());
    }
}
