use std::path::Path;

use simple_dub_core::tts::{
    PiperRuntime, SileroRuntime, TtsBackend, TtsEngine, TtsRuntimeError, resolve_tts_runtime,
};
use tempfile::tempdir;

#[test]
fn piper_dmitri_fp32_is_the_stable_default_engine() {
    let engine = TtsEngine::default();

    assert_eq!(engine, TtsEngine::PiperDmitriFp32);
    assert_eq!(engine.id(), "piper-dmitri-fp32");
    assert_eq!(
        serde_json::to_string(&engine).unwrap(),
        "\"piper-dmitri-fp32\""
    );
    assert_eq!(
        serde_json::to_string(&TtsEngine::SileroEugene).unwrap(),
        "\"silero-v5-5-eugene\""
    );
}

#[test]
fn engines_expose_expected_backend_model_and_voice() {
    let piper = TtsEngine::PiperDmitriFp32.descriptor();
    assert_eq!(piper.backend, TtsBackend::SherpaOnnx);
    assert_eq!(piper.model_id, "ru_RU-dmitri-medium");
    assert_eq!(piper.speaker, "dmitri");
    assert_eq!(piper.sample_rate, 22_050);

    let silero = TtsEngine::SileroEugene.descriptor();
    assert_eq!(silero.backend, TtsBackend::SileroStandalone);
    assert_eq!(silero.model_id, "v5_5_ru");
    assert_eq!(silero.speaker, "eugene");
    assert_eq!(silero.sample_rate, 48_000);
}

#[test]
fn resolves_engine_specific_runtime_without_cross_engine_fallback() {
    let runtime = tempdir().unwrap();
    let root = runtime.path();
    create_piper_runtime(root);

    let resolved = resolve_tts_runtime(root, TtsEngine::PiperDmitriFp32)
        .expect("готовый Piper runtime должен разрешаться");

    match resolved {
        simple_dub_core::tts::TtsRuntime::Piper(PiperRuntime {
            library_path,
            model_path,
            tokens_path,
            data_dir,
        }) => {
            assert!(library_path.ends_with("sherpa-onnx-c-api.dll"));
            assert!(model_path.ends_with("ru_RU-dmitri-medium.onnx"));
            assert!(tokens_path.ends_with("tokens.txt"));
            assert!(data_dir.ends_with("espeak-ng-data"));
        }
        other => panic!("ожидался Piper runtime, получен {other:?}"),
    }

    create_silero_runtime(root);
    let resolved = resolve_tts_runtime(root, TtsEngine::SileroEugene)
        .expect("готовый Silero runtime должен разрешаться");
    match resolved {
        simple_dub_core::tts::TtsRuntime::Silero(SileroRuntime {
            worker_path,
            model_path,
        }) => {
            assert!(worker_path.ends_with("silero-worker.exe"));
            assert!(model_path.ends_with("v5_5_ru.pt"));
        }
        other => panic!("ожидался Silero runtime, получен {other:?}"),
    }
}

#[test]
fn missing_silero_runtime_returns_actionable_error_instead_of_piper() {
    let runtime = tempdir().unwrap();
    create_piper_runtime(runtime.path());

    let error = resolve_tts_runtime(runtime.path(), TtsEngine::SileroEugene)
        .expect_err("отсутствующий Silero не должен заменяться на Piper");

    assert!(matches!(
        error,
        TtsRuntimeError::MissingFiles {
            engine: TtsEngine::SileroEugene,
            ..
        }
    ));
    assert!(error.to_string().contains("Silero 5.5 · Eugene"));
    assert!(error.to_string().contains("установ"));
}

fn create_piper_runtime(root: &Path) {
    let runtime = PiperRuntime::expected(root);
    touch(&runtime.library_path);
    touch(&runtime.model_path);
    touch(&runtime.tokens_path);
    std::fs::create_dir_all(&runtime.data_dir).unwrap();
}

fn create_silero_runtime(root: &Path) {
    let runtime = SileroRuntime::expected(root);
    touch(&runtime.worker_path);
    touch(&runtime.model_path);
}

fn touch(path: &Path) {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, []).unwrap();
}
