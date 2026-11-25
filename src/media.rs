use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::ffi::{OsStr, OsString};
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

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
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
}

pub fn is_supported_media_extension(extension: Option<&str>) -> bool {
    let Some(extension) = extension else { return false; };
    let extension = extension.trim_start_matches('.').to_ascii_lowercase();
    VIDEO_EXTENSIONS.contains(&extension.as_str()) || AUDIO_EXTENSIONS.contains(&extension.as_str())
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
