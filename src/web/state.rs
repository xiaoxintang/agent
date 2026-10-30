use super::{
    Invoke,
    db::{Agent, Db},
};
use crate::{
    error::{Error, Result},
    web_config::LocalAgent,
};
use serde_json::Value;
use std::{
    collections::HashMap,
    path::PathBuf,
    sync::Arc,
    time::{Duration, Instant},
};
use tokio::sync::Mutex;

pub struct Web {
    pub db: Db,
    pub client: reqwest::Client,
    pub credentials: PathBuf,
    pub local_agent: PathBuf,
    pub secure_cookie: bool,
    pub locks: Mutex<HashMap<String, Arc<Mutex<()>>>>,
    pub(super) login_attempts: Mutex<(Instant, u32)>,
}

impl Web {
    pub fn agent(&self, id: &str) -> Result<Agent> {
        let mut a = self.db.agent(id)?;
        if id == "local" {
            let local: LocalAgent = serde_json::from_slice(&std::fs::read(&self.local_agent)?)?;
            a.url = local.url;
            a.token = local.token;
        }
        Ok(a)
    }
    pub async fn lock(&self, id: &str) -> Arc<Mutex<()>> {
        self.locks
            .lock()
            .await
            .entry(id.into())
            .or_insert_with(|| Arc::new(Mutex::new(())))
            .clone()
    }
    pub async fn request(&self, id: &str, r: &Invoke) -> Result<reqwest::Response> {
        let a = self.agent(id)?;
        let url = format!("{}{}", a.url.trim_end_matches('/'), r.path);
        let mut request = self
            .client
            .request(
                r.method
                    .parse()
                    .map_err(|_| Error::Validation("invalid HTTP method".into()))?,
                url,
            )
            .bearer_auth(a.token)
            .query(&r.query);
        if !r.body.is_null() {
            request = request.json(&r.body);
        }
        request
            .timeout(Duration::from_secs(if r.method == "GET" {
                20
            } else {
                600
            }))
            .send()
            .await
            .map_err(|_| {
                Error::Unavailable(
                    "agent request failed or timed out; inspect task state before retrying".into(),
                )
            })
    }
    pub async fn read(&self, id: &str, path: &str) -> Result<Value> {
        let r = Invoke {
            method: "GET".into(),
            path: path.into(),
            query: Default::default(),
            body: Value::Null,
            confirmation: None,
            expected_revision: None,
        };
        let response = self.request(id, &r).await?;
        let status = response.status();
        let body = response
            .json::<Value>()
            .await
            .map_err(|_| Error::Unavailable("agent returned an invalid JSON response".into()))?;
        if !status.is_success() {
            return Err(if status.as_u16() == 401 {
                Error::Validation("agent authentication failed; update its token".into())
            } else {
                Error::Unavailable(format!(
                    "agent returned {status}: {}",
                    body.get("error")
                        .and_then(Value::as_str)
                        .unwrap_or("request failed")
                ))
            });
        }
        Ok(body)
    }
}
