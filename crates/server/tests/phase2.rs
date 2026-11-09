use axum::{
    Router,
    body::{Body, to_bytes},
    http::{Request, StatusCode, header},
    response::Response,
};
use futures_util::{SinkExt, StreamExt};
use panel_agent::{AgentError, config, connection, store::Store};
use panel_common::{
    AgentInfo,
    protocol::{Envelope, Message},
};
use panel_server::{app, auth, db, state::AppState};
use serde_json::{Value, json};
use std::{
    path::PathBuf,
    sync::Arc,
    time::{Duration, Instant},
};
use tempfile::TempDir;
use tokio::sync::watch;
use tokio_tungstenite::{
    Connector, MaybeTlsStream, WebSocketStream,
    tungstenite::{self, Message as WsMessage, client::IntoClientRequest},
};
use tower::ServiceExt;
use uuid::Uuid;

type Socket = WebSocketStream<MaybeTlsStream<tokio::net::TcpStream>>;

struct Harness {
    state: AppState,
    router: Router,
    origin: String,
    url: String,
    ca: PathBuf,
    cookie: String,
    csrf: String,
    directory: TempDir,
    handle: axum_server::Handle<std::net::SocketAddr>,
    server: tokio::task::JoinHandle<()>,
}

impl Harness {
    async fn new() -> Self {
        Self::with_names(vec!["localhost".into(), "127.0.0.1".into()]).await
    }
    async fn with_names(names: Vec<String>) -> Self {
        let directory = tempfile::tempdir().unwrap();
        let pool = db::open(&directory.path().join("data")).await.unwrap();
        auth::bootstrap_admin(&pool, "admin", "test-only-phase2-password".into())
            .await
            .unwrap();
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        listener.set_nonblocking(true).unwrap();
        let origin = format!("https://{}", listener.local_addr().unwrap());
        let url = format!("wss://{}/agent/v1/connect", listener.local_addr().unwrap());
        let mut state = AppState::new(pool, true).await.unwrap();
        state.tls_enabled = true;
        let router = app::router(state.clone());
        let key = rcgen::generate_simple_self_signed(names).unwrap();
        let ca = directory.path().join("ca.pem");
        std::fs::write(&ca, key.cert.pem()).unwrap();
        let tls = axum_server::tls_rustls::RustlsConfig::from_pem(
            key.cert.pem().into_bytes(),
            key.signing_key.serialize_pem().into_bytes(),
        )
        .await
        .unwrap();
        let handle = axum_server::Handle::new();
        let serve = axum_server::from_tcp_rustls(listener, tls)
            .unwrap()
            .handle(handle.clone());
        let served_router = router.clone();
        let server = tokio::spawn(async move {
            serve
                .serve(served_router.into_make_service())
                .await
                .unwrap();
        });
        handle.listening().await.unwrap();
        let mut this = Self {
            state,
            router,
            origin,
            url,
            ca,
            cookie: String::new(),
            csrf: String::new(),
            directory,
            handle,
            server,
        };
        let response = this
            .request(
                "POST",
                "/api/v1/auth/login",
                Some(json!({"username":"admin", "password":"test-only-phase2-password"})),
            )
            .await;
        assert_eq!(response.status(), StatusCode::OK);
        this.cookie = response.headers()[header::SET_COOKIE]
            .to_str()
            .unwrap()
            .split(';')
            .next()
            .unwrap()
            .to_owned();
        this.csrf = body(response).await["csrf_token"]
            .as_str()
            .unwrap()
            .to_owned();
        this
    }
    fn connector(&self) -> Connector {
        config::connector(Some(&self.ca)).unwrap()
    }
    async fn request(&self, method: &str, path: &str, input: Option<Value>) -> Response {
        let mut request = Request::builder()
            .method(method)
            .uri(path)
            .header("origin", &self.origin);
        if !self.cookie.is_empty() {
            request = request
                .header("cookie", &self.cookie)
                .header("x-csrf-token", &self.csrf);
        }
        let input = if let Some(value) = input {
            request = request.header("content-type", "application/json");
            value.to_string()
        } else {
            String::new()
        };
        self.router
            .clone()
            .oneshot(request.body(Body::from(input)).unwrap())
            .await
            .unwrap()
    }
    async fn node(&self) -> String {
        let response = self
            .request("POST", "/api/v1/nodes", Some(json!({"name":"test node"})))
            .await;
        assert_eq!(response.status(), StatusCode::CREATED);
        body(response).await["id"].as_str().unwrap().to_owned()
    }
    async fn issue(&self, node: &str) -> String {
        let response = self
            .request(
                "POST",
                "/api/v1/nodes/enrollments",
                Some(json!({"node_id":node})),
            )
            .await;
        assert_eq!(response.status(), StatusCode::OK);
        body(response).await["token"].as_str().unwrap().to_owned()
    }
    async fn enroll(&self, node: &str, index: u8) -> Arc<Store> {
        let store = Arc::new(
            Store::open(&self.directory.path().join(format!("agent-{index}")))
                .await
                .unwrap(),
        );
        let token = self.issue(node).await;
        let enrolled = connection::enroll(&store, &self.url, &token, false, self.connector())
            .await
            .unwrap();
        assert_eq!(enrolled, node);
        store
    }
    async fn socket(&self, kind: &str, token: &str) -> Result<Socket, tungstenite::Error> {
        let mut request = self.url.as_str().into_client_request().unwrap();
        let mut value: tungstenite::http::HeaderValue = format!("{kind} {token}").parse().unwrap();
        value.set_sensitive(true);
        request.headers_mut().insert("authorization", value);
        tokio_tungstenite::connect_async_tls_with_config(
            request,
            None,
            false,
            Some(self.connector()),
        )
        .await
        .map(|(socket, _)| socket)
    }
    async fn online(&self, node: &str) {
        let deadline = Instant::now() + Duration::from_secs(12);
        loop {
            let status: String = sqlx::query_scalar("SELECT status FROM nodes WHERE id = ?")
                .bind(node)
                .fetch_one(&self.state.pool)
                .await
                .unwrap();
            if status == "online" {
                return;
            }
            assert!(Instant::now() < deadline, "node did not become online");
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
    }
    async fn task(&self, node: &str, key: &str) -> Value {
        let response = self
            .request(
                "POST",
                &format!("/api/v1/nodes/{node}/tasks"),
                Some(json!({"operation":"agent_status", "idempotency_key":key})),
            )
            .await;
        assert!(response.status() == StatusCode::CREATED || response.status() == StatusCode::OK);
        body(response).await
    }
    async fn finished(&self, id: &str) -> Value {
        let deadline = Instant::now() + Duration::from_secs(8);
        loop {
            let task = body(
                self.request("GET", &format!("/api/v1/tasks/{id}"), None)
                    .await,
            )
            .await;
            if task["status"] == "succeeded" {
                return task;
            }
            assert!(Instant::now() < deadline, "task did not succeed: {task}");
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
    }
}
impl Drop for Harness {
    fn drop(&mut self) {
        let _ = self.state.agents_shutdown.send(true);
        self.handle.shutdown();
        self.server.abort();
    }
}

async fn body(response: Response) -> Value {
    serde_json::from_slice(&to_bytes(response.into_body(), 128 * 1024).await.unwrap()).unwrap()
}
async fn frame(socket: &mut Socket, node: &str, message: Message) {
    socket
        .send(WsMessage::Text(
            serde_json::to_string(&Envelope::new(node, message))
                .unwrap()
                .into(),
        ))
        .await
        .unwrap();
}
async fn receive(socket: &mut Socket) -> Envelope {
    loop {
        let message = tokio::time::timeout(Duration::from_secs(6), socket.next())
            .await
            .unwrap()
            .unwrap()
            .unwrap();
        match message {
            WsMessage::Text(text) => return Envelope::parse(&text, None).unwrap(),
            WsMessage::Ping(bytes) => socket.send(WsMessage::Pong(bytes)).await.unwrap(),
            _ => (),
        }
    }
}

#[tokio::test]
async fn committed_tasks_wake_the_agent_and_polling_recovers_a_missed_notification() {
    let server = Harness::new().await;
    let node = server.node().await;
    let store = server.enroll(&node, 1).await;
    let credential = store.identity().await.unwrap().unwrap().credential;
    let mut socket = server.socket("Bearer", &credential).await.unwrap();
    frame(&mut socket, &node, Message::Hello(AgentInfo::current())).await;
    receive(&mut socket).await;
    tokio::time::sleep(Duration::from_millis(100)).await;
    let task = server.task(&node, &Uuid::new_v4().to_string()).await;
    let delivered = tokio::time::timeout(Duration::from_secs(2), receive(&mut socket))
        .await
        .unwrap();
    let Message::Task(command) = delivered.message else {
        panic!("expected task")
    };
    assert_eq!(command.id, task["id"]);
    assert_eq!(
        sqlx::query_scalar::<_, String>("SELECT status FROM tasks WHERE id=?")
            .bind(&command.id)
            .fetch_one(&server.state.pool)
            .await
            .unwrap(),
        "dispatched"
    );
    frame(
        &mut socket,
        &node,
        Message::TaskResult {
            task_id: command.id,
            result: panel_common::TaskOutcome::Succeeded(AgentInfo::current()),
        },
    )
    .await;
    assert!(matches!(
        receive(&mut socket).await.message,
        Message::ResultAck { .. }
    ));
    tokio::time::sleep(Duration::from_millis(100)).await;
    let queued = panel_common::TaskCommand {
        id: Uuid::new_v4().to_string(),
        operation: panel_common::TaskOperation::AgentStatus,
        deadline_at: db::now() + 60,
        private_config: None,
    };
    // Deliberately bypass the notification path, simulating a lost wakeup.
    sqlx::query("INSERT INTO tasks (id,node_id,operation,payload_json,idempotency_key,status,created_at,deadline_at) VALUES (?,?,'agent_status',?,?,'pending',?,?)")
        .bind(&queued.id).bind(&node).bind(serde_json::to_string(&queued).unwrap())
        .bind(Uuid::new_v4().to_string()).bind(db::now()).bind(queued.deadline_at)
        .execute(&server.state.pool).await.unwrap();
    let delivered = tokio::time::timeout(Duration::from_secs(12), async {
        loop {
            match socket.next().await.unwrap().unwrap() {
                WsMessage::Text(text) => return Envelope::parse(&text, Some(&node)).unwrap(),
                WsMessage::Ping(bytes) => socket.send(WsMessage::Pong(bytes)).await.unwrap(),
                other => panic!("unexpected WebSocket message: {other:?}"),
            }
        }
    })
    .await
    .unwrap();
    assert!(matches!(delivered.message, Message::Task(task) if task.id==queued.id));
    let _ = socket.close(None).await;
    store.close().await;
}
fn unauthorized(error: tungstenite::Error) {
    assert!(matches!(error, tungstenite::Error::Http(response) if response.status() == 401));
}

#[tokio::test]
async fn agent_websocket_accepts_origin_with_valid_credentials() {
    let server = Harness::new().await;
    let node = server.node().await;
    let token = server.issue(&node).await;
    let mut request = server.url.as_str().into_client_request().unwrap();
    request
        .headers_mut()
        .insert("origin", "https://other.example".parse().unwrap());
    request.headers_mut().insert(
        "authorization",
        format!("Enrollment {token}").parse().unwrap(),
    );
    let (mut socket, _) = tokio_tungstenite::connect_async_tls_with_config(
        request,
        None,
        false,
        Some(server.connector()),
    )
    .await
    .unwrap();
    socket.close(None).await.unwrap();
}

#[tokio::test]
async fn two_agents_connect_heartbeat_reconnect_and_recover_persistent_tasks_over_wss() {
    let server = Harness::new().await;
    let first = server.node().await;
    let second = server.node().await;
    let one = server.enroll(&first, 1).await;
    let two = server.enroll(&second, 2).await;
    let (stop_one, rx_one) = watch::channel(false);
    let (stop_two, rx_two) = watch::channel(false);
    let agent_one = {
        let store = one.clone();
        let connector = server.connector();
        tokio::spawn(async move { connection::run(&store, false, connector, rx_one).await })
    };
    let agent_two = {
        let store = two.clone();
        let connector = server.connector();
        tokio::spawn(async move { connection::run(&store, false, connector, rx_two).await })
    };
    server.online(&first).await;
    server.online(&second).await;
    let old: String = sqlx::query_scalar("SELECT connection_id FROM nodes WHERE id = ?")
        .bind(&first)
        .fetch_one(&server.state.pool)
        .await
        .unwrap();
    sqlx::query("UPDATE nodes SET connection_id = 'test-forced-disconnect', status = 'offline' WHERE id = ?").bind(&first).execute(&server.state.pool).await.unwrap();
    let key = Uuid::new_v4().to_string();
    let task = server.task(&first, &key).await;
    let repeated = server.task(&first, &key).await;
    assert_eq!(task["id"], repeated["id"]);
    server.online(&first).await;
    let new: String = sqlx::query_scalar("SELECT connection_id FROM nodes WHERE id = ?")
        .bind(&first)
        .fetch_one(&server.state.pool)
        .await
        .unwrap();
    assert_ne!(new, old);
    let task = server.finished(task["id"].as_str().unwrap()).await;
    assert_eq!(task["result"]["data"]["version"], panel_common::VERSION);
    assert_eq!(task["result"]["data"]["os"], std::env::consts::OS);
    assert!(task["attempt_count"].as_i64().unwrap() >= 1);
    let second_task = server.task(&second, &Uuid::new_v4().to_string()).await;
    server.finished(second_task["id"].as_str().unwrap()).await;
    let before: i64 = sqlx::query_scalar("SELECT last_seen_at FROM nodes WHERE id = ?")
        .bind(&second)
        .fetch_one(&server.state.pool)
        .await
        .unwrap();
    let deadline = Instant::now() + Duration::from_secs(20);
    loop {
        let after: i64 = sqlx::query_scalar("SELECT last_seen_at FROM nodes WHERE id = ?")
            .bind(&second)
            .fetch_one(&server.state.pool)
            .await
            .unwrap();
        if after > before {
            break;
        }
        assert!(
            Instant::now() < deadline,
            "real heartbeat did not update last_seen_at"
        );
        tokio::time::sleep(Duration::from_millis(200)).await;
    }
    stop_one.send(true).unwrap();
    stop_two.send(true).unwrap();
    agent_one.await.unwrap().unwrap();
    agent_two.await.unwrap().unwrap();
    one.close().await;
    two.close().await;
}

#[tokio::test]
async fn enrollment_is_atomic_single_use_expiring_hashed_and_not_browser_readable() {
    let server = Harness::new().await;
    let node = server.node().await;
    let token = server.issue(&node).await;
    let (one, two) = tokio::join!(
        server.socket("Enrollment", &token),
        server.socket("Enrollment", &token)
    );
    let mut socket = match (one, two) {
        (Ok(socket), Err(error)) | (Err(error), Ok(socket)) => {
            unauthorized(error);
            socket
        }
        _ => panic!("one-time token was not atomic"),
    };
    let enrolled = receive(&mut socket).await;
    let Message::Enrolled { credential } = enrolled.message else {
        panic!("expected enrolled");
    };
    let hash: String =
        sqlx::query_scalar("SELECT credential_hash FROM node_credentials WHERE node_id = ?")
            .bind(&node)
            .fetch_one(&server.state.pool)
            .await
            .unwrap();
    assert_eq!(hash, auth::token_hash(&credential));
    let audit = body(server.request("GET", "/api/v1/audit", None).await)
        .await
        .to_string();
    assert!(!audit.contains(&credential));
    assert!(!audit.contains(&token));
    assert!(!audit.contains(&hash));
    let response = server
        .request("GET", &format!("/api/v1/nodes/{node}"), None)
        .await;
    assert!(!body(response).await.to_string().contains(&credential));
    let _ = socket.close(None).await;
    unauthorized(server.socket("Enrollment", &token).await.err().unwrap());
    let token = server.issue(&node).await;
    sqlx::query("UPDATE enrollments SET expires_at = ? WHERE node_id = ?")
        .bind(db::now() - 1)
        .bind(&node)
        .execute(&server.state.pool)
        .await
        .unwrap();
    unauthorized(server.socket("Enrollment", &token).await.err().unwrap());
}

#[tokio::test]
async fn untrusted_and_wrong_hostname_certificates_fail_closed_before_consuming_tokens() {
    let server = Harness::new().await;
    let node = server.node().await;
    let token = server.issue(&node).await;
    let directory = tempfile::tempdir().unwrap();
    let store = Store::open(&directory.path().join("agent")).await.unwrap();
    assert_eq!(
        connection::enroll(
            &store,
            &server.url,
            &token,
            false,
            config::connector(None).unwrap()
        )
        .await
        .unwrap_err(),
        AgentError::Connection
    );
    let consumed: Option<i64> =
        sqlx::query_scalar("SELECT consumed_at FROM enrollments WHERE node_id = ?")
            .bind(&node)
            .fetch_one(&server.state.pool)
            .await
            .unwrap();
    assert!(consumed.is_none());
    let wrong_name = Harness::with_names(vec!["wrong.example".into()]).await;
    let node = wrong_name.node().await;
    let token = wrong_name.issue(&node).await;
    assert_eq!(
        connection::enroll(
            &store,
            &wrong_name.url,
            &token,
            false,
            wrong_name.connector()
        )
        .await
        .unwrap_err(),
        AgentError::Connection
    );
    store.close().await;
}

#[tokio::test]
async fn ack_result_replay_and_wrong_node_task_results_are_checked() {
    let server = Harness::new().await;
    let node = server.node().await;
    let other = server.node().await;
    let store = server.enroll(&node, 1).await;
    let identity = store.identity().await.unwrap().unwrap();
    let mut socket = server.socket("Bearer", &identity.credential).await.unwrap();
    frame(&mut socket, &node, Message::Hello(AgentInfo::current())).await;
    assert!(matches!(
        receive(&mut socket).await.message,
        Message::Welcome
    ));
    let task = server.task(&node, &Uuid::new_v4().to_string()).await;
    let command = match receive(&mut socket).await.message {
        Message::Task(command) => command,
        _ => panic!("expected task"),
    };
    assert_eq!(task["id"], command.id);
    frame(
        &mut socket,
        &node,
        Message::TaskAck {
            task_id: command.id.clone(),
        },
    )
    .await;
    tokio::time::sleep(Duration::from_millis(100)).await;
    let acked: String = sqlx::query_scalar("SELECT status FROM tasks WHERE id = ?")
        .bind(&command.id)
        .fetch_one(&server.state.pool)
        .await
        .unwrap();
    assert_eq!(acked, "acked");
    for _ in 0..2 {
        frame(
            &mut socket,
            &node,
            Message::TaskResult {
                task_id: command.id.clone(),
                result: panel_common::TaskOutcome::Succeeded(AgentInfo::current()),
            },
        )
        .await;
        assert!(matches!(
            receive(&mut socket).await.message,
            Message::ResultAck { .. }
        ));
    }
    server.finished(&command.id).await;
    let alien = server.task(&other, &Uuid::new_v4().to_string()).await;
    frame(
        &mut socket,
        &node,
        Message::TaskResult {
            task_id: alien["id"].as_str().unwrap().into(),
            result: panel_common::TaskOutcome::Succeeded(AgentInfo::current()),
        },
    )
    .await;
    let _ = tokio::time::timeout(Duration::from_secs(3), socket.next())
        .await
        .unwrap();
    let status: String = sqlx::query_scalar("SELECT status FROM tasks WHERE id = ?")
        .bind(alien["id"].as_str().unwrap())
        .fetch_one(&server.state.pool)
        .await
        .unwrap();
    assert_eq!(status, "pending");
    store.close().await;
}

#[tokio::test]
async fn revocation_closes_the_connection_cancels_tasks_and_rejects_credentials() {
    let server = Harness::new().await;
    let node = server.node().await;
    let store = server.enroll(&node, 1).await;
    let credential = store.identity().await.unwrap().unwrap().credential;
    let mut socket = server.socket("Bearer", &credential).await.unwrap();
    frame(&mut socket, &node, Message::Hello(AgentInfo::current())).await;
    receive(&mut socket).await;
    let task = server.task(&node, &Uuid::new_v4().to_string()).await;
    assert_eq!(
        server
            .request("DELETE", &format!("/api/v1/nodes/{node}"), None)
            .await
            .status(),
        StatusCode::NO_CONTENT
    );
    loop {
        let incoming = tokio::time::timeout(Duration::from_secs(3), socket.next())
            .await
            .unwrap();
        if matches!(
            incoming,
            None | Some(Err(_)) | Some(Ok(WsMessage::Close(_)))
        ) {
            break;
        }
    }
    unauthorized(server.socket("Bearer", &credential).await.err().unwrap());
    let task = body(
        server
            .request(
                "GET",
                &format!("/api/v1/tasks/{}", task["id"].as_str().unwrap()),
                None,
            )
            .await,
    )
    .await;
    assert_eq!(task["status"], "canceled");
    store.close().await;
}

#[tokio::test]
async fn offline_deadlines_and_session_cookie_cannot_bypass_agent_authentication() {
    let server = Harness::new().await;
    let node = server.node().await;
    let task = server.task(&node, &Uuid::new_v4().to_string()).await;
    sqlx::query("UPDATE tasks SET deadline_at = ? WHERE id = ?")
        .bind(db::now() - 1)
        .bind(task["id"].as_str().unwrap())
        .execute(&server.state.pool)
        .await
        .unwrap();
    let result = body(
        server
            .request(
                "GET",
                &format!("/api/v1/tasks/{}", task["id"].as_str().unwrap()),
                None,
            )
            .await,
    )
    .await;
    assert_eq!(result["status"], "expired");
    let session_token = server.cookie.split_once('=').unwrap().1;
    unauthorized(server.socket("Bearer", session_token).await.err().unwrap());
    assert_eq!(
        server
            .request(
                "POST",
                &format!("/api/v1/nodes/{node}/tasks"),
                Some(json!({"operation":"shell", "idempotency_key":Uuid::new_v4().to_string()}))
            )
            .await
            .status(),
        StatusCode::BAD_REQUEST
    );
}

#[tokio::test]
async fn node_identity_and_protocol_version_are_bound_to_the_authenticated_connection() {
    let server = Harness::new().await;
    let node = server.node().await;
    let store = server.enroll(&node, 1).await;
    let credential = store.identity().await.unwrap().unwrap().credential;
    for scenario in 0..4 {
        let mut socket = server.socket("Bearer", &credential).await.unwrap();
        let mut hello = Envelope::new(&node, Message::Hello(AgentInfo::current()));
        if scenario == 0 {
            hello.node_id = Uuid::new_v4().to_string();
        } else {
            hello.version = 99;
        }
        let message = match scenario {
            2 => WsMessage::Text(
                "x".repeat(panel_common::protocol::MAX_MESSAGE_BYTES + 1)
                    .into(),
            ),
            3 => WsMessage::Binary(vec![0].into()),
            _ => WsMessage::Text(serde_json::to_string(&hello).unwrap().into()),
        };
        socket.send(message).await.unwrap();
        let _ = tokio::time::timeout(Duration::from_secs(3), socket.next())
            .await
            .unwrap();
    }
    let status: String = sqlx::query_scalar("SELECT status FROM nodes WHERE id = ?")
        .bind(&node)
        .fetch_one(&server.state.pool)
        .await
        .unwrap();
    assert_eq!(status, "offline");
    store.close().await;
}

#[tokio::test]
async fn heartbeat_timeout_marks_a_silent_connected_agent_offline() {
    let server = Harness::new().await;
    let node = server.node().await;
    let store = server.enroll(&node, 1).await;
    let credential = store.identity().await.unwrap().unwrap().credential;
    let mut socket = server.socket("Bearer", &credential).await.unwrap();
    frame(&mut socket, &node, Message::Hello(AgentInfo::current())).await;
    receive(&mut socket).await;
    let deadline = Instant::now() + Duration::from_secs(50);
    loop {
        let status: String = sqlx::query_scalar("SELECT status FROM nodes WHERE id = ?")
            .bind(&node)
            .fetch_one(&server.state.pool)
            .await
            .unwrap();
        if status == "offline" {
            break;
        }
        assert!(
            Instant::now() < deadline,
            "silent agent was not marked offline"
        );
        tokio::time::sleep(Duration::from_millis(200)).await;
    }
    let _ = socket.close(None).await;
    store.close().await;
}

#[tokio::test]
async fn credential_rotation_and_new_connections_invalidate_previous_connections() {
    let server = Harness::new().await;
    let node = server.node().await;
    let store = server.enroll(&node, 1).await;
    let old = store.identity().await.unwrap().unwrap().credential;
    let mut previous = server.socket("Bearer", &old).await.unwrap();
    frame(&mut previous, &node, Message::Hello(AgentInfo::current())).await;
    receive(&mut previous).await;
    let mut newest = server.socket("Bearer", &old).await.unwrap();
    frame(&mut newest, &node, Message::Hello(AgentInfo::current())).await;
    receive(&mut newest).await;
    let _ = tokio::time::timeout(Duration::from_secs(3), previous.next())
        .await
        .unwrap();
    tokio::time::sleep(Duration::from_millis(200)).await;
    let status: String = sqlx::query_scalar("SELECT status FROM nodes WHERE id = ?")
        .bind(&node)
        .fetch_one(&server.state.pool)
        .await
        .unwrap();
    assert_eq!(status, "online");
    let token = server.issue(&node).await;
    connection::enroll(&store, &server.url, &token, false, server.connector())
        .await
        .unwrap();
    unauthorized(server.socket("Bearer", &old).await.err().unwrap());
    let current = store.identity().await.unwrap().unwrap().credential;
    assert_ne!(current, old);
    sqlx::query("UPDATE node_credentials SET expires_at = ? WHERE node_id = ?")
        .bind(db::now() - 1)
        .bind(&node)
        .execute(&server.state.pool)
        .await
        .unwrap();
    unauthorized(server.socket("Bearer", &current).await.err().unwrap());
    let _ = newest.close(None).await;
    store.close().await;
}

fn metrics_fixture() -> panel_common::MetricsSnapshot {
    // Explicit transport fixture; real Linux collection is verified separately.
    panel_common::MetricsSnapshot {
        sampled_at: db::now(),
        cpu_percent: Some(25.0),
        memory: Some(panel_common::ByteUsage {
            total_bytes: "1000".into(),
            used_bytes: "400".into(),
            available_bytes: "600".into(),
        }),
        disk: Some(panel_common::ByteUsage {
            total_bytes: "10000".into(),
            used_bytes: "4000".into(),
            available_bytes: "5000".into(),
        }),
        network: Some(panel_common::NetworkUsage {
            received_bytes: "9007199254740993".into(),
            transmitted_bytes: "200".into(),
            received_bytes_per_second: Some(10.0),
            transmitted_bytes_per_second: None,
        }),
        issues: vec![],
    }
}

#[tokio::test]
async fn phase3_metrics_are_authenticated_latest_only_and_staleness_is_explicit() {
    let server = Harness::new().await;
    let node = server.node().await;
    let path = format!("/api/v1/nodes/{node}/metrics");
    let before = body(server.request("GET", &path, None).await).await;
    assert!(before["snapshot"].is_null());
    assert_eq!(before["stale"], true);
    let anonymous = server
        .router
        .clone()
        .oneshot(Request::builder().uri(&path).body(Body::empty()).unwrap())
        .await
        .unwrap();
    assert_eq!(anonymous.status(), StatusCode::UNAUTHORIZED);
    let store = server.enroll(&node, 1).await;
    let credential = store.identity().await.unwrap().unwrap().credential;
    let mut socket = server.socket("Bearer", &credential).await.unwrap();
    frame(&mut socket, &node, Message::Hello(AgentInfo::current())).await;
    receive(&mut socket).await;
    for value in [25.0, 50.0] {
        let mut snapshot = metrics_fixture();
        snapshot.cpu_percent = Some(value);
        frame(&mut socket, &node, Message::Metrics(snapshot)).await;
        let deadline = Instant::now() + Duration::from_secs(3);
        loop {
            let response = body(server.request("GET", &path, None).await).await;
            if response["snapshot"]["cpu_percent"] == value {
                assert_eq!(response["stale"], false);
                assert_eq!(
                    response["snapshot"]["network"]["received_bytes"],
                    "9007199254740993"
                );
                break;
            }
            assert!(Instant::now() < deadline);
            tokio::time::sleep(Duration::from_millis(30)).await;
        }
    }
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM node_metrics WHERE node_id = ?")
            .bind(&node)
            .fetch_one(&server.state.pool)
            .await
            .unwrap(),
        1
    );
    sqlx::query("UPDATE node_metrics SET received_at = ? WHERE node_id = ?")
        .bind(db::now() - 16)
        .bind(&node)
        .execute(&server.state.pool)
        .await
        .unwrap();
    assert_eq!(
        body(server.request("GET", &path, None).await).await["stale"],
        true
    );
    sqlx::query("UPDATE node_metrics SET received_at = ?, connection_id = 'previous-generation' WHERE node_id = ?").bind(db::now()).bind(&node).execute(&server.state.pool).await.unwrap();
    assert_eq!(
        body(server.request("GET", &path, None).await).await["stale"],
        true
    );
    assert_eq!(
        server
            .request("DELETE", &format!("/api/v1/nodes/{node}"), None)
            .await
            .status(),
        StatusCode::NO_CONTENT
    );
    assert_eq!(
        server.request("GET", &path, None).await.status(),
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM node_metrics WHERE node_id = ?")
            .bind(&node)
            .fetch_one(&server.state.pool)
            .await
            .unwrap(),
        0
    );
    let _ = socket.close(None).await;
    store.close().await;
}

#[tokio::test]
async fn phase3_invalid_or_cross_node_metrics_never_overwrite_observed_data() {
    let server = Harness::new().await;
    let node = server.node().await;
    let other = server.node().await;
    let store = server.enroll(&node, 1).await;
    let credential = store.identity().await.unwrap().unwrap().credential;
    for wrong_identity in [false, true] {
        let mut socket = server.socket("Bearer", &credential).await.unwrap();
        frame(&mut socket, &node, Message::Hello(AgentInfo::current())).await;
        receive(&mut socket).await;
        let mut snapshot = metrics_fixture();
        if !wrong_identity {
            snapshot.cpu_percent = Some(101.0);
        }
        frame(
            &mut socket,
            if wrong_identity { &other } else { &node },
            Message::Metrics(snapshot),
        )
        .await;
        let _ = tokio::time::timeout(Duration::from_secs(3), socket.next())
            .await
            .unwrap();
    }
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM node_metrics")
            .fetch_one(&server.state.pool)
            .await
            .unwrap(),
        0
    );
    store.close().await;
}

#[tokio::test]
async fn phase3_forbidden_units_methods_and_operations_are_denied_and_audited() {
    let server = Harness::new().await;
    let node = server.node().await;
    let path = format!("/api/v1/nodes/{node}/tasks");
    for operation in [
        json!({"service_status":{"service":"ssh.service"}}),
        json!({"service_status":{"service":"../../private-test-value"}}),
        json!({"service_status":{"service":"panel_agent", "method":"RestartUnit"}}),
        json!({"service_restart":{"service":"panel_agent"}}),
        json!({"shell":"id"}),
    ] {
        let response = server
            .request(
                "POST",
                &path,
                Some(json!({"operation":operation, "idempotency_key":Uuid::new_v4().to_string()})),
            )
            .await;
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    }
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM tasks")
            .fetch_one(&server.state.pool)
            .await
            .unwrap(),
        0
    );
    let audit = body(server.request("GET", "/api/v1/audit", None).await).await;
    let rejected: Vec<_> = audit
        .as_array()
        .unwrap()
        .iter()
        .filter(|entry| entry["action"] == "task.reject")
        .collect();
    assert_eq!(rejected.len(), 5);
    assert!(rejected.iter().all(|entry| entry["result"] == "denied" && entry["metadata"]["request_id"].is_string()));
    assert!(!audit.to_string().contains("private-test-value"));
}

#[tokio::test]
async fn phase3_service_tasks_keep_their_operation_and_idempotency_conflicts_are_rejected() {
    let server = Harness::new().await;
    let node = server.node().await;
    let key = Uuid::new_v4().to_string();
    let operation = json!({"service_status":{"service":"panel_agent"}});
    let path = format!("/api/v1/nodes/{node}/tasks");
    let input = json!({"operation":operation, "idempotency_key":key});
    let response = server.request("POST", &path, Some(input.clone())).await;
    assert_eq!(response.status(), StatusCode::CREATED);
    let task = body(response).await;
    assert_eq!(task["operation"], operation);
    let response = server.request("POST", &path, Some(input)).await;
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(body(response).await["id"], task["id"]);
    let response = server
        .request(
            "POST",
            &path,
            Some(json!({"operation":{"service_status":{"service":"xray"}}, "idempotency_key":key})),
        )
        .await;
    assert_eq!(response.status(), StatusCode::CONFLICT);
    assert_eq!(
        body(response).await["error"]["code"],
        "idempotency_conflict"
    );
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM tasks")
            .fetch_one(&server.state.pool)
            .await
            .unwrap(),
        1
    );
}

#[tokio::test]
async fn phase3_real_agent_reports_platform_capability_and_read_only_service_result() {
    let server = Harness::new().await;
    let node = server.node().await;
    let store = server.enroll(&node, 1).await;
    let (stop, receiver) = watch::channel(false);
    let task = {
        let store = store.clone();
        let connector = server.connector();
        tokio::spawn(async move { connection::run(&store, false, connector, receiver).await })
    };
    server.online(&node).await;
    let path = format!("/api/v1/nodes/{node}/metrics");
    let deadline = Instant::now() + Duration::from_secs(8);
    let metrics = loop {
        let response = body(server.request("GET", &path, None).await).await;
        if !response["snapshot"].is_null() {
            break response;
        }
        assert!(Instant::now() < deadline);
        tokio::time::sleep(Duration::from_millis(50)).await;
    };
    assert_eq!(metrics["stale"], false);
    #[cfg(target_os = "linux")]
    assert!(metrics["snapshot"]["memory"]["total_bytes"].is_string());
    #[cfg(not(target_os = "linux"))]
    {
        assert!(metrics["snapshot"]["memory"].is_null());
        assert_eq!(metrics["snapshot"]["issues"].as_array().unwrap().len(), 4);
    }
    let response = server.request("POST", &format!("/api/v1/nodes/{node}/tasks"), Some(json!({"operation":{"service_status":{"service":"panel_agent"}},"idempotency_key":Uuid::new_v4().to_string()}))).await;
    let id = body(response).await["id"].as_str().unwrap().to_owned();
    let deadline = Instant::now() + Duration::from_secs(8);
    loop {
        let result = body(
            server
                .request("GET", &format!("/api/v1/tasks/{id}"), None)
                .await,
        )
        .await;
        if ["succeeded", "failed"]
            .iter()
            .any(|status| result["status"] == *status)
        {
            #[cfg(not(target_os = "linux"))]
            {
                assert_eq!(result["status"], "failed");
                assert_eq!(result["result"]["data"]["reason"], "unsupported_platform");
            }
            break;
        }
        assert!(Instant::now() < deadline);
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    assert_eq!(
        sqlx::query_scalar::<_, i64>(
            "SELECT COUNT(*) FROM audit_logs WHERE action = 'service.status' AND target = ?"
        )
        .bind(&id)
        .fetch_one(&server.state.pool)
        .await
        .unwrap(),
        1
    );
    stop.send(true).unwrap();
    task.await.unwrap().unwrap();
    store.close().await;
}

#[tokio::test]
async fn phase3_a_service_result_cannot_impersonate_another_allowlisted_unit() {
    let server = Harness::new().await;
    let node = server.node().await;
    let store = server.enroll(&node, 1).await;
    let credential = store.identity().await.unwrap().unwrap().credential;
    let mut socket = server.socket("Bearer", &credential).await.unwrap();
    frame(&mut socket, &node, Message::Hello(AgentInfo::current())).await;
    receive(&mut socket).await;
    let response = server.request("POST", &format!("/api/v1/nodes/{node}/tasks"), Some(json!({"operation":{"service_status":{"service":"panel_agent"}},"idempotency_key":Uuid::new_v4().to_string()}))).await;
    let id = body(response).await["id"].as_str().unwrap().to_owned();
    assert!(matches!(
        receive(&mut socket).await.message,
        Message::Task(_)
    ));
    frame(
        &mut socket,
        &node,
        Message::TaskResult {
            task_id: id.clone(),
            result: panel_common::TaskOutcome::ServiceStatus(panel_common::ServiceReport {
                service: panel_common::ManagedService::Xray,
                load_state: "loaded".into(),
                active_state: Some("active".into()),
                sub_state: Some("running".into()),
            }),
        },
    )
    .await;
    let _ = tokio::time::timeout(Duration::from_secs(3), socket.next())
        .await
        .unwrap();
    let result: Option<String> = sqlx::query_scalar("SELECT result_json FROM tasks WHERE id = ?")
        .bind(&id)
        .fetch_one(&server.state.pool)
        .await
        .unwrap();
    assert!(result.is_none());
    store.close().await;
}

#[tokio::test]
async fn phase4_lifecycle_routes_enforce_pinned_inputs_and_bound_results() {
    let server = Harness::new().await;
    let node = server.node().await;
    let path = format!("/api/v1/nodes/{node}/xray/install");
    let response = server
        .request(
            "POST",
            &path,
            Some(json!({"version":"latest", "idempotency_key":Uuid::new_v4().to_string()})),
        )
        .await;
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    let key = Uuid::new_v4().to_string();
    let response = server
        .request(
            "POST",
            &path,
            Some(json!({"version":"26.10.31", "idempotency_key":key})),
        )
        .await;
    assert_eq!(response.status(), StatusCode::CREATED);
    let task = body(response).await;
    assert_eq!(
        task["operation"],
        json!({"xray":{"install":{"version":"26.10.31"}}})
    );
    assert!(task["deadline_at"].as_i64().unwrap() >= db::now() + 295);
    let invalid = server
        .request(
            "POST",
            &format!("/api/v1/nodes/{node}/xray/restart"),
            Some(json!({"version":"26.10.31", "idempotency_key":Uuid::new_v4().to_string()})),
        )
        .await;
    assert_eq!(invalid.status(), StatusCode::BAD_REQUEST);
    let report = panel_common::XrayReport {
        installed: true,
        version: Some(panel_common::XrayVersion::V26_10_21),
        previous_version: None,
        active: true,
    };
    assert!(!report.valid_for(&panel_common::XrayOperation::Install(
        panel_common::XrayRelease {
            version: panel_common::XrayVersion::V26_10_31
        }
    )));
}

#[tokio::test]
async fn phase4_a_long_inflight_task_is_not_replaced_by_a_new_queue_head() {
    let server = Harness::new().await;
    let node = server.node().await;
    let store = server.enroll(&node, 1).await;
    let credential = store.identity().await.unwrap().unwrap().credential;
    let mut socket = server.socket("Bearer", &credential).await.unwrap();
    frame(&mut socket, &node, Message::Hello(AgentInfo::current())).await;
    receive(&mut socket).await;
    let first = server.task(&node, &Uuid::new_v4().to_string()).await;
    let received = match receive(&mut socket).await.message {
        Message::Task(task) => task,
        _ => panic!("expected task"),
    };
    assert_eq!(received.id, first["id"]);
    let second = server.task(&node, &Uuid::new_v4().to_string()).await;
    sqlx::query("UPDATE tasks SET created_at = created_at - 10 WHERE id = ?")
        .bind(second["id"].as_str().unwrap())
        .execute(&server.state.pool)
        .await
        .unwrap();
    let repeated = tokio::time::timeout(Duration::from_secs(13), async {
        loop {
            match socket.next().await.unwrap().unwrap() {
                WsMessage::Text(text) => {
                    if let Message::Task(task) =
                        Envelope::parse(&text, Some(&node)).unwrap().message
                    {
                        break task;
                    }
                }
                WsMessage::Ping(data) => socket.send(WsMessage::Pong(data)).await.unwrap(),
                _ => (),
            }
        }
    })
    .await
    .unwrap();
    assert_eq!(repeated.id, received.id);
    let _ = socket.close(None).await;
    store.close().await;
}

#[tokio::test]
async fn phase5_config_snapshots_are_private_and_revision_results_are_bound() {
    let server = Harness::new().await;
    let node = server.node().await;
    let path = format!("/api/v1/nodes/{node}/inbounds");
    let input = json!({"name":"reality","listen_address":"127.0.0.1","listen_port":38443,"target":"www.example.com:443","server_names":["www.example.com"],"short_ids":["abcd"],"enabled":true});
    let response = server.request("POST", &path, Some(input.clone())).await;
    assert_eq!(response.status(), StatusCode::CREATED);
    let inbound = body(response).await;
    assert!(inbound["public_key"].is_string());
    assert!(!inbound.to_string().contains("private_key"));
    let inbound_id = inbound["id"].as_str().unwrap();
    assert_eq!(
        server
            .request("POST", &path, Some(input.clone()))
            .await
            .status(),
        StatusCode::CONFLICT
    );
    let mut invalid = input.clone();
    invalid["target"] = json!("file:/etc/shadow");
    assert_eq!(
        server
            .request(
                "PATCH",
                &format!("/api/v1/inbounds/{inbound_id}"),
                Some(invalid)
            )
            .await
            .status(),
        StatusCode::BAD_REQUEST
    );
    let created = body(
        server
            .request(
                "POST",
                "/api/v1/clients",
                Some(json!({"display_name":"alice","enabled":true})),
            )
            .await,
    )
    .await;
    let client_id = created["client"]["id"].as_str().unwrap();
    let credential = created["protocol_id"].as_str().unwrap();
    assert!(Uuid::parse_str(credential).is_ok());
    assert!(
        !body(server.request("GET", "/api/v1/clients", None).await)
            .await
            .to_string()
            .contains(credential)
    );
    assert_eq!(
        server
            .request(
                "POST",
                &format!("/api/v1/inbounds/{inbound_id}/clients/{client_id}"),
                None
            )
            .await
            .status(),
        StatusCode::NO_CONTENT
    );
    let apply = format!("/api/v1/nodes/{node}/config/apply");
    let key = json!({"idempotency_key":Uuid::new_v4().to_string()});
    let first = body(server.request("POST", &apply, Some(key.clone())).await).await;
    assert_eq!(first["revision"], 1);
    assert_eq!(
        body(server.request("POST", &apply, Some(key)).await).await["id"],
        first["id"]
    );
    let second = body(
        server
            .request(
                "POST",
                &apply,
                Some(json!({"idempotency_key":Uuid::new_v4().to_string()})),
            )
            .await,
    )
    .await;
    let task_id = second["task_id"].as_str().unwrap();
    let public = body(
        server
            .request("GET", &format!("/api/v1/tasks/{task_id}"), None)
            .await,
    )
    .await
    .to_string();
    assert!(
        !public.contains(credential)
            && !public.contains("private_key")
            && !public.contains("private_config")
    );
    let rows = body(
        server
            .request(
                "GET",
                &format!("/api/v1/nodes/{node}/config/revisions"),
                None,
            )
            .await,
    )
    .await;
    assert_eq!(rows[1]["status"], "failed");
    sqlx::query("UPDATE tasks SET deadline_at=? WHERE id=?")
        .bind(db::now() - 8 * 86400)
        .bind(task_id)
        .execute(&server.state.pool)
        .await
        .unwrap();
    db::cleanup(&server.state.pool).await.unwrap();
    assert!(
        sqlx::query_scalar::<_, bool>("SELECT EXISTS(SELECT 1 FROM tasks WHERE id=?)")
            .bind(task_id)
            .fetch_one(&server.state.pool)
            .await
            .unwrap(),
        "latest desired delivery must survive retention for offline reconciliation"
    );
    let store = server.enroll(&node, 1).await;
    let identity = store.identity().await.unwrap().unwrap();
    let mut socket = server.socket("Bearer", &identity.credential).await.unwrap();
    frame(&mut socket, &node, Message::Hello(AgentInfo::current())).await;
    receive(&mut socket).await;
    let task = match receive(&mut socket).await.message {
        Message::Task(task) => task,
        _ => panic!("expected configuration task"),
    };
    let reference = match &task.operation {
        panel_common::TaskOperation::ApplyConfig(reference) => reference.clone(),
        _ => panic!("wrong operation"),
    };
    assert_ne!(
        task.id, task_id,
        "expired delivery must be reconciled with a fresh task ID"
    );
    let private = task.private_config.as_ref().unwrap();
    assert_eq!(private.inbounds[0].clients[0].protocol_id, credential);
    assert_eq!(private.content_hash().unwrap(), reference.content_hash);
    frame(
        &mut socket,
        &node,
        Message::TaskAck {
            task_id: task.id.clone(),
        },
    )
    .await;
    frame(
        &mut socket,
        &node,
        Message::TaskResult {
            task_id: task.id.clone(),
            result: panel_common::TaskOutcome::ConfigApplied(reference),
        },
    )
    .await;
    receive(&mut socket).await;
    server.finished(&task.id).await;
    let rows = body(
        server
            .request(
                "GET",
                &format!("/api/v1/nodes/{node}/config/revisions"),
                None,
            )
            .await,
    )
    .await;
    assert_eq!(rows[0]["status"], "applied");
    assert!(rows[0]["applied_at"].is_number());
    assert!(!rows.to_string().contains(credential));
    let _ = socket.close(None).await;
    store.close().await;
    assert_eq!(
        server
            .request("DELETE", &format!("/api/v1/clients/{client_id}"), None)
            .await
            .status(),
        StatusCode::NO_CONTENT
    );
    assert_eq!(
        server
            .request("DELETE", &format!("/api/v1/inbounds/{inbound_id}"), None)
            .await
            .status(),
        StatusCode::NO_CONTENT
    );
}

#[tokio::test]
async fn phase6_usage_is_persistent_deduplicated_reset_safe_and_policy_preserves_drafts() {
    use panel_common::traffic::{ClientCounter, TrafficReport, TrafficSample};
    let server = Harness::new().await;
    let node = server.node().await;
    let input = json!({"name":"metered","listen_address":"127.0.0.1","listen_port":38443,"target":"www.example.com:443","server_names":["www.example.com"],"short_ids":["abcd"],"enabled":true});
    let inbound = body(
        server
            .request(
                "POST",
                &format!("/api/v1/nodes/{node}/inbounds"),
                Some(input.clone()),
            )
            .await,
    )
    .await;
    let inbound_id = inbound["id"].as_str().unwrap();
    let first = body(
        server
            .request(
                "POST",
                "/api/v1/clients",
                Some(json!({"display_name":"quota","enabled":true,"quota_bytes":"50"})),
            )
            .await,
    )
    .await;
    let second = body(
        server
            .request(
                "POST",
                "/api/v1/clients",
                Some(json!({"display_name":"expiry","enabled":true,"expiry_at":db::now()+3600})),
            )
            .await,
    )
    .await;
    let first_id = first["client"]["id"].as_str().unwrap();
    let second_id = second["client"]["id"].as_str().unwrap();
    for client in [first_id, second_id] {
        assert_eq!(
            server
                .request(
                    "POST",
                    &format!("/api/v1/inbounds/{inbound_id}/clients/{client}"),
                    None
                )
                .await
                .status(),
            StatusCode::NO_CONTENT
        );
    }
    assert_eq!(
        server
            .request(
                "POST",
                &format!("/api/v1/nodes/{node}/config/apply"),
                Some(json!({"idempotency_key":Uuid::new_v4().to_string()}))
            )
            .await
            .status(),
        StatusCode::CREATED
    );
    let mut draft = input;
    draft["listen_port"] = json!(38444);
    assert_eq!(
        server
            .request(
                "PATCH",
                &format!("/api/v1/inbounds/{inbound_id}"),
                Some(draft)
            )
            .await
            .status(),
        StatusCode::OK
    );
    let store = server.enroll(&node, 1).await;
    let credential = store.identity().await.unwrap().unwrap().credential;
    let mut socket = server.socket("Bearer", &credential).await.unwrap();
    frame(&mut socket, &node, Message::Hello(AgentInfo::current())).await;
    receive(&mut socket).await;
    let at = db::now();
    let instance = "a".repeat(32);
    let sample = |time, epoch: String, up, down| TrafficSample {
        instance_id: epoch,
        collected_at: time,
        counters: vec![ClientCounter {
            client_id: first_id.into(),
            uplink_bytes: up,
            downlink_bytes: down,
        }],
    };
    let baseline = sample(at, instance.clone(), 10, 10);
    let usage = sample(at + 1, instance.clone(), 50, 60);
    for value in [
        baseline,
        usage.clone(),
        usage.clone(),
        sample(at, instance.clone(), 999, 999),
        sample(at + 2, "b".repeat(32), 0, 0),
        sample(at + 3, "b".repeat(32), 5, 7),
    ] {
        frame(
            &mut socket,
            &node,
            Message::Traffic(TrafficReport::Available(value)),
        )
        .await;
    }
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        let used = sqlx::query_scalar::<_, i64>("SELECT used_bytes FROM clients WHERE id=?")
            .bind(first_id)
            .fetch_one(&server.state.pool)
            .await
            .unwrap();
        if used == 102 {
            break;
        }
        assert!(Instant::now() < deadline, "unexpected usage {used}");
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    let policy=sqlx::query_as::<_,(i64,String)>("SELECT revision,desired_config_json FROM config_revisions WHERE node_id=? ORDER BY revision DESC LIMIT 1").bind(&node).fetch_one(&server.state.pool).await.unwrap();
    assert_eq!(policy.0, 2);
    let desired: panel_common::configuration::NodeConfig = serde_json::from_str(&policy.1).unwrap();
    assert_eq!(
        desired.inbounds[0].listen_port, 38443,
        "quota must not activate an unapplied draft"
    );
    assert_eq!(desired.inbounds[0].clients.len(), 1);
    assert_eq!(desired.inbounds[0].clients[0].id, second_id);
    let client = body(
        server
            .request("GET", &format!("/api/v1/clients/{first_id}"), None)
            .await,
    )
    .await;
    assert_eq!(client["used_bytes"], "102");
    assert_eq!(client["policy_blocked"], true);
    let response = server
        .request(
            "PATCH",
            &format!("/api/v1/clients/{second_id}"),
            Some(json!({"display_name":"expiry","enabled":true,"expiry_at":0})),
        )
        .await;
    assert_eq!(response.status(), StatusCode::OK);
    panel_server::traffic::enforce(&server.state.pool)
        .await
        .unwrap();
    panel_server::traffic::enforce(&server.state.pool)
        .await
        .unwrap();
    let (revision,private)=sqlx::query_as::<_,(i64,String)>("SELECT revision,desired_config_json FROM config_revisions WHERE node_id=? ORDER BY revision DESC LIMIT 1").bind(&node).fetch_one(&server.state.pool).await.unwrap();
    assert_eq!(revision, 3);
    let desired: panel_common::configuration::NodeConfig = serde_json::from_str(&private).unwrap();
    assert!(desired.inbounds[0].clients.is_empty());
    let traffic = body(
        server
            .request("GET", &format!("/api/v1/nodes/{node}/traffic"), None)
            .await,
    )
    .await;
    assert_eq!(traffic["available"], true);
    assert_eq!(traffic["stale"], false);
    assert_eq!(traffic["clients"][0]["uplink_bytes"], "45");
    assert_eq!(traffic["clients"][0]["downlink_bytes"], "57");
    assert!(
        !traffic
            .to_string()
            .contains(first["protocol_id"].as_str().unwrap())
    );
    let _ = socket.close(None).await;
    store.close().await;
}

#[tokio::test]
async fn phase7_agent_retirement_revokes_only_its_bound_credential_and_allows_reenrollment() {
    let server = Harness::new().await;
    let node = server.node().await;
    let other = server.node().await;
    let store = server.enroll(&node, 1).await;
    let other_store = server.enroll(&other, 2).await;
    let old = store.identity().await.unwrap().unwrap().credential;
    connection::retire(&store, false, server.connector())
        .await
        .unwrap();
    unauthorized(server.socket("Bearer", &old).await.err().unwrap());
    assert!(
        server
            .socket(
                "Bearer",
                &other_store.identity().await.unwrap().unwrap().credential
            )
            .await
            .is_ok()
    );
    let token = server.issue(&node).await;
    assert_eq!(
        connection::enroll(&store, &server.url, &token, false, server.connector())
            .await
            .unwrap(),
        node
    );
    assert_ne!(store.identity().await.unwrap().unwrap().credential, old);
    let audit = body(server.request("GET", "/api/v1/audit", None).await).await;
    assert!(
        audit
            .as_array()
            .unwrap()
            .iter()
            .any(|entry| entry["action"] == "agent.retire" && entry["node_id"] == node)
    );
    store.close().await;
    other_store.close().await;
}
