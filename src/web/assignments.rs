use super::{Web, db::Assignment, worker};
use crate::{
    error::{Error, Result},
    web_config,
};
use axum::{
    Json,
    extract::{Path, State},
};
use serde_json::{Value, json};
use std::sync::Arc;

pub(super) async fn assignments(State(web): State<Arc<Web>>) -> Result<Json<Vec<Assignment>>> {
    Ok(Json(web.db.assignments()?))
}
pub(super) async fn assign(
    State(web): State<Arc<Web>>,
    Json(v): Json<Value>,
) -> Result<Json<Value>> {
    let agent = v["agent_id"].as_str().unwrap_or_default();
    let tag = v["inbound_tag"].as_str().unwrap_or_default();
    let lock = web.lock(agent).await;
    let _guard = lock.lock().await;
    let config = web.read(agent, "/api/v1/xray/config").await?;
    let inbound = config["config"]["inbounds"]
        .as_array()
        .and_then(|list| list.iter().find(|i| i["tag"] == tag))
        .ok_or_else(|| Error::NotFound("inbound not found".into()))?;
    let existing = web.db.assignments()?.into_iter().find(|a| {
        a.agent_id == agent
            && a.inbound_tag == tag
            && a.user_id == v["user_id"].as_str().unwrap_or_default()
    });
    if existing.as_ref().is_some_and(|a| a.status != "revoked") {
        return Err(Error::Conflict(
            "revoke the existing assignment before assigning again".into(),
        ));
    }
    let id = existing
        .as_ref()
        .map(|a| a.id.clone())
        .unwrap_or_else(|| uuid::Uuid::new_v4().to_string());
    let generation = existing.as_ref().map_or(1, |a| a.generation + 1);
    let adopt = v["adopt_email"].as_str();
    let user = if let Some(email) = adopt {
        worker::native_users(inbound)
            .into_iter()
            .find(|u| {
                u["email"]
                    .as_str()
                    .is_some_and(|s| s.eq_ignore_ascii_case(email))
            })
            .cloned()
            .ok_or_else(|| Error::NotFound("existing inbound user not found".into()))?
    } else {
        let mut user = json!({"email":format!("web-{id}"),"level":0});
        match inbound["protocol"].as_str().unwrap_or_default() {
            "vless" | "vmess" => user["id"] = json!(uuid::Uuid::new_v4().to_string()),
            "trojan" => user["password"] = json!(web_config::secret(24)?),
            "shadowsocks" => {
                let method = inbound["settings"]["method"].as_str().unwrap_or_default();
                let bytes = if method.contains("aes-128") { 16 } else { 32 };
                use base64::Engine;
                let mut random = vec![0; bytes];
                use rand::TryRngCore;
                rand::rngs::OsRng
                    .try_fill_bytes(&mut random)
                    .map_err(|e| Error::Internal(e.to_string()))?;
                user["password"] = json!(base64::engine::general_purpose::STANDARD.encode(random));
            }
            "hysteria" => user["auth"] = json!(web_config::secret(24)?),
            "masque" => user["pass"] = json!(web_config::secret(24)?),
            _ => {
                return Err(Error::Unsupported(
                    "this protocol requires explicit native user management or adoption".into(),
                ));
            }
        }
        if let Some(extra) = v["user"].as_object() {
            for (key, value) in extra {
                if key != "email" {
                    user[key] = value.clone();
                }
            }
        }
        user
    };
    let a = Assignment {
        id,
        user_id: v["user_id"].as_str().unwrap_or_default().into(),
        agent_id: agent.into(),
        inbound_tag: tag.into(),
        native_user: user,
        desired_present: true,
        generation,
        status: "pending".into(),
    };
    let task = web.db.create_assignment(&a)?;
    Ok(Json(json!({"assignment":a,"task_id":task})))
}
pub(super) async fn revoke(
    State(web): State<Arc<Web>>,
    Path(id): Path<String>,
) -> Result<Json<Value>> {
    Ok(Json(json!({"task_id":web.db.revoke(&id)?})))
}
