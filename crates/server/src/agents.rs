use crate::{
    app::RequestId,
    auth::{self, Session},
    db,
    error::{ApiJson, AppError},
    state::AppState,
    tasks,
};
use axum::{
    Extension, Json,
    extract::{
        State, WebSocketUpgrade,
        ws::{Message as WsMessage, WebSocket, rejection::WebSocketUpgradeRejection},
    },
    http::{HeaderMap, StatusCode, Uri},
    response::Response,
};
use futures_util::{SinkExt, StreamExt};
use panel_common::{
    EnrollmentInput, EnrollmentResponse, TaskCommand, TaskOutcome,
    protocol::{Envelope, HEARTBEAT_SECONDS, MAX_MESSAGE_BYTES, Message, OFFLINE_SECONDS},
};
use std::time::{Duration, Instant};
use tokio::sync::OwnedSemaphorePermit;
use uuid::Uuid;

pub async fn issue(
    State(state): State<AppState>,
    Extension(request_id): Extension<RequestId>,
    session: Session,
    ApiJson(input): ApiJson<EnrollmentInput>,
) -> Result<Json<EnrollmentResponse>, AppError> {
    let node_id = Uuid::parse_str(&input.node_id)
        .map_err(|_| AppError::invalid())?
        .to_string();
    let token = auth::random_token();
    let expires_at = db::now() + 600;
    let mut tx = state.pool.begin_with("BEGIN IMMEDIATE").await?;
    let active: bool = sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM nodes WHERE id = ? AND revoked_at IS NULL)",
    )
    .bind(&node_id)
    .fetch_one(&mut *tx)
    .await?;
    if !active {
        return Err(AppError::not_found());
    }
    sqlx::query(
        "DELETE FROM enrollments WHERE (node_id = ? AND consumed_at IS NULL) OR expires_at < ?",
    )
    .bind(&node_id)
    .bind(db::now() - 86400)
    .execute(&mut *tx)
    .await?;
    sqlx::query("INSERT INTO enrollments (id, node_id, token_hash, expires_at, created_at) VALUES (?, ?, ?, ?, ?)")
        .bind(Uuid::new_v4().to_string()).bind(&node_id).bind(auth::token_hash(&token)).bind(expires_at).bind(db::now()).execute(&mut *tx).await?;
    db::audit(
        &mut tx,
        Some(&session.admin.id),
        Some(&node_id),
        "node.enrollment.issue",
        &node_id,
        "success",
        Some(&request_id.0),
    )
    .await?;
    tx.commit().await?;
    Ok(Json(EnrollmentResponse {
        node_id,
        token,
        expires_at,
    }))
}

struct Identity {
    node_id: String,
    credential_hash: String,
    enrollment_credential: Option<String>,
}

async fn authenticate(state: &AppState, headers: &HeaderMap) -> Result<Identity, AppError> {
    let mut values = headers.get_all("authorization").iter();
    let value = values
        .next()
        .and_then(|value| value.to_str().ok())
        .ok_or_else(AppError::unauthorized)?;
    if values.next().is_some() {
        return Err(AppError::unauthorized());
    }
    let (kind, token) = value.split_once(' ').ok_or_else(AppError::unauthorized)?;
    if token.len() != 64 || !token.bytes().all(|value| value.is_ascii_hexdigit()) {
        return Err(AppError::unauthorized());
    }
    let hash = auth::token_hash(token);
    match kind {
        "Bearer" => {
            let node_id = sqlx::query_scalar::<_, String>("SELECT c.node_id FROM node_credentials c JOIN nodes n ON n.id = c.node_id WHERE c.credential_hash = ? AND c.revoked_at IS NULL AND c.expires_at > ? AND n.revoked_at IS NULL")
                .bind(&hash).bind(db::now()).fetch_optional(&state.pool).await?.ok_or_else(AppError::unauthorized)?;
            Ok(Identity {
                node_id,
                credential_hash: hash,
                enrollment_credential: None,
            })
        }
        "Enrollment" => {
            let mut tx = state.pool.begin_with("BEGIN IMMEDIATE").await?;
            let node_id = sqlx::query_scalar::<_, String>("SELECT e.node_id FROM enrollments e JOIN nodes n ON n.id = e.node_id WHERE e.token_hash = ? AND e.consumed_at IS NULL AND e.expires_at > ? AND n.revoked_at IS NULL")
                .bind(&hash).bind(db::now()).fetch_optional(&mut *tx).await?.ok_or_else(AppError::unauthorized)?;
            sqlx::query("UPDATE enrollments SET consumed_at = ? WHERE token_hash = ?")
                .bind(db::now())
                .bind(hash)
                .execute(&mut *tx)
                .await?;
            let credential = auth::random_token();
            let credential_hash = auth::token_hash(&credential);
            sqlx::query("INSERT INTO node_credentials (node_id, credential_hash, issued_at, expires_at) VALUES (?, ?, ?, ?) ON CONFLICT(node_id) DO UPDATE SET credential_hash = excluded.credential_hash, issued_at = excluded.issued_at, expires_at = excluded.expires_at, revoked_at = NULL")
                .bind(&node_id).bind(&credential_hash).bind(db::now()).bind(db::now() + 365 * 86400).execute(&mut *tx).await?;
            sqlx::query("UPDATE nodes SET status = 'offline', connection_id = NULL WHERE id = ?")
                .bind(&node_id)
                .execute(&mut *tx)
                .await?;
            db::audit(
                &mut tx,
                None,
                Some(&node_id),
                "agent.enroll",
                &node_id,
                "success",
                None,
            )
            .await?;
            tx.commit().await?;
            Ok(Identity {
                node_id,
                credential_hash,
                enrollment_credential: Some(credential),
            })
        }
        _ => Err(AppError::unauthorized()),
    }
}

pub async fn connect(
    State(state): State<AppState>,
    uri: Uri,
    headers: HeaderMap,
    upgrade: Result<WebSocketUpgrade, WebSocketUpgradeRejection>,
) -> Result<Response, AppError> {
    if !state.tls_enabled && state.security.secure_cookie {
        return Err(AppError::new(
            StatusCode::FORBIDDEN,
            "tls_required",
            "Agent connections require TLS or an explicitly enabled HTTP mode",
        ));
    }
    if uri.query().is_some() {
        return Err(AppError::invalid());
    }
    let upgrade = upgrade.map_err(|_| AppError::invalid())?;
    let permit = state
        .agent_slots
        .clone()
        .try_acquire_owned()
        .map_err(|_| AppError::limited())?;
    let identity = authenticate(&state, &headers).await?;
    Ok(upgrade
        .max_message_size(MAX_MESSAGE_BYTES)
        .max_frame_size(MAX_MESSAGE_BYTES)
        .on_upgrade(move |socket| connection(socket, state, identity, permit)))
}

async fn send(socket: &mut WebSocket, node_id: &str, message: Message) -> Result<(), AppError> {
    let text = serde_json::to_string(&Envelope::new(node_id, message))
        .map_err(|_| AppError::internal())?;
    tokio::time::timeout(
        Duration::from_secs(5),
        socket.send(WsMessage::Text(text.into())),
    )
    .await
    .map_err(|_| AppError::internal())?
    .map_err(|_| AppError::internal())
}

async fn current(
    state: &AppState,
    identity: &Identity,
    generation: &str,
) -> Result<bool, AppError> {
    Ok(sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM nodes n JOIN node_credentials c ON c.node_id = n.id WHERE n.id = ? AND n.connection_id = ? AND n.revoked_at IS NULL AND c.credential_hash = ? AND c.revoked_at IS NULL AND c.expires_at > ?)")
        .bind(&identity.node_id).bind(generation).bind(&identity.credential_hash).bind(db::now()).fetch_one(&state.pool).await?)
}

async fn connection(
    mut socket: WebSocket,
    state: AppState,
    identity: Identity,
    _permit: OwnedSemaphorePermit,
) {
    let generation = Uuid::new_v4().to_string();
    let _ = session(&mut socket, &state, &identity, &generation).await;
    let _ = sqlx::query("UPDATE nodes SET status = 'offline', connection_id = NULL WHERE id = ? AND connection_id = ? AND revoked_at IS NULL")
        .bind(&identity.node_id).bind(generation).execute(&state.pool).await;
    let _ = tokio::time::timeout(Duration::from_secs(1), socket.close()).await;
}

async fn session(
    socket: &mut WebSocket,
    state: &AppState,
    identity: &Identity,
    generation: &str,
) -> Result<(), AppError> {
    if let Some(credential) = &identity.enrollment_credential {
        send(
            socket,
            &identity.node_id,
            Message::Enrolled {
                credential: credential.clone(),
            },
        )
        .await?;
    }
    let first = tokio::time::timeout(Duration::from_secs(10), socket.next())
        .await
        .map_err(|_| AppError::invalid())?
        .ok_or_else(AppError::invalid)?
        .map_err(|_| AppError::invalid())?;
    let WsMessage::Text(text) = first else {
        return Err(AppError::invalid());
    };
    let frame = Envelope::parse(&text, Some(&identity.node_id)).map_err(|_| AppError::invalid())?;
    let Message::Hello(info) = frame.message else {
        return Err(AppError::invalid());
    };
    if !info.valid() {
        return Err(AppError::invalid());
    }
    let changed = sqlx::query("UPDATE nodes SET status = 'online', connection_id = ?, agent_version = ?, os = ?, arch = ?, last_seen_at = ? WHERE id = ? AND revoked_at IS NULL AND EXISTS(SELECT 1 FROM node_credentials c WHERE c.node_id = nodes.id AND c.credential_hash = ? AND c.revoked_at IS NULL AND c.expires_at > ?)")
        .bind(generation).bind(info.version).bind(info.os).bind(info.arch).bind(db::now()).bind(&identity.node_id).bind(&identity.credential_hash).bind(db::now()).execute(&state.pool).await?;
    if changed.rows_affected() != 1 {
        return Err(AppError::unauthorized());
    }
    crate::configuration::reconcile(&state.pool, &identity.node_id).await?;
    send(socket, &identity.node_id, Message::Welcome).await?;
    let mut tick = tokio::time::interval(Duration::from_secs(1));
    tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    let mut shutdown = state.agents_shutdown.subscribe();
    let mut heartbeat = Instant::now();
    let mut ping = Instant::now();
    let mut inflight: Option<(String, Instant)> = None;
    loop {
        tokio::select! {
            _ = shutdown.changed() => return Ok(()),
            _ = tick.tick() => {
                if heartbeat.elapsed() >= Duration::from_secs(OFFLINE_SECONDS) || !current(state, identity, generation).await? { return Ok(()); }
                tasks::expire(&state.pool, &identity.node_id).await?;
                if ping.elapsed() >= Duration::from_secs(HEARTBEAT_SECONDS) {
                    tokio::time::timeout(Duration::from_secs(5), socket.send(WsMessage::Ping(Vec::new().into()))).await.map_err(|_| AppError::internal())?.map_err(|_| AppError::internal())?;
                    ping = Instant::now();
                }
                if inflight.as_ref().is_none_or(|(_, sent)| sent.elapsed() >= Duration::from_secs(10)) {
                    let payload = if let Some((id, _)) = &inflight {
                        sqlx::query_scalar::<_, String>("SELECT payload_json FROM tasks WHERE id = ? AND node_id = ? AND status IN ('pending','dispatched','acked') AND deadline_at > ?")
                            .bind(id).bind(&identity.node_id).bind(db::now()).fetch_optional(&state.pool).await?
                    } else {
                        sqlx::query_scalar::<_, String>("SELECT payload_json FROM tasks WHERE node_id = ? AND status IN ('pending','dispatched','acked') AND deadline_at > ? ORDER BY created_at, id LIMIT 1")
                            .bind(&identity.node_id).bind(db::now()).fetch_optional(&state.pool).await?
                    };
                    if let Some(payload) = payload {
                        let task: TaskCommand = serde_json::from_str(&payload).map_err(|_| AppError::internal())?;
                        let changed = sqlx::query("UPDATE tasks SET status = CASE WHEN status = 'acked' THEN status ELSE 'dispatched' END, attempt_count = attempt_count + 1 WHERE id = ? AND status IN ('pending','dispatched','acked') AND deadline_at > ? AND EXISTS(SELECT 1 FROM nodes WHERE id=tasks.node_id AND connection_id=? AND revoked_at IS NULL)").bind(&task.id).bind(db::now()).bind(generation).execute(&state.pool).await?;
                        if changed.rows_affected() == 0 { inflight = None; continue; }
                        inflight = Some((task.id.clone(), Instant::now()));
                        send(socket, &identity.node_id, Message::Task(task)).await?;
                    } else { inflight = None; }
                }
            }
            incoming = socket.next() => {
                let Some(Ok(incoming)) = incoming else { return Ok(()); };
                if !current(state, identity, generation).await? { return Ok(()); }
                match incoming {
                    WsMessage::Text(text) => {
                        let frame = Envelope::parse(&text, Some(&identity.node_id)).map_err(|_| AppError::invalid())?;
                        match frame.message {
                            Message::Heartbeat => {
                                heartbeat = Instant::now();
                                sqlx::query("UPDATE nodes SET last_seen_at = ? WHERE id = ? AND connection_id = ?").bind(db::now()).bind(&identity.node_id).bind(generation).execute(&state.pool).await?;
                            }
                            Message::Metrics(snapshot) => {
                                if !snapshot.valid(db::now()) { return Err(AppError::invalid()); }
                                sqlx::query("INSERT INTO node_metrics (node_id, connection_id, snapshot_json, received_at) SELECT ?, ?, ?, ? WHERE EXISTS(SELECT 1 FROM nodes WHERE id = ? AND connection_id = ? AND revoked_at IS NULL) ON CONFLICT(node_id) DO UPDATE SET connection_id = excluded.connection_id, snapshot_json = excluded.snapshot_json, received_at = excluded.received_at")
                                    .bind(&identity.node_id).bind(generation).bind(serde_json::to_string(&snapshot).map_err(|_| AppError::internal())?).bind(db::now()).bind(&identity.node_id).bind(generation).execute(&state.pool).await?;
                            }
                            Message::Retire => {
                                let mut tx=state.pool.begin_with("BEGIN IMMEDIATE").await?;
                                let changed=sqlx::query("UPDATE node_credentials SET revoked_at=? WHERE node_id=? AND credential_hash=? AND revoked_at IS NULL").bind(db::now()).bind(&identity.node_id).bind(&identity.credential_hash).execute(&mut *tx).await?;
                                if changed.rows_affected()!=1 {return Err(AppError::unauthorized());}
                                sqlx::query("UPDATE tasks SET status='canceled',finished_at=? WHERE node_id=? AND status IN ('pending','dispatched','acked')").bind(db::now()).bind(&identity.node_id).execute(&mut *tx).await?;
                                db::audit(&mut tx,None,Some(&identity.node_id),"agent.retire",&identity.node_id,"success",None).await?;
                                tx.commit().await?;
                                send(socket,&identity.node_id,Message::Retired).await?;
                                return Ok(());
                            }
                            Message::Traffic(report) => { crate::traffic::record(&state.pool, &identity.node_id, generation, &report).await?; }
                            Message::TaskAck { task_id } => {
                                let changed = sqlx::query("UPDATE tasks SET status = 'acked' WHERE id = ? AND node_id = ? AND status IN ('dispatched','acked') AND deadline_at > ?")
                                    .bind(&task_id).bind(&identity.node_id).bind(db::now()).execute(&state.pool).await?;
                                if changed.rows_affected() == 0 && !belongs(state, &task_id, &identity.node_id).await? { return Err(AppError::invalid()); }
                            }
                            Message::TaskResult { task_id, result } => {
                                record_result(state, identity, generation, &task_id, &result).await?;
                                if inflight.as_ref().is_some_and(|(id, _)| id == &task_id) { inflight = None; }
                                send(socket, &identity.node_id, Message::ResultAck { task_id }).await?;
                            }
                            _ => return Err(AppError::invalid()),
                        }
                    }
                    WsMessage::Ping(data) => { tokio::time::timeout(Duration::from_secs(5), socket.send(WsMessage::Pong(data))).await.map_err(|_| AppError::internal())?.map_err(|_| AppError::internal())?; }
                    WsMessage::Pong(_) => (),
                    WsMessage::Close(_) => return Ok(()),
                    WsMessage::Binary(_) => return Err(AppError::invalid()),
                }
            }
        }
    }
}

async fn record_result(
    state: &AppState,
    identity: &Identity,
    generation: &str,
    task_id: &str,
    result: &TaskOutcome,
) -> Result<(), AppError> {
    let payload: String =
        sqlx::query_scalar("SELECT payload_json FROM tasks WHERE id = ? AND node_id = ?")
            .bind(task_id)
            .bind(&identity.node_id)
            .fetch_optional(&state.pool)
            .await?
            .ok_or_else(AppError::invalid)?;
    let command: TaskCommand = serde_json::from_str(&payload).map_err(|_| AppError::internal())?;
    if !result.valid_for(&command.operation) {
        return Err(AppError::invalid());
    }
    let status = if result.is_success() {
        "succeeded"
    } else {
        "failed"
    };
    let mut tx = state.pool.begin_with("BEGIN IMMEDIATE").await?;
    let changed = sqlx::query("UPDATE tasks SET status = ?, result_json = ?, finished_at = ? WHERE id = ? AND node_id = ? AND status IN ('dispatched','acked') AND deadline_at > ? AND EXISTS(SELECT 1 FROM nodes n JOIN node_credentials c ON c.node_id = n.id WHERE n.id = tasks.node_id AND n.connection_id = ? AND n.revoked_at IS NULL AND c.credential_hash = ? AND c.revoked_at IS NULL AND c.expires_at > ?)")
        .bind(status).bind(serde_json::to_string(result).map_err(|_| AppError::internal())?).bind(db::now()).bind(task_id).bind(&identity.node_id).bind(db::now()).bind(generation).bind(&identity.credential_hash).bind(db::now()).execute(&mut *tx).await?;
    if changed.rows_affected() == 1
        && matches!(
            command.operation,
            panel_common::TaskOperation::ApplyConfig(_)
        )
    {
        sqlx::query(
            "UPDATE config_revisions SET status=?, applied_at=? WHERE task_id=? AND node_id=?",
        )
        .bind(if result.is_success() {
            "applied"
        } else {
            "failed"
        })
        .bind(if result.is_success() {
            Some(db::now())
        } else {
            None
        })
        .bind(task_id)
        .bind(&identity.node_id)
        .execute(&mut *tx)
        .await?;
        db::audit(
            &mut tx,
            None,
            Some(&identity.node_id),
            "config.result",
            task_id,
            if result.is_success() {
                "success"
            } else {
                "denied"
            },
            None,
        )
        .await?;
    }
    if changed.rows_affected() == 1
        && matches!(
            command.operation,
            panel_common::TaskOperation::ServiceStatus(_) | panel_common::TaskOperation::Xray(_)
        )
    {
        db::audit(
            &mut tx,
            None,
            Some(&identity.node_id),
            if matches!(command.operation, panel_common::TaskOperation::Xray(_)) {
                "xray.operation"
            } else {
                "service.status"
            },
            task_id,
            if result.is_success() {
                "success"
            } else {
                "denied"
            },
            None,
        )
        .await?;
    }
    tx.commit().await?;
    Ok(())
}

async fn belongs(state: &AppState, task_id: &str, node_id: &str) -> Result<bool, AppError> {
    Ok(
        sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM tasks WHERE id = ? AND node_id = ?)")
            .bind(task_id)
            .bind(node_id)
            .fetch_one(&state.pool)
            .await?,
    )
}
