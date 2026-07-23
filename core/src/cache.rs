//! Детерминированная идентификация задания для безопасного кэша.

use serde::Serialize;
use sha2::{Digest, Sha256};

/// Семантические параметры, влияющие на результат дубляжа.
#[derive(Debug, Clone, Serialize)]
pub struct JobFingerprintInput<'a> {
    pub input_path: &'a str,
    pub input_size: u64,
    pub input_modified_unix_ms: u64,
    pub audio_stream_index: u32,
    pub subtitle_stream_index: Option<u32>,
    pub translation_model: &'a str,
    pub tts_model: &'a str,
    pub tts_speaker: &'a str,
    pub original_volume_milli: u16,
}

/// Вычислить стабильный SHA-256 ключ задания.
pub fn job_fingerprint(input: &JobFingerprintInput<'_>) -> String {
    let serialized =
        serde_json::to_vec(input).expect("структура ключа задания всегда сериализуема");
    let digest = Sha256::digest(serialized);
    digest.iter().map(|byte| format!("{byte:02x}")).collect()
}
