use super::{Effect, Invoke, Web, auth::hash, endpoint, mutating, worker};
use crate::error::{Error, Result};
use axum::{
    Json,
    body::Body,
    extract::{Path, State},
    http::{StatusCode, header},
    response::{IntoResponse, Response},
};
use serde_json::{Value, json};
use std::sync::Arc;

async fn preview_data(web: &Web, id: &str, r: &Invoke) -> Result<Value> {
    let e = endpoint(r)?;
    let snapshot = if e.mutating && worker::configuration_write(r) {
        web.read(id, "/api/v1/xray/config").await?
    } else {
        Value::Null
    };
    let effects = worker::effects(&web.db.assignments()?, id, r);
    let mut normalized = r.clone();
    normalized.confirmation = None;
    normalized.expected_revision = None;
    let token = hash(&serde_json::to_string(
        &json!({"request":normalized,"agent":id,"revision":snapshot["revision"],"effects":effects}),
    )?);
    Ok(
        json!({"confirmation":token,"revision":snapshot["revision"],"effects":effects,"restarts_xray":r.method=="PUT"&&r.path=="/api/v1/xray/config","request":normalized}),
    )
}
pub(super) async fn preview(
    State(web): State<Arc<Web>>,
    Path(id): Path<String>,
    Json(r): Json<Invoke>,
) -> Result<Json<Value>> {
    web.agent(&id)?;
    Ok(Json(preview_data(&web, &id, &r).await?))
}
pub(super) async fn invoke(
    State(web): State<Arc<Web>>,
    Path(id): Path<String>,
    Json(mut r): Json<Invoke>,
) -> Result<Response> {
    r.method = r.method.to_uppercase();
    let e = endpoint(&r)?;
    if e.kind == crate::api::Kind::Events {
        return Err(Error::Validation(
            "use the authenticated /events stream endpoint".into(),
        ));
    }
    if !mutating(&r)? {
        let upstream = web.request(&id, &r).await?;
        let status = upstream.status();
        let content_type = upstream.headers().get(header::CONTENT_TYPE).cloned();
        let mut response = Response::new(Body::from_stream(upstream.bytes_stream()));
        *response.status_mut() = status;
        if let Some(ct) = content_type {
            response.headers_mut().insert(header::CONTENT_TYPE, ct);
        }
        return Ok(response);
    }
    let lock = web.lock(&id).await;
    let _guard = lock.lock().await;
    let preview = preview_data(&web, &id, &r).await?;
    let effects: Vec<Effect> = serde_json::from_value(preview["effects"].clone())?;
    if (!effects.is_empty() || r.path == "/api/v1/xray/config")
        && r.confirmation.as_deref() != preview["confirmation"].as_str()
    {
        return Err(Error::Conflict(
            "preview and confirm the current impact before submitting".into(),
        ));
    }
    r.expected_revision = if r.path == "/api/v1/xray/config" && r.method == "PUT" {
        preview["revision"].as_i64()
    } else {
        None
    };
    r.confirmation = None;
    let task = web.db.enqueue(&id, &r, &effects)?;
    Ok((StatusCode::ACCEPTED, Json(json!({"task_id":task}))).into_response())
}
