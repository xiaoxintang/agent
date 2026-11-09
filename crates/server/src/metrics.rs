use crate::{api::NodeId, auth::Session, db, error::AppError, state::AppState};
use axum::{Json, extract::State};
use panel_common::{
    MetricsSnapshot, NodeMetrics, NodeMetricsSummary, NodeSummary, protocol::OFFLINE_SECONDS,
    telemetry::METRICS_SECONDS,
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

impl Row {
    fn metrics(self, node_id: String) -> Result<NodeMetrics, AppError> {
        let stale = self.status != "online"
            || self.connection_id != self.metrics_connection
            || self
                .last_seen_at
                .is_none_or(|seen| seen <= db::now() - OFFLINE_SECONDS as i64)
            || self
                .received_at
                .is_none_or(|seen| seen <= db::now() - (METRICS_SECONDS * 3) as i64);
        let snapshot = self
            .snapshot_json
            .map(|value| {
                serde_json::from_str::<MetricsSnapshot>(&value).map_err(|_| AppError::internal())
            })
            .transpose()?;
        Ok(NodeMetrics {
            node_id,
            snapshot,
            received_at: self.received_at,
            stale,
        })
    }
}

#[derive(sqlx::FromRow)]
struct SummaryRow {
    #[sqlx(flatten)]
    node: crate::api::NodeRow,
    #[sqlx(flatten)]
    metrics: Row,
}

pub async fn summaries(
    State(state): State<AppState>,
    _: Session,
) -> Result<Json<Vec<NodeSummary>>, AppError> {
    let rows = sqlx::query_as::<_, SummaryRow>("SELECT n.*, m.connection_id AS metrics_connection, m.snapshot_json, m.received_at FROM nodes n LEFT JOIN node_metrics m ON m.node_id=n.id WHERE n.revoked_at IS NULL ORDER BY n.created_at DESC, n.id LIMIT 100")
        .fetch_all(&state.pool).await?;
    let summaries = rows
        .into_iter()
        .map(|row| {
            let node: panel_common::Node = row.node.into();
            let metrics = row.metrics.metrics(node.id.clone())?;
            Ok(NodeSummary {
                node,
                metrics: NodeMetricsSummary {
                    cpu_percent: metrics
                        .snapshot
                        .as_ref()
                        .and_then(|snapshot| snapshot.cpu_percent),
                    memory: metrics.snapshot.and_then(|snapshot| snapshot.memory),
                    received_at: metrics.received_at,
                    stale: metrics.stale,
                },
            })
        })
        .collect::<Result<Vec<_>, AppError>>()?;
    Ok(Json(summaries))
}

pub async fn get(
    State(state): State<AppState>,
    _: Session,
    NodeId(node_id): NodeId,
) -> Result<Json<NodeMetrics>, AppError> {
    let row = sqlx::query_as::<_, Row>("SELECT n.status, n.last_seen_at, n.connection_id, m.connection_id AS metrics_connection, m.snapshot_json, m.received_at FROM nodes n LEFT JOIN node_metrics m ON m.node_id = n.id WHERE n.id = ? AND n.revoked_at IS NULL")
        .bind(&node_id).fetch_optional(&state.pool).await?.ok_or_else(AppError::not_found)?;
    Ok(Json(row.metrics(node_id)?))
}
