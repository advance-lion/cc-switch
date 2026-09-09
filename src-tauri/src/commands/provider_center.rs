use crate::provider_center::{
    self, ImportCandidate, ModelDiscoveryResult, ProviderApplyPreview,
    ProviderApplyTransaction, ProviderBinding, ProviderCenterOperationState, ProviderCenterState,
    ProviderDefinition, SaveProviderDefinitionInput, UnifiedModelCatalog,
};
use crate::store::AppState;
use tauri::State;

#[tauri::command]
pub fn get_provider_center(state: State<'_, AppState>) -> Result<ProviderCenterState, String> {
    provider_center::state(state.inner()).map_err(|error| error.to_string())
}

#[tauri::command]
pub fn get_provider_center_model_catalog(
    state: State<'_, AppState>,
    #[allow(non_snake_case)] appTypes: Option<Vec<String>>,
) -> Result<UnifiedModelCatalog, String> {
    provider_center::unified_model_catalog(state.inner(), appTypes.unwrap_or_default())
        .map_err(|error| error.to_string())
}

#[tauri::command]
pub fn save_provider_center_definition(
    state: State<'_, AppState>,
    input: SaveProviderDefinitionInput,
) -> Result<ProviderDefinition, String> {
    provider_center::save_definition(state.inner(), input).map_err(|error| error.to_string())
}

#[tauri::command]
pub fn delete_provider_center_definition(
    state: State<'_, AppState>,
    #[allow(non_snake_case)] providerId: String,
) -> Result<(), String> {
    provider_center::delete_definition(state.inner(), &providerId)
        .map_err(|error| error.to_string())
}

#[tauri::command]
pub fn duplicate_provider_center_definition(
    state: State<'_, AppState>,
    #[allow(non_snake_case)] providerId: String,
) -> Result<ProviderDefinition, String> {
    provider_center::duplicate_definition(state.inner(), &providerId)
        .map_err(|error| error.to_string())
}

#[tauri::command]
pub async fn discover_provider_center_models(
    state: State<'_, AppState>,
    #[allow(non_snake_case)] providerId: String,
) -> Result<ModelDiscoveryResult, String> {
    provider_center::discover_models(state.inner(), &providerId)
        .await
        .map_err(|error| error.to_string())
}

#[tauri::command]
pub fn scan_provider_center_imports(
    state: State<'_, AppState>,
) -> Result<Vec<ImportCandidate>, String> {
    provider_center::scan_imports(state.inner()).map_err(|error| error.to_string())
}

#[tauri::command]
pub fn import_provider_center_candidate(
    state: State<'_, AppState>,
    #[allow(non_snake_case)] sourceRef: String,
    #[allow(non_snake_case)] appTypes: Vec<String>,
) -> Result<ProviderDefinition, String> {
    provider_center::import_candidate(state.inner(), &sourceRef, appTypes)
        .map_err(|error| error.to_string())
}

#[tauri::command]
pub async fn apply_provider_center_bindings(
    state: State<'_, AppState>,
    operations: State<'_, ProviderCenterOperationState>,
    #[allow(non_snake_case)] providerId: String,
    #[allow(non_snake_case)] appTypes: Vec<String>,
) -> Result<Vec<ProviderBinding>, String> {
    let targets =
        provider_center::apply_target_app_types(state.inner(), &providerId, appTypes.clone())
            .map_err(|error| error.to_string())?;
    let _guards = operations.lock_apps(targets).await;
    provider_center::apply_bindings(state.inner(), &providerId, appTypes)
        .map_err(|error| error.to_string())
}

#[tauri::command]
pub fn preview_provider_center_apply(
    state: State<'_, AppState>,
    #[allow(non_snake_case)] providerId: String,
    #[allow(non_snake_case)] appTypes: Vec<String>,
) -> Result<ProviderApplyPreview, String> {
    provider_center::preview_apply(state.inner(), &providerId, appTypes)
        .map_err(|error| error.to_string())
}

#[tauri::command]
pub async fn apply_provider_center_transaction(
    state: State<'_, AppState>,
    operations: State<'_, ProviderCenterOperationState>,
    #[allow(non_snake_case)] providerId: String,
    #[allow(non_snake_case)] appTypes: Vec<String>,
    #[allow(non_snake_case)] previewToken: String,
) -> Result<ProviderApplyTransaction, String> {
    let targets =
        provider_center::apply_target_app_types(state.inner(), &providerId, appTypes.clone())
            .map_err(|error| error.to_string())?;
    let _guards = operations.lock_apps(targets).await;
    provider_center::apply_transaction(
        state.inner(),
        &providerId,
        appTypes,
        &previewToken,
    )
    .map_err(|error| error.to_string())
}

#[tauri::command]
pub async fn restore_provider_center_transaction(
    state: State<'_, AppState>,
    operations: State<'_, ProviderCenterOperationState>,
    #[allow(non_snake_case)] transactionId: String,
) -> Result<ProviderApplyTransaction, String> {
    let targets = provider_center::restore_target_app_types(state.inner(), &transactionId)
        .map_err(|error| error.to_string())?;
    let _guards = operations.lock_apps(targets).await;
    provider_center::restore_transaction(state.inner(), &transactionId)
        .map_err(|error| error.to_string())
}

#[tauri::command]
pub fn set_provider_center_binding_override(
    state: State<'_, AppState>,
    #[allow(non_snake_case)] providerId: String,
    #[allow(non_snake_case)] appType: String,
    enabled: bool,
) -> Result<(), String> {
    provider_center::set_binding_override(state.inner(), &providerId, &appType, enabled)
        .map_err(|error| error.to_string())
}
