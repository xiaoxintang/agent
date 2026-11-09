use crate::{
    app::RequestId,
    auth::Session,
    db,
    error::{ApiJson, AppError},
    state::AppState,
};
use axum::{
    Extension, Json,
    extract::{FromRequestParts, Path, State},
    http::{StatusCode, request::Parts},
};
use panel_common::{AuditEntry, AuditMetadata, Node, NodeInput};
use uuid::Uuid;

#[derive(sqlx::FromRow)]
pub(crate) struct NodeRow {
    id: String,
    name: String,
    status: String,
    agent_version: Option<String>,
    os: Option<String>,
    arch: Option<String>,
    last_seen_at: Option<i64>,
    created_at: i64,
    revoked_at: Option<i64>,
}
impl From<NodeRow> for Node {
    fn from(row: NodeRow) -> Self {
        let status = if row.status == "online"
            && row.last_seen_at.is_none_or(|seen| {
                seen <= db::now() - panel_common::protocol::OFFLINE_SECONDS as i64
            }) {
            "offline".to_owned()
        } else {
            row.status
        };
        Self {
            id: row.id,
            name: row.name,
            status,
            agent_version: row.agent_version,
            os: row.os,
            arch: row.arch,
            last_seen_at: row.last_seen_at,
            created_at: row.created_at,
            revoked_at: row.revoked_at,
        }
    }
}

pub struct NodeId(pub(crate) String);
impl FromRequestParts<AppState> for NodeId {
    type Rejection = AppError;
    async fn from_request_parts(
        parts: &mut Parts,
        state: &AppState,
    ) -> Result<Self, Self::Rejection> {
        let Path(value) = Path::<String>::from_request_parts(parts, state)
            .await
            .map_err(|_| AppError::invalid())?;
        Ok(Self(
            Uuid::parse_str(&value)
                .map_err(|_| AppError::invalid())?
                .to_string(),
        ))
    }
}

fn name(input: NodeInput) -> Result<String, AppError> {
    let value = input.name.trim();
    if value.is_empty() || value.chars().count() > 64 || value.chars().any(char::is_control) {
        return Err(AppError::invalid());
    }
    Ok(value.to_owned())
}

pub async fn nodes(State(state): State<AppState>, _: Session) -> Result<Json<Vec<Node>>, AppError> {
    let rows = sqlx::query_as::<_, NodeRow>(
        "SELECT * FROM nodes WHERE revoked_at IS NULL ORDER BY created_at DESC, id LIMIT 100",
    )
    .fetch_all(&state.pool)
    .await?;
    Ok(Json(rows.into_iter().map(Into::into).collect()))
}

pub async fn local_node(
    State(state): State<AppState>,
    _: Session,
) -> Result<Json<serde_json::Value>, AppError> {
    let node_id: Option<String> =
        sqlx::query_scalar("SELECT node_id FROM local_node WHERE singleton = 1")
            .fetch_optional(&state.pool)
            .await?;
    Ok(Json(serde_json::json!({"node_id": node_id})))
}

pub async fn set_local_node(
    State(state): State<AppState>,
    Extension(request_id): Extension<RequestId>,
    session: Session,
    NodeId(id): NodeId,
) -> Result<Json<serde_json::Value>, AppError> {
    let mut tx = state.pool.begin().await?;
    let changed = sqlx::query("INSERT INTO local_node (singleton, node_id) SELECT 1, id FROM nodes WHERE id = ? AND revoked_at IS NULL ON CONFLICT(singleton) DO UPDATE SET node_id = excluded.node_id")
        .bind(&id).execute(&mut *tx).await?;
    if changed.rows_affected() == 0 {
        return Err(AppError::not_found());
    }
    db::audit(
        &mut tx,
        Some(&session.admin.id),
        Some(&id),
        "node.local.set",
        &id,
        "success",
        Some(&request_id.0),
    )
    .await?;
    tx.commit().await?;
    Ok(Json(serde_json::json!({"node_id": id})))
}

pub async fn node(
    State(state): State<AppState>,
    _: Session,
    NodeId(id): NodeId,
) -> Result<Json<Node>, AppError> {
    let row =
        sqlx::query_as::<_, NodeRow>("SELECT * FROM nodes WHERE id = ? AND revoked_at IS NULL")
            .bind(id)
            .fetch_optional(&state.pool)
            .await?
            .ok_or_else(AppError::not_found)?;
    Ok(Json(row.into()))
}

pub async fn create_node(
    State(state): State<AppState>,
    Extension(request_id): Extension<RequestId>,
    session: Session,
    ApiJson(input): ApiJson<NodeInput>,
) -> Result<(StatusCode, Json<Node>), AppError> {
    let name = name(input)?;
    let id = Uuid::new_v4().to_string();
    let mut tx = state.pool.begin().await?;
    // One write statement enforces the bound even when requests race.
    let inserted = sqlx::query("INSERT INTO nodes (id, name, created_at) SELECT ?, ?, ? WHERE (SELECT COUNT(*) FROM nodes WHERE revoked_at IS NULL) < 100")
        .bind(&id).bind(name).bind(db::now()).execute(&mut *tx).await?;
    if inserted.rows_affected() != 1 {
        return Err(AppError::new(
            StatusCode::CONFLICT,
            "node_limit",
            "At most 100 active nodes are supported",
        ));
    }
    db::audit(
        &mut tx,
        Some(&session.admin.id),
        Some(&id),
        "node.create",
        &id,
        "success",
        Some(&request_id.0),
    )
    .await?;
    let row = sqlx::query_as::<_, NodeRow>("SELECT * FROM nodes WHERE id = ?")
        .bind(id)
        .fetch_one(&mut *tx)
        .await?;
    tx.commit().await?;
    Ok((StatusCode::CREATED, Json(row.into())))
}

pub async fn update_node(
    State(state): State<AppState>,
    Extension(request_id): Extension<RequestId>,
    session: Session,
    NodeId(id): NodeId,
    ApiJson(input): ApiJson<NodeInput>,
) -> Result<Json<Node>, AppError> {
    let name = name(input)?;
    let mut tx = state.pool.begin().await?;
    let changed = sqlx::query("UPDATE nodes SET name = ? WHERE id = ? AND revoked_at IS NULL")
        .bind(name)
        .bind(&id)
        .execute(&mut *tx)
        .await?;
    if changed.rows_affected() == 0 {
        return Err(AppError::not_found());
    }
    db::audit(
        &mut tx,
        Some(&session.admin.id),
        Some(&id),
        "node.update",
        &id,
        "success",
        Some(&request_id.0),
    )
    .await?;
    let row = sqlx::query_as::<_, NodeRow>("SELECT * FROM nodes WHERE id = ?")
        .bind(id)
        .fetch_one(&mut *tx)
        .await?;
    tx.commit().await?;
    Ok(Json(row.into()))
}

pub async fn delete_node(
    State(state): State<AppState>,
    Extension(request_id): Extension<RequestId>,
    session: Session,
    NodeId(id): NodeId,
) -> Result<StatusCode, AppError> {
    let mut tx = state.pool.begin().await?;
    let changed = sqlx::query(
        "UPDATE nodes SET revoked_at = ?, status = 'revoked' WHERE id = ? AND revoked_at IS NULL",
    )
    .bind(db::now())
    .bind(&id)
    .execute(&mut *tx)
    .await?;
    if changed.rows_affected() == 0 {
        return Err(AppError::not_found());
    }
    sqlx::query("DELETE FROM local_node WHERE node_id = ?")
        .bind(&id)
        .execute(&mut *tx)
        .await?;
    sqlx::query("DELETE FROM node_metrics WHERE node_id = ?")
        .bind(&id)
        .execute(&mut *tx)
        .await?;
    sqlx::query("UPDATE node_credentials SET revoked_at = ? WHERE node_id = ?")
        .bind(db::now())
        .bind(&id)
        .execute(&mut *tx)
        .await?;
    sqlx::query("UPDATE enrollments SET consumed_at = ? WHERE node_id = ? AND consumed_at IS NULL")
        .bind(db::now())
        .bind(&id)
        .execute(&mut *tx)
        .await?;
    sqlx::query("UPDATE tasks SET status = 'canceled', finished_at = ? WHERE node_id = ? AND status IN ('pending','dispatched','acked')").bind(db::now()).bind(&id).execute(&mut *tx).await?;
    db::audit(
        &mut tx,
        Some(&session.admin.id),
        Some(&id),
        "node.revoke",
        &id,
        "success",
        Some(&request_id.0),
    )
    .await?;
    tx.commit().await?;
    state.wake_agent(&id);
    Ok(StatusCode::NO_CONTENT)
}

#[derive(sqlx::FromRow)]
struct AuditRow {
    id: String,
    actor_id: Option<String>,
    node_id: Option<String>,
    action: String,
    target: String,
    result: String,
    created_at: i64,
    metadata_json: String,
}

pub async fn audit(
    State(state): State<AppState>,
    _: Session,
) -> Result<Json<Vec<AuditEntry>>, AppError> {
    let rows = sqlx::query_as::<_, AuditRow>("SELECT CAST(id AS TEXT) AS id, actor_id, node_id, action, target, result, created_at, metadata_json FROM audit_logs WHERE created_at >= ? ORDER BY created_at DESC, id DESC LIMIT 100")
        .bind(db::now() - db::AUDIT_RETENTION).fetch_all(&state.pool).await?;
    let entries = rows
        .into_iter()
        .map(|row| {
            Ok(AuditEntry {
                id: row.id,
                actor_id: row.actor_id,
                node_id: row.node_id,
                action: row.action,
                target: row.target,
                result: row.result,
                created_at: row.created_at,
                metadata: serde_json::from_str::<AuditMetadata>(&row.metadata_json)
                    .map_err(|_| AppError::internal())?,
            })
        })
        .collect::<Result<Vec<_>, AppError>>()?;
    Ok(Json(entries))
}
