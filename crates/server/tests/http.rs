use axum::{
    body::{Body, to_bytes},
    http::{Request, StatusCode, header},
    response::Response,
};
use panel_server::{app, db, state::AppState};
use tower::ServiceExt;

async fn request(method: &str, uri: &str) -> Response {
    let directory = tempfile::tempdir().unwrap();
    let pool = db::open(&directory.path().join("data")).await.unwrap();
    let state = AppState::new(pool.clone(), true).await.unwrap();
    let response = app::router(state)
        .oneshot(
            Request::builder()
                .method(method)
                .uri(uri)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    pool.close().await;
    response
}

async fn body(response: Response) -> Vec<u8> {
    to_bytes(response.into_body(), 2 * 1024 * 1024)
        .await
        .unwrap()
        .to_vec()
}

#[tokio::test]
async fn health_matches_the_public_contract() {
    let response = request("GET", "/api/v1/health").await;
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(response.headers()[header::CONTENT_TYPE], "application/json");
    assert_eq!(response.headers()[header::CACHE_CONTROL], "no-store");
    let json: serde_json::Value = serde_json::from_slice(&body(response).await).unwrap();
    assert_eq!(
        json,
        serde_json::json!({"status": "ok", "version": panel_common::VERSION})
    );
}

#[tokio::test]
async fn serves_real_embedded_assets_with_correct_headers() {
    let response = request("GET", "/").await;
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(response.headers()[header::CONTENT_TYPE], "text/html");
    assert_eq!(response.headers()[header::CACHE_CONTROL], "no-cache");
    assert_eq!(
        response.headers()[header::X_CONTENT_TYPE_OPTIONS],
        "nosniff"
    );
    let csp = response.headers()[header::CONTENT_SECURITY_POLICY]
        .to_str()
        .unwrap();
    assert!(csp.contains("script-src 'self';"));
    assert!(csp.contains("frame-ancestors 'none'"));
    let html = String::from_utf8(body(response).await).unwrap();
    assert!(html.contains("<div id=\"root\"></div>"));
    assert!(!html.contains("/src/main.tsx"));
    for extension in [".js", ".css"] {
        let path = html
            .split('"')
            .find(|part| part.starts_with("/assets/") && part.ends_with(extension))
            .unwrap();
        let response = request("GET", path).await;
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(
            response.headers()[header::CACHE_CONTROL],
            "public, max-age=31536000, immutable"
        );
        let mime = response.headers()[header::CONTENT_TYPE].to_str().unwrap();
        if extension == ".js" {
            assert!(mime.contains("javascript"));
        } else {
            assert_eq!(mime, "text/css");
        }
        assert!(!body(response).await.is_empty());
    }
}

#[tokio::test]
async fn unknown_routes_and_private_files_never_fall_back_to_the_spa() {
    for path in [
        "/api",
        "/api/v1/missing",
        "/agent/v1/missing",
        "/nodes/missing",
        "/assets/missing.js",
        "/Cargo.toml",
        "/.git/config",
        "/assets/../index.html",
        "/assets/%2e%2e/index.html",
    ] {
        let response = request("GET", path).await;
        assert_eq!(response.status(), StatusCode::NOT_FOUND, "{path}");
        assert_eq!(response.headers()[header::CONTENT_TYPE], "application/json");
        let json: serde_json::Value = serde_json::from_slice(&body(response).await).unwrap();
        assert_eq!(json["error"]["code"], "not_found");
    }
}

#[tokio::test]
async fn frontend_routes_serve_the_spa_and_support_head() {
    for path in [
        "/login",
        "/overview",
        "/resources",
        "/xray",
        "/configuration",
        "/traffic",
        "/agent",
        "/nodes",
        "/audit",
    ] {
        let response = request("GET", path).await;
        assert_eq!(response.status(), StatusCode::OK, "{path}");
        assert_eq!(response.headers()[header::CONTENT_TYPE], "text/html");
        let response = request("HEAD", path).await;
        assert_eq!(response.status(), StatusCode::OK, "{path}");
        assert!(body(response).await.is_empty());
    }
}

#[tokio::test]
async fn rejects_unsupported_methods_and_handles_head() {
    for path in ["/", "/api/v1/health"] {
        let response = request("POST", path).await;
        assert_eq!(response.status(), StatusCode::METHOD_NOT_ALLOWED);
        let json: serde_json::Value = serde_json::from_slice(&body(response).await).unwrap();
        assert_eq!(json["error"]["code"], "method_not_allowed");
        let response = request("HEAD", path).await;
        assert_eq!(response.status(), StatusCode::OK);
        assert!(body(response).await.is_empty());
    }
}
