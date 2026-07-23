//! Описание поддерживаемых TTS-движков и их локального runtime.

use std::fmt;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use thiserror::Error;

/// TTS-движок, выбранный для конкретного задания.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum TtsEngine {
    /// Нативный Piper FP32 через `sherpa-onnx`.
    #[default]
    #[serde(rename = "piper-dmitri-fp32")]
    PiperDmitriFp32,
    /// Silero 5.5 в изолированном Python/PyTorch runtime.
    #[serde(rename = "silero-v5-5-eugene")]
    SileroEugene,
}

impl TtsEngine {
    /// Все варианты в порядке отображения в интерфейсе.
    pub const ALL: [Self; 2] = [Self::PiperDmitriFp32, Self::SileroEugene];

    /// Стабильный идентификатор для настроек, кэша и API.
    pub const fn id(self) -> &'static str {
        match self {
            Self::PiperDmitriFp32 => "piper-dmitri-fp32",
            Self::SileroEugene => "silero-v5-5-eugene",
        }
    }

    /// Пользовательское название движка и голоса.
    pub const fn display_name(self) -> &'static str {
        match self {
            Self::PiperDmitriFp32 => "Piper · Dmitri FP32",
            Self::SileroEugene => "Silero 5.5 · Eugene",
        }
    }

    /// Неизменяемые параметры синтеза для выбранного варианта.
    pub const fn descriptor(self) -> TtsEngineDescriptor {
        match self {
            Self::PiperDmitriFp32 => TtsEngineDescriptor {
                engine: self,
                backend: TtsBackend::SherpaOnnx,
                model_id: "ru_RU-dmitri-medium",
                speaker: "dmitri",
                sample_rate: 22_050,
            },
            Self::SileroEugene => TtsEngineDescriptor {
                engine: self,
                backend: TtsBackend::SileroPython,
                model_id: "v5_5_ru",
                speaker: "eugene",
                sample_rate: 48_000,
            },
        }
    }
}

impl fmt::Display for TtsEngine {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.display_name())
    }
}

/// Способ выполнения синтеза.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum TtsBackend {
    SherpaOnnx,
    SileroPython,
}

/// Метаданные TTS-варианта, не зависящие от локальных путей.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct TtsEngineDescriptor {
    pub engine: TtsEngine,
    pub backend: TtsBackend,
    pub model_id: &'static str,
    pub speaker: &'static str,
    pub sample_rate: u32,
}

/// Файлы нативного Piper runtime.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PiperRuntime {
    pub library_path: PathBuf,
    pub model_path: PathBuf,
    pub tokens_path: PathBuf,
    pub data_dir: PathBuf,
}

impl PiperRuntime {
    /// Ожидаемое размещение файлов относительно общего каталога runtime.
    pub fn expected(root: &Path) -> Self {
        let engine_root = root.join("tts").join(TtsEngine::PiperDmitriFp32.id());
        Self {
            library_path: engine_root.join("bin").join("sherpa-onnx-c-api.dll"),
            model_path: engine_root.join("model").join("ru_RU-dmitri-medium.onnx"),
            tokens_path: engine_root.join("model").join("tokens.txt"),
            data_dir: engine_root.join("model").join("espeak-ng-data"),
        }
    }

    fn missing_files(&self) -> Vec<PathBuf> {
        required_files([
            (&self.library_path, false),
            (&self.model_path, false),
            (&self.tokens_path, false),
            (&self.data_dir, true),
        ])
    }
}

/// Файлы изолированного Silero runtime.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SileroRuntime {
    pub python_path: PathBuf,
    pub worker_path: PathBuf,
    pub model_path: PathBuf,
}

impl SileroRuntime {
    /// Ожидаемое размещение файлов относительно общего каталога runtime.
    pub fn expected(root: &Path) -> Self {
        let engine_root = root.join("tts").join(TtsEngine::SileroEugene.id());
        Self {
            python_path: engine_root.join("python").join("python.exe"),
            worker_path: engine_root.join("worker").join("silero_worker.py"),
            model_path: engine_root.join("models").join("v5_5_ru.pt"),
        }
    }

    fn missing_files(&self) -> Vec<PathBuf> {
        required_files([
            (&self.python_path, false),
            (&self.worker_path, false),
            (&self.model_path, false),
        ])
    }
}

/// Проверенный runtime конкретного движка.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TtsRuntime {
    Piper(PiperRuntime),
    Silero(SileroRuntime),
}

/// Ошибка подготовки выбранного TTS-движка.
#[derive(Debug, Error)]
pub enum TtsRuntimeError {
    /// Выбранный runtime установлен не полностью.
    #[error(
        "TTS-движок «{engine}» не установлен: отсутствуют файлы {missing:?}. \
         Установите его в настройках приложения."
    )]
    MissingFiles {
        engine: TtsEngine,
        missing: Vec<PathBuf>,
    },
}

/// Проверить файлы строго выбранного движка, не подменяя его другим.
pub fn resolve_tts_runtime(root: &Path, engine: TtsEngine) -> Result<TtsRuntime, TtsRuntimeError> {
    match engine {
        TtsEngine::PiperDmitriFp32 => {
            let runtime = PiperRuntime::expected(root);
            ensure_present(engine, runtime.missing_files())?;
            Ok(TtsRuntime::Piper(runtime))
        }
        TtsEngine::SileroEugene => {
            let runtime = SileroRuntime::expected(root);
            ensure_present(engine, runtime.missing_files())?;
            Ok(TtsRuntime::Silero(runtime))
        }
    }
}

fn ensure_present(engine: TtsEngine, missing: Vec<PathBuf>) -> Result<(), TtsRuntimeError> {
    if missing.is_empty() {
        Ok(())
    } else {
        Err(TtsRuntimeError::MissingFiles { engine, missing })
    }
}

fn required_files<'a>(paths: impl IntoIterator<Item = (&'a PathBuf, bool)>) -> Vec<PathBuf> {
    paths
        .into_iter()
        .filter_map(|(path, is_directory)| {
            let exists = if is_directory {
                path.is_dir()
            } else {
                path.is_file()
            };
            (!exists).then(|| path.clone())
        })
        .collect()
}
