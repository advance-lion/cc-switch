//! Unified Provider Center persistence.
//!
//! The v19 tables are the authoritative store for definitions, models,
//! bindings and apply history. API keys and encrypted recovery snapshots are
//! deliberately kept out of these tables.

use crate::database::{lock_conn, to_json_string, Database};
use crate::error::AppError;
use crate::provider_center::{
    ProviderApplyTargetResult, ProviderApplyTransaction, ProviderBinding, ProviderDefinition,
    ProviderSource,
};
use rusqlite::{params, OptionalExtension, Transaction};
use serde_json::{json, Value};

#[derive(Debug, Clone)]
pub struct ProviderImportSessionRecord {
    pub id: String,
    pub state: String,
    pub requested_apps_json: String,
    pub error_summary_json: String,
    pub created_at: i64,
    pub expires_at: i64,
    pub completed_at: Option<i64>,
}

#[derive(Debug, Clone)]
pub struct ProviderImportCandidateRecord {
    pub id: String,
    pub session_id: String,
    pub source_app_type: String,
    pub source_provider_id: Option<String>,
    pub source_locator: Option<String>,
    pub normalized_json: String,
    pub models_json: String,
    pub temporary_secret_ref: Option<String>,
    pub credential_configured: bool,
    pub fingerprint: String,
    pub conflict_json: Option<String>,
}

fn binding_id(provider_id: &str, app_type: &str) -> String {
    format!("{provider_id}:{app_type}")
}

fn parse_json<T: serde::de::DeserializeOwned>(text: &str, field: &str) -> Result<T, AppError> {
    serde_json::from_str(text)
        .map_err(|error| AppError::Database(format!("解析 {field} 失败: {error}")))
}

fn upsert_definition_tx(
    tx: &Transaction<'_>,
    definition: &ProviderDefinition,
) -> Result<(), AppError> {
    let metadata = to_json_string(&json!({
        "discoveredModels": definition.discovered_models,
        "lastDiscoveryAt": definition.last_discovery_at,
        "lastDiscoveryError": definition.last_discovery_error,
    }))?;
    tx.execute(
        "INSERT INTO provider_definitions (
            id, name, provider_kind, protocol, base_url, secret_ref, notes,
            enabled, metadata_json, revision, created_at, updated_at
         ) VALUES (?1, ?2, 'api_key', ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)
         ON CONFLICT(id) DO UPDATE SET
            name = excluded.name,
            protocol = excluded.protocol,
            base_url = excluded.base_url,
            secret_ref = excluded.secret_ref,
            notes = excluded.notes,
            enabled = excluded.enabled,
            metadata_json = excluded.metadata_json,
            revision = excluded.revision,
            updated_at = excluded.updated_at",
        params![
            definition.id,
            definition.name,
            definition.protocol,
            definition.base_url,
            definition
                .credential_configured
                .then(|| format!("cc-switch/provider/{}/api-key", definition.id)),
            definition.notes,
            definition.enabled,
            metadata,
            definition.revision as i64,
            definition.created_at,
            definition.updated_at,
        ],
    )
    .map_err(|error| AppError::Database(format!("保存模型服务失败: {error}")))?;

    tx.execute(
        "DELETE FROM provider_models WHERE provider_id = ?1",
        params![definition.id],
    )
    .map_err(|error| AppError::Database(format!("更新模型列表失败: {error}")))?;
    for (sort_order, model_id) in definition.models.iter().enumerate() {
        tx.execute(
            "INSERT INTO provider_models (
                provider_id, model_id, enabled, sort_order
             ) VALUES (?1, ?2, 1, ?3)",
            params![definition.id, model_id, sort_order as i64],
        )
        .map_err(|error| AppError::Database(format!("保存模型失败: {error}")))?;
    }

    tx.execute(
        "DELETE FROM provider_sources WHERE provider_id = ?1",
        params![definition.id],
    )
    .map_err(|error| AppError::Database(format!("更新导入来源失败: {error}")))?;
    if let Some(source) = &definition.source {
        tx.execute(
            "INSERT INTO provider_sources (
                id, provider_id, source_app_type, source_provider_id,
                source_locator, source_fingerprint, imported_at, last_observed_at
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
            params![
                format!("source:{}", definition.id),
                definition.id,
                source.source_app,
                source.source_ref,
                source.source_ref,
                source
                    .source_fingerprint
                    .as_deref()
                    .unwrap_or("legacy-import"),
                source.imported_at,
                source.last_observed_at.unwrap_or(source.imported_at),
            ],
        )
        .map_err(|error| AppError::Database(format!("保存导入来源失败: {error}")))?;
    }
    Ok(())
}

fn upsert_binding_tx(
    tx: &Transaction<'_>,
    binding: &ProviderBinding,
    desired_revision: i64,
) -> Result<(), AppError> {
    let overrides = to_json_string(&json!({
        "enabled": binding.enabled,
        "overrideEnabled": binding.override_enabled,
    }))?;
    tx.execute(
        "INSERT INTO provider_bindings (
            id, provider_id, app_type, state, adapter_id, adapter_version,
            projected_provider_id, overrides_json, desired_revision,
            applied_revision, projection_digest, last_apply_transaction_id,
            last_error_message, created_at, updated_at
         ) VALUES (?1, ?2, ?3, ?4, ?5, 1, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?13)
         ON CONFLICT(provider_id, app_type) DO UPDATE SET
            state = excluded.state,
            adapter_id = excluded.adapter_id,
            projected_provider_id = excluded.projected_provider_id,
            overrides_json = excluded.overrides_json,
            desired_revision = excluded.desired_revision,
            applied_revision = excluded.applied_revision,
            projection_digest = excluded.projection_digest,
            last_apply_transaction_id = excluded.last_apply_transaction_id,
            last_error_message = excluded.last_error_message,
            updated_at = excluded.updated_at",
        params![
            binding_id(&binding.provider_id, &binding.app_type),
            binding.provider_id,
            binding.app_type,
            binding.status,
            binding.app_type,
            format!(
                "provider-center-{}-{}",
                binding.app_type, binding.provider_id
            ),
            overrides,
            desired_revision,
            binding.applied_revision.map(|value| value as i64),
            binding.expected_fingerprint,
            binding.last_transaction_id,
            binding.last_error,
            binding.updated_at,
        ],
    )
    .map_err(|error| AppError::Database(format!("保存应用绑定失败: {error}")))?;
    Ok(())
}

impl Database {
    pub fn save_provider_import_session(
        &self,
        session: &ProviderImportSessionRecord,
        candidates: &[ProviderImportCandidateRecord],
    ) -> Result<(), AppError> {
        let mut conn = lock_conn!(self.conn);
        let tx = conn
            .transaction()
            .map_err(|error| AppError::Database(error.to_string()))?;
        tx.execute(
            "INSERT INTO provider_import_sessions (
                id, state, requested_apps_json, error_summary_json,
                created_at, expires_at, completed_at
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)
             ON CONFLICT(id) DO UPDATE SET
                state = excluded.state,
                requested_apps_json = excluded.requested_apps_json,
                error_summary_json = excluded.error_summary_json,
                expires_at = excluded.expires_at,
                completed_at = excluded.completed_at",
            params![
                session.id,
                session.state,
                session.requested_apps_json,
                session.error_summary_json,
                session.created_at,
                session.expires_at,
                session.completed_at,
            ],
        )
        .map_err(|error| AppError::Database(format!("保存导入会话失败: {error}")))?;
        tx.execute(
            "DELETE FROM provider_import_candidates WHERE session_id = ?1",
            params![session.id],
        )
        .map_err(|error| AppError::Database(error.to_string()))?;
        for candidate in candidates {
            tx.execute(
                "INSERT INTO provider_import_candidates (
                    id, session_id, source_app_type, source_provider_id,
                    source_locator, normalized_json, models_json,
                    temporary_secret_ref, credential_configured, fingerprint,
                    conflict_json
                 ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)",
                params![
                    candidate.id,
                    candidate.session_id,
                    candidate.source_app_type,
                    candidate.source_provider_id,
                    candidate.source_locator,
                    candidate.normalized_json,
                    candidate.models_json,
                    candidate.temporary_secret_ref,
                    candidate.credential_configured,
                    candidate.fingerprint,
                    candidate.conflict_json,
                ],
            )
            .map_err(|error| AppError::Database(format!("保存导入候选失败: {error}")))?;
        }
        tx.commit()
            .map_err(|error| AppError::Database(error.to_string()))
    }

    pub fn get_provider_import_session(
        &self,
        session_id: &str,
    ) -> Result<Option<ProviderImportSessionRecord>, AppError> {
        let conn = lock_conn!(self.conn);
        conn.query_row(
            "SELECT id, state, requested_apps_json, error_summary_json,
                    created_at, expires_at, completed_at
             FROM provider_import_sessions WHERE id = ?1",
            params![session_id],
            |row| {
                Ok(ProviderImportSessionRecord {
                    id: row.get(0)?,
                    state: row.get(1)?,
                    requested_apps_json: row.get(2)?,
                    error_summary_json: row.get(3)?,
                    created_at: row.get(4)?,
                    expires_at: row.get(5)?,
                    completed_at: row.get(6)?,
                })
            },
        )
        .optional()
        .map_err(|error| AppError::Database(error.to_string()))
    }

    pub fn list_provider_import_candidates(
        &self,
        session_id: &str,
    ) -> Result<Vec<ProviderImportCandidateRecord>, AppError> {
        let conn = lock_conn!(self.conn);
        let mut stmt = conn
            .prepare(
                "SELECT id, session_id, source_app_type, source_provider_id,
                        source_locator, normalized_json, models_json,
                        temporary_secret_ref, credential_configured, fingerprint,
                        conflict_json
                 FROM provider_import_candidates WHERE session_id = ?1
                 ORDER BY source_app_type ASC, id ASC",
            )
            .map_err(|error| AppError::Database(error.to_string()))?;
        let rows = stmt
            .query_map(params![session_id], |row| {
                Ok(ProviderImportCandidateRecord {
                    id: row.get(0)?,
                    session_id: row.get(1)?,
                    source_app_type: row.get(2)?,
                    source_provider_id: row.get(3)?,
                    source_locator: row.get(4)?,
                    normalized_json: row.get(5)?,
                    models_json: row.get(6)?,
                    temporary_secret_ref: row.get(7)?,
                    credential_configured: row.get(8)?,
                    fingerprint: row.get(9)?,
                    conflict_json: row.get(10)?,
                })
            })
            .map_err(|error| AppError::Database(error.to_string()))?;
        rows.collect::<Result<Vec<_>, _>>()
            .map_err(|error| AppError::Database(error.to_string()))
    }

    pub fn get_provider_import_candidate(
        &self,
        session_id: &str,
        candidate_id: &str,
    ) -> Result<Option<ProviderImportCandidateRecord>, AppError> {
        let conn = lock_conn!(self.conn);
        conn.query_row(
            "SELECT id, session_id, source_app_type, source_provider_id,
                    source_locator, normalized_json, models_json,
                    temporary_secret_ref, credential_configured, fingerprint,
                    conflict_json
             FROM provider_import_candidates
             WHERE session_id = ?1 AND id = ?2",
            params![session_id, candidate_id],
            |row| {
                Ok(ProviderImportCandidateRecord {
                    id: row.get(0)?,
                    session_id: row.get(1)?,
                    source_app_type: row.get(2)?,
                    source_provider_id: row.get(3)?,
                    source_locator: row.get(4)?,
                    normalized_json: row.get(5)?,
                    models_json: row.get(6)?,
                    temporary_secret_ref: row.get(7)?,
                    credential_configured: row.get(8)?,
                    fingerprint: row.get(9)?,
                    conflict_json: row.get(10)?,
                })
            },
        )
        .optional()
        .map_err(|error| AppError::Database(error.to_string()))
    }

    pub fn update_provider_import_candidate_state(
        &self,
        session_id: &str,
        candidate_id: &str,
        state_json: &str,
        completed_at: i64,
    ) -> Result<(), AppError> {
        let mut conn = lock_conn!(self.conn);
        let tx = conn
            .transaction()
            .map_err(|error| AppError::Database(error.to_string()))?;
        let changed = tx
            .execute(
                "UPDATE provider_import_candidates
                 SET conflict_json = ?3, temporary_secret_ref = NULL
                 WHERE id = ?1 AND session_id = ?2",
                params![candidate_id, session_id, state_json],
            )
            .map_err(|error| AppError::Database(error.to_string()))?;
        if changed == 0 {
            return Err(AppError::Message("导入候选不存在".to_string()));
        }
        let pending: i64 = tx
            .query_row(
                "SELECT COUNT(*) FROM provider_import_candidates
                 WHERE session_id = ?1
                   AND (conflict_json IS NULL
                        OR json_valid(conflict_json) = 0
                        OR (json_extract(conflict_json, '$.outcome') IS NULL
                            AND json_extract(conflict_json, '$.quarantine') IS NULL))",
                params![session_id],
                |row| row.get(0),
            )
            .map_err(|error| AppError::Database(error.to_string()))?;
        if pending == 0 {
            tx.execute(
                "UPDATE provider_import_sessions
                 SET state = 'completed', completed_at = ?2 WHERE id = ?1",
                params![session_id, completed_at],
            )
            .map_err(|error| AppError::Database(error.to_string()))?;
        }
        tx.commit()
            .map_err(|error| AppError::Database(error.to_string()))
    }

    pub fn complete_provider_import_candidate(
        &self,
        session_id: &str,
        candidate_id: &str,
        completed_at: i64,
    ) -> Result<(), AppError> {
        let mut conn = lock_conn!(self.conn);
        let tx = conn
            .transaction()
            .map_err(|error| AppError::Database(error.to_string()))?;
        let changed = tx
            .execute(
                "DELETE FROM provider_import_candidates WHERE id = ?1 AND session_id = ?2",
                params![candidate_id, session_id],
            )
            .map_err(|error| AppError::Database(error.to_string()))?;
        if changed == 0 {
            return Err(AppError::Message("导入候选不存在或已经处理".to_string()));
        }
        let remaining: i64 = tx
            .query_row(
                "SELECT COUNT(*) FROM provider_import_candidates WHERE session_id = ?1",
                params![session_id],
                |row| row.get(0),
            )
            .map_err(|error| AppError::Database(error.to_string()))?;
        if remaining == 0 {
            tx.execute(
                "UPDATE provider_import_sessions
                 SET state = 'completed', completed_at = ?2 WHERE id = ?1",
                params![session_id, completed_at],
            )
            .map_err(|error| AppError::Database(error.to_string()))?;
        }
        tx.commit()
            .map_err(|error| AppError::Database(error.to_string()))
    }

    pub fn expire_provider_import_sessions(&self, now: i64) -> Result<Vec<String>, AppError> {
        let mut conn = lock_conn!(self.conn);
        let tx = conn
            .transaction()
            .map_err(|error| AppError::Database(error.to_string()))?;
        let mut stmt = tx
            .prepare(
                "SELECT temporary_secret_ref FROM provider_import_candidates
                 WHERE temporary_secret_ref IS NOT NULL AND session_id IN (
                    SELECT id FROM provider_import_sessions
                    WHERE expires_at <= ?1 AND state NOT IN ('completed', 'expired')
                 )",
            )
            .map_err(|error| AppError::Database(error.to_string()))?;
        let refs = stmt
            .query_map(params![now], |row| row.get::<_, String>(0))
            .map_err(|error| AppError::Database(error.to_string()))?
            .collect::<Result<Vec<_>, _>>()
            .map_err(|error| AppError::Database(error.to_string()))?;
        drop(stmt);
        tx.execute(
            "UPDATE provider_import_sessions SET state = 'expired'
             WHERE expires_at <= ?1 AND state NOT IN ('completed', 'expired')",
            params![now],
        )
        .map_err(|error| AppError::Database(error.to_string()))?;
        tx.execute(
            "DELETE FROM provider_import_candidates WHERE session_id IN (
                SELECT id FROM provider_import_sessions WHERE state = 'expired'
             )",
            [],
        )
        .map_err(|error| AppError::Database(error.to_string()))?;
        tx.commit()
            .map_err(|error| AppError::Database(error.to_string()))?;
        Ok(refs)
    }

    pub fn provider_center_has_definitions(&self) -> Result<bool, AppError> {
        let conn = lock_conn!(self.conn);
        let count: i64 = conn
            .query_row("SELECT COUNT(*) FROM provider_definitions", [], |row| {
                row.get(0)
            })
            .map_err(|error| AppError::Database(error.to_string()))?;
        Ok(count > 0)
    }

    pub fn save_provider_center_core(
        &self,
        definitions: &[ProviderDefinition],
        bindings: &[ProviderBinding],
    ) -> Result<(), AppError> {
        let mut conn = lock_conn!(self.conn);
        let tx = conn
            .transaction()
            .map_err(|error| AppError::Database(error.to_string()))?;
        for definition in definitions {
            upsert_definition_tx(&tx, definition)?;
        }
        for binding in bindings {
            let revision = definitions
                .iter()
                .find(|definition| definition.id == binding.provider_id)
                .map(|definition| definition.revision as i64)
                .unwrap_or(1);
            upsert_binding_tx(&tx, binding, revision)?;
        }
        tx.commit()
            .map_err(|error| AppError::Database(format!("提交模型服务数据失败: {error}")))
    }

    pub fn load_provider_center_definitions(&self) -> Result<Vec<ProviderDefinition>, AppError> {
        let conn = lock_conn!(self.conn);
        let mut stmt = conn
            .prepare(
                "SELECT id, name, protocol, base_url, secret_ref, notes, enabled,
                        metadata_json, revision, created_at, updated_at
                 FROM provider_definitions ORDER BY created_at ASC, id ASC",
            )
            .map_err(|error| AppError::Database(error.to_string()))?;
        let rows = stmt
            .query_map([], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, String>(3)?,
                    row.get::<_, Option<String>>(4)?,
                    row.get::<_, Option<String>>(5)?,
                    row.get::<_, bool>(6)?,
                    row.get::<_, String>(7)?,
                    row.get::<_, i64>(8)?,
                    row.get::<_, i64>(9)?,
                    row.get::<_, i64>(10)?,
                ))
            })
            .map_err(|error| AppError::Database(error.to_string()))?;
        let mut definitions = Vec::new();
        for row in rows {
            let (
                id,
                name,
                protocol,
                base_url,
                secret_ref,
                notes,
                enabled,
                metadata_text,
                revision,
                created_at,
                updated_at,
            ) = row.map_err(|error| AppError::Database(error.to_string()))?;
            let metadata: Value = parse_json(&metadata_text, "Provider metadata")?;
            let mut model_stmt = conn
                .prepare(
                    "SELECT model_id FROM provider_models
                     WHERE provider_id = ?1 AND enabled = 1
                     ORDER BY sort_order ASC, model_id ASC",
                )
                .map_err(|error| AppError::Database(error.to_string()))?;
            let model_rows = model_stmt
                .query_map(params![id], |model_row| model_row.get::<_, String>(0))
                .map_err(|error| AppError::Database(error.to_string()))?;
            let models = model_rows
                .collect::<Result<Vec<_>, _>>()
                .map_err(|error| AppError::Database(error.to_string()))?;
            let source = conn
                .query_row(
                    "SELECT source_app_type, COALESCE(source_provider_id, source_locator), imported_at,
                            source_fingerprint, last_observed_at
                     FROM provider_sources WHERE provider_id = ?1
                     ORDER BY imported_at ASC LIMIT 1",
                    params![id],
                    |source_row| {
                        Ok(ProviderSource {
                            source_app: source_row.get(0)?,
                            source_ref: source_row.get(1)?,
                            imported_at: source_row.get(2)?,
                            source_fingerprint: source_row.get(3)?,
                            last_observed_at: source_row.get(4)?,
                        })
                    },
                )
                .optional()
                .map_err(|error| AppError::Database(error.to_string()))?;
            definitions.push(ProviderDefinition {
                id,
                name,
                protocol,
                base_url,
                models,
                discovered_models: metadata
                    .get("discoveredModels")
                    .cloned()
                    .map(serde_json::from_value)
                    .transpose()
                    .map_err(|error| AppError::Database(error.to_string()))?
                    .unwrap_or_default(),
                notes: notes.unwrap_or_default(),
                enabled,
                revision: revision.max(1) as u64,
                source,
                credential_configured: secret_ref.is_some(),
                credential_hint: None,
                last_discovery_at: metadata.get("lastDiscoveryAt").and_then(Value::as_i64),
                last_discovery_error: metadata
                    .get("lastDiscoveryError")
                    .and_then(Value::as_str)
                    .map(str::to_string),
                created_at,
                updated_at,
            });
        }
        Ok(definitions)
    }

    pub fn load_provider_center_bindings(&self) -> Result<Vec<ProviderBinding>, AppError> {
        let conn = lock_conn!(self.conn);
        let mut stmt = conn
            .prepare(
                "SELECT provider_id, app_type, state, overrides_json,
                        applied_revision, projection_digest,
                        last_error_message, last_apply_transaction_id, updated_at
                 FROM provider_bindings ORDER BY created_at ASC, id ASC",
            )
            .map_err(|error| AppError::Database(error.to_string()))?;
        let rows = stmt
            .query_map([], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, String>(3)?,
                    row.get::<_, Option<i64>>(4)?,
                    row.get::<_, Option<String>>(5)?,
                    row.get::<_, Option<String>>(6)?,
                    row.get::<_, Option<String>>(7)?,
                    row.get::<_, i64>(8)?,
                ))
            })
            .map_err(|error| AppError::Database(error.to_string()))?;
        let mut bindings = Vec::new();
        for row in rows {
            let (
                provider_id,
                app_type,
                status,
                overrides_text,
                applied_revision,
                expected_fingerprint,
                last_error,
                last_transaction_id,
                updated_at,
            ) = row.map_err(|error| AppError::Database(error.to_string()))?;
            let overrides: Value = parse_json(&overrides_text, "Binding overrides")?;
            bindings.push(ProviderBinding {
                provider_id,
                app_type,
                status,
                enabled: overrides
                    .get("enabled")
                    .and_then(Value::as_bool)
                    .unwrap_or(true),
                override_enabled: overrides
                    .get("overrideEnabled")
                    .and_then(Value::as_bool)
                    .unwrap_or(false),
                applied_revision: applied_revision.map(|value| value.max(0) as u64),
                expected_fingerprint,
                last_error,
                last_transaction_id,
                updated_at,
            });
        }
        Ok(bindings)
    }

    pub fn delete_provider_center_definition(&self, provider_id: &str) -> Result<(), AppError> {
        let mut conn = lock_conn!(self.conn);
        let tx = conn
            .transaction()
            .map_err(|error| AppError::Database(error.to_string()))?;
        tx.execute(
            "DELETE FROM provider_apply_transaction_targets
             WHERE binding_id IN (SELECT id FROM provider_bindings WHERE provider_id = ?1)",
            params![provider_id],
        )
        .map_err(|error| AppError::Database(error.to_string()))?;
        tx.execute(
            "DELETE FROM provider_bindings WHERE provider_id = ?1",
            params![provider_id],
        )
        .map_err(|error| AppError::Database(error.to_string()))?;
        let changed = tx
            .execute(
                "DELETE FROM provider_definitions WHERE id = ?1",
                params![provider_id],
            )
            .map_err(|error| AppError::Database(error.to_string()))?;
        if changed == 0 {
            return Err(AppError::Message("模型服务不存在".to_string()));
        }
        tx.commit()
            .map_err(|error| AppError::Database(error.to_string()))
    }

    pub fn upsert_provider_center_transaction(
        &self,
        transaction: &ProviderApplyTransaction,
    ) -> Result<(), AppError> {
        let mut conn = lock_conn!(self.conn);
        let tx = conn
            .transaction()
            .map_err(|error| AppError::Database(error.to_string()))?;
        tx.execute(
            "INSERT INTO provider_apply_transactions (
                id, provider_id, provider_revision, reason, state,
                idempotency_key, created_at, completed_at
             ) VALUES (?1, ?2, ?3, 'apply', ?4, ?1, ?5, ?6)
             ON CONFLICT(id) DO UPDATE SET
                state = excluded.state,
                completed_at = excluded.completed_at",
            params![
                transaction.id,
                transaction.provider_id,
                transaction.provider_revision as i64,
                transaction.status,
                transaction.created_at,
                transaction.completed_at,
            ],
        )
        .map_err(|error| AppError::Database(format!("保存应用事务失败: {error}")))?;
        for target in &transaction.targets {
            let id = binding_id(&transaction.provider_id, &target.app_type);
            let binding_exists: bool = tx
                .query_row(
                    "SELECT EXISTS(SELECT 1 FROM provider_bindings WHERE id = ?1)",
                    params![id],
                    |row| row.get(0),
                )
                .map_err(|error| AppError::Database(error.to_string()))?;
            if !binding_exists {
                continue;
            }
            tx.execute(
                "INSERT INTO provider_apply_transaction_targets (
                    transaction_id, binding_id, app_type, state, error_message
                 ) VALUES (?1, ?2, ?3, ?4, ?5)
                 ON CONFLICT(transaction_id, binding_id) DO UPDATE SET
                    state = excluded.state,
                    error_message = excluded.error_message",
                params![
                    transaction.id,
                    id,
                    target.app_type,
                    target.status,
                    target.message,
                ],
            )
            .map_err(|error| AppError::Database(format!("保存应用事务目标失败: {error}")))?;
        }
        tx.commit()
            .map_err(|error| AppError::Database(error.to_string()))
    }

    pub fn load_provider_center_transactions(
        &self,
    ) -> Result<Vec<ProviderApplyTransaction>, AppError> {
        let conn = lock_conn!(self.conn);
        let mut stmt = conn
            .prepare(
                "SELECT id, provider_id, provider_revision, state, created_at, completed_at
                 FROM provider_apply_transactions ORDER BY created_at DESC LIMIT 100",
            )
            .map_err(|error| AppError::Database(error.to_string()))?;
        let rows = stmt
            .query_map([], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, i64>(2)?,
                    row.get::<_, String>(3)?,
                    row.get::<_, i64>(4)?,
                    row.get::<_, Option<i64>>(5)?,
                ))
            })
            .map_err(|error| AppError::Database(error.to_string()))?;
        let mut transactions = Vec::new();
        for row in rows {
            let (id, provider_id, provider_revision, status, created_at, completed_at) =
                row.map_err(|error| AppError::Database(error.to_string()))?;
            let mut target_stmt = conn
                .prepare(
                    "SELECT app_type, state, error_message
                     FROM provider_apply_transaction_targets
                     WHERE transaction_id = ?1 ORDER BY app_type ASC",
                )
                .map_err(|error| AppError::Database(error.to_string()))?;
            let targets = target_stmt
                .query_map(params![id], |target_row| {
                    Ok(ProviderApplyTargetResult {
                        app_type: target_row.get(0)?,
                        status: target_row.get(1)?,
                        message: target_row.get(2)?,
                    })
                })
                .map_err(|error| AppError::Database(error.to_string()))?
                .collect::<Result<Vec<_>, _>>()
                .map_err(|error| AppError::Database(error.to_string()))?;
            transactions.push(ProviderApplyTransaction {
                id,
                provider_id,
                provider_revision: provider_revision.max(0) as u64,
                status,
                targets,
                created_at,
                completed_at,
            });
        }
        Ok(transactions)
    }

    pub fn get_provider_center_transaction(
        &self,
        transaction_id: &str,
    ) -> Result<Option<ProviderApplyTransaction>, AppError> {
        let conn = lock_conn!(self.conn);
        let header = conn
            .query_row(
                "SELECT id, provider_id, provider_revision, state, created_at, completed_at
                 FROM provider_apply_transactions WHERE id = ?1",
                params![transaction_id],
                |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, i64>(2)?,
                        row.get::<_, String>(3)?,
                        row.get::<_, i64>(4)?,
                        row.get::<_, Option<i64>>(5)?,
                    ))
                },
            )
            .optional()
            .map_err(|error| AppError::Database(error.to_string()))?;
        let Some((id, provider_id, revision, status, created_at, completed_at)) = header else {
            return Ok(None);
        };
        let mut stmt = conn
            .prepare(
                "SELECT app_type, state, error_message
                 FROM provider_apply_transaction_targets
                 WHERE transaction_id = ?1 ORDER BY app_type ASC",
            )
            .map_err(|error| AppError::Database(error.to_string()))?;
        let targets = stmt
            .query_map(params![id], |row| {
                Ok(ProviderApplyTargetResult {
                    app_type: row.get(0)?,
                    status: row.get(1)?,
                    message: row.get(2)?,
                })
            })
            .map_err(|error| AppError::Database(error.to_string()))?
            .collect::<Result<Vec<_>, _>>()
            .map_err(|error| AppError::Database(error.to_string()))?;
        Ok(Some(ProviderApplyTransaction {
            id,
            provider_id,
            provider_revision: revision.max(0) as u64,
            status,
            targets,
            created_at,
            completed_at,
        }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn provider_center_v19_round_trip_uses_normalized_tables() {
        let db = Database::memory().expect("memory database");
        let definition = ProviderDefinition {
            id: "shared-openai".to_string(),
            name: "Shared OpenAI".to_string(),
            protocol: "openai-chat".to_string(),
            base_url: "https://example.test/v1".to_string(),
            models: vec!["model-a".to_string(), "model-b".to_string()],
            discovered_models: vec!["model-c".to_string()],
            notes: "test".to_string(),
            enabled: true,
            revision: 3,
            source: Some(ProviderSource {
                source_app: "codex".to_string(),
                source_ref: "saved:codex:origin".to_string(),
                imported_at: 100,
                source_fingerprint: Some("fingerprint".to_string()),
                last_observed_at: Some(100),
            }),
            credential_configured: false,
            credential_hint: None,
            last_discovery_at: Some(200),
            last_discovery_error: None,
            created_at: 10,
            updated_at: 20,
        };
        let binding = ProviderBinding {
            provider_id: definition.id.clone(),
            app_type: "codex".to_string(),
            status: "pending".to_string(),
            enabled: true,
            override_enabled: false,
            applied_revision: None,
            expected_fingerprint: None,
            last_error: None,
            last_transaction_id: None,
            updated_at: 20,
        };
        db.save_provider_center_core(
            std::slice::from_ref(&definition),
            std::slice::from_ref(&binding),
        )
        .expect("save core");

        let loaded_definitions = db
            .load_provider_center_definitions()
            .expect("load definitions");
        let loaded_bindings = db.load_provider_center_bindings().expect("load bindings");
        assert_eq!(loaded_definitions.len(), 1);
        assert_eq!(loaded_definitions[0].models, definition.models);
        assert_eq!(
            loaded_definitions[0].discovered_models,
            definition.discovered_models
        );
        assert_eq!(
            loaded_definitions[0].source.as_ref().unwrap().source_ref,
            "saved:codex:origin"
        );
        assert_eq!(loaded_bindings.len(), 1);
        assert_eq!(loaded_bindings[0].provider_id, definition.id);

        let transaction = ProviderApplyTransaction {
            id: "tx-1".to_string(),
            provider_id: definition.id,
            provider_revision: 3,
            status: "applied".to_string(),
            targets: vec![ProviderApplyTargetResult {
                app_type: "codex".to_string(),
                status: "applied".to_string(),
                message: None,
            }],
            created_at: 30,
            completed_at: Some(40),
        };
        db.upsert_provider_center_transaction(&transaction)
            .expect("save transaction");
        let loaded_transactions = db
            .load_provider_center_transactions()
            .expect("load transactions");
        assert_eq!(loaded_transactions.len(), 1);
        assert_eq!(loaded_transactions[0].targets.len(), 1);
        assert_eq!(loaded_transactions[0].targets[0].app_type, "codex");

        let session = ProviderImportSessionRecord {
            id: "scan-1".to_string(),
            state: "ready".to_string(),
            requested_apps_json: "[]".to_string(),
            error_summary_json: "[]".to_string(),
            created_at: 50,
            expires_at: 100,
            completed_at: None,
        };
        let candidate = ProviderImportCandidateRecord {
            id: "candidate-1".to_string(),
            session_id: session.id.clone(),
            source_app_type: "codex".to_string(),
            source_provider_id: Some("saved:codex:origin".to_string()),
            source_locator: Some("saved:codex:origin".to_string()),
            normalized_json: "{}".to_string(),
            models_json: "[]".to_string(),
            temporary_secret_ref: Some("temporary-secret".to_string()),
            credential_configured: true,
            fingerprint: "fingerprint".to_string(),
            conflict_json: None,
        };
        db.save_provider_import_session(&session, std::slice::from_ref(&candidate))
            .expect("save import session");
        assert_eq!(
            db.list_provider_import_candidates(&session.id)
                .unwrap()
                .len(),
            1
        );
        let refs = db.expire_provider_import_sessions(101).unwrap();
        assert_eq!(refs, vec!["temporary-secret".to_string()]);
        assert!(db
            .list_provider_import_candidates(&session.id)
            .unwrap()
            .is_empty());
    }
}
