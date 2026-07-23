use std::path::{Path, PathBuf};
use std::process::Command;

use serde::Serialize;
use simple_dub_core::media::{MediaInfo, parse_ffprobe_json};
use simple_dub_core::tts::{TtsBackend, TtsEngine, resolve_tts_runtime};
use tauri::Manager;

pub mod dubbing;
pub mod runtime;
pub mod settings;

use dubbing::{DubJobRequest, DubJobResult, ProgressReporter};
use settings::{OpenRouterCredentialStore, OpenRouterKeyStatus};

#[tauri::command]
fn inspect_media(app: tauri::AppHandle, path: String) -> Result<MediaInfo, String> {
    let input_path = PathBuf::from(&path);
    if !input_path.is_file() {
        return Err(format!("Файл не найден: {path}"));
    }

    let runtime_root = resolve_runtime_root(&app)?;
    runtime::ensure_sidecars(&app, &runtime_root)?;
    let output = Command::new(runtime_root.join("bin").join("ffprobe.exe"))
        .args([
            "-v",
            "error",
            "-show_streams",
            "-show_chapters",
            "-show_format",
            "-of",
            "json",
        ])
        .arg(&input_path)
        .output()
        .map_err(|error| format!("Не удалось запустить ffprobe: {error}"))?;

    if !output.status.success() {
        return Err(String::from_utf8_lossy(&output.stderr).trim().to_owned());
    }

    let json = String::from_utf8(output.stdout)
        .map_err(|error| format!("ffprobe вернул не UTF-8: {error}"))?;
    parse_ffprobe_json(&json).map_err(|error| error.to_string())
}

#[tauri::command]
fn openrouter_key_status() -> Result<OpenRouterKeyStatus, String> {
    OpenRouterCredentialStore::new()?.status()
}

#[tauri::command]
fn save_openrouter_key(key: String) -> Result<OpenRouterKeyStatus, String> {
    OpenRouterCredentialStore::new()?.save(&key)
}

#[tauri::command]
fn delete_openrouter_key() -> Result<OpenRouterKeyStatus, String> {
    OpenRouterCredentialStore::new()?.delete()
}

/// Доступность TTS-варианта в локальном runtime.
#[derive(Debug, Serialize)]
struct TtsEngineStatus {
    id: &'static str,
    display_name: &'static str,
    backend: TtsBackend,
    model_id: &'static str,
    speaker: &'static str,
    sample_rate: u32,
    installed: bool,
    status_message: String,
}

#[tauri::command]
fn tts_engine_statuses(app: tauri::AppHandle) -> Result<Vec<TtsEngineStatus>, String> {
    let runtime_root = resolve_runtime_root(&app)?;
    runtime::ensure_sidecars(&app, &runtime_root)?;
    Ok(TtsEngine::ALL
        .into_iter()
        .map(|engine| {
            let descriptor = engine.descriptor();
            let runtime = resolve_tts_runtime(&runtime_root, engine);
            let (installed, status_message) = match runtime {
                Ok(_) => (true, "Компонент установлен и готов.".to_owned()),
                Err(error) => (false, error.to_string()),
            };
            TtsEngineStatus {
                id: engine.id(),
                display_name: engine.display_name(),
                backend: descriptor.backend,
                model_id: descriptor.model_id,
                speaker: descriptor.speaker,
                sample_rate: descriptor.sample_rate,
                installed,
                status_message,
            }
        })
        .collect())
}

#[tauri::command]
async fn start_dub_job(
    app: tauri::AppHandle,
    request: DubJobRequest,
) -> Result<DubJobResult, String> {
    let runtime_root = resolve_runtime_root(&app)?;
    let worker_app = app.clone();
    tauri::async_runtime::spawn_blocking(move || {
        runtime::ensure_for_job(
            &worker_app,
            &runtime_root,
            request.tts_engine,
            request.subtitle_stream_index.is_none()
                || request.subtitle_kind.as_deref() != Some("text"),
        )?;
        let progress = ProgressReporter::tauri(worker_app);
        dubbing::run_dub_job(&progress, &runtime_root, &request)
    })
    .await
    .map_err(|error| format!("Worker дубляжа аварийно завершился: {error}"))?
}

fn resolve_runtime_root(app: &tauri::AppHandle) -> Result<PathBuf, String> {
    if let Some(path) = std::env::var_os("SIMPLE_DUB_RUNTIME") {
        return Ok(PathBuf::from(path));
    }

    if cfg!(debug_assertions) {
        return Ok(Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .expect("src-tauri находится внутри корня проекта")
            .join("runtime"));
    }

    app.path()
        .app_local_data_dir()
        .map(|path| path.join("runtime"))
        .map_err(|error| format!("Не удалось определить каталог runtime: {error}"))
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .invoke_handler(tauri::generate_handler![
            inspect_media,
            openrouter_key_status,
            save_openrouter_key,
            delete_openrouter_key,
            tts_engine_statuses,
            start_dub_job
        ])
        .run(tauri::generate_context!())
        .expect("ошибка запуска Simple Dub");
}
