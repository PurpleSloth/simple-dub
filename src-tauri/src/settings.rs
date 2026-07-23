//! Безопасное хранение пользовательского ключа OpenRouter.

use keyring::{Entry, Error as KeyringError};
use serde::Serialize;

const SERVICE_NAME: &str = "Simple Dub";
const ACCOUNT_NAME: &str = "openrouter-api-key";

/// Публичный статус настройки. Сам ключ намеренно не сериализуется.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct OpenRouterKeyStatus {
    pub configured: bool,
}

/// Хранилище ключа OpenRouter в системном менеджере учётных данных.
pub struct OpenRouterCredentialStore {
    entry: Entry,
}

impl OpenRouterCredentialStore {
    /// Создать хранилище приложения.
    pub fn new() -> Result<Self, String> {
        Self::with_service(SERVICE_NAME)
    }

    /// Создать хранилище с отдельным service name.
    ///
    /// Метод публичен, чтобы интеграционный тест не затрагивал рабочую запись.
    pub fn with_service(service: &str) -> Result<Self, String> {
        let entry = Entry::new(service, ACCOUNT_NAME)
            .map_err(|error| credential_error("Не удалось открыть хранилище ключей", error))?;
        Ok(Self { entry })
    }

    /// Сохранить проверенный ключ в системном хранилище.
    pub fn save(&self, key: &str) -> Result<OpenRouterKeyStatus, String> {
        let normalized = validate_openrouter_key(key)?;
        self.entry
            .set_password(&normalized)
            .map_err(|error| credential_error("Не удалось сохранить ключ", error))?;
        Ok(OpenRouterKeyStatus { configured: true })
    }

    /// Узнать, настроен ли ключ, не возвращая его значение.
    pub fn status(&self) -> Result<OpenRouterKeyStatus, String> {
        match self.entry.get_password() {
            Ok(key) => Ok(OpenRouterKeyStatus {
                configured: !key.trim().is_empty(),
            }),
            Err(KeyringError::NoEntry) => Ok(OpenRouterKeyStatus { configured: false }),
            Err(error) => Err(credential_error(
                "Не удалось проверить сохранённый ключ",
                error,
            )),
        }
    }

    /// Получить ключ только для внутреннего API-запроса.
    ///
    /// Значение не сериализуется и не передаётся интерфейсу.
    pub fn read(&self) -> Result<String, String> {
        self.entry
            .get_password()
            .map_err(|error| credential_error("Не удалось прочитать сохранённый ключ", error))
            .and_then(|key| validate_openrouter_key(&key))
    }

    /// Удалить ключ. Отсутствующая запись считается успешным результатом.
    pub fn delete(&self) -> Result<OpenRouterKeyStatus, String> {
        match self.entry.delete_credential() {
            Ok(()) | Err(KeyringError::NoEntry) => Ok(OpenRouterKeyStatus { configured: false }),
            Err(error) => Err(credential_error("Не удалось удалить ключ", error)),
        }
    }
}

/// Проверить формат и удалить случайные пробелы по краям.
pub fn validate_openrouter_key(key: &str) -> Result<String, String> {
    let normalized = key.trim();
    let has_valid_prefix = normalized.starts_with("sk-or-");
    let has_minimum_length = normalized.len() >= 24;
    let has_only_safe_characters = normalized
        .chars()
        .all(|character| character.is_ascii_alphanumeric() || matches!(character, '-' | '_'));

    if !has_valid_prefix || !has_minimum_length || !has_only_safe_characters {
        return Err("Введите корректный ключ OpenRouter вида sk-or-…".to_owned());
    }

    Ok(normalized.to_owned())
}

fn credential_error(context: &str, error: KeyringError) -> String {
    format!("{context}: {error}")
}
