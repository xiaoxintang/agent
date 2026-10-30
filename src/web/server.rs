use super::{Web, assignments, auth, contract, db::Db, events, management, proxy, worker};
use crate::{
    error::{Error, Result},
    web_config::{self, Credentials, LocalAgent},
};
use axum::{
    Json, Router,
    extract::DefaultBodyLimit,
    http::StatusCode,
    middleware,
    routing::{delete, get, post},
};
use serde_json::{Value, json};
use std::{
    collections::HashMap,
    path::{Path as FsPath, PathBuf},
    sync::Arc,
    time::{Duration, Instant},
};
use tokio::sync::Mutex;
use tower_http::services::{ServeDir, ServeFile};

pub async fn serve(
    settings: web_config::Settings,
    database: PathBuf,
    credentials: PathBuf,
    local_agent: PathBuf,
    frontend: PathBuf,
) -> Result<()> {
    let manifest: Value = serde_json::from_slice(&std::fs::read(frontend.join("version.json"))?)?;
    if manifest["version"] != env!("CARGO_PKG_VERSION") {
        return Err(Error::Validation(
            "frontend/backend version mismatch".into(),
        ));
    }
    let output = tokio::process::Command::new("/usr/local/bin/agent")
        .arg("-v")
        .output()
        .await?;
    if !output.status.success()
        || String::from_utf8_lossy(&output.stdout).trim()
            != format!("agent {}", env!("CARGO_PKG_VERSION"))
    {
        return Err(Error::Validation("local agent/Web version mismatch".into()));
    }
    Credentials::load(&credentials)?;
    let lock_path = database.with_extension("lock");
    let _instance = std::fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .open(lock_path)?;
    fs2::FileExt::try_lock_exclusive(&_instance)
        .map_err(|_| Error::Conflict("another Web worker is using this database".into()))?;
    let listener =
        tokio::net::TcpListener::bind((std::net::Ipv4Addr::UNSPECIFIED, settings.port)).await?;
    let local: LocalAgent = serde_json::from_slice(&std::fs::read(&local_agent)?)?;
    let db = Db::open(&database)?;
    db.local(&local.url)?;
    let state = Arc::new(Web {
        db,
        client: reqwest::Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .connect_timeout(Duration::from_secs(10))
            .build()
            .map_err(|e| Error::Internal(e.to_string()))?,
        credentials,
        local_agent,
        secure_cookie: settings.secure_cookie,
        locks: Mutex::new(HashMap::new()),
        login_attempts: Mutex::new((Instant::now(), 0)),
    });
    let stop = tokio_util::sync::CancellationToken::new();
    let worker = tokio::spawn(worker::run(state.clone(), stop.clone()));
    let shutdown = stop.clone();
    let result = axum::serve(listener, router(state, &frontend))
        .with_graceful_shutdown(async move {
            #[cfg(unix)]
            {
                let mut term =
                    tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
                        .expect("SIGTERM handler");
                tokio::select! {_ = tokio::signal::ctrl_c()=>{},_ = term.recv()=>{}}
            }
            #[cfg(not(unix))]
            {
                let _ = tokio::signal::ctrl_c().await;
            }
            shutdown.cancel();
        })
        .await;
    stop.cancel();
    let _ = worker.await;
    result?;
    Ok(())
}

pub fn router(state: Arc<Web>, frontend: &FsPath) -> Router {
    let private = Router::new()
        .route("/session", get(auth::session))
        .route("/logout", post(auth::logout))
        .route(
            "/agents",
            get(management::agents).post(management::save_agent),
        )
        .route("/agents/{id}", delete(management::remove_agent))
        .route("/agents/{id}/discovery", get(management::discovery))
        .route("/agents/{id}/preview", post(proxy::preview))
        .route("/agents/{id}/invoke", post(proxy::invoke))
        .route("/agents/{id}/events", get(events::events))
        .route("/users", get(management::users).post(management::add_user))
        .route("/users/{id}", delete(management::delete_user))
        .route(
            "/assignments",
            get(assignments::assignments).post(assignments::assign),
        )
        .route("/assignments/{id}", delete(assignments::revoke))
        .route("/tasks", get(management::tasks))
        .route("/tasks/{id}/retry", post(management::retry))
        .route_layer(middleware::from_fn_with_state(
            state.clone(),
            auth::authenticate,
        ));
    let api = private
        .route("/login", post(auth::login))
        .route(
            "/meta",
            get(|| async { Json(json!({"version":env!("CARGO_PKG_VERSION")})) }),
        )
        .route("/openapi.json", get(|| async { Json(contract()) }))
        .fallback(|| async { auth::failure(StatusCode::NOT_FOUND, "unknown Web API") });
    Router::new()
        .nest("/api", api)
        .fallback_service(
            ServeDir::new(frontend).not_found_service(ServeFile::new(frontend.join("index.html"))),
        )
        .layer(DefaultBodyLimit::max(4 * 1024 * 1024))
        .layer(middleware::from_fn(auth::browser_policy))
        .with_state(state)
}
