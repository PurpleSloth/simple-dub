//! Разбор метаданных `ffprobe` и классификация медиапотоков.

use serde::{Deserialize, Serialize};
use thiserror::Error;

/// Тип потока внутри медиаконтейнера.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum StreamType {
    Video,
    Audio,
    Subtitle,
    Other,
}

/// Представление субтитров, важное для выбора дальнейшего пайплайна.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum SubtitleKind {
    Text,
    Image,
}

/// Нормализованная информация о потоке.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct StreamInfo {
    pub index: u32,
    pub stream_type: StreamType,
    pub codec_name: Option<String>,
    pub language: Option<String>,
    pub title: Option<String>,
    pub channels: Option<u32>,
    pub channel_layout: Option<String>,
    pub is_default: bool,
    pub subtitle_kind: Option<SubtitleKind>,
}

/// Информация о контейнере и доступных потоках.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct MediaInfo {
    pub streams: Vec<StreamInfo>,
    pub chapter_count: usize,
    pub duration_seconds: Option<f64>,
    pub format_name: Option<String>,
}

impl MediaInfo {
    /// Вернуть все аудиопотоки в исходном порядке.
    pub fn audio_streams(&self) -> Vec<&StreamInfo> {
        self.streams
            .iter()
            .filter(|stream| stream.stream_type == StreamType::Audio)
            .collect()
    }

    /// Вернуть все потоки субтитров в исходном порядке.
    pub fn subtitle_streams(&self) -> Vec<&StreamInfo> {
        self.streams
            .iter()
            .filter(|stream| stream.stream_type == StreamType::Subtitle)
            .collect()
    }
}

/// Ошибка разбора ответа `ffprobe`.
#[derive(Debug, Error)]
pub enum MediaProbeError {
    #[error("некорректный JSON ffprobe: {0}")]
    InvalidJson(#[from] serde_json::Error),
}

#[derive(Debug, Deserialize)]
struct RawProbe {
    #[serde(default)]
    streams: Vec<RawStream>,
    #[serde(default)]
    chapters: Vec<serde_json::Value>,
    format: Option<RawFormat>,
}

#[derive(Debug, Deserialize)]
struct RawStream {
    index: u32,
    codec_name: Option<String>,
    codec_type: String,
    channels: Option<u32>,
    channel_layout: Option<String>,
    disposition: Option<RawDisposition>,
    tags: Option<RawTags>,
}

#[derive(Debug, Deserialize)]
struct RawDisposition {
    #[serde(default)]
    default: u8,
}

#[derive(Debug, Deserialize)]
struct RawTags {
    language: Option<String>,
    title: Option<String>,
}

#[derive(Debug, Deserialize)]
struct RawFormat {
    duration: Option<String>,
    format_name: Option<String>,
}

/// Разобрать JSON, полученный через `ffprobe -show_streams -show_chapters`.
pub fn parse_ffprobe_json(json: &str) -> Result<MediaInfo, MediaProbeError> {
    let raw: RawProbe = serde_json::from_str(json)?;

    let streams = raw
        .streams
        .into_iter()
        .map(|stream| {
            let stream_type = match stream.codec_type.as_str() {
                "video" => StreamType::Video,
                "audio" => StreamType::Audio,
                "subtitle" => StreamType::Subtitle,
                _ => StreamType::Other,
            };
            let subtitle_kind = (stream_type == StreamType::Subtitle)
                .then(|| classify_subtitle(stream.codec_name.as_deref()));
            let tags = stream.tags;

            StreamInfo {
                index: stream.index,
                stream_type,
                codec_name: stream.codec_name,
                language: tags
                    .as_ref()
                    .and_then(|value| value.language.as_deref())
                    .map(normalize_language),
                title: tags.and_then(|value| value.title),
                channels: stream.channels,
                channel_layout: stream.channel_layout,
                is_default: stream
                    .disposition
                    .is_some_and(|disposition| disposition.default == 1),
                subtitle_kind,
            }
        })
        .collect();

    let duration_seconds = raw
        .format
        .as_ref()
        .and_then(|format| format.duration.as_deref())
        .and_then(|duration| duration.parse::<f64>().ok());
    let format_name = raw.format.and_then(|format| format.format_name);

    Ok(MediaInfo {
        streams,
        chapter_count: raw.chapters.len(),
        duration_seconds,
        format_name,
    })
}

fn classify_subtitle(codec_name: Option<&str>) -> SubtitleKind {
    match codec_name.unwrap_or_default() {
        "hdmv_pgs_subtitle" | "dvd_subtitle" | "dvb_subtitle" | "xsub" => {
            SubtitleKind::Image
        }
        _ => SubtitleKind::Text,
    }
}

fn normalize_language(language: &str) -> String {
    language.trim().to_lowercase()
}
