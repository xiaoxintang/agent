//! 保存配置快照与操作日志，区分“数据库已提交”和“运行时、文件均已同步”。
//! SQLite 事务只覆盖本地状态，不能与 Xray gRPC 组成一个原子事务。

use crate::{
    config::private_file,
    error::{Error, Result},
};
use rusqlite::{Connection, params};
use serde::Serialize;
use serde_json::Value;
use std::{
    path::Path,
    sync::Mutex,
    time::{SystemTime, UNIX_EPOCH},
};
use utoipa::ToSchema;

pub fn now() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs() as i64
}

#[derive(Clone, Debug, Serialize, ToSchema)]
pub struct Snapshot {
    // 修订号单调递增；候选快照也会分配修订号，因此已提交版本不保证连续。
    pub revision: i64,
    pub config: Value,
    /// Balancer tag -> runtime override target; not a native Xray JSON setting.
    pub overrides: Value,
}

#[derive(Clone, Debug, Serialize, ToSchema)]
pub struct Operation {
    pub id: String,
    pub kind: String,
    // 典型流程为 prepared → dispatched → committed → complete；异常时保留 uncertain。
    // 安装事务另有 switching/activated 阶段，恢复时必须按 kind 分开处理。
    pub phase: String,
    pub previous_revision: i64,
    // 纯版本切换不改变配置，所以没有候选修订号。
    pub candidate_revision: Option<i64>,
    pub details: Value,
    pub error: Option<String>,
    pub created_at: i64,
}

// 同步互斥锁仅保护短数据库操作，调用方不能持有连接锁跨越 gRPC 或子进程等待。
pub struct Store(Mutex<Connection>);

impl Store {
    pub fn open(path: &Path, initial: &Value) -> Result<Self> {
        let connection = Connection::open(path)?;
        private_file(path)?;
        connection.busy_timeout(std::time::Duration::from_secs(5))?;
        // snapshots 保留历史内容，state 指向权威修订，operations 记录可恢复的中间步骤。
        // WAL 配合 FULL 同步保证事务持久性，不等同于外部配置文件已经发布。
        connection.execute_batch(
            "PRAGMA journal_mode=WAL; PRAGMA synchronous=FULL; PRAGMA foreign_keys=ON;
            CREATE TABLE IF NOT EXISTS snapshots (
                revision INTEGER PRIMARY KEY AUTOINCREMENT,
                config TEXT NOT NULL, overrides TEXT NOT NULL, created_at INTEGER NOT NULL
            );
            CREATE TABLE IF NOT EXISTS state (
                singleton INTEGER PRIMARY KEY CHECK(singleton=1),
                revision INTEGER NOT NULL REFERENCES snapshots(revision),
                sync_state TEXT NOT NULL
            );
            CREATE TABLE IF NOT EXISTS operations (
                id TEXT PRIMARY KEY, kind TEXT NOT NULL, phase TEXT NOT NULL,
                previous_revision INTEGER NOT NULL, candidate_revision INTEGER,
                details TEXT NOT NULL, error TEXT, created_at INTEGER NOT NULL
            );
            PRAGMA user_version=1;",
        )?;
        if !connection.query_row("SELECT EXISTS(SELECT 1 FROM state)", [], |row| {
            row.get::<_, bool>(0)
        })? {
            let transaction = connection.unchecked_transaction()?;
            transaction.execute(
                "INSERT INTO snapshots(config,overrides,created_at) VALUES(?1,'{}',?2)",
                params![initial.to_string(), now()],
            )?;
            let revision = transaction.last_insert_rowid();
            transaction.execute("INSERT INTO state VALUES(1,?1,'in_sync')", [revision])?;
            transaction.commit()?;
        }
        Ok(Self(Mutex::new(connection)))
    }

    fn connection(&self) -> Result<std::sync::MutexGuard<'_, Connection>> {
        self.0
            .lock()
            .map_err(|_| Error::Internal("database lock poisoned".into()))
    }

    pub fn committed(&self) -> Result<Snapshot> {
        // 启动辅助命令只能使用 state 指向的快照，不能按最大 revision 读取未提交候选。
        let connection = self.connection()?;
        Self::read_snapshot(&connection, None)
    }

    pub fn snapshot(&self, revision: i64) -> Result<Snapshot> {
        let connection = self.connection()?;
        Self::read_snapshot(&connection, Some(revision))
    }

    fn read_snapshot(connection: &Connection, revision: Option<i64>) -> Result<Snapshot> {
        let (revision, config, overrides): (i64, String, String) = connection.query_row(
            "SELECT revision,config,overrides FROM snapshots WHERE revision=COALESCE(?1,(SELECT revision FROM state WHERE singleton=1))",
            [revision], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )?;
        Ok(Snapshot {
            revision,
            config: serde_json::from_str(&config)?,
            overrides: serde_json::from_str(&overrides)?,
        })
    }

    pub fn sync_state(&self) -> Result<String> {
        Ok(self.connection()?.query_row(
            "SELECT sync_state FROM state WHERE singleton=1",
            [],
            |row| row.get(0),
        )?)
    }

    pub fn ensure_consistent(&self) -> Result<()> {
        // 同时检查同步标记和操作日志，防止遗留事务被一次新的变更覆盖。
        if self.sync_state()? != "in_sync" || !self.pending()?.is_empty() {
            return Err(Error::Conflict("an unfinished operation needs reconciliation; inspect /status and explicitly restart to restore committed configuration".into()));
        }
        Ok(())
    }

    pub fn begin(
        &self,
        kind: &str,
        candidate: Option<(&Value, &Value)>,
        details: Value,
    ) -> Result<Operation> {
        // 候选快照和恢复记录一起提交，但此时不移动 state 的权威配置指针。
        let mut connection = self.connection()?;
        let transaction = connection.transaction()?;
        let previous_revision: i64 =
            transaction.query_row("SELECT revision FROM state WHERE singleton=1", [], |row| {
                row.get(0)
            })?;
        let candidate_revision = if let Some((config, overrides)) = candidate {
            transaction.execute(
                "INSERT INTO snapshots(config,overrides,created_at) VALUES(?1,?2,?3)",
                params![config.to_string(), overrides.to_string(), now()],
            )?;
            Some(transaction.last_insert_rowid())
        } else {
            None
        };
        let operation = Operation {
            id: uuid::Uuid::new_v4().to_string(),
            kind: kind.to_owned(),
            phase: "prepared".into(),
            previous_revision,
            candidate_revision,
            details,
            error: None,
            created_at: now(),
        };
        transaction.execute(
            "INSERT INTO operations VALUES(?1,?2,?3,?4,?5,?6,NULL,?7)",
            params![
                operation.id,
                operation.kind,
                operation.phase,
                operation.previous_revision,
                operation.candidate_revision,
                operation.details.to_string(),
                operation.created_at
            ],
        )?;
        transaction.commit()?;
        Ok(operation)
    }

    pub fn phase(&self, id: &str, phase: &str) -> Result<()> {
        let changed = self.connection()?.execute(
            "UPDATE operations SET phase=?2 WHERE id=?1",
            params![id, phase],
        )?;
        if changed != 1 {
            return Err(Error::Internal("operation no longer exists".into()));
        }
        Ok(())
    }

    pub fn details(&self, id: &str, details: &Value) -> Result<()> {
        self.connection()?.execute(
            "UPDATE operations SET details=?2 WHERE id=?1",
            params![id, details.to_string()],
        )?;
        Ok(())
    }

    /// The caller must publish the committed snapshot before marking the operation complete.
    pub fn commit_candidate(&self, operation: &Operation) -> Result<()> {
        let revision = operation
            .candidate_revision
            .ok_or_else(|| Error::Internal("operation has no candidate".into()))?;
        let mut connection = self.connection()?;
        let transaction = connection.transaction()?;
        // gRPC 已成功，先提交权威配置；publication_pending 阻止在文件发布前继续写入。
        transaction.execute(
            "UPDATE state SET revision=?1,sync_state='publication_pending' WHERE singleton=1",
            [revision],
        )?;
        transaction.execute(
            "UPDATE operations SET phase='committed' WHERE id=?1",
            [&operation.id],
        )?;
        transaction.commit()?;
        Ok(())
    }

    pub fn complete(&self, id: &str) -> Result<()> {
        // 仅在外部副作用已完成后调用：配置文件发布或版本激活与清理已经结束。
        let mut connection = self.connection()?;
        let transaction = connection.transaction()?;
        transaction.execute(
            "UPDATE operations SET phase='complete',error=NULL WHERE id=?1",
            [id],
        )?;
        transaction.execute(
            "UPDATE state SET sync_state='in_sync' WHERE singleton=1",
            [],
        )?;
        transaction.commit()?;
        Ok(())
    }

    pub fn fail(&self, id: &str, error: &str, uncertain: bool) -> Result<()> {
        let mut connection = self.connection()?;
        let transaction = connection.transaction()?;
        transaction.execute(
            "UPDATE operations SET phase=?2,error=?3 WHERE id=?1",
            params![id, if uncertain { "uncertain" } else { "failed" }, error],
        )?;
        if uncertain {
            // “不知道是否生效”必须阻塞后续配置写入，不能当作可直接重试的普通失败。
            transaction.execute(
                "UPDATE state SET sync_state='needs_confirmation' WHERE singleton=1",
                [],
            )?;
        } else {
            transaction.execute("UPDATE state SET sync_state='in_sync' WHERE singleton=1 AND NOT EXISTS(SELECT 1 FROM operations WHERE phase NOT IN ('complete','failed','reconciled'))", [])?;
        }
        transaction.commit()?;
        Ok(())
    }

    pub fn restore_revision(&self, revision: i64) -> Result<()> {
        // 切回旧快照不代表旧运行时已恢复，重启和健康检查完成前仍需保持待确认。
        self.connection()?.execute(
            "UPDATE state SET revision=?1,sync_state='needs_confirmation' WHERE singleton=1",
            [revision],
        )?;
        Ok(())
    }

    /// Called only after an explicit restart has loaded the committed snapshot and overrides.
    pub fn reconciled(&self) -> Result<()> {
        let mut connection = self.connection()?;
        let transaction = connection.transaction()?;
        transaction.execute("UPDATE operations SET phase='reconciled' WHERE phase NOT IN ('complete','failed','reconciled')", [])?;
        transaction.execute(
            "UPDATE state SET sync_state='in_sync' WHERE singleton=1",
            [],
        )?;
        transaction.commit()?;
        Ok(())
    }

    pub fn pending(&self) -> Result<Vec<Operation>> {
        // failed/reconciled 仍保留在历史中，但不再参与恢复和版本删除保护。
        let connection = self.connection()?;
        let mut query = connection.prepare("SELECT id,kind,phase,previous_revision,candidate_revision,details,error,created_at FROM operations WHERE phase NOT IN ('complete','failed','reconciled') ORDER BY created_at,id")?;
        let rows = query.query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, i64>(3)?,
                row.get::<_, Option<i64>>(4)?,
                row.get::<_, String>(5)?,
                row.get::<_, Option<String>>(6)?,
                row.get::<_, i64>(7)?,
            ))
        })?;
        rows.map(|row| {
            let (
                id,
                kind,
                phase,
                previous_revision,
                candidate_revision,
                details,
                error,
                created_at,
            ) = row?;
            Ok(Operation {
                id,
                kind,
                phase,
                previous_revision,
                candidate_revision,
                details: serde_json::from_str(&details)?,
                error,
                created_at,
            })
        })
        .collect()
    }

    pub fn recover_mutations(&self) -> Result<()> {
        for operation in self.pending()? {
            if operation.kind == "install" || operation.kind == "replace_config" {
                // 这些操作包含显式重启及回滚，由 App 的专用恢复路径处理。
                continue;
            }
            match operation.phase.as_str() {
                "prepared" => self.fail(&operation.id, "agent stopped before dispatch", false)?,
                "committed" => {} // Publication is recoverable from the authoritative snapshot.
                _ => self.fail(
                    &operation.id,
                    "agent stopped after dispatch; runtime outcome needs confirmation",
                    true,
                )?,
            }
        }
        Ok(())
    }

    pub fn has_history(&self) -> Result<bool> {
        Ok(self
            .connection()?
            .query_row("SELECT EXISTS(SELECT 1 FROM operations)", [], |row| {
                row.get(0)
            })?)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn pending_candidate_does_not_replace_committed_configuration() {
        let temp = tempfile::tempdir().unwrap();
        let store = Store::open(&temp.path().join("state.db"), &json!({"inbounds":[]})).unwrap();
        let original = store.committed().unwrap();
        let operation = store
            .begin(
                "mutation",
                Some((&json!({"inbounds":[{"tag":"new"}]}), &json!({}))),
                json!({}),
            )
            .unwrap();
        store.phase(&operation.id, "dispatched").unwrap();
        store.recover_mutations().unwrap();
        assert_eq!(store.committed().unwrap().revision, original.revision);
        assert!(store.ensure_consistent().is_err());
        store.reconciled().unwrap();
        assert!(store.ensure_consistent().is_ok());
    }

    #[test]
    fn committed_snapshot_survives_reopening_before_file_publication() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("state.db");
        let candidate = json!({"inbounds":[{"tag":"persisted"}]});
        {
            let store = Store::open(&path, &json!({})).unwrap();
            let op = store
                .begin(
                    "mutation",
                    Some((&candidate, &json!({"balance":"direct"}))),
                    json!({}),
                )
                .unwrap();
            store.commit_candidate(&op).unwrap();
        }
        let store = Store::open(&path, &json!({})).unwrap();
        assert_eq!(store.committed().unwrap().config, candidate);
        assert_eq!(store.sync_state().unwrap(), "publication_pending");
    }
}
