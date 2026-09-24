//! Safe adapter for Qoder CLI's native provider registry.
//!
//! Qoder stores user preferences and custom providers in one
//! `~/.qoder/settings.json` document. CC Switch owns only entries under the
//! top-level `providers` object and preserves every other setting verbatim.

use crate::config::{atomic_write_private, get_home_dir};
use crate::error::AppError;
use indexmap::IndexMap;
use serde_json::{Map, Value};
use sha2::{Digest, Sha256};
use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::{LazyLock, Mutex, MutexGuard};

const MAX_QODER_SETTINGS_BYTES: u64 = 1024 * 1024;
const MISSING_SETTINGS_REVISION: &str = "missing";
static SETTINGS_FILE_LOCK: LazyLock<Mutex<()>> = LazyLock::new(|| Mutex::new(()));
#[cfg(test)]
static TEST_QODER_DIR: LazyLock<Mutex<Option<PathBuf>>> = LazyLock::new(|| Mutex::new(None));

pub(crate) fn get_qoder_dir() -> PathBuf {
    #[cfg(test)]
    if let Some(path) = TEST_QODER_DIR
        .lock()
        .expect("lock Qoder test directory")
        .clone()
    {
        return path;
    }

    get_home_dir().join(".qoder")
}

pub(crate) fn get_qoder_settings_path() -> PathBuf {
    get_qoder_dir().join("settings.json")
}

pub(crate) fn read_qoder_native_providers() -> Result<IndexMap<String, Value>, AppError> {
    let _guard = lock_settings_file()?;
    let path = get_qoder_settings_path();
    let document = read_settings_document(&path)?;
    Ok(providers(&document, &path)?
        .iter()
        .map(|(provider_key, config)| (provider_key.clone(), config.clone()))
        .collect())
}

pub(crate) fn read_qoder_native_provider(provider_key: &str) -> Result<Option<Value>, AppError> {
    let _guard = lock_settings_file()?;
    let path = get_qoder_settings_path();
    let document = read_settings_document(&path)?;
    Ok(providers(&document, &path)?.get(provider_key).cloned())
}

pub(crate) fn qoder_provider_exists(provider_key: &str) -> Result<bool, AppError> {
    Ok(read_qoder_native_provider(provider_key)?.is_some())
}

pub(crate) fn insert_qoder_provider(provider_key: &str, config: &Value) -> Result<bool, AppError> {
    validate_provider_node(provider_key, config)?;
    let _guard = lock_settings_file()?;
    let path = get_qoder_settings_path();
    let (mut document, expected_revision) = read_settings_document_with_revision(&path)?;
    let providers = providers_mut(&mut document, &path)?;

    match providers.get(provider_key) {
        Some(current) if current == config => return Ok(false),
        Some(_) => {
            return Err(AppError::InvalidInput(format!(
                "Qoder provider key '{provider_key}' already exists in settings.json"
            )))
        }
        None => {}
    }

    providers.insert(provider_key.to_string(), config.clone());
    write_settings_document(&path, &document, &expected_revision)?;
    Ok(true)
}

pub(crate) fn replace_qoder_provider(
    provider_key: &str,
    expected: &Value,
    replacement: &Value,
) -> Result<(), AppError> {
    validate_provider_node(provider_key, replacement)?;
    let _guard = lock_settings_file()?;
    let path = get_qoder_settings_path();
    let (mut document, expected_revision) = read_settings_document_with_revision(&path)?;
    let providers = providers_mut(&mut document, &path)?;
    let current = providers.get(provider_key).ok_or_else(|| {
        AppError::Conflict(format!(
            "Qoder provider '{provider_key}' is no longer present in settings.json"
        ))
    })?;
    if current != expected {
        return Err(AppError::Conflict(format!(
            "Qoder provider '{provider_key}' changed outside CC Switch"
        )));
    }
    if current == replacement {
        return Ok(());
    }
    providers.insert(provider_key.to_string(), replacement.clone());
    write_settings_document(&path, &document, &expected_revision)
}

pub(crate) fn replace_qoder_provider_if_present(
    provider_key: &str,
    replacement: &Value,
) -> Result<Option<Value>, AppError> {
    validate_provider_node(provider_key, replacement)?;
    let _guard = lock_settings_file()?;
    let path = get_qoder_settings_path();
    let (mut document, expected_revision) = read_settings_document_with_revision(&path)?;
    let providers = providers_mut(&mut document, &path)?;
    let Some(current) = providers.get(provider_key).cloned() else {
        return Ok(None);
    };
    if current == *replacement {
        return Ok(Some(current));
    }
    providers.insert(provider_key.to_string(), replacement.clone());
    write_settings_document(&path, &document, &expected_revision)?;
    Ok(Some(current))
}

pub(crate) fn remove_qoder_provider(provider_key: &str) -> Result<Option<Value>, AppError> {
    remove_qoder_provider_inner(provider_key, None)
}

pub(crate) fn remove_qoder_provider_if_matches(
    provider_key: &str,
    expected: &Value,
) -> Result<bool, AppError> {
    remove_qoder_provider_inner(provider_key, Some(expected)).map(|removed| removed.is_some())
}

fn remove_qoder_provider_inner(
    provider_key: &str,
    expected: Option<&Value>,
) -> Result<Option<Value>, AppError> {
    let _guard = lock_settings_file()?;
    let path = get_qoder_settings_path();
    let (mut document, expected_revision) = read_settings_document_with_revision(&path)?;
    let providers = providers_mut(&mut document, &path)?;
    let Some(current) = providers.get(provider_key).cloned() else {
        return Ok(None);
    };
    if expected.is_some_and(|expected| current != *expected) {
        return Err(AppError::Conflict(format!(
            "Qoder provider '{provider_key}' changed outside CC Switch"
        )));
    }
    providers.remove(provider_key);
    write_settings_document(&path, &document, &expected_revision)?;
    Ok(Some(current))
}

pub(crate) fn restore_qoder_provider_if_missing(
    provider_key: &str,
    config: &Value,
) -> Result<(), AppError> {
    let _guard = lock_settings_file()?;
    let path = get_qoder_settings_path();
    let (mut document, expected_revision) = read_settings_document_with_revision(&path)?;
    let providers = providers_mut(&mut document, &path)?;
    match providers.get(provider_key) {
        Some(current) if current == config => Ok(()),
        Some(_) => Err(AppError::Conflict(format!(
            "cannot restore Qoder provider '{provider_key}' because another value now owns the key"
        ))),
        None => {
            providers.insert(provider_key.to_string(), config.clone());
            write_settings_document(&path, &document, &expected_revision)
        }
    }
}

pub(crate) fn validate_provider_node(provider_key: &str, config: &Value) -> Result<(), AppError> {
    if provider_key.trim().is_empty() {
        return Err(AppError::InvalidInput(
            "Qoder provider key cannot be empty".to_string(),
        ));
    }
    if !provider_key.chars().all(|character| {
        character.is_ascii_lowercase() || character.is_ascii_digit() || character == '-'
    }) || provider_key.starts_with('-')
        || provider_key.ends_with('-')
    {
        return Err(AppError::InvalidInput(
            "Qoder provider key must use lowercase letters, numbers, and single hyphen separators"
                .to_string(),
        ));
    }

    let object = config.as_object().ok_or_else(|| {
        AppError::InvalidInput("Qoder provider configuration must be an object".to_string())
    })?;
    let protocol = required_string(object, "protocol")?;
    if !matches!(protocol, "openai" | "openai-responses" | "anthropic") {
        return Err(AppError::InvalidInput(format!(
            "Qoder provider protocol '{protocol}' is not supported"
        )));
    }
    required_string(object, "baseUrl")?;
    required_string(object, "model")?;
    let models = object
        .get("models")
        .and_then(Value::as_array)
        .ok_or_else(|| {
            AppError::InvalidInput("Qoder provider models must be an array".to_string())
        })?;
    if models.is_empty() {
        return Err(AppError::InvalidInput(
            "Qoder provider must contain at least one model".to_string(),
        ));
    }
    for model in models {
        let model = model.as_object().ok_or_else(|| {
            AppError::InvalidInput("Each Qoder model must be an object".to_string())
        })?;
        required_string(model, "model")?;
    }
    Ok(())
}

pub(crate) fn provider_base_url(config: &Value) -> Result<String, AppError> {
    let provider = config.as_object().ok_or_else(|| {
        AppError::InvalidInput("Qoder provider configuration must be an object".to_string())
    })?;
    Ok(required_string(provider, "baseUrl")?.to_string())
}

fn required_string<'a>(object: &'a Map<String, Value>, key: &str) -> Result<&'a str, AppError> {
    object
        .get(key)
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| {
            AppError::InvalidInput(format!("Qoder provider '{key}' must be a non-empty string"))
        })
}

fn lock_settings_file() -> Result<MutexGuard<'static, ()>, AppError> {
    SETTINGS_FILE_LOCK
        .lock()
        .map_err(|error| AppError::Config(format!("Qoder settings file lock is poisoned: {error}")))
}

fn read_settings_document(path: &Path) -> Result<Value, AppError> {
    read_settings_document_with_revision(path).map(|(document, _)| document)
}

fn read_settings_document_with_revision(path: &Path) -> Result<(Value, String), AppError> {
    if !path.exists() {
        return Ok((
            Value::Object(Map::new()),
            MISSING_SETTINGS_REVISION.to_string(),
        ));
    }
    let bytes = read_file_limited(path)?;
    let revision = revision(&bytes);
    let source = String::from_utf8(bytes).map_err(|error| {
        AppError::Config(format!(
            "Qoder settings file must be UTF-8 ({}): {error}",
            path.display()
        ))
    })?;
    let document = json5::from_str(&source).map_err(|error| {
        AppError::Config(format!(
            "Qoder settings file is not valid JSON/JSONC ({}): {error}",
            path.display()
        ))
    })?;
    Ok((document, revision))
}

fn read_file_limited(path: &Path) -> Result<Vec<u8>, AppError> {
    let file = fs::File::open(path).map_err(|error| AppError::io(path, error))?;
    let metadata = file.metadata().map_err(|error| AppError::io(path, error))?;
    if metadata.len() > MAX_QODER_SETTINGS_BYTES {
        return Err(AppError::InvalidInput(format!(
            "Qoder settings file exceeds the 1 MiB limit: {}",
            path.display()
        )));
    }
    let mut bytes = Vec::with_capacity(metadata.len() as usize);
    file.take(MAX_QODER_SETTINGS_BYTES + 1)
        .read_to_end(&mut bytes)
        .map_err(|error| AppError::io(path, error))?;
    if bytes.len() as u64 > MAX_QODER_SETTINGS_BYTES {
        return Err(AppError::InvalidInput(format!(
            "Qoder settings file exceeds the 1 MiB limit: {}",
            path.display()
        )));
    }
    Ok(bytes)
}

fn providers<'a>(document: &'a Value, path: &Path) -> Result<&'a Map<String, Value>, AppError> {
    let root = document.as_object().ok_or_else(|| {
        AppError::Config(format!(
            "Qoder settings root must be an object: {}",
            path.display()
        ))
    })?;
    match root.get("providers") {
        None => Ok(empty_json_object()),
        Some(Value::Object(providers)) => Ok(providers),
        Some(_) => Err(AppError::Config(format!(
            "Qoder settings 'providers' must be an object: {}",
            path.display()
        ))),
    }
}

fn providers_mut<'a>(
    document: &'a mut Value,
    path: &Path,
) -> Result<&'a mut Map<String, Value>, AppError> {
    let root = document.as_object_mut().ok_or_else(|| {
        AppError::Config(format!(
            "Qoder settings root must be an object: {}",
            path.display()
        ))
    })?;
    root.entry("providers".to_string())
        .or_insert_with(|| Value::Object(Map::new()))
        .as_object_mut()
        .ok_or_else(|| {
            AppError::Config(format!(
                "Qoder settings 'providers' must be an object: {}",
                path.display()
            ))
        })
}

fn empty_json_object() -> &'static Map<String, Value> {
    static EMPTY: LazyLock<Map<String, Value>> = LazyLock::new(Map::new);
    &EMPTY
}

fn write_settings_document(
    path: &Path,
    document: &Value,
    expected_revision: &str,
) -> Result<(), AppError> {
    let mut bytes =
        serde_json::to_vec_pretty(document).map_err(|source| AppError::JsonSerialize { source })?;
    bytes.push(b'\n');
    ensure_private_parent(path)?;
    ensure_settings_revision(path, expected_revision)?;
    atomic_write_private(path, &bytes)
}

fn ensure_settings_revision(path: &Path, expected_revision: &str) -> Result<(), AppError> {
    let actual_revision = match fs::File::open(path) {
        Ok(_) => revision(&read_file_limited(path)?),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            MISSING_SETTINGS_REVISION.to_string()
        }
        Err(error) => return Err(AppError::io(path, error)),
    };
    if actual_revision == expected_revision {
        Ok(())
    } else {
        Err(AppError::Conflict(format!(
            "Qoder settings.json changed outside CC Switch: {}",
            path.display()
        )))
    }
}

fn revision(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

fn ensure_private_parent(path: &Path) -> Result<(), AppError> {
    let parent = path.parent().ok_or_else(|| {
        AppError::Config(format!(
            "Qoder settings path has no parent directory: {}",
            path.display()
        ))
    })?;
    let created = !parent.exists();
    fs::create_dir_all(parent).map_err(|source| AppError::io(parent, source))?;

    #[cfg(not(unix))]
    let _ = created;

    #[cfg(unix)]
    if created {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(parent, fs::Permissions::from_mode(0o700))
            .map_err(|source| AppError::io(parent, source))?;
    }
    Ok(())
}

#[cfg(test)]
pub(crate) mod test_support {
    use std::path::{Path, PathBuf};

    pub(crate) struct TestQoderDir {
        _dir: Option<tempfile::TempDir>,
        previous: Option<PathBuf>,
    }

    impl TestQoderDir {
        pub(crate) fn new() -> Self {
            let dir = tempfile::tempdir().expect("create Qoder test directory");
            let qoder_dir = dir.path().join(".qoder");
            Self::set(qoder_dir, Some(dir))
        }

        pub(crate) fn at(qoder_dir: &Path) -> Self {
            Self::set(qoder_dir.to_path_buf(), None)
        }

        fn set(qoder_dir: PathBuf, dir: Option<tempfile::TempDir>) -> Self {
            let previous = super::TEST_QODER_DIR
                .lock()
                .expect("lock Qoder test directory")
                .replace(qoder_dir);
            Self {
                _dir: dir,
                previous,
            }
        }
    }

    impl Drop for TestQoderDir {
        fn drop(&mut self) {
            *super::TEST_QODER_DIR
                .lock()
                .expect("lock Qoder test directory") = self.previous.take();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use serial_test::serial;

    fn provider() -> Value {
        json!({
            "type": "openai-compatible",
            "displayName": "Test Provider",
            "protocol": "openai-responses",
            "authType": "bearer",
            "baseUrl": "https://api.example.com/v1",
            "apiKey": "secret",
            "model": "model-a",
            "models": [{
                "model": "model-a",
                "displayName": "Model A",
                "contextWindow": 128000,
                "maxOutputTokens": 8192,
                "capabilities": { "tools": true, "vision": false }
            }]
        })
    }

    #[test]
    #[serial]
    fn provider_edits_preserve_unrelated_qoder_settings() {
        let _qoder = test_support::TestQoderDir::new();
        let path = get_qoder_settings_path();
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(
            &path,
            r#"{"theme":"dark","providers":{"native":{"future":true}}}"#,
        )
        .unwrap();

        insert_qoder_provider("cc-switch", &provider()).expect("insert provider");
        let document: Value = serde_json::from_slice(&fs::read(path).unwrap()).unwrap();
        assert_eq!(document["theme"], json!("dark"));
        assert_eq!(document["providers"]["native"]["future"], json!(true));
        assert_eq!(document["providers"]["cc-switch"], provider());
    }

    #[test]
    fn validates_qoder_native_protocols() {
        for protocol in ["openai", "openai-responses", "anthropic"] {
            let mut value = provider();
            value["protocol"] = json!(protocol);
            validate_provider_node("valid-key", &value).expect("supported protocol");
        }
        let mut invalid = provider();
        invalid["protocol"] = json!("gemini");
        assert!(validate_provider_node("valid-key", &invalid).is_err());
    }
}
