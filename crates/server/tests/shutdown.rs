#![cfg(unix)]

use futures_util::{SinkExt, StreamExt};
use panel_common::{
    AgentInfo,
    protocol::{Envelope, Message},
};
use panel_server::{auth, db};
use std::{os::unix::fs::PermissionsExt, process::Stdio, time::Duration};
use tokio::{
    io::{AsyncBufReadExt, BufReader},
    process::Command,
};
use tokio_tungstenite::tungstenite::{Message as WsMessage, client::IntoClientRequest};
use uuid::Uuid;

#[tokio::test]
async fn sigterm_drains_agent_connections_and_exits_cleanly_in_http_and_https_modes() {
    for tls in [false, true] {
        let directory = tempfile::tempdir().unwrap();
        let data = directory.path().join("data");
        let pool = db::open(&data).await.unwrap();
        let node = Uuid::new_v4().to_string();
        let credential = auth::random_token();
        sqlx::query("INSERT INTO nodes (id,name,created_at) VALUES (?,'shutdown-test',?)")
            .bind(&node)
            .bind(db::now())
            .execute(&pool)
            .await
            .unwrap();
        sqlx::query("INSERT INTO node_credentials (node_id,credential_hash,issued_at,expires_at) VALUES (?,?,?,?)")
            .bind(&node).bind(auth::token_hash(&credential)).bind(db::now()).bind(db::now()+60)
            .execute(&pool).await.unwrap();
        let mut command = Command::new(env!("CARGO_BIN_EXE_panel-server"));
        command
            .env_clear()
            .env("PANEL_BIND", "127.0.0.1:0")
            .env("PANEL_DATA_DIR", &data)
            .stdout(Stdio::piped())
            .kill_on_drop(true);
        let cert_path = directory.path().join("cert.pem");
        if tls {
            let cert = rcgen::generate_simple_self_signed(vec!["127.0.0.1".into()]).unwrap();
            let key = directory.path().join("key.pem");
            std::fs::write(&cert_path, cert.cert.pem()).unwrap();
            std::fs::write(&key, cert.signing_key.serialize_pem()).unwrap();
            std::fs::set_permissions(&key, std::fs::Permissions::from_mode(0o600)).unwrap();
            command
                .env("PANEL_TLS_CERT", &cert_path)
                .env("PANEL_TLS_KEY", &key);
        } else {
            command.env("PANEL_DEV_HTTP", "1");
        }
        let mut child = command.spawn().unwrap();
        let mut lines = BufReader::new(child.stdout.take().unwrap()).lines();
        let origin = tokio::time::timeout(Duration::from_secs(10), async {
            while let Some(line) = lines.next_line().await.unwrap() {
                if let Some((_, origin)) = line.split_once(" listening on ") {
                    return origin.to_owned();
                }
            }
            panic!("server exited before listening");
        })
        .await
        .unwrap();
        let url = format!("{}/agent/v1/connect", origin.replacen("http", "ws", 1));
        let mut request = url.into_client_request().unwrap();
        request.headers_mut().insert(
            "authorization",
            format!("Bearer {credential}").parse().unwrap(),
        );
        let connector =
            panel_agent::config::connector(if tls { Some(&cert_path) } else { None }).unwrap();
        let (mut socket, _) =
            tokio_tungstenite::connect_async_tls_with_config(request, None, false, Some(connector))
                .await
                .unwrap();
        socket
            .send(WsMessage::Text(
                serde_json::to_string(&Envelope::new(&node, Message::Hello(AgentInfo::current())))
                    .unwrap()
                    .into(),
            ))
            .await
            .unwrap();
        let welcome = tokio::time::timeout(Duration::from_secs(5), socket.next())
            .await
            .unwrap()
            .unwrap()
            .unwrap();
        let WsMessage::Text(welcome) = welcome else {
            panic!("expected Welcome")
        };
        assert!(matches!(
            Envelope::parse(&welcome, Some(&node)).unwrap().message,
            Message::Welcome
        ));
        assert!(
            Command::new("kill")
                .args(["-TERM", &child.id().unwrap().to_string()])
                .status()
                .await
                .unwrap()
                .success()
        );
        let status = tokio::time::timeout(Duration::from_secs(10), child.wait())
            .await
            .unwrap()
            .unwrap();
        assert!(
            status.success(),
            "SIGTERM bypassed graceful shutdown (TLS={tls}): {status}"
        );
        let state = sqlx::query_as::<_, (String, Option<String>)>(
            "SELECT status,connection_id FROM nodes WHERE id=?",
        )
        .bind(node)
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(state, ("offline".into(), None));
        pool.close().await;
    }
}
