use super::{configuration_write, reconcile::applied};
use crate::{
    error::{Error, Result},
    web::Web,
};
use serde_json::{Value, json};

pub(super) async fn step(web: &Web, agent: &str) -> Result<()> {
    let lock = web.lock(agent).await;
    let _guard = lock.lock().await;
    let Some(task) = web.db.due(agent)? else {
        return Ok(());
    };
    if !web.db.current(&task)? {
        return web.db.mark(
            &task,
            "canceled",
            Some("superseded by newer assignment intent"),
            None,
            false,
        );
    }
    let status = match web.read(agent, "/api/v1/xray/status").await {
        Ok(v) => v,
        Err(e) => {
            return web.db.mark(
                &task,
                if agent != "local" && matches!(e, Error::Validation(_)) {
                    "blocked"
                } else {
                    "retry"
                },
                Some(&e.to_string()),
                None,
                task.uncertain,
            );
        }
    };
    let configuration = configuration_write(&task.request);
    if configuration
        && (status["sync_state"] != "in_sync"
            || status["operation_in_progress"] == true
            || status["grpc_available"] != true)
    {
        return web.db.mark(
            &task,
            "retry",
            Some("agent is unavailable or requires consistency recovery; no write was sent"),
            None,
            task.uncertain,
        );
    }
    // Known matching committed state is safe only when the Agent reports consistency.
    if configuration && status["sync_state"] == "in_sync" {
        if applied(web, &task).await? {
            return web.db.mark(
                &task,
                "complete",
                None,
                Some(&json!({"reconciled":true})),
                false,
            );
        }
    }
    if task.uncertain && !configuration {
        return web.db.mark(&task,"blocked",Some("execution result is uncertain; inspect the agent before submitting a new operation"),None,true);
    }
    if let Some(revision) = task.request.expected_revision {
        if status["revision"].as_i64() != Some(revision) {
            return web.db.mark(
                &task,
                "blocked",
                Some("configuration changed after preview; reload, preview and submit again"),
                None,
                false,
            );
        }
    }
    let endpoint = crate::web::endpoint(&task.request)?;
    let schema = match web.read(agent, "/openapi.json").await {
        Ok(s) => s,
        Err(e) => {
            return web
                .db
                .mark(&task, "retry", Some(&e.to_string()), None, task.uncertain);
        }
    };
    if schema["paths"][endpoint.path][endpoint.verb].is_null() {
        return web.db.mark(
            &task,
            "blocked",
            Some("remote agent does not advertise this operation"),
            None,
            false,
        );
    }
    web.db.mark(&task, "running", None, None, true)?;
    match web.request(agent, &task.request).await {
        Err(e) => web
            .db
            .mark(&task, "retry", Some(&e.to_string()), None, true),
        Ok(response) => {
            let status = response.status();
            let bytes = match response.bytes().await {
                Ok(v) => v,
                Err(_) => {
                    return web.db.mark(
                        &task,
                        "retry",
                        Some("response was interrupted; reconcile before retry"),
                        None,
                        true,
                    );
                }
            };
            let value = serde_json::from_slice::<Value>(&bytes)
                .unwrap_or_else(|_| json!({"text":String::from_utf8_lossy(&bytes)}));
            let busy = status.as_u16() == 409
                && value["error"]
                    .as_str()
                    .is_some_and(|e| e.contains("another management operation"));
            let result = json!({"status":status.as_u16(),"body":value});
            if status.is_success() {
                web.db.mark(
                    &task,
                    if status.as_u16() == 202 {
                        "accepted"
                    } else {
                        "complete"
                    },
                    None,
                    Some(&result),
                    false,
                )
            } else if busy || status.is_server_error() && status.as_u16() != 501 {
                web.db.mark(&task,"retry",Some("agent rejected or could not confirm the operation; reconciliation required"),Some(&result),status.is_server_error())
            } else {
                web.db.mark(
                    &task,
                    "blocked",
                    Some("agent rejected the operation; inspect its response"),
                    Some(&result),
                    false,
                )
            }
        }
    }
}
