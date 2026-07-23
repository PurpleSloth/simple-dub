//! Исполняемый конвейер одноголосого дубляжа.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::time::Duration;

use serde::{Deserialize, Serialize};
use simple_dub_core::commands::{MixOptions, MuxOptions, build_mix_filter, build_mux_args};
use simple_dub_core::subtitles::{SubtitleSegment, parse_srt, write_srt};
use simple_dub_core::tts::{PiperRuntime, SileroRuntime, TtsEngine};
use tauri::{AppHandle, Emitter};

use crate::settings::OpenRouterCredentialStore;

const TRANSLATION_MODEL: &str = "google/gemini-3.5-flash-lite";
const TARGET_SAMPLE_RATE: u32 = 48_000;

/// Параметры запуска полного задания из интерфейса.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DubJobRequest {
    pub input_path: String,
    pub audio_stream_index: usize,
    pub subtitle_stream_index: Option<usize>,
    pub subtitle_kind: Option<String>,
    pub subtitle_language: Option<String>,
    pub existing_audio_streams: usize,
    pub duration_seconds: f64,
    pub tts_engine: TtsEngine,
    pub original_volume: f32,
}

/// Итог успешно завершённого задания.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DubJobResult {
    pub output_path: String,
    pub segment_count: usize,
}

/// Событие прогресса для интерфейса.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct JobProgress {
    pub percent: u8,
    pub stage: &'static str,
    pub message: String,
}

/// Получатель прогресса для GUI или headless smoke-теста.
#[derive(Clone)]
pub struct ProgressReporter {
    app: Option<AppHandle>,
}

impl ProgressReporter {
    /// Отправлять события в окно Tauri.
    pub fn tauri(app: AppHandle) -> Self {
        Self { app: Some(app) }
    }

    /// Выполнять задание без GUI.
    pub fn silent() -> Self {
        Self { app: None }
    }

    fn emit(&self, progress: JobProgress) {
        if let Some(app) = &self.app {
            app.emit("job-progress", progress).ok();
        }
    }
}

/// Выполнить полный конвейер в блокирующем worker-потоке.
pub fn run_dub_job(
    progress: &ProgressReporter,
    runtime_root: &Path,
    request: &DubJobRequest,
) -> Result<DubJobResult, String> {
    let input_path = PathBuf::from(&request.input_path);
    if !input_path.is_file() {
        return Err(format!("Исходный файл не найден: {}", input_path.display()));
    }
    let ffmpeg = resolve_tool(runtime_root, "ffmpeg.exe", r"C:\ffmpeg\bin\ffmpeg.exe")?;
    let job_id = uuid::Uuid::new_v4().simple().to_string();
    let work_dir = runtime_root.join("jobs").join(&job_id);
    fs::create_dir_all(&work_dir)
        .map_err(|error| format!("Не удалось создать рабочий каталог: {error}"))?;

    emit(progress, 2, "prepare", "Подготовка временных файлов");
    let (mut segments, needs_translation) =
        obtain_segments(progress, runtime_root, &ffmpeg, request, &work_dir)?;
    if segments.is_empty() {
        return Err("Не найдено ни одной реплики для озвучки.".to_owned());
    }

    if needs_translation {
        emit(progress, 26, "translate", "Перевод реплик на русский");
        let key = OpenRouterCredentialStore::new()?.read()?;
        segments = translate_segments(progress, &key, &segments)?;
    }
    fs::write(work_dir.join("russian.srt"), write_srt(&segments))
        .map_err(|error| format!("Не удалось сохранить русские субтитры: {error}"))?;

    emit(
        progress,
        42,
        "tts",
        &format!("Озвучка через {}", request.tts_engine.display_name()),
    );
    let audio_fragments = synthesize_segments(
        progress,
        runtime_root,
        request.tts_engine,
        &segments,
        &work_dir,
    )?;

    emit(progress, 78, "align", "Выравнивание реплик по таймкодам");
    let voice_track = work_dir.join("voice.wav");
    assemble_voice_track(
        &ffmpeg,
        &segments,
        &audio_fragments,
        request.duration_seconds,
        &work_dir,
        &voice_track,
    )?;

    emit(progress, 88, "mix", "Смешивание с приглушённым оригиналом");
    let mixed_audio = work_dir.join("dubbed.mka");
    mix_audio(
        &ffmpeg,
        &input_path,
        request.audio_stream_index,
        request.original_volume,
        &voice_track,
        &mixed_audio,
    )?;

    emit(progress, 95, "mux", "Добавление новой дорожки в MKV");
    let output_path = output_path_for(&input_path);
    mux_output(
        &ffmpeg,
        &input_path,
        &mixed_audio,
        &output_path,
        request.existing_audio_streams,
    )?;

    emit(progress, 100, "done", "Дубляж готов");
    let result = DubJobResult {
        output_path: output_path.to_string_lossy().into_owned(),
        segment_count: segments.len(),
    };
    fs::remove_dir_all(&work_dir).ok();
    Ok(result)
}

fn obtain_segments(
    progress: &ProgressReporter,
    runtime_root: &Path,
    ffmpeg: &Path,
    request: &DubJobRequest,
    work_dir: &Path,
) -> Result<(Vec<SubtitleSegment>, bool), String> {
    let uses_text_subtitles =
        request.subtitle_stream_index.is_some() && request.subtitle_kind.as_deref() == Some("text");
    if uses_text_subtitles {
        emit(progress, 8, "subtitles", "Извлечение выбранных субтитров");
        let subtitle_path = work_dir.join("source.srt");
        run_checked(
            Command::new(ffmpeg)
                .args(["-y", "-v", "error", "-i"])
                .arg(&request.input_path)
                .args([
                    "-map",
                    &format!("0:{}", request.subtitle_stream_index.unwrap()),
                ])
                .arg(&subtitle_path),
            "Не удалось извлечь субтитры",
        )?;
        let content = fs::read_to_string(&subtitle_path)
            .map_err(|error| format!("Не удалось прочитать SRT: {error}"))?;
        let segments = parse_srt(&content).map_err(|error| error.to_string())?;
        let needs_translation = !is_russian(request.subtitle_language.as_deref());
        return Ok((segments, needs_translation));
    }

    emit(progress, 8, "asr", "Извлечение оригинальной аудиодорожки");
    let asr_audio = work_dir.join("asr.wav");
    run_checked(
        Command::new(ffmpeg)
            .args(["-y", "-v", "error", "-i"])
            .arg(&request.input_path)
            .args(["-map", &format!("0:{}", request.audio_stream_index)])
            .args(["-ac", "1", "-ar", "16000", "-c:a", "pcm_s16le"])
            .arg(&asr_audio),
        "Не удалось подготовить аудио для Whisper",
    )?;

    emit(progress, 15, "asr", "Распознавание речи через whisper.cpp");
    let whisper = resolve_tool(runtime_root, "whisper-cli.exe", "whisper-cli.exe")?;
    let model = runtime_root.join("models").join("ggml-large-v3-turbo.bin");
    if !model.is_file() {
        return Err(format!(
            "Модель Whisper не установлена: {}",
            model.display()
        ));
    }
    let output_base = work_dir.join("transcript");
    let mut command = Command::new(whisper);
    command
        .args(["--model"])
        .arg(model)
        .args(["--file"])
        .arg(&asr_audio)
        .args(["--language", "auto", "--output-srt", "--output-file"])
        .arg(&output_base)
        .args(["--print-progress", "--flash-attn"]);
    let vad = runtime_root.join("models").join("ggml-silero-v6.2.0.bin");
    if vad.is_file() {
        command.args(["--vad", "--vad-model"]).arg(vad);
    }
    run_checked(&mut command, "Whisper не смог распознать речь")?;
    let content = fs::read_to_string(output_base.with_extension("srt"))
        .map_err(|error| format!("Whisper не создал SRT: {error}"))?;
    let segments = parse_srt(&content).map_err(|error| error.to_string())?;
    Ok((segments, true))
}

#[derive(Serialize)]
struct TranslationInput<'a> {
    id: &'a str,
    duration_seconds: f64,
    text: &'a str,
}

#[derive(Deserialize)]
struct TranslationEnvelope {
    segments: Vec<TranslationOutput>,
}

#[derive(Deserialize)]
struct TranslationOutput {
    id: String,
    text: String,
}

#[derive(Deserialize)]
struct OpenRouterResponse {
    choices: Vec<OpenRouterChoice>,
}

#[derive(Deserialize)]
struct OpenRouterChoice {
    message: OpenRouterMessage,
}

#[derive(Deserialize)]
struct OpenRouterMessage {
    content: String,
}

fn translate_segments(
    progress: &ProgressReporter,
    key: &str,
    segments: &[SubtitleSegment],
) -> Result<Vec<SubtitleSegment>, String> {
    let client = reqwest::blocking::Client::builder()
        .timeout(Duration::from_secs(180))
        .build()
        .map_err(|error| format!("Не удалось создать HTTP-клиент: {error}"))?;
    let mut translated = Vec::with_capacity(segments.len());
    let batches = segments.len().div_ceil(24);

    for (batch_index, batch) in segments.chunks(24).enumerate() {
        let input: Vec<_> = batch
            .iter()
            .map(|segment| TranslationInput {
                id: &segment.id,
                duration_seconds: segment.duration_ms() as f64 / 1_000.0,
                text: &segment.text,
            })
            .collect();
        let prompt = format!(
            "Переведи реплики на естественный разговорный русский для одноголосого \
             дубляжа. Сохрани смысл, имена и порядок. Каждая фраза должна реально \
             помещаться в duration_seconds при нормальном темпе речи; сокращай \
             формулировку, но не добавляй новых фактов. Верни только JSON-объект \
             {{\"segments\":[{{\"id\":\"...\",\"text\":\"...\"}}]}} с теми же id.\n\n{}",
            serde_json::to_string(&input)
                .map_err(|error| format!("Не удалось подготовить перевод: {error}"))?
        );
        let response = client
            .post("https://openrouter.ai/api/v1/chat/completions")
            .bearer_auth(key)
            .header("X-Title", "Simple Dub")
            .json(&serde_json::json!({
                "model": TRANSLATION_MODEL,
                "messages": [
                    {
                        "role": "system",
                        "content": "Ты профессиональный переводчик субтитров для озвучки."
                    },
                    {"role": "user", "content": prompt}
                ],
                "temperature": 0.2,
                "reasoning": {"effort": "minimal", "exclude": true},
                "response_format": {"type": "json_object"}
            }))
            .send()
            .map_err(|error| format!("Ошибка OpenRouter: {error}"))?;
        let status = response.status();
        let body = response
            .text()
            .map_err(|error| format!("Не удалось прочитать ответ OpenRouter: {error}"))?;
        if !status.is_success() {
            return Err(format!("OpenRouter вернул {status}: {body}"));
        }
        let response: OpenRouterResponse = serde_json::from_str(&body)
            .map_err(|error| format!("Неверный ответ OpenRouter: {error}"))?;
        let content = response
            .choices
            .first()
            .ok_or("OpenRouter не вернул вариант перевода")?
            .message
            .content
            .trim();
        let json = extract_json_object(content)?;
        let envelope: TranslationEnvelope = serde_json::from_str(json)
            .map_err(|error| format!("Модель вернула неверный JSON: {error}"))?;
        if envelope.segments.len() != batch.len() {
            return Err(format!(
                "Модель вернула {} реплик вместо {}",
                envelope.segments.len(),
                batch.len()
            ));
        }
        for original in batch {
            let item = envelope
                .segments
                .iter()
                .find(|item| item.id == original.id)
                .ok_or_else(|| format!("В переводе отсутствует id {}", original.id))?;
            let text = item.text.trim();
            if text.is_empty() {
                return Err(format!("Пустой перевод для id {}", original.id));
            }
            translated.push(SubtitleSegment {
                id: original.id.clone(),
                start_ms: original.start_ms,
                end_ms: original.end_ms,
                text: text.to_owned(),
            });
        }
        let percent = 26 + (((batch_index + 1) * 14) / batches.max(1)) as u8;
        emit(
            progress,
            percent,
            "translate",
            &format!(
                "Переведено {} из {} реплик",
                translated.len(),
                segments.len()
            ),
        );
    }
    Ok(translated)
}

fn synthesize_segments(
    progress: &ProgressReporter,
    runtime_root: &Path,
    engine: TtsEngine,
    segments: &[SubtitleSegment],
    work_dir: &Path,
) -> Result<Vec<PathBuf>, String> {
    match engine {
        TtsEngine::PiperDmitriFp32 => synthesize_piper(progress, runtime_root, segments, work_dir),
        TtsEngine::SileroEugene => synthesize_silero(progress, runtime_root, segments, work_dir),
    }
}

fn synthesize_piper(
    progress: &ProgressReporter,
    runtime_root: &Path,
    segments: &[SubtitleSegment],
    work_dir: &Path,
) -> Result<Vec<PathBuf>, String> {
    let worker = [
        runtime_root
            .join("tts")
            .join(TtsEngine::PiperDmitriFp32.id())
            .join("bin")
            .join("piper-worker.exe"),
        runtime_root.join("bin").join("piper-worker.exe"),
    ]
    .into_iter()
    .find(|path| path.is_file())
    .ok_or("Piper worker не установлен")?;
    let runtime = PiperRuntime::expected(runtime_root);
    let model_dir = runtime
        .model_path
        .parent()
        .ok_or("Неверный путь модели Piper")?;
    if !runtime.model_path.is_file() {
        return Err(format!(
            "Модель Piper не установлена: {}",
            runtime.model_path.display()
        ));
    }

    let output_dir = work_dir.join("tts");
    fs::create_dir_all(&output_dir)
        .map_err(|error| format!("Не удалось создать каталог TTS: {error}"))?;
    let mut outputs = Vec::with_capacity(segments.len());
    for (position, segment) in segments.iter().enumerate() {
        let text_path = output_dir.join(format!("{position:05}.txt"));
        let audio_path = output_dir.join(format!("{position:05}.wav"));
        fs::write(&text_path, &segment.text)
            .map_err(|error| format!("Не удалось сохранить текст TTS: {error}"))?;
        run_checked(
            Command::new(&worker)
                .arg(model_dir)
                .arg(&text_path)
                .arg(&audio_path),
            "Piper не смог озвучить реплику",
        )?;
        outputs.push(audio_path);
        emit_tts_progress(progress, position + 1, segments.len());
    }
    Ok(outputs)
}

#[derive(Serialize)]
struct SileroInput<'a> {
    id: &'a str,
    text: &'a str,
}

#[derive(Deserialize)]
struct SileroOutput {
    audio_path: Option<String>,
    error: Option<String>,
}

fn synthesize_silero(
    progress: &ProgressReporter,
    runtime_root: &Path,
    segments: &[SubtitleSegment],
    work_dir: &Path,
) -> Result<Vec<PathBuf>, String> {
    let expected = SileroRuntime::expected(runtime_root);
    let (mut command, model) = if expected.worker_path.is_file() && expected.model_path.is_file() {
        (Command::new(expected.worker_path), expected.model_path)
    } else {
        let project = Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .expect("src-tauri находится в корне проекта");
        let python = project.join("env").join("Scripts").join("python.exe");
        let worker = project
            .join("_legacy_python")
            .join("workers")
            .join("silero_worker.py");
        let model = runtime_root.join("models").join("v5_5_ru.pt");
        if !python.is_file() || !worker.is_file() || !model.is_file() {
            return Err(
                "Silero runtime не установлен. Повторите установку компонента в приложении."
                    .to_owned(),
            );
        }
        let mut command = Command::new(python);
        command.arg(worker);
        (command, model)
    };

    let input_path = work_dir.join("silero-input.json");
    let output_dir = work_dir.join("tts");
    let input: Vec<_> = segments
        .iter()
        .map(|segment| SileroInput {
            id: &segment.id,
            text: &segment.text,
        })
        .collect();
    fs::write(
        &input_path,
        serde_json::to_vec(&input)
            .map_err(|error| format!("Не удалось подготовить Silero manifest: {error}"))?,
    )
    .map_err(|error| format!("Не удалось сохранить Silero manifest: {error}"))?;

    let output = run_checked(
        command
            .args(["--model"])
            .arg(model)
            .args(["--input"])
            .arg(input_path)
            .args(["--output-dir"])
            .arg(&output_dir)
            .args(["--speaker", "eugene", "--sample-rate", "48000"]),
        "Silero не смог озвучить реплики",
    )?;
    let results: Vec<SileroOutput> = serde_json::from_slice(&output.stdout)
        .map_err(|error| format!("Неверный ответ Silero worker: {error}"))?;
    if results.len() != segments.len() {
        return Err("Silero вернул неверное количество аудиофрагментов".to_owned());
    }
    let mut paths = Vec::with_capacity(results.len());
    for result in results {
        if let Some(error) = result.error {
            return Err(format!("Ошибка Silero: {error}"));
        }
        paths.push(PathBuf::from(
            result
                .audio_path
                .ok_or("Silero не вернул путь аудиофрагмента")?,
        ));
    }
    emit_tts_progress(progress, segments.len(), segments.len());
    Ok(paths)
}

fn assemble_voice_track(
    ffmpeg: &Path,
    segments: &[SubtitleSegment],
    fragments: &[PathBuf],
    duration_seconds: f64,
    work_dir: &Path,
    output_path: &Path,
) -> Result<(), String> {
    if fragments.len() != segments.len() {
        return Err("Количество TTS-фрагментов не совпадает с репликами".to_owned());
    }
    let spec = hound::WavSpec {
        channels: 1,
        sample_rate: TARGET_SAMPLE_RATE,
        bits_per_sample: 16,
        sample_format: hound::SampleFormat::Int,
    };
    let mut writer = hound::WavWriter::create(output_path, spec)
        .map_err(|error| format!("Не удалось создать голосовую дорожку: {error}"))?;
    let mut cursor = 0_u64;

    for (position, (segment, fragment)) in segments.iter().zip(fragments).enumerate() {
        let fitted = work_dir.join(format!("fitted-{position:05}.wav"));
        let source_duration = wav_duration_seconds(fragment)?;
        let slot_seconds = segment.duration_ms().max(1) as f64 / 1_000.0;
        let speed = (source_duration / slot_seconds).max(1.0);
        let filter = atempo_filter(speed);
        run_checked(
            Command::new(ffmpeg)
                .args(["-y", "-v", "error", "-i"])
                .arg(fragment)
                .args(["-filter:a", &filter, "-ar", "48000", "-ac", "1"])
                .args(["-c:a", "pcm_s16le"])
                .arg(&fitted),
            "Не удалось выровнять TTS-фрагмент",
        )?;

        let start_sample = segment.start_ms * TARGET_SAMPLE_RATE as u64 / 1_000;
        while cursor < start_sample {
            writer
                .write_sample(0_i16)
                .map_err(|error| format!("Ошибка записи тишины: {error}"))?;
            cursor += 1;
        }
        let max_samples = segment.duration_ms() * TARGET_SAMPLE_RATE as u64 / 1_000;
        let reader = hound::WavReader::open(&fitted)
            .map_err(|error| format!("Не удалось прочитать TTS WAV: {error}"))?;
        for sample in reader.into_samples::<i16>().take(max_samples as usize) {
            writer
                .write_sample(sample.map_err(|error| format!("Повреждён TTS WAV: {error}"))?)
                .map_err(|error| format!("Ошибка записи голоса: {error}"))?;
            cursor += 1;
        }
    }

    let total_samples = (duration_seconds.max(0.0) * TARGET_SAMPLE_RATE as f64).ceil() as u64;
    while cursor < total_samples {
        writer
            .write_sample(0_i16)
            .map_err(|error| format!("Ошибка записи финальной тишины: {error}"))?;
        cursor += 1;
    }
    writer
        .finalize()
        .map_err(|error| format!("Не удалось завершить голосовую дорожку: {error}"))
}

fn mix_audio(
    ffmpeg: &Path,
    input: &Path,
    audio_stream_index: usize,
    original_volume: f32,
    voice_track: &Path,
    output: &Path,
) -> Result<(), String> {
    let filter = build_mix_filter(&MixOptions {
        original_volume,
        input_audio_stream_index: audio_stream_index,
    });
    run_checked(
        Command::new(ffmpeg)
            .args(["-y", "-v", "error", "-i"])
            .arg(input)
            .args(["-i"])
            .arg(voice_track)
            .args(["-filter_complex", &filter, "-map", "[mix]"])
            .args(["-c:a", "aac", "-b:a", "192k"])
            .arg(output),
        "Не удалось смешать дубляж с оригиналом",
    )?;
    Ok(())
}

fn mux_output(
    ffmpeg: &Path,
    input: &Path,
    mixed_audio: &Path,
    output: &Path,
    existing_audio_streams: usize,
) -> Result<(), String> {
    let args = build_mux_args(&MuxOptions {
        input_path: input,
        dubbed_audio_path: mixed_audio,
        output_path: output,
        existing_audio_streams,
        make_default: false,
    });
    run_checked(
        Command::new(ffmpeg).args(["-y", "-v", "error"]).args(args),
        "Не удалось собрать итоговый MKV",
    )?;
    Ok(())
}

fn run_checked(command: &mut Command, context: &str) -> Result<Output, String> {
    let output = command
        .output()
        .map_err(|error| format!("{context}: {error}"))?;
    if output.status.success() {
        Ok(output)
    } else {
        let stderr = String::from_utf8_lossy(&output.stderr);
        let details = if stderr.trim().is_empty() {
            format!("процесс завершился с кодом {}", output.status)
        } else {
            stderr.trim().to_owned()
        };
        Err(format!("{context}: {details}"))
    }
}

fn resolve_tool(runtime_root: &Path, name: &str, fallback: &str) -> Result<PathBuf, String> {
    let bundled = runtime_root.join("bin").join(name);
    if bundled.is_file() {
        return Ok(bundled);
    }
    let fallback = PathBuf::from(fallback);
    if fallback.is_file() || fallback.components().count() == 1 {
        return Ok(fallback);
    }
    Err(format!("Не установлен обязательный компонент: {name}"))
}

fn wav_duration_seconds(path: &Path) -> Result<f64, String> {
    let reader = hound::WavReader::open(path)
        .map_err(|error| format!("Не удалось прочитать WAV {}: {error}", path.display()))?;
    Ok(reader.duration() as f64 / reader.spec().sample_rate as f64)
}

fn atempo_filter(speed: f64) -> String {
    let mut remaining = speed.clamp(1.0, 100.0);
    let mut filters = Vec::new();
    while remaining > 2.0 {
        filters.push("atempo=2.0".to_owned());
        remaining /= 2.0;
    }
    filters.push(format!("atempo={remaining:.6}"));
    filters.join(",")
}

fn output_path_for(input: &Path) -> PathBuf {
    let stem = input
        .file_stem()
        .and_then(|value| value.to_str())
        .unwrap_or("video");
    let preferred = input.with_file_name(format!("{stem}.dub.ru.mkv"));
    if !preferred.exists() {
        return preferred;
    }
    for copy_number in 2..10_000 {
        let candidate = input.with_file_name(format!("{stem}.dub.ru.{copy_number}.mkv"));
        if !candidate.exists() {
            return candidate;
        }
    }
    input.with_file_name(format!(
        "{stem}.dub.ru.{}.mkv",
        uuid::Uuid::new_v4().simple()
    ))
}

fn extract_json_object(content: &str) -> Result<&str, String> {
    let start = content.find('{').ok_or("Модель не вернула JSON-объект")?;
    let end = content
        .rfind('}')
        .ok_or("Модель вернула незавершённый JSON-объект")?;
    Ok(&content[start..=end])
}

fn is_russian(language: Option<&str>) -> bool {
    matches!(
        language.unwrap_or_default().trim().to_lowercase().as_str(),
        "ru" | "rus" | "russian"
    )
}

fn emit(progress: &ProgressReporter, percent: u8, stage: &'static str, message: &str) {
    progress.emit(JobProgress {
        percent,
        stage,
        message: message.to_owned(),
    });
}

fn emit_tts_progress(progress: &ProgressReporter, complete: usize, total: usize) {
    let percent = 42 + ((complete * 34) / total.max(1)) as u8;
    emit(
        progress,
        percent,
        "tts",
        &format!("Озвучено {complete} из {total} реплик"),
    );
}
