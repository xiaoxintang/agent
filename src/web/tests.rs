use crate::web::{Invoke, Web, db::Db, endpoint, mutating, router};
use crate::web_config::Credentials;
use axum::{
    body::Body,
    http::{StatusCode, header},
};
use serde_json::{Value, json};
use std::{collections::HashMap, sync::Arc, time::Instant};
use tokio::sync::Mutex;
use tower::ServiceExt;

#[test]
fn allowlist_rejects_arbitrary_urls_and_normalized_traversal() {
    for path in [
        "https://example.com/api/v1/xray/status",
        "/api/v1/xray/config/../status",
        "/api/v1/xray/inbounds/%2e%2e",
    ] {
        let r = Invoke {
            method: "GET".into(),
            path: path.into(),
            query: Default::default(),
            body: Value::Null,
            confirmation: None,
            expected_revision: None,
        };
        assert!(endpoint(&r).is_err());
    }
}

#[tokio::test]
async fn password_reset_revokes_cookie_and_credentials_are_never_in_session_response() {
    let dir = tempfile::tempdir().unwrap();
    let credential_path = dir.path().join("credentials.json");
    let credential = Credentials::create(&credential_path).unwrap();
    let web = Arc::new(Web {
        db: Db::open(&dir.path().join("web.db")).unwrap(),
        client: reqwest::Client::new(),
        credentials: credential_path.clone(),
        local_agent: dir.path().join("local.json"),
        secure_cookie: false,
        locks: Mutex::new(HashMap::new()),
        login_attempts: Mutex::new((Instant::now(), 0)),
    });
    let router = router(web, dir.path());
    let response = router
        .clone()
        .oneshot(
            axum::http::Request::builder()
                .method("POST")
                .uri("/api/login")
                .header("content-type", "application/json")
                .header("x-web-request", "1")
                .body(Body::from(
                    json!({"username":credential.username,"password":credential.password})
                        .to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let cookie = response.headers()[header::SET_COOKIE]
        .to_str()
        .unwrap()
        .split(';')
        .next()
        .unwrap()
        .to_owned();
    let response = router
        .clone()
        .oneshot(
            axum::http::Request::builder()
                .uri("/api/session")
                .header("cookie", &cookie)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let bytes = axum::body::to_bytes(response.into_body(), 1024)
        .await
        .unwrap();
    let value: Value = serde_json::from_slice(&bytes).unwrap();
    assert!(value.get("password").is_none());
    Credentials::reset(&credential_path).unwrap();
    let response = router
        .oneshot(
            axum::http::Request::builder()
                .uri("/api/session")
                .header("cookie", &cookie)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
}

#[test]
fn only_explicit_counter_resets_enter_the_write_queue() {
    let mut request = Invoke {
        method: "POST".into(),
        path: "/api/v1/xray/stats/query".into(),
        query: Default::default(),
        body: json!({"reset":false}),
        confirmation: None,
        expected_revision: None,
    };
    assert!(!mutating(&request).unwrap());
    request.body["reset"] = json!(true);
    assert!(mutating(&request).unwrap());
}
