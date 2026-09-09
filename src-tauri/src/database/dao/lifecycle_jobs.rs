//! Durable lifecycle job storage for CLI/Desktop component operations.

use crate::database::{lock_conn, Database};
use crate::error::AppError;
use rusqlite::{params, OptionalExtension};

#[derive(Debug, Clone)]
pub struct LifecycleJobRecord {
    pub id: String,
    pub app_id: String,
    pub component: String,
    pub action: String,
    pub state: String,
    pub plan_json: String,
    pub pre_probe_json: Option<String>,
    pub post_probe_json: Option<String>,
    pub error_code: Option<String>,
    pub error_message: Option<String>,
    pub created_at: i64,
    pub started_at: Option<i64>,
    pub completed_at: Option<i64>,
}

impl Database {
    pub fn save_lifecycle_job(&self, job: &LifecycleJobRecord) -> Result<(), AppError> {
        let conn = lock_conn!(self.conn);
        conn.execute(
            "INSERT INTO lifecycle_jobs (
                id, app_id, component, action, state, plan_json,
                pre_probe_json, post_probe_json, error_code, error_message,
                created_at, started_at, completed_at
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13)
             ON CONFLICT(id) DO UPDATE SET
                state = excluded.state,
                plan_json = excluded.plan_json,
                pre_probe_json = excluded.pre_probe_json,
                post_probe_json = excluded.post_probe_json,
                error_code = excluded.error_code,
                error_message = excluded.error_message,
                started_at = excluded.started_at,
                completed_at = excluded.completed_at",
            params![
                job.id,
                job.app_id,
                job.component,
                job.action,
                job.state,
                job.plan_json,
                job.pre_probe_json,
                job.post_probe_json,
                job.error_code,
                job.error_message,
                job.created_at,
                job.started_at,
                job.completed_at,
            ],
        )
        .map_err(|error| AppError::Database(format!("保存应用任务失败: {error}")))?;
        Ok(())
    }

    pub fn get_lifecycle_job(&self, id: &str) -> Result<Option<LifecycleJobRecord>, AppError> {
        let conn = lock_conn!(self.conn);
        conn.query_row(
            "SELECT id, app_id, component, action, state, plan_json,
                    pre_probe_json, post_probe_json, error_code, error_message,
                    created_at, started_at, completed_at
             FROM lifecycle_jobs WHERE id = ?1",
            params![id],
            |row| {
                Ok(LifecycleJobRecord {
                    id: row.get(0)?,
                    app_id: row.get(1)?,
                    component: row.get(2)?,
                    action: row.get(3)?,
                    state: row.get(4)?,
                    plan_json: row.get(5)?,
                    pre_probe_json: row.get(6)?,
                    post_probe_json: row.get(7)?,
                    error_code: row.get(8)?,
                    error_message: row.get(9)?,
                    created_at: row.get(10)?,
                    started_at: row.get(11)?,
                    completed_at: row.get(12)?,
                })
            },
        )
        .optional()
        .map_err(|error| AppError::Database(error.to_string()))
    }

    pub fn list_lifecycle_jobs(
        &self,
        app_id: Option<&str>,
        limit: usize,
    ) -> Result<Vec<LifecycleJobRecord>, AppError> {
        let conn = lock_conn!(self.conn);
        let safe_limit = limit.clamp(1, 100) as i64;
        let sql = if app_id.is_some() {
            "SELECT id, app_id, component, action, state, plan_json,
                    pre_probe_json, post_probe_json, error_code, error_message,
                    created_at, started_at, completed_at
             FROM lifecycle_jobs WHERE app_id = ?1
             ORDER BY created_at DESC LIMIT ?2"
        } else {
            "SELECT id, app_id, component, action, state, plan_json,
                    pre_probe_json, post_probe_json, error_code, error_message,
                    created_at, started_at, completed_at
             FROM lifecycle_jobs ORDER BY created_at DESC LIMIT ?2"
        };
        let mut stmt = conn
            .prepare(sql)
            .map_err(|error| AppError::Database(error.to_string()))?;
        let map_row = |row: &rusqlite::Row<'_>| {
            Ok(LifecycleJobRecord {
                id: row.get(0)?,
                app_id: row.get(1)?,
                component: row.get(2)?,
                action: row.get(3)?,
                state: row.get(4)?,
                plan_json: row.get(5)?,
                pre_probe_json: row.get(6)?,
                post_probe_json: row.get(7)?,
                error_code: row.get(8)?,
                error_message: row.get(9)?,
                created_at: row.get(10)?,
                started_at: row.get(11)?,
                completed_at: row.get(12)?,
            })
        };
        let rows = if let Some(app_id) = app_id {
            stmt.query_map(params![app_id, safe_limit], map_row)
        } else {
            stmt.query_map(params![rusqlite::types::Null, safe_limit], map_row)
        }
        .map_err(|error| AppError::Database(error.to_string()))?;
        rows.collect::<Result<Vec<_>, _>>()
            .map_err(|error| AppError::Database(error.to_string()))
    }

    pub fn interrupt_incomplete_lifecycle_jobs(
        &self,
        completed_at: i64,
    ) -> Result<usize, AppError> {
        let conn = lock_conn!(self.conn);
        conn.execute(
            "UPDATE lifecycle_jobs
             SET state = 'interrupted', completed_at = ?1,
                 error_code = 'JOB_INTERRUPTED',
                 error_message = '应用在任务完成前退出，请重新检测后重试'
             WHERE state IN ('queued', 'running', 'verifying', 'cancelling')",
            params![completed_at],
        )
        .map_err(|error| AppError::Database(error.to_string()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lifecycle_job_survives_updates_and_is_reconciled_after_restart() {
        let db = Database::memory().expect("memory database");
        let mut job = LifecycleJobRecord {
            id: "job-1".to_string(),
            app_id: "codex-desktop".to_string(),
            component: "desktop".to_string(),
            action: "install".to_string(),
            state: "running".to_string(),
            plan_json: "{}".to_string(),
            pre_probe_json: None,
            post_probe_json: None,
            error_code: None,
            error_message: None,
            created_at: 10,
            started_at: Some(11),
            completed_at: None,
        };
        db.save_lifecycle_job(&job).expect("insert job");
        job.state = "verifying".to_string();
        db.save_lifecycle_job(&job).expect("update job");
        assert_eq!(
            db.get_lifecycle_job("job-1")
                .expect("get job")
                .expect("job exists")
                .state,
            "verifying"
        );

        assert_eq!(db.interrupt_incomplete_lifecycle_jobs(99).unwrap(), 1);
        let interrupted = db.get_lifecycle_job("job-1").unwrap().expect("job exists");
        assert_eq!(interrupted.state, "interrupted");
        assert_eq!(interrupted.error_code.as_deref(), Some("JOB_INTERRUPTED"));
        assert_eq!(interrupted.completed_at, Some(99));
    }
}
