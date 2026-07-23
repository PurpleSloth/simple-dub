//! Решения о маршруте обработки и подгонке длительности реплик.

use crate::media::{StreamInfo, SubtitleKind};

/// Ветка обработки после выбора субтитров.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PipelineRoute {
    UseRussianSubtitles,
    TranslateSubtitles,
    TranscribeAndTranslate,
}

/// Действие после измерения реальной длительности TTS-фрагмента.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum FitDecision {
    Accept,
    SpeedUp { factor: f32 },
    ShortenTranslation { target_ratio: f32 },
}

/// Выбрать ветку пайплайна по типу и языку выбранных субтитров.
pub fn choose_pipeline_route(
    subtitle: Option<&StreamInfo>,
    target_language: &str,
) -> PipelineRoute {
    let Some(subtitle) = subtitle else {
        return PipelineRoute::TranscribeAndTranslate;
    };

    if subtitle.subtitle_kind == Some(SubtitleKind::Image) {
        return PipelineRoute::TranscribeAndTranslate;
    }

    if is_same_language(subtitle.language.as_deref(), target_language) {
        PipelineRoute::UseRussianSubtitles
    } else {
        PipelineRoute::TranslateSubtitles
    }
}

/// Решить, помещается ли TTS-фрагмент в исходный временной слот.
pub fn decide_duration_fit(slot_ms: u64, synthesized_ms: u64) -> FitDecision {
    if synthesized_ms == 0 || synthesized_ms <= slot_ms.saturating_mul(110) / 100 {
        return FitDecision::Accept;
    }

    let ratio = synthesized_ms as f32 / slot_ms.max(1) as f32;
    if ratio <= 1.25 {
        return FitDecision::SpeedUp {
            factor: round_three(ratio),
        };
    }

    FitDecision::ShortenTranslation {
        target_ratio: round_three((1.0 / ratio) * 0.95),
    }
}

fn is_same_language(stream_language: Option<&str>, target_language: &str) -> bool {
    let target = canonical_language(target_language);
    stream_language
        .map(canonical_language)
        .is_some_and(|language| language == target)
}

fn canonical_language(language: &str) -> &str {
    match language.trim().to_lowercase().as_str() {
        "ru" | "rus" | "russian" => "ru",
        _ => language,
    }
}

fn round_three(value: f32) -> f32 {
    (value * 1_000.0).round() / 1_000.0
}
