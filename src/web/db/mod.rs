use crate::error::{Error, Result};
use rusqlite::Connection;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::{
    path::Path,
    sync::{Mutex, MutexGuard},
};

pub struct Db(Mutex<Connection>);
#[derive(Clone, Serialize, Deserialize)]
pub struct Agent {
    pub id: String,
    pub name: String,
    pub url: String,
    #[serde(skip_serializing)]
    pub token: String,
}
#[derive(Clone, Serialize, Deserialize)]
pub struct Assignment {
    pub id: String,
    pub user_id: String,
    pub agent_id: String,
    pub inbound_tag: String,
    pub native_user: Value,
    pub desired_present: bool,
    pub generation: i64,
    pub status: String,
}

mod agents;
mod assignments;
mod queue;
mod tasks;
#[cfg(test)]
mod tests;
mod users;

impl Db {
    pub fn open(path: &Path) -> Result<Self> {
        let c = Connection::open(path)?;
        crate::config::private_file(path)?;
        c.busy_timeout(std::time::Duration::from_secs(5))?;
        c.execute_batch("PRAGMA journal_mode=WAL; PRAGMA synchronous=FULL; PRAGMA foreign_keys=ON;
        CREATE TABLE IF NOT EXISTS agents(id TEXT PRIMARY KEY,name TEXT NOT NULL,url TEXT NOT NULL,token TEXT NOT NULL);
        CREATE TABLE IF NOT EXISTS users(id TEXT PRIMARY KEY,name TEXT NOT NULL,status TEXT NOT NULL DEFAULT 'active');
        CREATE TABLE IF NOT EXISTS assignments(id TEXT PRIMARY KEY,user_id TEXT NOT NULL REFERENCES users(id),agent_id TEXT NOT NULL REFERENCES agents(id),inbound_tag TEXT NOT NULL,native_user TEXT NOT NULL,desired_present INTEGER NOT NULL,generation INTEGER NOT NULL,status TEXT NOT NULL,UNIQUE(user_id,agent_id,inbound_tag));
        CREATE TABLE IF NOT EXISTS tasks(id TEXT PRIMARY KEY,agent_id TEXT NOT NULL REFERENCES agents(id),request TEXT NOT NULL,effects TEXT NOT NULL,status TEXT NOT NULL,attempts INTEGER NOT NULL DEFAULT 0,next_attempt INTEGER NOT NULL DEFAULT 0,error TEXT,result TEXT,uncertain INTEGER NOT NULL DEFAULT 0,created_at INTEGER NOT NULL);
        CREATE TABLE IF NOT EXISTS sessions(token_hash TEXT PRIMARY KEY,epoch TEXT NOT NULL,expires INTEGER NOT NULL);
        CREATE INDEX IF NOT EXISTS task_due ON tasks(status,next_attempt);
        PRAGMA user_version=1;")?;
        // A terminated worker must reconcile, never blindly repeat its last write.
        c.execute(
            "UPDATE tasks SET status='retry',uncertain=1 WHERE status='running'",
            [],
        )?;
        Ok(Self(Mutex::new(c)))
    }
    pub fn conn(&self) -> Result<MutexGuard<'_, Connection>> {
        self.0
            .lock()
            .map_err(|_| Error::Internal("Web database lock poisoned".into()))
    }
}
