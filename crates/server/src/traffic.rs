use crate::{api::NodeId, auth::Session, configuration, db, error::AppError, state::AppState};
use axum::{Json, extract::State};
use panel_common::{
    configuration::NodeConfig,
    traffic::{
        ClientTraffic, MAX_COUNTER, NodeTraffic, TRAFFIC_SECONDS, TrafficBucket, TrafficReport,
    },
};
use sqlx::{QueryBuilder, Sqlite, SqlitePool, Transaction};
use std::collections::BTreeSet;

pub(crate) async fn record(
    state: &AppState,
    node: &str,
    generation: &str,
    report: &TrafficReport,
) -> Result<(), AppError> {
    let pool = &state.pool;
    let mut tx = pool.begin_with("BEGIN IMMEDIATE").await?;
    let mut affected_clients = BTreeSet::new();
    let current = sqlx::query_scalar::<_, bool>(
        "SELECT EXISTS(SELECT 1 FROM nodes WHERE id=? AND connection_id=? AND revoked_at IS NULL)",
    )
    .bind(node)
    .bind(generation)
    .fetch_one(&mut *tx)
    .await?;
    if !current {
        return Err(AppError::unauthorized());
    }
    let (available, collected) = match report {
        TrafficReport::Available(sample) => {
            if !sample.valid(db::now()) {
                return Err(AppError::invalid());
            }
            (true, Some(sample.collected_at))
        }
        TrafficReport::Unavailable => (false, None),
    };
    sqlx::query("INSERT INTO node_traffic_state(node_id,connection_id,available,received_at,collected_at) VALUES (?,?,?,?,?) ON CONFLICT(node_id) DO UPDATE SET connection_id=excluded.connection_id,available=excluded.available,received_at=excluded.received_at,collected_at=excluded.collected_at")
        .bind(node).bind(generation).bind(available).bind(db::now()).bind(collected).execute(&mut *tx).await?;
    if let TrafficReport::Available(sample) = report {
        for counter in &sample.counters {
            let assigned=sqlx::query_scalar::<_,bool>("SELECT EXISTS(SELECT 1 FROM client_assignments a JOIN inbounds i ON i.id=a.inbound_id WHERE a.client_id=? AND i.node_id=?)").bind(&counter.client_id).bind(node).fetch_one(&mut *tx).await?;
            if !assigned {
                continue;
            }
            let old=sqlx::query_as::<_,(String,i64,i64,i64)>("SELECT instance_id,collected_at,uplink_bytes,downlink_bytes FROM traffic_baselines WHERE node_id=? AND client_id=?").bind(node).bind(&counter.client_id).fetch_optional(&mut *tx).await?;
            if old
                .as_ref()
                .is_some_and(|(_, at, _, _)| *at >= sample.collected_at)
            {
                continue;
            }
            let (up, down) = panel_common::traffic::delta(
                old.as_ref()
                    .map(|(instance, at, up, down)| (instance.as_str(), *at, *up, *down)),
                sample,
                counter,
            );
            sqlx::query("INSERT INTO traffic_baselines(node_id,client_id,instance_id,collected_at,uplink_bytes,downlink_bytes) VALUES (?,?,?,?,?,?) ON CONFLICT(node_id,client_id) DO UPDATE SET instance_id=excluded.instance_id,collected_at=excluded.collected_at,uplink_bytes=excluded.uplink_bytes,downlink_bytes=excluded.downlink_bytes")
                .bind(node).bind(&counter.client_id).bind(&sample.instance_id).bind(sample.collected_at).bind(counter.uplink_bytes).bind(counter.downlink_bytes).execute(&mut *tx).await?;
            if up == 0 && down == 0 {
                continue;
            }
            sqlx::query("UPDATE clients SET used_bytes=MIN(used_bytes+?,?) WHERE id=?")
                .bind(up + down)
                .bind(MAX_COUNTER)
                .bind(&counter.client_id)
                .execute(&mut *tx)
                .await?;
            affected_clients.insert(counter.client_id.clone());
            sqlx::query("INSERT INTO traffic_totals(node_id,client_id,uplink_bytes,downlink_bytes) VALUES (?,?,?,?) ON CONFLICT(node_id,client_id) DO UPDATE SET uplink_bytes=MIN(uplink_bytes+excluded.uplink_bytes,9007199254740991),downlink_bytes=MIN(downlink_bytes+excluded.downlink_bytes,9007199254740991)")
                .bind(node).bind(&counter.client_id).bind(up).bind(down).execute(&mut *tx).await?;
            // Bucket by server receipt time: delayed samples never rewrite past accounting windows.
            sqlx::query("INSERT INTO traffic_aggregates(node_id,client_id,bucket_start,uplink_bytes,downlink_bytes) VALUES (?,?,?,?,?) ON CONFLICT(node_id,client_id,bucket_start) DO UPDATE SET uplink_bytes=MIN(uplink_bytes+excluded.uplink_bytes,9007199254740991),downlink_bytes=MIN(downlink_bytes+excluded.downlink_bytes,9007199254740991)")
                .bind(node).bind(&counter.client_id).bind(db::now()/3600*3600).bind(up).bind(down).execute(&mut *tx).await?;
        }
    }
    let affected_nodes = if affected_clients.is_empty() {
        Vec::new()
    } else {
        enforce_clients(&mut tx, Some(&affected_clients)).await?
    };
    tx.commit().await?;
    for node in affected_nodes {
        state.wake_agent(&node);
    }
    Ok(())
}

/// Policy removes users only from the last submitted desired snapshot; drafts stay untouched.
pub async fn enforce(pool: &SqlitePool) -> Result<Vec<String>, AppError> {
    let mut tx = pool.begin_with("BEGIN IMMEDIATE").await?;
    let nodes = enforce_clients(&mut tx, None).await?;
    tx.commit().await?;
    Ok(nodes)
}

async fn enforce_clients(
    tx: &mut Transaction<'_, Sqlite>,
    candidates: Option<&BTreeSet<String>>,
) -> Result<Vec<String>, AppError> {
    let now = db::now();
    let mut query =
        QueryBuilder::<Sqlite>::new("SELECT id, ((expiry_at IS NOT NULL AND expiry_at<=");
    query.push_bind(now).push(") OR (quota_bytes IS NOT NULL AND used_bytes>=quota_bytes)) FROM clients WHERE policy_blocked != ((expiry_at IS NOT NULL AND expiry_at<=");
    query
        .push_bind(now)
        .push(") OR (quota_bytes IS NOT NULL AND used_bytes>=quota_bytes))");
    if let Some(candidates) = candidates {
        query.push(" AND id IN (");
        let mut ids = query.separated(",");
        for client in candidates {
            ids.push_bind(client);
        }
        ids.push_unseparated(")");
    }
    let changed = query
        .build_query_as::<(String, bool)>()
        .fetch_all(&mut **tx)
        .await?;
    let mut blocked_clients = BTreeSet::new();
    for (client, blocked) in changed {
        sqlx::query("UPDATE clients SET policy_blocked=? WHERE id=?")
            .bind(blocked)
            .bind(&client)
            .execute(&mut **tx)
            .await?;
        db::audit(
            tx,
            None,
            None,
            if blocked {
                "client.policy_block"
            } else {
                "client.policy_clear"
            },
            &client,
            "success",
            None,
        )
        .await?;
        if blocked {
            blocked_clients.insert(client);
        }
    }
    let mut nodes = Vec::new();
    if !blocked_clients.is_empty() {
        // Match committed snapshots rather than draft assignments: an unapplied
        // unassignment must not hide a client that is still running on a node.
        let mut query = QueryBuilder::<Sqlite>::new(
            "SELECT r.node_id,r.desired_config_json FROM config_revisions r JOIN nodes n ON n.id=r.node_id WHERE n.revoked_at IS NULL AND r.revision=(SELECT MAX(revision) FROM config_revisions WHERE node_id=r.node_id) AND EXISTS (SELECT 1 FROM json_each(r.desired_config_json,'$.inbounds') i, json_each(i.value,'$.clients') c WHERE json_extract(c.value,'$.id') IN (",
        );
        let mut ids = query.separated(",");
        for client in &blocked_clients {
            ids.push_bind(client);
        }
        ids.push_unseparated("))");
        let snapshots = query
            .build_query_as::<(String, String)>()
            .fetch_all(&mut **tx)
            .await?;
        for (node, private) in snapshots {
            let mut config: NodeConfig =
                serde_json::from_str(&private).map_err(|_| AppError::internal())?;
            let before: usize = config
                .inbounds
                .iter()
                .map(|inbound| inbound.clients.len())
                .sum();
            for inbound in &mut config.inbounds {
                inbound
                    .clients
                    .retain(|client| !blocked_clients.contains(&client.id));
            }
            if config
                .inbounds
                .iter()
                .map(|inbound| inbound.clients.len())
                .sum::<usize>()
                != before
            {
                configuration::queue_policy(tx, &node, config).await?;
                nodes.push(node);
            }
        }
    }
    Ok(nodes)
}

pub async fn run(state: AppState, mut shutdown: tokio::sync::watch::Receiver<bool>) {
    let mut tick = tokio::time::interval(std::time::Duration::from_secs(5));
    tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    loop {
        if *shutdown.borrow() {
            return;
        }
        tokio::select! {
            _=shutdown.changed()=>return,
            _=tick.tick()=>match enforce(&state.pool).await {
                Ok(nodes) => for node in nodes { state.wake_agent(&node); },
                Err(error) => crate::diagnostics::report("policy_error", &error, None, None, None),
            }
        }
    }
}

pub async fn get(
    State(state): State<AppState>,
    _: Session,
    NodeId(node): NodeId,
) -> Result<Json<NodeTraffic>, AppError> {
    let (status, connection) = sqlx::query_as::<_, (String, Option<String>)>(
        "SELECT status,connection_id FROM nodes WHERE id=? AND revoked_at IS NULL",
    )
    .bind(&node)
    .fetch_optional(&state.pool)
    .await?
    .ok_or_else(AppError::not_found)?;
    let observed = sqlx::query_as::<_, (bool, i64, String)>(
        "SELECT available,received_at,connection_id FROM node_traffic_state WHERE node_id=?",
    )
    .bind(&node)
    .fetch_optional(&state.pool)
    .await?;
    let available = observed
        .as_ref()
        .is_some_and(|(available, _, _)| *available);
    let stale = status != "online"
        || observed.as_ref().is_none_or(|(_, at, generation)| {
            connection.as_ref() != Some(generation)
                || *at < db::now() - (TRAFFIC_SECONDS * 3) as i64
        });
    let buckets=sqlx::query_as::<_,(i64,i64,i64)>("SELECT bucket_start,SUM(uplink_bytes),SUM(downlink_bytes) FROM traffic_aggregates WHERE node_id=? AND bucket_start>=? GROUP BY bucket_start ORDER BY bucket_start LIMIT 168").bind(&node).bind(db::now()-7*86400).fetch_all(&state.pool).await?.into_iter().map(|(bucket_start,up,down)|TrafficBucket{bucket_start,uplink_bytes:up.to_string(),downlink_bytes:down.to_string()}).collect();
    let clients=sqlx::query_as::<_,(String,String,i64,i64)>("SELECT c.id,c.display_name,t.uplink_bytes,t.downlink_bytes FROM traffic_totals t JOIN clients c ON c.id=t.client_id WHERE t.node_id=? ORDER BY c.id LIMIT 1000").bind(&node).fetch_all(&state.pool).await?.into_iter().map(|(client_id,display_name,up,down)|ClientTraffic{client_id,display_name,uplink_bytes:up.to_string(),downlink_bytes:down.to_string()}).collect();
    Ok(Json(NodeTraffic {
        available,
        stale,
        received_at: observed.map(|(_, at, _)| at),
        buckets,
        clients,
    }))
}

#[cfg(test)]
mod tests;
