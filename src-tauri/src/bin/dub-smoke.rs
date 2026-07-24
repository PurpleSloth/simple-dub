//! Headless-запуск полного конвейера для интеграционной проверки.

use std::path::PathBuf;

use simple_dub_core::tts::TtsEngine;
use simple_dub_desktop_lib::dubbing::{DubJobRequest, ProgressReporter, run_dub_job};

fn main() -> Result<(), String> {
    let arguments: Vec<String> = std::env::args().collect();
    if !(8..=9).contains(&arguments.len()) {
        return Err(
            "Использование: dub-smoke INPUT RUNTIME AUDIO_INDEX SUBTITLE_INDEX \
             DURATION_SECONDS AUDIO_STREAM_COUNT TTS [SUBTITLE_LANGUAGE]. \
             Вместо SUBTITLE_INDEX можно указать none."
                .to_owned(),
        );
    }
    let engine = match arguments[7].as_str() {
        "piper" => TtsEngine::PiperDmitriFp32,
        "silero" => TtsEngine::SileroEugene,
        value => return Err(format!("Неизвестный TTS: {value}")),
    };
    let subtitle_index = if arguments[4].eq_ignore_ascii_case("none") {
        None
    } else {
        Some(parse(&arguments[4], "SUBTITLE_INDEX")?)
    };
    let request = DubJobRequest {
        input_path: arguments[1].clone(),
        audio_stream_index: parse(&arguments[3], "AUDIO_INDEX")?,
        subtitle_stream_index: subtitle_index,
        subtitle_kind: subtitle_index.map(|_| "text".to_owned()),
        subtitle_language: subtitle_index.map(|_| {
            arguments
                .get(8)
                .cloned()
                .unwrap_or_else(|| "rus".to_owned())
        }),
        existing_audio_streams: parse(&arguments[6], "AUDIO_STREAM_COUNT")?,
        duration_seconds: arguments[5]
            .parse()
            .map_err(|error| format!("Неверный DURATION_SECONDS: {error}"))?,
        tts_engine: engine,
        ducking_gap_db: 14.0,
    };
    let result = run_dub_job(
        &ProgressReporter::silent(),
        &PathBuf::from(&arguments[2]),
        &request,
    )?;
    println!(
        "{}",
        serde_json::to_string(&result)
            .map_err(|error| format!("Не удалось вывести результат: {error}"))?
    );
    Ok(())
}

fn parse(value: &str, name: &str) -> Result<usize, String> {
    value
        .parse()
        .map_err(|error| format!("Неверный {name}: {error}"))
}
