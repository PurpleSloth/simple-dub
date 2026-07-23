//! Общий формат временных реплик и работа с SRT.

use serde::{Deserialize, Serialize};
use thiserror::Error;

/// Текстовая реплика с абсолютным временным интервалом.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SubtitleSegment {
    pub id: String,
    pub start_ms: u64,
    pub end_ms: u64,
    pub text: String,
}

impl SubtitleSegment {
    /// Доступная длительность реплики.
    pub fn duration_ms(&self) -> u64 {
        self.end_ms.saturating_sub(self.start_ms)
    }
}

/// Ошибка разбора SRT.
#[derive(Debug, Error)]
pub enum SubtitleError {
    #[error("пустой блок субтитров")]
    EmptyBlock,
    #[error("в блоке {block} отсутствует строка времени")]
    MissingTiming { block: usize },
    #[error("неверная временная метка «{value}»")]
    InvalidTimestamp { value: String },
    #[error("неверный интервал в блоке {block}: конец не позже начала")]
    ReversedInterval { block: usize },
}

/// Разобрать SRT, нормализовав текст для последующей озвучки.
pub fn parse_srt(input: &str) -> Result<Vec<SubtitleSegment>, SubtitleError> {
    let normalized = input
        .trim_start_matches('\u{feff}')
        .replace("\r\n", "\n")
        .replace('\r', "\n");
    let mut segments = Vec::new();

    for (position, block) in normalized.split("\n\n").enumerate() {
        let lines: Vec<&str> = block
            .lines()
            .filter(|line| !line.trim().is_empty())
            .collect();
        if lines.is_empty() {
            continue;
        }

        let timing_position = lines.iter().position(|line| line.contains("-->")).ok_or(
            SubtitleError::MissingTiming {
                block: position + 1,
            },
        )?;
        let timing = lines[timing_position];
        let (start, end) =
            timing
                .split_once("-->")
                .ok_or_else(|| SubtitleError::InvalidTimestamp {
                    value: timing.to_owned(),
                })?;
        let start_ms = parse_timestamp(start.trim())?;
        let end_ms = parse_timestamp(end.split_whitespace().next().unwrap_or_default().trim())?;
        if end_ms <= start_ms {
            return Err(SubtitleError::ReversedInterval {
                block: position + 1,
            });
        }

        let id = if timing_position > 0 {
            lines[timing_position - 1].trim().to_owned()
        } else {
            (segments.len() + 1).to_string()
        };
        let text = normalize_spoken_text(&lines[timing_position + 1..].join(" "));
        if text.is_empty() {
            continue;
        }
        segments.push(SubtitleSegment {
            id,
            start_ms,
            end_ms,
            text,
        });
    }

    if segments.is_empty() && !normalized.trim().is_empty() {
        return Err(SubtitleError::EmptyBlock);
    }
    Ok(segments)
}

/// Сериализовать реплики в стандартный SRT.
pub fn write_srt(segments: &[SubtitleSegment]) -> String {
    let mut output = String::new();
    for (position, segment) in segments.iter().enumerate() {
        output.push_str(&(position + 1).to_string());
        output.push('\n');
        output.push_str(&format_timestamp(segment.start_ms));
        output.push_str(" --> ");
        output.push_str(&format_timestamp(segment.end_ms));
        output.push('\n');
        output.push_str(&segment.text);
        output.push_str("\n\n");
    }
    output
}

fn parse_timestamp(value: &str) -> Result<u64, SubtitleError> {
    let normalized = value.replace('.', ",");
    let (clock, millis) =
        normalized
            .split_once(',')
            .ok_or_else(|| SubtitleError::InvalidTimestamp {
                value: value.to_owned(),
            })?;
    let parts: Vec<&str> = clock.split(':').collect();
    if parts.len() != 3 {
        return Err(SubtitleError::InvalidTimestamp {
            value: value.to_owned(),
        });
    }
    let parse = |part: &str| {
        part.parse::<u64>()
            .map_err(|_| SubtitleError::InvalidTimestamp {
                value: value.to_owned(),
            })
    };
    let hours = parse(parts[0])?;
    let minutes = parse(parts[1])?;
    let seconds = parse(parts[2])?;
    let millis = match millis.len() {
        1 => parse(millis)? * 100,
        2 => parse(millis)? * 10,
        _ => parse(&millis[..millis.len().min(3)])?,
    };
    if minutes >= 60 || seconds >= 60 {
        return Err(SubtitleError::InvalidTimestamp {
            value: value.to_owned(),
        });
    }
    Ok((((hours * 60 + minutes) * 60 + seconds) * 1_000) + millis)
}

fn format_timestamp(value_ms: u64) -> String {
    let millis = value_ms % 1_000;
    let total_seconds = value_ms / 1_000;
    let seconds = total_seconds % 60;
    let total_minutes = total_seconds / 60;
    let minutes = total_minutes % 60;
    let hours = total_minutes / 60;
    format!("{hours:02}:{minutes:02}:{seconds:02},{millis:03}")
}

fn normalize_spoken_text(value: &str) -> String {
    let mut output = String::with_capacity(value.len());
    let mut angle_depth = 0_u32;
    let mut brace_depth = 0_u32;
    for character in value.chars() {
        match character {
            '<' => angle_depth += 1,
            '>' if angle_depth > 0 => angle_depth -= 1,
            '{' if angle_depth == 0 => brace_depth += 1,
            '}' if brace_depth > 0 => brace_depth -= 1,
            _ if angle_depth == 0 && brace_depth == 0 => output.push(character),
            _ => {}
        }
    }

    output
        .replace("&amp;", "&")
        .replace("&quot;", "\"")
        .replace("&#39;", "'")
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}
