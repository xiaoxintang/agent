use super::Web;
use crate::{
    error::{Error, Result},
    store::now,
    web_config::{self, Credentials},
};
use axum::{
    Json,
    extract::{Request, State},
    http::{StatusCode, header},
    middleware::Next,
    response::{IntoResponse, Response},
};
use serde::Deserialize;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{
    sync::Arc,
    time::{Duration, Instant},
};

pub(super) fn hash(value: &str) -> String {
    hex::encode(Sha256::digest(value.as_bytes()))
}
pub(super) fn failure(status: StatusCode, message: &str) -> Response {
    (status, Json(json!({"error":message}))).into_response()
}

pub(super) async fn browser_policy(request: Request, next: Next) -> Response {
    if request.uri().path().starts_with("/api/") && request.method() != axum::http::Method::GET {
        if request
            .headers()
            .get("x-web-request")
            .and_then(|v| v.to_str().ok())
            != Some("1")
        {
            return failure(StatusCode::FORBIDDEN, "missing same-origin request header");
        }
        if let Some(origin) = request
            .headers()
            .get(header::ORIGIN)
            .and_then(|v| v.to_str().ok())
        {
            let host = request
                .headers()
                .get(header::HOST)
                .and_then(|v| v.to_str().ok())
                .unwrap_or_default();
            if origin != format!("http://{host}") && origin != format!("https://{host}") {
                return failure(StatusCode::FORBIDDEN, "cross-origin request rejected");
            }
        }
    }
    if let Some(version) = request
        .headers()
        .get("x-web-version")
        .and_then(|v| v.to_str().ok())
    {
        if version != env!("CARGO_PKG_VERSION") {
            return failure(
                StatusCode::CONFLICT,
                "Web version changed; refresh this page",
            );
        }
    }
    let mut response = next.run(request).await;
    response.headers_mut().insert(
        header::CACHE_CONTROL,
        axum::http::HeaderValue::from_static("no-store"),
    );
    response.headers_mut().insert(
        header::X_CONTENT_TYPE_OPTIONS,
        axum::http::HeaderValue::from_static("nosniff"),
    );
    response
}
pub(super) fn cookie(request: &Request) -> Option<String> {
    request
        .headers()
        .get(header::COOKIE)?
        .to_str()
        .ok()?
        .split(';')
        .find_map(|c| c.trim().strip_prefix("xray_session=").map(str::to_owned))
}
pub(super) async fn authenticate(
    State(web): State<Arc<Web>>,
    request: Request,
    next: Next,
) -> Response {
    let authenticated = (|| -> Result<bool> {
        let Some(token) = cookie(&request) else {
            return Ok(false);
        };
        let credential = Credentials::load(&web.credentials)?;
        let found = web.db.conn()?.query_row(
            "SELECT EXISTS(SELECT 1 FROM sessions WHERE token_hash=?1 AND epoch=?2 AND expires>?3)",
            rusqlite::params![hash(&token), credential.epoch, now()],
            |r| r.get(0),
        )?;
        Ok(found)
    })();
    match authenticated {
        Ok(true) => next.run(request).await,
        Ok(false) => failure(StatusCode::UNAUTHORIZED, "login required"),
        Err(e) => e.into_response(),
    }
}
#[derive(Deserialize)]
pub(super) struct Login {
    username: String,
    password: String,
}
pub(super) async fn login(
    State(web): State<Arc<Web>>,
    Json(input): Json<Login>,
) -> Result<Response> {
    {
        let mut rate = web.login_attempts.lock().await;
        if rate.0.elapsed() > Duration::from_secs(60) {
            *rate = (Instant::now(), 0);
        }
        if rate.1 >= 30 {
            return Ok(failure(
                StatusCode::TOO_MANY_REQUESTS,
                "too many login attempts; retry later",
            ));
        }
        rate.1 += 1;
    }
    if input.username.len() > 128 || input.password.len() > 512 {
        return Ok(failure(StatusCode::UNAUTHORIZED, "invalid credentials"));
    }
    let credentials = Credentials::load(&web.credentials)?;
    let epoch = credentials.epoch.clone();
    let verified =
        tokio::task::spawn_blocking(move || credentials.verify(&input.username, &input.password))
            .await
            .map_err(|e| Error::Internal(e.to_string()))?;
    if !verified {
        return Ok(failure(StatusCode::UNAUTHORIZED, "invalid credentials"));
    }
    let token = web_config::secret(32)?;
    let c = web.db.conn()?;
    c.execute(
        "DELETE FROM sessions WHERE expires<=?1 OR epoch!=?2",
        rusqlite::params![now(), epoch],
    )?;
    c.execute(
        "INSERT INTO sessions VALUES(?1,?2,?3)",
        rusqlite::params![hash(&token), epoch, now() + 86400],
    )?;
    let cookie = format!(
        "xray_session={token}; HttpOnly; SameSite=Strict; Path=/; Max-Age=86400{}",
        if web.secure_cookie { "; Secure" } else { "" }
    );
    Ok((
        [(header::SET_COOKIE, cookie)],
        Json(json!({"authenticated":true})),
    )
        .into_response())
}
pub(super) async fn session(State(web): State<Arc<Web>>) -> Result<Json<Value>> {
    Ok(Json(
        json!({"username":Credentials::load(&web.credentials)?.username,"version":env!("CARGO_PKG_VERSION")}),
    ))
}
pub(super) async fn logout(State(web): State<Arc<Web>>, request: Request) -> Result<Response> {
    if let Some(token) = cookie(&request) {
        web.db
            .conn()?
            .execute("DELETE FROM sessions WHERE token_hash=?1", [hash(&token)])?;
    }
    Ok((
        [(
            header::SET_COOKIE,
            "xray_session=; HttpOnly; SameSite=Strict; Path=/; Max-Age=0",
        )],
        Json(json!({"ok":true})),
    )
        .into_response())
}
