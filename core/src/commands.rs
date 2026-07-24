//! Построение аргументов для внешних медиапроцессов.

use std::path::Path;

/// Параметры запуска `whisper.cpp`.
pub struct WhisperOptions<'a> {
    pub model_path: &'a Path,
    pub audio_path: &'a Path,
    pub output_base: &'a Path,
    pub language: &'a str,
    pub vad_model_path: Option<&'a Path>,
}

/// Параметры worker-процесса Silero TTS.
pub struct SileroOptions<'a> {
    pub model_path: &'a Path,
    pub input_manifest: &'a Path,
    pub output_dir: &'a Path,
    pub speaker: &'a str,
    pub sample_rate: u32,
}

/// Параметры stereo-микса.
pub struct MixOptions {
    /// Глобальный индекс выбранного аудиопотока из `ffprobe`.
    pub input_audio_stream_index: usize,
    /// Усиление голосовой дорожки до целевой громкости.
    pub voice_gain_db: f32,
    /// Порог sidechain-компрессора в линейной шкале FFmpeg.
    pub ducking_threshold: f32,
    /// Степень приглушения оригинала во время речи.
    pub ducking_ratio: f32,
}

/// Параметры добавления дубляжа в Matroska.
pub struct MuxOptions<'a> {
    pub input_path: &'a Path,
    pub dubbed_audio_path: &'a Path,
    pub output_path: &'a Path,
    pub existing_audio_streams: usize,
    pub make_default: bool,
}

/// Построить аргументы CLI `whisper.cpp` для JSON-транскрипции с VAD и GPU.
pub fn build_whisper_args(options: &WhisperOptions<'_>) -> Vec<String> {
    let mut args = vec![
        "--model".into(),
        path_text(options.model_path),
        "--file".into(),
        path_text(options.audio_path),
        "--output-file".into(),
        path_text(options.output_base),
        "--language".into(),
        options.language.into(),
        "--output-json".into(),
        "--print-progress".into(),
        "--flash-attn".into(),
    ];

    if let Some(vad_model_path) = options.vad_model_path {
        args.extend([
            "--vad".into(),
            "--vad-model".into(),
            path_text(vad_model_path),
        ]);
    }

    args
}

/// Построить аргументы worker-процесса Silero `v5_5_ru`.
pub fn build_silero_args(options: &SileroOptions<'_>) -> Vec<String> {
    vec![
        "--model".into(),
        path_text(options.model_path),
        "--input".into(),
        path_text(options.input_manifest),
        "--output-dir".into(),
        path_text(options.output_dir),
        "--speaker".into(),
        options.speaker.into(),
        "--sample-rate".into(),
        options.sample_rate.to_string(),
    ]
}

/// Построить фильтр, сохраняющий stereo-фон и размещающий mono-голос по центру.
pub fn build_mix_filter(options: &MixOptions) -> String {
    format!(
        "[0:{}]aformat=channel_layouts=stereo[bed];\
         [1:a:0]aformat=channel_layouts=mono,volume={:.3}dB,\
         asplit=2[voice_sc][voice_pan];\
         [voice_pan]pan=stereo|c0=0.707*c0|c1=0.707*c0[voice];\
         [bed][voice_sc]sidechaincompress=threshold={:.6}:ratio={:.3}:\
         attack=20:release=350[ducked];\
         [ducked][voice]amix=inputs=2:duration=first:normalize=0,\
         alimiter=limit=0.850:level=disabled[mix]",
        options.input_audio_stream_index,
        options.voice_gain_db.clamp(-20.0, 20.0),
        options.ducking_threshold.clamp(0.000_975, 1.0),
        options.ducking_ratio.clamp(1.0, 20.0)
    )
}

/// Построить аргументы ffmpeg для сохранения исходных потоков и добавления дубляжа.
pub fn build_mux_args(options: &MuxOptions<'_>) -> Vec<String> {
    let audio_index = options.existing_audio_streams;
    let codec_key = format!("-c:a:{audio_index}");
    let bitrate_key = format!("-b:a:{audio_index}");
    let language_key = format!("-metadata:s:a:{audio_index}");
    let title_key = language_key.clone();
    let disposition_key = format!("-disposition:a:{audio_index}");

    let mut args = vec![
        "-i".into(),
        path_text(options.input_path),
        "-i".into(),
        path_text(options.dubbed_audio_path),
        "-map".into(),
        "0".into(),
        "-map".into(),
        "1:a:0".into(),
        "-map_metadata".into(),
        "0".into(),
        "-map_chapters".into(),
        "0".into(),
        "-c".into(),
        "copy".into(),
        codec_key,
        "aac".into(),
        bitrate_key,
        "192k".into(),
        language_key,
        "language=rus".into(),
        title_key,
        "title=Русский одноголосый дубляж".into(),
    ];
    if options.make_default {
        for index in 0..options.existing_audio_streams {
            args.extend([format!("-disposition:a:{index}"), "0".into()]);
        }
    }
    args.extend([
        disposition_key,
        if options.make_default { "default" } else { "0" }.into(),
        path_text(options.output_path),
    ]);
    args
}

fn path_text(path: &Path) -> String {
    path.to_string_lossy().into_owned().replace('\\', "/")
}
