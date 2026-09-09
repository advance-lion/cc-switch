//! 增量式「模型服务中心」。
//!
//! 此模块刻意不替换既有 providers 表或 ProviderService：它只保存一个不含
//! 明文密钥的共享定义和绑定关系；用户明确点击「应用」后，才投影为原有的
//! 应用级 Provider。这样原有的单应用自定义仍然是唯一的最终写入路径。

use crate::app_config::AppType;
use crate::error::AppError;
use crate::provider::{
    ClaudeModelConfig, CodexModelConfig, GeminiModelConfig, Provider, UniversalProvider,
};
use crate::services::ProviderService;
use crate::store::AppState;
use base64::{engine::general_purpose::STANDARD as BASE64, Engine as _};
use serde::{Deserialize, Serialize};
use serde_json::json;
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

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ImportCandidate {
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

/// Serializes Provider Center writes that touch the same application.
/// Multi-app operations acquire locks in sorted order to avoid deadlocks.
#[derive(Default)]
pub struct ProviderCenterOperationState {
    locks: RwLock<HashMap<String, Arc<Mutex<()>>>>,
}

impl ProviderCenterOperationState {
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

#[cfg(not(target_os = "windows"))]
fn protect_secret(_: &str) -> Result<String, AppError> {
    Err(AppError::Message(
        "当前平台尚未接入系统安全存储".to_string(),
    ))
}
#[cfg(not(target_os = "windows"))]
fn unprotect_secret(_: &str) -> Result<String, AppError> {
    Err(AppError::Message(
        "当前平台尚未接入系统安全存储".to_string(),
    ))
}

fn save_secret(state: &AppState, id: &str, secret: &str) -> Result<(), AppError> {
    let mut secrets: EncryptedSecrets = read_json(state, SECRETS_KEY)?;
    secrets.insert(id.to_string(), protect_secret(secret)?);
    write_json(state, SECRETS_KEY, &secrets)
}

fn remove_secret(state: &AppState, id: &str) -> Result<(), AppError> {
    let mut secrets: EncryptedSecrets = read_json(state, SECRETS_KEY)?;
    secrets.remove(id);
    write_json(state, SECRETS_KEY, &secrets)
}

fn secret_metadata(state: &AppState, id: &str) -> Result<(bool, Option<String>), AppError> {
    let secrets: EncryptedSecrets = read_json(state, SECRETS_KEY)?;
    let Some(blob) = secrets.get(id) else {
        return Ok((false, None));
    };
    let secret = unprotect_secret(blob)?;
    Ok((true, secret_hint(&secret)))
}

fn get_secret(state: &AppState, id: &str) -> Result<String, AppError> {
    let secrets: EncryptedSecrets = read_json(state, SECRETS_KEY)?;
    let blob = secrets
        .get(id)
        .ok_or_else(|| AppError::Message("该模型服务没有可用的 API Key".to_string()))?;
    unprotect_secret(blob)
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
        let actual = state
            .db
            .get_provider_by_id(&projected.id, app.as_str())?;
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
    let definitions: Definitions = read_json(state, DEFINITIONS_KEY)?;
    let mut bindings: Bindings = read_json(state, BINDINGS_KEY)?;
    if refresh_binding_states(state, &definitions, &mut bindings)? {
        write_json(state, BINDINGS_KEY, &bindings)?;
    }
    Ok(ProviderCenterState {
        definitions,
        bindings,
        transactions: read_json(state, TRANSACTIONS_KEY)?,
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
    let definitions: Definitions = read_json(state, DEFINITIONS_KEY)?;
    let bindings: Bindings = read_json(state, BINDINGS_KEY)?;
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
        let Some(actual) = state
            .db
            .get_provider_by_id(&projected.id, app.as_str())?
        else {
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
                id: format!("api-key:{}:{}:{}", binding.app_type, definition.id, model_id),
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
        let current = ProviderService::current(state, app.clone()).unwrap_or_default();
        let providers = match ProviderService::list(state, app.clone()) {
            Ok(providers) => providers,
            Err(_) => continue,
        };
        for provider in providers.values().filter(|provider| {
            provider.category.as_deref() == Some("official")
                || provider.uses_managed_account_auth()
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
        (&left.app_type, &left.source_type, &left.provider_name, &left.model_id).cmp(&(
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
    let mut transactions: Vec<ProviderApplyTransaction> = read_json(state, TRANSACTIONS_KEY)?;
    let mut changed = 0;
    for transaction in &mut transactions {
        if matches!(transaction.status.as_str(), "running" | "restoring") {
            transaction.status = "recoveryRequired".to_string();
            transaction.completed_at.get_or_insert_with(now);
            changed += 1;
        }
    }
    if changed > 0 {
        write_json(state, TRANSACTIONS_KEY, &transactions)?;
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
    let mut definitions: Definitions = read_json(state, DEFINITIONS_KEY)?;
    let mut bindings: Bindings = read_json(state, BINDINGS_KEY)?;
    let timestamp = now();
    let id = input.id.clone().unwrap_or_else(|| Uuid::new_v4().to_string());
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
        if input.api_key.as_ref().is_some_and(|key| !key.trim().is_empty()) {
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
        protocol: if input.protocol.trim().is_empty() {
            "openai-chat".to_string()
        } else {
            input.protocol.trim().to_string()
        },
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
    write_json(state, DEFINITIONS_KEY, &definitions)?;
    write_json(state, BINDINGS_KEY, &bindings)?;
    Ok(definition)
}

fn models_from_settings(settings: &serde_json::Value) -> Vec<String> {
    let mut models = Vec::new();
    for pointer in [
        "/env/ANTHROPIC_MODEL",
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
    models.sort();
    models.dedup();
    models
}

fn protocol_for(app: &AppType) -> &'static str {
    match app {
        AppType::Claude | AppType::ClaudeDesktop => "anthropic",
        AppType::Gemini => "gemini",
        _ => "openai-chat",
    }
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
        source_ref,
        source_app: app.as_str().to_string(),
        name: provider.name.clone(),
        protocol: protocol_for(app).to_string(),
        base_url,
        models: models_from_settings(&provider.settings_config),
        credential_configured: !key.trim().is_empty(),
        credential_hint: secret_hint(&key),
    })
}

pub fn scan_imports(state: &AppState) -> Result<Vec<ImportCandidate>, AppError> {
    let mut candidates = Vec::new();
    for app in AppType::all() {
        // 一个应用的损坏配置不能遮蔽其他应用的可导入结果。
        if let Ok(providers) = ProviderService::list(state, app.clone()) {
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
        if let Ok(settings) = ProviderService::read_live_settings(app.clone()) {
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
    }
    candidates.sort_by(|left, right| left.source_ref.cmp(&right.source_ref));
    candidates.dedup_by(|left, right| left.source_ref == right.source_ref);
    Ok(candidates)
}

fn source_provider(state: &AppState, source_ref: &str) -> Result<(AppType, Provider), AppError> {
    let parts: Vec<&str> = source_ref.split(':').collect();
    match parts.as_slice() {
        ["saved", app, id] => {
            let app = AppType::from_str(app)?;
            let provider = state
                .db
                .get_provider_by_id(id, app.as_str())?
                .ok_or_else(|| AppError::Message("来源配置已不存在".to_string()))?;
            Ok((app, provider))
        }
        ["live", app] => {
            let app = AppType::from_str(app)?;
            let settings = ProviderService::read_live_settings(app.clone())?;
            Ok((
                app.clone(),
                Provider::with_id(
                    "live".to_string(),
                    format!("{} 当前配置", app.as_str()),
                    settings,
                    None,
                ),
            ))
        }
        _ => Err(AppError::Message("无效的导入来源".to_string())),
    }
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
    save_definition(
        state,
        SaveProviderDefinitionInput {
            id: None,
            name: provider.name,
            protocol: protocol_for(&source_app).to_string(),
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
            }),
        },
    )
}

pub fn delete_definition(state: &AppState, provider_id: &str) -> Result<(), AppError> {
    let mut definitions: Definitions = read_json(state, DEFINITIONS_KEY)?;
    let before = definitions.len();
    definitions.retain(|item| item.id != provider_id);
    if definitions.len() == before {
        return Err(AppError::Message("模型服务不存在".to_string()));
    }
    let mut bindings: Bindings = read_json(state, BINDINGS_KEY)?;
    bindings.retain(|item| item.provider_id != provider_id);
    remove_secret(state, provider_id)?;
    write_json(state, DEFINITIONS_KEY, &definitions)?;
    write_json(state, BINDINGS_KEY, &bindings)
}

pub fn duplicate_definition(
    state: &AppState,
    provider_id: &str,
) -> Result<ProviderDefinition, AppError> {
    let definitions: Definitions = read_json(state, DEFINITIONS_KEY)?;
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
    let definitions: Definitions = read_json(state, DEFINITIONS_KEY)?;
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
        "anthropic" => model_endpoint(&definition.base_url, "/v1/models"),
        "gemini" => model_endpoint(&definition.base_url, "/models"),
        "ollama" => model_endpoint(&definition.base_url, "/api/tags"),
        _ => model_endpoint(&definition.base_url, "/models"),
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

    let mut definitions: Definitions = read_json(state, DEFINITIONS_KEY)?;
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
    write_json(state, DEFINITIONS_KEY, &definitions)?;
    Ok(result)
}

fn toml_string(value: &str) -> String {
    value.replace('\\', "\\\\").replace('"', "\\\"")
}

fn projection(
    definition: &ProviderDefinition,
    secret: String,
    app: &str,
) -> Result<Provider, AppError> {
    let mut universal = UniversalProvider::new(
        format!("pc-{}", definition.id),
        definition.name.clone(),
        definition.protocol.clone(),
        definition.base_url.clone(),
        secret.clone(),
    );
    let models = if definition.models.is_empty() {
        vec!["default".to_string()]
    } else {
        definition.models.clone()
    };
    let model = models.first().cloned();
    let mut provider = match app {
        "claude" => {
            universal.apps.claude = true;
            universal.models.claude = Some(ClaudeModelConfig {
                model,
                ..Default::default()
            });
            universal.to_claude_provider()
        }
        "codex" => {
            universal.apps.codex = true;
            universal.models.codex = Some(CodexModelConfig {
                model,
                reasoning_effort: Some("medium".to_string()),
            });
            universal.to_codex_provider()
        }
        "gemini" => {
            universal.apps.gemini = true;
            universal.models.gemini = Some(GeminiModelConfig { model });
            universal.to_gemini_provider()
        }
        "claude-desktop" => {
            if definition.protocol != "anthropic" {
                return Err(AppError::Message("Claude Desktop 直连仅支持 Anthropic 协议；OpenAI 兼容服务请保留单应用配置或等待网关适配".to_string()));
            }
            Some(Provider::with_id(String::new(), definition.name.clone(), json!({ "env": { "ANTHROPIC_BASE_URL": definition.base_url, "ANTHROPIC_AUTH_TOKEN": secret } }), None))
        }
        "grokbuild" => {
            let model = model.unwrap_or_else(|| "gpt-4o".to_string());
            let model_key = "shared";
            let config = format!(
                "[models]\ndefault = \"{model_key}\"\n\n[model.{model_key}]\nmodel = \"{}\"\nbase_url = \"{}\"\napi_key = \"{}\"\nname = \"{}\"\napi_backend = \"openai\"\ncontext_window = 128000\n",
                toml_string(&model), toml_string(&definition.base_url), toml_string(&secret), toml_string(&definition.name)
            );
            Some(Provider::with_id(String::new(), definition.name.clone(), json!({ "config": config }), None))
        }
        "opencode" => Some(Provider::with_id(String::new(), definition.name.clone(), json!({
            "npm": "@ai-sdk/openai-compatible", "name": definition.name,
            "options": { "baseURL": definition.base_url, "apiKey": secret },
            "models": models.iter().map(|item| (item.clone(), json!({ "name": item }))).collect::<serde_json::Map<String, serde_json::Value>>()
        }), None)),
        "openclaw" => Some(Provider::with_id(String::new(), definition.name.clone(), json!({
            "baseUrl": definition.base_url, "apiKey": secret, "api": "openai-completions",
            "models": models.iter().map(|item| json!({ "id": item, "name": item })).collect::<Vec<_>>()
        }), None)),
        "hermes" => Some(Provider::with_id(String::new(), definition.name.clone(), json!({
            "base_url": definition.base_url, "api_key": secret,
            "models": models.iter().map(|item| (item.clone(), json!({}))).collect::<serde_json::Map<String, serde_json::Value>>()
        }), None)),
        "pi" => Some(Provider::with_id(String::new(), definition.name.clone(), json!({
            "apiKey": secret,
            "models": models.iter().map(|item| json!({ "id": item, "name": item, "api": "openai-completions", "baseUrl": definition.base_url })).collect::<Vec<_>>()
        }), None)),
        _ => None,
    }.ok_or_else(|| AppError::Message("该应用尚未有安全的配置适配器".to_string()))?;
    provider.id = format!("provider-center-{app}-{}", definition.id);
    provider.category = Some("provider-center".to_string());
    Ok(provider)
}

fn provider_fingerprint(provider: &Provider) -> Result<String, AppError> {
    let bytes = serde_json::to_vec(provider)
        .map_err(|error| AppError::Message(format!("无法计算配置指纹: {error}")))?;
    Ok(format!("{:x}", Sha256::digest(bytes)))
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
    let bindings: Bindings = read_json(state, BINDINGS_KEY)?;
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
    let definitions: Definitions = read_json(state, DEFINITIONS_KEY)?;
    let definition = definitions
        .iter()
        .find(|item| item.id == provider_id)
        .cloned()
        .ok_or_else(|| AppError::Message("模型服务不存在".to_string()))?;
    if !definition.enabled {
        return Err(AppError::Message("模型服务已停用，不能应用".to_string()));
    }
    let secret = get_secret(state, provider_id)?;
    let bindings: Bindings = read_json(state, BINDINGS_KEY)?;
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
                    message: Some(error.to_string()),
                });
                continue;
            }
        };
        let existing = state.db.get_provider_by_id(&projected.id, app.as_str())?;
        let current_fingerprint = existing
            .as_ref()
            .map(provider_fingerprint)
            .transpose()?;
        let expected = bindings
            .iter()
            .find(|binding| binding.provider_id == provider_id && binding.app_type == app_name)
            .and_then(|binding| binding.expected_fingerprint.as_ref());
        let drifted = expected.is_some() && expected != current_fingerprint.as_ref();
        let current_id = ProviderService::current(state, app.clone())?;
        previews.push(ProviderApplyPreviewTarget {
            app_type: app_name,
            operation: if existing.is_some() { "update" } else { "create" }.to_string(),
            compatible: true,
            drifted,
            current_provider_id: (!current_id.is_empty()).then_some(current_id),
            message: drifted.then(|| "目标配置已在 CC Switch 外部发生变化，需要先确认冲突".to_string()),
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
    let mut transactions: Vec<ProviderApplyTransaction> = read_json(state, TRANSACTIONS_KEY)?;
    if let Some(index) = transactions.iter().position(|item| item.id == transaction.id) {
        transactions[index] = transaction;
    } else {
        transactions.insert(0, transaction);
        transactions.truncate(100);
    }
    write_json(state, TRANSACTIONS_KEY, &transactions)
}

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

fn apply_projected_provider(
    state: &AppState,
    app: AppType,
    provider: Provider,
) -> Result<(), AppError> {
    let id = provider.id.clone();
    if state.db.get_provider_by_id(&id, app.as_str())?.is_some() {
        ProviderService::update(state, app.clone(), Some(&id), provider)?;
    } else {
        ProviderService::add(state, app.clone(), provider, true)?;
    }
    if !app.is_additive_mode() {
        ProviderService::switch(state, app, &id)?;
    }
    Ok(())
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
                if state.db.get_provider_by_id(previous, app.as_str())?.is_some() {
                    ProviderService::switch(state, app.clone(), previous)?;
                }
            } else if !app.is_additive_mode() {
                crate::settings::set_current_provider(&app, None)?;
            }
            if state
                .db
                .get_provider_by_id(&snapshot.projected_provider_id, app.as_str())?
                .is_some()
            {
                if ProviderService::delete(state, app.clone(), &snapshot.projected_provider_id)
                    .is_err()
                {
                    state
                        .db
                        .delete_provider(app.as_str(), &snapshot.projected_provider_id)?;
                }
            }
        }
    }
    if let Some(previous) = snapshot.previous_current_provider_id.as_deref() {
        if state.db.get_provider_by_id(previous, app.as_str())?.is_some() {
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
) -> Result<ProviderApplyTransaction, AppError> {
    let preview = preview_apply(state, provider_id, app_types.clone())?;
    if preview.token != preview_token_value {
        return Err(AppError::Message("应用预览已过期，请重新预览".to_string()));
    }
    if preview.targets.iter().any(|target| !target.compatible || target.drifted) {
        return Err(AppError::Message(
            "存在不兼容或已被外部修改的目标，未执行任何写入".to_string(),
        ));
    }
    let definitions: Definitions = read_json(state, DEFINITIONS_KEY)?;
    let definition = definitions
        .iter()
        .find(|item| item.id == provider_id)
        .cloned()
        .ok_or_else(|| AppError::Message("模型服务不存在".to_string()))?;
    let secret = get_secret(state, provider_id)?;
    let transaction_id = Uuid::new_v4().to_string();
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
    save_transaction(state, transaction.clone())?;

    let mut snapshots = Vec::new();
    for target in &preview.targets {
        let app = AppType::from_str(&target.app_type)?;
        let projected = projection(&definition, secret.clone(), &target.app_type)?;
        let previous = state.db.get_provider_by_id(&projected.id, app.as_str())?;
        let before_fingerprint = previous
            .as_ref()
            .map(provider_fingerprint)
            .transpose()?;
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

    let mut bindings: Bindings = read_json(state, BINDINGS_KEY)?;
    let mut succeeded = Vec::new();
    let mut failed = false;
    for (index, snapshot) in snapshots.iter().enumerate() {
        let app = AppType::from_str(&snapshot.app_type)?;
        let projected = projection(&definition, secret.clone(), &snapshot.app_type)?;
        let result = apply_projected_provider(state, app.clone(), projected).and_then(|_| {
            let actual = state
                .db
                .get_provider_by_id(&snapshot.projected_provider_id, app.as_str())?
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
                break;
            }
        }
    }
    if failed {
        let mut rollback_failed = false;
        for index in succeeded.into_iter().rev() {
            match restore_snapshot(state, &snapshots[index]) {
                Ok(()) => {
                    transaction.targets[index].status = "rolledBack".to_string();
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
                    transaction.targets[index].message = Some(error.to_string());
                }
            }
        }
        transaction.status = if rollback_failed {
            "recoveryRequired".to_string()
        } else {
            "rolledBack".to_string()
        };
    } else {
        transaction.status = "applied".to_string();
    }
    transaction.completed_at = Some(now());
    write_json(state, BINDINGS_KEY, &bindings)?;
    save_transaction(state, transaction.clone())?;
    Ok(transaction)
}

pub fn restore_transaction(
    state: &AppState,
    transaction_id: &str,
) -> Result<ProviderApplyTransaction, AppError> {
    let snapshots = load_snapshots(state, transaction_id)?;
    let transactions: Vec<ProviderApplyTransaction> = read_json(state, TRANSACTIONS_KEY)?;
    let source = transactions
        .iter()
        .find(|item| item.id == transaction_id)
        .cloned()
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
    let mut bindings: Bindings = read_json(state, BINDINGS_KEY)?;
    for binding in bindings
        .iter_mut()
        .filter(|binding| binding.provider_id == source.provider_id)
    {
        if snapshots.iter().any(|item| item.app_type == binding.app_type) {
            binding.status = if binding.enabled { "pending" } else { "detached" }.to_string();
            binding.applied_revision = None;
            binding.last_transaction_id = Some(restore_id.clone());
            binding.updated_at = now();
        }
    }
    write_json(state, BINDINGS_KEY, &bindings)?;
    result.status = if failed { "recoveryRequired" } else { "restored" }.to_string();
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
    )?;
    if transaction.status != "applied" {
        return Err(AppError::Message(format!(
            "共享配置应用未完成，事务状态：{}",
            transaction.status
        )));
    }
    let bindings: Bindings = read_json(state, BINDINGS_KEY)?;
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
    let mut bindings: Bindings = read_json(state, BINDINGS_KEY)?;
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
    write_json(state, BINDINGS_KEY, &bindings)
}
