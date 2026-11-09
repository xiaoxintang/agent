use super::*;
use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use panel_common::{
    configuration::{ConfigClient, ConfigInbound},
    traffic::{ClientCounter, TrafficSample},
};
use uuid::Uuid;

#[tokio::test]
async fn scoped_quota_enforcement_is_atomic_and_finds_committed_nodes_after_draft_unassignment() {
    let directory = tempfile::tempdir().unwrap();
    let pool = db::open(&directory.path().join("data")).await.unwrap();
    let state = AppState::new(pool.clone(), true).await.unwrap();
    let metered = Uuid::new_v4().to_string();
    let expired = Uuid::new_v4().to_string();
    for client in [&metered, &expired] {
        sqlx::query("INSERT INTO clients (id,display_name,protocol_identity_json,enabled,quota_bytes,expiry_at,created_at) VALUES (?,?,'\"identity\"',1,?,?,?)")
            .bind(client).bind(client).bind(if client==&metered {Some(50)} else {None})
            .bind(if client==&expired {Some(0)} else {None}).bind(db::now())
            .execute(&pool).await.unwrap();
    }
    let nodes = [
        Uuid::new_v4().to_string(),
        Uuid::new_v4().to_string(),
        Uuid::new_v4().to_string(),
    ];
    for (index, node) in nodes.iter().enumerate() {
        sqlx::query("INSERT INTO nodes (id,name,status,connection_id,created_at) VALUES (?,?,'online','current',?)")
            .bind(node).bind(node).bind(db::now()).execute(&pool).await.unwrap();
        let client = if index == 2 { &expired } else { &metered };
        let inbound = Uuid::new_v4().to_string();
        let config = NodeConfig {
            inbounds: vec![ConfigInbound {
                id: inbound.clone(),
                listen_address: "127.0.0.1".into(),
                listen_port: 38443,
                target: "www.example.com:443".into(),
                server_names: vec!["www.example.com".into()],
                short_ids: vec!["abcd".into()],
                private_key: URL_SAFE_NO_PAD.encode([7; 32]),
                clients: vec![ConfigClient {
                    id: client.clone(),
                    protocol_id: Uuid::new_v4().to_string(),
                }],
            }],
        };
        sqlx::query("INSERT INTO config_revisions (id,node_id,revision,desired_config_json,content_hash,status,created_at) VALUES (?,?,1,?,?,'applied',?)")
            .bind(Uuid::new_v4().to_string()).bind(node).bind(serde_json::to_string(&config).unwrap())
            .bind(config.content_hash().unwrap()).bind(db::now()).execute(&pool).await.unwrap();
        // Only the reporting node retains a draft assignment. The other node's
        // committed snapshot must still be included in quota removal.
        if index == 0 {
            sqlx::query("INSERT INTO inbounds (id,node_id,name,listen_address,listen_port,settings_json,enabled,created_at,updated_at) VALUES (?,?,'draft','127.0.0.1',38444,'{}',1,?,?)")
                .bind(&inbound).bind(node).bind(db::now()).bind(db::now()).execute(&pool).await.unwrap();
            sqlx::query("INSERT INTO client_assignments (id,client_id,inbound_id) VALUES (?,?,?)")
                .bind(Uuid::new_v4().to_string())
                .bind(client)
                .bind(inbound)
                .execute(&pool)
                .await
                .unwrap();
        }
    }
    let now = db::now();
    let sample = |at, up| {
        TrafficReport::Available(TrafficSample {
            instance_id: "a".repeat(32),
            collected_at: at,
            counters: vec![ClientCounter {
                client_id: metered.clone(),
                uplink_bytes: up,
                downlink_bytes: 0,
            }],
        })
    };
    record(&state, &nodes[0], "current", &sample(now, 10))
        .await
        .unwrap();
    sqlx::raw_sql("CREATE TRIGGER fail_policy BEFORE INSERT ON config_revisions WHEN NEW.revision>1 BEGIN SELECT RAISE(ABORT, 'private-policy-error'); END;")
        .execute(&pool).await.unwrap();
    assert!(
        record(&state, &nodes[0], "current", &sample(now + 1, 70))
            .await
            .is_err()
    );
    let usage = sqlx::query_as::<_, (i64, bool)>(
        "SELECT used_bytes,policy_blocked FROM clients WHERE id=?",
    )
    .bind(&metered)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(usage, (0, false));
    assert_eq!(
        sqlx::query_scalar::<_, i64>(
            "SELECT uplink_bytes FROM traffic_baselines WHERE node_id=? AND client_id=?"
        )
        .bind(&nodes[0])
        .bind(&metered)
        .fetch_one(&pool)
        .await
        .unwrap(),
        10
    );
    sqlx::query("DROP TRIGGER fail_policy")
        .execute(&pool)
        .await
        .unwrap();
    let mut events = state.agent_events.subscribe();
    record(&state, &nodes[0], "current", &sample(now + 1, 70))
        .await
        .unwrap();
    let notified = BTreeSet::from([events.try_recv().unwrap(), events.try_recv().unwrap()]);
    assert_eq!(
        notified,
        BTreeSet::from([nodes[0].clone(), nodes[1].clone()])
    );
    assert!(events.try_recv().is_err());
    record(&state, &nodes[0], "current", &sample(now + 1, 70))
        .await
        .unwrap();
    assert!(events.try_recv().is_err());
    for (index, node) in nodes.iter().enumerate() {
        let (revision, private) = sqlx::query_as::<_,(i64,String)>("SELECT revision,desired_config_json FROM config_revisions WHERE node_id=? ORDER BY revision DESC LIMIT 1")
            .bind(node).fetch_one(&pool).await.unwrap();
        assert_eq!(revision, if index == 2 { 1 } else { 2 });
        let config: NodeConfig = serde_json::from_str(&private).unwrap();
        assert_eq!(config.inbounds[0].listen_port, 38443);
        assert_eq!(config.inbounds[0].clients.is_empty(), index != 2);
    }
    let usage = sqlx::query_as::<_, (i64, bool)>(
        "SELECT used_bytes,policy_blocked FROM clients WHERE id=?",
    )
    .bind(&metered)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(usage, (60, true));
    assert!(
        !sqlx::query_scalar::<_, bool>("SELECT policy_blocked FROM clients WHERE id=?")
            .bind(expired)
            .fetch_one(&pool)
            .await
            .unwrap()
    );
    assert_eq!(enforce(&pool).await.unwrap(), vec![nodes[2].clone()]);
    assert!(enforce(&pool).await.unwrap().is_empty());
    pool.close().await;
}
