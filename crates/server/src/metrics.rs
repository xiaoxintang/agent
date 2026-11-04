use crate::{api::NodeId, auth::Session, db, error::AppError, state::AppState};
use axum::{Json, extract::State};
use panel_common::{
    MetricsSnapshot, NodeMetrics, protocol::OFFLINE_SECONDS, telemetry::METRICS_SECONDS,
};

#[derive(sqlx::FromRow)]
struct Row {
    status: String,
    last_seen_at: Option<i64>,
    connection_id: Option<String>,
    metrics_connection: Option<String>,
    snapshot_json: Option<String>,
    received_at: Option<i64>,
}

pub async fn get(
    State(state): State<AppState>,
    _: Session,
    NodeId(node_id): NodeId,
) -> Result<Json<NodeMetrics>, AppError> {
    let row = sqlx::query_as::<_, Row>("SELECT n.status, n.last_seen_at, n.connection_id, m.connection_id AS metrics_connection, m.snapshot_json, m.received_at FROM nodes n LEFT JOIN node_metrics m ON m.node_id = n.id WHERE n.id = ? AND n.revoked_at IS NULL")
        .bind(&node_id).fetch_optional(&state.pool).await?.ok_or_else(AppError::not_found)?;
    let stale = row.status != "online"
        || row.connection_id != row.metrics_connection
        || row
            .last_seen_at
            .is_none_or(|seen| seen <= db::now() - OFFLINE_SECONDS as i64)
        || row
            .received_at
            .is_none_or(|seen| seen <= db::now() - (METRICS_SECONDS * 3) as i64);
    let snapshot = row
        .snapshot_json
        .map(|value| {
            serde_json::from_str::<MetricsSnapshot>(&value).map_err(|_| AppError::internal())
        })
        .transpose()?;
    Ok(Json(NodeMetrics {
        node_id,
        snapshot,
        received_at: row.received_at,
        stale,
    }))
}
