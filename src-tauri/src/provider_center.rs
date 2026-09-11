//! 增量式「模型服务中心」。
//!
//! 此模块刻意不替换既有 providers 表或 ProviderService：它只保存一个不含
//! 明文密钥的共享定义和绑定关系；用户明确点击「应用」后，才投影为原有的
//! 应用级 Provider。这样原有的单应用自定义仍然是唯一的最终写入路径。

mod app_adapter;

use self::app_adapter::{projected_provider_id, provider_fingerprint, AppAdapterRegistry};
use crate::app_config::AppType;
use crate::database::{ProviderImportCandidateRecord, ProviderImportSessionRecord};
use crate::error::AppError;
use crate::provider::{Provider, UniversalProvider};
use crate::services::ProviderService;
use crate::store::AppState;
use base64::{engine::general_purpose::STANDARD as BASE64, Engine as _};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::{HashMap, HashSet};
use std::str::FromStr;
use std::sync::Arc;
use tokio::sync::{Mutex, OwnedMutexGuard, RwLock};
use uuid::Uuid;

const DEFINITIONS_KEY: &str = "provider_center_definitions_v1";
const BINDINGS_KEY: &str = "provider_center_bindings_v1";
const SECRETS_KEY: &str = "provider_center_dpapi_secrets_v1";
const TRANSACTIONS_KEY: &str = "provider_center_transactions_v1";
const SNAPSHOTS_KEY: &str = "provider_center_transaction_snapshots_v1";
const SQLITE_MIGRATED_KEY: &str = "provider_center_sqlite_migrated_v2";

fn default_true() -> bool {
    true
}

fn default_revision() -> u64 {
    1
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct ProviderSource {
    pub source_app: String,
    pub source_ref: String,
    #[serde(default)]
    pub imported_at: i64,
    #[serde(default)]
    pub source_fingerprint: Option<String>,
    #[serde(default)]
    pub last_observed_at: Option<i64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProviderDefinition {
    pub id: String,
    pub name: String,
    pub protocol: String,
    pub base_url: String,
    #[serde(default)]
    pub models: Vec<String>,
    #[serde(default)]
    pub discovered_models: Vec<String>,
    #[serde(default)]
    pub notes: String,
    #[serde(default = "default_true")]
    pub enabled: bool,
    #[serde(default = "default_revision")]
    pub revision: u64,
    #[serde(default)]
    pub source: Option<ProviderSource>,
    pub credential_configured: bool,
    #[serde(default)]
    pub credential_hint: Option<String>,
    #[serde(default)]
    pub last_discovery_at: Option<i64>,
    #[serde(default)]
    pub last_discovery_error: Option<String>,
    pub created_at: i64,
    pub updated_at: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProviderBinding {
    pub provider_id: String,
    pub app_type: String,
    /// pending | applied | overridden | unsupported | drifted | detached | failed
    pub status: String,
    #[serde(default = "default_true")]
    pub enabled: bool,
    #[serde(default)]
    pub override_enabled: bool,
    #[serde(default)]
    pub applied_revision: Option<u64>,
    #[serde(default)]
    pub expected_fingerprint: Option<String>,
    #[serde(default)]
    pub last_error: Option<String>,
    #[serde(default)]
    pub last_transaction_id: Option<String>,
    pub updated_at: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProviderCenterState {
    pub definitions: Vec<ProviderDefinition>,
    pub bindings: Vec<ProviderBinding>,
    #[serde(default)]
    pub transactions: Vec<ProviderApplyTransaction>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SaveProviderDefinitionInput {
    pub id: Option<String>,
    pub name: String,
    pub protocol: String,
    pub base_url: String,
    #[serde(default)]
    pub models: Vec<String>,
    #[serde(default)]
    pub notes: String,
    pub enabled: Option<bool>,
    pub expected_revision: Option<u64>,
    /// keep | replace | clear。旧前端不传时，有 apiKey 就替换，否则保留。
    pub credential_action: Option<String>,
    #[serde(default)]
    pub source: Option<ProviderSource>,
    /// 只接受写入；永不作为 IPC 响应返回。
    pub api_key: Option<String>,
    #[serde(default)]
    pub app_types: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ImportFailure {
    pub app_type: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_ref: Option<String>,
    pub code: String,
    pub stage: String,
    pub message: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ImportConflict {
    pub existing_provider_id: String,
    pub existing_name: String,
    pub existing_revision: u64,
    #[serde(default)]
    pub reasons: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ImportCommitDecision {
    pub action: String,
    #[serde(default)]
    pub target_provider_id: Option<String>,
    #[serde(default)]
    pub expected_revision: Option<u64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ImportCommitResult {
    pub action: String,
    #[serde(default)]
    pub provider: Option<ProviderDefinition>,
    pub repeated: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ImportCandidate {
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub id: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub session_id: String,
    pub source_ref: String,
    pub source_app: String,
    pub name: String,
    pub protocol: String,
    pub base_url: String,
    #[serde(default)]
    pub models: Vec<String>,
    pub credential_configured: bool,
    /// 只显示安全尾码，帮助区分条目；绝不返回密钥。
    pub credential_hint: Option<String>,
    #[serde(default)]
    pub conflicts: Vec<ImportConflict>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProviderImportSession {
    pub id: String,
    pub state: String,
    pub candidates: Vec<ImportCandidate>,
    #[serde(default)]
    pub errors: Vec<ImportFailure>,
    pub created_at: i64,
    pub expires_at: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
struct PersistedImportCandidateState {
    #[serde(default)]
    conflicts: Vec<ImportConflict>,
    #[serde(default)]
    outcome: Option<PersistedImportOutcome>,
    #[serde(default)]
    quarantine: Option<ImportFailure>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct PersistedImportOutcome {
    action: String,
    provider_id: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ModelDiscoveryResult {
    pub provider_id: String,
    pub models: Vec<String>,
    pub discovered_at: i64,
    pub error: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UnifiedModelCatalogEntry {
    pub id: String,
    pub app_type: String,
    pub provider_id: String,
    pub provider_name: String,
    pub model_id: String,
    pub protocol: String,
    /// apiKey | nativeAccount
    pub source_type: String,
    pub is_default: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UnifiedModelCatalog {
    pub entries: Vec<UnifiedModelCatalogEntry>,
    pub generated_at: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProviderApplyPreviewTarget {
    pub app_type: String,
    pub operation: String,
    pub compatible: bool,
    pub drifted: bool,
    pub current_provider_id: Option<String>,
    pub live_fingerprint: Option<String>,
    pub message: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProviderApplyPreview {
    pub token: String,
    pub provider_id: String,
    pub provider_revision: u64,
    pub targets: Vec<ProviderApplyPreviewTarget>,
    pub created_at: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProviderApplyTargetResult {
    pub app_type: String,
    pub status: String,
    pub message: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProviderApplyTransaction {
    pub id: String,
    pub provider_id: String,
    pub provider_revision: u64,
    pub status: String,
    pub targets: Vec<ProviderApplyTargetResult>,
    pub created_at: i64,
    pub completed_at: Option<i64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ProjectionSnapshot {
    app_type: String,
    projected_provider_id: String,
    previous_projected_provider: Option<Provider>,
    previous_current_provider_id: Option<String>,
    before_fingerprint: Option<String>,
    after_fingerprint: String,
}

fn now() -> i64 {
    chrono::Utc::now().timestamp_millis()
}

fn read_json<T: for<'a> Deserialize<'a> + Default>(
    state: &AppState,
    key: &str,
) -> Result<T, AppError> {
    match state.db.get_setting(key)? {
        Some(value) => serde_json::from_str(&value)
            .map_err(|err| AppError::Database(format!("模型服务中心数据损坏: {err}"))),
        None => Ok(T::default()),
    }
}

fn write_json<T: Serialize>(state: &AppState, key: &str, value: &T) -> Result<(), AppError> {
    let text = serde_json::to_string(value)
        .map_err(|err| AppError::Database(format!("模型服务中心数据序列化失败: {err}")))?;
    state.db.set_setting(key, &text)
}

type Definitions = Vec<ProviderDefinition>;
type Bindings = Vec<ProviderBinding>;
type EncryptedSecrets = HashMap<String, String>;
type EncryptedSnapshots = HashMap<String, String>;

fn legacy_universal_source_ref(universal_id: &str) -> String {
    format!("universal:{universal_id}")
}

fn migrated_legacy_universal_id(
    definitions: &[ProviderDefinition],
    universal_id: &str,
) -> Option<String> {
    let source_ref = legacy_universal_source_ref(universal_id);
    definitions.iter().find_map(|definition| {
        definition
            .source
            .as_ref()
            .filter(|source| source.source_app == "cc-switch" && source.source_ref == source_ref)
            .map(|_| definition.id.clone())
    })
}

fn convert_legacy_universal(
    universal: &UniversalProvider,
    id: String,
    timestamp: i64,
) -> (ProviderDefinition, Vec<ProviderBinding>) {
    let mut models = Vec::new();
    if let Some(config) = &universal.models.claude {
        models.extend(
            [
                config.model.clone(),
                config.haiku_model.clone(),
                config.sonnet_model.clone(),
                config.opus_model.clone(),
            ]
            .into_iter()
            .flatten(),
        );
    }
    if let Some(config) = &universal.models.codex {
        models.extend(config.model.clone());
    }
    if let Some(config) = &universal.models.gemini {
        models.extend(config.model.clone());
    }
    models.retain(|model| !model.trim().is_empty());
    models.sort();
    models.dedup();

    let definition = ProviderDefinition {
        id: id.clone(),
        name: universal.name.clone(),
        protocol: match universal.provider_type.as_str() {
            "openai-responses" | "openai-chat" | "anthropic" | "gemini" | "ollama" => {
                universal.provider_type.clone()
            }
            _ => "openai-chat".to_string(),
        },
        base_url: universal.base_url.trim_end_matches('/').to_string(),
        models,
        discovered_models: Vec::new(),
        notes: universal.notes.clone().unwrap_or_default(),
        enabled: true,
        revision: 1,
        source: Some(ProviderSource {
            source_app: "cc-switch".to_string(),
            source_ref: legacy_universal_source_ref(&universal.id),
            imported_at: timestamp,
            source_fingerprint: None,
            last_observed_at: None,
        }),
        credential_configured: !universal.api_key.trim().is_empty(),
        credential_hint: secret_hint(&universal.api_key),
        last_discovery_at: None,
        last_discovery_error: None,
        created_at: timestamp,
        updated_at: timestamp,
    };
    let bindings = [
        ("claude", universal.apps.claude),
        ("codex", universal.apps.codex),
        ("gemini", universal.apps.gemini),
    ]
    .into_iter()
    .filter(|(_, enabled)| *enabled)
    .map(|(app_type, _)| ProviderBinding {
        provider_id: id.clone(),
        app_type: app_type.to_string(),
        status: "pending".to_string(),
        enabled: true,
        override_enabled: false,
        applied_revision: None,
        expected_fingerprint: None,
        last_error: None,
        last_transaction_id: None,
        updated_at: timestamp,
    })
    .collect();
    (definition, bindings)
}

fn ensure_sqlite_migrated(state: &AppState) -> Result<(), AppError> {
    if state.db.get_bool_flag(SQLITE_MIGRATED_KEY)? {
        return Ok(());
    }

    // Only seed an empty v19 store. This makes the migration idempotent even
    // if the process exits after committing SQLite but before writing the
    // marker. The legacy settings remain as a read-only rollback artifact.
    let had_sqlite_definitions = state.db.provider_center_has_definitions()?;
    let mut definitions = if had_sqlite_definitions {
        state.db.load_provider_center_definitions()?
    } else {
        read_json(state, DEFINITIONS_KEY)?
    };
    let mut bindings = if had_sqlite_definitions {
        state.db.load_provider_center_bindings()?
    } else {
        read_json(state, BINDINGS_KEY)?
    };
    let legacy_universal = state.db.get_all_universal_providers()?;
    let mut migrated_universal_ids = Vec::new();
    for universal in legacy_universal.values() {
        if let Some(id) = migrated_legacy_universal_id(&definitions, &universal.id) {
            // A previous run may have committed v19 and then stopped before
            // deleting the plaintext legacy row. Re-store the secret and
            // finish cleanup without creating a second definition.
            if !universal.api_key.trim().is_empty() {
                save_secret(state, &id, universal.api_key.trim())?;
            }
            migrated_universal_ids.push(universal.id.clone());
            continue;
        }
        let mut id = universal.id.clone();
        if definitions.iter().any(|definition| definition.id == id) {
            id = format!("legacy-universal-{}", universal.id);
        }
        if definitions.iter().any(|definition| definition.id == id) {
            let suffix = Uuid::new_v4().simple().to_string();
            id = format!("legacy-universal-{}-{}", universal.id, &suffix[..8]);
        }
        if !universal.api_key.trim().is_empty() {
            save_secret(state, &id, universal.api_key.trim())?;
        }
        let timestamp = universal.created_at.unwrap_or_else(now);
        let (definition, migrated_bindings) = convert_legacy_universal(universal, id, timestamp);
        definitions.push(definition);
        bindings.extend(migrated_bindings);
        migrated_universal_ids.push(universal.id.clone());
    }

    state
        .db
        .save_provider_center_core(&definitions, &bindings)?;
    if !had_sqlite_definitions {
        let transactions: Vec<ProviderApplyTransaction> = read_json(state, TRANSACTIONS_KEY)?;
        for transaction in &transactions {
            state.db.upsert_provider_center_transaction(transaction)?;
        }
    }
    // Remove the legacy plaintext definitions only after the v19 rows and
    // secure-store entries have committed successfully.
    for id in migrated_universal_ids {
        state.db.delete_universal_provider(&id)?;
    }
    state.db.set_setting(SQLITE_MIGRATED_KEY, "true")
}

fn load_definitions(state: &AppState) -> Result<Definitions, AppError> {
    ensure_sqlite_migrated(state)?;
    let mut definitions = state.db.load_provider_center_definitions()?;
    for definition in &mut definitions {
        let (configured, hint) = secret_metadata(state, &definition.id)?;
        definition.credential_configured = configured;
        definition.credential_hint = hint;
    }
    Ok(definitions)
}

fn load_bindings(state: &AppState) -> Result<Bindings, AppError> {
    ensure_sqlite_migrated(state)?;
    state.db.load_provider_center_bindings()
}

fn save_core(
    state: &AppState,
    definitions: &[ProviderDefinition],
    bindings: &[ProviderBinding],
) -> Result<(), AppError> {
    state.db.save_provider_center_core(definitions, bindings)
}

fn load_transactions(state: &AppState) -> Result<Vec<ProviderApplyTransaction>, AppError> {
    ensure_sqlite_migrated(state)?;
    state.db.load_provider_center_transactions()
}

/// Serializes Provider Center writes that touch the same application.
/// Multi-app operations acquire locks in sorted order to avoid deadlocks.
#[derive(Default)]
pub struct ProviderCenterOperationState {
    data_lock: Arc<Mutex<()>>,
    locks: RwLock<HashMap<String, Arc<Mutex<()>>>>,
}

impl ProviderCenterOperationState {
    pub async fn lock_data(&self) -> OwnedMutexGuard<()> {
        self.data_lock.clone().lock_owned().await
    }

    async fn lock_for(&self, app_type: &str) -> Arc<Mutex<()>> {
        if let Some(lock) = self.locks.read().await.get(app_type).cloned() {
            return lock;
        }
        let mut locks = self.locks.write().await;
        locks
            .entry(app_type.to_string())
            .or_insert_with(|| Arc::new(Mutex::new(())))
            .clone()
    }

    pub async fn lock_apps(&self, app_types: Vec<String>) -> Vec<OwnedMutexGuard<()>> {
        let mut app_types = app_types;
        app_types.sort();
        app_types.dedup();

        let mut locks = Vec::with_capacity(app_types.len());
        for app_type in app_types {
            locks.push(self.lock_for(&app_type).await);
        }

        let mut guards = Vec::with_capacity(locks.len());
        for lock in locks {
            guards.push(lock.lock_owned().await);
        }
        guards
    }
}

fn secret_hint(secret: &str) -> Option<String> {
    let chars: Vec<char> = secret.chars().collect();
    (chars.len() >= 4).then(|| format!("…{}", chars[chars.len() - 4..].iter().collect::<String>()))
}

// 密钥使用 Windows 当前用户 DPAPI 加密。加密后的 blob 可以随 CC Switch 数据库
// 备份，但无法由其他 Windows 用户或机器解开；IPC/前端从不读取它。
#[cfg(target_os = "windows")]
fn protect_secret(secret: &str) -> Result<String, AppError> {
    use std::ffi::c_void;
    use std::ptr::{null, null_mut};
    use windows_sys::Win32::Foundation::LocalFree;
    use windows_sys::Win32::Security::Cryptography::{
        CryptProtectData, CRYPTPROTECT_UI_FORBIDDEN, CRYPT_INTEGER_BLOB,
    };

    let bytes = secret.as_bytes();
    let input = CRYPT_INTEGER_BLOB {
        cbData: bytes.len() as u32,
        pbData: bytes.as_ptr() as *mut u8,
    };
    let mut output = CRYPT_INTEGER_BLOB {
        cbData: 0,
        pbData: null_mut(),
    };
    let ok = unsafe {
        CryptProtectData(
            &input,
            null(),
            null(),
            null_mut(),
            null(),
            CRYPTPROTECT_UI_FORBIDDEN,
            &mut output,
        )
    };
    if ok == 0 {
        return Err(AppError::Message(
            "无法使用 Windows 安全存储保护 API Key".to_string(),
        ));
    }
    let protected =
        unsafe { std::slice::from_raw_parts(output.pbData, output.cbData as usize).to_vec() };
    unsafe { LocalFree(output.pbData as *mut c_void) };
    Ok(BASE64.encode(protected))
}

#[cfg(target_os = "windows")]
fn unprotect_secret(blob: &str) -> Result<String, AppError> {
    use std::ffi::c_void;
    use std::ptr::{null, null_mut};
    use windows_sys::Win32::Foundation::LocalFree;
    use windows_sys::Win32::Security::Cryptography::{
        CryptUnprotectData, CRYPTPROTECT_UI_FORBIDDEN, CRYPT_INTEGER_BLOB,
    };

    let mut bytes = BASE64
        .decode(blob)
        .map_err(|_| AppError::Message("已保存的 API Key 数据无效".to_string()))?;
    let input = CRYPT_INTEGER_BLOB {
        cbData: bytes.len() as u32,
        pbData: bytes.as_mut_ptr(),
    };
    let mut output = CRYPT_INTEGER_BLOB {
        cbData: 0,
        pbData: null_mut(),
    };
    let ok = unsafe {
        CryptUnprotectData(
            &input,
            null_mut(),
            null(),
            null_mut(),
            null(),
            CRYPTPROTECT_UI_FORBIDDEN,
            &mut output,
        )
    };
    if ok == 0 {
        return Err(AppError::Message(
            "无法从 Windows 安全存储读取 API Key".to_string(),
        ));
    }
    let secret =
        unsafe { std::slice::from_raw_parts(output.pbData, output.cbData as usize).to_vec() };
    unsafe { LocalFree(output.pbData as *mut c_void) };
    String::from_utf8(secret)
        .map_err(|_| AppError::Message("已保存的 API Key 不是有效文本".to_string()))
}

#[cfg(target_os = "windows")]
fn save_secret(state: &AppState, id: &str, secret: &str) -> Result<(), AppError> {
    let mut secrets: EncryptedSecrets = read_json(state, SECRETS_KEY)?;
    secrets.insert(id.to_string(), protect_secret(secret)?);
    write_json(state, SECRETS_KEY, &secrets)
}

#[cfg(target_os = "windows")]
fn remove_secret(state: &AppState, id: &str) -> Result<(), AppError> {
    let mut secrets: EncryptedSecrets = read_json(state, SECRETS_KEY)?;
    secrets.remove(id);
    write_json(state, SECRETS_KEY, &secrets)
}

#[cfg(target_os = "windows")]
fn secret_metadata(state: &AppState, id: &str) -> Result<(bool, Option<String>), AppError> {
    let secrets: EncryptedSecrets = read_json(state, SECRETS_KEY)?;
    let Some(blob) = secrets.get(id) else {
        return Ok((false, None));
    };
    let secret = unprotect_secret(blob)?;
    Ok((true, secret_hint(&secret)))
}

#[cfg(target_os = "windows")]
fn get_secret(state: &AppState, id: &str) -> Result<String, AppError> {
    let secrets: EncryptedSecrets = read_json(state, SECRETS_KEY)?;
    let blob = secrets
        .get(id)
        .ok_or_else(|| AppError::Message("该模型服务没有可用的 API Key".to_string()))?;
    unprotect_secret(blob)
}

#[cfg(not(target_os = "windows"))]
fn secure_entry(id: &str) -> Result<keyring::Entry, AppError> {
    keyring::Entry::new("com.ccswitch.provider-center", id)
        .map_err(|error| AppError::Message(format!("无法访问系统安全存储: {error}")))
}

#[cfg(not(target_os = "windows"))]
fn save_secret(_: &AppState, id: &str, secret: &str) -> Result<(), AppError> {
    secure_entry(id)?
        .set_password(secret)
        .map_err(|error| AppError::Message(format!("无法写入系统安全存储: {error}")))
}

#[cfg(not(target_os = "windows"))]
fn remove_secret(_: &AppState, id: &str) -> Result<(), AppError> {
    match secure_entry(id)?.delete_credential() {
        Ok(()) | Err(keyring::Error::NoEntry) => Ok(()),
        Err(error) => Err(AppError::Message(format!("无法清除系统安全存储: {error}"))),
    }
}

#[cfg(not(target_os = "windows"))]
fn secret_metadata(_: &AppState, id: &str) -> Result<(bool, Option<String>), AppError> {
    match secure_entry(id)?.get_password() {
        Ok(secret) => Ok((true, secret_hint(&secret))),
        Err(keyring::Error::NoEntry) => Ok((false, None)),
        Err(error) => Err(AppError::Message(format!("无法读取系统安全存储: {error}"))),
    }
}

#[cfg(not(target_os = "windows"))]
fn get_secret(_: &AppState, id: &str) -> Result<String, AppError> {
    match secure_entry(id)?.get_password() {
        Ok(secret) => Ok(secret),
        Err(keyring::Error::NoEntry) => Err(AppError::Message(
            "该模型服务没有可用的 API Key".to_string(),
        )),
        Err(error) => Err(AppError::Message(format!("无法读取系统安全存储: {error}"))),
    }
}

fn refresh_binding_states(
    state: &AppState,
    definitions: &[ProviderDefinition],
    bindings: &mut [ProviderBinding],
) -> Result<bool, AppError> {
    let mut changed = false;
    for binding in bindings.iter_mut().filter(|binding| {
        binding.enabled && !binding.override_enabled && binding.status == "applied"
    }) {
        let Some(definition) = definitions
            .iter()
            .find(|definition| definition.id == binding.provider_id)
        else {
            binding.status = "detached".to_string();
            binding.last_error = Some("共享模型服务已不存在".to_string());
            binding.updated_at = now();
            changed = true;
            continue;
        };
        if binding.applied_revision != Some(definition.revision) {
            binding.status = "pending".to_string();
            binding.last_error = None;
            binding.updated_at = now();
            changed = true;
            continue;
        }
        let app = match AppType::from_str(&binding.app_type) {
            Ok(app) => app,
            Err(_) => {
                binding.status = "unsupported".to_string();
                binding.last_error = Some("应用类型无效".to_string());
                binding.updated_at = now();
                changed = true;
                continue;
            }
        };
        let secret = if definition.protocol == "ollama" {
            String::new()
        } else {
            match get_secret(state, &definition.id) {
                Ok(secret) => secret,
                Err(_) => {
                    binding.status = "failed".to_string();
                    binding.last_error = Some("API Key 已不可用，请重新配置".to_string());
                    binding.updated_at = now();
                    changed = true;
                    continue;
                }
            }
        };
        let projected = match projection(definition, secret, &binding.app_type) {
            Ok(projected) => projected,
            Err(error) => {
                binding.status = "unsupported".to_string();
                binding.last_error = Some(error.to_string());
                binding.updated_at = now();
                changed = true;
                continue;
            }
        };
        let actual = state.db.get_provider_by_id(&projected.id, app.as_str())?;
        let actual_fingerprint = actual.as_ref().map(provider_fingerprint).transpose()?;
        if actual_fingerprint.as_ref() != binding.expected_fingerprint.as_ref() {
            binding.status = "drifted".to_string();
            binding.last_error = Some("目标应用配置已在共享中心之外发生变化".to_string());
            binding.updated_at = now();
            changed = true;
        }
    }
    Ok(changed)
}

pub fn state(state: &AppState) -> Result<ProviderCenterState, AppError> {
    let definitions = load_definitions(state)?;
    let mut bindings = load_bindings(state)?;
    if refresh_binding_states(state, &definitions, &mut bindings)? {
        save_core(state, &definitions, &bindings)?;
    }
    Ok(ProviderCenterState {
        definitions,
        bindings,
        transactions: load_transactions(state)?,
    })
}

/// Return only models that the selected application can actually use now.
/// Shared API-key models require an enabled, applied, non-drifted binding at
/// the current definition revision. Native/Coding Plan providers are kept in a
/// separate source bucket so the UI never conflates account login with API
/// key storage.
pub fn unified_model_catalog(
    state: &AppState,
    requested_apps: Vec<String>,
) -> Result<UnifiedModelCatalog, AppError> {
    let requested = requested_apps
        .into_iter()
        .filter_map(|value| AppType::from_str(&value).ok())
        .map(|app| app.as_str().to_string())
        .collect::<HashSet<_>>();
    let includes = |app: &str| requested.is_empty() || requested.contains(app);
    let definitions = load_definitions(state)?;
    let bindings = load_bindings(state)?;
    let mut entries = Vec::new();

    for binding in bindings.iter().filter(|binding| {
        binding.enabled
            && !binding.override_enabled
            && binding.status == "applied"
            && includes(&binding.app_type)
    }) {
        let Some(definition) = definitions.iter().find(|item| {
            item.id == binding.provider_id
                && item.enabled
                && binding.applied_revision == Some(item.revision)
        }) else {
            continue;
        };
        let secret = if definition.protocol == "ollama" {
            String::new()
        } else {
            match get_secret(state, &definition.id) {
                Ok(secret) => secret,
                Err(_) => continue,
            }
        };
        let app = match AppType::from_str(&binding.app_type) {
            Ok(app) => app,
            Err(_) => continue,
        };
        let projected = match projection(definition, secret, &binding.app_type) {
            Ok(projected) => projected,
            Err(_) => continue,
        };
        let Some(actual) = state.db.get_provider_by_id(&projected.id, app.as_str())? else {
            continue;
        };
        let actual_fingerprint = provider_fingerprint(&actual)?;
        if binding.expected_fingerprint.as_deref() != Some(actual_fingerprint.as_str()) {
            continue;
        }

        let mut models = definition.models.clone();
        models.extend(definition.discovered_models.clone());
        models.retain(|model| !model.trim().is_empty());
        models.sort();
        models.dedup();
        for (index, model_id) in models.into_iter().enumerate() {
            entries.push(UnifiedModelCatalogEntry {
                id: format!(
                    "api-key:{}:{}:{}",
                    binding.app_type, definition.id, model_id
                ),
                app_type: binding.app_type.clone(),
                provider_id: definition.id.clone(),
                provider_name: definition.name.clone(),
                model_id,
                protocol: definition.protocol.clone(),
                source_type: "apiKey".to_string(),
                is_default: index == 0,
            });
        }
    }

    for app in AppType::all().filter(|app| includes(app.as_str())) {
        let current = app_adapter(&app).current(state).unwrap_or_default();
        let providers = match app_adapter(&app).list(state) {
            Ok(providers) => providers,
            Err(_) => continue,
        };
        for provider in providers.values().filter(|provider| {
            provider.category.as_deref() == Some("official") || provider.uses_managed_account_auth()
        }) {
            let models = models_from_settings(&provider.settings_config);
            for model_id in models {
                entries.push(UnifiedModelCatalogEntry {
                    id: format!("native:{}:{}:{}", app.as_str(), provider.id, model_id),
                    app_type: app.as_str().to_string(),
                    provider_id: provider.id.clone(),
                    provider_name: provider.name.clone(),
                    model_id,
                    protocol: protocol_for(&app).to_string(),
                    source_type: "nativeAccount".to_string(),
                    is_default: current == provider.id,
                });
            }
        }
    }

    entries.sort_by(|left, right| {
        (
            &left.app_type,
            &left.source_type,
            &left.provider_name,
            &left.model_id,
        )
            .cmp(&(
                &right.app_type,
                &right.source_type,
                &right.provider_name,
                &right.model_id,
            ))
    });
    entries.dedup_by(|left, right| left.id == right.id);
    Ok(UnifiedModelCatalog {
        entries,
        generated_at: now(),
    })
}

/// A process exit can leave an operation persisted as running/restoring even
/// though no worker can still own it after the next launch. Promote those
/// records to recoveryRequired once during application startup so the UI keeps
/// an actionable, durable warning instead of displaying a stale in-progress
/// state forever.
pub fn reconcile_interrupted_transactions(state: &AppState) -> Result<usize, AppError> {
    let mut transactions = load_transactions(state)?;
    let mut changed = 0;
    for transaction in &mut transactions {
        if matches!(transaction.status.as_str(), "running" | "restoring") {
            transaction.status = "recoveryRequired".to_string();
            transaction.completed_at.get_or_insert_with(now);
            changed += 1;
        }
    }
    if changed > 0 {
        for transaction in &transactions {
            state.db.upsert_provider_center_transaction(transaction)?;
        }
    }
    Ok(changed)
}

pub fn save_definition(
    state: &AppState,
    input: SaveProviderDefinitionInput,
) -> Result<ProviderDefinition, AppError> {
    if input.name.trim().is_empty() || input.base_url.trim().is_empty() {
        return Err(AppError::Message(
            "请填写模型服务名称和请求地址".to_string(),
        ));
    }
    let protocol = if input.protocol.trim().is_empty() {
        "openai-chat".to_string()
    } else {
        input.protocol.trim().to_string()
    };
    if !matches!(
        protocol.as_str(),
        "openai-responses" | "openai-chat" | "anthropic" | "gemini" | "ollama"
    ) {
        return Err(AppError::Message("不支持的接口协议".to_string()));
    }
    let parsed_url = url::Url::parse(input.base_url.trim())
        .map_err(|_| AppError::Message("请求地址不是有效的 URL".to_string()))?;
    if !matches!(parsed_url.scheme(), "http" | "https")
        || parsed_url.host_str().is_none()
        || !parsed_url.username().is_empty()
        || parsed_url.password().is_some()
        || parsed_url.query().is_some()
        || parsed_url.fragment().is_some()
    {
        return Err(AppError::Message(
            "请求地址必须是无账号、查询参数和片段的 HTTP(S) 地址".to_string(),
        ));
    }
    let mut definitions = load_definitions(state)?;
    let mut bindings = load_bindings(state)?;
    let timestamp = now();
    let id = input
        .id
        .clone()
        .unwrap_or_else(|| Uuid::new_v4().to_string());
    let existing = definitions.iter().find(|item| item.id == id).cloned();
    if let (Some(expected), Some(current)) = (input.expected_revision, existing.as_ref()) {
        if expected != current.revision {
            return Err(AppError::Message(format!(
                "模型服务已被其他操作修改（当前版本 {}，提交版本 {}），请刷新后重试",
                current.revision, expected
            )));
        }
    }
    let credential_action = input.credential_action.as_deref().unwrap_or_else(|| {
        if input
            .api_key
            .as_ref()
            .is_some_and(|key| !key.trim().is_empty())
        {
            "replace"
        } else {
            "keep"
        }
    });
    match credential_action {
        "keep" => {}
        "replace" => {
            let key = input
                .api_key
                .as_deref()
                .map(str::trim)
                .filter(|key| !key.is_empty())
                .ok_or_else(|| AppError::Message("替换 API Key 时不能为空".to_string()))?;
            save_secret(state, &id, key)?;
        }
        "clear" => remove_secret(state, &id)?,
        _ => return Err(AppError::Message("无效的 API Key 保存方式".to_string())),
    }
    let (credential_configured, credential_hint) = secret_metadata(state, &id)?;
    let selected_apps: HashSet<String> = input
        .app_types
        .iter()
        .filter(|app| AppType::from_str(app).is_ok())
        .cloned()
        .collect();
    let definition = ProviderDefinition {
        id: id.clone(),
        name: input.name.trim().to_string(),
        protocol,
        base_url: input.base_url.trim().trim_end_matches('/').to_string(),
        models: input
            .models
            .into_iter()
            .map(|model| model.trim().to_string())
            .filter(|model| !model.is_empty())
            .collect(),
        discovered_models: existing
            .as_ref()
            .map(|item| item.discovered_models.clone())
            .unwrap_or_default(),
        notes: input.notes.trim().to_string(),
        enabled: input
            .enabled
            .unwrap_or_else(|| existing.as_ref().map(|item| item.enabled).unwrap_or(true)),
        revision: existing
            .as_ref()
            .map(|item| item.revision.saturating_add(1))
            .unwrap_or(1),
        source: input
            .source
            .or_else(|| existing.as_ref().and_then(|item| item.source.clone())),
        credential_configured,
        credential_hint,
        last_discovery_at: existing.as_ref().and_then(|item| item.last_discovery_at),
        last_discovery_error: existing
            .as_ref()
            .and_then(|item| item.last_discovery_error.clone()),
        created_at: existing
            .as_ref()
            .map(|item| item.created_at)
            .unwrap_or(timestamp),
        updated_at: timestamp,
    };
    if let Some(index) = definitions.iter().position(|item| item.id == id) {
        definitions[index] = definition.clone();
    } else {
        definitions.push(definition.clone());
    }
    for binding in bindings.iter_mut().filter(|item| item.provider_id == id) {
        if selected_apps.contains(&binding.app_type) {
            binding.enabled = true;
            binding.status = if binding.override_enabled {
                "overridden".to_string()
            } else {
                "pending".to_string()
            };
            binding.last_error = None;
        } else {
            binding.enabled = false;
            binding.status = "detached".to_string();
        }
        binding.updated_at = timestamp;
    }
    for app in selected_apps {
        if let Some(binding) = bindings
            .iter_mut()
            .find(|item| item.provider_id == id && item.app_type == app)
        {
            binding.enabled = true;
        } else {
            bindings.push(ProviderBinding {
                provider_id: id.clone(),
                app_type: app,
                status: "pending".to_string(),
                enabled: true,
                override_enabled: false,
                applied_revision: None,
                expected_fingerprint: None,
                last_error: None,
                last_transaction_id: None,
                updated_at: timestamp,
            });
        }
    }
    save_core(state, &definitions, &bindings)?;
    Ok(definition)
}

fn models_from_settings(settings: &serde_json::Value) -> Vec<String> {
    let mut models = Vec::new();
    for pointer in [
        "/env/ANTHROPIC_MODEL",
        "/env/ANTHROPIC_DEFAULT_HAIKU_MODEL",
        "/env/ANTHROPIC_DEFAULT_SONNET_MODEL",
        "/env/ANTHROPIC_DEFAULT_OPUS_MODEL",
        "/env/GEMINI_MODEL",
        "/model",
        "/options/model",
    ] {
        if let Some(model) = settings
            .pointer(pointer)
            .and_then(serde_json::Value::as_str)
        {
            if !model.trim().is_empty() {
                models.push(model.to_string());
            }
        }
    }
    if let Some(value) = settings.get("models") {
        match value {
            serde_json::Value::Object(items) => models.extend(items.keys().cloned()),
            serde_json::Value::Array(items) => models.extend(items.iter().filter_map(|item| {
                item.get("id")
                    .or_else(|| item.get("model"))
                    .or_else(|| item.get("name"))
                    .and_then(serde_json::Value::as_str)
                    .map(str::to_string)
            })),
            _ => {}
        }
    }
    if let Some(config) = settings.get("config").and_then(serde_json::Value::as_str) {
        if let Ok(value) = toml::from_str::<toml::Value>(config) {
            fn collect(value: &toml::Value, models: &mut Vec<String>) {
                match value {
                    toml::Value::Table(table) => {
                        for (key, value) in table {
                            if matches!(key.as_str(), "model" | "default_model") {
                                if let Some(model) = value.as_str() {
                                    models.push(model.to_string());
                                }
                            } else {
                                collect(value, models);
                            }
                        }
                    }
                    toml::Value::Array(items) => {
                        for item in items {
                            collect(item, models);
                        }
                    }
                    _ => {}
                }
            }
            collect(&value, &mut models);
        }
    }
    models.sort();
    models.dedup();
    models
}

fn app_adapter(app: &AppType) -> &'static app_adapter::AppAdapter {
    AppAdapterRegistry::global()
        .resolve(app.as_str())
        .expect("all AppType variants must have Provider Center adapters")
}

fn protocol_for(app: &AppType) -> &'static str {
    app_adapter(app).default_protocol()
}

fn protocol_from_provider(app: &AppType, provider: &Provider) -> String {
    app_adapter(app).infer_protocol(provider)
}

fn candidate_from_provider(
    source_ref: String,
    app: &AppType,
    provider: &Provider,
) -> Option<ImportCandidate> {
    let (base_url, key) = provider.resolve_usage_credentials(app);
    if base_url.trim().is_empty() {
        return None;
    }
    Some(ImportCandidate {
        id: String::new(),
        session_id: String::new(),
        source_ref,
        source_app: app.as_str().to_string(),
        name: provider.name.clone(),
        protocol: protocol_from_provider(app, provider),
        base_url,
        models: models_from_settings(&provider.settings_config),
        credential_configured: !key.trim().is_empty(),
        credential_hint: secret_hint(&key),
        conflicts: Vec::new(),
    })
}

fn scan_imports_for_apps(
    state: &AppState,
    requested_apps: &HashSet<String>,
) -> (Vec<ImportCandidate>, Vec<ImportFailure>) {
    let mut candidates = Vec::new();
    let mut failures = Vec::new();
    for app in AppType::all() {
        if !requested_apps.is_empty() && !requested_apps.contains(app.as_str()) {
            continue;
        }
        match app_adapter(&app).list(state) {
            Ok(providers) => {
                for provider in providers.values() {
                    if let Some(candidate) = candidate_from_provider(
                        format!("saved:{}:{}", app.as_str(), provider.id),
                        &app,
                        provider,
                    ) {
                        candidates.push(candidate);
                    }
                }
            }
            Err(error) => failures.push(ImportFailure {
                app_type: app.as_str().to_string(),
                source_ref: None,
                code: "IMPORT_SAVED_SCAN_FAILED".to_string(),
                stage: "scanSaved".to_string(),
                message: error.to_string(),
            }),
        }
        match app_adapter(&app).read_live_settings() {
            Ok(settings) => {
                let live = Provider::with_id(
                    "live".to_string(),
                    format!("{} 当前配置", app.as_str()),
                    settings,
                    None,
                );
                if let Some(candidate) =
                    candidate_from_provider(format!("live:{}", app.as_str()), &app, &live)
                {
                    candidates.push(candidate);
                }
            }
            Err(error) => failures.push(ImportFailure {
                app_type: app.as_str().to_string(),
                source_ref: Some(format!("live:{}", app.as_str())),
                code: "IMPORT_LIVE_SCAN_FAILED".to_string(),
                stage: "scanLive".to_string(),
                message: error.to_string(),
            }),
        }
    }
    candidates.sort_by(|left, right| left.source_ref.cmp(&right.source_ref));
    candidates.dedup_by(|left, right| left.source_ref == right.source_ref);
    (candidates, failures)
}

pub fn scan_imports(state: &AppState) -> Result<Vec<ImportCandidate>, AppError> {
    Ok(scan_imports_for_apps(state, &HashSet::new()).0)
}

fn import_candidate_fingerprint(app: &AppType, provider: &Provider) -> Result<String, AppError> {
    app_adapter(app).fingerprint(provider)
}

fn cleanup_expired_import_sessions(state: &AppState) -> Result<(), AppError> {
    for secret_ref in state.db.expire_provider_import_sessions(now())? {
        remove_secret(state, &secret_ref)?;
    }
    Ok(())
}

fn normalized_import_url(value: &str) -> String {
    let trimmed = value.trim().trim_end_matches('/');
    match url::Url::parse(trimmed) {
        Ok(mut url) => {
            let _ = url.set_username("");
            let _ = url.set_password(None);
            url.set_query(None);
            url.set_fragment(None);
            url.to_string().trim_end_matches('/').to_ascii_lowercase()
        }
        Err(_) => trimmed.to_ascii_lowercase(),
    }
}

fn import_conflicts(
    definitions: &[ProviderDefinition],
    candidate: &ImportCandidate,
    fingerprint: &str,
) -> Vec<ImportConflict> {
    definitions
        .iter()
        .filter_map(|definition| {
            let mut reasons = Vec::new();
            if definition.source.as_ref().is_some_and(|source| {
                source.source_app == candidate.source_app
                    && source.source_ref == candidate.source_ref
            }) {
                reasons.push("sameSource".to_string());
            }
            if definition.protocol == candidate.protocol
                && normalized_import_url(&definition.base_url)
                    == normalized_import_url(&candidate.base_url)
            {
                reasons.push("sameEndpoint".to_string());
            }
            if definition
                .source
                .as_ref()
                .and_then(|source| source.source_fingerprint.as_deref())
                == Some(fingerprint)
            {
                reasons.push("sameFingerprint".to_string());
            }
            if definition
                .name
                .trim()
                .eq_ignore_ascii_case(candidate.name.trim())
            {
                reasons.push("sameName".to_string());
            }
            (!reasons.is_empty()).then(|| ImportConflict {
                existing_provider_id: definition.id.clone(),
                existing_name: definition.name.clone(),
                existing_revision: definition.revision,
                reasons,
            })
        })
        .collect()
}

fn candidate_state(record: &ProviderImportCandidateRecord) -> PersistedImportCandidateState {
    record
        .conflict_json
        .as_deref()
        .and_then(|text| serde_json::from_str(text).ok())
        .unwrap_or_default()
}

fn import_failure(
    app_type: impl Into<String>,
    source_ref: Option<String>,
    code: &str,
    stage: &str,
    message: impl Into<String>,
) -> ImportFailure {
    ImportFailure {
        app_type: app_type.into(),
        source_ref,
        code: code.to_string(),
        stage: stage.to_string(),
        message: message.into(),
    }
}

fn decode_session_errors(text: &str) -> Vec<ImportFailure> {
    if let Ok(errors) = serde_json::from_str::<Vec<ImportFailure>>(text) {
        return errors;
    }
    if let Ok(errors) = serde_json::from_str::<Vec<String>>(text) {
        return errors
            .into_iter()
            .map(|message| import_failure("unknown", None, "IMPORT_SCAN_FAILED", "scan", message))
            .collect();
    }
    vec![import_failure(
        "provider-center",
        None,
        "IMPORT_SESSION_ERRORS_CORRUPT",
        "decodeSession",
        "导入错误摘要已损坏",
    )]
}

pub fn start_import_session(
    state: &AppState,
    requested_apps: Vec<String>,
) -> Result<ProviderImportSession, AppError> {
    cleanup_expired_import_sessions(state)?;
    let requested = requested_apps
        .into_iter()
        .filter(|app| AppType::from_str(app).is_ok())
        .collect::<Vec<_>>();
    let requested_set = requested.iter().cloned().collect::<HashSet<_>>();
    let session_id = Uuid::new_v4().to_string();
    let created_at = now();
    let expires_at = created_at + 30 * 60 * 1_000;
    let (scanned, mut errors) = scan_imports_for_apps(state, &requested_set);
    let definitions = load_definitions(state)?;
    let mut candidates = Vec::new();
    let mut records = Vec::new();

    for mut candidate in scanned {
        let candidate_id = Uuid::new_v4().to_string();
        let (source_app_type, source) = match source_provider(state, &candidate.source_ref) {
            Ok((app, provider)) => (app, provider),
            Err(error) => {
                errors.push(import_failure(
                    candidate.source_app,
                    Some(candidate.source_ref),
                    "IMPORT_SOURCE_READ_FAILED",
                    "readSource",
                    error.to_string(),
                ));
                continue;
            }
        };
        let fingerprint = match import_candidate_fingerprint(&source_app_type, &source) {
            Ok(fingerprint) => fingerprint,
            Err(error) => {
                errors.push(import_failure(
                    candidate.source_app,
                    Some(candidate.source_ref),
                    "IMPORT_FINGERPRINT_FAILED",
                    "fingerprint",
                    error.to_string(),
                ));
                continue;
            }
        };
        let source_app = match AppType::from_str(&candidate.source_app) {
            Ok(app) => app,
            Err(error) => {
                errors.push(import_failure(
                    candidate.source_app,
                    Some(candidate.source_ref),
                    "IMPORT_APP_UNSUPPORTED",
                    "credentials",
                    error.to_string(),
                ));
                continue;
            }
        };
        let (_, api_key) = source.resolve_usage_credentials(&source_app);
        let temporary_secret_ref = if api_key.trim().is_empty() {
            None
        } else {
            let reference = format!("import:{session_id}:{candidate_id}");
            save_secret(state, &reference, api_key.trim())?;
            Some(reference)
        };
        candidate.id = candidate_id.clone();
        candidate.session_id = session_id.clone();
        candidate.conflicts = import_conflicts(&definitions, &candidate, &fingerprint);
        let persisted_state = PersistedImportCandidateState {
            conflicts: candidate.conflicts.clone(),
            ..Default::default()
        };
        records.push(ProviderImportCandidateRecord {
            id: candidate_id,
            session_id: session_id.clone(),
            source_app_type: candidate.source_app.clone(),
            source_provider_id: Some(candidate.source_ref.clone()),
            source_locator: Some(candidate.source_ref.clone()),
            normalized_json: serde_json::to_string(&candidate)
                .map_err(|error| AppError::Database(error.to_string()))?,
            models_json: serde_json::to_string(&candidate.models)
                .map_err(|error| AppError::Database(error.to_string()))?,
            temporary_secret_ref,
            credential_configured: candidate.credential_configured,
            fingerprint,
            conflict_json: Some(
                serde_json::to_string(&persisted_state)
                    .map_err(|error| AppError::Database(error.to_string()))?,
            ),
        });
        candidates.push(candidate);
    }

    let state_name = if errors.is_empty() {
        "ready"
    } else {
        "readyWithErrors"
    };
    state.db.save_provider_import_session(
        &ProviderImportSessionRecord {
            id: session_id.clone(),
            state: state_name.to_string(),
            requested_apps_json: serde_json::to_string(&requested)
                .map_err(|error| AppError::Database(error.to_string()))?,
            error_summary_json: serde_json::to_string(&errors)
                .map_err(|error| AppError::Database(error.to_string()))?,
            created_at,
            expires_at,
            completed_at: None,
        },
        &records,
    )?;
    Ok(ProviderImportSession {
        id: session_id,
        state: state_name.to_string(),
        candidates,
        errors,
        created_at,
        expires_at,
    })
}

pub fn get_import_session(
    state: &AppState,
    session_id: &str,
) -> Result<ProviderImportSession, AppError> {
    cleanup_expired_import_sessions(state)?;
    let session = state
        .db
        .get_provider_import_session(session_id)?
        .ok_or_else(|| AppError::Message("导入会话不存在".to_string()))?;
    if session.expires_at <= now() || session.state == "expired" {
        return Err(AppError::Message("导入会话已过期，请重新扫描".to_string()));
    }
    let mut errors = decode_session_errors(&session.error_summary_json);
    let mut candidates = Vec::new();
    for record in state.db.list_provider_import_candidates(session_id)? {
        let persisted = candidate_state(&record);
        if let Some(error) = persisted.quarantine {
            errors.push(error);
            continue;
        }
        if persisted.outcome.is_some() {
            continue;
        }
        match serde_json::from_str::<ImportCandidate>(&record.normalized_json) {
            Ok(mut candidate) => {
                candidate.conflicts = persisted.conflicts;
                candidates.push(candidate);
            }
            Err(_) => {
                let failure = import_failure(
                    record.source_app_type,
                    record.source_locator,
                    "IMPORT_CANDIDATE_CORRUPT",
                    "decodeCandidate",
                    "一个已保存的导入候选已损坏并被隔离",
                );
                let quarantine = PersistedImportCandidateState {
                    quarantine: Some(failure.clone()),
                    ..Default::default()
                };
                let state_json = serde_json::to_string(&quarantine)
                    .map_err(|error| AppError::Database(error.to_string()))?;
                state.db.update_provider_import_candidate_state(
                    session_id,
                    &record.id,
                    &state_json,
                    now(),
                )?;
                if let Some(reference) = record.temporary_secret_ref.as_deref() {
                    let _ = remove_secret(state, reference);
                }
                errors.push(failure);
            }
        }
    }
    Ok(ProviderImportSession {
        id: session.id,
        state: session.state,
        candidates,
        errors,
        created_at: session.created_at,
        expires_at: session.expires_at,
    })
}

pub fn commit_import_session_candidate(
    state: &AppState,
    session_id: &str,
    candidate_id: &str,
    app_types: Vec<String>,
    decision: ImportCommitDecision,
) -> Result<ImportCommitResult, AppError> {
    let session = state
        .db
        .get_provider_import_session(session_id)?
        .ok_or_else(|| AppError::Message("导入会话不存在".to_string()))?;
    if session.expires_at <= now() || session.state == "expired" {
        cleanup_expired_import_sessions(state)?;
        return Err(AppError::Message("导入会话已过期，请重新扫描".to_string()));
    }
    let record = state
        .db
        .get_provider_import_candidate(session_id, candidate_id)?
        .ok_or_else(|| AppError::Message("导入候选不存在".to_string()))?;
    let persisted = candidate_state(&record);
    if let Some(outcome) = persisted.outcome {
        let provider = outcome
            .provider_id
            .as_deref()
            .map(|id| {
                load_definitions(state)?
                    .into_iter()
                    .find(|item| item.id == id)
                    .ok_or_else(|| AppError::Message("已提交的导入结果不存在".to_string()))
            })
            .transpose()?;
        return Ok(ImportCommitResult {
            action: outcome.action,
            provider,
            repeated: true,
        });
    }
    if persisted.quarantine.is_some() {
        return Err(AppError::Message("导入候选已损坏并被隔离".to_string()));
    }
    let candidate: ImportCandidate = serde_json::from_str(&record.normalized_json)
        .map_err(|_| AppError::Message("导入候选已损坏并被隔离".to_string()))?;
    if decision.action == "skip" {
        let completed = PersistedImportCandidateState {
            conflicts: persisted.conflicts,
            outcome: Some(PersistedImportOutcome {
                action: "skip".to_string(),
                provider_id: None,
            }),
            quarantine: None,
        };
        state.db.update_provider_import_candidate_state(
            session_id,
            candidate_id,
            &serde_json::to_string(&completed)
                .map_err(|error| AppError::Database(error.to_string()))?,
            now(),
        )?;
        if let Some(reference) = record.temporary_secret_ref.as_deref() {
            remove_secret(state, reference)?;
        }
        return Ok(ImportCommitResult {
            action: "skip".to_string(),
            provider: None,
            repeated: false,
        });
    }
    if !matches!(decision.action.as_str(), "createCopy" | "merge") {
        return Err(AppError::Message("无效的导入冲突处理方式".to_string()));
    }
    let (source_app_type, current_source) = source_provider(state, &candidate.source_ref)?;
    if import_candidate_fingerprint(&source_app_type, &current_source)? != record.fingerprint {
        return Err(AppError::Message(
            "来源配置在扫描后发生变化，请重新扫描再导入".to_string(),
        ));
    }
    let api_key = record
        .temporary_secret_ref
        .as_deref()
        .map(|reference| get_secret(state, reference))
        .transpose()?;
    let (provider_id, expected_revision) = if decision.action == "merge" {
        let target = decision
            .target_provider_id
            .as_deref()
            .ok_or_else(|| AppError::Message("合并时必须选择目标模型服务".to_string()))?;
        if !persisted
            .conflicts
            .iter()
            .any(|conflict| conflict.existing_provider_id == target)
        {
            return Err(AppError::Message(
                "合并目标不在已确认的冲突列表中".to_string(),
            ));
        }
        let revision = decision.expected_revision.ok_or_else(|| {
            AppError::Message("合并时必须提供目标的 expectedRevision".to_string())
        })?;
        (target.to_string(), Some(revision))
    } else {
        (format!("import-{candidate_id}"), None)
    };
    let result = save_definition(
        state,
        SaveProviderDefinitionInput {
            id: Some(provider_id.clone()),
            name: candidate.name,
            protocol: candidate.protocol,
            base_url: candidate.base_url,
            models: candidate.models,
            notes: String::new(),
            enabled: Some(true),
            expected_revision,
            credential_action: if api_key.is_some() {
                Some("replace".to_string())
            } else if decision.action == "merge" {
                Some("keep".to_string())
            } else {
                None
            },
            source: Some(ProviderSource {
                source_app: candidate.source_app,
                source_ref: candidate.source_ref,
                imported_at: now(),
                source_fingerprint: Some(record.fingerprint.clone()),
                last_observed_at: Some(now()),
            }),
            api_key: api_key.clone(),
            app_types,
        },
    )?;
    let completed = PersistedImportCandidateState {
        conflicts: persisted.conflicts,
        outcome: Some(PersistedImportOutcome {
            action: decision.action.clone(),
            provider_id: Some(result.id.clone()),
        }),
        quarantine: None,
    };
    state.db.update_provider_import_candidate_state(
        session_id,
        candidate_id,
        &serde_json::to_string(&completed)
            .map_err(|error| AppError::Database(error.to_string()))?,
        now(),
    )?;
    if let Some(reference) = record.temporary_secret_ref.as_deref() {
        remove_secret(state, reference)?;
    }
    Ok(ImportCommitResult {
        action: decision.action,
        provider: Some(result),
        repeated: false,
    })
}

fn source_provider(state: &AppState, source_ref: &str) -> Result<(AppType, Provider), AppError> {
    if let Some(rest) = source_ref.strip_prefix("saved:") {
        if let Some((app, id)) = rest.split_once(':') {
            let app = AppType::from_str(app)?;
            let provider = state
                .db
                .get_provider_by_id(id, app.as_str())?
                .ok_or_else(|| AppError::Message("来源配置已不存在".to_string()))?;
            return Ok((app, provider));
        }
    }
    if let Some(app) = source_ref.strip_prefix("live:") {
        if !app.contains(':') {
            let app = AppType::from_str(app)?;
            let settings = app_adapter(&app).read_live_settings()?;
            return Ok((
                app.clone(),
                Provider::with_id(
                    "live".to_string(),
                    format!("{} 当前配置", app.as_str()),
                    settings,
                    None,
                ),
            ));
        }
    }
    Err(AppError::Message("无效的导入来源".to_string()))
}

pub fn import_candidate(
    state: &AppState,
    source_ref: &str,
    app_types: Vec<String>,
) -> Result<ProviderDefinition, AppError> {
    let (source_app, provider) = source_provider(state, source_ref)?;
    let (base_url, api_key) = provider.resolve_usage_credentials(&source_app);
    if base_url.trim().is_empty() {
        return Err(AppError::Message(
            "来源配置没有可导入的请求地址".to_string(),
        ));
    }
    let source_fingerprint = import_candidate_fingerprint(&source_app, &provider)?;
    let source_protocol = protocol_from_provider(&source_app, &provider);
    save_definition(
        state,
        SaveProviderDefinitionInput {
            id: None,
            name: provider.name,
            protocol: source_protocol,
            base_url,
            models: models_from_settings(&provider.settings_config),
            api_key: (!api_key.trim().is_empty()).then_some(api_key),
            app_types,
            notes: String::new(),
            enabled: Some(true),
            expected_revision: None,
            credential_action: None,
            source: Some(ProviderSource {
                source_app: source_app.as_str().to_string(),
                source_ref: source_ref.to_string(),
                imported_at: now(),
                source_fingerprint: Some(source_fingerprint),
                last_observed_at: Some(now()),
            }),
        },
    )
}

pub fn delete_definition(state: &AppState, provider_id: &str) -> Result<(), AppError> {
    let definitions = load_definitions(state)?;
    if !definitions.iter().any(|item| item.id == provider_id) {
        return Err(AppError::Message("模型服务不存在".to_string()));
    }
    state.db.delete_provider_center_definition(provider_id)?;
    remove_secret(state, provider_id)
}

pub fn duplicate_definition(
    state: &AppState,
    provider_id: &str,
) -> Result<ProviderDefinition, AppError> {
    let definitions = load_definitions(state)?;
    let source = definitions
        .iter()
        .find(|item| item.id == provider_id)
        .cloned()
        .ok_or_else(|| AppError::Message("模型服务不存在".to_string()))?;
    let secret = if source.credential_configured {
        Some(get_secret(state, provider_id)?)
    } else {
        None
    };
    save_definition(
        state,
        SaveProviderDefinitionInput {
            id: None,
            name: format!("{} 副本", source.name),
            protocol: source.protocol,
            base_url: source.base_url,
            models: source.models,
            notes: source.notes,
            enabled: Some(source.enabled),
            expected_revision: None,
            credential_action: secret.as_ref().map(|_| "replace".to_string()),
            source: source.source,
            api_key: secret,
            app_types: Vec::new(),
        },
    )
}

fn model_endpoint(base_url: &str, suffix: &str) -> String {
    let base = base_url.trim().trim_end_matches('/');
    if base.ends_with("/v1") && suffix.starts_with("/v1/") {
        format!("{base}{}", &suffix[3..])
    } else {
        format!("{base}{suffix}")
    }
}

fn model_ids(protocol: &str, body: &serde_json::Value) -> Vec<String> {
    let mut models = match protocol {
        "gemini" => body
            .get("models")
            .and_then(serde_json::Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(|item| item.get("name").and_then(serde_json::Value::as_str))
            .map(|name| name.strip_prefix("models/").unwrap_or(name).to_string())
            .collect::<Vec<_>>(),
        "ollama" => body
            .get("models")
            .and_then(serde_json::Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(|item| {
                item.get("name")
                    .or_else(|| item.get("model"))
                    .and_then(serde_json::Value::as_str)
            })
            .map(str::to_string)
            .collect::<Vec<_>>(),
        _ => body
            .get("data")
            .and_then(serde_json::Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(|item| item.get("id").and_then(serde_json::Value::as_str))
            .map(str::to_string)
            .collect::<Vec<_>>(),
    };
    models.retain(|model| !model.trim().is_empty());
    models.sort();
    models.dedup();
    models
}

pub async fn discover_models(
    state: &AppState,
    provider_id: &str,
) -> Result<ModelDiscoveryResult, AppError> {
    let definitions = load_definitions(state)?;
    let definition = definitions
        .iter()
        .find(|item| item.id == provider_id)
        .cloned()
        .ok_or_else(|| AppError::Message("模型服务不存在".to_string()))?;
    let secret = if definition.credential_configured {
        Some(get_secret(state, provider_id)?)
    } else {
        None
    };
    if definition.protocol != "ollama" && secret.is_none() {
        return Err(AppError::Message("请先配置 API Key".to_string()));
    }
    let endpoint = match definition.protocol.as_str() {
        "anthropic" | "openai-chat" | "openai-responses" => {
            model_endpoint(&definition.base_url, "/v1/models")
        }
        "gemini" => {
            let base = definition.base_url.trim().trim_end_matches('/');
            if base.ends_with("/v1") || base.ends_with("/v1beta") {
                format!("{base}/models")
            } else {
                format!("{base}/v1beta/models")
            }
        }
        "ollama" => model_endpoint(&definition.base_url, "/api/tags"),
        _ => unreachable!("protocol validated when the definition is saved"),
    };
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(15))
        .build()
        .map_err(|_| AppError::Message("无法初始化模型发现请求".to_string()))?;
    let mut request = client.get(endpoint);
    if let Some(secret) = secret.as_deref() {
        request = match definition.protocol.as_str() {
            "anthropic" => request
                .header("x-api-key", secret)
                .header("anthropic-version", "2023-06-01"),
            "gemini" => request.header("x-goog-api-key", secret),
            _ => request.bearer_auth(secret),
        };
    }
    let timestamp = now();
    let discovery = async {
        let response = request
            .send()
            .await
            .map_err(|_| AppError::Message("模型接口请求失败或超时".to_string()))?;
        if !response.status().is_success() {
            return Err(AppError::Message(format!(
                "模型接口返回 HTTP {}",
                response.status().as_u16()
            )));
        }
        let body: serde_json::Value = response
            .json()
            .await
            .map_err(|_| AppError::Message("模型接口返回了无法识别的数据".to_string()))?;
        let models = model_ids(&definition.protocol, &body);
        if models.is_empty() {
            return Err(AppError::Message("模型接口没有返回可用模型".to_string()));
        }
        Ok::<Vec<String>, AppError>(models)
    }
    .await;

    let mut definitions = load_definitions(state)?;
    let bindings = load_bindings(state)?;
    let current = definitions
        .iter_mut()
        .find(|item| item.id == provider_id)
        .ok_or_else(|| AppError::Message("模型服务已被删除".to_string()))?;
    current.last_discovery_at = Some(timestamp);
    let result = match discovery {
        Ok(models) => {
            current.discovered_models = models.clone();
            current.last_discovery_error = None;
            current.updated_at = timestamp;
            current.revision = current.revision.saturating_add(1);
            ModelDiscoveryResult {
                provider_id: provider_id.to_string(),
                models,
                discovered_at: timestamp,
                error: None,
            }
        }
        Err(error) => {
            let message = error.to_string();
            current.last_discovery_error = Some(message.clone());
            current.updated_at = timestamp;
            ModelDiscoveryResult {
                provider_id: provider_id.to_string(),
                models: current.discovered_models.clone(),
                discovered_at: timestamp,
                error: Some(message),
            }
        }
    };
    save_core(state, &definitions, &bindings)?;
    Ok(result)
}

fn projection(
    definition: &ProviderDefinition,
    secret: String,
    app: &str,
) -> Result<Provider, AppError> {
    AppAdapterRegistry::global()
        .resolve(app)?
        .render_projection(definition, &secret)
}

fn selected_targets(
    bindings: &[ProviderBinding],
    provider_id: &str,
    requested: Vec<String>,
) -> Vec<String> {
    let requested: HashSet<String> = requested.into_iter().collect();
    let mut targets = bindings
        .iter()
        .filter(|binding| {
            binding.provider_id == provider_id
                && binding.enabled
                && !binding.override_enabled
                && (requested.is_empty() || requested.contains(&binding.app_type))
        })
        .map(|binding| binding.app_type.clone())
        .collect::<Vec<_>>();
    targets.sort();
    targets.dedup();
    targets
}

pub fn apply_target_app_types(
    state: &AppState,
    provider_id: &str,
    requested: Vec<String>,
) -> Result<Vec<String>, AppError> {
    let bindings = load_bindings(state)?;
    Ok(selected_targets(&bindings, provider_id, requested))
}

pub fn restore_target_app_types(
    state: &AppState,
    transaction_id: &str,
) -> Result<Vec<String>, AppError> {
    let mut app_types = load_snapshots(state, transaction_id)?
        .into_iter()
        .map(|snapshot| snapshot.app_type)
        .collect::<Vec<_>>();
    app_types.sort();
    app_types.dedup();
    Ok(app_types)
}

fn preview_token(
    provider_id: &str,
    revision: u64,
    targets: &[ProviderApplyPreviewTarget],
) -> Result<String, AppError> {
    let payload = serde_json::to_vec(&(provider_id, revision, targets))
        .map_err(|error| AppError::Message(format!("无法生成预览令牌: {error}")))?;
    Ok(format!("{:x}", Sha256::digest(payload)))
}

pub fn preview_apply(
    state: &AppState,
    provider_id: &str,
    app_types: Vec<String>,
) -> Result<ProviderApplyPreview, AppError> {
    let definitions = load_definitions(state)?;
    let definition = definitions
        .iter()
        .find(|item| item.id == provider_id)
        .cloned()
        .ok_or_else(|| AppError::Message("模型服务不存在".to_string()))?;
    if !definition.enabled {
        return Err(AppError::Message("模型服务已停用，不能应用".to_string()));
    }
    let secret = if definition.protocol == "ollama" {
        String::new()
    } else {
        get_secret(state, provider_id)?
    };
    let bindings = load_bindings(state)?;
    let targets = selected_targets(&bindings, provider_id, app_types);
    if targets.is_empty() {
        return Err(AppError::Message("没有可应用的目标应用".to_string()));
    }
    let mut previews = Vec::new();
    for app_name in targets {
        let app = match AppType::from_str(&app_name) {
            Ok(app) => app,
            Err(_) => {
                previews.push(ProviderApplyPreviewTarget {
                    app_type: app_name,
                    operation: "unsupported".to_string(),
                    compatible: false,
                    drifted: false,
                    current_provider_id: None,
                    live_fingerprint: None,
                    message: Some("应用类型无效".to_string()),
                });
                continue;
            }
        };
        let projected = match projection(&definition, secret.clone(), &app_name) {
            Ok(provider) => provider,
            Err(error) => {
                previews.push(ProviderApplyPreviewTarget {
                    app_type: app_name,
                    operation: "unsupported".to_string(),
                    compatible: false,
                    drifted: false,
                    current_provider_id: None,
                    live_fingerprint: None,
                    message: Some(error.to_string()),
                });
                continue;
            }
        };
        let existing = state.db.get_provider_by_id(&projected.id, app.as_str())?;
        let current_fingerprint = existing.as_ref().map(provider_fingerprint).transpose()?;
        let expected = bindings
            .iter()
            .find(|binding| binding.provider_id == provider_id && binding.app_type == app_name)
            .and_then(|binding| binding.expected_fingerprint.as_ref());
        let drifted = expected.is_some() && expected != current_fingerprint.as_ref();
        let current_id = app_adapter(&app).current(state)?;
        previews.push(ProviderApplyPreviewTarget {
            app_type: app_name,
            operation: if existing.is_some() {
                "update"
            } else {
                "create"
            }
            .to_string(),
            compatible: true,
            drifted,
            current_provider_id: (!current_id.is_empty()).then_some(current_id),
            live_fingerprint: current_fingerprint,
            message: drifted
                .then(|| "目标配置已在 CC Switch 外部发生变化，需要先确认冲突".to_string()),
        });
    }
    let token = preview_token(provider_id, definition.revision, &previews)?;
    Ok(ProviderApplyPreview {
        token,
        provider_id: provider_id.to_string(),
        provider_revision: definition.revision,
        targets: previews,
        created_at: now(),
    })
}

fn save_transaction(
    state: &AppState,
    transaction: ProviderApplyTransaction,
) -> Result<(), AppError> {
    state.db.upsert_provider_center_transaction(&transaction)
}

#[cfg(target_os = "windows")]
fn save_snapshots(
    state: &AppState,
    transaction_id: &str,
    snapshots: &[ProjectionSnapshot],
) -> Result<(), AppError> {
    let serialized = serde_json::to_string(snapshots)
        .map_err(|error| AppError::Message(format!("无法创建恢复快照: {error}")))?;
    let mut all: EncryptedSnapshots = read_json(state, SNAPSHOTS_KEY)?;
    all.insert(transaction_id.to_string(), protect_secret(&serialized)?);
    write_json(state, SNAPSHOTS_KEY, &all)
}

#[cfg(target_os = "windows")]
fn load_snapshots(
    state: &AppState,
    transaction_id: &str,
) -> Result<Vec<ProjectionSnapshot>, AppError> {
    let all: EncryptedSnapshots = read_json(state, SNAPSHOTS_KEY)?;
    let encrypted = all
        .get(transaction_id)
        .ok_or_else(|| AppError::Message("恢复快照不存在".to_string()))?;
    let serialized = unprotect_secret(encrypted)?;
    serde_json::from_str(&serialized)
        .map_err(|error| AppError::Message(format!("恢复快照已损坏: {error}")))
}

#[cfg(not(target_os = "windows"))]
fn save_snapshots(
    state: &AppState,
    transaction_id: &str,
    snapshots: &[ProjectionSnapshot],
) -> Result<(), AppError> {
    let serialized = serde_json::to_string(snapshots)
        .map_err(|error| AppError::Message(format!("无法创建恢复快照: {error}")))?;
    let reference = format!("snapshot:{transaction_id}");
    save_secret(state, &reference, &serialized)?;
    let mut all: EncryptedSnapshots = read_json(state, SNAPSHOTS_KEY)?;
    all.insert(transaction_id.to_string(), reference);
    write_json(state, SNAPSHOTS_KEY, &all)
}

#[cfg(not(target_os = "windows"))]
fn load_snapshots(
    state: &AppState,
    transaction_id: &str,
) -> Result<Vec<ProjectionSnapshot>, AppError> {
    let all: EncryptedSnapshots = read_json(state, SNAPSHOTS_KEY)?;
    let reference = all
        .get(transaction_id)
        .ok_or_else(|| AppError::Message("恢复快照不存在".to_string()))?;
    let serialized = get_secret(state, reference)?;
    serde_json::from_str(&serialized)
        .map_err(|error| AppError::Message(format!("恢复快照已损坏: {error}")))
}

fn apply_projected_provider(
    state: &AppState,
    app: &str,
    provider: Provider,
) -> Result<(), AppError> {
    AppAdapterRegistry::global()
        .resolve(app)?
        .apply(state, provider)
}

fn restore_snapshot(state: &AppState, snapshot: &ProjectionSnapshot) -> Result<(), AppError> {
    let app = AppType::from_str(&snapshot.app_type)?;
    match snapshot.previous_projected_provider.clone() {
        Some(provider) => {
            let id = provider.id.clone();
            if state.db.get_provider_by_id(&id, app.as_str())?.is_some() {
                ProviderService::update(state, app.clone(), Some(&id), provider)?;
            } else {
                ProviderService::add(state, app.clone(), provider, true)?;
            }
        }
        None => {
            if let Some(previous) = snapshot.previous_current_provider_id.as_deref() {
                if state
                    .db
                    .get_provider_by_id(previous, app.as_str())?
                    .is_some()
                {
                    ProviderService::switch(state, app.clone(), previous)?;
                }
            } else if !app.is_additive_mode() {
                crate::settings::set_current_provider(&app, None)?;
            }
            if state
                .db
                .get_provider_by_id(&snapshot.projected_provider_id, app.as_str())?
                .is_some()
                && ProviderService::delete(state, app.clone(), &snapshot.projected_provider_id)
                    .is_err()
            {
                state
                    .db
                    .delete_provider(app.as_str(), &snapshot.projected_provider_id)?;
            }
        }
    }
    if let Some(previous) = snapshot.previous_current_provider_id.as_deref() {
        if state
            .db
            .get_provider_by_id(previous, app.as_str())?
            .is_some()
        {
            ProviderService::switch(state, app, previous)?;
        }
    } else if !app.is_additive_mode() {
        crate::settings::set_current_provider(&app, None)?;
        state.db.clear_current_provider(app.as_str())?;
    }
    Ok(())
}

pub fn apply_transaction(
    state: &AppState,
    provider_id: &str,
    app_types: Vec<String>,
    preview_token_value: &str,
    idempotency_key: Option<&str>,
) -> Result<ProviderApplyTransaction, AppError> {
    if let Some(key) = idempotency_key {
        if key.len() > 128
            || key.is_empty()
            || !key
                .chars()
                .all(|character| character.is_ascii_alphanumeric() || "-_:".contains(character))
        {
            return Err(AppError::Message("无效的幂等请求标识".to_string()));
        }
        if let Some(existing) = state.db.get_provider_center_transaction(key)? {
            return Ok(existing);
        }
    }
    let preview = preview_apply(state, provider_id, app_types.clone())?;
    if preview.token != preview_token_value {
        return Err(AppError::Message("应用预览已过期，请重新预览".to_string()));
    }
    if preview
        .targets
        .iter()
        .any(|target| !target.compatible || target.drifted)
    {
        return Err(AppError::Message(
            "存在不兼容或已被外部修改的目标，未执行任何写入".to_string(),
        ));
    }
    let definitions = load_definitions(state)?;
    let definition = definitions
        .iter()
        .find(|item| item.id == provider_id)
        .cloned()
        .ok_or_else(|| AppError::Message("模型服务不存在".to_string()))?;
    let secret = if definition.protocol == "ollama" {
        String::new()
    } else {
        get_secret(state, provider_id)?
    };
    let transaction_id = idempotency_key
        .map(str::to_string)
        .unwrap_or_else(|| Uuid::new_v4().to_string());
    let mut transaction = ProviderApplyTransaction {
        id: transaction_id.clone(),
        provider_id: provider_id.to_string(),
        provider_revision: definition.revision,
        status: "running".to_string(),
        targets: preview
            .targets
            .iter()
            .map(|target| ProviderApplyTargetResult {
                app_type: target.app_type.clone(),
                status: "pending".to_string(),
                message: None,
            })
            .collect(),
        created_at: now(),
        completed_at: None,
    };
    let mut snapshots = Vec::new();
    for target in &preview.targets {
        let app = AppType::from_str(&target.app_type)?;
        let projected = projection(&definition, secret.clone(), &target.app_type)?;
        let previous = state.db.get_provider_by_id(&projected.id, app.as_str())?;
        let before_fingerprint = previous.as_ref().map(provider_fingerprint).transpose()?;
        let current = ProviderService::current(state, app)?;
        snapshots.push(ProjectionSnapshot {
            app_type: target.app_type.clone(),
            projected_provider_id: projected.id.clone(),
            previous_projected_provider: previous,
            previous_current_provider_id: (!current.is_empty()).then_some(current),
            before_fingerprint,
            after_fingerprint: provider_fingerprint(&projected)?,
        });
    }
    save_snapshots(state, &transaction_id, &snapshots)?;
    save_transaction(state, transaction.clone())?;

    let mut bindings = load_bindings(state)?;
    let mut succeeded = Vec::new();
    let mut failed = false;
    let mut failed_index = None;
    for (index, snapshot) in snapshots.iter().enumerate() {
        let adapter = AppAdapterRegistry::global().resolve(&snapshot.app_type)?;
        let projected = projection(&definition, secret.clone(), &snapshot.app_type)?;
        let result =
            apply_projected_provider(state, &snapshot.app_type, projected).and_then(|_| {
                let actual = state
                    .db
                    .get_provider_by_id(&snapshot.projected_provider_id, adapter.app_id())?
                    .ok_or_else(|| AppError::Message("写入后未找到目标配置".to_string()))?;
                if provider_fingerprint(&actual)? != snapshot.after_fingerprint {
                    return Err(AppError::Message("写入后配置校验失败".to_string()));
                }
                Ok(())
            });
        match result {
            Ok(()) => {
                transaction.targets[index].status = "applied".to_string();
                succeeded.push(index);
                if let Some(binding) = bindings.iter_mut().find(|binding| {
                    binding.provider_id == provider_id && binding.app_type == snapshot.app_type
                }) {
                    binding.status = "applied".to_string();
                    binding.applied_revision = Some(definition.revision);
                    binding.expected_fingerprint = Some(snapshot.after_fingerprint.clone());
                    binding.last_error = None;
                    binding.last_transaction_id = Some(transaction_id.clone());
                    binding.updated_at = now();
                }
            }
            Err(error) => {
                let message = error.to_string();
                transaction.targets[index].status = "failed".to_string();
                transaction.targets[index].message = Some(message.clone());
                if let Some(binding) = bindings.iter_mut().find(|binding| {
                    binding.provider_id == provider_id && binding.app_type == snapshot.app_type
                }) {
                    binding.status = "failed".to_string();
                    binding.last_error = Some(message);
                    binding.last_transaction_id = Some(transaction_id.clone());
                    binding.updated_at = now();
                }
                failed = true;
                failed_index = Some(index);
                break;
            }
        }
    }
    if failed {
        let mut rollback_failed = false;
        let mut rollback_indices = succeeded;
        if let Some(index) = failed_index {
            rollback_indices.push(index);
        }
        rollback_indices.sort_unstable();
        rollback_indices.dedup();
        for index in rollback_indices.into_iter().rev() {
            let original_message = transaction.targets[index].message.clone();
            match restore_snapshot(state, &snapshots[index]) {
                Ok(()) => {
                    transaction.targets[index].status = "rolled_back".to_string();
                    transaction.targets[index].message = original_message;
                    if let Some(binding) = bindings.iter_mut().find(|binding| {
                        binding.provider_id == provider_id
                            && binding.app_type == snapshots[index].app_type
                    }) {
                        binding.status = "pending".to_string();
                        binding.expected_fingerprint = snapshots[index].before_fingerprint.clone();
                    }
                }
                Err(error) => {
                    rollback_failed = true;
                    transaction.targets[index].status = "rollbackFailed".to_string();
                    transaction.targets[index].message = Some(match original_message {
                        Some(original) => format!("{original}；回滚失败：{error}"),
                        None => error.to_string(),
                    });
                }
            }
        }
        transaction.status = if rollback_failed {
            "recoveryRequired".to_string()
        } else {
            "rolled_back".to_string()
        };
    } else {
        transaction.status = "applied".to_string();
    }
    transaction.completed_at = Some(now());
    save_core(state, &definitions, &bindings)?;
    save_transaction(state, transaction.clone())?;
    Ok(transaction)
}

pub fn restore_transaction(
    state: &AppState,
    transaction_id: &str,
) -> Result<ProviderApplyTransaction, AppError> {
    let snapshots = load_snapshots(state, transaction_id)?;
    let source = state
        .db
        .get_provider_center_transaction(transaction_id)?
        .ok_or_else(|| AppError::Message("应用历史不存在".to_string()))?;
    let restore_id = Uuid::new_v4().to_string();
    // A restore may itself be interrupted or partially fail. Persist an
    // equivalent snapshot under the new transaction id so the recovery action
    // shown for that record remains executable after a restart.
    save_snapshots(state, &restore_id, &snapshots)?;
    let mut result = ProviderApplyTransaction {
        id: restore_id.clone(),
        provider_id: source.provider_id.clone(),
        provider_revision: source.provider_revision,
        status: "restoring".to_string(),
        targets: Vec::new(),
        created_at: now(),
        completed_at: None,
    };
    let mut failed = false;
    for snapshot in snapshots.iter().rev() {
        match restore_snapshot(state, snapshot) {
            Ok(()) => result.targets.push(ProviderApplyTargetResult {
                app_type: snapshot.app_type.clone(),
                status: "restored".to_string(),
                message: None,
            }),
            Err(error) => {
                failed = true;
                result.targets.push(ProviderApplyTargetResult {
                    app_type: snapshot.app_type.clone(),
                    status: "restoreFailed".to_string(),
                    message: Some(error.to_string()),
                });
            }
        }
    }
    let definitions = load_definitions(state)?;
    let mut bindings = load_bindings(state)?;
    for binding in bindings
        .iter_mut()
        .filter(|binding| binding.provider_id == source.provider_id)
    {
        if snapshots
            .iter()
            .any(|item| item.app_type == binding.app_type)
        {
            binding.status = if binding.enabled {
                "pending"
            } else {
                "detached"
            }
            .to_string();
            binding.applied_revision = None;
            binding.last_transaction_id = Some(restore_id.clone());
            binding.updated_at = now();
        }
    }
    save_core(state, &definitions, &bindings)?;
    result.status = if failed {
        "recoveryRequired"
    } else {
        "restored"
    }
    .to_string();
    result.completed_at = Some(now());
    save_transaction(state, result.clone())?;
    Ok(result)
}

pub fn apply_bindings(
    state: &AppState,
    provider_id: &str,
    app_types: Vec<String>,
) -> Result<Vec<ProviderBinding>, AppError> {
    // Compatibility entry point for older frontends. It uses the same
    // preview/token/snapshot/rollback path as the transaction API so callers
    // cannot bypass transactional safety.
    let preview = preview_apply(state, provider_id, app_types)?;
    let transaction = apply_transaction(
        state,
        provider_id,
        preview
            .targets
            .iter()
            .map(|target| target.app_type.clone())
            .collect(),
        &preview.token,
        None,
    )?;
    if transaction.status != "applied" {
        return Err(AppError::Message(format!(
            "共享配置应用未完成，事务状态：{}",
            transaction.status
        )));
    }
    let bindings = load_bindings(state)?;
    Ok(bindings
        .into_iter()
        .filter(|item| item.provider_id == provider_id)
        .collect())
}

pub fn set_binding_override(
    state: &AppState,
    provider_id: &str,
    app_type: &str,
    enabled: bool,
) -> Result<(), AppError> {
    let definitions = load_definitions(state)?;
    let mut bindings = load_bindings(state)?;
    let binding = bindings
        .iter_mut()
        .find(|item| item.provider_id == provider_id && item.app_type == app_type)
        .ok_or_else(|| AppError::Message("绑定关系不存在".to_string()))?;
    binding.override_enabled = enabled;
    binding.status = if enabled {
        "overridden".to_string()
    } else {
        "pending".to_string()
    };
    binding.updated_at = now();
    save_core(state, &definitions, &bindings)
}

pub fn disable_binding(
    state: &AppState,
    provider_id: &str,
    app_type: &str,
    remove_projection: bool,
) -> Result<(), AppError> {
    let definitions = load_definitions(state)?;
    let definition = definitions
        .iter()
        .find(|definition| definition.id == provider_id)
        .ok_or_else(|| AppError::Message("模型服务不存在".to_string()))?;
    let mut bindings = load_bindings(state)?;
    let binding = bindings
        .iter_mut()
        .find(|binding| binding.provider_id == provider_id && binding.app_type == app_type)
        .ok_or_else(|| AppError::Message("绑定关系不存在".to_string()))?;
    if remove_projection {
        let app = AppType::from_str(app_type)?;
        let projected_id = projected_provider_id(definition, app_type);
        if state
            .db
            .get_provider_by_id(&projected_id, app.as_str())?
            .is_some()
        {
            app_adapter(&app).delete(state, &projected_id)?;
        }
    }
    binding.enabled = false;
    binding.override_enabled = false;
    binding.status = "detached".to_string();
    binding.applied_revision = None;
    binding.expected_fingerprint = None;
    binding.last_error = None;
    binding.updated_at = now();
    save_core(state, &definitions, &bindings)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::provider::{ClaudeModelConfig, CodexModelConfig, GeminiModelConfig};
    use serde_json::json;

    fn definition(protocol: &str) -> ProviderDefinition {
        ProviderDefinition {
            id: "shared".to_string(),
            name: "Shared".to_string(),
            protocol: protocol.to_string(),
            base_url: "https://api.example.test/v1".to_string(),
            models: vec!["model-a".to_string()],
            discovered_models: Vec::new(),
            notes: String::new(),
            enabled: true,
            revision: 1,
            source: None,
            credential_configured: true,
            credential_hint: Some("…1234".to_string()),
            last_discovery_at: None,
            last_discovery_error: None,
            created_at: 1,
            updated_at: 1,
        }
    }

    #[test]
    fn rollback_status_serializes_with_frontend_contract() {
        let transaction = ProviderApplyTransaction {
            id: "tx-rollback".to_string(),
            provider_id: "shared".to_string(),
            provider_revision: 1,
            status: "rolled_back".to_string(),
            targets: vec![ProviderApplyTargetResult {
                app_type: "codex".to_string(),
                status: "rolled_back".to_string(),
                message: None,
            }],
            created_at: 1,
            completed_at: Some(2),
        };

        let json = serde_json::to_value(transaction).expect("serialize rollback transaction");
        assert_eq!(json["status"], "rolled_back");
        assert_eq!(json["targets"][0]["status"], "rolled_back");
        assert!(!json.to_string().contains("rolledBack"));
    }

    #[test]
    fn provider_definition_dto_never_contains_plaintext_secret_or_secret_reference() {
        let json = serde_json::to_string(&definition("openai-chat")).unwrap();
        assert!(!json.contains("sk-secret"));
        assert!(!json.contains("secretRef"));
        assert!(json.contains("credentialConfigured"));
    }

    #[test]
    fn registered_app_projections_are_deterministic() {
        for app in [
            "claude",
            "codex",
            "gemini",
            "grokbuild",
            "opencode",
            "openclaw",
            "hermes",
            "pi",
        ] {
            let protocol = match app {
                "claude" => "anthropic",
                "codex" => "openai-responses",
                "gemini" => "gemini",
                _ => "openai-chat",
            };
            let first = projection(&definition(protocol), "sk-secret".to_string(), app)
                .unwrap_or_else(|error| panic!("{app}: {error}"));
            let second = projection(&definition(protocol), "sk-secret".to_string(), app)
                .unwrap_or_else(|error| panic!("{app}: {error}"));
            assert_eq!(
                provider_fingerprint(&first).unwrap(),
                provider_fingerprint(&second).unwrap()
            );
            assert_eq!(first.id, format!("provider-center-{app}-shared"));
        }
    }

    #[test]
    fn projections_enforce_protocol_compatibility_and_render_native_api_names() {
        assert!(projection(&definition("openai-chat"), "sk-secret".to_string(), "codex").is_err());
        assert!(projection(
            &definition("openai-responses"),
            "sk-secret".to_string(),
            "codex"
        )
        .is_ok());
        assert!(projection(&definition("anthropic"), "sk-secret".to_string(), "claude").is_ok());
        assert!(projection(&definition("gemini"), "sk-secret".to_string(), "claude").is_err());

        let openclaw = projection(
            &definition("anthropic"),
            "sk-secret".to_string(),
            "openclaw",
        )
        .expect("OpenClaw supports Anthropic Messages");
        assert_eq!(
            openclaw.settings_config["api"].as_str(),
            Some("anthropic-messages")
        );

        let mut ollama = definition("ollama");
        ollama.base_url = "http://127.0.0.1:11434".to_string();
        let pi = projection(&ollama, String::new(), "pi").expect("Pi supports Ollama");
        assert_eq!(
            pi.settings_config["models"][0]["api"].as_str(),
            Some("openai-completions")
        );
        assert_eq!(
            pi.settings_config["models"][0]["baseUrl"].as_str(),
            Some("http://127.0.0.1:11434/v1")
        );
    }

    #[test]
    fn provider_save_rejects_unknown_protocols_and_credential_bearing_urls() {
        use crate::database::Database;

        let state = AppState::new(Arc::new(Database::memory().expect("memory database")));
        let input = |protocol: &str, base_url: &str| SaveProviderDefinitionInput {
            id: None,
            expected_revision: None,
            name: "Example".to_string(),
            protocol: protocol.to_string(),
            base_url: base_url.to_string(),
            models: Vec::new(),
            notes: String::new(),
            enabled: Some(true),
            credential_action: Some("keep".to_string()),
            source: None,
            api_key: None,
            app_types: Vec::new(),
        };

        assert!(save_definition(&state, input("unknown", "https://api.example.test")).is_err());
        assert!(save_definition(
            &state,
            input("openai-chat", "https://user:secret@api.example.test/v1")
        )
        .is_err());
        assert!(save_definition(
            &state,
            input("openai-chat", "https://api.example.test/v1?api_key=secret")
        )
        .is_err());
        assert!(save_definition(&state, input("ollama", "http://127.0.0.1:11434")).is_ok());
    }

    #[test]
    fn import_protocol_is_inferred_from_each_apps_native_configuration() {
        let codex = Provider::with_id(
            "codex-source".to_string(),
            "Codex".to_string(),
            json!({
                "config": "model_provider = \"custom\"\n[model_providers.custom]\nbase_url = \"https://api.example/v1\"\nwire_api = \"responses\"\n"
            }),
            None,
        );
        assert_eq!(
            protocol_from_provider(&AppType::Codex, &codex),
            "openai-responses"
        );

        let openclaw = Provider::with_id(
            "openclaw-source".to_string(),
            "OpenClaw".to_string(),
            json!({ "api": "anthropic-messages" }),
            None,
        );
        assert_eq!(
            protocol_from_provider(&AppType::OpenClaw, &openclaw),
            "anthropic"
        );

        let pi = Provider::with_id(
            "pi-source".to_string(),
            "Pi".to_string(),
            json!({ "models": [{ "id": "gemini", "api": "google-generative-ai" }] }),
            None,
        );
        assert_eq!(protocol_from_provider(&AppType::Pi, &pi), "gemini");
    }

    #[test]
    fn claude_desktop_rejects_incompatible_protocols() {
        assert!(projection(
            &definition("openai-chat"),
            "sk-secret".to_string(),
            "claude-desktop"
        )
        .is_err());
        assert!(projection(
            &definition("anthropic"),
            "sk-secret".to_string(),
            "claude-desktop"
        )
        .is_ok());
    }

    fn legacy_universal() -> UniversalProvider {
        let mut provider = UniversalProvider::new(
            "legacy-shared".to_string(),
            "Legacy Shared".to_string(),
            "openai-chat".to_string(),
            "https://legacy.example/v1/".to_string(),
            "sk-legacy-secret".to_string(),
        );
        provider.created_at = Some(1234);
        provider.notes = Some("migrated note".to_string());
        provider.apps = crate::provider::UniversalProviderApps {
            claude: true,
            codex: true,
            gemini: false,
        };
        provider.models = crate::provider::UniversalProviderModels {
            claude: Some(ClaudeModelConfig {
                model: Some("claude-main".to_string()),
                haiku_model: Some("claude-haiku".to_string()),
                sonnet_model: Some("claude-main".to_string()),
                opus_model: None,
            }),
            codex: Some(CodexModelConfig {
                model: Some("gpt-codex".to_string()),
                reasoning_effort: Some("high".to_string()),
            }),
            gemini: Some(GeminiModelConfig {
                model: Some("gemini-pro".to_string()),
            }),
        };
        provider
    }

    #[test]
    fn legacy_universal_conversion_preserves_models_bindings_and_source() {
        let legacy = legacy_universal();
        let (definition, bindings) =
            convert_legacy_universal(&legacy, "migrated-id".to_string(), 1234);

        assert_eq!(definition.id, "migrated-id");
        assert_eq!(definition.base_url, "https://legacy.example/v1");
        assert_eq!(
            definition.models,
            vec!["claude-haiku", "claude-main", "gemini-pro", "gpt-codex"]
        );
        assert_eq!(definition.credential_hint.as_deref(), Some("…cret"));
        assert_eq!(
            definition
                .source
                .as_ref()
                .map(|source| source.source_ref.as_str()),
            Some("universal:legacy-shared")
        );
        assert_eq!(
            bindings
                .iter()
                .map(|binding| binding.app_type.as_str())
                .collect::<Vec<_>>(),
            vec!["claude", "codex"]
        );
        assert!(bindings.iter().all(|binding| binding.status == "pending"));
    }

    #[cfg(target_os = "windows")]
    #[test]
    fn legacy_universal_migration_encrypts_key_deletes_plaintext_and_is_idempotent() {
        use crate::database::Database;

        let db = Arc::new(Database::memory().expect("create memory database"));
        let state = AppState::new(db.clone());
        let legacy = legacy_universal();
        db.save_universal_provider(&legacy)
            .expect("seed plaintext legacy provider");

        ensure_sqlite_migrated(&state).expect("migrate legacy provider");

        let definitions = db
            .load_provider_center_definitions()
            .expect("load migrated definitions");
        let migrated_id = migrated_legacy_universal_id(&definitions, &legacy.id)
            .expect("migrated definition must retain source identity");
        assert_eq!(get_secret(&state, &migrated_id).unwrap(), legacy.api_key);
        assert!(db.get_all_universal_providers().unwrap().is_empty());
        let legacy_json = db
            .get_setting("universal_providers")
            .unwrap()
            .unwrap_or_default();
        assert!(!legacy_json.contains(&legacy.api_key));

        let definition_count = definitions.len();
        let binding_count = db.load_provider_center_bindings().unwrap().len();
        db.save_universal_provider(&legacy)
            .expect("simulate cleanup interruption");
        db.set_setting(SQLITE_MIGRATED_KEY, "false")
            .expect("force migration retry");

        ensure_sqlite_migrated(&state).expect("retry migration");

        assert_eq!(
            db.load_provider_center_definitions().unwrap().len(),
            definition_count,
            "retry must not duplicate the definition"
        );
        assert_eq!(
            db.load_provider_center_bindings().unwrap().len(),
            binding_count,
            "retry must not duplicate bindings"
        );
        assert!(db.get_all_universal_providers().unwrap().is_empty());
    }

    #[cfg(target_os = "windows")]
    #[test]
    fn import_session_copies_source_hides_key_and_creates_pending_bindings() {
        use crate::database::Database;

        let db = Arc::new(Database::memory().expect("create memory database"));
        let state = AppState::new(db.clone());
        let source = Provider::with_id(
            "source-codex".to_string(),
            "Imported Codex".to_string(),
            json!({
                "auth": { "OPENAI_API_KEY": "sk-import-secret" },
                "config": "model = \"gpt-5\"\nmodel_provider = \"custom\"\n[model_providers.custom]\nbase_url = \"https://import.example/v1\"\n"
            }),
            None,
        );
        db.save_provider("codex", &source)
            .expect("seed source provider");
        let source_before = db
            .get_provider_by_id("source-codex", "codex")
            .unwrap()
            .expect("source exists");

        let session =
            start_import_session(&state, vec!["codex".to_string()]).expect("start import session");
        let candidate = session
            .candidates
            .iter()
            .find(|candidate| candidate.source_ref == "saved:codex:source-codex")
            .expect("saved provider candidate")
            .clone();
        assert_eq!(candidate.credential_hint.as_deref(), Some("…cret"));
        let record = db
            .list_provider_import_candidates(&session.id)
            .unwrap()
            .into_iter()
            .find(|record| record.id == candidate.id)
            .expect("candidate record");
        let temporary_secret_ref = record
            .temporary_secret_ref
            .clone()
            .expect("temporary secret reference");
        assert!(!record.normalized_json.contains("sk-import-secret"));

        let imported = commit_import_session_candidate(
            &state,
            &session.id,
            &candidate.id,
            vec!["codex".to_string(), "openclaw".to_string()],
            ImportCommitDecision {
                action: "createCopy".to_string(),
                target_provider_id: None,
                expected_revision: None,
            },
        )
        .expect("commit import")
        .provider
        .expect("createCopy returns the imported definition");

        assert_eq!(imported.id, format!("import-{}", candidate.id));
        assert_eq!(imported.models, vec!["gpt-5"]);
        assert_eq!(
            get_secret(&state, &imported.id).unwrap(),
            "sk-import-secret"
        );
        assert!(get_secret(&state, &temporary_secret_ref).is_err());
        let source_after = db
            .get_provider_by_id("source-codex", "codex")
            .unwrap()
            .expect("source remains");
        assert_eq!(
            serde_json::to_value(source_after).unwrap(),
            serde_json::to_value(source_before).unwrap(),
            "import must not modify the source provider"
        );
        let bindings = db.load_provider_center_bindings().unwrap();
        assert_eq!(
            bindings
                .iter()
                .filter(|binding| binding.provider_id == imported.id)
                .map(|binding| binding.app_type.as_str())
                .collect::<Vec<_>>(),
            vec!["codex", "openclaw"]
        );
        assert!(bindings
            .iter()
            .filter(|binding| binding.provider_id == imported.id)
            .all(|binding| binding.status == "pending"));
    }
}
