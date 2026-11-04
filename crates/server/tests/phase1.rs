use axum::{
    Router,
    body::{Body, to_bytes},
    http::{Request, StatusCode, header},
    response::Response,
};
use panel_server::{app, auth, db, state::AppState};
use serde_json::{Value, json};
use sqlx::SqlitePool;
use tempfile::TempDir;
use tower::ServiceExt;

const ORIGIN: &str = "http://127.0.0.1:8080";
const PASSWORD: &str = "test-only-long-password";

struct TestApp {
    router: Router,
    pool: SqlitePool,
    _directory: TempDir,
}

impl TestApp {
    async fn new(secure: bool) -> Self {
        let directory = tempfile::tempdir().unwrap();
        let pool = db::open(&directory.path().join("data")).await.unwrap();
        auth::bootstrap_admin(&pool, "admin", PASSWORD.into())
            .await
            .unwrap();
        let state = AppState::new(pool.clone(), ORIGIN.into(), secure)
            .await
            .unwrap();
        Self {
            router: app::router(state),
            pool,
            _directory: directory,
        }
    }
    async fn request(
        &self,
        method: &str,
        path: &str,
        body: Option<Value>,
        headers: &[(&str, &str)],
    ) -> Response {
        let mut request = Request::builder().method(method).uri(path);
        for (name, value) in headers {
            request = request.header(*name, *value);
        }
        let body = if let Some(body) = body {
            request = request.header(header::CONTENT_TYPE, "application/json");
            body.to_string()
        } else {
            String::new()
        };
        self.router
            .clone()
            .oneshot(request.body(Body::from(body)).unwrap())
            .await
            .unwrap()
    }
    async fn login(&self) -> (String, String) {
        let response = self
            .request(
                "POST",
                "/api/v1/auth/login",
                Some(json!({"username":"admin", "password":PASSWORD})),
                &[("origin", ORIGIN)],
            )
            .await;
        assert_eq!(response.status(), StatusCode::OK);
        let cookie = response.headers()[header::SET_COOKIE]
            .to_str()
            .unwrap()
            .split(';')
            .next()
            .unwrap()
            .to_owned();
        let json = body(response).await;
        (cookie, json["csrf_token"].as_str().unwrap().to_owned())
    }
}

async fn body(response: Response) -> Value {
    serde_json::from_slice(&to_bytes(response.into_body(), 128 * 1024).await.unwrap()).unwrap()
}

#[tokio::test]
async fn migrations_are_idempotent_and_enforce_foreign_keys_and_private_files() {
    let directory = tempfile::tempdir().unwrap();
    let pool = db::open(&directory.path().join("data")).await.unwrap();
    db::MIGRATOR.run(&pool).await.unwrap();
    db::MIGRATOR.run(&pool).await.unwrap();
    let migrations: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM _sqlx_migrations WHERE success = 1")
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(migrations, 5);
    let journal: String = sqlx::query_scalar("PRAGMA journal_mode")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(journal, "wal");
    let keys: i64 = sqlx::query_scalar("PRAGMA foreign_keys")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(keys, 1);
    let timeout: i64 = sqlx::query_scalar("PRAGMA busy_timeout")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(timeout, 5000);
    assert!(
        sqlx::query(
            "INSERT INTO sessions VALUES ('id', 'missing-admin', 'hash', 9999999999, NULL, 0)"
        )
        .execute(&pool)
        .await
        .is_err()
    );
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(
            std::fs::metadata(directory.path().join("data/panel.sqlite3"))
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o600
        );
    }
    pool.close().await;
    let reopened = db::open(&directory.path().join("data")).await.unwrap();
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM _sqlx_migrations")
            .fetch_one(&reopened)
            .await
            .unwrap(),
        5
    );
    reopened.close().await;
}

#[tokio::test]
async fn bootstrap_has_no_default_password_and_cannot_replace_an_admin() {
    let directory = tempfile::tempdir().unwrap();
    let pool = db::open(&directory.path().join("data")).await.unwrap();
    assert!(
        auth::bootstrap_admin(&pool, "admin", "short".into())
            .await
            .is_err()
    );
    auth::bootstrap_admin(&pool, "admin", PASSWORD.into())
        .await
        .unwrap();
    let error = auth::bootstrap_admin(&pool, "another", "different-long-password".into())
        .await
        .unwrap_err();
    assert_eq!(error.code, "already_initialized");
    let stored: String = sqlx::query_scalar("SELECT password_hash FROM admins")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert!(stored.starts_with("$argon2id$v=19$"));
    assert!(!stored.contains(PASSWORD));
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM admins")
            .fetch_one(&pool)
            .await
            .unwrap(),
        1
    );
    pool.close().await;
}

#[tokio::test]
async fn all_management_endpoints_require_a_valid_session() {
    let app = TestApp::new(true).await;
    for (method, path) in [
        ("GET", "/api/v1/auth/me"),
        ("POST", "/api/v1/auth/logout"),
        ("GET", "/api/v1/nodes"),
        ("POST", "/api/v1/nodes"),
        ("GET", "/api/v1/nodes/00000000-0000-4000-8000-000000000000"),
        (
            "PATCH",
            "/api/v1/nodes/00000000-0000-4000-8000-000000000000",
        ),
        (
            "DELETE",
            "/api/v1/nodes/00000000-0000-4000-8000-000000000000",
        ),
        ("GET", "/api/v1/audit"),
    ] {
        let response = app.request(method, path, None, &[]).await;
        assert_eq!(
            response.status(),
            StatusCode::UNAUTHORIZED,
            "{method} {path}"
        );
        assert_eq!(response.headers()[header::CACHE_CONTROL], "no-store");
        assert!(response.headers().contains_key("x-request-id"));
        assert_eq!(body(response).await["error"]["code"], "unauthorized");
    }
    app.pool.close().await;
}

#[tokio::test]
async fn login_cookie_is_secure_and_only_hashes_are_stored_or_returned() {
    let app = TestApp::new(true).await;
    let response = app
        .request(
            "POST",
            "/api/v1/auth/login",
            Some(json!({"username":"admin", "password":PASSWORD})),
            &[("origin", ORIGIN), ("x-request-id", "untrusted-client-id")],
        )
        .await;
    assert_eq!(response.status(), StatusCode::OK);
    assert_ne!(response.headers()["x-request-id"], "untrusted-client-id");
    let cookie = response.headers()[header::SET_COOKIE]
        .to_str()
        .unwrap()
        .to_owned();
    for value in [
        "__Host-panel_session=",
        "HttpOnly",
        "SameSite=Strict",
        "Secure",
        "Path=/",
        "Max-Age=3600",
    ] {
        assert!(cookie.contains(value));
    }
    assert!(!cookie.contains("Domain="));
    let token = cookie.split(';').next().unwrap().split_once('=').unwrap().1;
    let response = body(response).await;
    assert_eq!(response["admin"]["username"], "admin");
    assert!(!response.to_string().contains(token));
    assert!(!response.to_string().contains("password"));
    assert!(!response.to_string().contains(PASSWORD));
    assert!(response["expires_at"].as_i64().unwrap() <= db::now() + 3600);
    let hash: String = sqlx::query_scalar("SELECT token_hash FROM sessions")
        .fetch_one(&app.pool)
        .await
        .unwrap();
    assert_eq!(hash, auth::token_hash(token));
    assert_ne!(hash, token);
    let cookie = cookie.split(';').next().unwrap();
    let me = app
        .request("GET", "/api/v1/auth/me", None, &[("cookie", cookie)])
        .await;
    assert_eq!(me.status(), StatusCode::OK);
    assert_eq!(body(me).await["csrf_token"], response["csrf_token"]);
    let audit = app
        .request("GET", "/api/v1/audit", None, &[("cookie", cookie)])
        .await;
    let audit = body(audit).await.to_string();
    for secret in [
        PASSWORD,
        token,
        hash.as_str(),
        response["csrf_token"].as_str().unwrap(),
    ] {
        assert!(!audit.contains(secret));
    }
    app.pool.close().await;
}

#[tokio::test]
async fn development_cookie_omits_secure_only_when_explicitly_requested() {
    let app = TestApp::new(false).await;
    let (cookie, _) = app.login().await;
    assert!(cookie.starts_with("panel_session="));
    let me = app
        .request("GET", "/api/v1/auth/me", None, &[("cookie", &cookie)])
        .await;
    assert_eq!(me.status(), StatusCode::OK);
    app.pool.close().await;
}

#[tokio::test]
async fn login_requires_the_exact_origin_and_uses_generic_credential_errors() {
    let app = TestApp::new(true).await;
    for origin in [None, Some("http://evil.example"), Some("null")] {
        let headers = origin
            .map(|value| vec![("origin", value)])
            .unwrap_or_default();
        let response = app
            .request(
                "POST",
                "/api/v1/auth/login",
                Some(json!({"username":"admin", "password":PASSWORD})),
                &headers,
            )
            .await;
        assert_eq!(response.status(), StatusCode::FORBIDDEN);
    }
    let mut errors = Vec::new();
    for (username, password) in [("admin", "wrong"), ("missing-user", PASSWORD)] {
        let response = app
            .request(
                "POST",
                "/api/v1/auth/login",
                Some(json!({"username":username, "password":password})),
                &[("origin", ORIGIN)],
            )
            .await;
        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
        errors.push(body(response).await);
    }
    sqlx::query("UPDATE admins SET disabled_at = ?")
        .bind(db::now())
        .execute(&app.pool)
        .await
        .unwrap();
    let response = app
        .request(
            "POST",
            "/api/v1/auth/login",
            Some(json!({"username":"admin", "password":PASSWORD})),
            &[("origin", ORIGIN)],
        )
        .await;
    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    errors.push(body(response).await);
    assert!(errors.windows(2).all(|pair| pair[0] == pair[1]));
    app.pool.close().await;
}

#[tokio::test]
async fn mutations_require_both_origin_and_the_session_bound_csrf_token() {
    let app = TestApp::new(true).await;
    let (cookie, csrf) = app.login().await;
    for headers in [
        vec![("cookie", cookie.as_str())],
        vec![("cookie", cookie.as_str()), ("origin", ORIGIN)],
        vec![
            ("cookie", cookie.as_str()),
            ("origin", ORIGIN),
            ("x-csrf-token", "wrong"),
        ],
        vec![
            ("cookie", cookie.as_str()),
            ("origin", "http://evil.example"),
            ("x-csrf-token", csrf.as_str()),
        ],
        vec![
            ("cookie", cookie.as_str()),
            ("origin", ORIGIN),
            ("x-csrf-token", csrf.as_str()),
            ("sec-fetch-site", "cross-site"),
        ],
    ] {
        let response = app
            .request(
                "POST",
                "/api/v1/nodes",
                Some(json!({"name":"node"})),
                &headers,
            )
            .await;
        assert_eq!(response.status(), StatusCode::FORBIDDEN);
    }
    let (other_cookie, _) = app.login().await;
    let response = app
        .request(
            "POST",
            "/api/v1/nodes",
            Some(json!({"name":"node"})),
            &[
                ("cookie", &other_cookie),
                ("origin", ORIGIN),
                ("x-csrf-token", &csrf),
            ],
        )
        .await;
    assert_eq!(response.status(), StatusCode::FORBIDDEN);
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM nodes")
            .fetch_one(&app.pool)
            .await
            .unwrap(),
        0
    );
    app.pool.close().await;
}

#[tokio::test]
async fn logout_expiry_disabled_accounts_and_duplicate_cookies_invalidate_access() {
    let app = TestApp::new(true).await;
    let (cookie, csrf) = app.login().await;
    let response = app
        .request(
            "POST",
            "/api/v1/auth/logout",
            None,
            &[
                ("cookie", &cookie),
                ("origin", ORIGIN),
                ("x-csrf-token", &csrf),
            ],
        )
        .await;
    assert_eq!(response.status(), StatusCode::NO_CONTENT);
    assert!(
        response.headers()[header::SET_COOKIE]
            .to_str()
            .unwrap()
            .contains("Max-Age=0")
    );
    assert_eq!(
        app.request("GET", "/api/v1/nodes", None, &[("cookie", &cookie)])
            .await
            .status(),
        StatusCode::UNAUTHORIZED
    );
    let (cookie, _) = app.login().await;
    let duplicates = format!("{cookie}; {cookie}");
    assert_eq!(
        app.request("GET", "/api/v1/nodes", None, &[("cookie", &duplicates)])
            .await
            .status(),
        StatusCode::UNAUTHORIZED
    );
    sqlx::query("UPDATE sessions SET expires_at = ?")
        .bind(db::now())
        .execute(&app.pool)
        .await
        .unwrap();
    assert_eq!(
        app.request("GET", "/api/v1/nodes", None, &[("cookie", &cookie)])
            .await
            .status(),
        StatusCode::UNAUTHORIZED
    );
    let (cookie, _) = app.login().await;
    sqlx::query("UPDATE admins SET disabled_at = ?")
        .bind(db::now())
        .execute(&app.pool)
        .await
        .unwrap();
    assert_eq!(
        app.request("GET", "/api/v1/nodes", None, &[("cookie", &cookie)])
            .await
            .status(),
        StatusCode::UNAUTHORIZED
    );
    app.pool.close().await;
}

#[tokio::test]
async fn node_crud_is_real_and_revocation_preserves_redacted_audits() {
    let app = TestApp::new(true).await;
    let (cookie, csrf) = app.login().await;
    let headers = [
        ("cookie", cookie.as_str()),
        ("origin", ORIGIN),
        ("x-csrf-token", csrf.as_str()),
    ];
    let response = app
        .request(
            "POST",
            "/api/v1/nodes",
            Some(json!({"name":"  测试节点  "})),
            &headers,
        )
        .await;
    assert_eq!(response.status(), StatusCode::CREATED);
    let created = body(response).await;
    assert_eq!(created["name"], "测试节点");
    assert_eq!(created["status"], "offline");
    assert!(created["agent_version"].is_null());
    let id = created["id"].as_str().unwrap();
    let path = format!("/api/v1/nodes/{id}");
    assert_eq!(
        body(app.request("GET", &path, None, &headers).await).await,
        created
    );
    let updated = app
        .request("PATCH", &path, Some(json!({"name":"Renamed"})), &headers)
        .await;
    assert_eq!(updated.status(), StatusCode::OK);
    assert_eq!(body(updated).await["name"], "Renamed");
    let list = body(app.request("GET", "/api/v1/nodes", None, &headers).await).await;
    assert_eq!(list.as_array().unwrap().len(), 1);
    assert_eq!(
        app.request("DELETE", &path, None, &headers).await.status(),
        StatusCode::NO_CONTENT
    );
    assert_eq!(
        app.request("DELETE", &path, None, &headers).await.status(),
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        app.request("GET", &path, None, &headers).await.status(),
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        body(app.request("GET", "/api/v1/nodes", None, &headers).await).await,
        json!([])
    );
    let retained: Option<i64> = sqlx::query_scalar("SELECT revoked_at FROM nodes WHERE id = ?")
        .bind(id)
        .fetch_one(&app.pool)
        .await
        .unwrap();
    assert!(retained.is_some());
    let audits = body(app.request("GET", "/api/v1/audit", None, &headers).await).await;
    for action in ["node.create", "node.update", "node.revoke"] {
        assert!(
            audits
                .as_array()
                .unwrap()
                .iter()
                .any(|entry| entry["action"] == action
                    && entry["node_id"] == id
                    && entry["metadata"]["request_id"].is_string())
        );
    }
    assert!(!audits.to_string().contains(PASSWORD));
    assert!(!audits.to_string().contains(&csrf));
    app.pool.close().await;
}

#[tokio::test]
async fn failed_audit_rolls_back_the_business_change() {
    let app = TestApp::new(true).await;
    let (cookie, csrf) = app.login().await;
    sqlx::raw_sql("CREATE TRIGGER fail_audit BEFORE INSERT ON audit_logs BEGIN SELECT RAISE(ABORT, 'test-only failure'); END;").execute(&app.pool).await.unwrap();
    let response = app
        .request(
            "POST",
            "/api/v1/nodes",
            Some(json!({"name":"must not persist"})),
            &[
                ("cookie", &cookie),
                ("origin", ORIGIN),
                ("x-csrf-token", &csrf),
            ],
        )
        .await;
    assert_eq!(response.status(), StatusCode::INTERNAL_SERVER_ERROR);
    assert_eq!(body(response).await["error"]["code"], "internal_error");
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM nodes")
            .fetch_one(&app.pool)
            .await
            .unwrap(),
        0
    );
    app.pool.close().await;
}

#[tokio::test]
async fn validates_names_ids_json_media_type_and_request_size() {
    let app = TestApp::new(true).await;
    let (cookie, csrf) = app.login().await;
    let headers = [
        ("cookie", cookie.as_str()),
        ("origin", ORIGIN),
        ("x-csrf-token", csrf.as_str()),
    ];
    for input in [
        json!({"name":" "}),
        json!({"name":"a\nb"}),
        json!({"name":"x".repeat(65)}),
        json!({"name":3}),
        json!({"name":"ok", "command":"untrusted"}),
    ] {
        let response = app
            .request("POST", "/api/v1/nodes", Some(input), &headers)
            .await;
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
        assert!(body(response).await["error"]["code"].is_string());
    }
    assert_eq!(
        app.request("GET", "/api/v1/nodes/not-a-uuid", None, &headers)
            .await
            .status(),
        StatusCode::BAD_REQUEST
    );
    assert_eq!(
        app.request("POST", "/api/v1/nodes", None, &headers)
            .await
            .status(),
        StatusCode::UNSUPPORTED_MEDIA_TYPE
    );
    let response = app
        .request(
            "POST",
            "/api/v1/nodes",
            Some(json!({"name":"x".repeat(17000)})),
            &headers,
        )
        .await;
    assert_eq!(response.status(), StatusCode::PAYLOAD_TOO_LARGE);
    assert_eq!(body(response).await["error"]["code"], "invalid_json");
    app.pool.close().await;
}

#[tokio::test]
async fn login_rate_limit_is_bounded_and_includes_invalid_json_attempts() {
    let app = TestApp::new(true).await;
    for _ in 0..10 {
        assert_eq!(
            app.request("POST", "/api/v1/auth/login", None, &[("origin", ORIGIN)])
                .await
                .status(),
            StatusCode::UNSUPPORTED_MEDIA_TYPE
        );
    }
    let response = app
        .request(
            "POST",
            "/api/v1/auth/login",
            Some(json!({"username":"admin", "password":PASSWORD})),
            &[("origin", ORIGIN)],
        )
        .await;
    assert_eq!(response.status(), StatusCode::TOO_MANY_REQUESTS);
    assert_eq!(body(response).await["error"]["code"], "rate_limited");
    app.pool.close().await;
}

#[tokio::test]
async fn audit_retention_removes_old_rows_and_queries_are_bounded() {
    let app = TestApp::new(true).await;
    sqlx::query("INSERT INTO audit_logs (action, target, result, created_at, metadata_json) VALUES ('old', 'none', 'success', ?, '{\"request_id\":null}')").bind(db::now() - db::AUDIT_RETENTION - 1).execute(&app.pool).await.unwrap();
    let (cookie, _) = app.login().await;
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM audit_logs WHERE action = 'old'")
            .fetch_one(&app.pool)
            .await
            .unwrap(),
        0
    );
    for _ in 0..105 {
        sqlx::query("INSERT INTO audit_logs (action, target, result, created_at, metadata_json) VALUES ('test', 'none', 'success', ?, '{\"request_id\":null}')").bind(db::now()).execute(&app.pool).await.unwrap();
    }
    let entries = body(
        app.request("GET", "/api/v1/audit", None, &[("cookie", &cookie)])
            .await,
    )
    .await;
    assert_eq!(entries.as_array().unwrap().len(), 100);
    assert!(entries[0]["id"].is_string());
    app.pool.close().await;
}

#[cfg(unix)]
#[tokio::test]
async fn rejects_symlinked_and_insecure_database_paths() {
    use std::os::unix::fs::{PermissionsExt, symlink};
    let directory = tempfile::tempdir().unwrap();
    let insecure = directory.path().join("insecure");
    std::fs::create_dir(&insecure).unwrap();
    std::fs::set_permissions(&insecure, std::fs::Permissions::from_mode(0o755)).unwrap();
    assert!(db::open(&insecure).await.is_err());
    let linked = directory.path().join("linked");
    symlink(&insecure, &linked).unwrap();
    assert!(db::open(&linked).await.is_err());
    let private = directory.path().join("private");
    std::fs::create_dir(&private).unwrap();
    std::fs::set_permissions(&private, std::fs::Permissions::from_mode(0o700)).unwrap();
    let target = directory.path().join("unowned");
    std::fs::write(&target, "do not touch").unwrap();
    symlink(&target, private.join("panel.sqlite3")).unwrap();
    assert!(db::open(&private).await.is_err());
    assert_eq!(std::fs::read_to_string(target).unwrap(), "do not touch");
}

#[tokio::test]
async fn concurrent_logins_succeed_without_sqlite_snapshot_write_conflicts() {
    let app = TestApp::new(true).await;
    let input = json!({"username":"admin", "password":PASSWORD});
    let headers = [("origin", ORIGIN)];
    let (first, second) = tokio::join!(
        app.request("POST", "/api/v1/auth/login", Some(input.clone()), &headers),
        app.request("POST", "/api/v1/auth/login", Some(input), &headers)
    );
    assert_eq!(first.status(), StatusCode::OK);
    assert_eq!(second.status(), StatusCode::OK);
    assert_ne!(
        first.headers()[header::SET_COOKIE],
        second.headers()[header::SET_COOKIE]
    );
    app.pool.close().await;
}

#[tokio::test]
async fn login_rotates_an_existing_cookie_and_revokes_its_session() {
    let app = TestApp::new(true).await;
    let (old_cookie, _) = app.login().await;
    let response = app
        .request(
            "POST",
            "/api/v1/auth/login",
            Some(json!({"username":"admin", "password":PASSWORD})),
            &[("origin", ORIGIN), ("cookie", &old_cookie)],
        )
        .await;
    assert_eq!(response.status(), StatusCode::OK);
    let new_cookie = response.headers()[header::SET_COOKIE]
        .to_str()
        .unwrap()
        .split(';')
        .next()
        .unwrap();
    assert_ne!(new_cookie, old_cookie);
    assert_eq!(
        app.request("GET", "/api/v1/auth/me", None, &[("cookie", &old_cookie)])
            .await
            .status(),
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        app.request("GET", "/api/v1/auth/me", None, &[("cookie", new_cookie)])
            .await
            .status(),
        StatusCode::OK
    );
    app.pool.close().await;
}

#[tokio::test]
async fn concurrent_node_creation_cannot_exceed_the_documented_bound() {
    let app = TestApp::new(true).await;
    let (cookie, csrf) = app.login().await;
    for index in 0..99 {
        sqlx::query("INSERT INTO nodes (id, name, created_at) VALUES (?, 'fixture', ?)")
            .bind(format!("00000000-0000-4000-8000-{index:012x}"))
            .bind(db::now())
            .execute(&app.pool)
            .await
            .unwrap();
    }
    let headers = [
        ("cookie", cookie.as_str()),
        ("origin", ORIGIN),
        ("x-csrf-token", csrf.as_str()),
    ];
    let (first, second) = tokio::join!(
        app.request(
            "POST",
            "/api/v1/nodes",
            Some(json!({"name":"first"})),
            &headers
        ),
        app.request(
            "POST",
            "/api/v1/nodes",
            Some(json!({"name":"second"})),
            &headers
        )
    );
    let mut statuses = [first.status().as_u16(), second.status().as_u16()];
    statuses.sort();
    assert_eq!(statuses, [201, 409]);
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM nodes WHERE revoked_at IS NULL")
            .fetch_one(&app.pool)
            .await
            .unwrap(),
        100
    );
    app.pool.close().await;
}
