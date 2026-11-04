use crate::AgentError;
use panel_common::{TaskCommand, TaskOutcome, storage};
use sha2::{Digest, Sha256};
use sqlx::{
    SqlitePool,
    sqlite::{SqliteConnectOptions, SqliteJournalMode, SqlitePoolOptions},
};
use std::{fs::File, path::Path, time::Duration};

pub struct Store {
    pool: SqlitePool,
    _lock: File,
}
// No Debug: local credentials are intentionally not printable.
#[derive(sqlx::FromRow)]
pub struct Identity {
    pub server_url: String,
    pub node_id: String,
    pub credential: String,
}

impl Store {
    pub async fn open(directory: &Path) -> Result<Self, AgentError> {
        storage::private_directory(directory).map_err(|_| AgentError::Configuration)?;
        let lock = storage::private_file(&directory.join("agent.lock"))
            .map_err(|_| AgentError::Storage)?;
        lock.try_lock().map_err(|_| AgentError::Storage)?;
        let filename = directory.join("agent.sqlite3");
        storage::private_file(&filename).map_err(|_| AgentError::Storage)?;
        let pool = SqlitePoolOptions::new()
            .max_connections(1)
            .connect_with(
                SqliteConnectOptions::new()
                    .filename(filename)
                    .foreign_keys(true)
                    .journal_mode(SqliteJournalMode::Wal)
                    .busy_timeout(Duration::from_secs(5)),
            )
            .await?;
        sqlx::raw_sql("CREATE TABLE IF NOT EXISTS identity (singleton INTEGER PRIMARY KEY CHECK(singleton = 1), server_url TEXT NOT NULL, node_id TEXT NOT NULL, credential TEXT NOT NULL); CREATE TABLE IF NOT EXISTS receipts (task_id TEXT PRIMARY KEY NOT NULL, payload_hash TEXT NOT NULL, deadline_at INTEGER NOT NULL, result_json TEXT); CREATE INDEX IF NOT EXISTS receipt_retention ON receipts(deadline_at);").execute(&pool).await?;
        sqlx::query("DELETE FROM receipts WHERE deadline_at < ?")
            .bind(now() - 86400)
            .execute(&pool)
            .await?;
        Ok(Self { pool, _lock: lock })
    }
    pub async fn identity(&self) -> Result<Option<Identity>, AgentError> {
        Ok(sqlx::query_as(
            "SELECT server_url, node_id, credential FROM identity WHERE singleton = 1",
        )
        .fetch_optional(&self.pool)
        .await?)
    }
    pub async fn save_identity(
        &self,
        server_url: &str,
        node_id: &str,
        credential: &str,
    ) -> Result<(), AgentError> {
        if credential.len() != 64 || !credential.bytes().all(|byte| byte.is_ascii_hexdigit()) {
            return Err(AgentError::Protocol);
        }
        let changed = sqlx::query("INSERT INTO identity (singleton, server_url, node_id, credential) VALUES (1, ?, ?, ?) ON CONFLICT(singleton) DO UPDATE SET credential = excluded.credential WHERE identity.server_url = excluded.server_url AND identity.node_id = excluded.node_id")
            .bind(server_url).bind(node_id).bind(credential).execute(&self.pool).await?;
        if changed.rows_affected() != 1 {
            return Err(AgentError::Configuration);
        }
        Ok(())
    }
    pub async fn execute(
        &self,
        task: &TaskCommand,
        effect: impl FnOnce() -> TaskOutcome,
    ) -> Result<TaskOutcome, AgentError> {
        self.execute_async(task, || std::future::ready(effect()))
            .await
    }
    pub async fn execute_async<F, Fut>(
        &self,
        task: &TaskCommand,
        effect: F,
    ) -> Result<TaskOutcome, AgentError>
    where
        F: FnOnce() -> Fut,
        Fut: std::future::Future<Output = TaskOutcome>,
    {
        if uuid::Uuid::parse_str(&task.id).is_err() || task.deadline_at > now() + 300 {
            return Err(AgentError::Protocol);
        }
        let payload = serde_json::to_vec(task).map_err(|_| AgentError::Protocol)?;
        let hash = hex::encode(Sha256::digest(payload));
        let mut tx = self.pool.begin().await?;
        let existing = sqlx::query_as::<_, (String, Option<String>)>(
            "SELECT payload_hash, result_json FROM receipts WHERE task_id = ?",
        )
        .bind(&task.id)
        .fetch_optional(&mut *tx)
        .await?;
        if let Some((old_hash, result)) = existing {
            if old_hash != hash {
                return Err(AgentError::Protocol);
            }
            if let Some(result) = result {
                return serde_json::from_str(&result).map_err(|_| AgentError::Storage);
            }
            let result = TaskOutcome::Interrupted;
            sqlx::query("UPDATE receipts SET result_json = ? WHERE task_id = ?")
                .bind(serde_json::to_string(&result).map_err(|_| AgentError::Storage)?)
                .bind(&task.id)
                .execute(&mut *tx)
                .await?;
            tx.commit().await?;
            return Ok(result);
        }
        sqlx::query("INSERT INTO receipts (task_id, payload_hash, deadline_at) VALUES (?, ?, ?)")
            .bind(&task.id)
            .bind(hash)
            .bind(task.deadline_at)
            .execute(&mut *tx)
            .await?;
        tx.commit().await?;
        let result = if task.deadline_at <= now() {
            TaskOutcome::Expired
        } else {
            effect().await
        };
        sqlx::query("UPDATE receipts SET result_json = ? WHERE task_id = ?")
            .bind(serde_json::to_string(&result).map_err(|_| AgentError::Storage)?)
            .bind(&task.id)
            .execute(&self.pool)
            .await?;
        sqlx::query("DELETE FROM receipts WHERE deadline_at < ?")
            .bind(now() - 86400)
            .execute(&self.pool)
            .await?;
        Ok(result)
    }
    pub async fn close(&self) {
        self.pool.close().await;
    }
}

fn now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("UTC clock")
        .as_secs() as i64
}

#[cfg(test)]
mod tests {
    use super::*;
    use panel_common::{AgentInfo, TaskOperation};
    use std::sync::atomic::{AtomicUsize, Ordering};
    #[tokio::test]
    async fn duplicate_task_and_restart_do_not_repeat_a_test_side_effect() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("data");
        let store = Store::open(&path).await.unwrap();
        assert!(Store::open(&path).await.is_err());
        let task = TaskCommand {
            id: uuid::Uuid::new_v4().to_string(),
            operation: TaskOperation::AgentStatus,
            deadline_at: now() + 60,
            private_config: None,
        };
        let counter = AtomicUsize::new(0);
        // Explicitly test-only side effect; production executes AgentStatus only.
        let expected = store
            .execute(&task, || {
                counter.fetch_add(1, Ordering::SeqCst);
                TaskOutcome::Succeeded(AgentInfo::current())
            })
            .await
            .unwrap();
        assert_eq!(
            store
                .execute(&task, || panic!("duplicate executed"))
                .await
                .unwrap(),
            expected
        );
        store.close().await;
        drop(store);
        let store = Store::open(&path).await.unwrap();
        assert_eq!(
            store
                .execute(&task, || panic!("restart executed duplicate"))
                .await
                .unwrap(),
            expected
        );
        assert_eq!(counter.load(Ordering::SeqCst), 1);
        let mut changed = task.clone();
        changed.deadline_at += 1;
        assert_eq!(
            store
                .execute(&changed, || panic!("conflicting task executed"))
                .await
                .unwrap_err(),
            AgentError::Protocol
        );
        store.close().await;
    }
    #[tokio::test]
    async fn expired_and_interrupted_receipts_are_never_executed() {
        let directory = tempfile::tempdir().unwrap();
        let store = Store::open(&directory.path().join("data")).await.unwrap();
        let mut task = TaskCommand {
            id: uuid::Uuid::new_v4().to_string(),
            operation: TaskOperation::AgentStatus,
            deadline_at: now() - 1,
            private_config: None,
        };
        assert_eq!(
            store
                .execute(&task, || panic!("expired executed"))
                .await
                .unwrap(),
            TaskOutcome::Expired
        );
        task.id = uuid::Uuid::new_v4().to_string();
        task.deadline_at = now() + 60;
        let hash = hex::encode(Sha256::digest(serde_json::to_vec(&task).unwrap()));
        sqlx::query("INSERT INTO receipts VALUES (?, ?, ?, NULL)")
            .bind(&task.id)
            .bind(hash)
            .bind(task.deadline_at)
            .execute(&store.pool)
            .await
            .unwrap();
        assert_eq!(
            store
                .execute(&task, || panic!("interrupted executed"))
                .await
                .unwrap(),
            TaskOutcome::Interrupted
        );
        store.close().await;
    }
}
