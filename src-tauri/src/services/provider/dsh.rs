use crate::error::AppError;
use crate::provider::Provider;
use crate::secure_store::{self, SecretScope};
use crate::store::AppState;
use futures::StreamExt;
use serde::{Deserialize, Serialize};
use serde_json::{json, Map, Value};
use std::fmt;
use std::future::Future;
use std::time::Duration;
use uuid::Uuid;

const DSH_BASE_URL: &str = "http://127.0.0.1:3080";
const SETTINGS_NAMESPACE: &str = "llm-pi-ai";
const MAX_RESPONSE_BYTES: usize = 1024 * 1024;
const MAX_CAS_ATTEMPTS: usize = 3;

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProviderLiveMembership {
    pub status: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub provider_ids: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

impl ProviderLiveMembership {
    pub fn available(provider_ids: Vec<String>) -> Self {
        Self {
            status: "available",
            provider_ids: Some(provider_ids),
            error: None,
        }
    }

    pub fn unavailable(error: impl Into<String>) -> Self {
        Self {
            status: "unavailable",
            provider_ids: None,
            error: Some(error.into()),
        }
    }
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
struct CredentialReference {
    source: String,
    id: String,
}

#[derive(Debug, Clone)]
struct PreparedProvider {
    provider: Provider,
    profile: Value,
    credential_ref: Option<CredentialReference>,
    credential_name: Option<String>,
    submitted_secret: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct SettingsDescribe {
    writable: bool,
    namespaces: Vec<SettingsNamespace>,
}

#[derive(Debug, Clone, Deserialize)]
struct SettingsNamespace {
    ns: String,
    value: Value,
    revision: u64,
}

#[derive(Debug)]
struct RpcFailure {
    code: Option<String>,
    message: String,
}

impl RpcFailure {
    fn transport(message: impl Into<String>) -> Self {
        Self {
            code: None,
            message: message.into(),
        }
    }

    fn is_conflict(&self) -> bool {
        self.code.as_deref() == Some("settings/conflict")
            || self.code.as_deref() == Some("gateway/conflict")
    }

    fn into_app_error(self) -> AppError {
        let message = match self.code {
            Some(code) => format!("DSH RPC {code}: {}", self.message),
            None => self.message,
        };
        AppError::Message(message)
    }
}

impl fmt::Display for RpcFailure {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match &self.code {
            Some(code) => write!(formatter, "DSH RPC {code}: {}", self.message),
            None => formatter.write_str(&self.message),
        }
    }
}

#[derive(Clone)]
struct DshRpcClient {
    base_url: String,
    client: reqwest::Client,
}

impl DshRpcClient {
    fn loopback() -> Result<Self, AppError> {
        Self::new(DSH_BASE_URL)
    }

    fn new(base_url: &str) -> Result<Self, AppError> {
        let parsed = reqwest::Url::parse(base_url)
            .map_err(|error| AppError::Message(format!("DSH 地址无效: {error}")))?;
        if parsed.scheme() != "http"
            || parsed.host_str() != Some("127.0.0.1")
            || (parsed.path() != "/" && !parsed.path().is_empty())
            || parsed.query().is_some()
            || parsed.fragment().is_some()
        {
            return Err(AppError::Message(
                "DSH RPC 仅允许连接固定的本机 loopback HTTP 服务".to_string(),
            ));
        }
        let client = reqwest::Client::builder()
            .no_proxy()
            .connect_timeout(Duration::from_secs(2))
            .timeout(Duration::from_secs(6))
            .build()
            .map_err(|error| AppError::Message(format!("无法创建 DSH RPC 客户端: {error}")))?;
        Ok(Self {
            base_url: base_url.trim_end_matches('/').to_string(),
            client,
        })
    }

    async fn call(&self, endpoint: &str, args: Value) -> Result<Value, RpcFailure> {
        if !matches!(
            endpoint,
            "settings/describe"
                | "settings/mutate"
                | "credentials/describe"
                | "credentials/set"
                | "credentials/unset"
        ) {
            return Err(RpcFailure::transport("拒绝调用未登记的 DSH RPC 方法"));
        }

        let rpc_id = Uuid::new_v4().to_string();
        let response = self
            .client
            .post(format!("{}/api/{endpoint}", self.base_url))
            .header(reqwest::header::CONTENT_TYPE, "application/json")
            .json(&json!({
                "type": "client-request",
                "rpcId": rpc_id,
                "method": endpoint,
                "payload": { "args": args },
            }))
            .send()
            .await
            .map_err(|error| {
                RpcFailure::transport(if error.is_timeout() {
                    "连接 DSH 超时；请确认 DeepSeek Harness Web 服务正在运行".to_string()
                } else if error.is_connect() {
                    "无法连接 DSH；请先启动 DeepSeek Harness".to_string()
                } else {
                    format!("DSH RPC 请求失败: {error}")
                })
            })?;

        if !response.status().is_success() {
            return Err(RpcFailure::transport(format!(
                "DSH RPC 返回 HTTP {}",
                response.status()
            )));
        }
        if response
            .content_length()
            .is_some_and(|length| length > MAX_RESPONSE_BYTES as u64)
        {
            return Err(RpcFailure::transport("DSH RPC 响应超过安全上限"));
        }

        let mut body = Vec::new();
        let mut stream = response.bytes_stream();
        while let Some(chunk) = stream.next().await {
            let chunk = chunk.map_err(|error| {
                RpcFailure::transport(format!("读取 DSH RPC 响应失败: {error}"))
            })?;
            if body.len().saturating_add(chunk.len()) > MAX_RESPONSE_BYTES {
                return Err(RpcFailure::transport("DSH RPC 响应超过安全上限"));
            }
            body.extend_from_slice(&chunk);
        }

        let envelope: Value = serde_json::from_slice(&body)
            .map_err(|_| RpcFailure::transport("DSH RPC 返回了无效 JSON"))?;
        let object = envelope
            .as_object()
            .ok_or_else(|| RpcFailure::transport("DSH RPC 响应 envelope 无效"))?;
        if object.get("type").and_then(Value::as_str) != Some("server-response") {
            return Err(RpcFailure::transport("目标服务不是受支持的 DSH RPC 服务"));
        }
        if object.get("rpcId").and_then(Value::as_str) != Some(rpc_id.as_str()) {
            return Err(RpcFailure::transport("DSH RPC 响应 rpcId 不匹配"));
        }
        let result = object
            .get("result")
            .and_then(Value::as_object)
            .ok_or_else(|| RpcFailure::transport("DSH RPC result 无效"))?;
        match result.get("ok").and_then(Value::as_bool) {
            Some(true) => Ok(result.get("value").cloned().unwrap_or(Value::Null)),
            Some(false) => {
                let error = result
                    .get("error")
                    .and_then(Value::as_object)
                    .ok_or_else(|| RpcFailure::transport("DSH RPC error 无效"))?;
                let code = error
                    .get("code")
                    .and_then(Value::as_str)
                    .map(str::to_string);
                let message = error
                    .get("message")
                    .and_then(Value::as_str)
                    .unwrap_or("DSH 拒绝了请求")
                    .to_string();
                Err(RpcFailure { code, message })
            }
            _ => Err(RpcFailure::transport("DSH RPC result.ok 无效")),
        }
    }

    async fn describe_settings(&self) -> Result<SettingsDescribe, RpcFailure> {
        let value = self.call("settings/describe", json!({})).await?;
        serde_json::from_value(value)
            .map_err(|_| RpcFailure::transport("DSH settings/describe 响应结构不兼容"))
    }

    async fn namespace(&self) -> Result<SettingsNamespace, RpcFailure> {
        let description = self.describe_settings().await?;
        if !description.writable {
            return Err(RpcFailure::transport("DSH settings 当前不可写"));
        }
        description
            .namespaces
            .into_iter()
            .find(|namespace| namespace.ns == SETTINGS_NAMESPACE)
            .ok_or_else(|| RpcFailure::transport("DSH 未提供兼容的 llm-pi-ai settings namespace"))
    }

    async fn provider_ids(&self) -> Result<Vec<String>, RpcFailure> {
        let namespace = self.namespace().await?;
        let providers = namespace
            .value
            .get("providers")
            .and_then(Value::as_object)
            .cloned()
            .unwrap_or_default();
        let mut ids = providers.keys().cloned().collect::<Vec<_>>();
        ids.sort();
        Ok(ids)
    }

    async fn provider_profile(&self, route: &str) -> Result<Option<Value>, RpcFailure> {
        let namespace = self.namespace().await?;
        Ok(namespace
            .value
            .get("providers")
            .and_then(Value::as_object)
            .and_then(|providers| providers.get(route))
            .cloned())
    }

    async fn mutate_provider(
        &self,
        route: &str,
        profile: Option<&Value>,
    ) -> Result<(), RpcFailure> {
        let desired = profile.cloned();
        let mut last_conflict = None;
        for _ in 0..MAX_CAS_ATTEMPTS {
            let namespace = self.namespace().await?;
            let current = namespace
                .value
                .get("providers")
                .and_then(Value::as_object)
                .and_then(|providers| providers.get(route))
                .cloned();
            if current == desired {
                return Ok(());
            }
            let operation = match &desired {
                Some(value) => json!({
                    "op": "set",
                    "path": ["providers", route],
                    "value": value,
                }),
                None => json!({
                    "op": "unset",
                    "path": ["providers", route],
                }),
            };
            match self
                .call(
                    "settings/mutate",
                    json!({
                        "ns": SETTINGS_NAMESPACE,
                        "ops": [operation],
                        "expectedRevision": namespace.revision,
                    }),
                )
                .await
            {
                Ok(_) => return Ok(()),
                Err(error) if error.is_conflict() => last_conflict = Some(error),
                Err(error) => return Err(error),
            }
        }
        Err(last_conflict
            .unwrap_or_else(|| RpcFailure::transport("DSH settings 在多次并发冲突后仍无法更新")))
    }

    async fn set_credential(&self, name: &str, value: &str) -> Result<(), RpcFailure> {
        self.call("credentials/set", json!({ "ref": name, "value": value }))
            .await
            .map(|_| ())
    }

    async fn unset_credential(&self, name: &str) -> Result<(), RpcFailure> {
        self.call("credentials/unset", json!({ "ref": name }))
            .await
            .map(|_| ())
    }

    async fn credential_is_referenced(&self, name: &str) -> Result<bool, RpcFailure> {
        let namespace = self.namespace().await?;
        Ok(namespace
            .value
            .get("providers")
            .and_then(Value::as_object)
            .into_iter()
            .flat_map(|providers| providers.values())
            .any(|profile| profile.get("apiKeyEnv").and_then(Value::as_str) == Some(name)))
    }
}

pub struct DshProviderService;

impl DshProviderService {
    pub fn validate_provider(provider: &Provider) -> Result<(), AppError> {
        prepare_provider(provider.clone(), None).map(|_| ())
    }

    pub fn membership() -> ProviderLiveMembership {
        match run_rpc(async {
            DshRpcClient::loopback()?
                .provider_ids()
                .await
                .map_err(RpcFailure::into_app_error)
        }) {
            Ok(ids) => ProviderLiveMembership::available(ids),
            Err(error) => ProviderLiveMembership::unavailable(error.to_string()),
        }
    }

    pub fn exists(route: &str) -> Result<bool, AppError> {
        validate_route(route)?;
        run_rpc(async move {
            DshRpcClient::loopback()?
                .provider_profile(route)
                .await
                .map(|profile| profile.is_some())
                .map_err(RpcFailure::into_app_error)
        })
    }

    pub fn add(state: &AppState, provider: Provider, add_to_live: bool) -> Result<bool, AppError> {
        let previous_secret = optional_secret(state, SecretScope::DshProvider, &provider.id)?;
        let mut prepared = prepare_provider(provider, None)?;
        prepared
            .provider
            .meta
            .get_or_insert_with(Default::default)
            .live_config_managed = Some(add_to_live);
        persist_submitted_secret(state, &prepared)?;

        let commit = (|| {
            if add_to_live {
                enable_prepared(state, &prepared)?;
            }
            state.db.save_provider(
                crate::app_config::AppType::DeepSeekHarness.as_str(),
                &prepared.provider,
            )
        })();
        if let Err(error) = commit {
            if add_to_live {
                let _ = disable_prepared(&prepared);
            }
            let _ = secure_store::restore(
                state,
                SecretScope::DshProvider,
                &prepared.provider.id,
                previous_secret.as_deref(),
            );
            return Err(error);
        }
        Ok(true)
    }

    pub fn update(
        state: &AppState,
        original_id: Option<&str>,
        provider: Provider,
    ) -> Result<bool, AppError> {
        let original_id = original_id.unwrap_or(&provider.id).to_string();
        if original_id != provider.id {
            return Err(AppError::Message(
                "DSH Provider 添加后不能修改 Route".to_string(),
            ));
        }
        let existing = state
            .db
            .get_provider_by_id(
                &original_id,
                crate::app_config::AppType::DeepSeekHarness.as_str(),
            )?
            .ok_or_else(|| AppError::Message(format!("DSH Provider '{original_id}' 不存在")))?;
        let previous_secret = optional_secret(state, SecretScope::DshProvider, &original_id)?;
        let mut prepared = prepare_provider(provider, Some(&existing))?;
        let was_live = if existing
            .meta
            .as_ref()
            .and_then(|meta| meta.live_config_managed)
            == Some(false)
        {
            false
        } else {
            Self::exists(&original_id)?
        };
        prepared
            .provider
            .meta
            .get_or_insert_with(Default::default)
            .live_config_managed = Some(was_live);
        let previous_profile = if was_live {
            let profile_route = original_id.clone();
            run_rpc(async move {
                DshRpcClient::loopback()?
                    .provider_profile(&profile_route)
                    .await
                    .map_err(RpcFailure::into_app_error)
            })?
        } else {
            None
        };

        persist_submitted_secret(state, &prepared)?;
        let commit = (|| {
            if was_live {
                enable_prepared(state, &prepared)?;
            }
            state.db.save_provider(
                crate::app_config::AppType::DeepSeekHarness.as_str(),
                &prepared.provider,
            )
        })();
        if let Err(error) = commit {
            let _ = secure_store::restore(
                state,
                SecretScope::DshProvider,
                &original_id,
                previous_secret.as_deref(),
            );
            if was_live {
                let _ = restore_live_profile(
                    &prepared,
                    previous_profile.as_ref(),
                    previous_secret.as_deref(),
                );
            }
            return Err(error);
        }
        Ok(true)
    }

    pub fn set_enabled(state: &AppState, route: &str, enabled: bool) -> Result<(), AppError> {
        let provider = state
            .db
            .get_provider_by_id(route, crate::app_config::AppType::DeepSeekHarness.as_str())?
            .ok_or_else(|| AppError::Message(format!("DSH Provider '{route}' 不存在")))?;
        let mut prepared = prepare_provider(provider, None)?;
        if enabled {
            enable_prepared(state, &prepared)?;
        } else {
            disable_prepared(&prepared)?;
        }
        prepared
            .provider
            .meta
            .get_or_insert_with(Default::default)
            .live_config_managed = Some(enabled);
        state.db.save_provider(
            crate::app_config::AppType::DeepSeekHarness.as_str(),
            &prepared.provider,
        )
    }

    pub fn delete(state: &AppState, route: &str) -> Result<(), AppError> {
        let existing = state
            .db
            .get_provider_by_id(route, crate::app_config::AppType::DeepSeekHarness.as_str())?;
        let Some(existing) = existing else {
            return Ok(());
        };
        let prepared = prepare_provider(existing.clone(), Some(&existing))?;
        let was_live = if existing
            .meta
            .as_ref()
            .and_then(|meta| meta.live_config_managed)
            == Some(false)
        {
            false
        } else {
            Self::exists(route)?
        };
        let previous_secret = optional_secret(state, SecretScope::DshProvider, route)?;
        if was_live {
            disable_prepared(&prepared)?;
        }
        if let Err(error) = state
            .db
            .delete_provider(crate::app_config::AppType::DeepSeekHarness.as_str(), route)
        {
            if was_live {
                let _ = enable_prepared(state, &prepared);
            }
            return Err(error);
        }
        if prepared
            .credential_ref
            .as_ref()
            .is_some_and(|reference| reference.source == "dshProvider")
        {
            if let Err(error) = secure_store::remove(state, SecretScope::DshProvider, route) {
                let _ = state.db.save_provider(
                    crate::app_config::AppType::DeepSeekHarness.as_str(),
                    &existing,
                );
                let _ = secure_store::restore(
                    state,
                    SecretScope::DshProvider,
                    route,
                    previous_secret.as_deref(),
                );
                if was_live {
                    let _ = enable_prepared(state, &prepared);
                }
                return Err(error);
            }
        }
        Ok(())
    }
}

fn run_rpc<T>(future: impl Future<Output = Result<T, AppError>>) -> Result<T, AppError> {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(|error| AppError::Message(format!("无法启动 DSH RPC runtime: {error}")))?
        .block_on(future)
}

fn prepare_provider(
    mut provider: Provider,
    existing: Option<&Provider>,
) -> Result<PreparedProvider, AppError> {
    validate_route(&provider.id)?;
    let settings = provider
        .settings_config
        .as_object()
        .ok_or_else(|| AppError::Message("DSH Provider 配置必须是 JSON 对象".to_string()))?;
    let schema_version = settings
        .get("schemaVersion")
        .and_then(Value::as_u64)
        .unwrap_or(1);
    if schema_version != 1 {
        return Err(AppError::Message(format!(
            "不支持 DSH Provider schemaVersion {schema_version}"
        )));
    }
    if let Some(route) = settings.get("route").and_then(Value::as_str) {
        if route != provider.id {
            return Err(AppError::Message(
                "DSH Provider Route 与 Provider ID 不一致".to_string(),
            ));
        }
    }

    let submitted_secret = settings
        .get("apiKey")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|secret| !secret.is_empty())
        .map(str::to_string);
    let existing_settings = existing.and_then(|item| item.settings_config.as_object());
    let mut credential_ref = settings
        .get("credentialRef")
        .cloned()
        .or_else(|| existing_settings.and_then(|value| value.get("credentialRef").cloned()))
        .map(serde_json::from_value)
        .transpose()
        .map_err(|_| AppError::Message("DSH credentialRef 格式无效".to_string()))?;
    if credential_ref.is_none() && submitted_secret.is_some() {
        credential_ref = Some(CredentialReference {
            source: "dshProvider".to_string(),
            id: provider.id.clone(),
        });
    }
    if let Some(reference) = &credential_ref {
        match reference.source.as_str() {
            "dshProvider" if reference.id == provider.id => {}
            "providerCenter" => {}
            _ => {
                return Err(AppError::Message(
                    "DSH credentialRef 来源或标识无效".to_string(),
                ))
            }
        }
    }

    let raw_api = settings
        .get("api")
        .and_then(Value::as_str)
        .unwrap_or("openai-completions");
    let api = match raw_api {
        "ollama" => "openai-completions",
        "openai-completions" | "openai-responses" | "anthropic-messages" => raw_api,
        _ => {
            return Err(AppError::Message(format!(
                "DSH 不支持 API 协议 '{raw_api}'"
            )))
        }
    };
    let base_url = settings
        .get("baseURL")
        .and_then(Value::as_str)
        .map(str::trim)
        .unwrap_or_default();
    if base_url.is_empty() {
        return Err(AppError::Message("DSH Provider 缺少 baseURL".to_string()));
    }
    let parsed_base_url = reqwest::Url::parse(base_url)
        .map_err(|_| AppError::Message("DSH Provider baseURL 无效".to_string()))?;
    if !matches!(parsed_base_url.scheme(), "http" | "https") {
        return Err(AppError::Message(
            "DSH Provider baseURL 仅支持 HTTP 或 HTTPS".to_string(),
        ));
    }
    let models = settings
        .get("models")
        .cloned()
        .unwrap_or_else(|| Value::Array(Vec::new()));
    if !models.is_array() {
        return Err(AppError::Message(
            "DSH Provider models 必须是数组".to_string(),
        ));
    }

    let credential_name = settings
        .get("apiKeyEnv")
        .and_then(Value::as_str)
        .map(str::to_string)
        .or_else(|| {
            credential_ref
                .as_ref()
                .map(|_| credential_name_for_route(&provider.id))
        });
    let mut profile = Map::new();
    profile.insert(
        "displayName".to_string(),
        Value::String(
            settings
                .get("displayName")
                .and_then(Value::as_str)
                .unwrap_or(&provider.name)
                .to_string(),
        ),
    );
    profile.insert("api".to_string(), Value::String(api.to_string()));
    profile.insert("baseURL".to_string(), Value::String(base_url.to_string()));
    profile.insert("models".to_string(), models.clone());
    if let Some(name) = &credential_name {
        profile.insert("apiKeyEnv".to_string(), Value::String(name.clone()));
    }

    let mut stored = profile.clone();
    stored.insert("schemaVersion".to_string(), json!(1));
    stored.insert("route".to_string(), Value::String(provider.id.clone()));
    if let Some(reference) = &credential_ref {
        stored.insert(
            "credentialRef".to_string(),
            serde_json::to_value(reference)
                .map_err(|error| AppError::Message(error.to_string()))?,
        );
    }
    provider.settings_config = Value::Object(stored);

    Ok(PreparedProvider {
        provider,
        profile: Value::Object(profile),
        credential_ref,
        credential_name,
        submitted_secret,
    })
}

fn validate_route(route: &str) -> Result<(), AppError> {
    let valid = !route.is_empty()
        && route.len() <= 128
        && route.split('-').all(|segment| {
            !segment.is_empty()
                && segment
                    .chars()
                    .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit())
        });
    if !valid {
        return Err(AppError::Message(
            "DSH Route 必须是小写字母、数字和单个连字符组成的稳定标识".to_string(),
        ));
    }
    Ok(())
}

fn credential_name_for_route(route: &str) -> String {
    let stable = route
        .chars()
        .map(|character| {
            if character.is_ascii_alphanumeric() {
                character.to_ascii_uppercase()
            } else {
                '_'
            }
        })
        .collect::<String>();
    format!("CC_SWITCH_DSH_{stable}_API_KEY")
}

fn optional_secret(
    state: &AppState,
    scope: SecretScope,
    id: &str,
) -> Result<Option<String>, AppError> {
    let (configured, _) = secure_store::metadata(state, scope, id)?;
    configured
        .then(|| secure_store::get(state, scope, id))
        .transpose()
}

fn persist_submitted_secret(state: &AppState, prepared: &PreparedProvider) -> Result<(), AppError> {
    if let (Some(reference), Some(secret)) = (&prepared.credential_ref, &prepared.submitted_secret)
    {
        if reference.source != "dshProvider" || reference.id != prepared.provider.id {
            return Err(AppError::Message(
                "受管 DSH Provider 不接受表单覆盖凭据".to_string(),
            ));
        }
        secure_store::save(
            state,
            SecretScope::DshProvider,
            &prepared.provider.id,
            secret,
        )?;
    }
    Ok(())
}

fn resolve_secret(
    state: &AppState,
    prepared: &PreparedProvider,
) -> Result<Option<String>, AppError> {
    let Some(reference) = &prepared.credential_ref else {
        return Ok(None);
    };
    match reference.source.as_str() {
        "dshProvider" if reference.id == prepared.provider.id => {
            secure_store::get(state, SecretScope::DshProvider, &reference.id).map(Some)
        }
        "providerCenter" => crate::provider_center::projection_secret_for_provider(
            state,
            &crate::app_config::AppType::DeepSeekHarness,
            &prepared.provider,
            &reference.id,
        )
        .map(Some),
        _ => Err(AppError::Message(
            "DSH credentialRef 未通过所有权校验".to_string(),
        )),
    }
}

fn enable_prepared(state: &AppState, prepared: &PreparedProvider) -> Result<(), AppError> {
    let secret = resolve_secret(state, prepared)?;
    let client = DshRpcClient::loopback()?;
    let route = prepared.provider.id.clone();
    let profile = prepared.profile.clone();
    let credential_name = prepared.credential_name.clone();
    run_rpc(async move {
        if let (Some(name), Some(secret)) = (&credential_name, &secret) {
            client
                .set_credential(name, secret)
                .await
                .map_err(RpcFailure::into_app_error)?;
        } else if credential_name.is_some() {
            return Err(AppError::Message(
                "DSH Provider 缺少可用凭据；请重新输入 API Key".to_string(),
            ));
        }
        if let Err(error) = client.mutate_provider(&route, Some(&profile)).await {
            if let Some(name) = &credential_name {
                if !client.credential_is_referenced(name).await.unwrap_or(true) {
                    let _ = client.unset_credential(name).await;
                }
            }
            return Err(error.into_app_error());
        }
        Ok(())
    })
}

fn disable_prepared(prepared: &PreparedProvider) -> Result<(), AppError> {
    let client = DshRpcClient::loopback()?;
    let route = prepared.provider.id.clone();
    let credential_name = prepared.credential_name.clone();
    run_rpc(async move {
        client
            .mutate_provider(&route, None)
            .await
            .map_err(RpcFailure::into_app_error)?;
        if let Some(name) = credential_name {
            if !client
                .credential_is_referenced(&name)
                .await
                .map_err(RpcFailure::into_app_error)?
            {
                client
                    .unset_credential(&name)
                    .await
                    .map_err(RpcFailure::into_app_error)?;
            }
        }
        Ok(())
    })
}

fn restore_live_profile(
    prepared: &PreparedProvider,
    previous_profile: Option<&Value>,
    previous_secret: Option<&str>,
) -> Result<(), AppError> {
    let client = DshRpcClient::loopback()?;
    let route = prepared.provider.id.clone();
    let credential_name = prepared.credential_name.clone();
    let previous_profile = previous_profile.cloned();
    let previous_secret = previous_secret.map(str::to_string);
    run_rpc(async move {
        if let (Some(name), Some(secret)) = (&credential_name, previous_secret.as_deref()) {
            client
                .set_credential(name, secret)
                .await
                .map_err(RpcFailure::into_app_error)?;
        }
        client
            .mutate_provider(&route, previous_profile.as_ref())
            .await
            .map_err(RpcFailure::into_app_error)
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn validates_stable_routes() {
        assert!(validate_route("deepseek-v3").is_ok());
        assert!(validate_route("provider-center-dsh-1234").is_ok());
        assert!(validate_route("DeepSeek").is_err());
        assert!(validate_route("deepseek--v3").is_err());
        assert!(validate_route("../deepseek").is_err());
    }

    #[test]
    fn prepares_redacted_native_profile() {
        let provider = Provider::with_id(
            "local-ollama".to_string(),
            "Local Ollama".to_string(),
            json!({
                "schemaVersion": 1,
                "route": "local-ollama",
                "displayName": "Local Ollama",
                "api": "ollama",
                "baseURL": "http://127.0.0.1:11434/v1/",
                "apiKey": "must-not-remain",
                "models": [{ "id": "qwen3", "name": "qwen3" }],
            }),
            None,
        );
        let prepared = prepare_provider(provider, None).expect("prepare provider");
        assert_eq!(prepared.profile["api"], "openai-completions");
        assert_eq!(prepared.profile["baseURL"], "http://127.0.0.1:11434/v1/");
        assert!(prepared.provider.settings_config.get("apiKey").is_none());
        assert!(!serde_json::to_string(&prepared.provider)
            .expect("serialize provider")
            .contains("must-not-remain"));
    }

    #[test]
    fn rejects_non_loopback_rpc_base() {
        assert!(DshRpcClient::new("https://example.com").is_err());
        assert!(DshRpcClient::new("http://localhost:3080").is_err());
        assert!(DshRpcClient::new("http://127.0.0.1:3080").is_ok());
    }
}
