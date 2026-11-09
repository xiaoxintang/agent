use crate::error::AppError;
use panel_common::AuditMetadata;
use sqlx::{
    Sqlite, SqlitePool, Transaction,
    migrate::Migrator,
    sqlite::{SqliteConnectOptions, SqliteJournalMode, SqlitePoolOptions},
};
use std::{
    error::Error,
    path::Path,
    time::{Duration, SystemTime, UNIX_EPOCH},
};

pub static MIGRATOR: Migrator = sqlx::migrate!("./migrations");
pub const AUDIT_RETENTION: i64 = 90 * 24 * 60 * 60;

pub fn now() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("Clock must be after Unix epoch")
        .as_secs() as i64
}

pub async fn open(directory: &Path) -> Result<SqlitePool, Box<dyn Error>> {
    panel_common::storage::private_directory(directory)?;
    let filename = directory.join("panel.sqlite3");
    panel_common::storage::private_file(&filename)?;
    let pool = SqlitePoolOptions::new()
        .max_connections(4)
        .connect_with(
            SqliteConnectOptions::new()
                .filename(filename)
                .foreign_keys(true)
                .journal_mode(SqliteJournalMode::Wal)
                .busy_timeout(Duration::from_secs(5)),
        )
        .await?;
    MIGRATOR.run(&pool).await?;
    cleanup(&pool).await?;
    Ok(pool)
}

pub async fn cleanup(pool: &SqlitePool) -> Result<(), sqlx::Error> {
    let mut tx = pool.begin().await?;
    sqlx::query("DELETE FROM sessions WHERE expires_at <= ? OR revoked_at IS NOT NULL")
        .bind(now())
        .execute(&mut *tx)
        .await?;
    sqlx::query("DELETE FROM audit_logs WHERE created_at < ?")
        .bind(now() - AUDIT_RETENTION)
        .execute(&mut *tx)
        .await?;
    sqlx::query("UPDATE tasks SET status = 'expired', finished_at = ?, error_code = 'deadline_exceeded' WHERE deadline_at <= ? AND status IN ('pending','dispatched','acked')").bind(now()).bind(now()).execute(&mut *tx).await?;
    sqlx::query("UPDATE config_revisions SET status='failed' WHERE status='pending' AND task_id IN (SELECT id FROM tasks WHERE status IN ('expired','canceled','failed'))").execute(&mut *tx).await?;
    sqlx::query("DELETE FROM tasks WHERE deadline_at < ? AND status IN ('succeeded','failed','expired','canceled') AND id NOT IN (SELECT task_id FROM config_revisions r WHERE task_id IS NOT NULL AND revision=(SELECT MAX(revision) FROM config_revisions WHERE node_id=r.node_id))").bind(now() - 7 * 86400).execute(&mut *tx).await?;
    sqlx::query("DELETE FROM enrollments WHERE expires_at < ?")
        .bind(now() - 86400)
        .execute(&mut *tx)
        .await?;
    sqlx::query("DELETE FROM traffic_aggregates WHERE bucket_start < ?")
        .bind(now() - 7 * 86400)
        .execute(&mut *tx)
        .await?;
    tx.commit().await
}

pub async fn maintain(pool: SqlitePool, mut shutdown: tokio::sync::watch::Receiver<bool>) {
    let mut tick = tokio::time::interval(Duration::from_secs(60));
    tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    tick.tick().await;
    loop {
        if *shutdown.borrow() {
            return;
        }
        tokio::select! {
            _ = shutdown.changed() => return,
            _ = tick.tick() => if let Err(error) = cleanup(&pool).await {
                crate::diagnostics::report("maintenance_error", &error.into(), None, None, None);
            },
        }
    }
}

pub async fn audit(
    tx: &mut Transaction<'_, Sqlite>,
    actor_id: Option<&str>,
    node_id: Option<&str>,
    action: &str,
    target: &str,
    result: &str,
    request_id: Option<&str>,
) -> Result<(), AppError> {
    let metadata = serde_json::to_string(&AuditMetadata {
        request_id: request_id.map(str::to_owned),
    })
    .map_err(|_| AppError::internal())?;
    sqlx::query("INSERT INTO audit_logs (actor_id, node_id, action, target, result, created_at, metadata_json) VALUES (?, ?, ?, ?, ?, ?, ?)")
        .bind(actor_id).bind(node_id).bind(action).bind(target).bind(result).bind(now()).bind(metadata).execute(&mut **tx).await?;
    Ok(())
}
