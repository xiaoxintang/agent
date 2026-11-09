use crate::{
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
use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use panel_common::{TaskCommand, TaskOperation, configuration::*};
use serde::{Deserialize, Serialize};
use sqlx::{Sqlite, Transaction};
use uuid::Uuid;

#[derive(Deserialize, Serialize)]
struct Reality {
    target: String,
    server_names: Vec<String>,
    short_ids: Vec<String>,
    private_key: String,
    public_key: String,
}
#[derive(sqlx::FromRow)]
struct InboundRow {
    id: String,
    node_id: String,
    name: String,
    listen_address: String,
    listen_port: u16,
    settings_json: String,
    enabled: bool,
}
#[derive(sqlx::FromRow)]
struct ClientRow {
    expiry_at: Option<i64>,
    quota_bytes: Option<i64>,
    used_bytes: i64,
    policy_blocked: bool,
    id: String,
    display_name: String,
    enabled: bool,
    created_at: i64,
}
#[derive(sqlx::FromRow)]
struct RevisionRow {
    id: String,
    node_id: String,
    revision: i64,
    content_hash: String,
    status: String,
    created_at: i64,
    applied_at: Option<i64>,
    task_id: Option<String>,
}

fn id(value: &str) -> Result<String, AppError> {
    Ok(Uuid::parse_str(value)
        .map_err(|_| AppError::invalid())?
        .to_string())
}
fn name(value: &str) -> Result<String, AppError> {
    let value = value.trim();
    if value.is_empty() || value.chars().count() > 64 || value.chars().any(char::is_control) {
        return Err(AppError::invalid());
    }
    Ok(value.into())
}
async fn active(pool: &sqlx::SqlitePool, node: &str) -> Result<(), AppError> {
    if !sqlx::query_scalar::<_, bool>(
        "SELECT EXISTS(SELECT 1 FROM nodes WHERE id=? AND revoked_at IS NULL)",
    )
    .bind(node)
    .fetch_one(pool)
    .await?
    {
        return Err(AppError::not_found());
    }
    Ok(())
}
fn sql_error(error: sqlx::Error) -> AppError {
    if matches!(&error,sqlx::Error::Database(error) if error.is_unique_violation()) {
        AppError::new(
            StatusCode::CONFLICT,
            "conflict",
            "Record conflicts with an existing record",
        )
    } else {
        error.into()
    }
}
async fn view(pool: &sqlx::SqlitePool, row: InboundRow) -> Result<InboundView, AppError> {
    let reality: Reality =
        serde_json::from_str(&row.settings_json).map_err(|_| AppError::internal())?;
    let client_ids=sqlx::query_scalar("SELECT client_id FROM client_assignments WHERE inbound_id=? AND enabled=1 ORDER BY client_id").bind(&row.id).fetch_all(pool).await?;
    Ok(InboundView {
        id: row.id,
        node_id: row.node_id,
        name: row.name,
        listen_address: row.listen_address,
        listen_port: row.listen_port,
        target: reality.target,
        server_names: reality.server_names,
        short_ids: reality.short_ids,
        public_key: reality.public_key,
        enabled: row.enabled,
        client_ids,
    })
}
async fn inbound(pool: &sqlx::SqlitePool, inbound_id: &str) -> Result<InboundRow, AppError> {
    sqlx::query_as::<_,InboundRow>("SELECT i.* FROM inbounds i JOIN nodes n ON n.id=i.node_id WHERE i.id=? AND n.revoked_at IS NULL").bind(inbound_id).fetch_optional(pool).await?.ok_or_else(AppError::not_found)
}
fn validate(input: &InboundInput, private_key: &str) -> Result<(), AppError> {
    NodeConfig {
        inbounds: vec![ConfigInbound {
            id: Uuid::new_v4().to_string(),
            listen_address: input.listen_address.clone(),
            listen_port: input.listen_port,
            target: input.target.clone(),
            server_names: input.server_names.clone(),
            short_ids: input.short_ids.clone(),
            private_key: private_key.into(),
            clients: vec![],
        }],
    }
    .render()
    .map_err(|_| AppError::invalid())?;
    Ok(())
}

pub async fn inbounds(
    State(state): State<AppState>,
    _: Session,
    Path(node): Path<String>,
) -> Result<Json<Vec<InboundView>>, AppError> {
    let node = id(&node)?;
    active(&state.pool, &node).await?;
    let rows = sqlx::query_as::<_, InboundRow>(
        "SELECT * FROM inbounds WHERE node_id=? ORDER BY id LIMIT 16",
    )
    .bind(node)
    .fetch_all(&state.pool)
    .await?;
    let mut result = Vec::new();
    for row in rows {
        result.push(view(&state.pool, row).await?);
    }
    Ok(Json(result))
}
pub async fn get_inbound(
    State(state): State<AppState>,
    _: Session,
    Path(value): Path<String>,
) -> Result<Json<InboundView>, AppError> {
    Ok(Json(
        view(&state.pool, inbound(&state.pool, &id(&value)?).await?).await?,
    ))
}
pub async fn create_inbound(
    State(state): State<AppState>,
    Extension(request): Extension<RequestId>,
    session: Session,
    Path(node): Path<String>,
    ApiJson(input): ApiJson<InboundInput>,
) -> Result<(StatusCode, Json<InboundView>), AppError> {
    let node = id(&node)?;
    active(&state.pool, &node).await?;
    let label = name(&input.name)?;
    let mut seed = [0_u8; 32];
    getrandom::fill(&mut seed).map_err(|_| AppError::internal())?;
    let secret = x25519_dalek::StaticSecret::from(seed);
    let public = x25519_dalek::PublicKey::from(&secret);
    let reality = Reality {
        target: input.target.clone(),
        server_names: input.server_names.clone(),
        short_ids: input.short_ids.clone(),
        private_key: URL_SAFE_NO_PAD.encode(seed),
        public_key: URL_SAFE_NO_PAD.encode(public.as_bytes()),
    };
    validate(&input, &reality.private_key)?;
    let value = Uuid::new_v4().to_string();
    let mut tx = state.pool.begin_with("BEGIN IMMEDIATE").await?;
    let changed=sqlx::query("INSERT INTO inbounds (id,node_id,name,listen_address,listen_port,settings_json,enabled,created_at,updated_at) SELECT ?,?,?,?,?,?,?,?,? WHERE (SELECT COUNT(*) FROM inbounds WHERE node_id=?)<16").bind(&value).bind(&node).bind(label).bind(input.listen_address).bind(input.listen_port).bind(serde_json::to_string(&reality).map_err(|_|AppError::internal())?).bind(input.enabled).bind(db::now()).bind(db::now()).bind(&node).execute(&mut *tx).await.map_err(sql_error)?;
    if changed.rows_affected() == 0 {
        return Err(AppError::new(
            StatusCode::CONFLICT,
            "inbound_limit",
            "At most 16 inbounds per node",
        ));
    }
    db::audit(
        &mut tx,
        Some(&session.admin.id),
        Some(&node),
        "inbound.create",
        &value,
        "success",
        Some(&request.0),
    )
    .await?;
    tx.commit().await?;
    Ok((
        StatusCode::CREATED,
        Json(view(&state.pool, inbound(&state.pool, &value).await?).await?),
    ))
}
pub async fn update_inbound(
    State(state): State<AppState>,
    Extension(request): Extension<RequestId>,
    session: Session,
    Path(value): Path<String>,
    ApiJson(input): ApiJson<InboundInput>,
) -> Result<Json<InboundView>, AppError> {
    let value = id(&value)?;
    let old = inbound(&state.pool, &value).await?;
    let mut reality: Reality =
        serde_json::from_str(&old.settings_json).map_err(|_| AppError::internal())?;
    validate(&input, &reality.private_key)?;
    reality.target = input.target;
    reality.server_names = input.server_names;
    reality.short_ids = input.short_ids;
    let mut tx = state.pool.begin().await?;
    sqlx::query("UPDATE inbounds SET name=?,listen_address=?,listen_port=?,settings_json=?,enabled=?,updated_at=? WHERE id=?").bind(name(&input.name)?).bind(input.listen_address).bind(input.listen_port).bind(serde_json::to_string(&reality).map_err(|_|AppError::internal())?).bind(input.enabled).bind(db::now()).bind(&value).execute(&mut *tx).await.map_err(sql_error)?;
    db::audit(
        &mut tx,
        Some(&session.admin.id),
        Some(&old.node_id),
        "inbound.update",
        &value,
        "success",
        Some(&request.0),
    )
    .await?;
    tx.commit().await?;
    Ok(Json(
        view(&state.pool, inbound(&state.pool, &value).await?).await?,
    ))
}
pub async fn delete_inbound(
    State(state): State<AppState>,
    Extension(request): Extension<RequestId>,
    session: Session,
    Path(value): Path<String>,
) -> Result<StatusCode, AppError> {
    let value = id(&value)?;
    let old = inbound(&state.pool, &value).await?;
    let mut tx = state.pool.begin().await?;
    sqlx::query("DELETE FROM inbounds WHERE id=?")
        .bind(&value)
        .execute(&mut *tx)
        .await?;
    db::audit(
        &mut tx,
        Some(&session.admin.id),
        Some(&old.node_id),
        "inbound.delete",
        &value,
        "success",
        Some(&request.0),
    )
    .await?;
    tx.commit().await?;
    Ok(StatusCode::NO_CONTENT)
}
fn validate_policy(input: &ClientInput) -> Result<Option<i64>, AppError> {
    if input
        .expiry_at
        .is_some_and(|value| !(0..=41_024_448_000).contains(&value))
    {
        return Err(AppError::invalid());
    }
    input
        .quota_bytes
        .as_ref()
        .map(|value| {
            value
                .parse::<i64>()
                .ok()
                .filter(|value| (1..=panel_common::traffic::MAX_COUNTER).contains(value))
                .ok_or_else(AppError::invalid)
        })
        .transpose()
}
fn client_view(row: ClientRow) -> ClientView {
    ClientView {
        id: row.id,
        display_name: row.display_name,
        enabled: row.enabled,
        created_at: row.created_at,
        expiry_at: row.expiry_at,
        quota_bytes: row.quota_bytes.map(|value| value.to_string()),
        used_bytes: row.used_bytes.to_string(),
        policy_blocked: row.policy_blocked,
    }
}
pub async fn clients(
    State(state): State<AppState>,
    _: Session,
) -> Result<Json<Vec<ClientView>>, AppError> {
    Ok(Json(
        sqlx::query_as::<_, ClientRow>("SELECT * FROM clients ORDER BY id LIMIT 1000")
            .fetch_all(&state.pool)
            .await?
            .into_iter()
            .map(client_view)
            .collect(),
    ))
}
pub async fn get_client(
    State(state): State<AppState>,
    _: Session,
    Path(value): Path<String>,
) -> Result<Json<ClientView>, AppError> {
    Ok(Json(client_view(
        sqlx::query_as::<_, ClientRow>("SELECT * FROM clients WHERE id=?")
            .bind(id(&value)?)
            .fetch_optional(&state.pool)
            .await?
            .ok_or_else(AppError::not_found)?,
    )))
}
pub async fn create_client(
    State(state): State<AppState>,
    Extension(request): Extension<RequestId>,
    session: Session,
    ApiJson(input): ApiJson<ClientInput>,
) -> Result<(StatusCode, Json<CreatedClient>), AppError> {
    let quota = validate_policy(&input)?;
    let value = Uuid::new_v4().to_string();
    let identity = Uuid::new_v4().to_string();
    let mut tx = state.pool.begin_with("BEGIN IMMEDIATE").await?;
    let changed=sqlx::query("INSERT INTO clients (id,display_name,protocol_identity_json,enabled,created_at,expiry_at,quota_bytes) SELECT ?,?,?,?,?,?,? WHERE (SELECT COUNT(*) FROM clients)<1000").bind(&value).bind(name(&input.display_name)?).bind(serde_json::to_string(&identity).map_err(|_|AppError::internal())?).bind(input.enabled).bind(db::now()).bind(input.expiry_at).bind(quota).execute(&mut *tx).await?;
    if changed.rows_affected() == 0 {
        return Err(AppError::new(
            StatusCode::CONFLICT,
            "client_limit",
            "At most 1000 clients",
        ));
    }
    db::audit(
        &mut tx,
        Some(&session.admin.id),
        None,
        "client.create",
        &value,
        "success",
        Some(&request.0),
    )
    .await?;
    let row = sqlx::query_as::<_, ClientRow>("SELECT * FROM clients WHERE id=?")
        .bind(value)
        .fetch_one(&mut *tx)
        .await?;
    tx.commit().await?;
    Ok((
        StatusCode::CREATED,
        Json(CreatedClient {
            client: client_view(row),
            protocol_id: identity,
        }),
    ))
}
pub async fn update_client(
    State(state): State<AppState>,
    Extension(request): Extension<RequestId>,
    session: Session,
    Path(value): Path<String>,
    ApiJson(input): ApiJson<ClientInput>,
) -> Result<Json<ClientView>, AppError> {
    let value = id(&value)?;
    let quota = validate_policy(&input)?;
    let mut tx = state.pool.begin().await?;
    if sqlx::query(
        "UPDATE clients SET display_name=?,enabled=?,expiry_at=?,quota_bytes=? WHERE id=?",
    )
    .bind(name(&input.display_name)?)
    .bind(input.enabled)
    .bind(input.expiry_at)
    .bind(quota)
    .bind(&value)
    .execute(&mut *tx)
    .await?
    .rows_affected()
        == 0
    {
        return Err(AppError::not_found());
    }
    db::audit(
        &mut tx,
        Some(&session.admin.id),
        None,
        "client.update",
        &value,
        "success",
        Some(&request.0),
    )
    .await?;
    let row = sqlx::query_as::<_, ClientRow>("SELECT * FROM clients WHERE id=?")
        .bind(value)
        .fetch_one(&mut *tx)
        .await?;
    tx.commit().await?;
    Ok(Json(client_view(row)))
}
pub async fn delete_client(
    State(state): State<AppState>,
    Extension(request): Extension<RequestId>,
    session: Session,
    Path(value): Path<String>,
) -> Result<StatusCode, AppError> {
    let value = id(&value)?;
    let mut tx = state.pool.begin().await?;
    if sqlx::query("DELETE FROM clients WHERE id=?")
        .bind(&value)
        .execute(&mut *tx)
        .await?
        .rows_affected()
        == 0
    {
        return Err(AppError::not_found());
    }
    db::audit(
        &mut tx,
        Some(&session.admin.id),
        None,
        "client.delete",
        &value,
        "success",
        Some(&request.0),
    )
    .await?;
    tx.commit().await?;
    Ok(StatusCode::NO_CONTENT)
}
pub async fn assign(
    State(state): State<AppState>,
    Extension(request): Extension<RequestId>,
    session: Session,
    Path((inbound_id, client_id)): Path<(String, String)>,
) -> Result<StatusCode, AppError> {
    assignment(state, request, session, inbound_id, client_id, true).await
}
pub async fn unassign(
    State(state): State<AppState>,
    Extension(request): Extension<RequestId>,
    session: Session,
    Path((inbound_id, client_id)): Path<(String, String)>,
) -> Result<StatusCode, AppError> {
    assignment(state, request, session, inbound_id, client_id, false).await
}
async fn assignment(
    state: AppState,
    request: RequestId,
    session: Session,
    inbound_id: String,
    client_id: String,
    enabled: bool,
) -> Result<StatusCode, AppError> {
    let inbound_id = id(&inbound_id)?;
    let client_id = id(&client_id)?;
    let row = inbound(&state.pool, &inbound_id).await?;
    let mut tx = state.pool.begin_with("BEGIN IMMEDIATE").await?;
    if !sqlx::query_scalar::<_, bool>("SELECT EXISTS(SELECT 1 FROM clients WHERE id=?)")
        .bind(&client_id)
        .fetch_one(&mut *tx)
        .await?
    {
        return Err(AppError::not_found());
    }
    if enabled {
        sqlx::query("INSERT INTO client_assignments(id,client_id,inbound_id,enabled) VALUES (?,?,?,1) ON CONFLICT(client_id,inbound_id) DO UPDATE SET enabled=1").bind(Uuid::new_v4().to_string()).bind(&client_id).bind(&inbound_id).execute(&mut *tx).await?;
    } else {
        sqlx::query("DELETE FROM client_assignments WHERE client_id=? AND inbound_id=?")
            .bind(&client_id)
            .bind(&inbound_id)
            .execute(&mut *tx)
            .await?;
    }
    db::audit(
        &mut tx,
        Some(&session.admin.id),
        Some(&row.node_id),
        if enabled {
            "client.assign"
        } else {
            "client.unassign"
        },
        &inbound_id,
        "success",
        Some(&request.0),
    )
    .await?;
    tx.commit().await?;
    Ok(StatusCode::NO_CONTENT)
}

async fn snapshot(tx: &mut Transaction<'_, Sqlite>, node: &str) -> Result<NodeConfig, AppError> {
    let rows = sqlx::query_as::<_, InboundRow>(
        "SELECT * FROM inbounds WHERE node_id=? AND enabled=1 ORDER BY id",
    )
    .bind(node)
    .fetch_all(&mut **tx)
    .await?;
    let mut inbounds = Vec::new();
    for row in rows {
        let reality: Reality =
            serde_json::from_str(&row.settings_json).map_err(|_| AppError::internal())?;
        let records=sqlx::query_as::<_,(String,String)>("SELECT c.id,c.protocol_identity_json FROM clients c JOIN client_assignments a ON a.client_id=c.id WHERE a.inbound_id=? AND a.enabled=1 AND c.enabled=1 AND c.policy_blocked=0 AND (c.expiry_at IS NULL OR c.expiry_at>unixepoch()) AND (c.quota_bytes IS NULL OR c.used_bytes<c.quota_bytes) ORDER BY c.id").bind(&row.id).fetch_all(&mut **tx).await?;
        let mut clients = Vec::new();
        for (id, identity) in records {
            clients.push(ConfigClient {
                id,
                protocol_id: serde_json::from_str(&identity).map_err(|_| AppError::internal())?,
            });
        }
        inbounds.push(ConfigInbound {
            id: row.id,
            listen_address: row.listen_address,
            listen_port: row.listen_port,
            target: reality.target,
            server_names: reality.server_names,
            private_key: reality.private_key,
            short_ids: reality.short_ids,
            clients,
        });
    }
    let config = NodeConfig { inbounds };
    config.render().map_err(|_| AppError::invalid())?;
    Ok(config)
}
fn revision(row: RevisionRow) -> RevisionView {
    RevisionView {
        id: row.id,
        node_id: row.node_id,
        revision: row.revision,
        content_hash: row.content_hash,
        status: row.status,
        created_at: row.created_at,
        applied_at: row.applied_at,
        task_id: row.task_id,
    }
}
pub async fn revisions(
    State(state): State<AppState>,
    _: Session,
    Path(node): Path<String>,
) -> Result<Json<Vec<RevisionView>>, AppError> {
    let node = id(&node)?;
    active(&state.pool, &node).await?;
    crate::tasks::expire(&state.pool, &node).await?;
    Ok(Json(
        sqlx::query_as::<_, RevisionRow>(
            "SELECT * FROM config_revisions WHERE node_id=? ORDER BY revision DESC LIMIT 20",
        )
        .bind(node)
        .fetch_all(&state.pool)
        .await?
        .into_iter()
        .map(revision)
        .collect(),
    ))
}
pub async fn apply(
    State(state): State<AppState>,
    Extension(request): Extension<RequestId>,
    session: Session,
    Path(node): Path<String>,
    ApiJson(input): ApiJson<ApplyInput>,
) -> Result<(StatusCode, Json<RevisionView>), AppError> {
    let node = id(&node)?;
    active(&state.pool, &node).await?;
    let key = id(&input.idempotency_key)?;
    crate::tasks::expire(&state.pool, &node).await?;
    let mut tx = state.pool.begin_with("BEGIN IMMEDIATE").await?;
    if let Some(row)=sqlx::query_as::<_,RevisionRow>("SELECT r.* FROM config_revisions r JOIN tasks t ON t.id=r.task_id WHERE r.node_id=? AND t.idempotency_key=?").bind(&node).bind(&key).fetch_optional(&mut *tx).await?{tx.commit().await?;return Ok((StatusCode::OK,Json(revision(row))));}
    let config = snapshot(&mut tx, &node).await?;
    let row = enqueue(&mut tx, &node, config, &key).await?;
    db::audit(
        &mut tx,
        Some(&session.admin.id),
        Some(&node),
        "config.apply",
        &row.id,
        "success",
        Some(&request.0),
    )
    .await?;
    tx.commit().await?;
    state.wake_agent(&node);
    Ok((StatusCode::CREATED, Json(revision(row))))
}

async fn enqueue(
    tx: &mut Transaction<'_, Sqlite>,
    node: &str,
    config: NodeConfig,
    key: &str,
) -> Result<RevisionRow, AppError> {
    let number = sqlx::query_scalar::<_, i64>(
        "SELECT COALESCE(MAX(revision),0)+1 FROM config_revisions WHERE node_id=?",
    )
    .bind(node)
    .fetch_one(&mut **tx)
    .await?;
    let reference = ConfigRef {
        revision: number,
        content_hash: config.content_hash().map_err(|_| AppError::invalid())?,
    };
    sqlx::query("UPDATE tasks SET status='canceled',finished_at=? WHERE node_id=? AND operation='apply_config' AND status='pending'").bind(db::now()).bind(node).execute(&mut **tx).await?;
    sqlx::query("UPDATE config_revisions SET status='failed' WHERE node_id=? AND status='pending' AND task_id IN (SELECT id FROM tasks WHERE status='canceled')").bind(node).execute(&mut **tx).await?;
    let command = TaskCommand {
        id: Uuid::new_v4().to_string(),
        operation: TaskOperation::ApplyConfig(reference.clone()),
        deadline_at: db::now() + 300,
        private_config: Some(config.clone()),
    };
    let payload = serde_json::to_string(&command).map_err(|_| AppError::internal())?;
    if payload.len() > 60 * 1024 {
        return Err(AppError::invalid());
    }
    sqlx::query("INSERT INTO tasks(id,node_id,operation,payload_json,idempotency_key,desired_revision,status,created_at,deadline_at) VALUES (?,?,'apply_config',?,?,?,'pending',?,?)").bind(&command.id).bind(node).bind(payload).bind(key).bind(number).bind(db::now()).bind(command.deadline_at).execute(&mut **tx).await?;
    let id = Uuid::new_v4().to_string();
    sqlx::query("INSERT INTO config_revisions(id,node_id,revision,desired_config_json,content_hash,status,created_at,task_id) VALUES (?,?,?,?,?,'pending',?,?)").bind(&id).bind(node).bind(number).bind(serde_json::to_string(&config).map_err(|_|AppError::internal())?).bind(&reference.content_hash).bind(db::now()).bind(&command.id).execute(&mut **tx).await?;
    sqlx::query(
        "DELETE FROM config_revisions WHERE node_id=? AND revision<? AND status!='pending'",
    )
    .bind(node)
    .bind(number - 19)
    .execute(&mut **tx)
    .await?;
    Ok(
        sqlx::query_as::<_, RevisionRow>("SELECT * FROM config_revisions WHERE id=?")
            .bind(id)
            .fetch_one(&mut **tx)
            .await?,
    )
}

pub(crate) async fn queue_policy(
    tx: &mut Transaction<'_, Sqlite>,
    node: &str,
    config: NodeConfig,
) -> Result<(), AppError> {
    let row = enqueue(tx, node, config, &Uuid::new_v4().to_string()).await?;
    db::audit(
        tx,
        None,
        Some(node),
        "config.policy",
        &row.id,
        "success",
        None,
    )
    .await
}

/// Requeue only the latest desired revision when its delivery deadline elapsed offline.
pub async fn reconcile(pool: &sqlx::SqlitePool, node: &str) -> Result<(), AppError> {
    crate::tasks::expire(pool, node).await?;
    let mut tx = pool.begin_with("BEGIN IMMEDIATE").await?;
    let latest = sqlx::query_as::<_, (String, i64, String, String, Option<String>)>("SELECT r.id,r.revision,r.content_hash,r.desired_config_json,t.status FROM config_revisions r LEFT JOIN tasks t ON t.id=r.task_id WHERE r.node_id=? ORDER BY r.revision DESC LIMIT 1").bind(node).fetch_optional(&mut *tx).await?;
    if let Some((id, revision, content_hash, private_json, Some(status))) = latest
        && status == "expired"
    {
        let command = TaskCommand {
            id: Uuid::new_v4().to_string(),
            operation: TaskOperation::ApplyConfig(ConfigRef {
                revision,
                content_hash,
            }),
            deadline_at: db::now() + 300,
            private_config: Some(
                serde_json::from_str(&private_json).map_err(|_| AppError::internal())?,
            ),
        };
        sqlx::query("INSERT INTO tasks(id,node_id,operation,payload_json,idempotency_key,desired_revision,status,created_at,deadline_at) VALUES (?,?,'apply_config',?,?,?,'pending',?,?)").bind(&command.id).bind(node).bind(serde_json::to_string(&command).map_err(|_| AppError::internal())?).bind(Uuid::new_v4().to_string()).bind(revision).bind(db::now()).bind(command.deadline_at).execute(&mut *tx).await?;
        sqlx::query("UPDATE config_revisions SET task_id=?,status='pending' WHERE id=?")
            .bind(&command.id)
            .bind(id)
            .execute(&mut *tx)
            .await?;
        db::audit(
            &mut tx,
            None,
            Some(node),
            "config.reconcile",
            &command.id,
            "success",
            None,
        )
        .await?;
    }
    tx.commit().await?;
    Ok(())
}
