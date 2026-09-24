//! Additive Provider service for Qoder CLI.

use super::{ProviderLiveMembership, ProviderService, SwitchResult};
use crate::app_config::AppType;
use crate::error::AppError;
use crate::provider::{Provider, ProviderMeta};
use crate::store::AppState;
use indexmap::IndexMap;
use serde_json::Value;

const QODER_APP: &str = "qoder";

pub(super) fn membership() -> ProviderLiveMembership {
    match crate::qoder_config::read_qoder_native_providers() {
        Ok(providers) => ProviderLiveMembership::available(providers.into_keys().collect()),
        Err(error) => ProviderLiveMembership::unavailable(error.to_string()),
    }
}

pub(super) fn list(state: &AppState) -> Result<IndexMap<String, Provider>, AppError> {
    let _guard = futures::executor::block_on(state.proxy_service.lock_switch_for_app(QODER_APP));
    match crate::qoder_config::read_qoder_native_providers() {
        Ok(native) => {
            if let Err(error) = sync_native_locked(state, &native) {
                log::warn!("Failed to sync Qoder providers from native config: {error}");
            }
        }
        Err(error) => {
            log::warn!("Failed to read Qoder providers; showing saved catalog: {error}");
        }
    }
    state.db.get_all_providers(QODER_APP)
}

pub(super) fn list_read_only(state: &AppState) -> Result<IndexMap<String, Provider>, AppError> {
    let _guard = futures::executor::block_on(state.proxy_service.lock_switch_for_app(QODER_APP));
    let mut providers = state.db.get_all_providers(QODER_APP)?;
    match crate::qoder_config::read_qoder_native_providers() {
        Ok(native) => {
            for (id, config) in native {
                let mut provider = providers.shift_remove(&id).unwrap_or_else(|| {
                    let name = native_provider_name(&config).unwrap_or(&id).to_string();
                    let mut native = Provider::with_id(id.clone(), name, config.clone(), None);
                    native.category = Some("custom".to_string());
                    native.icon = Some("qoder".to_string());
                    native
                });
                merge_native_config(&mut provider, config);
                providers.insert(id, provider);
            }
        }
        Err(error) => {
            log::warn!("Failed to read Qoder providers; showing saved catalog: {error}");
        }
    }
    Ok(providers)
}

pub(super) fn add(
    state: &AppState,
    mut provider: Provider,
    add_to_live: bool,
) -> Result<bool, AppError> {
    let app_type = AppType::Qoder;
    let _guard =
        futures::executor::block_on(state.proxy_service.lock_switch_for_app(app_type.as_str()));
    strip_unsupported_metadata(&mut provider);
    align_native_display_name(&mut provider);
    ProviderService::validate_provider_settings(&app_type, &provider)?;
    ProviderService::normalize_usage_script_credential_overrides(&app_type, &mut provider);

    if state
        .db
        .get_provider_by_id(&provider.id, app_type.as_str())?
        .is_some()
    {
        return Err(AppError::InvalidInput(format!(
            "Qoder provider '{}' already exists",
            provider.id
        )));
    }
    if !add_to_live && crate::qoder_config::qoder_provider_exists(&provider.id)? {
        return Err(AppError::InvalidInput(format!(
            "Qoder provider key '{}' already exists in settings.json",
            provider.id
        )));
    }

    let native_inserted = if add_to_live {
        crate::qoder_config::insert_qoder_provider(&provider.id, &provider.settings_config)?
    } else {
        false
    };
    if let Err(error) = state.db.save_provider(app_type.as_str(), &provider) {
        if native_inserted {
            if let Err(rollback) = crate::qoder_config::remove_qoder_provider_if_matches(
                &provider.id,
                &provider.settings_config,
            ) {
                return Err(AppError::Config(format!(
                    "failed to save Qoder provider: {error}; native rollback failed: {rollback}"
                )));
            }
        }
        return Err(error);
    }
    Ok(true)
}

pub(super) fn update(
    state: &AppState,
    original_id: Option<&str>,
    mut provider: Provider,
) -> Result<bool, AppError> {
    let app_type = AppType::Qoder;
    let _guard =
        futures::executor::block_on(state.proxy_service.lock_switch_for_app(app_type.as_str()));
    let original_id = original_id.unwrap_or(&provider.id).to_string();
    if original_id != provider.id {
        return Err(AppError::InvalidInput(
            "Qoder provider keys cannot be renamed while they are managed".to_string(),
        ));
    }
    state
        .db
        .get_provider_by_id(&original_id, app_type.as_str())?
        .ok_or_else(|| {
            AppError::InvalidInput(format!("Qoder provider '{original_id}' not found"))
        })?;
    strip_unsupported_metadata(&mut provider);
    align_native_display_name(&mut provider);
    ProviderService::validate_provider_settings(&app_type, &provider)?;
    ProviderService::normalize_usage_script_credential_overrides(&app_type, &mut provider);

    let previous_native = crate::qoder_config::replace_qoder_provider_if_present(
        &original_id,
        &provider.settings_config,
    )?;
    if let Err(error) = state.db.save_provider(app_type.as_str(), &provider) {
        if let Some(previous_native) = previous_native.as_ref() {
            if let Err(rollback) = crate::qoder_config::replace_qoder_provider(
                &original_id,
                &provider.settings_config,
                previous_native,
            ) {
                return Err(AppError::Config(format!(
                    "failed to save Qoder provider: {error}; native rollback failed: {rollback}"
                )));
            }
        }
        return Err(error);
    }
    Ok(true)
}

pub(super) fn delete(state: &AppState, id: &str) -> Result<(), AppError> {
    let app_type = AppType::Qoder;
    let _guard =
        futures::executor::block_on(state.proxy_service.lock_switch_for_app(app_type.as_str()));
    if state
        .db
        .get_provider_by_id(id, app_type.as_str())?
        .is_none()
    {
        return Ok(());
    }
    let removed = crate::qoder_config::remove_qoder_provider(id)?;
    if let Err(error) = state.db.delete_provider(app_type.as_str(), id) {
        if let Some(removed) = removed.as_ref() {
            if let Err(rollback) =
                crate::qoder_config::restore_qoder_provider_if_missing(id, removed)
            {
                return Err(AppError::Config(format!(
                    "failed to delete Qoder provider: {error}; native rollback failed: {rollback}"
                )));
            }
        }
        return Err(error);
    }
    Ok(())
}

pub(super) fn remove(state: &AppState, id: &str) -> Result<(), AppError> {
    let app_type = AppType::Qoder;
    let _guard =
        futures::executor::block_on(state.proxy_service.lock_switch_for_app(app_type.as_str()));
    let provider = state
        .db
        .get_provider_by_id(id, app_type.as_str())?
        .ok_or_else(|| AppError::InvalidInput(format!("Qoder provider '{id}' not found")))?;
    let Some(removed) = crate::qoder_config::remove_qoder_provider(id)? else {
        return Ok(());
    };
    let mut synced = provider;
    merge_native_config(&mut synced, removed.clone());
    if let Err(error) = state.db.save_provider(app_type.as_str(), &synced) {
        if let Err(rollback) = crate::qoder_config::restore_qoder_provider_if_missing(id, &removed)
        {
            return Err(AppError::Config(format!(
                "failed to preserve Qoder provider before removal: {error}; native rollback failed: {rollback}"
            )));
        }
        return Err(error);
    }
    Ok(())
}

pub(super) fn enable(state: &AppState, id: &str) -> Result<SwitchResult, AppError> {
    let app_type = AppType::Qoder;
    let _guard =
        futures::executor::block_on(state.proxy_service.lock_switch_for_app(app_type.as_str()));
    let provider = state
        .db
        .get_provider_by_id(id, app_type.as_str())?
        .ok_or_else(|| AppError::InvalidInput(format!("Qoder provider '{id}' not found")))?;
    if let Some(native) = crate::qoder_config::read_qoder_native_provider(id)? {
        let mut synced = provider;
        merge_native_config(&mut synced, native);
        state.db.save_provider(app_type.as_str(), &synced)?;
        return Ok(SwitchResult::default());
    }
    ProviderService::validate_provider_settings(&app_type, &provider)?;
    crate::qoder_config::insert_qoder_provider(id, &provider.settings_config)?;
    Ok(SwitchResult::default())
}

fn sync_native_locked(
    state: &AppState,
    native: &IndexMap<String, Value>,
) -> Result<usize, AppError> {
    let saved = state.db.get_all_providers(QODER_APP)?;
    let mut changed = 0;
    for (id, config) in native {
        let mut provider = saved.get(id).cloned().unwrap_or_else(|| {
            let name = native_provider_name(config).unwrap_or(id).to_string();
            let mut imported = Provider::with_id(id.clone(), name, config.clone(), None);
            imported.category = Some("custom".to_string());
            imported.icon = Some("qoder".to_string());
            imported
        });
        let is_new = !saved.contains_key(id);
        let previous_name = provider.name.clone();
        let previous_config = provider.settings_config.clone();
        merge_native_config(&mut provider, config.clone());
        if !is_new && provider.name == previous_name && provider.settings_config == previous_config
        {
            continue;
        }
        state.db.save_provider(QODER_APP, &provider)?;
        changed += 1;
    }
    Ok(changed)
}

fn merge_native_config(provider: &mut Provider, config: Value) {
    if let Some(name) = native_provider_name(&config) {
        provider.name = name.to_string();
    }
    provider.settings_config = config;
}

fn native_provider_name(config: &Value) -> Option<&str> {
    config
        .get("displayName")
        .and_then(Value::as_str)
        .filter(|name| !name.trim().is_empty())
}

fn align_native_display_name(provider: &mut Provider) {
    if let Some(config) = provider.settings_config.as_object_mut() {
        config.insert(
            "displayName".to_string(),
            Value::String(provider.name.clone()),
        );
    }
}

fn strip_unsupported_metadata(provider: &mut Provider) {
    provider.in_failover_queue = false;
    let Some(meta) = provider.meta.take() else {
        return;
    };
    provider.meta = Some(ProviderMeta {
        usage_script: meta.usage_script,
        is_partner: meta.is_partner,
        partner_promotion_key: meta.partner_promotion_key,
        ..ProviderMeta::default()
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::database::Database;
    use crate::qoder_config::test_support::TestQoderDir;
    use serde_json::json;
    use serial_test::serial;
    use std::sync::Arc;

    fn state() -> AppState {
        AppState::new(Arc::new(
            Database::memory().expect("create in-memory database"),
        ))
    }

    fn input() -> Provider {
        Provider::with_id(
            "qoder-test".to_string(),
            "Qoder Test".to_string(),
            json!({
                "type": "openai-compatible",
                "displayName": "Qoder Test",
                "protocol": "openai",
                "authType": "bearer",
                "baseUrl": "https://api.example.com/v1",
                "apiKey": "secret",
                "model": "model-a",
                "models": [{ "model": "model-a" }]
            }),
            None,
        )
    }

    #[test]
    #[serial]
    fn additive_membership_round_trips_through_qoder_settings() {
        let _qoder = TestQoderDir::new();
        let state = state();
        ProviderService::add(&state, AppType::Qoder, input(), false).expect("save provider");
        assert!(!crate::qoder_config::qoder_provider_exists("qoder-test").unwrap());
        assert_eq!(membership().provider_ids, Some(Vec::new()));

        ProviderService::switch(&state, AppType::Qoder, "qoder-test").expect("enable provider");
        assert!(crate::qoder_config::qoder_provider_exists("qoder-test").unwrap());
        assert_eq!(
            membership().provider_ids,
            Some(vec!["qoder-test".to_string()])
        );

        ProviderService::remove_from_live_config(&state, AppType::Qoder, "qoder-test")
            .expect("disable provider");
        assert!(!crate::qoder_config::qoder_provider_exists("qoder-test").unwrap());
        assert_eq!(membership().provider_ids, Some(Vec::new()));
        assert!(state
            .db
            .get_provider_by_id("qoder-test", QODER_APP)
            .unwrap()
            .is_some());
    }
}
