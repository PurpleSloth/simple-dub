//! Подготовка приватного runtime приложения без ручной настройки системы.

use std::fs;
use std::io::{BufRead, BufReader, Read};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::thread;

use simple_dub_core::tts::{TtsEngine, resolve_tts_runtime};
use tauri::{AppHandle, Emitter};

use crate::dubbing::JobProgress;

const SETUP_SCRIPT: &str = include_str!("../../scripts/setup-runtime.ps1");

/// Скопировать поставляемые sidecar-инструменты в приватный runtime.
pub fn ensure_sidecars(app: &AppHandle, runtime_root: &Path) -> Result<(), String> {
    let bin_dir = runtime_root.join("bin");
    fs::create_dir_all(&bin_dir)
        .map_err(|error| format!("Не удалось создать каталог runtime: {error}"))?;
    copy_sidecar(app, "ffmpeg", &bin_dir.join("ffmpeg.exe"))?;
    copy_sidecar(app, "ffprobe", &bin_dir.join("ffprobe.exe"))?;
    copy_sidecar(app, "piper-worker", &bin_dir.join("piper-worker.exe"))?;
    Ok(())
}

/// Установить только компоненты, необходимые выбранному маршруту.
pub fn ensure_for_job(
    app: &AppHandle,
    runtime_root: &Path,
    engine: TtsEngine,
    needs_whisper: bool,
) -> Result<(), String> {
    ensure_sidecars(app, runtime_root)?;
    let install_piper =
        engine == TtsEngine::PiperDmitriFp32 && resolve_tts_runtime(runtime_root, engine).is_err();
    let install_silero =
        engine == TtsEngine::SileroEugene && resolve_tts_runtime(runtime_root, engine).is_err();
    let install_whisper = needs_whisper
        && (!runtime_root.join("bin").join("whisper-cli.exe").is_file()
            || !runtime_root
                .join("models")
                .join("ggml-large-v3-turbo.bin")
                .is_file());
    if !install_piper && !install_silero && !install_whisper {
        return Ok(());
    }

    app.emit(
        "job-progress",
        JobProgress {
            percent: 1,
            stage: "runtime",
            message: "Установка недостающих компонентов…".to_owned(),
        },
    )
    .ok();

    let script_path = runtime_root.join("install-runtime.ps1");
    fs::write(&script_path, SETUP_SCRIPT)
        .map_err(|error| format!("Не удалось подготовить установщик runtime: {error}"))?;
    let mut command = Command::new("powershell.exe");
    command
        .args(["-NoProfile", "-ExecutionPolicy", "Bypass", "-File"])
        .arg(&script_path)
        .arg("-RuntimeRoot")
        .arg(runtime_root)
        .arg("-PiperWorkerSource")
        .arg(runtime_root.join("bin").join("piper-worker.exe"));
    if install_piper {
        command.arg("-InstallPiper");
    }
    if install_whisper {
        command.arg("-InstallWhisper");
    }
    if install_silero {
        command.arg("-InstallSilero");
    }
    command.stdout(Stdio::piped()).stderr(Stdio::piped());
    let mut child = command
        .spawn()
        .map_err(|error| format!("Не удалось запустить установщик компонентов: {error}"))?;
    let stderr = child
        .stderr
        .take()
        .ok_or("Не удалось прочитать stderr установщика")?;
    let stderr_reader = thread::spawn(move || {
        let mut text = String::new();
        BufReader::new(stderr).read_to_string(&mut text).ok();
        text
    });
    let stdout = child
        .stdout
        .take()
        .ok_or("Не удалось прочитать stdout установщика")?;
    let mut setup_output = Vec::new();
    for line in BufReader::new(stdout).lines().map_while(Result::ok) {
        if let Some((percent, message)) = parse_progress(&line) {
            app.emit(
                "job-progress",
                JobProgress {
                    percent: 1 + percent / 10,
                    stage: "runtime",
                    message,
                },
            )
            .ok();
        } else if !line.trim().is_empty() {
            setup_output.push(line);
        }
    }
    let status = child
        .wait()
        .map_err(|error| format!("Не удалось дождаться установки компонентов: {error}"))?;
    let stderr = stderr_reader.join().unwrap_or_default();
    if !status.success() {
        let details = [setup_output.join("\n"), stderr.trim().to_owned()]
            .into_iter()
            .filter(|value| !value.is_empty())
            .collect::<Vec<_>>()
            .join("\n");
        return Err(format!(
            "Установка компонентов завершилась с ошибкой: {}",
            if details.is_empty() {
                status.to_string()
            } else {
                details
            }
        ));
    }
    resolve_tts_runtime(runtime_root, engine).map_err(|error| error.to_string())?;
    if install_whisper
        && !runtime_root
            .join("models")
            .join("ggml-large-v3-turbo.bin")
            .is_file()
    {
        return Err("После установки не найдена модель Whisper".to_owned());
    }
    Ok(())
}

fn copy_sidecar(app: &AppHandle, name: &str, destination: &Path) -> Result<(), String> {
    if destination.is_file() {
        return Ok(());
    }
    let source = sidecar_candidates(app, name)
        .into_iter()
        .find(|path| path.is_file())
        .ok_or_else(|| format!("В сборке отсутствует обязательный компонент: {name}.exe"))?;
    fs::copy(&source, destination).map_err(|error| {
        format!(
            "Не удалось подготовить {} из {}: {error}",
            destination.display(),
            source.display()
        )
    })?;
    Ok(())
}

fn sidecar_candidates(_app: &AppHandle, name: &str) -> Vec<PathBuf> {
    let manifest = Path::new(env!("CARGO_MANIFEST_DIR"));
    let project = manifest.parent().expect("src-tauri находится в корне");
    let mut candidates = Vec::new();
    if let Ok(executable) = std::env::current_exe()
        && let Some(directory) = executable.parent()
    {
        candidates.push(directory.join(format!("{name}.exe")));
    }
    candidates.push(
        manifest
            .join("bin")
            .join(format!("{name}-x86_64-pc-windows-msvc.exe")),
    );
    match name {
        "ffmpeg" => candidates.push(
            project
                .join("node_modules")
                .join("ffmpeg-static")
                .join("ffmpeg.exe"),
        ),
        "ffprobe" => candidates.push(
            project
                .join("node_modules")
                .join("ffprobe-static")
                .join("bin")
                .join("win32")
                .join("x64")
                .join("ffprobe.exe"),
        ),
        _ => {}
    }
    candidates
}

fn parse_progress(line: &str) -> Option<(u8, String)> {
    let payload = line.strip_prefix("[progress]")?;
    let (percent, message) = payload.split_once('|')?;
    Some((percent.parse().ok()?, message.trim().to_owned()))
}

#[cfg(test)]
mod tests {
    use super::parse_progress;

    #[test]
    fn parses_installer_progress() {
        assert_eq!(
            parse_progress("[progress]50|Загрузка модели"),
            Some((50, "Загрузка модели".to_owned()))
        );
        assert_eq!(parse_progress("обычный вывод"), None);
    }
}
