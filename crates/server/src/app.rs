use axum::{
    Json, Router,
    extract::{DefaultBodyLimit, Request},
    http::{HeaderValue, Method, StatusCode, header},
    middleware::{self, Next},
    response::{IntoResponse, Response},
    routing::get,
};
use panel_common::{ApiError, ErrorResponse, HealthResponse, HealthStatus, VERSION};

use crate::{agents, api, auth, configuration, metrics, state::AppState, static_assets, tasks};

#[derive(Clone)]
pub struct RequestId(pub String);

pub fn router(state: AppState) -> Router {
    Router::new()
        .route("/api/v1/health", get(health))
        .route("/api/v1/auth/login", axum::routing::post(auth::login))
        .route("/api/v1/auth/logout", axum::routing::post(auth::logout))
        .route("/api/v1/auth/me", get(auth::me))
        .route("/api/v1/nodes", get(api::nodes).post(api::create_node))
        .route("/api/v1/local-node", get(api::local_node))
        .route(
            "/api/v1/local-node/{id}",
            axum::routing::put(api::set_local_node),
        )
        .route(
            "/api/v1/nodes/enrollments",
            axum::routing::post(agents::issue),
        )
        .route(
            "/api/v1/nodes/{id}/tasks",
            axum::routing::post(tasks::create),
        )
        .route("/api/v1/nodes/{id}/status", get(api::node))
        .route("/api/v1/nodes/{id}/metrics", get(metrics::get))
        .route("/api/v1/nodes/{id}/traffic", get(crate::traffic::get))
        .route(
            "/api/v1/nodes/{id}/xray/{action}",
            axum::routing::post(tasks::xray),
        )
        .route("/api/v1/tasks/{id}", get(tasks::get))
        .route("/agent/v1/connect", get(agents::connect))
        .route(
            "/api/v1/nodes/{id}",
            get(api::node)
                .patch(api::update_node)
                .delete(api::delete_node),
        )
        .route("/api/v1/audit", get(api::audit))
        .route(
            "/api/v1/nodes/{id}/inbounds",
            get(configuration::inbounds).post(configuration::create_inbound),
        )
        .route(
            "/api/v1/inbounds/{id}",
            get(configuration::get_inbound)
                .patch(configuration::update_inbound)
                .delete(configuration::delete_inbound),
        )
        .route(
            "/api/v1/clients",
            get(configuration::clients).post(configuration::create_client),
        )
        .route(
            "/api/v1/clients/{id}",
            get(configuration::get_client)
                .patch(configuration::update_client)
                .delete(configuration::delete_client),
        )
        .route(
            "/api/v1/inbounds/{id}/clients/{client}",
            axum::routing::post(configuration::assign).delete(configuration::unassign),
        )
        .route(
            "/api/v1/nodes/{id}/config/revisions",
            get(configuration::revisions),
        )
        .route(
            "/api/v1/nodes/{id}/config/apply",
            axum::routing::post(configuration::apply),
        )
        .route("/", get(static_assets::index))
        .route("/login", get(static_assets::index))
        .route("/overview", get(static_assets::index))
        .route("/resources", get(static_assets::index))
        .route("/xray", get(static_assets::index))
        .route("/configuration", get(static_assets::index))
        .route("/traffic", get(static_assets::index))
        .route("/agent", get(static_assets::index))
        .route("/nodes", get(static_assets::index))
        .route("/audit", get(static_assets::index))
        .fallback(fallback)
        .method_not_allowed_fallback(method_not_allowed)
        .layer(DefaultBodyLimit::max(16 * 1024))
        .layer(middleware::from_fn(security_headers))
        .with_state(state)
}

async fn health() -> impl IntoResponse {
    (
        [(header::CACHE_CONTROL, "no-store")],
        Json(HealthResponse {
            status: HealthStatus::Ok,
            version: VERSION.into(),
        }),
    )
}

async fn fallback(request: Request) -> Response {
    let path = request.uri().path();
    if (request.method() == Method::GET || request.method() == Method::HEAD)
        && path.starts_with("/assets/")
    {
        return static_assets::asset(&path[1..]);
    }
    error(StatusCode::NOT_FOUND, "not_found", "Resource not found")
}

async fn method_not_allowed() -> Response {
    error(
        StatusCode::METHOD_NOT_ALLOWED,
        "method_not_allowed",
        "Method not allowed",
    )
}

pub(crate) fn error(status: StatusCode, code: &'static str, message: &'static str) -> Response {
    (
        status,
        [(header::CACHE_CONTROL, "no-store")],
        Json(ErrorResponse {
            error: ApiError { code, message },
        }),
    )
        .into_response()
}

async fn security_headers(mut request: Request, next: Next) -> Response {
    let id = uuid::Uuid::new_v4().to_string();
    request.extensions_mut().insert(RequestId(id.clone()));
    let is_api = request.uri().path().starts_with("/api/");
    let mut response = next.run(request).await;
    let headers = response.headers_mut();
    headers.insert(
        "x-request-id",
        HeaderValue::from_str(&id).expect("UUID is a valid header"),
    );
    if is_api {
        headers.insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    }
    headers.insert(
        header::X_CONTENT_TYPE_OPTIONS,
        HeaderValue::from_static("nosniff"),
    );
    headers.insert(
        header::REFERRER_POLICY,
        HeaderValue::from_static("no-referrer"),
    );
    headers.insert(
        header::CONTENT_SECURITY_POLICY,
        HeaderValue::from_static(
            "default-src 'none'; script-src 'self'; style-src 'self' 'unsafe-inline'; img-src 'self' data:; font-src 'self'; connect-src 'self'; base-uri 'none'; frame-ancestors 'none'; form-action 'self'; object-src 'none'",
        ),
    );
    response
}
