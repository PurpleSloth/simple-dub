//! Исполняемый конвейер одноголосого дубляжа.

use std::fs;
use std::io::{BufRead, BufReader, Read};
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};
use std::thread;
use std::time::Duration;

use serde::{Deserialize, Serialize};
use simple_dub_core::commands::{MixOptions, MuxOptions, build_mix_filter, build_mux_args};
use simple_dub_core::subtitles::{SubtitleSegment, parse_srt, write_srt};
use simple_dub_core::tts::{PiperRuntime, SileroRuntime, TtsEngine};
use tauri::{AppHandle, Emitter};

use crate::settings::OpenRouterCredentialStore;

const TRANSLATION_MODEL: &str = "google/gemini-3.5-flash-lite";
const TARGET_SAMPLE_RATE: u32 = 48_000;
const TARGET_VOICE_LUFS: f32 = -16.0;
const DUCKING_RATIO: f32 = 20.0;

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
    pub ducking_gap_db: f32,
}

/// Итог успешно завершённого задания.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DubJobResult {
    pub output_path: String,
    pub segment_count: usize,
    pub skipped_segment_count: usize,
    pub voice_lufs: f32,
    pub original_lufs: f32,
    pub mix_lufs: f32,
    pub mix_true_peak_dbfs: f32,
    pub ducking_db: f32,
}

/// Событие прогресса для интерфейса.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct JobProgress {
    pub percent: u8,
    pub stage: &'static str,
    pub stage_index: u8,
    pub stage_count: u8,
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

#[derive(Clone)]
struct StageProgress {
    reporter: ProgressReporter,
    index: u8,
    count: u8,
    stage: &'static str,
}

impl StageProgress {
    fn new(reporter: &ProgressReporter, index: u8, count: u8, stage: &'static str) -> Self {
        Self {
            reporter: reporter.clone(),
            index,
            count,
            stage,
        }
    }

    fn emit(&self, percent: u8, message: &str) {
        self.reporter.emit(JobProgress {
            percent: percent.min(100),
            stage: self.stage,
            stage_index: self.index,
            stage_count: self.count,
            message: message.to_owned(),
        });
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

    let uses_text_subtitles =
        request.subtitle_stream_index.is_some() && request.subtitle_kind.as_deref() == Some("text");
    let needs_translation =
        !uses_text_subtitles || !is_russian(request.subtitle_language.as_deref());
    let stage_count = 5 + u8::from(!uses_text_subtitles) + u8::from(needs_translation);
    let mut stage_index = 1;

    let prepare_stage = StageProgress::new(progress, stage_index, stage_count, "prepare");
    stage_index += 1;
    let asr_stage = if uses_text_subtitles {
        None
    } else {
        let stage = StageProgress::new(progress, stage_index, stage_count, "asr");
        stage_index += 1;
        Some(stage)
    };
    let (mut segments, needs_translation) = obtain_segments(
        &prepare_stage,
        asr_stage.as_ref(),
        runtime_root,
        &ffmpeg,
        request,
        &work_dir,
    )?;
    if segments.is_empty() {
        return Err("Не найдено ни одной реплики для озвучки.".to_owned());
    }

    let mut skipped_segment_count = 0;
    if needs_translation {
        let translate_stage = StageProgress::new(progress, stage_index, stage_count, "translate");
        stage_index += 1;
        translate_stage.emit(0, "Перевод реплик на русский");
        let key = OpenRouterCredentialStore::new()?.read()?;
        let translation = translate_segments(&translate_stage, &key, &segments)?;
        segments = translation.segments;
        skipped_segment_count = translation.skipped_count;
    }
    fs::write(work_dir.join("russian.srt"), write_srt(&segments))
        .map_err(|error| format!("Не удалось сохранить русские субтитры: {error}"))?;

    let tts_stage = StageProgress::new(progress, stage_index, stage_count, "tts");
    stage_index += 1;
    tts_stage.emit(
        0,
        &format!("Озвучка через {}", request.tts_engine.display_name()),
    );
    let audio_fragments = synthesize_segments(
        &tts_stage,
        runtime_root,
        request.tts_engine,
        &segments,
        &work_dir,
    )?;

    let align_stage = StageProgress::new(progress, stage_index, stage_count, "align");
    stage_index += 1;
    align_stage.emit(0, "Выравнивание реплик по таймкодам");
    let voice_track = work_dir.join("voice.wav");
    assemble_voice_track(
        &align_stage,
        &ffmpeg,
        &segments,
        &audio_fragments,
        request.duration_seconds,
        &work_dir,
        &voice_track,
    )?;

    let balance_stage = StageProgress::new(progress, stage_index, stage_count, "balance");
    stage_index += 1;
    balance_stage.emit(0, "Анализ громкости оригинала");
    let original_loudness = measure_loudness(
        &balance_stage,
        &ffmpeg,
        &LoudnessJob {
            input: &input_path,
            stream_index: Some(request.audio_stream_index),
            duration_seconds: request.duration_seconds,
            message: "Анализ громкости оригинала",
            start_percent: 0,
            end_percent: 20,
        },
    )?;
    balance_stage.emit(
        20,
        &format!(
            "Оригинал {:.1} LUFS · анализ громкости дубляжа",
            original_loudness.integrated_lufs
        ),
    );
    let voice_loudness = measure_loudness(
        &balance_stage,
        &ffmpeg,
        &LoudnessJob {
            input: &voice_track,
            stream_index: None,
            duration_seconds: request.duration_seconds,
            message: "Анализ громкости дубляжа",
            start_percent: 20,
            end_percent: 35,
        },
    )?;
    let loudness_plan = plan_loudness_mix(
        original_loudness.integrated_lufs,
        voice_loudness.integrated_lufs,
        TARGET_VOICE_LUFS,
        request.ducking_gap_db,
    );
    balance_stage.emit(
        35,
        &format!(
            "Голос {:.1} → {:.1} LUFS · приглушение оригинала {:.1} dB",
            voice_loudness.integrated_lufs,
            loudness_plan.normalized_voice_lufs,
            loudness_plan.bed_reduction_db
        ),
    );
    let mixed_audio = work_dir.join("dubbed.mka");
    mix_audio(
        &balance_stage,
        &ffmpeg,
        &MixAudioJob {
            input: &input_path,
            audio_stream_index: request.audio_stream_index,
            loudness_plan: &loudness_plan,
            voice_track: &voice_track,
            output: &mixed_audio,
            duration_seconds: request.duration_seconds,
        },
    )?;
    balance_stage.emit(85, "Проверка итоговой громкости и пиков");
    let mix_loudness = measure_loudness(
        &balance_stage,
        &ffmpeg,
        &LoudnessJob {
            input: &mixed_audio,
            stream_index: None,
            duration_seconds: request.duration_seconds,
            message: "Проверка итоговой громкости и пиков",
            start_percent: 85,
            end_percent: 100,
        },
    )?;
    balance_stage.emit(
        100,
        &format!(
            "Микс {:.1} LUFS · пик {:.1} dBFS",
            mix_loudness.integrated_lufs, mix_loudness.true_peak_dbfs
        ),
    );

    let mux_stage = StageProgress::new(progress, stage_index, stage_count, "mux");
    mux_stage.emit(0, "Добавление новой дорожки в MKV");
    let output_path = output_path_for(&input_path);
    mux_output(
        &mux_stage,
        &ffmpeg,
        &input_path,
        &mixed_audio,
        &output_path,
        request.existing_audio_streams,
        request.duration_seconds,
    )?;

    let result = DubJobResult {
        output_path: output_path.to_string_lossy().into_owned(),
        segment_count: segments.len(),
        skipped_segment_count,
        voice_lufs: loudness_plan.normalized_voice_lufs,
        original_lufs: original_loudness.integrated_lufs,
        mix_lufs: mix_loudness.integrated_lufs,
        mix_true_peak_dbfs: mix_loudness.true_peak_dbfs,
        ducking_db: loudness_plan.bed_reduction_db,
    };
    fs::remove_dir_all(&work_dir).ok();
    Ok(result)
}

fn obtain_segments(
    prepare_stage: &StageProgress,
    asr_stage: Option<&StageProgress>,
    runtime_root: &Path,
    ffmpeg: &Path,
    request: &DubJobRequest,
    work_dir: &Path,
) -> Result<(Vec<SubtitleSegment>, bool), String> {
    let uses_text_subtitles =
        request.subtitle_stream_index.is_some() && request.subtitle_kind.as_deref() == Some("text");
    if uses_text_subtitles {
        prepare_stage.emit(0, "Извлечение выбранных субтитров");
        let subtitle_path = work_dir.join("source.srt");
        run_ffmpeg_with_progress(
            Command::new(ffmpeg)
                .args(["-y", "-v", "error", "-progress", "pipe:2", "-nostats", "-i"])
                .arg(&request.input_path)
                .args([
                    "-map",
                    &format!("0:{}", request.subtitle_stream_index.unwrap()),
                ])
                .arg(&subtitle_path),
            "Не удалось извлечь субтитры",
            prepare_stage,
            request.duration_seconds,
            "Извлечение выбранных субтитров",
            0,
            100,
        )?;
        let content = fs::read_to_string(&subtitle_path)
            .map_err(|error| format!("Не удалось прочитать SRT: {error}"))?;
        let segments = parse_srt(&content).map_err(|error| error.to_string())?;
        let needs_translation = !is_russian(request.subtitle_language.as_deref());
        return Ok((segments, needs_translation));
    }

    prepare_stage.emit(0, "Извлечение оригинальной аудиодорожки");
    let asr_audio = work_dir.join("asr.wav");
    run_ffmpeg_with_progress(
        Command::new(ffmpeg)
            .args(["-y", "-v", "error", "-progress", "pipe:2", "-nostats", "-i"])
            .arg(&request.input_path)
            .args(["-map", &format!("0:{}", request.audio_stream_index)])
            .args(["-ac", "1", "-ar", "16000", "-c:a", "pcm_s16le"])
            .arg(&asr_audio),
        "Не удалось подготовить аудио для Whisper",
        prepare_stage,
        request.duration_seconds,
        "Извлечение оригинальной аудиодорожки",
        0,
        100,
    )?;

    let asr_stage = asr_stage.ok_or("Не задан этап распознавания Whisper")?;
    asr_stage.emit(0, "Загрузка модели whisper.cpp");
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
    run_whisper_with_progress(&mut command, "Whisper не смог распознать речь", asr_stage)?;
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

struct TranslationBatchResult {
    segments: Vec<SubtitleSegment>,
    skipped_count: usize,
}

fn translate_segments(
    progress: &StageProgress,
    key: &str,
    segments: &[SubtitleSegment],
) -> Result<TranslationBatchResult, String> {
    let client = reqwest::blocking::Client::builder()
        .timeout(Duration::from_secs(180))
        .build()
        .map_err(|error| format!("Не удалось создать HTTP-клиент: {error}"))?;
    let mut translated = Vec::with_capacity(segments.len());
    let mut skipped_count = 0;
    let batches = segments.len().div_ceil(24);

    for (batch_index, batch) in segments.chunks(24).enumerate() {
        let batch_result = translate_with_fallback(batch, 3, Duration::from_millis(750), |items| {
            translate_batch_once(&client, key, items)
        });
        translated.extend(batch_result.segments);
        skipped_count += batch_result.skipped_count;
        let percent = (((batch_index + 1) * 100) / batches.max(1)) as u8;
        progress.emit(
            percent,
            &format!(
                "Переведено {} из {} реплик, пропущено {}",
                translated.len(),
                segments.len(),
                skipped_count
            ),
        );
    }
    Ok(TranslationBatchResult {
        segments: translated,
        skipped_count,
    })
}

fn translate_with_fallback<F>(
    batch: &[SubtitleSegment],
    max_attempts: usize,
    retry_delay: Duration,
    mut translate: F,
) -> TranslationBatchResult
where
    F: FnMut(&[SubtitleSegment]) -> Result<Vec<SubtitleSegment>, String>,
{
    if let Ok(segments) = retry_translation(batch, max_attempts, retry_delay, &mut translate) {
        return TranslationBatchResult {
            segments,
            skipped_count: 0,
        };
    }
    if batch.len() == 1 {
        return TranslationBatchResult {
            segments: Vec::new(),
            skipped_count: 1,
        };
    }

    let mut translated = Vec::with_capacity(batch.len());
    let mut skipped_count = 0;
    for segment in batch {
        match retry_translation(
            std::slice::from_ref(segment),
            max_attempts,
            retry_delay,
            &mut translate,
        ) {
            Ok(mut item) => translated.append(&mut item),
            Err(_) => skipped_count += 1,
        }
    }
    TranslationBatchResult {
        segments: translated,
        skipped_count,
    }
}

fn retry_translation<F>(
    batch: &[SubtitleSegment],
    max_attempts: usize,
    retry_delay: Duration,
    translate: &mut F,
) -> Result<Vec<SubtitleSegment>, String>
where
    F: FnMut(&[SubtitleSegment]) -> Result<Vec<SubtitleSegment>, String>,
{
    let attempts = max_attempts.max(1);
    let mut last_error = String::new();
    for attempt in 0..attempts {
        match translate(batch) {
            Ok(segments) => return Ok(segments),
            Err(error) => last_error = error,
        }
        if attempt + 1 < attempts && !retry_delay.is_zero() {
            std::thread::sleep(retry_delay);
        }
    }
    Err(last_error)
}

fn translate_batch_once(
    client: &reqwest::blocking::Client,
    key: &str,
    batch: &[SubtitleSegment],
) -> Result<Vec<SubtitleSegment>, String> {
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
         формулировку, но не добавляй новых фактов. Верни объект с массивом \
         segments и теми же id.\n\n{}",
        serde_json::to_string(&input)
            .map_err(|error| format!("Не удалось подготовить перевод: {error}"))?
    );
    let response = client
        .post("https://openrouter.ai/api/v1/chat/completions")
        .bearer_auth(key)
        .header("X-Title", "Simple Dub")
        .json(&translation_request_body(&prompt, batch.len()))
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

    batch
        .iter()
        .map(|original| {
            let item = envelope
                .segments
                .iter()
                .find(|item| item.id == original.id)
                .ok_or_else(|| format!("В переводе отсутствует id {}", original.id))?;
            let text = item.text.trim();
            if text.is_empty() {
                return Err(format!("Пустой перевод для id {}", original.id));
            }
            Ok(SubtitleSegment {
                id: original.id.clone(),
                start_ms: original.start_ms,
                end_ms: original.end_ms,
                text: text.to_owned(),
            })
        })
        .collect()
}

fn translation_request_body(prompt: &str, expected_segments: usize) -> serde_json::Value {
    serde_json::json!({
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
        "response_format": {
            "type": "json_schema",
            "json_schema": {
                "name": "subtitle_translation",
                "strict": true,
                "schema": {
                    "type": "object",
                    "properties": {
                        "segments": {
                            "type": "array",
                            "minItems": expected_segments,
                            "maxItems": expected_segments,
                            "items": {
                                "type": "object",
                                "properties": {
                                    "id": {"type": "string"},
                                    "text": {"type": "string", "minLength": 1}
                                },
                                "required": ["id", "text"],
                                "additionalProperties": false
                            }
                        }
                    },
                    "required": ["segments"],
                    "additionalProperties": false
                }
            }
        }
    })
}

fn synthesize_segments(
    progress: &StageProgress,
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
    progress: &StageProgress,
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
    progress: &StageProgress,
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
    progress: &StageProgress,
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
        progress.emit(
            (((position + 1) * 100) / segments.len().max(1)) as u8,
            &format!("Выровнено {} из {} реплик", position + 1, segments.len()),
        );
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

struct MixAudioJob<'a> {
    input: &'a Path,
    audio_stream_index: usize,
    loudness_plan: &'a LoudnessMixPlan,
    voice_track: &'a Path,
    output: &'a Path,
    duration_seconds: f64,
}

fn mix_audio(progress: &StageProgress, ffmpeg: &Path, job: &MixAudioJob<'_>) -> Result<(), String> {
    let filter = build_mix_filter(&MixOptions {
        input_audio_stream_index: job.audio_stream_index,
        voice_gain_db: job.loudness_plan.voice_gain_db,
        ducking_threshold: job.loudness_plan.sidechain_threshold,
        ducking_ratio: DUCKING_RATIO,
    });
    run_ffmpeg_with_progress(
        Command::new(ffmpeg)
            .args(["-y", "-v", "error", "-progress", "pipe:2", "-nostats", "-i"])
            .arg(job.input)
            .args(["-i"])
            .arg(job.voice_track)
            .args(["-filter_complex", &filter, "-map", "[mix]"])
            .args(["-c:a", "aac", "-b:a", "192k"])
            .arg(job.output),
        "Не удалось смешать дубляж с оригиналом",
        progress,
        job.duration_seconds,
        "Смешивание с автоматическим приглушением",
        35,
        80,
    )?;
    Ok(())
}

fn mux_output(
    progress: &StageProgress,
    ffmpeg: &Path,
    input: &Path,
    mixed_audio: &Path,
    output: &Path,
    existing_audio_streams: usize,
    duration_seconds: f64,
) -> Result<(), String> {
    let args = build_mux_args(&MuxOptions {
        input_path: input,
        dubbed_audio_path: mixed_audio,
        output_path: output,
        existing_audio_streams,
        make_default: true,
    });
    run_ffmpeg_with_progress(
        Command::new(ffmpeg)
            .args(["-y", "-v", "error", "-progress", "pipe:2", "-nostats"])
            .args(args),
        "Не удалось собрать итоговый MKV",
        progress,
        duration_seconds,
        "Сборка итогового MKV",
        0,
        100,
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

fn run_whisper_with_progress(
    command: &mut Command,
    context: &str,
    progress: &StageProgress,
) -> Result<Output, String> {
    run_command_with_progress(
        command,
        context,
        progress,
        "Распознавание речи",
        100,
        parse_whisper_progress,
    )
}

fn run_ffmpeg_with_progress(
    command: &mut Command,
    context: &str,
    progress: &StageProgress,
    duration_seconds: f64,
    message: &str,
    start_percent: u8,
    end_percent: u8,
) -> Result<Output, String> {
    run_command_with_progress(command, context, progress, message, end_percent, |line| {
        parse_ffmpeg_progress(line, duration_seconds)
            .map(|percent| scale_progress(percent, start_percent, end_percent))
    })
}

fn run_command_with_progress<F>(
    command: &mut Command,
    context: &str,
    progress: &StageProgress,
    message: &str,
    completion_percent: u8,
    mut parse_progress: F,
) -> Result<Output, String>
where
    F: FnMut(&str) -> Option<u8>,
{
    command.stdout(Stdio::piped()).stderr(Stdio::piped());
    let mut child = command
        .spawn()
        .map_err(|error| format!("{context}: {error}"))?;
    let mut stdout = child
        .stdout
        .take()
        .ok_or_else(|| format!("{context}: не удалось прочитать stdout"))?;
    let stdout_reader = thread::spawn(move || {
        let mut bytes = Vec::new();
        stdout.read_to_end(&mut bytes).ok();
        bytes
    });
    let stderr = child
        .stderr
        .take()
        .ok_or_else(|| format!("{context}: не удалось прочитать stderr"))?;
    let mut stderr_bytes = Vec::new();
    let mut last_percent = None;
    for line in BufReader::new(stderr).lines().map_while(Result::ok) {
        stderr_bytes.extend_from_slice(line.as_bytes());
        stderr_bytes.push(b'\n');
        if let Some(percent) = parse_progress(&line)
            && last_percent != Some(percent)
        {
            progress.emit(percent, message);
            last_percent = Some(percent);
        }
    }
    let status = child
        .wait()
        .map_err(|error| format!("{context}: {error}"))?;
    let stdout = stdout_reader.join().unwrap_or_default();
    let output = Output {
        status,
        stdout,
        stderr: stderr_bytes,
    };
    if output.status.success() {
        if last_percent != Some(completion_percent) {
            progress.emit(completion_percent, message);
        }
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

fn parse_whisper_progress(line: &str) -> Option<u8> {
    let marker = "progress =";
    let value = line.split_once(marker)?.1.trim();
    value
        .trim_end_matches('%')
        .trim()
        .parse::<u8>()
        .ok()
        .map(|percent| percent.min(100))
}

fn parse_ffmpeg_progress(line: &str, duration_seconds: f64) -> Option<u8> {
    if line.trim() == "progress=end" {
        return Some(100);
    }
    let value = line.trim().strip_prefix("out_time_us=")?;
    let elapsed_microseconds = value.parse::<f64>().ok()?;
    if duration_seconds <= 0.0 {
        return None;
    }
    Some(
        (elapsed_microseconds / (duration_seconds * 1_000_000.0) * 100.0)
            .round()
            .clamp(0.0, 99.0) as u8,
    )
}

fn scale_progress(percent: u8, start_percent: u8, end_percent: u8) -> u8 {
    let start = start_percent.min(100);
    let end = end_percent.clamp(start, 100);
    start + ((u16::from(percent.min(100)) * u16::from(end - start)) / 100) as u8
}

#[derive(Debug, Clone, Copy)]
struct LoudnessMeasurement {
    integrated_lufs: f32,
    true_peak_dbfs: f32,
}

#[derive(Debug, Clone, Copy)]
struct LoudnessMixPlan {
    voice_gain_db: f32,
    normalized_voice_lufs: f32,
    bed_reduction_db: f32,
    sidechain_threshold: f32,
}

struct LoudnessJob<'a> {
    input: &'a Path,
    stream_index: Option<usize>,
    duration_seconds: f64,
    message: &'static str,
    start_percent: u8,
    end_percent: u8,
}

fn measure_loudness(
    progress: &StageProgress,
    ffmpeg: &Path,
    job: &LoudnessJob<'_>,
) -> Result<LoudnessMeasurement, String> {
    let mut command = Command::new(ffmpeg);
    command
        .args(["-hide_banner", "-progress", "pipe:2", "-nostats", "-i"])
        .arg(job.input);
    if let Some(stream_index) = job.stream_index {
        command.args(["-map", &format!("0:{stream_index}")]);
    }
    let output = run_ffmpeg_with_progress(
        command.args([
            "-filter:a",
            "ebur128=peak=true:framelog=verbose",
            "-f",
            "null",
            "NUL",
        ]),
        "Не удалось измерить громкость",
        progress,
        job.duration_seconds,
        job.message,
        job.start_percent,
        job.end_percent,
    )?;
    parse_loudness_measurement(&String::from_utf8_lossy(&output.stderr))
}

fn parse_loudness_measurement(output: &str) -> Result<LoudnessMeasurement, String> {
    let mut integrated_lufs = None;
    let mut true_peak_dbfs = None;
    for line in output.lines().map(str::trim) {
        if let Some(value) = line
            .strip_prefix("I:")
            .and_then(|value| value.strip_suffix("LUFS"))
            .and_then(|value| value.trim().parse::<f32>().ok())
        {
            integrated_lufs = Some(value);
        }
        if let Some(value) = line
            .strip_prefix("Peak:")
            .and_then(|value| value.strip_suffix("dBFS"))
            .and_then(|value| value.trim().parse::<f32>().ok())
        {
            true_peak_dbfs = Some(value);
        }
    }
    Ok(LoudnessMeasurement {
        integrated_lufs: integrated_lufs.ok_or("FFmpeg не вернул интегральную громкость LUFS")?,
        true_peak_dbfs: true_peak_dbfs.ok_or("FFmpeg не вернул True Peak")?,
    })
}

fn plan_loudness_mix(
    original_lufs: f32,
    voice_lufs: f32,
    target_voice_lufs: f32,
    desired_gap_db: f32,
) -> LoudnessMixPlan {
    let voice_gain_db = (target_voice_lufs - voice_lufs).clamp(-20.0, 20.0);
    let normalized_voice_lufs = voice_lufs + voice_gain_db;
    let target_bed_lufs = normalized_voice_lufs - desired_gap_db.clamp(6.0, 24.0);
    let bed_reduction_db = (original_lufs - target_bed_lufs).clamp(0.0, 30.0);
    let compression_factor = 1.0 - 1.0 / DUCKING_RATIO;
    let threshold_db = normalized_voice_lufs - bed_reduction_db / compression_factor.max(0.01);
    let sidechain_threshold = if bed_reduction_db <= f32::EPSILON {
        1.0
    } else {
        10_f32.powf(threshold_db / 20.0).clamp(0.000_975, 1.0)
    };
    LoudnessMixPlan {
        voice_gain_db,
        normalized_voice_lufs,
        bed_reduction_db,
        sidechain_threshold,
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

fn emit_tts_progress(progress: &StageProgress, complete: usize, total: usize) {
    let percent = ((complete * 100) / total.max(1)) as u8;
    progress.emit(percent, &format!("Озвучено {complete} из {total} реплик"));
}

#[cfg(test)]
mod tests {
    use super::*;

    fn segment(id: &str, text: &str) -> SubtitleSegment {
        SubtitleSegment {
            id: id.to_owned(),
            start_ms: 0,
            end_ms: 1_000,
            text: text.to_owned(),
        }
    }

    #[test]
    fn translation_request_uses_strict_json_schema() {
        let body = translation_request_body("prompt", 2);
        let response_format = &body["response_format"];

        assert_eq!(response_format["type"], "json_schema");
        assert_eq!(response_format["json_schema"]["strict"], true);
        assert_eq!(
            response_format["json_schema"]["schema"]["additionalProperties"],
            false
        );
        assert_eq!(
            response_format["json_schema"]["schema"]["properties"]["segments"]["minItems"],
            2
        );
        assert_eq!(
            response_format["json_schema"]["schema"]["properties"]["segments"]["maxItems"],
            2
        );
    }

    #[test]
    fn translation_retries_a_failed_batch() {
        let input = vec![segment("1", "Hello")];
        let mut attempts = 0;

        let result = translate_with_fallback(&input, 3, Duration::ZERO, |batch| {
            attempts += 1;
            if attempts == 1 {
                return Err("invalid JSON".to_owned());
            }
            Ok(batch
                .iter()
                .map(|item| segment(&item.id, "Привет"))
                .collect())
        });

        assert_eq!(attempts, 2);
        assert_eq!(result.segments.len(), 1);
        assert_eq!(result.segments[0].text, "Привет");
        assert_eq!(result.skipped_count, 0);
    }

    #[test]
    fn translation_skips_only_the_item_that_fails_three_times() {
        let input = vec![segment("1", "Hello"), segment("2", "Goodbye")];
        let mut batch_attempts = 0;
        let mut second_item_attempts = 0;

        let result = translate_with_fallback(&input, 3, Duration::ZERO, |batch| {
            if batch.len() > 1 {
                batch_attempts += 1;
                return Err("invalid batch JSON".to_owned());
            }
            if batch[0].id == "2" {
                second_item_attempts += 1;
                return Err("invalid item JSON".to_owned());
            }
            Ok(vec![segment("1", "Привет")])
        });

        assert_eq!(batch_attempts, 3);
        assert_eq!(second_item_attempts, 3);
        assert_eq!(result.segments.len(), 1);
        assert_eq!(result.segments[0].id, "1");
        assert_eq!(result.skipped_count, 1);
    }

    #[test]
    fn whisper_progress_is_parsed_from_streamed_stderr() {
        assert_eq!(
            parse_whisper_progress("whisper_print_progress_callback: progress =  37%"),
            Some(37)
        );
        assert_eq!(
            parse_whisper_progress("whisper_print_progress_callback: progress = 100%"),
            Some(100)
        );
        assert_eq!(
            parse_whisper_progress("whisper_model_load: loading model"),
            None
        );
    }

    #[test]
    fn loudness_summary_returns_integrated_lufs_and_true_peak() {
        let output = r#"
            Integrated loudness:
              I:         -18.6 LUFS
              Threshold: -28.9 LUFS
            True peak:
              Peak:       -0.7 dBFS
        "#;

        let measurement = parse_loudness_measurement(output).unwrap();

        assert!((measurement.integrated_lufs + 18.6).abs() < 0.01);
        assert!((measurement.true_peak_dbfs + 0.7).abs() < 0.01);
    }

    #[test]
    fn auto_balance_targets_voice_and_ducks_original_by_requested_gap() {
        let plan = plan_loudness_mix(-16.0, -20.0, -16.0, 14.0);

        assert!((plan.voice_gain_db - 4.0).abs() < 0.01);
        assert!((plan.normalized_voice_lufs + 16.0).abs() < 0.01);
        assert!((plan.bed_reduction_db - 14.0).abs() < 0.01);
        assert!(plan.sidechain_threshold > 0.0);
        assert!(plan.sidechain_threshold < 1.0);
    }

    #[test]
    fn stage_progress_serializes_stage_position() {
        let progress = JobProgress {
            percent: 37,
            stage: "asr",
            stage_index: 2,
            stage_count: 7,
            message: "Распознавание речи".to_owned(),
        };
        let json = serde_json::to_value(progress).unwrap();

        assert_eq!(json["percent"], 37);
        assert_eq!(json["stageIndex"], 2);
        assert_eq!(json["stageCount"], 7);
    }

    #[test]
    fn subprocess_progress_stays_inside_its_stage_range() {
        assert_eq!(scale_progress(0, 35, 80), 35);
        assert_eq!(scale_progress(50, 35, 80), 57);
        assert_eq!(scale_progress(100, 35, 80), 80);
    }
}
