use crate::{
    app::RequestId,
    db,
    error::{ApiJson, AppError},
    state::AppState,
};
use argon2::{
    Argon2,
    password_hash::{PasswordHasher, PasswordVerifier, phc::PasswordHash},
};
use axum::{
    Extension, Json,
    extract::{FromRequestParts, State},
    http::{HeaderMap, StatusCode, header, request::Parts},
    response::{IntoResponse, Response},
};
use panel_common::{AdminUser, LoginRequest, SessionResponse};
use sha2::{Digest, Sha256};
use sqlx::SqlitePool;
use subtle::ConstantTimeEq;
use uuid::Uuid;

pub const SESSION_SECONDS: i64 = 3600;

pub fn random_token() -> String {
    format!("{}{}", Uuid::new_v4().simple(), Uuid::new_v4().simple())
}

pub fn token_hash(value: &str) -> String {
    hex::encode(Sha256::digest(value.as_bytes()))
}

pub async fn hash_password(password: String) -> Result<String, AppError> {
    tokio::task::spawn_blocking(move || {
        Argon2::default()
            .hash_password(password.as_bytes())
            .map(|hash| hash.to_string())
            .map_err(|_| AppError::internal())
    })
    .await
    .map_err(|_| AppError::internal())?
}

pub async fn bootstrap_admin(
    pool: &SqlitePool,
    username: &str,
    password: String,
) -> Result<(), AppError> {
    if !valid_username(username)
        || !(12..=128).contains(&password.len())
        || password.chars().any(char::is_control)
    {
        return Err(AppError::invalid());
    }
    let hash = hash_password(password).await?;
    let id = Uuid::new_v4().to_string();
    let mut tx = pool.begin().await?;
    let result = sqlx::query(
        "INSERT INTO admins (id, username, password_hash, created_at) VALUES (?, ?, ?, ?)",
    )
    .bind(&id)
    .bind(username)
    .bind(hash)
    .bind(db::now())
    .execute(&mut *tx)
    .await;
    match result {
        Ok(_) => (),
        Err(sqlx::Error::Database(error)) if error.is_unique_violation() => {
            return Err(AppError::new(
                StatusCode::CONFLICT,
                "already_initialized",
                "Administrator already initialized",
            ));
        }
        Err(error) => return Err(error.into()),
    }
    db::audit(
        &mut tx,
        Some(&id),
        None,
        "admin.bootstrap",
        &id,
        "success",
        None,
    )
    .await?;
    tx.commit().await?;
    Ok(())
}

fn valid_username(value: &str) -> bool {
    (3..=64).contains(&value.len())
        && value
            .bytes()
            .all(|c| c.is_ascii_alphanumeric() || b"._-".contains(&c))
}

fn cookie_token(headers: &HeaderMap, state: &AppState) -> Option<String> {
    let mut tokens = headers
        .get_all(header::COOKIE)
        .iter()
        .filter_map(|header| header.to_str().ok())
        .flat_map(|value| value.split(';'))
        .filter_map(|cookie| cookie.trim().split_once('='))
        .filter(|(name, _)| *name == state.security.cookie_name())
        .map(|(_, value)| value);
    let value = tokens.next()?;
    if tokens.next().is_some() || value.len() != 64 || !value.bytes().all(|c| c.is_ascii_hexdigit())
    {
        return None;
    }
    Some(value.to_owned())
}

fn require_origin(headers: &HeaderMap, state: &AppState) -> Result<(), AppError> {
    let mut origins = headers.get_all(header::ORIGIN).iter();
    let origin = origins.next().and_then(|header| header.to_str().ok());
    if origins.next().is_some()
        || origin != Some(state.security.origin.as_str())
        || headers
            .get("sec-fetch-site")
            .is_some_and(|value| value == "cross-site")
    {
        return Err(AppError::forbidden());
    }
    Ok(())
}

pub struct LoginGuard;
impl FromRequestParts<AppState> for LoginGuard {
    type Rejection = AppError;
    async fn from_request_parts(
        parts: &mut Parts,
        state: &AppState,
    ) -> Result<Self, Self::Rejection> {
        require_origin(&parts.headers, state)?;
        state.security.login_attempt()?;
        Ok(Self)
    }
}

pub struct Session {
    pub admin: AdminUser,
    pub id: String,
    pub csrf_token: String,
    pub expires_at: i64,
}

#[derive(sqlx::FromRow)]
struct SessionRow {
    id: String,
    admin_id: String,
    username: String,
    expires_at: i64,
}

impl FromRequestParts<AppState> for Session {
    type Rejection = AppError;
    async fn from_request_parts(
        parts: &mut Parts,
        state: &AppState,
    ) -> Result<Self, Self::Rejection> {
        let token = cookie_token(&parts.headers, state).ok_or_else(AppError::unauthorized)?;
        let row = sqlx::query_as::<_, SessionRow>("SELECT s.id, s.admin_id, a.username, s.expires_at FROM sessions s JOIN admins a ON a.id = s.admin_id WHERE s.token_hash = ? AND s.expires_at > ? AND s.revoked_at IS NULL AND a.disabled_at IS NULL")
            .bind(token_hash(&token)).bind(db::now()).fetch_optional(&state.pool).await?.ok_or_else(AppError::unauthorized)?;
        let csrf_token = token_hash(&format!("csrf:v1:{token}"));
        if !parts.method.is_safe() {
            require_origin(&parts.headers, state)?;
            let mut headers = parts.headers.get_all("x-csrf-token").iter();
            let supplied = headers
                .next()
                .and_then(|value| value.to_str().ok())
                .ok_or_else(AppError::forbidden)?;
            if headers.next().is_some()
                || supplied.len() != csrf_token.len()
                || !bool::from(supplied.as_bytes().ct_eq(csrf_token.as_bytes()))
            {
                return Err(AppError::forbidden());
            }
        }
        Ok(Self {
            admin: AdminUser {
                id: row.admin_id,
                username: row.username,
            },
            id: row.id,
            csrf_token,
            expires_at: row.expires_at,
        })
    }
}

#[derive(sqlx::FromRow)]
struct LoginRow {
    id: String,
    username: String,
    password_hash: String,
    disabled_at: Option<i64>,
}

pub async fn login(
    State(state): State<AppState>,
    Extension(request_id): Extension<RequestId>,
    headers: HeaderMap,
    _: LoginGuard,
    ApiJson(input): ApiJson<LoginRequest>,
) -> Result<Response, AppError> {
    if input.password.len() > 128 || input.username.len() > 64 {
        return Err(AppError::invalid());
    }
    let permit = state
        .security
        .password_slots
        .clone()
        .try_acquire_owned()
        .map_err(|_| AppError::limited())?;
    let row = sqlx::query_as::<_, LoginRow>(
        "SELECT id, username, password_hash, disabled_at FROM admins WHERE username = ?",
    )
    .bind(&input.username)
    .fetch_optional(&state.pool)
    .await?;
    let hash = row
        .as_ref()
        .map(|row| &row.password_hash)
        .unwrap_or(&state.security.dummy_hash)
        .clone();
    let valid = tokio::task::spawn_blocking(move || {
        let _permit = permit;
        let parsed = PasswordHash::new(&hash).map_err(|_| AppError::internal())?;
        Ok::<_, AppError>(
            Argon2::default()
                .verify_password(input.password.as_bytes(), &parsed)
                .is_ok(),
        )
    })
    .await
    .map_err(|_| AppError::internal())??;
    let mut tx = state.pool.begin_with("BEGIN IMMEDIATE").await?;
    let Some(admin) = row.filter(|row| valid && row.disabled_at.is_none()) else {
        db::audit(
            &mut tx,
            None,
            None,
            "auth.login",
            "session",
            "denied",
            Some(&request_id.0),
        )
        .await?;
        tx.commit().await?;
        return Err(AppError::unauthorized());
    };
    let still_enabled: bool = sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM admins WHERE id = ? AND disabled_at IS NULL)",
    )
    .bind(&admin.id)
    .fetch_one(&mut *tx)
    .await?;
    if !still_enabled {
        return Err(AppError::unauthorized());
    }
    if let Some(old) = cookie_token(&headers, &state) {
        sqlx::query(
            "UPDATE sessions SET revoked_at = ? WHERE token_hash = ? AND revoked_at IS NULL",
        )
        .bind(db::now())
        .bind(token_hash(&old))
        .execute(&mut *tx)
        .await?;
    }
    sqlx::query("DELETE FROM sessions WHERE expires_at <= ? OR revoked_at IS NOT NULL")
        .bind(db::now())
        .execute(&mut *tx)
        .await?;
    let token = random_token();
    let expires_at = db::now() + SESSION_SECONDS;
    sqlx::query("INSERT INTO sessions (id, admin_id, token_hash, expires_at, created_at) VALUES (?, ?, ?, ?, ?)")
        .bind(Uuid::new_v4().to_string()).bind(&admin.id).bind(token_hash(&token)).bind(expires_at).bind(db::now()).execute(&mut *tx).await?;
    db::audit(
        &mut tx,
        Some(&admin.id),
        None,
        "auth.login",
        "session",
        "success",
        Some(&request_id.0),
    )
    .await?;
    tx.commit().await?;
    Ok((
        [(
            header::SET_COOKIE,
            state.security.cookie(&token, SESSION_SECONDS),
        )],
        Json(SessionResponse {
            admin: AdminUser {
                id: admin.id,
                username: admin.username,
            },
            csrf_token: token_hash(&format!("csrf:v1:{token}")),
            expires_at,
        }),
    )
        .into_response())
}

pub async fn me(session: Session) -> Json<SessionResponse> {
    Json(SessionResponse {
        admin: session.admin,
        csrf_token: session.csrf_token,
        expires_at: session.expires_at,
    })
}

pub async fn logout(
    State(state): State<AppState>,
    Extension(request_id): Extension<RequestId>,
    session: Session,
) -> Result<Response, AppError> {
    let mut tx = state.pool.begin().await?;
    sqlx::query("UPDATE sessions SET revoked_at = ? WHERE id = ?")
        .bind(db::now())
        .bind(&session.id)
        .execute(&mut *tx)
        .await?;
    db::audit(
        &mut tx,
        Some(&session.admin.id),
        None,
        "auth.logout",
        "session",
        "success",
        Some(&request_id.0),
    )
    .await?;
    tx.commit().await?;
    Ok((
        StatusCode::NO_CONTENT,
        [(header::SET_COOKIE, state.security.cookie("", 0))],
    )
        .into_response())
}
