use std::path::{Path, PathBuf};
use std::process::Command;

use simple_dub_core::media::{MediaInfo, parse_ffprobe_json};

pub mod settings;

use settings::{OpenRouterCredentialStore, OpenRouterKeyStatus};

#[tauri::command]
fn inspect_media(path: String) -> Result<MediaInfo, String> {
    let input_path = PathBuf::from(&path);
    if !input_path.is_file() {
        return Err(format!("Файл не найден: {path}"));
    }

    let output = Command::new(resolve_ffprobe())
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

fn resolve_ffprobe() -> PathBuf {
    if let Some(path) = std::env::var_os("SIMPLE_DUB_FFPROBE") {
        return PathBuf::from(path);
    }

    let bundled_development_path = Path::new(r"C:\ffmpeg\bin\ffprobe.exe");
    if bundled_development_path.is_file() {
        return bundled_development_path.to_path_buf();
    }

    PathBuf::from("ffprobe")
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .invoke_handler(tauri::generate_handler![
            inspect_media,
            openrouter_key_status,
            save_openrouter_key,
            delete_openrouter_key
        ])
        .run(tauri::generate_context!())
        .expect("ошибка запуска Simple Dub");
}
