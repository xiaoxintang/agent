#[cfg(target_os = "linux")]
async fn run() -> Result<(), Box<dyn std::error::Error>> {
    use panel_common::traffic::{ClientCounter, TrafficSample};
    use tokio::{
        io::{AsyncReadExt, AsyncWriteExt},
        process::Command,
    };
    if rustix::process::geteuid().as_raw() == 0 {
        return Err("Run as an ordinary user".into());
    }
    let binary = std::env::args_os()
        .nth(1)
        .ok_or("Usage: traffic_acceptance VERIFIED_XRAY")?;
    let directory = tempfile::tempdir()?;
    let key = Command::new(&binary)
        .args([
            "x25519",
            "-i",
            "BwcHBwcHBwcHBwcHBwcHBwcHBwcHBwcHBwcHBwcHBwc",
        ])
        .output()
        .await?;
    if !key.status.success() {
        return Err("test key derivation failed".into());
    }
    let text = String::from_utf8(key.stdout)?;
    let public = text
        .lines()
        .find_map(|line| {
            let (label, value) = line.split_once(':')?;
            matches!(label, "Password" | "PublicKey" | "Password (PublicKey)")
                .then_some(value.trim())
        })
        .ok_or("unexpected Xray x25519 output")?;
    let reservation = std::net::TcpListener::bind("127.0.0.1:0")?;
    let server_port = reservation.local_addr()?.port();
    drop(reservation);
    let client = panel_common::configuration::ConfigClient {
        id: uuid::Uuid::new_v4().to_string(),
        protocol_id: uuid::Uuid::new_v4().to_string(),
    };
    let config = panel_common::configuration::NodeConfig {
        inbounds: vec![panel_common::configuration::ConfigInbound {
            id: uuid::Uuid::new_v4().to_string(),
            listen_address: "127.0.0.1".into(),
            listen_port: server_port,
            target: "www.microsoft.com:443".into(),
            server_names: vec!["www.microsoft.com".into()],
            private_key: "BwcHBwcHBwcHBwcHBwcHBwcHBwcHBwcHBwcHBwcHBwc".into(),
            short_ids: vec!["abcd".into()],
            clients: vec![client.clone()],
        }],
    };
    let echo = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
    let echo_port = echo.local_addr()?.port();
    let path = directory.path().join("server.json");
    let mut fixture: serde_json::Value = serde_json::from_slice(&config.render()?)?;
    fixture["log"]["loglevel"] = serde_json::json!("debug");
    fixture["outbounds"][0]["settings"] = serde_json::json!({"finalRules":[{"action":"allow","ip":["127.0.0.1"],"port":echo_port.to_string()}]});
    // Isolated acceptance only: permit the one local echo port, while Stats remains blocked.
    fixture["routing"]["rules"].as_array_mut().unwrap().insert(1,serde_json::json!({"type":"field","ip":["127.0.0.1"],"port":echo_port.to_string(),"outboundTag":"direct"}));
    panel_installer::xray::lifecycle::atomic(&path, &serde_json::to_vec(&fixture)?, 0o600)
        .map_err(|error| format!("config staging: {error:?}"))?;
    let server_log = tempfile::NamedTempFile::new_in(directory.path())?;
    let client_log = tempfile::NamedTempFile::new_in(directory.path())?;
    let mut server = Command::new(&binary)
        .args(["run", "-config"])
        .arg(&path)
        .stdout(server_log.reopen()?)
        .stderr(server_log.reopen()?)
        .kill_on_drop(true)
        .spawn()?;
    tokio::time::sleep(std::time::Duration::from_millis(600)).await;
    println!("acceptance: real Xray server and loopback StatsService ready");
    let before = panel_common::stats::counters().await?;
    assert!(
        before
            .iter()
            .all(|counter| counter.uplink_bytes == 0 && counter.downlink_bytes == 0)
    );
    let epoch = uuid::Uuid::new_v4().simple().to_string();
    let proxy = std::net::TcpListener::bind("127.0.0.1:0")?;
    let proxy_port = proxy.local_addr()?.port();
    drop(proxy);
    let proxy_config = serde_json::json!({"log":{"loglevel":"debug"},"inbounds":[{"listen":"127.0.0.1","port":proxy_port,"protocol":"socks","settings":{"auth":"noauth","udp":false}}],"outbounds":[{"protocol":"vless","settings":{"vnext":[{"address":"127.0.0.1","port":server_port,"users":[{"id":client.protocol_id,"encryption":"none","flow":"xtls-rprx-vision"}]}]},"streamSettings":{"network":"tcp","security":"reality","realitySettings":{"serverName":"www.microsoft.com","fingerprint":"chrome","password":public,"publicKey":public,"shortId":"abcd","spiderX":"/"}}}]});
    let client_path = directory.path().join("client.json");
    panel_installer::xray::lifecycle::atomic(
        &client_path,
        &serde_json::to_vec(&proxy_config)?,
        0o600,
    )
    .map_err(|error| format!("client staging: {error:?}"))?;
    let mut proxy = Command::new(&binary)
        .args(["run", "-config"])
        .arg(&client_path)
        .stdout(client_log.reopen()?)
        .stderr(client_log.reopen()?)
        .kill_on_drop(true)
        .spawn()?;
    tokio::time::sleep(std::time::Duration::from_millis(600)).await;
    println!("acceptance: real Xray client started");
    let echo_task = tokio::spawn(async move {
        let (stream, _) = echo.accept().await?;
        let (mut read, mut write) = stream.into_split();
        tokio::io::copy(&mut read, &mut write).await
    });
    let transfer = tokio::time::timeout(std::time::Duration::from_secs(30), async {
        let mut socket = tokio::net::TcpStream::connect(("127.0.0.1", proxy_port)).await?;
        socket.write_all(&[5, 1, 0]).await?;
        let mut negotiation = [0; 2];
        socket.read_exact(&mut negotiation).await?;
        assert_eq!(negotiation, [5, 0]);
        let mut request = vec![5, 1, 0, 1, 127, 0, 0, 1];
        request.extend_from_slice(&echo_port.to_be_bytes());
        socket.write_all(&request).await?;
        println!("acceptance: SOCKS destination requested");
        let mut response = [0; 4];
        socket.read_exact(&mut response).await?;
        assert_eq!(&response[..2], &[5, 0]);
        let size = match response[3] {
            1 => 6,
            4 => 18,
            _ => return Err(std::io::Error::other("unexpected SOCKS address")),
        };
        let mut address = vec![0; size];
        socket.read_exact(&mut address).await?;
        println!("acceptance: SOCKS destination acknowledged");
        socket.write_all(&[0x33; 16]).await?;
        let mut warmup = [0; 16];
        socket.read_exact(&mut warmup).await?;
        assert_eq!(warmup, [0x33; 16]);
        let baseline = panel_common::stats::counters()
            .await
            .map_err(std::io::Error::other)?
            .into_iter()
            .find(|counter| counter.client_id == client.id)
            .ok_or_else(|| std::io::Error::other("missing warmed client counters"))?;
        let payload = vec![0x5a; 32768];
        let mut received = vec![0; payload.len()];
        for _ in 0..8 {
            socket.write_all(&payload).await?;
            socket.read_exact(&mut received).await?;
            assert_eq!(received, payload);
        }
        socket.shutdown().await?;
        Ok::<_, std::io::Error>(baseline)
    })
    .await;
    if !matches!(transfer, Ok(Ok(_))) {
        for (label, log) in [("server", server_log.path()), ("client", client_log.path())] {
            let mut diagnostic = std::fs::read_to_string(log)?;
            for secret in [
                &client.id,
                &client.protocol_id,
                public,
                "BwcHBwcHBwcHBwcHBwcHBwcHBwcHBwcHBwcHBwcHBwc",
            ] {
                diagnostic = diagnostic.replace(secret, "[test credential redacted]");
            }
            eprintln!(
                "acceptance {label} diagnostic: {}",
                diagnostic.chars().take(6000).collect::<String>()
            );
        }
    }
    let baseline = transfer??;
    echo_task.abort();
    tokio::time::sleep(std::time::Duration::from_millis(200)).await;
    let after = panel_common::stats::counters().await?;
    let counter = after
        .iter()
        .find(|counter| counter.client_id == client.id)
        .ok_or("missing client counters")?;
    let at = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)?
        .as_secs() as i64;
    let sample = TrafficSample {
        instance_id: epoch.clone(),
        collected_at: at,
        counters: after.clone(),
    };
    let delta = panel_common::traffic::delta(
        Some((
            &epoch,
            at - 15,
            baseline.uplink_bytes,
            baseline.downlink_bytes,
        )),
        &sample,
        counter,
    );
    assert!(
        delta.0 >= 262144 && delta.0 < 524288 && delta.1 >= 262144 && delta.1 < 524288,
        "controlled payload accounting outside expected bounds"
    );
    server.kill().await?;
    server.wait().await?;
    server = Command::new(&binary)
        .args(["run", "-config"])
        .arg(&path)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .kill_on_drop(true)
        .spawn()?;
    tokio::time::sleep(std::time::Duration::from_millis(600)).await;
    let reset = panel_common::stats::counters().await?;
    assert!(
        reset
            .iter()
            .all(|counter| counter.uplink_bytes == 0 && counter.downlink_bytes == 0)
    );
    let reset_sample = TrafficSample {
        instance_id: uuid::Uuid::new_v4().simple().to_string(),
        collected_at: at + 15,
        counters: reset,
    };
    let reset_counter = ClientCounter {
        client_id: client.id,
        uplink_bytes: 0,
        downlink_bytes: 0,
    };
    assert_eq!(
        panel_common::traffic::delta(
            Some((&epoch, at, counter.uplink_bytes, counter.downlink_bytes)),
            &reset_sample,
            &reset_counter
        ),
        (0, 0)
    );
    proxy.kill().await?;
    server.kill().await?;
    println!(
        "{{\"real_vless_reality_traffic\":true,\"payload_each_direction\":262144,\"uplink_bytes\":{},\"downlink_bytes\":{},\"grpc_stats\":true,\"restart_reset_safe\":true}}",
        delta.0, delta.1
    );
    Ok(())
}
#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    #[cfg(target_os = "linux")]
    return run().await;
    #[cfg(not(target_os = "linux"))]
    Err("Linux acceptance required".into())
}
