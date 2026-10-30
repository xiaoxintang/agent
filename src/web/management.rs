use super::{Web, db::Agent};
use crate::error::{Error, Result};
use axum::{
    Json,
    extract::{Path, State},
};
use serde_json::{Value, json};
use std::sync::Arc;

pub(super) async fn agents(State(web): State<Arc<Web>>) -> Result<Json<Vec<Agent>>> {
    Ok(Json(web.db.agents()?))
}
pub(super) async fn save_agent(
    State(web): State<Arc<Web>>,
    Json(v): Json<Value>,
) -> Result<Json<Value>> {
    let id = v["id"]
        .as_str()
        .map(str::to_owned)
        .unwrap_or_else(|| uuid::Uuid::new_v4().to_string());
    let url = v["url"].as_str().unwrap_or_default();
    let parsed =
        reqwest::Url::parse(url).map_err(|_| Error::Validation("invalid agent URL".into()))?;
    if !matches!(parsed.scheme(), "http" | "https")
        || parsed.host_str().is_none()
        || !parsed.username().is_empty()
        || parsed.password().is_some()
        || parsed.query().is_some()
        || parsed.fragment().is_some()
        || parsed.path() != "/"
    {
        return Err(Error::Validation(
            "agent URL must be an HTTP(S) origin without credentials or path".into(),
        ));
    }
    let token = v["token"]
        .as_str()
        .filter(|s| !s.is_empty())
        .map(str::to_owned)
        .or_else(|| web.db.agent(&id).ok().map(|a| a.token))
        .ok_or_else(|| Error::Validation("agent token is required".into()))?;
    if token.len() < 32 {
        return Err(Error::Validation("invalid agent token".into()));
    }
    let name = v["name"].as_str().unwrap_or_default();
    if name.trim().is_empty() {
        return Err(Error::Validation("agent name is required".into()));
    }
    web.db.save_agent(&Agent {
        id: id.clone(),
        name: name.into(),
        url: parsed.origin().ascii_serialization(),
        token,
    })?;
    Ok(Json(json!({"id":id})))
}
pub(super) async fn remove_agent(
    State(web): State<Arc<Web>>,
    Path(id): Path<String>,
) -> Result<Json<Value>> {
    web.db.remove_agent(&id)?;
    Ok(Json(json!({"ok":true})))
}
pub(super) async fn discovery(
    State(web): State<Arc<Web>>,
    Path(id): Path<String>,
) -> Result<Json<Value>> {
    let (status, capabilities, schema) = tokio::join!(
        web.read(&id, "/api/v1/xray/status"),
        web.read(&id, "/api/v1/xray/capabilities"),
        web.read(&id, "/openapi.json")
    );
    Ok(Json(
        json!({"status":status?,"capabilities":capabilities?,"openapi":schema?}),
    ))
}
pub(super) async fn users(State(web): State<Arc<Web>>) -> Result<Json<Vec<Value>>> {
    Ok(Json(web.db.users()?))
}
pub(super) async fn add_user(
    State(web): State<Arc<Web>>,
    Json(v): Json<Value>,
) -> Result<Json<Value>> {
    Ok(Json(
        web.db.add_user(v["name"].as_str().unwrap_or_default())?,
    ))
}
pub(super) async fn delete_user(
    State(web): State<Arc<Web>>,
    Path(id): Path<String>,
) -> Result<Json<Value>> {
    web.db.delete_user(&id)?;
    Ok(Json(json!({"ok":true})))
}
pub(super) async fn tasks(State(web): State<Arc<Web>>) -> Result<Json<Vec<Value>>> {
    Ok(Json(web.db.tasks()?))
}
pub(super) async fn retry(
    State(web): State<Arc<Web>>,
    Path(id): Path<String>,
) -> Result<Json<Value>> {
    web.db.retry(&id)?;
    Ok(Json(json!({"ok":true})))
}
