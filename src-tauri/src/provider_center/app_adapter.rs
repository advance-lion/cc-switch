//! Provider Center application adapters.
//!
//! This is intentionally a thin compatibility layer over the existing
//! `ProviderService` and provider configuration formats. It centralizes the
//! app-specific decisions used by Provider Center without introducing another
//! persistence or live-config implementation.

use crate::app_config::AppType;
use crate::error::AppError;
use crate::provider::{ClaudeDesktopMode, ClaudeDesktopModelRoute, Provider, ProviderMeta};
use crate::proxy::providers::capabilities::{self, Compatibility};
use crate::services::ProviderService;
use crate::store::AppState;
use serde_json::json;
use sha2::{Digest, Sha256};
use std::str::FromStr;
use std::sync::OnceLock;

use super::ProviderDefinition;
use super::ProviderModelDefinition;

pub(crate) struct AppAdapterRegistry {
    adapters: Vec<AppAdapter>,
}

pub(crate) struct AppAdapter {
    app_type: AppType,
    default_protocol: &'static str,
    supported_protocols: &'static [&'static str],
    render: fn(&ProviderDefinition, &str) -> Result<Provider, AppError>,
}

impl AppAdapterRegistry {
    pub(crate) fn global() -> &'static Self {
        static REGISTRY: OnceLock<AppAdapterRegistry> = OnceLock::new();
        REGISTRY.get_or_init(Self::new)
    }

    fn new() -> Self {
        let adapters = [
            AppAdapter::new(AppType::Claude, "anthropic", &["anthropic"], render_claude),
            AppAdapter::new(
                AppType::ClaudeDesktop,
                "anthropic",
                &["anthropic"],
                render_claude_desktop,
            ),
            AppAdapter::new(
                AppType::Codex,
                "openai-chat",
                &["openai-responses"],
                render_codex,
            ),
            AppAdapter::new(AppType::Gemini, "gemini", &["gemini"], render_gemini),
            AppAdapter::new(
                AppType::GrokBuild,
                "openai-chat",
                &["openai-chat"],
                render_grokbuild,
            ),
            AppAdapter::new(
                AppType::OpenCode,
                "openai-chat",
                &["openai-chat", "ollama"],
                render_opencode,
            ),
            AppAdapter::new(
                AppType::OpenClaw,
                "openai-chat",
                &[
                    "openai-chat",
                    "openai-responses",
                    "anthropic",
                    "gemini",
                    "ollama",
                ],
                render_openclaw,
            ),
            AppAdapter::new(
                AppType::Hermes,
                "openai-chat",
                &["openai-chat", "ollama"],
                render_hermes,
            ),
            AppAdapter::new(
                AppType::Pi,
                "openai-chat",
                &[
                    "openai-chat",
                    "openai-responses",
                    "anthropic",
                    "gemini",
                    "ollama",
                ],
                render_pi,
            ),
            AppAdapter::new(
                AppType::DeepSeekHarness,
                "openai-chat",
                &["openai-chat", "openai-responses", "anthropic", "ollama"],
                render_dsh,
            ),
        ];
        Self {
            adapters: adapters.into_iter().collect(),
        }
    }

    pub(crate) fn resolve(&self, app: &str) -> Result<&AppAdapter, AppError> {
        let normalized = AppType::from_str(app)
            .map(|app| app.as_str().to_string())
            .unwrap_or_else(|_| app.trim().to_lowercase());
        self.adapters
            .iter()
            .find(|adapter| adapter.app_id() == normalized)
            .ok_or_else(|| AppError::AppNotSupported {
                app: normalized.clone(),
            })
    }

    #[cfg(test)]
    pub(crate) fn adapters(&self) -> impl Iterator<Item = &AppAdapter> {
        self.adapters.iter()
    }
}

impl AppAdapter {
    fn new(
        app_type: AppType,
        default_protocol: &'static str,
        supported_protocols: &'static [&'static str],
        render: fn(&ProviderDefinition, &str) -> Result<Provider, AppError>,
    ) -> Self {
        Self {
            app_type,
            default_protocol,
            supported_protocols,
            render,
        }
    }

    pub(crate) fn app_type(&self) -> AppType {
        self.app_type.clone()
    }

    pub(crate) fn app_id(&self) -> &str {
        self.app_type.as_str()
    }

    pub(crate) fn default_protocol(&self) -> &'static str {
        self.default_protocol
    }

    pub(crate) fn validate_protocol(&self, protocol: &str) -> Result<(), AppError> {
        let compat = self.resolve_compatibility(protocol);
        match compat {
            Compatibility::Direct | Compatibility::Proxy { .. } => Ok(()),
            Compatibility::Unsupported { reason } => Err(AppError::Message(reason)),
        }
    }

    /// Resolve how this Agent can reach a provider with the given upstream
    /// protocol, using the unified capability registry that also powers the
    /// local proxy.
    pub(crate) fn resolve_compatibility(&self, protocol: &str) -> Compatibility {
        capabilities::resolve_compatibility(protocol, self.app_id())
    }

    pub(crate) fn infer_protocol(&self, provider: &Provider) -> String {
        let api = provider
            .settings_config
            .get("api")
            .and_then(serde_json::Value::as_str)
            .or_else(|| {
                provider
                    .settings_config
                    .get("models")
                    .and_then(serde_json::Value::as_array)
                    .and_then(|models| models.first())
                    .and_then(|model| model.get("api"))
                    .and_then(serde_json::Value::as_str)
            });
        if let Some(api) = api {
            return match api {
                "openai-responses" => "openai-responses",
                "anthropic" | "anthropic-messages" => "anthropic",
                "gemini" | "google-generative-ai" => "gemini",
                _ => "openai-chat",
            }
            .to_string();
        }
        if let Some(ref meta) = provider.meta {
            if let Some(ref api_format) = meta.api_format {
                return match api_format.as_str() {
                    "anthropic" => "anthropic",
                    "openai_chat" => "openai-chat",
                    "openai_responses" => "openai-responses",
                    "gemini_native" => "gemini",
                    _ => self.default_protocol,
                }
                .to_string();
            }
        }
        if self.app_type == AppType::Codex {
            if let Some(config) = provider
                .settings_config
                .get("config")
                .and_then(serde_json::Value::as_str)
                .and_then(|config| toml::from_str::<toml::Value>(config).ok())
            {
                let provider_name = config.get("model_provider").and_then(toml::Value::as_str);
                let wire_api = provider_name
                    .and_then(|name| config.get("model_providers")?.get(name))
                    .and_then(|value| value.get("wire_api"))
                    .and_then(toml::Value::as_str);
                return if wire_api == Some("chat") {
                    "openai-chat"
                } else {
                    "openai-responses"
                }
                .to_string();
            }
        }
        if self.app_type == AppType::OpenCode {
            let npm = provider
                .settings_config
                .get("npm")
                .and_then(serde_json::Value::as_str)
                .unwrap_or_default();
            if npm.contains("anthropic") {
                return "anthropic".to_string();
            }
            if npm.contains("google") || npm.contains("gemini") {
                return "gemini".to_string();
            }
            if npm.contains("ollama") {
                return "ollama".to_string();
            }
        }
        self.default_protocol.to_string()
    }

    pub(crate) fn render_projection(
        &self,
        definition: &ProviderDefinition,
        secret: &str,
    ) -> Result<Provider, AppError> {
        let compatibility = self.resolve_compatibility(&definition.protocol);
        if let Compatibility::Unsupported { reason } = &compatibility {
            return Err(AppError::Message(reason.clone()));
        }
        let mut provider = (self.render)(definition, secret)?;
        provider.id = projected_provider_id(definition, self.app_id());
        provider.category = Some("provider-center".to_string());
        // Set meta.apiFormat so the frontend (providerNeedsRouting) and the
        // runtime transform predicates (codex_provider_uses_chat_completions
        // etc.) can detect when proxy routing is required.  Without this, a
        // projected Codex provider with an openai-chat definition would have
        // wire_api="responses" in the TOML and no apiFormat, causing the
        // frontend to skip the "需要路由" badge and the proxy to never
        // transform Responses→Chat requests.
        let meta = provider.meta.get_or_insert_with(ProviderMeta::default);
        meta.api_format = protocol_to_api_format(&definition.protocol);
        if self.app_type == AppType::ClaudeDesktop {
            meta.claude_desktop_mode = Some(match compatibility {
                Compatibility::Direct => ClaudeDesktopMode::Direct,
                Compatibility::Proxy { .. } => ClaudeDesktopMode::Proxy,
                Compatibility::Unsupported { .. } => unreachable!(),
            });
            if matches!(meta.claude_desktop_mode, Some(ClaudeDesktopMode::Proxy)) {
                let mut definitions = model_defs(definition);
                if definitions.is_empty() {
                    definitions.push(ProviderModelDefinition {
                        id: "default".to_string(),
                        ..Default::default()
                    });
                }
                for (index, route) in crate::claude_desktop_config::DEFAULT_PROXY_ROUTES
                    .iter()
                    .enumerate()
                {
                    let model = &definitions[index.min(definitions.len() - 1)];
                    meta.claude_desktop_model_routes.insert(
                        route.route_id.to_string(),
                        ClaudeDesktopModelRoute {
                            model: model.id.clone(),
                            label_override: model
                                .display_name
                                .clone()
                                .or_else(|| Some(model.id.clone())),
                            supports_1m: Some(
                                model
                                    .context_window
                                    .is_some_and(|tokens| tokens >= 1_000_000)
                                    || route.supports_1m,
                            ),
                        },
                    );
                }
            }
        }
        if self.app_type == AppType::DeepSeekHarness {
            meta.live_config_managed = Some(false);
        }
        Ok(provider)
    }

    pub(crate) fn fingerprint(&self, provider: &Provider) -> Result<String, AppError> {
        provider_fingerprint(provider)
    }

    pub(crate) fn list(
        &self,
        state: &AppState,
    ) -> Result<indexmap::IndexMap<String, Provider>, AppError> {
        ProviderService::list(state, self.app_type())
    }
    /// Lists providers for an import scan without allowing app-specific list
    /// routines to persist native state as a side effect.
    pub(crate) fn list_for_scan(
        &self,
        state: &AppState,
    ) -> Result<indexmap::IndexMap<String, Provider>, AppError> {
        ProviderService::list_for_provider_center_scan(state, self.app_type())
    }

    pub(crate) fn current(&self, state: &AppState) -> Result<String, AppError> {
        ProviderService::current(state, self.app_type())
    }

    pub(crate) fn read_live_settings(&self) -> Result<serde_json::Value, AppError> {
        ProviderService::read_live_settings(self.app_type())
    }

    pub(crate) fn apply(&self, state: &AppState, mut provider: Provider) -> Result<(), AppError> {
        let id = provider.id.clone();
        if self.app_type == AppType::DeepSeekHarness {
            if let Some(existing) = state.db.get_provider_by_id(&id, self.app_id())? {
                provider
                    .meta
                    .get_or_insert_with(Default::default)
                    .live_config_managed = existing
                    .meta
                    .and_then(|meta| meta.live_config_managed)
                    .or(Some(false));
            }
            state.db.save_provider(self.app_id(), &provider)?;
        } else if state.db.get_provider_by_id(&id, self.app_id())?.is_some() {
            ProviderService::update(state, self.app_type(), Some(&id), provider)?;
        } else {
            ProviderService::add(state, self.app_type(), provider, false)?;
        }
        Ok(())
    }

    pub(crate) fn delete(&self, state: &AppState, provider_id: &str) -> Result<(), AppError> {
        if self.app_type == AppType::DeepSeekHarness {
            if state
                .db
                .get_provider_by_id(provider_id, self.app_id())?
                .and_then(|provider| provider.meta)
                .and_then(|meta| meta.live_config_managed)
                == Some(true)
            {
                return Err(AppError::Message(
                    "请先从 DeepSeek Harness 移除此 Provider，再删除受管投影".to_string(),
                ));
            }
            state.db.delete_provider(self.app_id(), provider_id)
        } else {
            ProviderService::delete(state, self.app_type(), provider_id)
        }
    }
}

/// Map a Provider Center protocol string to the canonical `meta.apiFormat`
/// value used by the frontend and runtime transform predicates.
///
/// Returns `None` for protocols that don't have a meaningful apiFormat
/// (e.g. `ollama`), leaving `meta` unset as before.
fn protocol_to_api_format(protocol: &str) -> Option<String> {
    match protocol.trim().to_lowercase().as_str() {
        "openai-chat" | "openai_chat" | "openai-chat-completions" => {
            Some("openai_chat".to_string())
        }
        "openai-responses" | "openai_responses" | "responses" => {
            Some("openai_responses".to_string())
        }
        "anthropic" | "anthropic-messages" | "anthropic_messages" => Some("anthropic".to_string()),
        "gemini" | "gemini-generate-content" | "gemini_native" | "gemini-native" => {
            Some("gemini_native".to_string())
        }
        _ => None,
    }
}

fn models(definition: &ProviderDefinition) -> Vec<String> {
    if definition.models.is_empty() {
        vec!["default".to_string()]
    } else {
        definition.models.clone()
    }
}

/// Return normalized model definitions, falling back to bare IDs when the
/// definition predates the `model_definitions` field.
fn model_defs(definition: &ProviderDefinition) -> Vec<ProviderModelDefinition> {
    if definition.model_definitions.is_empty() {
        definition
            .models
            .iter()
            .map(|id| ProviderModelDefinition {
                id: id.clone(),
                ..Default::default()
            })
            .collect()
    } else {
        definition.model_definitions.clone()
    }
}

fn first_model_id(definition: &ProviderDefinition, fallback: &str) -> String {
    models(definition)
        .first()
        .cloned()
        .unwrap_or_else(|| fallback.to_string())
}

fn render_claude(definition: &ProviderDefinition, secret: &str) -> Result<Provider, AppError> {
    let model = first_model_id(definition, "claude-sonnet-4-20250514");
    Ok(Provider::with_id(
        String::new(),
        definition.name.clone(),
        json!({
            "env": {
                "ANTHROPIC_BASE_URL": definition.base_url,
                "ANTHROPIC_AUTH_TOKEN": secret,
                "ANTHROPIC_MODEL": model,
                "ANTHROPIC_DEFAULT_HAIKU_MODEL": model,
                "ANTHROPIC_DEFAULT_SONNET_MODEL": model,
                "ANTHROPIC_DEFAULT_OPUS_MODEL": model,
            }
        }),
        None,
    ))
}

fn render_codex(definition: &ProviderDefinition, secret: &str) -> Result<Provider, AppError> {
    let model = first_model_id(definition, "gpt-4o");
    let reasoning_effort = "medium";
    let base_trimmed = definition.base_url.trim_end_matches('/');
    let origin_only = match base_trimmed.split_once("://") {
        Some((_scheme, rest)) => !rest.contains('/'),
        None => !base_trimmed.contains('/'),
    };
    let codex_base_url = if base_trimmed.ends_with("/v1") {
        base_trimmed.to_string()
    } else if origin_only {
        format!("{base_trimmed}/v1")
    } else {
        base_trimmed.to_string()
    };
    let config_toml = format!(
        r#"model_provider = "custom"
model = "{model}"
model_reasoning_effort = "{reasoning_effort}"
disable_response_storage = true

[model_providers.custom]
name = "NewAPI"
base_url = "{codex_base_url}"
wire_api = "responses"
requires_openai_auth = true"#
    );
    let mut settings = json!({
        "auth": {
            "OPENAI_API_KEY": secret
        },
        "config": config_toml
    });
    let defs = model_defs(definition);
    if !defs.is_empty() {
        let catalog_models: Vec<serde_json::Value> = defs
            .iter()
            .map(|d| {
                let mut entry = json!({
                    "model": d.id,
                });
                if let Some(name) = &d.display_name {
                    entry["displayName"] = json!(name);
                } else {
                    entry["displayName"] = json!(d.id);
                }
                if let Some(cw) = d.context_window {
                    entry["contextWindow"] = json!(cw);
                }
                if !d.input_modalities.is_empty() {
                    entry["inputModalities"] = json!(d.input_modalities);
                }
                entry
            })
            .collect();
        settings["modelCatalog"] = json!({ "models": catalog_models });
    }
    Ok(Provider::with_id(
        String::new(),
        definition.name.clone(),
        settings,
        None,
    ))
}

fn render_gemini(definition: &ProviderDefinition, secret: &str) -> Result<Provider, AppError> {
    let model = first_model_id(definition, "gemini-2.5-pro");
    Ok(Provider::with_id(
        String::new(),
        definition.name.clone(),
        json!({
            "env": {
                "GOOGLE_GEMINI_BASE_URL": definition.base_url,
                "GEMINI_API_KEY": secret,
                "GEMINI_MODEL": model,
            }
        }),
        None,
    ))
}

fn render_claude_desktop(
    definition: &ProviderDefinition,
    secret: &str,
) -> Result<Provider, AppError> {
    Ok(Provider::with_id(
        String::new(),
        definition.name.clone(),
        json!({ "env": { "ANTHROPIC_BASE_URL": definition.base_url, "ANTHROPIC_AUTH_TOKEN": secret } }),
        None,
    ))
}

fn toml_string(value: &str) -> String {
    value.replace('\\', "\\\\").replace('"', "\\\"")
}

fn render_grokbuild(definition: &ProviderDefinition, secret: &str) -> Result<Provider, AppError> {
    let model = first_model_id(definition, "gpt-4o");
    let model_key = "shared";
    // Use context_window from normalized model metadata when available,
    // falling back to a conservative default.
    let context_window = model_defs(definition)
        .first()
        .and_then(|d| d.context_window)
        .unwrap_or(128_000);
    let config = format!(
        "[models]\ndefault = \"{model_key}\"\n\n[model.{model_key}]\nmodel = \"{}\"\nbase_url = \"{}\"\napi_key = \"{}\"\nname = \"{}\"\napi_backend = \"openai\"\ncontext_window = {}\n",
        toml_string(&model),
        toml_string(&definition.base_url),
        toml_string(secret),
        toml_string(&definition.name),
        context_window
    );
    Ok(Provider::with_id(
        String::new(),
        definition.name.clone(),
        json!({ "config": config }),
        None,
    ))
}

fn openai_compatible_base_url(definition: &ProviderDefinition) -> String {
    let base = definition.base_url.trim_end_matches('/');
    if definition.protocol == "ollama" && !base.ends_with("/v1") {
        format!("{base}/v1")
    } else {
        base.to_string()
    }
}

fn render_opencode(definition: &ProviderDefinition, secret: &str) -> Result<Provider, AppError> {
    let defs = model_defs(definition);
    let model_map: serde_json::Map<String, serde_json::Value> = defs
        .iter()
        .map(|d| {
            let mut entry = json!({ "name": d.display_name.as_deref().unwrap_or(&d.id) });
            if let Some(cw) = d.context_window {
                entry["limit"] = json!({ "context": cw });
            }
            if let Some(mot) = d.max_output_tokens {
                if let Some(limit) = entry.get_mut("limit").and_then(|v| v.as_object_mut()) {
                    limit.insert("output".to_string(), json!(mot));
                } else {
                    entry["limit"] = json!({ "output": mot });
                }
            }
            (d.id.clone(), entry)
        })
        .collect();
    Ok(Provider::with_id(
        String::new(),
        definition.name.clone(),
        json!({
            "npm": "@ai-sdk/openai-compatible", "name": definition.name,
            "options": { "baseURL": openai_compatible_base_url(definition), "apiKey": secret },
            "models": model_map
        }),
        None,
    ))
}

fn native_api_name(protocol: &str) -> Option<&'static str> {
    match protocol {
        "openai-chat" | "ollama" => Some("openai-completions"),
        "openai-responses" => Some("openai-responses"),
        "anthropic" => Some("anthropic-messages"),
        "gemini" => Some("google-generative-ai"),
        _ => None,
    }
}

fn render_openclaw(definition: &ProviderDefinition, secret: &str) -> Result<Provider, AppError> {
    let api = native_api_name(&definition.protocol).ok_or_else(render_error)?;
    let defs = model_defs(definition);
    let models: Vec<serde_json::Value> = defs
        .iter()
        .map(|d| {
            let mut entry = json!({
                "id": d.id,
                "name": d.display_name.as_deref().unwrap_or(&d.id),
            });
            if let Some(r) = d.reasoning {
                entry["reasoning"] = json!(r);
            }
            if !d.input_modalities.is_empty() {
                entry["input"] = json!(d.input_modalities);
            }
            if let Some(cw) = d.context_window {
                entry["contextWindow"] = json!(cw);
            }
            if let Some(mot) = d.max_output_tokens {
                entry["maxTokens"] = json!(mot);
            }
            entry
        })
        .collect();
    Ok(Provider::with_id(
        String::new(),
        definition.name.clone(),
        json!({
            "baseUrl": openai_compatible_base_url(definition), "apiKey": secret,
            "api": api,
            "models": models
        }),
        None,
    ))
}

fn render_hermes(definition: &ProviderDefinition, secret: &str) -> Result<Provider, AppError> {
    let defs = model_defs(definition);
    // Hermes' database/UI shape is an ordered array of objects with `id`
    // and optional `context_length`. The YAML writer converts this to a
    // dict at activation time.
    let models: Vec<serde_json::Value> = defs
        .iter()
        .map(|d| {
            let mut entry = json!({ "id": d.id });
            if let Some(cw) = d.context_window {
                entry["context_length"] = json!(cw);
            }
            entry
        })
        .collect();
    Ok(Provider::with_id(
        String::new(),
        definition.name.clone(),
        json!({
            "base_url": openai_compatible_base_url(definition), "api_key": secret,
            "models": models
        }),
        None,
    ))
}

fn render_pi(definition: &ProviderDefinition, secret: &str) -> Result<Provider, AppError> {
    let api = native_api_name(&definition.protocol).ok_or_else(render_error)?;
    let defs = model_defs(definition);
    let base_url = openai_compatible_base_url(definition);
    let models: Vec<serde_json::Value> = defs
        .iter()
        .map(|d| {
            let mut entry = json!({
                "id": d.id,
                "name": d.display_name.as_deref().unwrap_or(&d.id),
                "api": api,
                "baseUrl": base_url,
            });
            if let Some(r) = d.reasoning {
                entry["reasoning"] = json!(r);
            }
            if !d.input_modalities.is_empty() {
                entry["input"] = json!(d.input_modalities);
            }
            if let Some(cw) = d.context_window {
                entry["contextWindow"] = json!(cw);
            }
            if let Some(mot) = d.max_output_tokens {
                entry["maxTokens"] = json!(mot);
            }
            entry
        })
        .collect();
    Ok(Provider::with_id(
        String::new(),
        definition.name.clone(),
        json!({
            "apiKey": secret,
            "models": models
        }),
        None,
    ))
}

fn render_dsh(definition: &ProviderDefinition, _secret: &str) -> Result<Provider, AppError> {
    let api = native_api_name(&definition.protocol).ok_or_else(render_error)?;
    let defs = model_defs(definition);
    let base_url = openai_compatible_base_url(definition);
    let models: Vec<serde_json::Value> = defs
        .iter()
        .map(|model| {
            let mut entry = json!({
                "id": model.id,
                "name": model.display_name.as_deref().unwrap_or(&model.id),
            });
            if let Some(context_window) = model.context_window {
                entry["contextWindow"] = json!(context_window);
            }
            if let Some(max_tokens) = model.max_output_tokens {
                entry["maxTokens"] = json!(max_tokens);
            }
            if !model.input_modalities.is_empty() {
                entry["input"] = json!(model.input_modalities);
            }
            entry
        })
        .collect();
    let route = projected_provider_id(definition, AppType::DeepSeekHarness.as_str());
    let mut settings = serde_json::Map::new();
    settings.insert(
        "displayName".to_string(),
        serde_json::Value::String(definition.name.clone()),
    );
    settings.insert(
        "api".to_string(),
        serde_json::Value::String(api.to_string()),
    );
    settings.insert("baseURL".to_string(), serde_json::Value::String(base_url));
    settings.insert("models".to_string(), serde_json::Value::Array(models));
    if definition.protocol != "ollama" {
        let credential_ref = format!("CC_SWITCH_DSH_{}_API_KEY", stable_identifier(&route));
        settings.insert(
            "apiKeyEnv".to_string(),
            serde_json::Value::String(credential_ref),
        );
    }
    settings.insert("schemaVersion".to_string(), json!(1));
    settings.insert("route".to_string(), serde_json::Value::String(route));
    if definition.protocol != "ollama" {
        settings.insert(
            "credentialRef".to_string(),
            json!({
                "source": "providerCenter",
                "id": definition.id,
            }),
        );
    }

    Ok(Provider::with_id(
        String::new(),
        definition.name.clone(),
        serde_json::Value::Object(settings),
        None,
    ))
}

fn stable_identifier(value: &str) -> String {
    value
        .chars()
        .map(|character| {
            if character.is_ascii_alphanumeric() {
                character.to_ascii_uppercase()
            } else {
                '_'
            }
        })
        .collect()
}

fn render_error() -> AppError {
    AppError::Message("该应用尚未有安全的配置适配器".to_string())
}

pub(crate) fn projected_provider_id(definition: &ProviderDefinition, app: &str) -> String {
    definition
        .source
        .as_ref()
        .and_then(|source| source.source_ref.strip_prefix("universal:"))
        .filter(|_| matches!(app, "claude" | "codex" | "gemini"))
        .map(|legacy_id| format!("universal-{app}-{legacy_id}"))
        .unwrap_or_else(|| format!("provider-center-{app}-{}", definition.id))
}

fn is_sensitive_key(key: &str) -> bool {
    let normalized = key
        .chars()
        .filter(|character| character.is_ascii_alphanumeric())
        .flat_map(char::to_lowercase)
        .collect::<String>();
    normalized == "token"
        || normalized.contains("apikey")
        || normalized.contains("accesstoken")
        || normalized.contains("refreshtoken")
        || normalized.contains("authtoken")
        || normalized.contains("authorization")
        || normalized.contains("password")
        || normalized.contains("secret")
}

fn redact_embedded_config(value: &str) -> String {
    value
        .lines()
        .map(|line| {
            let trimmed = line.trim_start();
            let key = trimmed
                .split_once('=')
                .map(|(key, _)| key)
                .or_else(|| trimmed.split_once(':').map(|(key, _)| key));
            if key.is_some_and(is_sensitive_key) {
                let indentation = &line[..line.len() - trimmed.len()];
                format!("{indentation}{} = <redacted>", key.unwrap().trim())
            } else {
                line.to_string()
            }
        })
        .collect::<Vec<_>>()
        .join("\n")
}

pub(crate) fn credential_safe_value(value: &serde_json::Value) -> serde_json::Value {
    match value {
        serde_json::Value::Object(values) => serde_json::Value::Object(
            values
                .iter()
                .filter(|(key, _)| !is_sensitive_key(key))
                .map(|(key, value)| (key.clone(), credential_safe_value(value)))
                .collect(),
        ),
        serde_json::Value::Array(values) => {
            serde_json::Value::Array(values.iter().map(credential_safe_value).collect())
        }
        serde_json::Value::String(value) => {
            serde_json::Value::String(redact_embedded_config(value))
        }
        _ => value.clone(),
    }
}

pub(crate) fn provider_fingerprint(provider: &Provider) -> Result<String, AppError> {
    let stable = json!({
        "settingsConfig": credential_safe_value(&provider.settings_config),
    });
    let bytes = serde_json::to_vec(&stable)
        .map_err(|error| AppError::Message(format!("无法计算配置指纹: {error}")))?;
    Ok(format!("{:x}", Sha256::digest(bytes)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unknown_app_returns_stable_error_code() {
        let result = AppAdapterRegistry::global().resolve("not-a-real-app");
        let error = match result {
            Ok(_) => panic!("unknown applications must be rejected"),
            Err(error) => error,
        };
        assert!(matches!(error, AppError::AppNotSupported { .. }));
        assert_eq!(error.to_string(), "APP_NOT_SUPPORTED: not-a-real-app");
    }

    #[test]
    fn registry_covers_every_app_type_variant() {
        let registry = AppAdapterRegistry::global();
        assert_eq!(registry.adapters().count(), AppType::all().count());
        for app in AppType::all() {
            registry
                .resolve(app.as_str())
                .unwrap_or_else(|error| panic!("{} has no adapter: {error}", app.as_str()));
        }
    }

    #[test]
    fn fingerprint_ignores_plain_token_fields_and_embedded_tokens() {
        let first = Provider::with_id(
            "first".to_string(),
            "First".to_string(),
            json!({
                "token": "token-first",
                "config": "model = \"gpt-5\"\ntoken = \"embedded-first\"\n",
            }),
            None,
        );
        let second = Provider::with_id(
            "second".to_string(),
            "Second".to_string(),
            json!({
                "token": "token-second",
                "config": "model = \"gpt-5\"\ntoken = \"embedded-second\"\n",
            }),
            None,
        );
        assert_eq!(
            provider_fingerprint(&first).expect("fingerprint first"),
            provider_fingerprint(&second).expect("fingerprint second")
        );

        let changed_model = Provider::with_id(
            "third".to_string(),
            "Third".to_string(),
            json!({
                "token": "token-third",
                "config": "model = \"gpt-5.1\"\ntoken = \"embedded-third\"\n",
            }),
            None,
        );
        assert_ne!(
            provider_fingerprint(&first).expect("fingerprint first"),
            provider_fingerprint(&changed_model).expect("fingerprint changed model")
        );
    }
}
