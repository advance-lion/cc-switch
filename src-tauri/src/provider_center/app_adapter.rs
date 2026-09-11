//! Provider Center application adapters.
//!
//! This is intentionally a thin compatibility layer over the existing
//! `ProviderService` and provider configuration formats. It centralizes the
//! app-specific decisions used by Provider Center without introducing another
//! persistence or live-config implementation.

use crate::app_config::AppType;
use crate::error::AppError;
use crate::provider::{
    ClaudeModelConfig, CodexModelConfig, GeminiModelConfig, Provider, UniversalProvider,
};
use crate::services::ProviderService;
use crate::store::AppState;
use serde_json::json;
use sha2::{Digest, Sha256};
use std::str::FromStr;
use std::sync::OnceLock;

use super::ProviderDefinition;

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
        if self.supported_protocols.contains(&protocol) {
            Ok(())
        } else {
            Err(AppError::Message(format!(
                "{} 当前不能直接使用 {protocol} 协议；请保留单应用配置或选择兼容协议",
                self.app_id()
            )))
        }
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
        self.validate_protocol(&definition.protocol)?;
        let mut provider = (self.render)(definition, secret)?;
        provider.id = projected_provider_id(definition, self.app_id());
        provider.category = Some("provider-center".to_string());
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

    pub(crate) fn current(&self, state: &AppState) -> Result<String, AppError> {
        ProviderService::current(state, self.app_type())
    }

    pub(crate) fn read_live_settings(&self) -> Result<serde_json::Value, AppError> {
        ProviderService::read_live_settings(self.app_type())
    }

    pub(crate) fn apply(&self, state: &AppState, provider: Provider) -> Result<(), AppError> {
        let id = provider.id.clone();
        if state.db.get_provider_by_id(&id, self.app_id())?.is_some() {
            ProviderService::update(state, self.app_type(), Some(&id), provider)?;
        } else {
            ProviderService::add(state, self.app_type(), provider, true)?;
        }
        if !self.app_type.is_additive_mode() {
            ProviderService::switch(state, self.app_type(), &id)?;
        }
        Ok(())
    }

    pub(crate) fn delete(&self, state: &AppState, provider_id: &str) -> Result<(), AppError> {
        ProviderService::delete(state, self.app_type(), provider_id)
    }
}

fn models(definition: &ProviderDefinition) -> Vec<String> {
    if definition.models.is_empty() {
        vec!["default".to_string()]
    } else {
        definition.models.clone()
    }
}

fn universal(definition: &ProviderDefinition, secret: &str) -> UniversalProvider {
    UniversalProvider::new(
        format!("pc-{}", definition.id),
        definition.name.clone(),
        definition.protocol.clone(),
        definition.base_url.clone(),
        secret.to_string(),
    )
}

fn render_claude(definition: &ProviderDefinition, secret: &str) -> Result<Provider, AppError> {
    let mut universal = universal(definition, secret);
    universal.apps.claude = true;
    universal.models.claude = Some(ClaudeModelConfig {
        model: models(definition).first().cloned(),
        ..Default::default()
    });
    universal.to_claude_provider().ok_or_else(render_error)
}

fn render_codex(definition: &ProviderDefinition, secret: &str) -> Result<Provider, AppError> {
    let mut universal = universal(definition, secret);
    universal.apps.codex = true;
    universal.models.codex = Some(CodexModelConfig {
        model: models(definition).first().cloned(),
        reasoning_effort: Some("medium".to_string()),
    });
    universal.to_codex_provider().ok_or_else(render_error)
}

fn render_gemini(definition: &ProviderDefinition, secret: &str) -> Result<Provider, AppError> {
    let mut universal = universal(definition, secret);
    universal.apps.gemini = true;
    universal.models.gemini = Some(GeminiModelConfig {
        model: models(definition).first().cloned(),
    });
    universal.to_gemini_provider().ok_or_else(render_error)
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
    let model = models(definition)
        .first()
        .cloned()
        .unwrap_or_else(|| "gpt-4o".to_string());
    let model_key = "shared";
    let config = format!(
        "[models]\ndefault = \"{model_key}\"\n\n[model.{model_key}]\nmodel = \"{}\"\nbase_url = \"{}\"\napi_key = \"{}\"\nname = \"{}\"\napi_backend = \"openai\"\ncontext_window = 128000\n",
        toml_string(&model),
        toml_string(&definition.base_url),
        toml_string(secret),
        toml_string(&definition.name)
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
    let models = models(definition);
    Ok(Provider::with_id(
        String::new(),
        definition.name.clone(),
        json!({
            "npm": "@ai-sdk/openai-compatible", "name": definition.name,
            "options": { "baseURL": openai_compatible_base_url(definition), "apiKey": secret },
            "models": models.iter().map(|item| (item.clone(), json!({ "name": item }))).collect::<serde_json::Map<String, serde_json::Value>>()
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
    let models = models(definition);
    Ok(Provider::with_id(
        String::new(),
        definition.name.clone(),
        json!({
            "baseUrl": openai_compatible_base_url(definition), "apiKey": secret,
            "api": api,
            "models": models.iter().map(|item| json!({ "id": item, "name": item })).collect::<Vec<_>>()
        }),
        None,
    ))
}

fn render_hermes(definition: &ProviderDefinition, secret: &str) -> Result<Provider, AppError> {
    let models = models(definition);
    Ok(Provider::with_id(
        String::new(),
        definition.name.clone(),
        json!({
            "base_url": openai_compatible_base_url(definition), "api_key": secret,
            "models": models.iter().map(|item| (item.clone(), json!({}))).collect::<serde_json::Map<String, serde_json::Value>>()
        }),
        None,
    ))
}

fn render_pi(definition: &ProviderDefinition, secret: &str) -> Result<Provider, AppError> {
    let api = native_api_name(&definition.protocol).ok_or_else(render_error)?;
    let models = models(definition);
    Ok(Provider::with_id(
        String::new(),
        definition.name.clone(),
        json!({
            "apiKey": secret,
            "models": models.iter().map(|item| json!({
                "id": item,
                "name": item,
                "api": api,
                "baseUrl": openai_compatible_base_url(definition)
            })).collect::<Vec<_>>()
        }),
        None,
    ))
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

pub(crate) fn provider_fingerprint(provider: &Provider) -> Result<String, AppError> {
    let stable = json!({
        "id": provider.id,
        "name": provider.name,
        "settingsConfig": provider.settings_config,
        "websiteUrl": provider.website_url,
        "category": provider.category,
        "notes": provider.notes,
        "meta": provider.meta,
        "icon": provider.icon,
        "iconColor": provider.icon_color,
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
}
