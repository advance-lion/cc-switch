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
use crate::proxy::providers::capabilities::Compatibility;
use crate::secure_store::{self, SecretScope};
use crate::services::ProviderService;
use crate::store::AppState;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::{HashMap, HashSet};
use std::str::FromStr;
use std::sync::Arc;
use tokio::sync::{Mutex, OwnedMutexGuard, RwLock};
use uuid::Uuid;

const DEFINITIONS_KEY: &str = "provider_center_definitions_v1";
const BINDINGS_KEY: &str = "provider_center_bindings_v1";
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

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ProviderModelDefinition {
    pub id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub display_name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub context_window: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_output_tokens: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reasoning: Option<bool>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub input_modalities: Vec<String>,
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
    pub model_definitions: Vec<ProviderModelDefinition>,
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
    #[serde(default)]
    pub sources: Vec<ProviderSource>,
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
    /// Credential-redacted app-specific Provider template. It is persisted in
    /// the binding overrides but never serialized over IPC.
    #[serde(default, skip_serializing)]
    pub provider_template: Option<Provider>,
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

/// One application-specific Provider annotated with its source and mutation
/// capabilities. Only display metadata is returned; provider credentials and
/// application settings remain outside the Provider Center IPC response.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentProviderCatalogItem {
    pub provider_id: String,
    pub provider_name: String,
    pub category: Option<String>,
    /// universal | agentOnly | nativeAccount
    pub scope: String,
    /// providerCenterProjection | ccSwitchManaged | agentNative
    pub ownership: String,
    pub definition_id: Option<String>,
    pub binding_status: Option<String>,
    pub applied_revision: Option<u64>,
    pub drifted: bool,
    pub read_only: bool,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentProviderCatalog {
    pub app_type: String,
    pub items: Vec<AgentProviderCatalogItem>,
    pub generated_at: i64,
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
    pub model_definitions: Vec<ProviderModelDefinition>,
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

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ManagedProviderDraftInput {
    pub app_type: String,
    pub provider: Provider,
    pub definition_id: String,
    #[serde(default)]
    pub expected_revision: Option<u64>,
    /// Agents to save the universal provider to. If empty, defaults to
    /// `[app_type]` (current agent only).
    #[serde(default)]
    pub target_app_types: Vec<String>,
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

fn default_import_source_kind() -> String {
    "localManaged".to_string()
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
    /// externalManaged | providerCenterProjection | localManaged
    #[serde(default = "default_import_source_kind")]
    pub source_kind: String,
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
    /// direct | proxy | unsupported
    pub connection_mode: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub route_id: Option<String>,
    pub requires_takeover: bool,
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
type EncryptedSnapshots = HashMap<String, String>;

fn provider_sources(definition: &ProviderDefinition) -> Vec<ProviderSource> {
    let mut sources = definition.sources.clone();
    if let Some(source) = definition.source.clone() {
        if !sources.iter().any(|item| {
            item.source_app == source.source_app && item.source_ref == source.source_ref
        }) {
            sources.insert(0, source);
        }
    }
    sources.sort_by(|left, right| {
        left.imported_at
            .cmp(&right.imported_at)
            .then_with(|| left.source_app.cmp(&right.source_app))
            .then_with(|| left.source_ref.cmp(&right.source_ref))
    });
    sources.dedup_by(|left, right| {
        left.source_app == right.source_app && left.source_ref == right.source_ref
    });
    sources
}

fn merge_provider_sources(
    existing: Option<&ProviderDefinition>,
    incoming: Option<ProviderSource>,
) -> Vec<ProviderSource> {
    let mut sources = existing.map(provider_sources).unwrap_or_default();
    if let Some(incoming) = incoming {
        if let Some(index) = sources.iter().position(|source| {
            source.source_app == incoming.source_app && source.source_ref == incoming.source_ref
        }) {
            sources[index] = incoming;
        } else {
            sources.push(incoming);
        }
    }
    sources.sort_by(|left, right| {
        left.imported_at
            .cmp(&right.imported_at)
            .then_with(|| left.source_app.cmp(&right.source_app))
            .then_with(|| left.source_ref.cmp(&right.source_ref))
    });
    sources
}

fn primary_source(
    existing: Option<&ProviderDefinition>,
    sources: &[ProviderSource],
) -> Option<ProviderSource> {
    existing
        .and_then(|definition| definition.source.clone())
        .or_else(|| sources.first().cloned())
}

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
            _ => "needs-review".to_string(),
        },
        base_url: universal.base_url.trim_end_matches('/').to_string(),
        models,
        model_definitions: Vec::new(),
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
        sources: vec![ProviderSource {
            source_app: "cc-switch".to_string(),
            source_ref: legacy_universal_source_ref(&universal.id),
            imported_at: timestamp,
            source_fingerprint: None,
            last_observed_at: None,
        }],
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
        provider_template: None,
        applied_revision: None,
        expected_fingerprint: None,
        last_error: None,
        last_transaction_id: None,
        updated_at: timestamp,
    })
    .collect();
    (definition, bindings)
}

pub fn ensure_sqlite_migrated(state: &AppState) -> Result<(), AppError> {
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
    secure_store::secret_hint(secret)
}

fn save_secret(state: &AppState, id: &str, secret: &str) -> Result<(), AppError> {
    secure_store::save(state, SecretScope::ProviderCenter, id, secret)
}

fn remove_secret(state: &AppState, id: &str) -> Result<(), AppError> {
    secure_store::remove(state, SecretScope::ProviderCenter, id)
}

fn secret_metadata(state: &AppState, id: &str) -> Result<(bool, Option<String>), AppError> {
    secure_store::metadata(state, SecretScope::ProviderCenter, id)
}

fn get_secret(state: &AppState, id: &str) -> Result<String, AppError> {
    secure_store::get(state, SecretScope::ProviderCenter, id)
}

fn restore_secret(state: &AppState, id: &str, previous: Option<&str>) -> Result<(), AppError> {
    secure_store::restore(state, SecretScope::ProviderCenter, id, previous)
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

fn is_agent_native_provider(provider: &Provider) -> bool {
    crate::database::is_official_seed_id(&provider.id)
}

fn classify_agent_provider(
    provider: Provider,
    projection: Option<(&ProviderDefinition, &ProviderBinding)>,
) -> AgentProviderCatalogItem {
    if let Some((definition, binding)) = projection {
        return AgentProviderCatalogItem {
            provider_id: provider.id,
            provider_name: provider.name,
            category: provider.category,
            scope: "universal".to_string(),
            ownership: "providerCenterProjection".to_string(),
            definition_id: Some(definition.id.clone()),
            binding_status: Some(binding.status.clone()),
            applied_revision: binding.applied_revision,
            drifted: binding.status == "drifted",
            read_only: true,
        };
    }

    let agent_native = is_agent_native_provider(&provider);
    let native_account = agent_native || provider.uses_managed_account_auth();
    AgentProviderCatalogItem {
        provider_id: provider.id,
        provider_name: provider.name,
        category: provider.category,
        scope: if native_account {
            "nativeAccount".to_string()
        } else {
            "agentOnly".to_string()
        },
        ownership: if agent_native {
            "agentNative".to_string()
        } else {
            "ccSwitchManaged".to_string()
        },
        definition_id: None,
        binding_status: None,
        applied_revision: None,
        drifted: false,
        read_only: false,
    }
}

/// Return the existing Provider list for one agent, annotated with Provider
/// Center binding state and native-account ownership/capability metadata.
pub fn agent_provider_catalog(
    state: &AppState,
    app_type: &str,
) -> Result<AgentProviderCatalog, AppError> {
    let app = AppType::from_str(app_type)?;
    let app_type = app.as_str().to_string();
    let definitions = load_definitions(state)?;
    let mut bindings = load_bindings(state)?;
    if refresh_binding_states(state, &definitions, &mut bindings)? {
        save_core(state, &definitions, &bindings)?;
    }

    let projection_metadata = definitions
        .iter()
        .filter_map(|definition| {
            let binding = bindings.iter().find(|binding| {
                binding.provider_id == definition.id
                    && binding.app_type == app_type
                    && binding.enabled
                    && binding.status != "detached"
            })?;
            Some((
                projected_provider_id(definition, &app_type),
                (definition, binding),
            ))
        })
        .collect::<HashMap<_, _>>();

    let items = app_adapter(&app)
        .list(state)?
        .into_values()
        .map(|provider| {
            let projection = projection_metadata.get(&provider.id).copied();
            classify_agent_provider(provider, projection)
        })
        .collect();

    Ok(AgentProviderCatalog {
        app_type,
        items,
        generated_at: now(),
    })
}

/// Ordinary Provider mutations must not bypass Provider Center ownership.
/// Provider Center's own apply/restore paths call ProviderService directly and
/// therefore remain able to maintain their projections.
pub fn guard_projection_mutation(
    state: &AppState,
    app_type: &AppType,
    provider_id: &str,
) -> Result<(), AppError> {
    let category_marks_projection = state
        .db
        .get_provider_by_id(provider_id, app_type.as_str())?
        .is_some_and(|provider| provider.category.as_deref() == Some("provider-center"));
    let definitions = state.db.load_provider_center_definitions()?;
    let binding_marks_projection =
        state
            .db
            .load_provider_center_bindings()?
            .iter()
            .any(|binding| {
                binding.app_type == app_type.as_str()
                    && binding.enabled
                    && binding.status != "detached"
                    && definitions.iter().any(|definition| {
                        definition.id == binding.provider_id
                            && projected_provider_id(definition, app_type.as_str()) == provider_id
                    })
            });
    if category_marks_projection || binding_marks_projection {
        return Err(AppError::Message(
            "Provider Center projection cannot be edited or deleted directly; manage it in Provider Center."
                .to_string(),
        ));
    }
    Ok(())
}

/// Resolve a Provider Center secret only for an active projection whose
/// definition, binding, application and projected provider id all agree.
pub(crate) fn projection_secret_for_provider(
    state: &AppState,
    app_type: &AppType,
    provider: &Provider,
    definition_id: &str,
) -> Result<String, AppError> {
    let definition = state
        .db
        .load_provider_center_definitions()?
        .into_iter()
        .find(|definition| definition.id == definition_id)
        .ok_or_else(|| AppError::Message("Provider Center definition 不存在".to_string()))?;
    let owns_projection = provider.category.as_deref() == Some("provider-center")
        && projected_provider_id(&definition, app_type.as_str()) == provider.id
        && state
            .db
            .load_provider_center_bindings()?
            .iter()
            .any(|binding| {
                binding.provider_id == definition_id
                    && binding.app_type == app_type.as_str()
                    && binding.enabled
                    && binding.status != "detached"
            });
    if !owns_projection {
        return Err(AppError::Message(
            "DSH Provider Center credentialRef 未通过投影所有权校验".to_string(),
        ));
    }
    get_secret(state, definition_id)
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
    let selected_apps: HashSet<String> = input
        .app_types
        .iter()
        .filter_map(|app| AppType::from_str(app).ok())
        .map(|app| app.as_str().to_string())
        .collect();
    let removed_apps = bindings
        .iter()
        .filter(|binding| {
            binding.provider_id == id
                && binding.enabled
                && !selected_apps.contains(&binding.app_type)
        })
        .map(|binding| binding.app_type.clone())
        .collect::<Vec<_>>();
    if !removed_apps.is_empty() {
        return Err(AppError::Message(format!(
            "不能通过保存模型服务解除已有绑定（{}）；请先显式选择移除投射或保留本地副本并解除绑定",
            removed_apps.join(", ")
        )));
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
    let credential_changed = matches!(credential_action, "replace" | "clear");
    let previous_secret = if credential_changed
        && existing
            .as_ref()
            .is_some_and(|definition| definition.credential_configured)
    {
        Some(get_secret(state, &id)?)
    } else {
        None
    };
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
    let sources = merge_provider_sources(existing.as_ref(), input.source.clone());
    let source = primary_source(existing.as_ref(), &sources);
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
        model_definitions: if !input.model_definitions.is_empty() {
            input.model_definitions.clone()
        } else {
            existing
                .as_ref()
                .map(|item| item.model_definitions.clone())
                .unwrap_or_default()
        },
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
        source,
        sources,
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
                provider_template: None,
                applied_revision: None,
                expected_fingerprint: None,
                last_error: None,
                last_transaction_id: None,
                updated_at: timestamp,
            });
        }
    }
    if let Err(error) = save_core(state, &definitions, &bindings) {
        if credential_changed {
            if let Err(restore_error) = restore_secret(state, &id, previous_secret.as_deref()) {
                return Err(AppError::Message(format!(
                    "保存模型服务失败: {error}；恢复 API Key 失败: {restore_error}"
                )));
            }
        }
        return Err(error);
    }
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

/// Extract normalized model definitions (ID + metadata) from a source
/// provider's `settings_config`. Unlike `models_from_settings`, this
/// preserves source order and captures display name, context window,
/// max output tokens, reasoning capability, and input modalities where
/// the source format provides them.
fn model_definitions_from_settings(settings: &serde_json::Value) -> Vec<ProviderModelDefinition> {
    let mut seen: HashSet<String> = HashSet::new();
    let mut defs: Vec<ProviderModelDefinition> = Vec::new();

    fn extract_meta(
        meta: Option<&serde_json::Map<String, serde_json::Value>>,
    ) -> (
        Option<String>,
        Option<u64>,
        Option<u64>,
        Option<bool>,
        Vec<String>,
    ) {
        let meta = match meta {
            Some(m) => m,
            None => {
                return (None, None, None, None, Vec::new());
            }
        };
        let display_name = meta
            .get("name")
            .or_else(|| meta.get("displayName"))
            .and_then(serde_json::Value::as_str)
            .map(str::to_string);
        let context_window = meta
            .get("context_window")
            .or_else(|| meta.get("contextWindow"))
            .or_else(|| meta.get("context_length"))
            .and_then(serde_json::Value::as_u64);
        let max_output_tokens = meta
            .get("max_tokens")
            .or_else(|| meta.get("maxTokens"))
            .or_else(|| meta.get("max_output_tokens"))
            .and_then(serde_json::Value::as_u64);
        let reasoning = meta.get("reasoning").and_then(serde_json::Value::as_bool);
        let input_modalities = meta
            .get("input")
            .or_else(|| meta.get("inputModalities"))
            .and_then(serde_json::Value::as_array)
            .map(|arr| {
                arr.iter()
                    .filter_map(|v| v.as_str().map(str::to_string))
                    .collect()
            })
            .unwrap_or_default();
        (
            display_name,
            context_window,
            max_output_tokens,
            reasoning,
            input_modalities,
        )
    }

    fn push_def(
        id: &str,
        meta: Option<&serde_json::Map<String, serde_json::Value>>,
        seen: &mut HashSet<String>,
        defs: &mut Vec<ProviderModelDefinition>,
    ) {
        let id = id.trim();
        if id.is_empty() || seen.contains(id) {
            return;
        }
        seen.insert(id.to_string());
        let (display_name, context_window, max_output_tokens, reasoning, input_modalities) =
            extract_meta(meta);
        defs.push(ProviderModelDefinition {
            id: id.to_string(),
            display_name,
            context_window,
            max_output_tokens,
            reasoning,
            input_modalities,
        });
    }

    // Env-var style pointers (Claude, Gemini) — IDs only, no metadata.
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
            push_def(model, None, &mut seen, &mut defs);
        }
    }

    // models as object map (OpenCode, Hermes) — keys are IDs, values carry metadata.
    if let Some(serde_json::Value::Object(items)) = settings.get("models") {
        for (id, value) in items {
            let meta = match value {
                serde_json::Value::Object(map) => Some(map),
                _ => None,
            };
            push_def(id, meta, &mut seen, &mut defs);
        }
    }

    // models as array (OpenClaw, Pi) — each item has id/model/name + metadata.
    if let Some(serde_json::Value::Array(items)) = settings.get("models") {
        for item in items {
            let id = item
                .get("id")
                .or_else(|| item.get("model"))
                .or_else(|| item.get("name"))
                .and_then(serde_json::Value::as_str);
            let meta = item.as_object();
            if let Some(id) = id {
                push_def(id, meta, &mut seen, &mut defs);
            }
        }
    }

    // Codex modelCatalog.models array — rich metadata per model.
    if let Some(catalog) = settings.get("modelCatalog").and_then(|v| v.get("models")) {
        if let Some(items) = catalog.as_array() {
            for item in items {
                if let Some(id) = item.get("model").and_then(serde_json::Value::as_str) {
                    push_def(id, item.as_object(), &mut seen, &mut defs);
                }
            }
        }
    }

    // TOML config string (Codex, GrokBuild) — extract model IDs and context_window.
    if let Some(config) = settings.get("config").and_then(serde_json::Value::as_str) {
        if let Ok(value) = toml::from_str::<toml::Value>(config) {
            fn collect_toml_models(
                value: &toml::Value,
                seen: &mut HashSet<String>,
                defs: &mut Vec<ProviderModelDefinition>,
            ) {
                match value {
                    toml::Value::Table(table) => {
                        for (key, value) in table {
                            if matches!(key.as_str(), "model" | "default_model" | "default") {
                                if let Some(model) = value.as_str() {
                                    push_def(model, None, seen, defs);
                                }
                            }
                            // GrokBuild [model.<key>] tables carry context_window.
                            if let toml::Value::Table(inner) = value {
                                if let Some(toml::Value::String(id)) = inner.get("model") {
                                    let mut meta = serde_json::Map::new();
                                    if let Some(cw) = inner
                                        .get("context_window")
                                        .and_then(toml::Value::as_integer)
                                    {
                                        meta.insert(
                                            "context_window".to_string(),
                                            serde_json::json!(cw),
                                        );
                                    }
                                    push_def(id, Some(&meta), seen, defs);
                                }
                            }
                            collect_toml_models(value, seen, defs);
                        }
                    }
                    toml::Value::Array(items) => {
                        for item in items {
                            collect_toml_models(item, seen, defs);
                        }
                    }
                    _ => {}
                }
            }
            collect_toml_models(&value, &mut seen, &mut defs);
        }
    }

    if defs.is_empty() {
        // Fall back to the old extraction to avoid losing model IDs entirely.
        defs = models_from_settings(settings)
            .into_iter()
            .map(|id| ProviderModelDefinition {
                id,
                ..Default::default()
            })
            .collect();
    }
    defs
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
    source_kind: &str,
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
        source_kind: source_kind.to_string(),
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
) -> Result<(Vec<ImportCandidate>, Vec<ImportFailure>), AppError> {
    let definitions = load_definitions(state)?;
    let bindings = load_bindings(state)?;
    let mut candidates = Vec::new();
    let mut failures = Vec::new();
    for app in AppType::all() {
        if !requested_apps.is_empty() && !requested_apps.contains(app.as_str()) {
            continue;
        }
        let projection_ids = definitions
            .iter()
            .filter(|definition| {
                bindings.iter().any(|binding| {
                    binding.provider_id == definition.id
                        && binding.app_type == app.as_str()
                        && binding.enabled
                        && binding.status != "detached"
                })
            })
            .map(|definition| projected_provider_id(definition, app.as_str()))
            .collect::<HashSet<_>>();
        match app_adapter(&app).list_for_scan(state) {
            Ok(providers) => {
                for provider in providers.values() {
                    if provider.category.as_deref() == Some("provider-center")
                        || projection_ids.contains(&provider.id)
                    {
                        continue;
                    }
                    if let Some(candidate) = candidate_from_provider(
                        format!("saved:{}:{}", app.as_str(), provider.id),
                        "localManaged",
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
                let current_is_projection = app_adapter(&app)
                    .current(state)
                    .ok()
                    .is_some_and(|id| projection_ids.contains(&id));
                let fingerprint_is_projection = import_candidate_fingerprint(&app, &live)
                    .ok()
                    .is_some_and(|fingerprint| {
                        bindings.iter().any(|binding| {
                            binding.app_type == app.as_str()
                                && binding.enabled
                                && binding.status != "detached"
                                && binding.expected_fingerprint.as_deref()
                                    == Some(fingerprint.as_str())
                        })
                    });
                if !current_is_projection && !fingerprint_is_projection {
                    if let Some(candidate) = candidate_from_provider(
                        format!("live:{}", app.as_str()),
                        "externalManaged",
                        &app,
                        &live,
                    ) {
                        candidates.push(candidate);
                    }
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
    Ok((candidates, failures))
}

pub fn scan_imports(state: &AppState) -> Result<Vec<ImportCandidate>, AppError> {
    Ok(scan_imports_for_apps(state, &HashSet::new())?.0)
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
            if provider_sources(definition).iter().any(|source| {
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
            if provider_sources(definition)
                .iter()
                .any(|source| source.source_fingerprint.as_deref() == Some(fingerprint))
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
    let (scanned, mut errors) = scan_imports_for_apps(state, &requested_set)?;
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
    if let Err(error) = state.db.save_provider_import_session(
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
    ) {
        for reference in records
            .iter()
            .filter_map(|record| record.temporary_secret_ref.as_deref())
        {
            if let Err(cleanup_error) = remove_secret(state, reference) {
                return Err(AppError::Message(format!(
                    "保存导入会话失败: {error}；清理临时 API Key 失败: {cleanup_error}"
                )));
            }
        }
        return Err(error);
    }
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

fn cleanup_completed_import_candidate_secret(
    state: &AppState,
    record: &ProviderImportCandidateRecord,
) -> Result<(), AppError> {
    if let Some(reference) = record.temporary_secret_ref.as_deref() {
        remove_secret(state, reference)?;
    }
    state
        .db
        .clear_provider_import_candidate_secret_ref(&record.session_id, &record.id, now())
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
        cleanup_completed_import_candidate_secret(state, &record)?;
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
        )?;
        cleanup_completed_import_candidate_secret(state, &record)?;
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
    ensure_importable_source(
        state,
        &source_app_type,
        &current_source,
        &candidate.source_ref,
    )?;
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
            model_definitions: Vec::new(),
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
    )?;
    cleanup_completed_import_candidate_secret(state, &record)?;
    Ok(ImportCommitResult {
        action: decision.action,
        provider: Some(result),
        repeated: false,
    })
}

fn source_is_provider_center_projection(
    state: &AppState,
    app: &AppType,
    provider: &Provider,
    source_ref: &str,
) -> Result<bool, AppError> {
    if provider.category.as_deref() == Some("provider-center") {
        return Ok(true);
    }
    let definitions = load_definitions(state)?;
    let bindings = load_bindings(state)?;
    let is_active_binding = |definition: &ProviderDefinition, binding: &ProviderBinding| {
        binding.provider_id == definition.id
            && binding.app_type == app.as_str()
            && binding.enabled
            && binding.status != "detached"
    };

    if source_ref.starts_with("live:") {
        let current_is_projection = app_adapter(app).current(state).ok().is_some_and(|id| {
            definitions.iter().any(|definition| {
                bindings.iter().any(|binding| {
                    is_active_binding(definition, binding)
                        && projected_provider_id(definition, app.as_str()) == id
                })
            })
        });
        if current_is_projection {
            return Ok(true);
        }
        let fingerprint = import_candidate_fingerprint(app, provider)?;
        return Ok(definitions.iter().any(|definition| {
            bindings.iter().any(|binding| {
                is_active_binding(definition, binding)
                    && binding.expected_fingerprint.as_deref() == Some(fingerprint.as_str())
            })
        }));
    }

    Ok(definitions.iter().any(|definition| {
        bindings.iter().any(|binding| {
            is_active_binding(definition, binding)
                && projected_provider_id(definition, app.as_str()) == provider.id
        })
    }))
}

fn ensure_importable_source(
    state: &AppState,
    app: &AppType,
    provider: &Provider,
    source_ref: &str,
) -> Result<(), AppError> {
    if source_is_provider_center_projection(state, app, provider, source_ref)? {
        return Err(AppError::Message(
            "Provider Center 投射不能再次导入".to_string(),
        ));
    }
    Ok(())
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
    ensure_importable_source(state, &source_app, &provider, source_ref)?;
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
            model_definitions: model_definitions_from_settings(&provider.settings_config),
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
    let definition = definitions
        .iter()
        .find(|item| item.id == provider_id)
        .cloned()
        .ok_or_else(|| AppError::Message("模型服务不存在".to_string()))?;
    let active_bindings = load_bindings(state)?
        .into_iter()
        .filter(|binding| binding.provider_id == provider_id && binding.enabled)
        .map(|binding| binding.app_type)
        .collect::<Vec<_>>();
    if !active_bindings.is_empty() {
        return Err(AppError::Message(format!(
            "模型服务仍绑定到 {}，请先选择移除投射或保留本地副本并解除绑定",
            active_bindings.join(", ")
        )));
    }
    let previous_secret = definition
        .credential_configured
        .then(|| get_secret(state, provider_id))
        .transpose()?;
    remove_secret(state, provider_id)?;
    if let Err(error) = state.db.delete_provider_center_definition(provider_id) {
        if let Err(restore_error) = restore_secret(state, provider_id, previous_secret.as_deref()) {
            return Err(AppError::Message(format!(
                "删除模型服务失败: {error}；恢复 API Key 失败: {restore_error}"
            )));
        }
        return Err(error);
    }
    Ok(())
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
            model_definitions: source.model_definitions.clone(),
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

fn normalized_target_app_types(requested: &[String], fallback: Option<&str>) -> Vec<String> {
    let values = if requested.is_empty() {
        fallback
            .map(|app_type| vec![app_type.to_string()])
            .unwrap_or_default()
    } else {
        requested.to_vec()
    };
    let mut seen = HashSet::new();
    values
        .into_iter()
        .filter_map(|app_type| {
            let trimmed = app_type.trim();
            if trimmed.is_empty() {
                return None;
            }
            let normalized = AppType::from_str(trimmed)
                .map(|app| app.as_str().to_string())
                .unwrap_or_else(|_| trimmed.to_ascii_lowercase());
            seen.insert(normalized.clone()).then_some(normalized)
        })
        .collect()
}

/// Return the exact normalized Agent lock/apply set used by managed-draft
/// confirmation. This keeps command-level locking aligned with the targets
/// validated by `preview_managed_draft` and written by `confirm_managed_draft`.
pub fn managed_draft_target_app_types(input: &ManagedProviderDraftInput) -> Vec<String> {
    normalized_target_app_types(&input.target_app_types, Some(&input.app_type))
}

fn unsupported_preview_target(app_type: String, message: String) -> ProviderApplyPreviewTarget {
    ProviderApplyPreviewTarget {
        app_type,
        operation: "unsupported".to_string(),
        compatible: false,
        connection_mode: "unsupported".to_string(),
        route_id: None,
        requires_takeover: false,
        drifted: false,
        current_provider_id: None,
        live_fingerprint: None,
        message: Some(message),
    }
}

/// Evaluate one target without mutating definitions, bindings, provider rows,
/// live configuration, current-provider state, or proxy state.
fn evaluate_preview_target(
    state: &AppState,
    definition: &ProviderDefinition,
    secret: &str,
    requested_app_type: &str,
    expected_fingerprint: Option<&String>,
) -> Result<ProviderApplyPreviewTarget, AppError> {
    let app = match AppType::from_str(requested_app_type) {
        Ok(app) => app,
        Err(_) => {
            return Ok(unsupported_preview_target(
                requested_app_type.to_string(),
                "应用类型无效".to_string(),
            ));
        }
    };
    let app_type = app.as_str().to_string();
    let adapter = app_adapter(&app);
    let compat = adapter.resolve_compatibility(&definition.protocol);
    log::info!(
        "[evaluate_preview_target] app={}, protocol={}, compat={:?}",
        app_type,
        definition.protocol,
        compat
    );
    let (connection_mode, route_id, requires_takeover) = match compat {
        Compatibility::Direct => ("direct".to_string(), None, false),
        Compatibility::Proxy {
            route_id,
            requires_takeover,
        } => ("proxy".to_string(), Some(route_id), requires_takeover),
        Compatibility::Unsupported { reason } => {
            return Ok(unsupported_preview_target(app_type, reason));
        }
    };
    let projected = match projection(definition, secret.to_string(), &app_type) {
        Ok(provider) => provider,
        Err(error) => {
            log::info!(
                "[evaluate_preview_target] projection FAILED for app={}, error={}",
                app_type,
                error
            );
            return Ok(unsupported_preview_target(app_type, error.to_string()));
        }
    };
    let existing = state.db.get_provider_by_id(&projected.id, app.as_str())?;
    let current_fingerprint = existing.as_ref().map(provider_fingerprint).transpose()?;
    let drifted =
        expected_fingerprint.is_some() && expected_fingerprint != current_fingerprint.as_ref();
    let current_id = adapter.current(state)?;
    Ok(ProviderApplyPreviewTarget {
        app_type,
        operation: if existing.is_some() {
            "update"
        } else {
            "create"
        }
        .to_string(),
        compatible: true,
        connection_mode,
        route_id,
        requires_takeover,
        drifted,
        current_provider_id: (!current_id.is_empty()).then_some(current_id),
        live_fingerprint: current_fingerprint,
        message: drifted.then(|| "目标配置已在 CC Switch 外部发生变化，需要先确认冲突".to_string()),
    })
}

/// Merge credential-bearing fields from the adapter-generated projection into
/// the stored app-specific template. Shared fields (endpoint, key, model list)
/// always come from the definition; Agent-specific advanced fields (Codex
/// model_catalog_json, custom endpoints, meta, icon, etc.) are preserved from
/// the template so re-applying a shared definition does not silently wipe them.
fn merge_template_with_projection(template: &Provider, mut projected: Provider) -> Provider {
    // Preserve identity/display fields from the template when the adapter
    // generated a placeholder id/name.
    if projected.name.is_empty() {
        projected.name = template.name.clone();
    }
    projected.website_url = template.website_url.clone().or(projected.website_url);
    projected.notes = template.notes.clone().or(projected.notes);
    projected.icon = template.icon.clone().or(projected.icon);
    projected.icon_color = template.icon_color.clone().or(projected.icon_color);
    projected.meta = template.meta.clone().or(projected.meta);
    projected.sort_index = template.sort_index.or(projected.sort_index);

    // Deep-merge settings_config: template provides the base, projection
    // overwrites shared credential/endpoint fields.
    if let (Some(template_config), Some(projected_config)) = (
        template.settings_config.as_object(),
        projected.settings_config.as_object(),
    ) {
        let mut merged = template_config.clone();
        for (key, value) in projected_config {
            merged.insert(key.clone(), value.clone());
        }
        projected.settings_config = serde_json::Value::Object(merged);
    }

    projected
}

/// Strip all credential-bearing fields from a Provider so the result is safe
/// to persist in a binding template and never leaks a key over IPC.
fn redact_provider_credentials(provider: &Provider) -> Provider {
    let redacted_config = app_adapter::credential_safe_value(&provider.settings_config);
    let mut redacted = provider.clone();
    redacted.settings_config = redacted_config;
    redacted
}

fn managed_draft_api_key(
    state: &AppState,
    app: &AppType,
    provider: &Provider,
    definition_id: &str,
    submitted_api_key: String,
    needs_key: bool,
) -> Result<(String, bool), AppError> {
    if !needs_key || !submitted_api_key.trim().is_empty() {
        return Ok((submitted_api_key, false));
    }
    if app != &AppType::DeepSeekHarness {
        return Err(AppError::Message(
            "无法从当前供应商配置中提取 API Key".to_string(),
        ));
    }

    let definition = load_definitions(state)?
        .into_iter()
        .find(|definition| definition.id == definition_id)
        .ok_or_else(|| AppError::Message("模型服务不存在".to_string()))?;
    let projected_id = projected_provider_id(&definition, app.as_str());
    if provider.id != projected_id {
        return Err(AppError::Message(
            "受管 DSH Provider 标识与投影不一致".to_string(),
        ));
    }
    let stored = state
        .db
        .get_provider_by_id(&projected_id, app.as_str())?
        .ok_or_else(|| AppError::Message("受管 DSH Provider 投影不存在".to_string()))?;
    projection_secret_for_provider(state, app, &stored, definition_id).map(|secret| (secret, true))
}

/// Preview applying an original Agent Provider form payload as a managed
/// shared definition. This performs NO writes: it validates the draft,
/// extracts shared fields, and returns a preview token. The caller must
/// invoke `confirm_managed_draft` with the same token to persist.
pub fn preview_managed_draft(
    state: &AppState,
    input: ManagedProviderDraftInput,
) -> Result<ProviderApplyPreview, AppError> {
    let app = AppType::from_str(&input.app_type)?;
    let app_str = app.as_str().to_string();
    let provider = input.provider;

    // Native OAuth / Coding Plan / official providers cannot be shared.
    if provider.uses_managed_account_auth() {
        return Err(AppError::Message(
            "原生 OAuth / Coding Plan 账号不能转为通用供应商".to_string(),
        ));
    }
    if provider.category.as_deref() == Some("official") {
        return Err(AppError::Message(
            "官方供应商不能转为通用供应商".to_string(),
        ));
    }

    let adapter = app_adapter(&app);
    let protocol = adapter.infer_protocol(&provider);
    adapter.validate_protocol(&protocol)?;

    let (base_url, api_key) = provider.resolve_usage_credentials(&app);
    let base_url = base_url.trim().to_string();
    if base_url.is_empty() {
        return Err(AppError::Message(
            "无法从当前供应商配置中提取请求地址".to_string(),
        ));
    }
    let needs_key = protocol != "ollama";
    let (api_key, _) = managed_draft_api_key(
        state,
        &app,
        &provider,
        &input.definition_id,
        api_key,
        needs_key,
    )?;

    let definitions = load_definitions(state)?;
    let existing = definitions
        .iter()
        .find(|item| item.id == input.definition_id)
        .cloned();
    if let (Some(expected), Some(current)) = (input.expected_revision, existing.as_ref()) {
        if expected != current.revision {
            return Err(AppError::Message(format!(
                "模型服务已被其他操作修改（当前版本 {}，提交版本 {}），请刷新后重试",
                current.revision, expected
            )));
        }
    }

    let model_defs = model_definitions_from_settings(&provider.settings_config);
    let models: Vec<String> = model_defs.iter().map(|d| d.id.clone()).collect();
    let name = provider.name.trim().to_string();
    let notes = provider.notes.as_deref().unwrap_or("").trim().to_string();
    let revision = existing
        .as_ref()
        .map(|item| item.revision)
        .unwrap_or(0)
        .saturating_add(1);

    // Build a synthetic definition solely for preview; it is NOT persisted.
    let preview_definition = ProviderDefinition {
        id: input.definition_id.clone(),
        name,
        protocol: protocol.clone(),
        base_url: base_url.clone(),
        models,
        model_definitions: model_defs.clone(),
        discovered_models: existing
            .as_ref()
            .map(|item| item.discovered_models.clone())
            .unwrap_or_default(),
        notes,
        enabled: true,
        revision,
        source: existing.as_ref().and_then(|item| item.source.clone()),
        sources: existing
            .as_ref()
            .map(|item| item.sources.clone())
            .unwrap_or_default(),
        credential_configured: needs_key,
        credential_hint: needs_key.then(|| secret_hint(&api_key)).flatten(),
        last_discovery_at: existing.as_ref().and_then(|item| item.last_discovery_at),
        last_discovery_error: existing
            .as_ref()
            .and_then(|item| item.last_discovery_error.clone()),
        created_at: existing
            .as_ref()
            .map(|item| item.created_at)
            .unwrap_or_else(now),
        updated_at: now(),
    };

    let secret = if needs_key { api_key } else { String::new() };

    // Determine target app types. If not specified, default to the current
    // agent only (preserving the original single-agent behavior).
    let target_apps = normalized_target_app_types(&input.target_app_types, Some(&app_str));
    let bindings = load_bindings(state)?;

    let mut targets = Vec::new();
    for target_app in &target_apps {
        let expected_fingerprint = bindings
            .iter()
            .find(|binding| {
                binding.provider_id == input.definition_id && binding.app_type == *target_app
            })
            .and_then(|binding| binding.expected_fingerprint.as_ref());
        targets.push(evaluate_preview_target(
            state,
            &preview_definition,
            &secret,
            target_app,
            expected_fingerprint,
        )?);
    }

    for target in &targets {
        log::info!(
            "[preview_managed_draft] target app={}, compatible={}, connection_mode={}, route_id={:?}, message={:?}",
            target.app_type,
            target.compatible,
            target.connection_mode,
            target.route_id,
            target.message
        );
    }

    let token = preview_token(&input.definition_id, revision, &targets)?;
    Ok(ProviderApplyPreview {
        token,
        provider_id: input.definition_id,
        provider_revision: revision,
        targets,
        created_at: now(),
    })
}

/// Confirm and persist a managed draft. This is the ONLY write path for
/// managed projections from the original Agent form. It:
/// 1. Re-validates the draft (revision, protocol, credentials).
/// 2. Saves/updates the definition with shared fields + secure secret.
/// 3. Stores a credential-redacted app-specific template on each target
///    binding.
/// 4. Applies via apply_transaction, which merges the template during
///    projection so Agent-specific advanced fields survive shared updates.
/// 5. AppAdapter::apply uses add_to_live=false and does not switch —
///    zero side effects on live config, current provider, or proxy state.
pub fn confirm_managed_draft(
    state: &AppState,
    input: ManagedProviderDraftInput,
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

    let app = AppType::from_str(&input.app_type)?;
    let app_str = app.as_str().to_string();
    let provider = input.provider.clone();

    if provider.uses_managed_account_auth() {
        return Err(AppError::Message(
            "原生 OAuth / Coding Plan 账号不能转为通用供应商".to_string(),
        ));
    }
    if provider.category.as_deref() == Some("official") {
        return Err(AppError::Message(
            "官方供应商不能转为通用供应商".to_string(),
        ));
    }

    let adapter = app_adapter(&app);
    let protocol = adapter.infer_protocol(&provider);
    adapter.validate_protocol(&protocol)?;
    let (base_url, api_key) = provider.resolve_usage_credentials(&app);
    let base_url = base_url.trim().to_string();
    if base_url.is_empty() {
        return Err(AppError::Message(
            "无法从当前供应商配置中提取请求地址".to_string(),
        ));
    }
    let needs_key = protocol != "ollama";
    let (api_key, keep_existing_key) = managed_draft_api_key(
        state,
        &app,
        &provider,
        &input.definition_id,
        api_key,
        needs_key,
    )?;

    // Re-run preview to recompute and verify the token.
    let preview = preview_managed_draft(state, input.clone())?;
    if preview.token != preview_token_value {
        return Err(AppError::Message("应用预览已过期，请重新预览".to_string()));
    }

    let model_defs = model_definitions_from_settings(&provider.settings_config);
    let models: Vec<String> = model_defs.iter().map(|d| d.id.clone()).collect();
    let redacted_template = redact_provider_credentials(&provider);

    // Determine the normalized, de-duplicated target app types. If not
    // specified, default to the current agent (single-agent compatibility).
    let target_apps = normalized_target_app_types(&input.target_app_types, Some(&app_str));

    // Save the definition with shared fields + secure secret. The definition
    // is saved with all target app types so bindings are created for each.
    let save_input = SaveProviderDefinitionInput {
        id: Some(input.definition_id.clone()),
        name: provider.name.trim().to_string(),
        protocol: protocol.clone(),
        base_url: base_url.clone(),
        models,
        model_definitions: model_defs,
        notes: provider.notes.as_deref().unwrap_or("").trim().to_string(),
        enabled: Some(true),
        expected_revision: input.expected_revision,
        credential_action: Some(if !needs_key {
            "clear"
        } else if keep_existing_key {
            "keep"
        } else {
            "replace"
        })
        .map(str::to_string),
        source: None,
        api_key: (needs_key && !keep_existing_key).then(|| api_key.trim().to_string()),
        app_types: target_apps.clone(),
    };
    let definition = save_definition(state, save_input)?;

    // Attach the redacted source-native template only to the source Agent's
    // binding so Agent-specific advanced fields survive shared updates. Other
    // target bindings rely on the target-native projection (which now carries
    // normalized model metadata) — no source-specific fields leak across.
    let mut bindings = load_bindings(state)?;
    for binding in bindings
        .iter_mut()
        .filter(|b| b.provider_id == definition.id && target_apps.contains(&b.app_type))
    {
        if binding.app_type == app_str {
            binding.provider_template = Some(redacted_template.clone());
        } else {
            binding.provider_template = None;
        }
        binding.updated_at = now();
    }
    save_core(state, &load_definitions(state)?, &bindings)?;

    // Apply via the standard transaction path. AppAdapter::apply uses
    // add_to_live=false and does not call switch, so this writes only to
    // the per-agent provider DB — no live config, no proxy start.
    let transaction = apply_transaction_with_drift_policy(
        state,
        &definition.id,
        target_apps,
        &preview.token,
        idempotency_key,
        true,
    )?;

    Ok(transaction)
}

fn selected_targets(
    bindings: &[ProviderBinding],
    provider_id: &str,
    requested: Vec<String>,
) -> Vec<String> {
    let eligible = bindings
        .iter()
        .filter(|binding| {
            binding.provider_id == provider_id && binding.enabled && !binding.override_enabled
        })
        .map(|binding| binding.app_type.clone())
        .collect::<HashSet<_>>();

    if requested.is_empty() {
        let mut targets = eligible.into_iter().collect::<Vec<_>>();
        targets.sort();
        return targets;
    }

    let mut seen = HashSet::new();
    requested
        .into_iter()
        .filter(|app_type| eligible.contains(app_type) && seen.insert(app_type.clone()))
        .collect()
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

pub fn delete_target_app_types(
    state: &AppState,
    provider_id: &str,
) -> Result<Vec<String>, AppError> {
    let bindings = load_bindings(state)?;
    let mut app_types = bindings
        .into_iter()
        .filter(|b| b.provider_id == provider_id && b.enabled)
        .map(|b| b.app_type)
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
    for app_type in targets {
        let expected = bindings
            .iter()
            .find(|binding| binding.provider_id == provider_id && binding.app_type == app_type)
            .and_then(|binding| binding.expected_fingerprint.as_ref());
        previews.push(evaluate_preview_target(
            state,
            &definition,
            &secret,
            &app_type,
            expected,
        )?);
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

/// Preview persisted-definition compatibility for exactly the explicitly
/// requested Agents. Unlike `preview_apply`, this does not filter through
/// enabled bindings and never creates bindings, projections, or transactions.
pub fn preview_definition_compatibility(
    state: &AppState,
    provider_id: &str,
    app_types: Vec<String>,
) -> Result<ProviderApplyPreview, AppError> {
    let definition = load_definitions(state)?
        .into_iter()
        .find(|item| item.id == provider_id)
        .ok_or_else(|| AppError::Message("模型服务不存在".to_string()))?;
    let targets = normalized_target_app_types(&app_types, None);
    if targets.is_empty() {
        return Err(AppError::Message("请至少选择一个目标应用".to_string()));
    }
    // Compatibility is protocol/Agent metadata. An empty secret is sufficient
    // to render safe projected identifiers/fingerprints and avoids requiring or
    // exposing persisted credentials on this read-only endpoint.
    let secret = String::new();
    let previews = targets
        .iter()
        .map(|app_type| evaluate_preview_target(state, &definition, &secret, app_type, None))
        .collect::<Result<Vec<_>, _>>()?;
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
    all.insert(
        transaction_id.to_string(),
        secure_store::protect_secret(&serialized)?,
    );
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
    let serialized = secure_store::unprotect_secret(encrypted)?;
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
            if app == AppType::DeepSeekHarness {
                AppAdapterRegistry::global()
                    .resolve(app.as_str())?
                    .apply(state, provider)?;
            } else if state.db.get_provider_by_id(&id, app.as_str())?.is_some() {
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
            {
                let delete_result = AppAdapterRegistry::global()
                    .resolve(app.as_str())?
                    .delete(state, &snapshot.projected_provider_id);
                if let Err(error) = delete_result {
                    if app == AppType::DeepSeekHarness {
                        return Err(error);
                    }
                    state
                        .db
                        .delete_provider(app.as_str(), &snapshot.projected_provider_id)?;
                }
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
    apply_transaction_with_drift_policy(
        state,
        provider_id,
        app_types,
        preview_token_value,
        idempotency_key,
        false,
    )
}

fn apply_transaction_with_drift_policy(
    state: &AppState,
    provider_id: &str,
    app_types: Vec<String>,
    preview_token_value: &str,
    idempotency_key: Option<&str>,
    allow_confirmed_drift: bool,
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
    // Reject if any target has drifted (external modification conflict).
    if !allow_confirmed_drift {
        if let Some(drifted) = preview.targets.iter().find(|t| t.drifted) {
            return Err(AppError::Message(format!(
                "目标 {} 的配置已被外部修改，请先确认冲突后重试",
                drifted.app_type
            )));
        }
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
                status: if target.compatible {
                    "pending".to_string()
                } else {
                    "detached".to_string()
                },
                message: if target.compatible {
                    None
                } else {
                    Some("协议不兼容，已解除共享".to_string())
                },
            })
            .collect(),
        created_at: now(),
        completed_at: None,
    };
    let mut bindings = load_bindings(state)?;

    // Detach incompatible targets' bindings before projection so they keep
    // their existing provider as an independent local copy.
    for target in &preview.targets {
        if !target.compatible {
            if let Some(binding) = bindings
                .iter_mut()
                .find(|b| b.provider_id == provider_id && b.app_type == target.app_type)
            {
                binding.enabled = false;
                binding.status = "detached".to_string();
                binding.expected_fingerprint = None;
                binding.last_error = None;
                binding.updated_at = now();
            }
        }
    }

    let mut snapshots = Vec::new();
    for target in &preview.targets {
        // Skip incompatible targets — they are detached above, not projected.
        if !target.compatible {
            continue;
        }
        let app = AppType::from_str(&target.app_type)?;
        let projected = projection(&definition, secret.clone(), &target.app_type)?;
        let template = bindings
            .iter()
            .find(|b| b.provider_id == provider_id && b.app_type == target.app_type)
            .and_then(|b| b.provider_template.clone());
        let mut projected = match template {
            Some(template) => merge_template_with_projection(&template, projected),
            None => projected,
        };
        ProviderService::normalize_provider_center_projection_for_storage(
            state,
            &app,
            &mut projected,
        )?;
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

    let mut succeeded = Vec::new();
    let mut failed = false;
    let mut failed_index = None;
    for (snapshot_index, snapshot) in snapshots.iter().enumerate() {
        let target_index = transaction
            .targets
            .iter()
            .position(|target| target.app_type == snapshot.app_type)
            .ok_or_else(|| AppError::Message("应用事务目标与快照不一致".to_string()))?;
        let adapter = AppAdapterRegistry::global().resolve(&snapshot.app_type)?;
        let projected = projection(&definition, secret.clone(), &snapshot.app_type)?;
        let template = bindings
            .iter()
            .find(|b| b.provider_id == provider_id && b.app_type == snapshot.app_type)
            .and_then(|b| b.provider_template.clone());
        let projected = match template {
            Some(template) => merge_template_with_projection(&template, projected),
            None => projected,
        };
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
                transaction.targets[target_index].status = "applied".to_string();
                succeeded.push(snapshot_index);
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
                transaction.targets[target_index].status = "failed".to_string();
                transaction.targets[target_index].message = Some(message.clone());
                if let Some(binding) = bindings.iter_mut().find(|binding| {
                    binding.provider_id == provider_id && binding.app_type == snapshot.app_type
                }) {
                    binding.status = "failed".to_string();
                    binding.last_error = Some(message);
                    binding.last_transaction_id = Some(transaction_id.clone());
                    binding.updated_at = now();
                }
                failed = true;
                failed_index = Some(snapshot_index);
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
        for snapshot_index in rollback_indices.into_iter().rev() {
            let snapshot = &snapshots[snapshot_index];
            let target_index = transaction
                .targets
                .iter()
                .position(|target| target.app_type == snapshot.app_type)
                .ok_or_else(|| AppError::Message("应用事务目标与快照不一致".to_string()))?;
            let original_message = transaction.targets[target_index].message.clone();
            match restore_snapshot(state, snapshot) {
                Ok(()) => {
                    transaction.targets[target_index].status = "rolled_back".to_string();
                    transaction.targets[target_index].message = original_message;
                    if let Some(binding) = bindings.iter_mut().find(|binding| {
                        binding.provider_id == provider_id && binding.app_type == snapshot.app_type
                    }) {
                        binding.status = "pending".to_string();
                        binding.expected_fingerprint = snapshot.before_fingerprint.clone();
                    }
                }
                Err(error) => {
                    rollback_failed = true;
                    transaction.targets[target_index].status = "rollbackFailed".to_string();
                    transaction.targets[target_index].message = Some(match original_message {
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

pub fn attach_binding(
    state: &AppState,
    provider_id: &str,
    app_type: &str,
) -> Result<ProviderBinding, AppError> {
    let definitions = load_definitions(state)?;
    let definition = definitions
        .iter()
        .find(|definition| definition.id == provider_id)
        .ok_or_else(|| AppError::Message("模型服务不存在".to_string()))?;
    if !definition.enabled {
        return Err(AppError::Message("模型服务已停用，不能绑定".to_string()));
    }

    let app = AppType::from_str(app_type)?;
    let normalized_app_type = app.as_str().to_string();
    app_adapter(&app).validate_protocol(&definition.protocol)?;
    if definition.protocol != "ollama" {
        get_secret(state, provider_id)?;
    }

    let mut bindings = load_bindings(state)?;
    if let Some(binding) = bindings.iter().find(|binding| {
        binding.provider_id == provider_id
            && binding.app_type == normalized_app_type
            && binding.enabled
    }) {
        return Ok(binding.clone());
    }

    let timestamp = now();
    let attached = if let Some(binding) = bindings.iter_mut().find(|binding| {
        binding.provider_id == provider_id && binding.app_type == normalized_app_type
    }) {
        binding.enabled = true;
        binding.override_enabled = false;
        binding.status = "pending".to_string();
        binding.applied_revision = None;
        binding.expected_fingerprint = None;
        binding.last_error = None;
        binding.last_transaction_id = None;
        binding.updated_at = timestamp;
        binding.clone()
    } else {
        let binding = ProviderBinding {
            provider_id: provider_id.to_string(),
            app_type: normalized_app_type,
            status: "pending".to_string(),
            enabled: true,
            override_enabled: false,
            provider_template: None,
            applied_revision: None,
            expected_fingerprint: None,
            last_error: None,
            last_transaction_id: None,
            updated_at: timestamp,
        };
        bindings.push(binding.clone());
        binding
    };

    save_core(state, &definitions, &bindings)?;
    Ok(attached)
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
    let binding_index = bindings
        .iter()
        .position(|binding| binding.provider_id == provider_id && binding.app_type == app_type)
        .ok_or_else(|| AppError::Message("绑定关系不存在".to_string()))?;

    // Drift check: refuse to disable if the projected provider has been
    // modified externally since the last apply. The user must re-apply or
    // explicitly resolve the conflict first.
    let app = AppType::from_str(app_type)?;
    let projected_id = projected_provider_id(definition, app_type);
    if let Some(expected) = bindings[binding_index].expected_fingerprint.as_ref() {
        if let Some(projected) = state.db.get_provider_by_id(&projected_id, app.as_str())? {
            let actual_fingerprint = provider_fingerprint(&projected)?;
            if &actual_fingerprint != expected {
                return Err(AppError::Message(
                    "目标配置已被外部修改（漂移），请先重新应用或手动解决冲突再解除绑定"
                        .to_string(),
                ));
            }
        }
    }

    if remove_projection {
        if state
            .db
            .get_provider_by_id(&projected_id, app.as_str())?
            .is_some()
        {
            app_adapter(&app).delete(state, &projected_id)?;
        }
        let binding = &mut bindings[binding_index];
        binding.enabled = false;
        binding.override_enabled = false;
        binding.status = "detached".to_string();
        binding.applied_revision = None;
        binding.expected_fingerprint = None;
        binding.last_error = None;
        binding.updated_at = now();
        return save_core(state, &definitions, &bindings);
    }

    let original_provider = state.db.get_provider_by_id(&projected_id, app.as_str())?;
    let mut previous_dsh_secret = None;
    if let Some(mut provider) = original_provider.clone() {
        if app == AppType::DeepSeekHarness
            && provider
                .settings_config
                .get("credentialRef")
                .and_then(serde_json::Value::as_object)
                .and_then(|reference| reference.get("source"))
                .and_then(serde_json::Value::as_str)
                == Some("providerCenter")
        {
            let (configured, _) =
                secure_store::metadata(state, SecretScope::DshProvider, &projected_id)?;
            previous_dsh_secret = configured
                .then(|| secure_store::get(state, SecretScope::DshProvider, &projected_id))
                .transpose()?;
            let secret = get_secret(state, provider_id)?;
            secure_store::save(state, SecretScope::DshProvider, &projected_id, &secret)?;
            provider.settings_config["credentialRef"] = serde_json::json!({
                "source": "dshProvider",
                "id": projected_id,
            });
        }
        provider.category = None;
        if let Err(error) = state.db.save_provider(app.as_str(), &provider) {
            if app == AppType::DeepSeekHarness {
                let _ = secure_store::restore(
                    state,
                    SecretScope::DshProvider,
                    &projected_id,
                    previous_dsh_secret.as_deref(),
                );
            }
            return Err(error);
        }
    }

    let binding = &mut bindings[binding_index];
    binding.enabled = false;
    binding.override_enabled = false;
    binding.status = "detached".to_string();
    binding.applied_revision = None;
    binding.expected_fingerprint = None;
    binding.last_error = None;
    binding.updated_at = now();
    if let Err(error) = save_core(state, &definitions, &bindings) {
        if let Some(original) = original_provider {
            let _ = state.db.save_provider(app.as_str(), &original);
        }
        if app == AppType::DeepSeekHarness {
            let _ = secure_store::restore(
                state,
                SecretScope::DshProvider,
                &projected_id,
                previous_dsh_secret.as_deref(),
            );
        }
        return Err(error);
    }
    Ok(())
}

/// Delete semantics for Provider Center managed providers.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum DeleteMode {
    /// Remove the projection from the current agent and detach the binding.
    RemoveCurrent,
    /// Keep the projected provider as an independent copy (strip category),
    /// detach the binding.
    DetachKeepIndependent,
    /// Delete all projections, all bindings, the definition, and the secret.
    DeleteGlobally,
}

pub fn delete_provider(
    state: &AppState,
    provider_id: &str,
    app_type: &str,
    mode: DeleteMode,
) -> Result<(), AppError> {
    match mode {
        DeleteMode::RemoveCurrent => delete_single(state, provider_id, app_type, true),
        DeleteMode::DetachKeepIndependent => delete_single(state, provider_id, app_type, false),
        DeleteMode::DeleteGlobally => delete_globally(state, provider_id),
    }
}

fn check_projection_not_in_use(
    state: &AppState,
    definition: &ProviderDefinition,
    app_type: &str,
) -> Result<(), AppError> {
    let app = AppType::from_str(app_type)?;
    let projected_id = projected_provider_id(definition, app_type);
    let current = ProviderService::current(state, app)?;
    if !current.is_empty() && current == projected_id {
        return Err(AppError::Message(format!(
            "该 Provider 正在 {} 中使用，请先切换到其他 Provider 再删除",
            app_type
        )));
    }
    Ok(())
}

fn delete_single(
    state: &AppState,
    provider_id: &str,
    app_type: &str,
    remove_projection: bool,
) -> Result<(), AppError> {
    let definitions = load_definitions(state)?;
    let definition = definitions
        .iter()
        .find(|d| d.id == provider_id)
        .ok_or_else(|| AppError::Message("模型服务不存在".to_string()))?;
    check_projection_not_in_use(state, definition, app_type)?;
    disable_binding(state, provider_id, app_type, remove_projection)?;
    auto_clean_definition_if_orphaned(state, provider_id)?;
    Ok(())
}

fn auto_clean_definition_if_orphaned(state: &AppState, provider_id: &str) -> Result<(), AppError> {
    let bindings = load_bindings(state)?;
    let has_active = bindings
        .iter()
        .any(|b| b.provider_id == provider_id && b.enabled);
    if has_active {
        return Ok(());
    }
    let definitions = load_definitions(state)?;
    let definition = definitions.iter().find(|d| d.id == provider_id).cloned();
    if let Some(definition) = definition {
        if definition.credential_configured {
            let _ = remove_secret(state, provider_id);
        }
        state.db.delete_provider_center_definition(provider_id)?;
    }
    Ok(())
}

fn delete_globally(state: &AppState, provider_id: &str) -> Result<(), AppError> {
    let definitions = load_definitions(state)?;
    let definition = definitions
        .iter()
        .find(|item| item.id == provider_id)
        .cloned()
        .ok_or_else(|| AppError::Message("模型服务不存在".to_string()))?;
    let bindings = load_bindings(state)?;
    let active_bindings: Vec<_> = bindings
        .iter()
        .filter(|b| b.provider_id == provider_id && b.enabled)
        .collect();

    // In-use prevention: refuse if any projected provider is the current
    // provider for its agent.
    for binding in &active_bindings {
        check_projection_not_in_use(state, &definition, &binding.app_type)?;
    }

    // Drift prevention: refuse if any projected provider has drifted.
    for binding in &active_bindings {
        let app = AppType::from_str(&binding.app_type)?;
        let projected_id = projected_provider_id(&definition, &binding.app_type);
        if let Some(expected) = binding.expected_fingerprint.as_ref() {
            if let Some(projected) = state.db.get_provider_by_id(&projected_id, app.as_str())? {
                let actual_fingerprint = provider_fingerprint(&projected)?;
                if &actual_fingerprint != expected {
                    return Err(AppError::Message(
                        "目标配置已被外部修改（漂移），请先重新应用或手动解决冲突再删除"
                            .to_string(),
                    ));
                }
            }
        }
    }

    // Delete all projections with best-effort rollback.
    let mut deleted_projections: Vec<(String, Provider)> = Vec::new();
    for binding in &active_bindings {
        let app = AppType::from_str(&binding.app_type)?;
        let projected_id = projected_provider_id(&definition, &binding.app_type);
        if let Some(projected) = state.db.get_provider_by_id(&projected_id, app.as_str())? {
            match AppAdapterRegistry::global()
                .resolve(app.as_str())?
                .delete(state, &projected_id)
            {
                Ok(()) => {
                    deleted_projections.push((binding.app_type.clone(), projected));
                }
                Err(error) => {
                    for (app_type_str, provider) in &deleted_projections {
                        if let Ok(adapter) = AppAdapterRegistry::global().resolve(app_type_str) {
                            let _ = adapter.apply(state, provider.clone());
                        }
                    }
                    return Err(error);
                }
            }
        }
    }

    // All projections deleted — remove bindings, definition, and secret.
    if definition.credential_configured {
        let _ = remove_secret(state, provider_id);
    }
    state.db.delete_provider_center_definition(provider_id)?;

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::provider::{ClaudeModelConfig, CodexModelConfig, GeminiModelConfig};
    use serde_json::json;
    use std::env;
    use std::fs;
    use std::sync::{Mutex, OnceLock};
    use tempfile::TempDir;

    struct TempHome {
        _dir: TempDir,
        home: Option<String>,
        test_home: Option<String>,
        #[cfg(windows)]
        local_app_data: Option<String>,
        #[cfg(windows)]
        userprofile: Option<String>,
    }

    impl TempHome {
        fn new() -> Self {
            let dir = tempfile::tempdir().expect("temporary test home");
            let home = env::var("HOME").ok();
            let test_home = env::var("CC_SWITCH_TEST_HOME").ok();
            #[cfg(windows)]
            let local_app_data = env::var("LOCALAPPDATA").ok();
            #[cfg(windows)]
            let userprofile = env::var("USERPROFILE").ok();
            env::set_var("HOME", dir.path());
            env::set_var("CC_SWITCH_TEST_HOME", dir.path());
            #[cfg(windows)]
            {
                env::set_var("LOCALAPPDATA", dir.path().join("AppData").join("Local"));
                env::set_var("USERPROFILE", dir.path());
            }
            Self {
                _dir: dir,
                home,
                test_home,
                #[cfg(windows)]
                local_app_data,
                #[cfg(windows)]
                userprofile,
            }
        }
    }

    impl Drop for TempHome {
        fn drop(&mut self) {
            match &self.home {
                Some(value) => env::set_var("HOME", value),
                None => env::remove_var("HOME"),
            }
            match &self.test_home {
                Some(value) => env::set_var("CC_SWITCH_TEST_HOME", value),
                None => env::remove_var("CC_SWITCH_TEST_HOME"),
            }
            #[cfg(windows)]
            {
                match &self.local_app_data {
                    Some(value) => env::set_var("LOCALAPPDATA", value),
                    None => env::remove_var("LOCALAPPDATA"),
                }
                match &self.userprofile {
                    Some(value) => env::set_var("USERPROFILE", value),
                    None => env::remove_var("USERPROFILE"),
                }
            }
        }
    }

    fn test_guard() -> std::sync::MutexGuard<'static, ()> {
        static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
        LOCK.get_or_init(|| Mutex::new(()))
            .lock()
            .unwrap_or_else(|error| error.into_inner())
    }

    fn with_test_home<T>(test: impl FnOnce(&AppState) -> T) -> T {
        let _guard = test_guard();
        let _home = TempHome::new();
        let state = AppState::new(Arc::new(
            crate::database::Database::memory().expect("in-memory database"),
        ));
        test(&state)
    }

    fn definition_input(
        id: Option<String>,
        expected_revision: Option<u64>,
        base_url: &str,
    ) -> SaveProviderDefinitionInput {
        SaveProviderDefinitionInput {
            id,
            expected_revision,
            name: "Shared OpenCode".to_string(),
            protocol: "openai-chat".to_string(),
            base_url: base_url.to_string(),
            models: vec!["model-a".to_string()],
            model_definitions: Vec::new(),
            notes: String::new(),
            enabled: Some(true),
            credential_action: Some("replace".to_string()),
            source: None,
            api_key: Some("test-only-secret".to_string()),
            app_types: vec!["opencode".to_string()],
        }
    }

    fn seed_open_code_definition(state: &AppState) -> ProviderDefinition {
        save_definition(
            state,
            definition_input(None, None, "https://api.example.test/v1"),
        )
        .expect("seed shared definition")
    }

    fn seed_open_code_and_openclaw_definition(state: &AppState) -> ProviderDefinition {
        let mut input = definition_input(None, None, "https://api.example.test/v1");
        input.app_types = vec!["opencode".to_string(), "openclaw".to_string()];
        save_definition(state, input).expect("seed shared definition")
    }

    fn definition(protocol: &str) -> ProviderDefinition {
        ProviderDefinition {
            id: "shared".to_string(),
            name: "Shared".to_string(),
            protocol: protocol.to_string(),
            base_url: "https://api.example.test/v1".to_string(),
            models: vec!["model-a".to_string()],
            model_definitions: Vec::new(),
            discovered_models: Vec::new(),
            notes: String::new(),
            enabled: true,
            revision: 1,
            source: None,
            sources: Vec::new(),
            credential_configured: true,
            credential_hint: Some("…1234".to_string()),
            last_discovery_at: None,
            last_discovery_error: None,
            created_at: 1,
            updated_at: 1,
        }
    }

    #[test]
    fn compatibility_preview_reports_direct_proxy_and_unsupported_without_writes() {
        with_test_home(|state| {
            let definition = seed_open_code_definition(state);
            let definitions_before =
                serde_json::to_value(load_definitions(state).expect("load definitions"))
                    .expect("serialize definitions");
            let bindings_before =
                serde_json::to_value(load_bindings(state).expect("load bindings"))
                    .expect("serialize bindings");

            let preview = preview_definition_compatibility(
                state,
                &definition.id,
                vec![
                    "opencode".to_string(),
                    "codex".to_string(),
                    "gemini".to_string(),
                    "codex".to_string(),
                ],
            )
            .expect("preview compatibility");

            assert_eq!(
                preview.targets.len(),
                3,
                "explicit targets are de-duplicated"
            );
            let direct = &preview.targets[0];
            assert_eq!(direct.app_type, "opencode");
            assert_eq!(direct.connection_mode, "direct");
            assert!(direct.compatible);
            assert_eq!(direct.route_id, None);
            assert!(!direct.requires_takeover);

            let proxy = &preview.targets[1];
            assert_eq!(proxy.app_type, "codex");
            assert_eq!(proxy.connection_mode, "proxy");
            assert!(proxy.compatible);
            assert_eq!(proxy.route_id.as_deref(), Some("codex-to-openai-chat"));
            assert!(proxy.requires_takeover);

            let unsupported = &preview.targets[2];
            assert_eq!(unsupported.app_type, "gemini");
            assert_eq!(unsupported.connection_mode, "unsupported");
            assert!(!unsupported.compatible);
            assert_eq!(unsupported.route_id, None);
            assert!(!unsupported.requires_takeover);

            let serialized = serde_json::to_value(proxy).expect("serialize target");
            assert_eq!(serialized["connectionMode"], "proxy");
            assert_eq!(serialized["routeId"], "codex-to-openai-chat");
            assert_eq!(serialized["requiresTakeover"], true);
            assert!(serialized.get("connection_mode").is_none());

            assert_eq!(
                serde_json::to_value(load_definitions(state).expect("reload definitions"))
                    .expect("serialize definitions"),
                definitions_before
            );
            assert_eq!(
                serde_json::to_value(load_bindings(state).expect("reload bindings"))
                    .expect("serialize bindings"),
                bindings_before
            );
            assert!(state
                .db
                .get_provider_by_id(&projected_provider_id(&definition, "codex"), "codex")
                .expect("query unbound Codex projection")
                .is_none());
            assert!(state
                .db
                .get_provider_by_id(&projected_provider_id(&definition, "gemini"), "gemini")
                .expect("query unbound Gemini projection")
                .is_none());
            assert!(load_transactions(state)
                .expect("load transactions")
                .is_empty());
        });
    }

    #[test]
    fn apply_preview_reports_capability_metadata_for_bound_targets() {
        with_test_home(|state| {
            let mut input = definition_input(None, None, "https://api.example.test/v1");
            input.app_types = vec![
                "opencode".to_string(),
                "codex".to_string(),
                "gemini".to_string(),
            ];
            let definition = save_definition(state, input).expect("seed multi-target definition");
            let preview = preview_apply(
                state,
                &definition.id,
                vec![
                    "opencode".to_string(),
                    "codex".to_string(),
                    "gemini".to_string(),
                ],
            )
            .expect("preview bound targets");

            assert_eq!(preview.targets[0].connection_mode, "direct");
            assert_eq!(preview.targets[1].connection_mode, "proxy");
            assert_eq!(
                preview.targets[1].route_id.as_deref(),
                Some("codex-to-openai-chat")
            );
            assert!(preview.targets[1].requires_takeover);
            assert_eq!(preview.targets[2].connection_mode, "unsupported");
            assert!(!preview.targets[2].compatible);
        });
    }

    #[test]
    fn selected_targets_preserve_explicit_order_and_remove_duplicates() {
        let bindings = vec![
            ProviderBinding {
                provider_id: "shared".to_string(),
                app_type: "openclaw".to_string(),
                status: "pending".to_string(),
                enabled: true,
                override_enabled: false,
                provider_template: None,
                applied_revision: None,
                expected_fingerprint: None,
                last_error: None,
                last_transaction_id: None,
                updated_at: 1,
            },
            ProviderBinding {
                provider_id: "shared".to_string(),
                app_type: "opencode".to_string(),
                status: "pending".to_string(),
                enabled: true,
                override_enabled: false,
                provider_template: None,
                applied_revision: None,
                expected_fingerprint: None,
                last_error: None,
                last_transaction_id: None,
                updated_at: 1,
            },
        ];

        assert_eq!(
            selected_targets(
                &bindings,
                "shared",
                vec![
                    "opencode".to_string(),
                    "openclaw".to_string(),
                    "opencode".to_string(),
                    "missing".to_string(),
                ],
            ),
            vec!["opencode".to_string(), "openclaw".to_string()]
        );
    }

    #[test]
    fn attach_binding_preserves_other_bindings_and_is_idempotent() {
        with_test_home(|state| {
            let definition = seed_open_code_and_openclaw_definition(state);
            let original_revision = definition.revision;
            let original_updated_at = definition.updated_at;
            let mut bindings = load_bindings(state).expect("load seeded bindings");
            let openclaw = bindings
                .iter_mut()
                .find(|binding| binding.app_type == "openclaw")
                .expect("openclaw binding");
            openclaw.enabled = false;
            openclaw.status = "detached".to_string();
            openclaw.last_error = Some("old error".to_string());
            let opencode_before = bindings
                .iter()
                .find(|binding| binding.app_type == "opencode")
                .expect("opencode binding")
                .clone();
            save_core(state, std::slice::from_ref(&definition), &bindings)
                .expect("save detached binding");

            let first = attach_binding(state, &definition.id, "openclaw")
                .expect("reattach openclaw binding");
            assert!(first.enabled);
            assert_eq!(first.status, "pending");
            assert!(first.last_error.is_none());

            let second = attach_binding(state, &definition.id, "openclaw")
                .expect("repeat attach is idempotent");
            assert_eq!(second.updated_at, first.updated_at);

            let loaded_definition = load_definitions(state)
                .expect("load definition")
                .into_iter()
                .find(|item| item.id == definition.id)
                .expect("definition remains");
            assert_eq!(loaded_definition.revision, original_revision);
            assert_eq!(loaded_definition.updated_at, original_updated_at);

            let loaded_bindings = load_bindings(state).expect("load bindings after attach");
            assert_eq!(loaded_bindings.len(), 2);
            let opencode_after = loaded_bindings
                .iter()
                .find(|binding| binding.app_type == "opencode")
                .expect("opencode binding remains");
            assert_eq!(opencode_after.status, opencode_before.status);
            assert_eq!(opencode_after.enabled, opencode_before.enabled);
            assert_eq!(
                loaded_bindings
                    .iter()
                    .filter(|binding| {
                        binding.provider_id == definition.id && binding.app_type == "openclaw"
                    })
                    .count(),
                1
            );
        });
    }

    #[test]
    fn attach_binding_rejects_incompatible_or_uncredentialed_definition() {
        with_test_home(|state| {
            let definition = seed_open_code_definition(state);
            // OpenAI Chat is now Proxy-compatible with Codex (via local route transform),
            // so attach should succeed.  Test a genuinely incompatible combination instead.
            let error = attach_binding(state, &definition.id, "gemini")
                .expect_err("OpenAI Chat is incompatible with Gemini (no transform path)");
            assert!(error.to_string().contains("不支持"));

            let mut ollama_input = definition_input(None, None, "http://127.0.0.1:11434");
            ollama_input.name = "Local Ollama".to_string();
            ollama_input.protocol = "ollama".to_string();
            ollama_input.credential_action = Some("clear".to_string());
            ollama_input.api_key = None;
            ollama_input.app_types.clear();
            let ollama = save_definition(state, ollama_input).expect("save ollama definition");
            attach_binding(state, &ollama.id, "opencode")
                .expect("ollama does not require credentials");

            let mut no_key_input = definition_input(None, None, "https://no-key.example.test/v1");
            no_key_input.name = "No Key".to_string();
            no_key_input.credential_action = Some("keep".to_string());
            no_key_input.api_key = None;
            no_key_input.app_types.clear();
            let no_key =
                save_definition(state, no_key_input).expect("save uncredentialed definition");
            let error = attach_binding(state, &no_key.id, "opencode")
                .expect_err("non-ollama requires a credential");
            assert!(error.to_string().contains("API Key"));
        });
    }

    #[test]
    fn apply_transaction_maps_snapshot_results_by_app_type() {
        with_test_home(|state| {
            let mut input = definition_input(None, None, "https://api.example.test/v1");
            input.app_types = vec!["gemini".to_string(), "opencode".to_string()];
            let definition = save_definition(state, input).expect("seed mixed targets");
            let preview = preview_apply(
                state,
                &definition.id,
                vec!["gemini".to_string(), "opencode".to_string()],
            )
            .expect("preview mixed targets");
            assert!(!preview.targets[0].compatible);
            assert!(preview.targets[1].compatible);

            let transaction = apply_transaction(
                state,
                &definition.id,
                vec!["gemini".to_string(), "opencode".to_string()],
                &preview.token,
                Some("mixed-target-index-regression"),
            )
            .expect("apply compatible target");

            assert_eq!(transaction.status, "applied");
            assert_eq!(transaction.targets[0].app_type, "gemini");
            assert_eq!(transaction.targets[0].status, "detached");
            assert_eq!(transaction.targets[1].app_type, "opencode");
            assert_eq!(transaction.targets[1].status, "applied");
            assert!(state
                .db
                .get_provider_by_id(&projected_provider_id(&definition, "opencode"), "opencode",)
                .expect("query OpenCode projection")
                .is_some());
        });
    }

    #[test]
    fn dsh_projection_apply_is_db_only_and_redacts_credentials() {
        with_test_home(|state| {
            let mut input = definition_input(None, None, "https://api.example.test/v1");
            input.name = "Shared DSH".to_string();
            input.app_types = vec!["dsh".to_string()];
            let definition = save_definition(state, input).expect("seed DSH definition");
            let rendered = projection(&definition, "test-only-secret".to_string(), "dsh")
                .expect("render DSH projection");
            ProviderService::add(state, AppType::DeepSeekHarness, rendered.clone(), false)
                .expect("save DSH projection directly");
            let normalized = state
                .db
                .get_provider_by_id(&rendered.id, "dsh")
                .expect("query normalized DSH projection")
                .expect("normalized DSH projection exists");
            assert_eq!(
                rendered.settings_config, normalized.settings_config,
                "DSH persistence must preserve projected settings"
            );
            state
                .db
                .delete_provider("dsh", &rendered.id)
                .expect("remove direct DSH projection fixture");

            let preview = preview_apply(state, &definition.id, vec!["dsh".to_string()])
                .expect("preview DSH projection without live RPC");
            assert_eq!(preview.targets.len(), 1);
            assert!(preview.targets[0].compatible);
            assert_eq!(preview.targets[0].connection_mode, "direct");

            let transaction = apply_transaction(
                state,
                &definition.id,
                vec!["dsh".to_string()],
                &preview.token,
                Some("dsh-db-only-projection"),
            )
            .expect("apply DSH projection without live RPC");
            assert_eq!(
                transaction.status, "applied",
                "DSH projection transaction failed: {:?}",
                transaction.targets
            );

            let projection = state
                .db
                .get_provider_by_id(&projected_provider_id(&definition, "dsh"), "dsh")
                .expect("query DSH projection")
                .expect("DSH projection exists");
            assert_eq!(
                projection
                    .meta
                    .as_ref()
                    .and_then(|meta| meta.live_config_managed),
                Some(false)
            );
            assert_eq!(
                projection.settings_config["credentialRef"]["source"],
                json!("providerCenter")
            );
            assert!(projection.settings_config.get("apiKey").is_none());
            assert!(!serde_json::to_string(&projection)
                .expect("serialize DSH projection")
                .contains("test-only-secret"));
        });
    }

    #[test]
    fn qoder_projection_uses_additive_template_without_implicit_activation() {
        with_test_home(|state| {
            let mut input = definition_input(None, None, "https://api.example.test/v1");
            input.name = "Shared Qoder".to_string();
            input.app_types = vec!["qoder".to_string()];
            let definition = save_definition(state, input).expect("seed Qoder definition");

            let preview = preview_apply(state, &definition.id, vec!["qoder".to_string()])
                .expect("preview Qoder projection");
            assert_eq!(preview.targets.len(), 1);
            assert!(preview.targets[0].compatible);
            assert_eq!(preview.targets[0].connection_mode, "direct");

            let transaction = apply_transaction(
                state,
                &definition.id,
                vec!["qoder".to_string()],
                &preview.token,
                Some("qoder-additive-projection"),
            )
            .expect("apply Qoder projection");
            assert_eq!(transaction.status, "applied");

            let projection_id = projected_provider_id(&definition, "qoder");
            let projected = state
                .db
                .get_provider_by_id(&projection_id, "qoder")
                .expect("query Qoder projection")
                .expect("Qoder projection exists");
            assert_eq!(projected.settings_config["protocol"], json!("openai"));
            assert!(projected.settings_config["models"].is_array());
            assert!(
                !crate::qoder_config::qoder_provider_exists(&projection_id)
                    .expect("query Qoder native membership"),
                "Provider Center registration must not silently activate an additive provider"
            );
        });
    }

    #[test]
    fn dsh_reapply_existing_live_managed_projection_is_offline_and_preserves_marker() {
        with_test_home(|state| {
            let mut input = definition_input(None, None, "https://api.example.test/v1");
            input.name = "Shared DSH".to_string();
            input.app_types = vec!["dsh".to_string()];
            let definition = save_definition(state, input).expect("seed DSH definition");
            let mut existing = projection(&definition, "test-only-secret".to_string(), "dsh")
                .expect("render existing DSH projection");
            existing
                .meta
                .get_or_insert_with(Default::default)
                .live_config_managed = Some(true);
            state
                .db
                .save_provider("dsh", &existing)
                .expect("seed live-managed DSH projection");

            let preview = preview_apply(state, &definition.id, vec!["dsh".to_string()])
                .expect("preview existing DSH projection without live RPC");
            let transaction = apply_transaction(
                state,
                &definition.id,
                vec!["dsh".to_string()],
                &preview.token,
                Some("dsh-offline-reapply-live-managed"),
            )
            .expect("reapply existing DSH projection without live RPC");
            assert_eq!(transaction.status, "applied");
            assert_eq!(transaction.targets[0].status, "applied");

            let reapplied = state
                .db
                .get_provider_by_id(&existing.id, "dsh")
                .expect("query reapplied DSH projection")
                .expect("reapplied DSH projection exists");
            assert_eq!(
                reapplied.meta.and_then(|meta| meta.live_config_managed),
                Some(true)
            );
        });
    }

    #[test]
    fn dsh_managed_edit_keeps_existing_secret_without_renderer_round_trip() {
        with_test_home(|state| {
            let mut definition_input = definition_input(None, None, "https://api.example.test/v1");
            definition_input.name = "Shared DSH".to_string();
            definition_input.app_types = vec!["dsh".to_string()];
            let definition = save_definition(state, definition_input).expect("seed DSH definition");
            let preview = preview_apply(state, &definition.id, vec!["dsh".to_string()])
                .expect("preview initial DSH projection");
            apply_transaction(
                state,
                &definition.id,
                vec!["dsh".to_string()],
                &preview.token,
                Some("dsh-managed-edit-seed"),
            )
            .expect("apply initial DSH projection");

            let projected_id = projected_provider_id(&definition, "dsh");
            let mut edited = state
                .db
                .get_provider_by_id(&projected_id, "dsh")
                .expect("query DSH projection")
                .expect("DSH projection exists");
            edited.name = "Updated DSH".to_string();
            edited.settings_config["displayName"] = json!("Updated DSH");
            assert!(edited.settings_config.get("apiKey").is_none());
            let input = ManagedProviderDraftInput {
                app_type: "dsh".to_string(),
                provider: edited,
                definition_id: definition.id.clone(),
                expected_revision: Some(definition.revision),
                target_app_types: vec!["dsh".to_string()],
            };

            let managed_preview = preview_managed_draft(state, input.clone())
                .expect("preview DSH managed edit without API key in renderer payload");
            let transaction = confirm_managed_draft(
                state,
                input,
                &managed_preview.token,
                Some("dsh-managed-edit-keep-secret"),
            )
            .expect("confirm DSH managed edit while keeping secret");
            assert_eq!(transaction.status, "applied");
            assert_eq!(
                get_secret(state, &definition.id).expect("read retained secret"),
                "test-only-secret"
            );
            let updated_definition = load_definitions(state)
                .expect("load definitions")
                .into_iter()
                .find(|item| item.id == definition.id)
                .expect("updated definition exists");
            assert_eq!(updated_definition.name, "Updated DSH");
            assert!(updated_definition.credential_configured);
        });
    }

    #[test]
    fn dsh_live_projection_must_be_removed_from_live_before_deletion() {
        with_test_home(|state| {
            let mut input = definition_input(None, None, "https://api.example.test/v1");
            input.name = "Shared DSH".to_string();
            input.app_types = vec!["dsh".to_string()];
            let definition = save_definition(state, input).expect("seed DSH definition");
            let mut projected = projection(&definition, "test-only-secret".to_string(), "dsh")
                .expect("render DSH projection");
            projected
                .meta
                .get_or_insert_with(Default::default)
                .live_config_managed = Some(true);
            state
                .db
                .save_provider("dsh", &projected)
                .expect("seed live-managed DSH projection");

            let error = delete_provider(state, &definition.id, "dsh", DeleteMode::RemoveCurrent)
                .expect_err("live-managed DSH projection deletion must be blocked");
            assert!(error.to_string().contains("先从 DeepSeek Harness 移除"));
            assert!(state
                .db
                .get_provider_by_id(&projected.id, "dsh")
                .expect("query protected DSH projection")
                .is_some());
            assert!(load_bindings(state)
                .expect("load protected DSH binding")
                .into_iter()
                .any(|binding| {
                    binding.provider_id == definition.id
                        && binding.app_type == "dsh"
                        && binding.enabled
                }));
            assert!(load_definitions(state)
                .expect("load protected DSH definition")
                .into_iter()
                .any(|item| item.id == definition.id));
        });
    }

    #[test]
    fn dsh_detach_keep_independent_moves_secret_ownership() {
        with_test_home(|state| {
            let mut input = definition_input(None, None, "https://api.example.test/v1");
            input.name = "Shared DSH".to_string();
            input.app_types = vec!["dsh".to_string()];
            let definition = save_definition(state, input).expect("seed DSH definition");
            let preview = preview_apply(state, &definition.id, vec!["dsh".to_string()])
                .expect("preview DSH projection");
            let transaction = apply_transaction(
                state,
                &definition.id,
                vec!["dsh".to_string()],
                &preview.token,
                Some("dsh-detach-secret-migration"),
            )
            .expect("apply DSH projection");
            assert_eq!(transaction.status, "applied");

            disable_binding(state, &definition.id, "dsh", false)
                .expect("detach DSH projection as independent provider");
            let projected_id = projected_provider_id(&definition, "dsh");
            let detached = state
                .db
                .get_provider_by_id(&projected_id, "dsh")
                .expect("query detached DSH provider")
                .expect("detached DSH provider exists");
            assert_eq!(detached.category, None);
            assert_eq!(
                detached.settings_config["credentialRef"],
                json!({ "source": "dshProvider", "id": projected_id })
            );
            assert_eq!(
                secure_store::get(state, SecretScope::DshProvider, &detached.id)
                    .expect("read migrated DSH secret"),
                "test-only-secret"
            );
            assert!(projection_secret_for_provider(
                state,
                &AppType::DeepSeekHarness,
                &detached,
                &definition.id,
            )
            .is_err());
        });
    }

    #[test]
    fn apply_transaction_rejects_stale_preview_token_without_writes() {
        with_test_home(|state| {
            let definition = seed_open_code_definition(state);
            let preview = preview_apply(state, &definition.id, vec!["opencode".to_string()])
                .expect("preview shared provider");
            let updated = save_definition(
                state,
                definition_input(
                    Some(definition.id.clone()),
                    Some(definition.revision),
                    "https://changed.example.test/v1",
                ),
            )
            .expect("update definition revision");

            let error = apply_transaction(
                state,
                &updated.id,
                vec!["opencode".to_string()],
                &preview.token,
                Some("stale-preview"),
            )
            .expect_err("stale token is rejected");

            assert!(error.to_string().contains("应用预览已过期"));
            let projection_id = format!("provider-center-opencode-{}", updated.id);
            assert!(state
                .db
                .get_provider_by_id(&projection_id, "opencode")
                .expect("query projection")
                .is_none());
            assert!(state
                .db
                .get_provider_center_transaction("stale-preview")
                .expect("query transaction")
                .is_none());
        });
    }

    #[test]
    fn apply_transaction_rejects_drift_without_writes() {
        with_test_home(|state| {
            let definition = seed_open_code_definition(state);
            let projection = projection(&definition, "test-only-secret".to_string(), "opencode")
                .expect("build expected projection");
            let expected = provider_fingerprint(&projection).expect("expected fingerprint");
            let mut drifted = projection.clone();
            drifted.settings_config["baseUrl"] =
                json!("https://externally-changed.example.test/v1");
            state
                .db
                .save_provider("opencode", &drifted)
                .expect("seed drifted projection");
            let mut bindings = load_bindings(state).expect("load bindings");
            bindings[0].expected_fingerprint = Some(expected);
            bindings[0].status = "applied".to_string();
            save_core(state, std::slice::from_ref(&definition), &bindings)
                .expect("save binding state");

            let preview = preview_apply(state, &definition.id, vec!["opencode".to_string()])
                .expect("preview detects drift");
            assert!(preview.targets[0].drifted);
            let error = apply_transaction(
                state,
                &definition.id,
                vec!["opencode".to_string()],
                &preview.token,
                Some("drifted-preview"),
            )
            .expect_err("drift is rejected");

            assert!(error.to_string().contains("已被外部修改"));
            let current = state
                .db
                .get_provider_by_id(&projection.id, "opencode")
                .expect("query projection")
                .expect("projection remains");
            assert_eq!(
                current.settings_config["baseUrl"],
                json!("https://externally-changed.example.test/v1")
            );
            assert!(state
                .db
                .get_provider_center_transaction("drifted-preview")
                .expect("query transaction")
                .is_none());
        });
    }

    #[test]
    fn apply_succeeds_without_live_config_write_when_path_is_blocked() {
        with_test_home(|state| {
            let definition = seed_open_code_and_openclaw_definition(state);
            let openclaw_path = crate::openclaw_config::get_openclaw_config_path();
            fs::create_dir_all(&openclaw_path).expect("make OpenClaw config path a directory");

            let preview = preview_apply(
                state,
                &definition.id,
                vec!["opencode".to_string(), "openclaw".to_string()],
            )
            .expect("preview shared provider");
            let transaction = apply_transaction(
                state,
                &definition.id,
                vec!["opencode".to_string(), "openclaw".to_string()],
                &preview.token,
                Some("apply-no-live-config-path-blocker"),
            )
            .expect("apply succeeds without live config write");

            // M4: AppAdapter::apply uses add_to_live=false, so both targets
            // succeed even when the live config path is blocked.
            assert_eq!(transaction.status, "applied");
            assert_eq!(transaction.targets[0].app_type, "opencode");
            assert_eq!(transaction.targets[0].status, "applied");
            assert_eq!(transaction.targets[1].app_type, "openclaw");
            assert_eq!(transaction.targets[1].status, "applied");
            assert!(state
                .db
                .get_provider_by_id(&projected_provider_id(&definition, "opencode"), "opencode")
                .expect("query OpenCode projection")
                .is_some());
            assert!(state
                .db
                .get_provider_by_id(&projected_provider_id(&definition, "openclaw"), "openclaw")
                .expect("query OpenClaw projection")
                .is_some());
            assert!(load_bindings(state)
                .expect("load bindings")
                .into_iter()
                .filter(|binding| binding.provider_id == definition.id)
                .all(|binding| binding.status == "applied"));
        });
    }

    #[test]
    fn confirm_managed_draft_has_zero_side_effects() {
        with_test_home(|state| {
            let provider = Provider::with_id(
                "test-managed-draft".to_string(),
                "Test Managed Draft".to_string(),
                json!({
                    "options": {
                        "baseURL": "https://api.example.test/v1",
                        "apiKey": "test-secret-key"
                    }
                }),
                None,
            );
            let definition_id = Uuid::new_v4().to_string();
            let input = ManagedProviderDraftInput {
                app_type: "opencode".to_string(),
                provider: provider.clone(),
                definition_id: definition_id.clone(),
                expected_revision: None,
                target_app_types: Vec::new(),
            };

            let preview =
                preview_managed_draft(state, input.clone()).expect("preview managed draft");
            assert_eq!(preview.targets.len(), 1);
            assert_eq!(preview.targets[0].app_type, "opencode");
            assert_eq!(preview.targets[0].operation, "create");
            assert!(preview.targets[0].compatible);

            let transaction = confirm_managed_draft(
                state,
                input,
                &preview.token,
                Some("confirm-managed-draft-zero-side-effects"),
            )
            .expect("confirm managed draft");

            assert_eq!(transaction.status, "applied");
            assert_eq!(transaction.targets[0].status, "applied");

            // The projected provider exists in DB.
            let projected = projection(
                &load_definitions(state)
                    .expect("load definitions")
                    .into_iter()
                    .find(|d| d.id == definition_id)
                    .expect("definition exists"),
                get_secret(state, &definition_id).expect("secret exists"),
                "opencode",
            )
            .expect("project");
            assert!(state
                .db
                .get_provider_by_id(&projected.id, "opencode")
                .expect("query projection")
                .is_some());

            // Zero side effects: no current provider set, no live config written.
            let current =
                ProviderService::current(state, AppType::OpenCode).expect("query current");
            assert!(current.is_empty(), "current provider should not be set");

            // The binding is applied.
            let binding = load_bindings(state)
                .expect("load bindings")
                .into_iter()
                .find(|b| b.provider_id == definition_id && b.app_type == "opencode")
                .expect("binding exists");
            assert_eq!(binding.status, "applied");
            assert!(binding.provider_template.is_some());
        });
    }

    #[test]
    fn confirm_codex_managed_draft_applies_all_targets_after_storage_normalization() {
        with_test_home(|state| {
            let common_config = r#"disable_response_storage = true
model_reasoning_effort = "low"

[features]
js_repl = false
"#;
            state
                .db
                .set_config_snippet("codex", Some(common_config.to_string()))
                .expect("save Codex common config");

            let mut provider = Provider::with_id(
                "source-codex-provider".to_string(),
                "Codex Shared Draft".to_string(),
                json!({
                    "auth": { "OPENAI_API_KEY": "test-secret-key" },
                    "config": r#"model_provider = "custom"
model = "gpt-test"
model_reasoning_effort = "low"
disable_response_storage = true

[model_providers.custom]
name = "custom"
base_url = "https://api.example.test/v1"
wire_api = "responses"
requires_openai_auth = true

[features]
js_repl = false
"#
                }),
                None,
            );
            provider.meta = Some(crate::provider::ProviderMeta {
                common_config_enabled: Some(true),
                api_format: Some("openai_responses".to_string()),
                ..Default::default()
            });
            let definition_id = Uuid::new_v4().to_string();
            let input = ManagedProviderDraftInput {
                app_type: "codex".to_string(),
                provider,
                definition_id: definition_id.clone(),
                expected_revision: None,
                target_app_types: vec![
                    "claude".to_string(),
                    "claude-desktop".to_string(),
                    "codex".to_string(),
                    "openclaw".to_string(),
                    "pi".to_string(),
                    "dsh".to_string(),
                ],
            };

            let preview =
                preview_managed_draft(state, input.clone()).expect("preview Codex managed draft");
            let transaction = confirm_managed_draft(
                state,
                input,
                &preview.token,
                Some("confirm-codex-common-config-normalization"),
            )
            .expect("confirm Codex managed draft");

            assert_eq!(transaction.status, "applied");
            assert!(transaction
                .targets
                .iter()
                .all(|target| target.status == "applied"));

            let definition = load_definitions(state)
                .expect("load definitions")
                .into_iter()
                .find(|definition| definition.id == definition_id)
                .expect("definition exists");
            let persisted = state
                .db
                .get_provider_by_id(&projected_provider_id(&definition, "codex"), "codex")
                .expect("query Codex projection")
                .expect("Codex projection exists");
            let binding = load_bindings(state)
                .expect("load bindings")
                .into_iter()
                .find(|binding| binding.provider_id == definition_id && binding.app_type == "codex")
                .expect("Codex binding exists");
            let persisted_fingerprint =
                provider_fingerprint(&persisted).expect("persisted fingerprint");

            assert_eq!(binding.status, "applied");
            assert_eq!(
                binding.expected_fingerprint.as_deref(),
                Some(persisted_fingerprint.as_str())
            );
            assert!(!persisted.settings_config["config"]
                .as_str()
                .expect("persisted Codex config")
                .contains("disable_response_storage"));
        });
    }

    #[test]
    fn confirm_managed_draft_explicit_openclaw_multi_target_has_zero_live_side_effects() {
        with_test_home(|state| {
            let openclaw_path = crate::openclaw_config::get_openclaw_config_path();
            fs::create_dir_all(&openclaw_path).expect("make OpenClaw config path a directory");
            let provider = Provider::with_id(
                "test-openclaw-managed-draft".to_string(),
                "Test OpenClaw Managed Draft".to_string(),
                json!({
                    "options": {
                        "baseURL": "https://api.example.test/v1",
                        "apiKey": "test-secret-key"
                    }
                }),
                None,
            );
            let definition_id = Uuid::new_v4().to_string();
            let input = ManagedProviderDraftInput {
                app_type: "opencode".to_string(),
                provider,
                definition_id: definition_id.clone(),
                expected_revision: None,
                target_app_types: vec![
                    "OpenCode".to_string(),
                    "openclaw".to_string(),
                    "OPENCLAW".to_string(),
                ],
            };
            assert_eq!(
                managed_draft_target_app_types(&input),
                vec!["opencode".to_string(), "openclaw".to_string()]
            );

            let preview = preview_managed_draft(state, input.clone())
                .expect("preview explicit multi-target managed draft");
            assert_eq!(
                preview
                    .targets
                    .iter()
                    .map(|target| target.app_type.as_str())
                    .collect::<Vec<_>>(),
                vec!["opencode", "openclaw"]
            );
            assert!(preview.targets.iter().all(|target| {
                target.compatible
                    && target.connection_mode == "direct"
                    && target.route_id.is_none()
                    && !target.requires_takeover
            }));

            let transaction = confirm_managed_draft(
                state,
                input,
                &preview.token,
                Some("confirm-openclaw-multi-zero-live-side-effects"),
            )
            .expect("confirm explicit multi-target managed draft");
            assert_eq!(transaction.status, "applied");
            assert_eq!(transaction.targets.len(), 2);
            assert!(transaction
                .targets
                .iter()
                .all(|target| target.status == "applied"));

            // A blocked OpenClaw live-config path proves neither target invokes
            // live projection. Confirm also leaves current-provider state empty.
            assert!(ProviderService::current(state, AppType::OpenCode)
                .expect("query OpenCode current provider")
                .is_empty());
            assert!(ProviderService::current(state, AppType::OpenClaw)
                .expect("query OpenClaw current provider")
                .is_empty());
            assert!(openclaw_path.is_dir());
            assert_eq!(
                load_bindings(state)
                    .expect("load bindings")
                    .into_iter()
                    .filter(|binding| binding.provider_id == definition_id)
                    .count(),
                2
            );
        });
    }

    #[test]
    fn confirm_managed_draft_is_idempotent() {
        with_test_home(|state| {
            let provider = Provider::with_id(
                "test-idempotent-draft".to_string(),
                "Test Idempotent Draft".to_string(),
                json!({
                    "options": {
                        "baseURL": "https://api.example.test/v1",
                        "apiKey": "test-secret-key"
                    }
                }),
                None,
            );
            let definition_id = Uuid::new_v4().to_string();
            let input = ManagedProviderDraftInput {
                app_type: "opencode".to_string(),
                provider: provider.clone(),
                definition_id: definition_id.clone(),
                expected_revision: None,
                target_app_types: Vec::new(),
            };

            let preview =
                preview_managed_draft(state, input.clone()).expect("preview managed draft");

            let idempotency_key = "confirm-managed-draft-idempotent";
            let first =
                confirm_managed_draft(state, input.clone(), &preview.token, Some(idempotency_key))
                    .expect("first confirm");
            assert_eq!(first.status, "applied");

            // Second call with the same idempotency key returns the same
            // transaction without re-applying.
            let second = confirm_managed_draft(state, input, &preview.token, Some(idempotency_key))
                .expect("second confirm");
            assert_eq!(second.id, first.id);
            assert_eq!(second.status, "applied");
        });
    }

    #[test]
    fn confirm_managed_draft_updates_all_bound_agents() {
        with_test_home(|state| {
            // Seed a definition with bindings for opencode and openclaw.
            let definition = seed_open_code_and_openclaw_definition(state);

            // Apply the definition so projections exist in DB.
            let preview = preview_apply(
                state,
                &definition.id,
                vec!["opencode".to_string(), "openclaw".to_string()],
            )
            .expect("preview apply");
            let applied = apply_transaction(
                state,
                &definition.id,
                vec!["opencode".to_string(), "openclaw".to_string()],
                &preview.token,
                Some("apply-for-edit-test"),
            )
            .expect("apply");
            assert_eq!(applied.status, "applied");

            // Edit the definition name via confirm_managed_draft with both targets.
            let provider = Provider::with_id(
                "test-edit-managed".to_string(),
                "Updated Shared Name".to_string(),
                json!({
                    "options": {
                        "baseURL": "https://api.example.test/v1",
                        "apiKey": "test-secret-key"
                    }
                }),
                None,
            );
            let input = ManagedProviderDraftInput {
                app_type: "opencode".to_string(),
                provider,
                definition_id: definition.id.clone(),
                expected_revision: Some(definition.revision),
                target_app_types: vec!["opencode".to_string(), "openclaw".to_string()],
            };

            let edit_preview = preview_managed_draft(state, input.clone()).expect("preview edit");
            let transaction = confirm_managed_draft(
                state,
                input,
                &edit_preview.token,
                Some("confirm-edit-multi-agent"),
            )
            .expect("confirm edit");
            assert_eq!(transaction.status, "applied");

            // Both projections should have the updated name from the shared definition.
            let opencode_projected = state
                .db
                .get_provider_by_id(&projected_provider_id(&definition, "opencode"), "opencode")
                .expect("query opencode")
                .expect("opencode projection exists");
            assert_eq!(opencode_projected.name, "Updated Shared Name");

            let openclaw_projected = state
                .db
                .get_provider_by_id(&projected_provider_id(&definition, "openclaw"), "openclaw")
                .expect("query openclaw")
                .expect("openclaw projection exists");
            assert_eq!(openclaw_projected.name, "Updated Shared Name");
        });
    }

    #[test]
    fn confirm_managed_draft_accepts_the_drift_shown_in_its_preview() {
        with_test_home(|state| {
            let definition = seed_open_code_and_openclaw_definition(state);
            let initial_preview = preview_apply(
                state,
                &definition.id,
                vec!["opencode".to_string(), "openclaw".to_string()],
            )
            .expect("preview initial projections");
            let initial = apply_transaction(
                state,
                &definition.id,
                vec!["opencode".to_string(), "openclaw".to_string()],
                &initial_preview.token,
                Some("apply-before-managed-drift-edit"),
            )
            .expect("apply initial projections");
            assert_eq!(initial.status, "applied");

            // Reproduce a stale pending binding left by an interrupted/rolled
            // back projection: the expected fingerprint remains, while the
            // projected provider row is missing.
            let projected_id = projected_provider_id(&definition, "opencode");
            state
                .db
                .delete_provider("opencode", &projected_id)
                .expect("remove projected provider");

            let provider = Provider::with_id(
                projected_id,
                "Updated Shared Name".to_string(),
                json!({
                    "options": {
                        "baseURL": "https://api.example.test/v1",
                        "apiKey": "test-secret-key"
                    }
                }),
                None,
            );
            let input = ManagedProviderDraftInput {
                app_type: "opencode".to_string(),
                provider,
                definition_id: definition.id.clone(),
                expected_revision: Some(definition.revision),
                target_app_types: vec!["opencode".to_string(), "openclaw".to_string()],
            };

            let edit_preview =
                preview_managed_draft(state, input.clone()).expect("preview drifted edit");
            assert!(edit_preview
                .targets
                .iter()
                .any(|target| target.app_type == "opencode" && target.drifted));

            let transaction = confirm_managed_draft(
                state,
                input,
                &edit_preview.token,
                Some("confirm-managed-drift-edit"),
            )
            .expect("explicitly confirmed drift should be applied");

            assert_eq!(transaction.status, "applied");
            assert!(state
                .db
                .get_provider_by_id(&projected_provider_id(&definition, "opencode"), "opencode")
                .expect("query repaired projection")
                .is_some());
        });
    }

    #[test]
    fn confirm_managed_draft_rejects_stale_revision() {
        with_test_home(|state| {
            let definition = seed_open_code_definition(state);

            let provider = Provider::with_id(
                "test-stale-revision".to_string(),
                "Stale Revision Test".to_string(),
                json!({
                    "options": {
                        "baseURL": "https://api.example.test/v1",
                        "apiKey": "test-secret-key"
                    }
                }),
                None,
            );
            let input = ManagedProviderDraftInput {
                app_type: "opencode".to_string(),
                provider,
                definition_id: definition.id.clone(),
                expected_revision: Some(999), // wrong revision
                target_app_types: Vec::new(),
            };

            let error =
                preview_managed_draft(state, input).expect_err("stale revision should be rejected");
            assert!(error.to_string().contains("版本"));
        });
    }

    #[test]
    fn disable_binding_rejects_drifted_projection() {
        with_test_home(|state| {
            let definition = seed_open_code_definition(state);

            // Apply so the projection exists and the binding has an expected fingerprint.
            let preview = preview_apply(state, &definition.id, vec!["opencode".to_string()])
                .expect("preview");
            let applied = apply_transaction(
                state,
                &definition.id,
                vec!["opencode".to_string()],
                &preview.token,
                Some("apply-for-drift-test"),
            )
            .expect("apply");
            assert_eq!(applied.status, "applied");

            // Simulate external drift: modify the projected provider's settings.
            let projected_id = projected_provider_id(&definition, "opencode");
            let mut projected = state
                .db
                .get_provider_by_id(&projected_id, "opencode")
                .expect("query")
                .expect("projection exists");
            // Change a non-credential settings field so the fingerprint differs.
            if let Some(options) = projected.settings_config.get_mut("options") {
                if let Some(base_url) = options.get_mut("baseURL") {
                    *base_url = json!("https://drifted.example.test/v1");
                }
            }
            state
                .db
                .save_provider("opencode", &projected)
                .expect("save drifted");

            // disable_binding should reject due to drift.
            let error = disable_binding(state, &definition.id, "opencode", true)
                .expect_err("drift should block disable");
            assert!(error.to_string().contains("漂移"));
        });
    }

    #[test]
    fn apply_then_restore_transaction_removes_new_projection_and_resets_binding() {
        with_test_home(|state| {
            let definition = seed_open_code_definition(state);
            let preview = preview_apply(state, &definition.id, vec!["opencode".to_string()])
                .expect("preview shared provider");
            let applied = apply_transaction(
                state,
                &definition.id,
                vec!["opencode".to_string()],
                &preview.token,
                Some("apply-then-restore"),
            )
            .expect("apply shared provider");

            assert_eq!(applied.status, "applied");
            assert_eq!(applied.targets[0].status, "applied");
            let projection_id = projected_provider_id(&definition, "opencode");
            assert!(state
                .db
                .get_provider_by_id(&projection_id, "opencode")
                .expect("query projection")
                .is_some());
            let applied_binding = load_bindings(state)
                .expect("load bindings")
                .into_iter()
                .find(|binding| binding.provider_id == definition.id)
                .expect("binding exists");
            assert_eq!(applied_binding.status, "applied");
            assert_eq!(applied_binding.applied_revision, Some(definition.revision));

            let restored = restore_transaction(state, &applied.id).expect("restore transaction");
            assert_eq!(restored.status, "restored");
            assert_eq!(restored.targets[0].status, "restored");
            assert!(state
                .db
                .get_provider_by_id(&projection_id, "opencode")
                .expect("query restored projection")
                .is_none());
            let restored_binding = load_bindings(state)
                .expect("load bindings")
                .into_iter()
                .find(|binding| binding.provider_id == definition.id)
                .expect("binding exists");
            assert_eq!(restored_binding.status, "pending");
            assert!(restored_binding.applied_revision.is_none());
            assert_eq!(restored_binding.last_transaction_id, Some(restored.id));
        });
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
    fn projection_fingerprint_ignores_credentials_and_display_metadata() {
        let mut first = projection(
            &definition("openai-chat"),
            "sk-first".to_string(),
            "openclaw",
        )
        .expect("first projection");
        let mut second = projection(
            &definition("openai-chat"),
            "sk-second".to_string(),
            "openclaw",
        )
        .expect("second projection");
        second.id = "another-id".to_string();
        second.name = "Another name".to_string();
        second.notes = Some("Another note".to_string());
        assert_eq!(
            provider_fingerprint(&first).unwrap(),
            provider_fingerprint(&second).unwrap()
        );

        first.settings_config["baseUrl"] = json!("https://changed.example/v1");
        assert_ne!(
            provider_fingerprint(&first).unwrap(),
            provider_fingerprint(&second).unwrap()
        );
    }

    #[test]
    fn embedded_config_fingerprint_ignores_api_keys_but_tracks_models() {
        let first = Provider::with_id(
            "one".to_string(),
            "One".to_string(),
            json!({ "config": "model = \"gpt-5\"\napi_key = \"sk-first\"\n" }),
            None,
        );
        let second = Provider::with_id(
            "two".to_string(),
            "Two".to_string(),
            json!({ "config": "model = \"gpt-5\"\napi_key = \"sk-second\"\n" }),
            None,
        );
        assert_eq!(
            provider_fingerprint(&first).unwrap(),
            provider_fingerprint(&second).unwrap()
        );

        let changed = Provider::with_id(
            "three".to_string(),
            "Three".to_string(),
            json!({ "config": "model = \"gpt-5.1\"\napi_key = \"sk-second\"\n" }),
            None,
        );
        assert_ne!(
            provider_fingerprint(&first).unwrap(),
            provider_fingerprint(&changed).unwrap()
        );
    }

    #[test]
    fn projections_enforce_protocol_compatibility_and_render_native_api_names() {
        // Codex × openai-chat is now Proxy-compatible (via local route transform).
        assert!(projection(&definition("openai-chat"), "sk-secret".to_string(), "codex").is_ok());
        // Codex × openai-responses is Direct.
        assert!(projection(
            &definition("openai-responses"),
            "sk-secret".to_string(),
            "codex"
        )
        .is_ok());
        // Claude × anthropic is Direct.
        assert!(projection(&definition("anthropic"), "sk-secret".to_string(), "claude").is_ok());
        // Claude × gemini is now Proxy-compatible (via local route transform).
        assert!(projection(&definition("gemini"), "sk-secret".to_string(), "claude").is_ok());
        // Gemini × openai-chat is Unsupported (no transform path for Gemini client).
        assert!(projection(
            &definition("openai-chat"),
            "sk-secret".to_string(),
            "gemini"
        )
        .is_err());

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
            model_definitions: Vec::new(),
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
        // Claude Desktop uses the local Anthropic gateway for non-native protocols.
        let desktop_proxy = projection(
            &definition("openai-chat"),
            "sk-secret".to_string(),
            "claude-desktop",
        )
        .expect("Claude Desktop supports OpenAI Chat through the local proxy");
        assert_eq!(
            desktop_proxy
                .meta
                .as_ref()
                .and_then(|meta| meta.claude_desktop_mode.as_ref()),
            Some(&crate::provider::ClaudeDesktopMode::Proxy)
        );
        assert!(!desktop_proxy
            .meta
            .as_ref()
            .expect("proxy metadata")
            .claude_desktop_model_routes
            .is_empty());
        crate::claude_desktop_config::validate_provider(&desktop_proxy)
            .expect("projected proxy provider must pass the real Desktop validator");
        assert!(projection(
            &definition("anthropic"),
            "sk-secret".to_string(),
            "claude-desktop"
        )
        .is_ok());
        // Claude Desktop × ollama is Unsupported.
        assert!(projection(
            &definition("ollama"),
            "sk-secret".to_string(),
            "claude-desktop"
        )
        .is_err());
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

    #[test]
    fn legacy_universal_conversion_marks_unknown_protocol_as_needs_review() {
        let mut legacy = legacy_universal();
        legacy.provider_type = "some-unknown-protocol".to_string();
        let (definition, _) = convert_legacy_universal(&legacy, "migrated-id".to_string(), 1234);
        assert_eq!(
            definition.protocol, "needs-review",
            "unknown protocols must not silently degrade to openai-chat"
        );
    }

    #[cfg(not(target_os = "windows"))]
    #[test]
    fn legacy_universal_migration_works_without_secret_on_non_windows() {
        use crate::database::Database;

        let db = Arc::new(Database::memory().expect("create memory database"));
        let state = AppState::new(db.clone());
        let mut legacy = legacy_universal();
        // Clear the API key so migration does not attempt keyring access,
        // which is unavailable in most non-Windows test environments.
        legacy.api_key = String::new();
        db.save_universal_provider(&legacy)
            .expect("seed plaintext legacy provider");

        ensure_sqlite_migrated(&state).expect("migrate legacy provider");

        let definitions = db
            .load_provider_center_definitions()
            .expect("load migrated definitions");
        assert_eq!(definitions.len(), 1);
        assert!(!definitions[0].credential_configured);
        let bindings = db.load_provider_center_bindings().unwrap();
        assert_eq!(bindings.len(), 2);
        assert!(db.get_all_universal_providers().unwrap().is_empty());

        // Idempotency: re-seed and re-run must not duplicate.
        let def_count = definitions.len();
        let bind_count = bindings.len();
        db.save_universal_provider(&legacy)
            .expect("simulate cleanup interruption");
        db.set_setting(SQLITE_MIGRATED_KEY, "false")
            .expect("force migration retry");

        ensure_sqlite_migrated(&state).expect("retry migration");

        assert_eq!(
            db.load_provider_center_definitions().unwrap().len(),
            def_count,
            "retry must not duplicate the definition"
        );
        assert_eq!(
            db.load_provider_center_bindings().unwrap().len(),
            bind_count,
            "retry must not duplicate bindings"
        );
        assert!(db.get_all_universal_providers().unwrap().is_empty());
    }

    #[cfg(target_os = "windows")]
    #[test]
    fn legacy_universal_migration_data_is_visible_and_editable() {
        use crate::database::Database;

        let db = Arc::new(Database::memory().expect("create memory database"));
        let state = AppState::new(db.clone());
        let legacy = legacy_universal();
        db.save_universal_provider(&legacy)
            .expect("seed plaintext legacy provider");

        ensure_sqlite_migrated(&state).expect("migrate legacy provider");

        // After migration the definition must be visible through the normal
        // load path (which also resolves credential metadata).
        let definitions = load_definitions(&state).expect("load definitions post-migration");
        let migrated = definitions
            .iter()
            .find(|d| {
                d.source
                    .as_ref()
                    .map_or(false, |s| s.source_ref == "universal:legacy-shared")
            })
            .expect("migrated definition must be visible");
        assert!(migrated.credential_configured);
        assert_eq!(migrated.name, "Legacy Shared");

        // The definition must be editable — saving a name change must persist
        // and bump the revision.
        let edit_input = SaveProviderDefinitionInput {
            id: Some(migrated.id.clone()),
            expected_revision: Some(migrated.revision),
            name: "Renamed Legacy".to_string(),
            protocol: migrated.protocol.clone(),
            base_url: migrated.base_url.clone(),
            models: migrated.models.clone(),
            model_definitions: migrated.model_definitions.clone(),
            notes: migrated.notes.clone(),
            enabled: Some(true),
            credential_action: Some("keep".to_string()),
            source: migrated.source.clone(),
            api_key: None,
            app_types: vec!["claude".to_string(), "codex".to_string()],
        };
        save_definition(&state, edit_input).expect("edit migrated definition");

        let refreshed = load_definitions(&state).expect("reload definitions");
        let updated = refreshed
            .iter()
            .find(|d| d.id == migrated.id)
            .expect("edited definition must still be visible");
        assert_eq!(updated.name, "Renamed Legacy");
        assert!(updated.revision > migrated.revision);
    }

    #[test]
    fn agent_provider_catalog_classifies_projection_managed_and_native_providers() {
        use crate::database::Database;
        use crate::provider::ProviderMeta;

        let db = Arc::new(Database::memory().expect("create memory database"));
        let state = AppState::new(db.clone());
        let definition = definition("ollama");
        let projected = projection(&definition, String::new(), "opencode").expect("projection");
        db.save_provider("opencode", &projected)
            .expect("save projection");
        let persisted_projection = db
            .get_provider_by_id(&projected.id, "opencode")
            .expect("read projection")
            .expect("projection exists");
        let fingerprint = provider_fingerprint(&persisted_projection).expect("fingerprint");
        let binding = ProviderBinding {
            provider_id: definition.id.clone(),
            app_type: "opencode".to_string(),
            status: "applied".to_string(),
            enabled: true,
            override_enabled: false,
            provider_template: None,
            applied_revision: Some(definition.revision),
            expected_fingerprint: Some(fingerprint),
            last_error: None,
            last_transaction_id: None,
            updated_at: 1,
        };
        save_core(&state, std::slice::from_ref(&definition), &[binding])
            .expect("save Provider Center state");
        let managed = Provider::with_id(
            "managed".to_string(),
            "Managed".to_string(),
            json!({ "api": "openai" }),
            None,
        );
        db.save_provider("opencode", &managed)
            .expect("save managed provider");
        let mut account = Provider::with_id(
            "copilot".to_string(),
            "Copilot".to_string(),
            json!({ "api": "openai" }),
            None,
        );
        account.meta = Some(ProviderMeta {
            provider_type: Some("github_copilot".to_string()),
            ..ProviderMeta::default()
        });
        db.save_provider("opencode", &account)
            .expect("save managed account provider");

        let catalog = agent_provider_catalog(&state, "opencode").expect("catalog");
        let projected_item = catalog
            .items
            .iter()
            .find(|item| item.provider_id == projected.id)
            .expect("projection item");
        assert_eq!(projected_item.scope, "universal");
        assert_eq!(projected_item.ownership, "providerCenterProjection");
        assert_eq!(projected_item.definition_id.as_deref(), Some("shared"));
        assert_eq!(projected_item.binding_status.as_deref(), Some("applied"));
        assert_eq!(projected_item.applied_revision, Some(1));
        assert!(!projected_item.drifted);
        assert!(projected_item.read_only);

        let managed_item = catalog
            .items
            .iter()
            .find(|item| item.provider_id == managed.id)
            .expect("managed item");
        assert_eq!(managed_item.scope, "agentOnly");
        assert_eq!(managed_item.ownership, "ccSwitchManaged");
        assert!(!managed_item.read_only);

        let account_item = catalog
            .items
            .iter()
            .find(|item| item.provider_id == account.id)
            .expect("managed account item");
        assert_eq!(account_item.scope, "nativeAccount");
        assert_eq!(account_item.ownership, "ccSwitchManaged");
        assert!(!account_item.read_only);
    }

    #[test]
    fn codex_official_is_always_agent_native_account() {
        use crate::database::{Database, CODEX_OFFICIAL_PROVIDER_ID};

        let db = Arc::new(Database::memory().expect("create memory database"));
        let state = AppState::new(db.clone());
        let provider = Provider::with_id(
            CODEX_OFFICIAL_PROVIDER_ID.to_string(),
            "Codex Official".to_string(),
            json!({}),
            None,
        );
        db.save_provider("codex", &provider)
            .expect("save official provider without category metadata");

        let catalog = agent_provider_catalog(&state, "codex").expect("catalog");
        let official = catalog
            .items
            .iter()
            .find(|item| item.provider_id == CODEX_OFFICIAL_PROVIDER_ID)
            .expect("codex official item");
        assert_eq!(official.scope, "nativeAccount");
        assert_eq!(official.ownership, "agentNative");
        assert!(!official.read_only);
    }

    #[test]
    fn projection_mutation_guard_blocks_projection_but_not_activation_path() {
        use crate::database::Database;

        let db = Arc::new(Database::memory().expect("create memory database"));
        let state = AppState::new(db.clone());
        let mut projected = Provider::with_id(
            "provider-center-codex-shared".to_string(),
            "Projected".to_string(),
            json!({}),
            None,
        );
        projected.category = Some("provider-center".to_string());
        db.save_provider("codex", &projected)
            .expect("save projection");
        let ordinary = Provider::with_id(
            "ordinary".to_string(),
            "Ordinary".to_string(),
            json!({}),
            None,
        );
        db.save_provider("codex", &ordinary)
            .expect("save ordinary provider");

        let error = guard_projection_mutation(&state, &AppType::Codex, &projected.id)
            .expect_err("projection mutation must be rejected");
        assert!(error.to_string().contains("Provider Center"));
        guard_projection_mutation(&state, &AppType::Codex, &ordinary.id)
            .expect("ordinary provider remains mutable");
        // Switching does not call this mutation-only guard.
    }

    #[test]
    fn scan_excludes_provider_center_projections() {
        use crate::database::Database;

        let db = Arc::new(Database::memory().expect("create memory database"));
        let state = AppState::new(db.clone());
        let definition = definition("ollama");
        let projected = projection(&definition, String::new(), "opencode").expect("projection");
        db.save_provider("opencode", &projected)
            .expect("save projection");
        let ordinary = Provider::with_id(
            "ordinary".to_string(),
            "Ordinary".to_string(),
            json!({
                "npm": "@ai-sdk/openai-compatible",
                "options": { "baseURL": "https://ordinary.example/v1", "apiKey": "sk-local" },
                "models": { "model-a": { "name": "model-a" } }
            }),
            None,
        );
        db.save_provider("opencode", &ordinary)
            .expect("save ordinary provider");
        let binding = ProviderBinding {
            provider_id: definition.id.clone(),
            app_type: "opencode".to_string(),
            status: "applied".to_string(),
            enabled: true,
            override_enabled: false,
            provider_template: None,
            applied_revision: Some(definition.revision),
            expected_fingerprint: Some(provider_fingerprint(&projected).unwrap()),
            last_error: None,
            last_transaction_id: None,
            updated_at: 1,
        };
        save_core(&state, std::slice::from_ref(&definition), &[binding])
            .expect("save Provider Center state");

        let requested = HashSet::from(["opencode".to_string()]);
        let (candidates, _) =
            scan_imports_for_apps(&state, &requested).expect("scan import candidates");
        assert!(candidates
            .iter()
            .all(|candidate| candidate.source_ref != format!("saved:opencode:{}", projected.id)));
        let ordinary_candidate = candidates
            .iter()
            .find(|candidate| candidate.source_ref == "saved:opencode:ordinary")
            .expect("ordinary provider remains importable");
        assert_eq!(ordinary_candidate.source_kind, "localManaged");
    }

    #[test]
    fn live_projection_fingerprint_is_not_importable() {
        use crate::database::Database;

        let db = Arc::new(Database::memory().expect("create memory database"));
        let state = AppState::new(db);
        let definition = definition("ollama");
        let projected = projection(&definition, String::new(), "opencode").expect("projection");
        let binding = ProviderBinding {
            provider_id: definition.id.clone(),
            app_type: "opencode".to_string(),
            status: "applied".to_string(),
            enabled: true,
            override_enabled: false,
            provider_template: None,
            applied_revision: Some(definition.revision),
            expected_fingerprint: Some(provider_fingerprint(&projected).unwrap()),
            last_error: None,
            last_transaction_id: None,
            updated_at: 1,
        };
        save_core(&state, std::slice::from_ref(&definition), &[binding])
            .expect("save Provider Center state");
        let live = Provider::with_id(
            "live".to_string(),
            "OpenCode current settings".to_string(),
            projected.settings_config,
            None,
        );

        let error = ensure_importable_source(&state, &AppType::OpenCode, &live, "live:opencode")
            .expect_err("live projection cannot be imported");
        assert!(error.to_string().contains("不能再次导入"));
    }

    #[test]
    fn save_definition_requires_explicit_detach_for_removed_app() {
        with_test_home(|state| {
            let mut input = definition_input(None, None, "http://127.0.0.1:11434");
            input.protocol = "ollama".to_string();
            input.credential_action = Some("clear".to_string());
            input.api_key = None;
            let definition = save_definition(state, input).expect("save ollama definition");
            let projected =
                projection(&definition, String::new(), "opencode").expect("build projection");
            state
                .db
                .save_provider("opencode", &projected)
                .expect("save projection");
            let mut bindings = load_bindings(state).expect("load binding");
            let binding = bindings
                .iter_mut()
                .find(|binding| binding.provider_id == definition.id)
                .expect("binding exists");
            binding.status = "applied".to_string();
            binding.applied_revision = Some(definition.revision);
            binding.expected_fingerprint = Some(provider_fingerprint(&projected).unwrap());
            save_core(state, std::slice::from_ref(&definition), &bindings)
                .expect("save applied binding");

            let mut update = definition_input(
                Some(definition.id.clone()),
                Some(definition.revision),
                "http://127.0.0.1:11434",
            );
            update.protocol = "ollama".to_string();
            update.credential_action = Some("keep".to_string());
            update.api_key = None;
            update.app_types.clear();
            let error = save_definition(state, update)
                .expect_err("save cannot implicitly detach an existing binding");

            assert!(error.to_string().contains("显式选择"));
            let stored_definition = load_definitions(state)
                .expect("load definitions")
                .into_iter()
                .find(|item| item.id == definition.id)
                .expect("definition remains");
            assert_eq!(stored_definition.revision, definition.revision);
            let stored_binding = load_bindings(state)
                .expect("load bindings")
                .into_iter()
                .find(|item| item.provider_id == definition.id && item.app_type == "opencode")
                .expect("binding remains");
            assert!(stored_binding.enabled);
            assert_eq!(stored_binding.status, "applied");
            let stored_projection = state
                .db
                .get_provider_by_id(&projected.id, "opencode")
                .expect("query projection")
                .expect("projection remains");
            assert_eq!(
                stored_projection.category.as_deref(),
                Some("provider-center")
            );
        });
    }

    #[test]
    fn preview_apply_does_not_persist_definition_binding_or_projection() {
        with_test_home(|state| {
            let definition = seed_open_code_definition(state);
            let definitions_before =
                serde_json::to_value(load_definitions(state).expect("load definitions"))
                    .expect("serialize definitions");
            let bindings_before =
                serde_json::to_value(load_bindings(state).expect("load bindings"))
                    .expect("serialize bindings");

            let preview = preview_apply(state, &definition.id, vec!["opencode".to_string()])
                .expect("preview shared provider");

            assert_eq!(preview.targets.len(), 1);
            assert_eq!(
                serde_json::to_value(load_definitions(state).expect("reload definitions"))
                    .expect("serialize definitions"),
                definitions_before
            );
            assert_eq!(
                serde_json::to_value(load_bindings(state).expect("reload bindings"))
                    .expect("serialize bindings"),
                bindings_before
            );
            assert!(state
                .db
                .get_provider_by_id(&projected_provider_id(&definition, "opencode"), "opencode",)
                .expect("query projection")
                .is_none());
            assert!(load_transactions(state)
                .expect("load transactions")
                .is_empty());
        });
    }

    #[test]
    fn detach_keep_converts_projection_to_mutable_local_provider() {
        use crate::database::Database;

        let db = Arc::new(Database::memory().expect("create memory database"));
        let state = AppState::new(db.clone());
        let definition = definition("ollama");
        let projected = projection(&definition, String::new(), "opencode").expect("projection");
        db.save_provider("opencode", &projected)
            .expect("save projection");
        let binding = ProviderBinding {
            provider_id: definition.id.clone(),
            app_type: "opencode".to_string(),
            status: "applied".to_string(),
            enabled: true,
            override_enabled: false,
            provider_template: None,
            applied_revision: Some(definition.revision),
            expected_fingerprint: Some(provider_fingerprint(&projected).unwrap()),
            last_error: None,
            last_transaction_id: None,
            updated_at: 1,
        };
        save_core(&state, std::slice::from_ref(&definition), &[binding])
            .expect("save Provider Center state");

        let source_ref = format!("saved:opencode:{}", projected.id);
        let error = import_candidate(&state, &source_ref, Vec::new())
            .expect_err("active projection cannot be imported");
        assert!(error.to_string().contains("不能再次导入"));

        disable_binding(&state, &definition.id, "opencode", false)
            .expect("detach and keep local copy");
        let detached = db
            .get_provider_by_id(&projected.id, "opencode")
            .unwrap()
            .expect("local copy remains");
        assert_eq!(detached.category, None);
        guard_projection_mutation(&state, &AppType::OpenCode, &detached.id)
            .expect("detached local copy is mutable");
        let catalog = agent_provider_catalog(&state, "opencode").expect("catalog");
        let item = catalog
            .items
            .iter()
            .find(|item| item.provider_id == detached.id)
            .expect("detached provider item");
        assert_eq!(item.scope, "agentOnly");
        assert_eq!(item.ownership, "ccSwitchManaged");
        assert!(!item.read_only);
        let requested = HashSet::from(["opencode".to_string()]);
        let (candidates, _) =
            scan_imports_for_apps(&state, &requested).expect("scan detached local copy");
        assert!(candidates.iter().any(|candidate| {
            candidate.source_ref == source_ref && candidate.source_kind == "localManaged"
        }));
    }

    #[test]
    fn detach_remove_deletes_projection_and_disables_binding() {
        use crate::database::Database;

        let db = Arc::new(Database::memory().expect("create memory database"));
        let state = AppState::new(db.clone());
        let definition = definition("ollama");
        let projected = projection(&definition, String::new(), "opencode").expect("projection");
        db.save_provider("opencode", &projected)
            .expect("save projection");
        let binding = ProviderBinding {
            provider_id: definition.id.clone(),
            app_type: "opencode".to_string(),
            status: "applied".to_string(),
            enabled: true,
            override_enabled: false,
            provider_template: None,
            applied_revision: Some(definition.revision),
            expected_fingerprint: Some(provider_fingerprint(&projected).unwrap()),
            last_error: None,
            last_transaction_id: None,
            updated_at: 1,
        };
        save_core(&state, std::slice::from_ref(&definition), &[binding])
            .expect("save Provider Center state");

        disable_binding(&state, &definition.id, "opencode", true)
            .expect("detach and remove projection");

        assert!(db
            .get_provider_by_id(&projected.id, "opencode")
            .expect("query projection")
            .is_none());
        let detached_binding = load_bindings(&state)
            .expect("load binding")
            .into_iter()
            .find(|item| item.provider_id == definition.id && item.app_type == "opencode")
            .expect("binding remains as detached history");
        assert!(!detached_binding.enabled);
        assert_eq!(detached_binding.status, "detached");
        assert!(detached_binding.applied_revision.is_none());
        assert!(detached_binding.expected_fingerprint.is_none());
    }

    #[test]
    fn definition_delete_requires_explicit_binding_detach() {
        use crate::database::Database;

        let db = Arc::new(Database::memory().expect("create memory database"));
        let state = AppState::new(db.clone());
        let definition = definition("ollama");
        let binding = ProviderBinding {
            provider_id: definition.id.clone(),
            app_type: "opencode".to_string(),
            status: "pending".to_string(),
            enabled: true,
            override_enabled: false,
            provider_template: None,
            applied_revision: None,
            expected_fingerprint: None,
            last_error: None,
            last_transaction_id: None,
            updated_at: 1,
        };
        save_core(&state, std::slice::from_ref(&definition), &[binding])
            .expect("save Provider Center state");

        let error = delete_definition(&state, &definition.id)
            .expect_err("active bindings must block direct deletion");
        assert!(error.to_string().contains("请先选择"));
        disable_binding(&state, &definition.id, "opencode", false).expect("detach binding");
        delete_definition(&state, &definition.id).expect("delete detached definition");
        assert!(db.load_provider_center_definitions().unwrap().is_empty());
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
        assert_eq!(candidate.source_kind, "localManaged");
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

    #[test]
    fn delete_remove_current_only_affects_target_agent() {
        with_test_home(|state| {
            let definition = seed_open_code_and_openclaw_definition(state);
            let preview = preview_apply(
                state,
                &definition.id,
                vec!["opencode".to_string(), "openclaw".to_string()],
            )
            .expect("preview");
            apply_transaction(
                state,
                &definition.id,
                vec!["opencode".to_string(), "openclaw".to_string()],
                &preview.token,
                Some("apply-for-delete-remove-test"),
            )
            .expect("apply");

            delete_provider(state, &definition.id, "opencode", DeleteMode::RemoveCurrent)
                .expect("delete remove current");

            let opencode_id = projected_provider_id(&definition, "opencode");
            assert!(state
                .db
                .get_provider_by_id(&opencode_id, "opencode")
                .expect("query")
                .is_none());

            let openclaw_id = projected_provider_id(&definition, "openclaw");
            assert!(state
                .db
                .get_provider_by_id(&openclaw_id, "openclaw")
                .expect("query")
                .is_some());

            let bindings = load_bindings(state).expect("load bindings");
            let opencode_binding = bindings
                .iter()
                .find(|b| b.provider_id == definition.id && b.app_type == "opencode")
                .expect("opencode binding");
            assert!(!opencode_binding.enabled);
            assert_eq!(opencode_binding.status, "detached");

            let openclaw_binding = bindings
                .iter()
                .find(|b| b.provider_id == definition.id && b.app_type == "openclaw")
                .expect("openclaw binding");
            assert!(openclaw_binding.enabled);
            assert_eq!(openclaw_binding.status, "applied");

            let definitions = load_definitions(state).expect("load definitions");
            assert!(definitions.iter().any(|d| d.id == definition.id));
        });
    }

    #[test]
    fn delete_detach_keep_independent_preserves_config() {
        with_test_home(|state| {
            let definition = seed_open_code_and_openclaw_definition(state);
            let preview = preview_apply(
                state,
                &definition.id,
                vec!["opencode".to_string(), "openclaw".to_string()],
            )
            .expect("preview");
            apply_transaction(
                state,
                &definition.id,
                vec!["opencode".to_string(), "openclaw".to_string()],
                &preview.token,
                Some("apply-for-detach-test"),
            )
            .expect("apply");

            delete_provider(
                state,
                &definition.id,
                "opencode",
                DeleteMode::DetachKeepIndependent,
            )
            .expect("delete detach keep");

            let projected_id = projected_provider_id(&definition, "opencode");
            let projected = state
                .db
                .get_provider_by_id(&projected_id, "opencode")
                .expect("query")
                .expect("projection still exists");
            assert!(projected.category.is_none());

            let bindings = load_bindings(state).expect("load bindings");
            let opencode_binding = bindings
                .iter()
                .find(|b| b.provider_id == definition.id && b.app_type == "opencode")
                .expect("opencode binding");
            assert!(!opencode_binding.enabled);
            assert_eq!(opencode_binding.status, "detached");

            // OpenClaw binding should still be active.
            let openclaw_binding = bindings
                .iter()
                .find(|b| b.provider_id == definition.id && b.app_type == "openclaw")
                .expect("openclaw binding");
            assert!(openclaw_binding.enabled);
        });
    }

    #[test]
    fn delete_globally_clears_all_projections_and_definition() {
        with_test_home(|state| {
            let definition = seed_open_code_and_openclaw_definition(state);
            let preview = preview_apply(
                state,
                &definition.id,
                vec!["opencode".to_string(), "openclaw".to_string()],
            )
            .expect("preview");
            apply_transaction(
                state,
                &definition.id,
                vec!["opencode".to_string(), "openclaw".to_string()],
                &preview.token,
                Some("apply-for-global-delete-test"),
            )
            .expect("apply");

            delete_provider(
                state,
                &definition.id,
                "opencode",
                DeleteMode::DeleteGlobally,
            )
            .expect("delete globally");

            let opencode_id = projected_provider_id(&definition, "opencode");
            assert!(state
                .db
                .get_provider_by_id(&opencode_id, "opencode")
                .expect("query")
                .is_none());
            let openclaw_id = projected_provider_id(&definition, "openclaw");
            assert!(state
                .db
                .get_provider_by_id(&openclaw_id, "openclaw")
                .expect("query")
                .is_none());

            let bindings = load_bindings(state).expect("load bindings");
            assert!(bindings.iter().all(|b| b.provider_id != definition.id));

            let definitions = load_definitions(state).expect("load definitions");
            assert!(!definitions.iter().any(|d| d.id == definition.id));

            assert!(get_secret(state, &definition.id).is_err());
        });
    }

    #[test]
    fn delete_auto_cleans_on_last_binding_removal() {
        with_test_home(|state| {
            let definition = seed_open_code_definition(state);
            let preview = preview_apply(state, &definition.id, vec!["opencode".to_string()])
                .expect("preview");
            apply_transaction(
                state,
                &definition.id,
                vec!["opencode".to_string()],
                &preview.token,
                Some("apply-for-auto-clean-test"),
            )
            .expect("apply");

            delete_provider(state, &definition.id, "opencode", DeleteMode::RemoveCurrent)
                .expect("delete remove current");

            let definitions = load_definitions(state).expect("load definitions");
            assert!(!definitions.iter().any(|d| d.id == definition.id));

            let bindings = load_bindings(state).expect("load bindings");
            assert!(bindings.iter().all(|b| b.provider_id != definition.id));

            assert!(get_secret(state, &definition.id).is_err());
        });
    }

    #[test]
    fn delete_blocks_when_projection_is_in_use() {
        with_test_home(|state| {
            let mut input = definition_input(None, None, "https://api.example.test/v1");
            input.app_types = vec!["codex".to_string()];
            let definition = save_definition(state, input).expect("seed codex definition");

            let preview =
                preview_apply(state, &definition.id, vec!["codex".to_string()]).expect("preview");
            apply_transaction(
                state,
                &definition.id,
                vec!["codex".to_string()],
                &preview.token,
                Some("apply-for-in-use-test"),
            )
            .expect("apply");

            let projected_id = projected_provider_id(&definition, "codex");
            state
                .db
                .set_current_provider("codex", &projected_id)
                .expect("set current");

            let error = delete_provider(state, &definition.id, "codex", DeleteMode::RemoveCurrent)
                .expect_err("in-use should block delete");
            assert!(error.to_string().contains("正在"));

            assert!(state
                .db
                .get_provider_by_id(&projected_id, "codex")
                .expect("query")
                .is_some());
        });
    }
}
