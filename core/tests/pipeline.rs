use std::path::Path;

use simple_dub_core::cache::{JobFingerprintInput, job_fingerprint};
use simple_dub_core::commands::{
    MixOptions, MuxOptions, SileroOptions, WhisperOptions, build_mix_filter, build_mux_args,
    build_silero_args, build_whisper_args,
};
use simple_dub_core::media::{SubtitleKind, parse_ffprobe_json};
use simple_dub_core::pipeline::{
    FitDecision, PipelineRoute, choose_pipeline_route, decide_duration_fit,
};

const PROBE_JSON: &str = r#"
{
  "streams": [
    {
      "index": 0,
      "codec_name": "h264",
      "codec_type": "video",
      "disposition": {"default": 1},
      "tags": {"language": "und", "title": "Video"}
    },
    {
      "index": 1,
      "codec_name": "aac",
      "codec_type": "audio",
      "channels": 2,
      "channel_layout": "stereo",
      "disposition": {"default": 1},
      "tags": {"language": "jpn", "title": "Original"}
    },
    {
      "index": 2,
      "codec_name": "ass",
      "codec_type": "subtitle",
      "disposition": {"default": 0},
      "tags": {"language": "rus", "title": "Russian"}
    },
    {
      "index": 3,
      "codec_name": "hdmv_pgs_subtitle",
      "codec_type": "subtitle",
      "disposition": {"default": 0},
      "tags": {"language": "eng", "title": "English PGS"}
    }
  ],
  "chapters": [{"id": 0, "start_time": "0.0", "end_time": "30.0"}],
  "format": {"duration": "120.5", "format_name": "matroska,webm"}
}
"#;

#[test]
fn parses_tracks_and_classifies_subtitles() {
    let media = parse_ffprobe_json(PROBE_JSON).expect("ffprobe JSON должен разбираться");

    assert_eq!(media.audio_streams().len(), 1);
    assert_eq!(media.audio_streams()[0].language.as_deref(), Some("jpn"));
    assert_eq!(media.audio_streams()[0].channels, Some(2));
    assert!(media.audio_streams()[0].is_default);

    let subtitles = media.subtitle_streams();
    assert_eq!(subtitles.len(), 2);
    assert_eq!(subtitles[0].subtitle_kind, Some(SubtitleKind::Text));
    assert_eq!(subtitles[1].subtitle_kind, Some(SubtitleKind::Image));
    assert_eq!(media.chapter_count, 1);
}

#[test]
fn chooses_pipeline_from_selected_subtitles() {
    let media = parse_ffprobe_json(PROBE_JSON).unwrap();
    let subtitles = media.subtitle_streams();

    assert_eq!(
        choose_pipeline_route(Some(subtitles[0]), "ru"),
        PipelineRoute::UseRussianSubtitles
    );
    assert_eq!(
        choose_pipeline_route(Some(subtitles[1]), "ru"),
        PipelineRoute::TranscribeAndTranslate
    );
    assert_eq!(
        choose_pipeline_route(None, "ru"),
        PipelineRoute::TranscribeAndTranslate
    );
}

#[test]
fn builds_whisper_cpp_command_for_multilingual_cuda_flow() {
    let args = build_whisper_args(&WhisperOptions {
        model_path: Path::new("models/ggml-large-v3-turbo.bin"),
        audio_path: Path::new("work/asr.wav"),
        output_base: Path::new("work/transcript"),
        language: "auto",
        vad_model_path: Some(Path::new("models/ggml-silero-v6.2.0.bin")),
    });

    assert!(
        args.windows(2)
            .any(|pair| pair == ["--model", "models/ggml-large-v3-turbo.bin"])
    );
    assert!(args.windows(2).any(|pair| pair == ["--language", "auto"]));
    assert!(args.iter().any(|arg| arg == "--output-json"));
    assert!(args.iter().any(|arg| arg == "--vad"));
    assert!(!args.iter().any(|arg| arg == "--no-gpu"));
}

#[test]
fn builds_silero_v5_5_worker_contract_without_qwen() {
    let args = build_silero_args(&SileroOptions {
        model_path: Path::new("models/v5_5_ru.pt"),
        input_manifest: Path::new("work/segments.json"),
        output_dir: Path::new("work/tts"),
        speaker: "aidar",
        sample_rate: 48_000,
    });

    assert!(
        args.windows(2)
            .any(|pair| pair == ["--model", "models/v5_5_ru.pt"])
    );
    assert!(args.windows(2).any(|pair| pair == ["--speaker", "aidar"]));
    assert!(
        args.windows(2)
            .any(|pair| pair == ["--sample-rate", "48000"])
    );
    assert!(!args.iter().any(|arg| arg.to_lowercase().contains("qwen")));
}

#[test]
fn duration_fit_uses_closed_loop_thresholds() {
    assert_eq!(decide_duration_fit(4_000, 4_300), FitDecision::Accept);
    assert_eq!(
        decide_duration_fit(4_000, 4_800),
        FitDecision::SpeedUp { factor: 1.2 }
    );

    match decide_duration_fit(4_000, 5_600) {
        FitDecision::ShortenTranslation { target_ratio } => {
            assert!((0.65..0.75).contains(&target_ratio));
        }
        other => panic!("ожидалось сокращение перевода, получено {other:?}"),
    }
}

#[test]
fn mix_keeps_stereo_bed_and_centers_mono_voice() {
    let filter = build_mix_filter(&MixOptions {
        original_volume: 0.3,
        input_audio_stream_index: 4,
    });

    assert!(filter.starts_with("[0:4]"));
    assert!(filter.contains("channel_layouts=stereo"));
    assert!(filter.contains("pan=stereo"));
    assert!(filter.contains("volume=0.300"));
    assert!(filter.contains("amix=inputs=2"));
    assert!(!filter.contains("ac=1"));
}

#[test]
fn mux_preserves_source_streams_and_adds_russian_dub() {
    let args = build_mux_args(&MuxOptions {
        input_path: Path::new("input.mkv"),
        dubbed_audio_path: Path::new("work/dubbed.mka"),
        output_path: Path::new("output.dub.ru.mkv"),
        existing_audio_streams: 2,
        make_default: false,
    });

    assert!(args.windows(2).any(|pair| pair == ["-map", "0"]));
    assert!(args.windows(2).any(|pair| pair == ["-map", "1:a:0"]));
    assert!(args.windows(2).any(|pair| pair == ["-c", "copy"]));
    assert!(
        args.windows(2)
            .any(|pair| pair == ["-metadata:s:a:2", "language=rus"])
    );
    assert!(
        args.windows(2)
            .any(|pair| pair == ["-metadata:s:a:2", "title=Русский одноголосый дубляж"])
    );
    assert_eq!(args.last().map(String::as_str), Some("output.dub.ru.mkv"));
}

#[test]
fn cache_key_changes_with_semantic_job_inputs() {
    let base = JobFingerprintInput {
        input_path: "show.mkv",
        input_size: 10_000,
        input_modified_unix_ms: 123,
        audio_stream_index: 1,
        subtitle_stream_index: Some(2),
        translation_model: "google/gemini-3.5-flash-lite",
        tts_engine: "piper-dmitri-fp32",
        tts_model: "v5_5_ru",
        tts_speaker: "aidar",
        original_volume_milli: 300,
    };

    let first = job_fingerprint(&base);
    let mut changed = base.clone();
    changed.tts_engine = "silero-v5-5-eugene";
    let second = job_fingerprint(&changed);

    assert_ne!(first, second);
    assert_eq!(first.len(), 64);
}
