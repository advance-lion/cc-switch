use crate::provider_center::{
    self, ImportCandidate, ImportCommitDecision, ImportCommitResult, ModelDiscoveryResult,
    ProviderApplyPreview, ProviderApplyTransaction, ProviderBinding, ProviderCenterOperationState,
    ProviderCenterState, ProviderDefinition, ProviderImportSession, SaveProviderDefinitionInput,
    UnifiedModelCatalog,
};
use crate::store::AppState;
use tauri::State;

#[tauri::command]
pub async fn get_provider_center(
    state: State<'_, AppState>,
    operations: State<'_, ProviderCenterOperationState>,
) -> Result<ProviderCenterState, String> {
    let _data_guard = operations.lock_data().await;
    provider_center::state(state.inner()).map_err(|error| error.to_string())
}

#[tauri::command]
pub async fn get_provider_center_model_catalog(
    state: State<'_, AppState>,
    operations: State<'_, ProviderCenterOperationState>,
    #[allow(non_snake_case)] appTypes: Option<Vec<String>>,
) -> Result<UnifiedModelCatalog, String> {
    let _data_guard = operations.lock_data().await;
    provider_center::unified_model_catalog(state.inner(), appTypes.unwrap_or_default())
        .map_err(|error| error.to_string())
}

#[tauri::command]
pub async fn save_provider_center_definition(
    state: State<'_, AppState>,
    operations: State<'_, ProviderCenterOperationState>,
    input: SaveProviderDefinitionInput,
) -> Result<ProviderDefinition, String> {
    let _data_guard = operations.lock_data().await;
    provider_center::save_definition(state.inner(), input).map_err(|error| error.to_string())
}

#[tauri::command]
pub async fn delete_provider_center_definition(
    state: State<'_, AppState>,
    operations: State<'_, ProviderCenterOperationState>,
    #[allow(non_snake_case)] providerId: String,
) -> Result<(), String> {
    let _data_guard = operations.lock_data().await;
    provider_center::delete_definition(state.inner(), &providerId)
        .map_err(|error| error.to_string())
}

#[tauri::command]
pub async fn duplicate_provider_center_definition(
    state: State<'_, AppState>,
    operations: State<'_, ProviderCenterOperationState>,
    #[allow(non_snake_case)] providerId: String,
) -> Result<ProviderDefinition, String> {
    let _data_guard = operations.lock_data().await;
    provider_center::duplicate_definition(state.inner(), &providerId)
        .map_err(|error| error.to_string())
}

#[tauri::command]
pub async fn discover_provider_center_models(
    state: State<'_, AppState>,
    operations: State<'_, ProviderCenterOperationState>,
    #[allow(non_snake_case)] providerId: String,
) -> Result<ModelDiscoveryResult, String> {
    let _data_guard = operations.lock_data().await;
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
pub async fn start_provider_center_import_session(
    state: State<'_, AppState>,
    operations: State<'_, ProviderCenterOperationState>,
    #[allow(non_snake_case)] appTypes: Option<Vec<String>>,
) -> Result<ProviderImportSession, String> {
    let _data_guard = operations.lock_data().await;
    provider_center::start_import_session(state.inner(), appTypes.unwrap_or_default())
        .map_err(|error| error.to_string())
}

#[tauri::command]
pub async fn get_provider_center_import_session(
    state: State<'_, AppState>,
    operations: State<'_, ProviderCenterOperationState>,
    #[allow(non_snake_case)] sessionId: String,
) -> Result<ProviderImportSession, String> {
    let _data_guard = operations.lock_data().await;
    provider_center::get_import_session(state.inner(), &sessionId)
        .map_err(|error| error.to_string())
}

#[tauri::command]
pub async fn commit_provider_center_import_candidate(
    state: State<'_, AppState>,
    operations: State<'_, ProviderCenterOperationState>,
    #[allow(non_snake_case)] sessionId: String,
    #[allow(non_snake_case)] candidateId: String,
    #[allow(non_snake_case)] appTypes: Vec<String>,
    decision: ImportCommitDecision,
) -> Result<ImportCommitResult, String> {
    let _data_guard = operations.lock_data().await;
    provider_center::commit_import_session_candidate(
        state.inner(),
        &sessionId,
        &candidateId,
        appTypes,
        decision,
    )
    .map_err(|error| error.to_string())
}

#[tauri::command]
pub async fn import_provider_center_candidate(
    state: State<'_, AppState>,
    operations: State<'_, ProviderCenterOperationState>,
    #[allow(non_snake_case)] sourceRef: String,
    #[allow(non_snake_case)] appTypes: Vec<String>,
) -> Result<ProviderDefinition, String> {
    let _data_guard = operations.lock_data().await;
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
    let _data_guard = operations.lock_data().await;
    let targets =
        provider_center::apply_target_app_types(state.inner(), &providerId, appTypes.clone())
            .map_err(|error| error.to_string())?;
    let _guards = operations.lock_apps(targets).await;
    provider_center::apply_bindings(state.inner(), &providerId, appTypes)
        .map_err(|error| error.to_string())
}

#[tauri::command]
pub async fn preview_provider_center_apply(
    state: State<'_, AppState>,
    operations: State<'_, ProviderCenterOperationState>,
    #[allow(non_snake_case)] providerId: String,
    #[allow(non_snake_case)] appTypes: Vec<String>,
) -> Result<ProviderApplyPreview, String> {
    let _data_guard = operations.lock_data().await;
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
    #[allow(non_snake_case)] idempotencyKey: Option<String>,
) -> Result<ProviderApplyTransaction, String> {
    let _data_guard = operations.lock_data().await;
    let targets =
        provider_center::apply_target_app_types(state.inner(), &providerId, appTypes.clone())
            .map_err(|error| error.to_string())?;
    let _guards = operations.lock_apps(targets).await;
    provider_center::apply_transaction(
        state.inner(),
        &providerId,
        appTypes,
        &previewToken,
        idempotencyKey.as_deref(),
    )
    .map_err(|error| error.to_string())
}

#[tauri::command]
pub async fn restore_provider_center_transaction(
    state: State<'_, AppState>,
    operations: State<'_, ProviderCenterOperationState>,
    #[allow(non_snake_case)] transactionId: String,
) -> Result<ProviderApplyTransaction, String> {
    let _data_guard = operations.lock_data().await;
    let targets = provider_center::restore_target_app_types(state.inner(), &transactionId)
        .map_err(|error| error.to_string())?;
    let _guards = operations.lock_apps(targets).await;
    provider_center::restore_transaction(state.inner(), &transactionId)
        .map_err(|error| error.to_string())
}

#[tauri::command]
pub async fn set_provider_center_binding_override(
    state: State<'_, AppState>,
    operations: State<'_, ProviderCenterOperationState>,
    #[allow(non_snake_case)] providerId: String,
    #[allow(non_snake_case)] appType: String,
    enabled: bool,
) -> Result<(), String> {
    let _data_guard = operations.lock_data().await;
    provider_center::set_binding_override(state.inner(), &providerId, &appType, enabled)
        .map_err(|error| error.to_string())
}

#[tauri::command]
pub async fn disable_provider_center_binding(
    state: State<'_, AppState>,
    operations: State<'_, ProviderCenterOperationState>,
    #[allow(non_snake_case)] providerId: String,
    #[allow(non_snake_case)] appType: String,
    #[allow(non_snake_case)] removeProjection: bool,
) -> Result<(), String> {
    let _data_guard = operations.lock_data().await;
    let _guards = operations.lock_apps(vec![appType.clone()]).await;
    provider_center::disable_binding(state.inner(), &providerId, &appType, removeProjection)
        .map_err(|error| error.to_string())
}
