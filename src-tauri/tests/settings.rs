use simple_dub_desktop_lib::settings::{
    OpenRouterCredentialStore, OpenRouterKeyStatus, validate_openrouter_key,
};

#[test]
fn validates_and_normalizes_openrouter_key_without_exposing_it_in_status() {
    let key = ["sk", "or", "v1", "test-only-value-with-enough-length"].join("-");
    let normalized =
        validate_openrouter_key(&format!("  {key}  ")).expect("ключ должен пройти проверку");

    assert_eq!(normalized, key);

    let serialized = serde_json::to_value(OpenRouterKeyStatus { configured: true })
        .expect("статус должен сериализоваться");
    assert_eq!(serialized, serde_json::json!({ "configured": true }));
    assert!(!serialized.to_string().contains(&key));

    assert!(validate_openrouter_key("   ").is_err());
    assert!(validate_openrouter_key("not-an-openrouter-key").is_err());
}

#[cfg(target_os = "windows")]
#[test]
fn stores_status_and_deletes_key_in_windows_credential_manager() {
    let service = format!("simple-dub-test-{}", std::process::id());
    let store = OpenRouterCredentialStore::with_service(&service)
        .expect("должна создаваться тестовая запись Credential Manager");
    let _cleanup = CredentialCleanup(&store);
    let key = ["sk", "or", "v1", "temporary-credential-manager-test"].join("-");

    store.delete().ok();
    assert_eq!(
        store.status().unwrap(),
        OpenRouterKeyStatus { configured: false }
    );

    store.save(&key).expect("ключ должен сохраняться");
    assert_eq!(
        store.status().unwrap(),
        OpenRouterKeyStatus { configured: true }
    );

    store.delete().expect("ключ должен удаляться");
    assert_eq!(
        store.status().unwrap(),
        OpenRouterKeyStatus { configured: false }
    );
}

#[cfg(target_os = "windows")]
struct CredentialCleanup<'a>(&'a OpenRouterCredentialStore);

#[cfg(target_os = "windows")]
impl Drop for CredentialCleanup<'_> {
    fn drop(&mut self) {
        self.0.delete().ok();
    }
}
