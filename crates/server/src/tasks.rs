use crate::{
    api::NodeId,
    app::RequestId,
    auth::Session,
    db,
    error::{ApiJson, AppError},
    state::AppState,
};
use axum::{
    Extension, Json,
    extract::{Path, State},
    http::StatusCode,
};
use panel_common::{TaskCommand, TaskInput, TaskOperation, TaskOutcome, TaskRecord};
use uuid::Uuid;

pub async fn xray(
    state: State<AppState>,
    request_id: Extension<RequestId>,
    session: Session,
    Path((id, action)): Path<(String, String)>,
    input: Result<ApiJson<panel_common::xray::XrayApiInput>, AppError>,
) -> Result<(StatusCode, Json<TaskRecord>), AppError> {
    use panel_common::{XrayOperation, XrayRelease};
    let id = Uuid::parse_str(&id)
        .map_err(|_| AppError::invalid())?
        .to_string();
    let task = input.and_then(|ApiJson(input)| {
        let operation = match (action.as_str(), input.version) {
            ("install", Some(version)) => XrayOperation::Install(XrayRelease { version }),
            ("upgrade", Some(version)) => XrayOperation::Upgrade(XrayRelease { version }),
            ("start", None) => XrayOperation::Start,
            ("stop", None) => XrayOperation::Stop,
            ("restart", None) => XrayOperation::Restart,
            ("rollback", None) => XrayOperation::Rollback,
            ("uninstall", None) => XrayOperation::Uninstall,
            ("status", None) => XrayOperation::Status,
            _ => return Err(AppError::invalid()),
        };
        Ok(ApiJson(TaskInput {
            operation: TaskOperation::Xray(operation),
            idempotency_key: input.idempotency_key,
        }))
    });
    create(state, request_id, session, NodeId(id), task).await
}

#[derive(sqlx::FromRow)]
struct Row {
    id: String,
    node_id: String,
    status: String,
    attempt_count: i32,
    created_at: i64,
    deadline_at: i64,
    finished_at: Option<i64>,
    result_json: Option<String>,
    payload_json: String,
}
impl Row {
    fn record(self) -> Result<TaskRecord, AppError> {
        let command: TaskCommand =
            serde_json::from_str(&self.payload_json).map_err(|_| AppError::internal())?;
        Ok(TaskRecord {
            id: self.id,
            node_id: self.node_id,
            operation: command.operation,
            status: self.status,
            attempt_count: self.attempt_count,
            created_at: self.created_at,
            deadline_at: self.deadline_at,
            finished_at: self.finished_at,
            result: self
                .result_json
                .map(|value| {
                    serde_json::from_str::<TaskOutcome>(&value).map_err(|_| AppError::internal())
                })
                .transpose()?,
        })
    }
}

pub async fn expire(pool: &sqlx::SqlitePool, node_id: &str) -> Result<(), AppError> {
    sqlx::query("UPDATE tasks SET status = 'expired', finished_at = ?, error_code = 'deadline_exceeded' WHERE node_id = ? AND deadline_at <= ? AND status IN ('pending','dispatched','acked')")
        .bind(db::now()).bind(node_id).bind(db::now()).execute(pool).await?;
    sqlx::query("UPDATE config_revisions SET status='failed' WHERE node_id=? AND status='pending' AND task_id IN (SELECT id FROM tasks WHERE status IN ('expired','canceled','failed'))").bind(node_id).execute(pool).await?;
    Ok(())
}

pub async fn create(
    State(state): State<AppState>,
    Extension(request_id): Extension<RequestId>,
    session: Session,
    NodeId(node_id): NodeId,
    input: Result<ApiJson<TaskInput>, AppError>,
) -> Result<(StatusCode, Json<TaskRecord>), AppError> {
    let ApiJson(input) = match input {
        Ok(input) => input,
        Err(error) => {
            let mut tx = state.pool.begin().await?;
            db::audit(
                &mut tx,
                Some(&session.admin.id),
                None,
                "task.reject",
                &node_id,
                "denied",
                Some(&request_id.0),
            )
            .await?;
            tx.commit().await?;
            return Err(error);
        }
    };
    let key = Uuid::parse_str(&input.idempotency_key)
        .map_err(|_| AppError::invalid())?
        .to_string();
    expire(&state.pool, &node_id).await?;
    sqlx::query("DELETE FROM tasks WHERE deadline_at < ? AND status IN ('succeeded','failed','expired','canceled') AND id NOT IN (SELECT task_id FROM config_revisions r WHERE task_id IS NOT NULL AND revision=(SELECT MAX(revision) FROM config_revisions WHERE node_id=r.node_id))").bind(db::now() - 7 * 86400).execute(&state.pool).await?;
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
    if let Some(row) =
        sqlx::query_as::<_, Row>("SELECT * FROM tasks WHERE node_id = ? AND idempotency_key = ?")
            .bind(&node_id)
            .bind(&key)
            .fetch_optional(&mut *tx)
            .await?
    {
        let record = row.record()?;
        if record.operation != input.operation {
            db::audit(
                &mut tx,
                Some(&session.admin.id),
                Some(&node_id),
                "task.reject",
                &record.id,
                "denied",
                Some(&request_id.0),
            )
            .await?;
            tx.commit().await?;
            return Err(AppError::new(
                StatusCode::CONFLICT,
                "idempotency_conflict",
                "Idempotency key already belongs to a different operation",
            ));
        }
        tx.commit().await?;
        return Ok((StatusCode::OK, Json(record)));
    }
    let deadline_at = db::now()
        + if matches!(input.operation, TaskOperation::Xray(_)) {
            300
        } else {
            60
        };
    let command = TaskCommand {
        id: Uuid::new_v4().to_string(),
        operation: input.operation,
        deadline_at,
        private_config: None,
    };
    let payload = serde_json::to_string(&command).map_err(|_| AppError::internal())?;
    let operation = match &command.operation {
        TaskOperation::AgentStatus => "agent_status",
        TaskOperation::ServiceStatus(_) => "service_status",
        TaskOperation::Xray(_) => "xray",
        TaskOperation::ApplyConfig(_) => return Err(AppError::invalid()),
    };
    let inserted = sqlx::query("INSERT INTO tasks (id, node_id, operation, payload_json, idempotency_key, status, created_at, deadline_at) SELECT ?, ?, ?, ?, ?, 'pending', ?, ? WHERE (SELECT COUNT(*) FROM tasks WHERE node_id = ? AND status IN ('pending','dispatched','acked') AND deadline_at > ?) < 100")
        .bind(&command.id).bind(&node_id).bind(operation).bind(payload).bind(key).bind(db::now()).bind(command.deadline_at).bind(&node_id).bind(db::now()).execute(&mut *tx).await?;
    if inserted.rows_affected() == 0 {
        return Err(AppError::new(
            StatusCode::CONFLICT,
            "task_queue_full",
            "Node task queue is full",
        ));
    }
    db::audit(
        &mut tx,
        Some(&session.admin.id),
        Some(&node_id),
        "task.create",
        &command.id,
        "success",
        Some(&request_id.0),
    )
    .await?;
    let row = sqlx::query_as::<_, Row>("SELECT * FROM tasks WHERE id = ?")
        .bind(&command.id)
        .fetch_one(&mut *tx)
        .await?;
    tx.commit().await?;
    Ok((StatusCode::CREATED, Json(row.record()?)))
}

pub async fn get(
    State(state): State<AppState>,
    _: Session,
    Path(id): Path<String>,
) -> Result<Json<TaskRecord>, AppError> {
    let id = Uuid::parse_str(&id)
        .map_err(|_| AppError::invalid())?
        .to_string();
    let row = sqlx::query_as::<_, Row>("SELECT * FROM tasks WHERE id = ?")
        .bind(&id)
        .fetch_optional(&state.pool)
        .await?
        .ok_or_else(AppError::not_found)?;
    expire(&state.pool, &row.node_id).await?;
    let row = sqlx::query_as::<_, Row>("SELECT * FROM tasks WHERE id = ?")
        .bind(id)
        .fetch_one(&state.pool)
        .await?;
    Ok(Json(row.record()?))
}
