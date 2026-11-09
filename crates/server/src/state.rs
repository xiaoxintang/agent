use crate::{auth::hash_password, error::AppError};
use sqlx::SqlitePool;
use std::{
    collections::VecDeque,
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};
use tokio::sync::{Semaphore, broadcast, watch};

#[derive(Clone)]
pub struct AppState {
    pub pool: SqlitePool,
    pub(crate) security: Arc<Security>,
    pub agents_shutdown: watch::Sender<bool>,
    pub(crate) agent_events: broadcast::Sender<String>,
    pub(crate) agent_slots: Arc<Semaphore>,
    pub tls_enabled: bool,
}

pub(crate) struct Security {
    pub secure_cookie: bool,
    pub dummy_hash: String,
    pub password_slots: Arc<Semaphore>,
    attempts: Mutex<VecDeque<Instant>>,
}

impl AppState {
    pub async fn new(pool: SqlitePool, secure_cookie: bool) -> Result<Self, AppError> {
        let dummy_hash = hash_password(crate::auth::random_token()).await?;
        sqlx::query(
            "UPDATE nodes SET status = 'offline', connection_id = NULL WHERE revoked_at IS NULL",
        )
        .execute(&pool)
        .await?;
        Ok(Self {
            pool,
            agents_shutdown: watch::channel(false).0,
            agent_events: broadcast::channel(256).0,
            agent_slots: Arc::new(Semaphore::new(128)),
            tls_enabled: false,
            security: Arc::new(Security {
                secure_cookie,
                dummy_hash,
                password_slots: Arc::new(Semaphore::new(2)),
                attempts: Mutex::new(VecDeque::new()),
            }),
        })
    }

    pub(crate) fn wake_agent(&self, node_id: &str) {
        let _ = self.agent_events.send(node_id.to_owned());
    }

    pub async fn drain_agents(&self) {
        let _permits = self.agent_slots.acquire_many(128).await;
    }
}

impl Security {
    pub fn login_attempt(&self) -> Result<(), AppError> {
        let mut attempts = self.attempts.lock().map_err(|_| AppError::internal())?;
        let now = Instant::now();
        while attempts
            .front()
            .is_some_and(|time| now.duration_since(*time) >= Duration::from_secs(60))
        {
            attempts.pop_front();
        }
        if attempts.len() >= 10 {
            return Err(AppError::limited());
        }
        attempts.push_back(now);
        Ok(())
    }

    pub fn cookie_name(&self) -> &'static str {
        if self.secure_cookie {
            "__Host-panel_session"
        } else {
            "panel_session"
        }
    }

    pub fn cookie(&self, token: &str, max_age: i64) -> String {
        format!(
            "{}={token}; Path=/; HttpOnly; SameSite=Strict; Max-Age={max_age}{}",
            self.cookie_name(),
            if self.secure_cookie { "; Secure" } else { "" }
        )
    }
}
