use crate::{AgentError, config, store::Store};
use futures_util::{SinkExt, StreamExt};
use panel_common::{
    AgentInfo, TaskCommand, TaskOperation, TaskOutcome,
    protocol::{Envelope, HEARTBEAT_SECONDS, MAX_MESSAGE_BYTES, Message, OFFLINE_SECONDS},
};
use std::time::{Duration, Instant};
use tokio::{net::TcpStream, sync::watch};
use tokio_tungstenite::{
    Connector, MaybeTlsStream, WebSocketStream,
    tungstenite::{
        self, Message as WsMessage, client::IntoClientRequest, protocol::WebSocketConfig,
    },
};

type Socket = WebSocketStream<MaybeTlsStream<TcpStream>>;

async fn connect(
    url: &str,
    kind: &str,
    token: &str,
    connector: Connector,
) -> Result<Socket, AgentError> {
    let mut request = url
        .into_client_request()
        .map_err(|_| AgentError::Configuration)?;
    let mut auth = format!("{kind} {token}")
        .parse::<tungstenite::http::HeaderValue>()
        .map_err(|_| AgentError::Configuration)?;
    auth.set_sensitive(true);
    request.headers_mut().insert("authorization", auth);
    let mut options = WebSocketConfig::default();
    options.max_message_size = Some(MAX_MESSAGE_BYTES);
    options.max_frame_size = Some(MAX_MESSAGE_BYTES);
    options.write_buffer_size = 0;
    options.max_write_buffer_size = MAX_MESSAGE_BYTES * 2;
    let (socket, _) = tokio::time::timeout(Duration::from_secs(10), tokio_tungstenite::connect_async_tls_with_config(request, Some(options), false, Some(connector))).await.map_err(|_| AgentError::Connection)?.map_err(|error| {
        if matches!(error, tungstenite::Error::Http(ref response) if response.status().as_u16() == 401 || response.status().as_u16() == 403) { AgentError::Authentication } else { AgentError::Connection }
    })?;
    Ok(socket)
}

async fn send(socket: &mut Socket, node_id: &str, message: Message) -> Result<(), AgentError> {
    let text = serde_json::to_string(&Envelope::new(node_id, message))
        .map_err(|_| AgentError::Protocol)?;
    tokio::time::timeout(
        Duration::from_secs(5),
        socket.send(WsMessage::Text(text.into())),
    )
    .await
    .map_err(|_| AgentError::Connection)?
    .map_err(|_| AgentError::Connection)
}

async fn receive(socket: &mut Socket, node_id: Option<&str>) -> Result<Envelope, AgentError> {
    loop {
        match socket
            .next()
            .await
            .ok_or(AgentError::Connection)?
            .map_err(|_| AgentError::Connection)?
        {
            WsMessage::Text(text) => {
                return Envelope::parse(&text, node_id).map_err(|_| AgentError::Protocol);
            }
            WsMessage::Ping(data) => socket
                .send(WsMessage::Pong(data))
                .await
                .map_err(|_| AgentError::Connection)?,
            WsMessage::Pong(_) => (),
            _ => return Err(AgentError::Protocol),
        }
    }
}

pub async fn enroll(
    store: &Store,
    server: &str,
    token: &str,
    allow_local_ws: bool,
    connector: Connector,
) -> Result<String, AgentError> {
    crate::security::require_unprivileged()?;
    let url = config::server_url(server, allow_local_ws)?;
    if token.len() != 64 || !token.bytes().all(|value| value.is_ascii_hexdigit()) {
        return Err(AgentError::Configuration);
    }
    if store
        .identity()
        .await?
        .is_some_and(|identity| identity.server_url != url)
    {
        return Err(AgentError::Configuration);
    }
    let mut socket = connect(&url, "Enrollment", token, connector).await?;
    let frame = tokio::time::timeout(Duration::from_secs(10), receive(&mut socket, None))
        .await
        .map_err(|_| AgentError::Connection)??;
    let Message::Enrolled { credential } = frame.message else {
        return Err(AgentError::Protocol);
    };
    store
        .save_identity(&url, &frame.node_id, &credential)
        .await?;
    send(
        &mut socket,
        &frame.node_id,
        Message::Hello(AgentInfo::current()),
    )
    .await?;
    let welcome = tokio::time::timeout(
        Duration::from_secs(10),
        receive(&mut socket, Some(&frame.node_id)),
    )
    .await
    .map_err(|_| AgentError::Connection)??;
    if !matches!(welcome.message, Message::Welcome) {
        return Err(AgentError::Protocol);
    }
    let _ = socket.close(None).await;
    Ok(frame.node_id)
}

pub async fn run(
    store: &Store,
    allow_local_ws: bool,
    connector: Connector,
    mut shutdown: watch::Receiver<bool>,
) -> Result<(), AgentError> {
    crate::security::require_unprivileged()?;
    let identity = store.identity().await?.ok_or(AgentError::Configuration)?;
    let url = config::server_url(&identity.server_url, allow_local_ws)?;
    let mut failures = 0_u32;
    loop {
        if *shutdown.borrow() {
            return Ok(());
        }
        let started = Instant::now();
        let session_shutdown = shutdown.clone();
        let result = tokio::select! {
            _ = shutdown.changed() => return Ok(()),
            result = session(store, &url, &identity.node_id, &identity.credential, connector.clone(), session_shutdown) => result,
        };
        match result {
            Ok(()) => return Ok(()),
            Err(AgentError::Connection) => (),
            Err(error) => return Err(error),
        }
        if started.elapsed() >= Duration::from_secs(OFFLINE_SECONDS) {
            failures = 0;
        }
        let seconds = (1_u64 << failures.min(6)).min(60);
        failures = failures.saturating_add(1);
        let jitter = u64::from(uuid::Uuid::new_v4().as_bytes()[0]) * 2;
        eprintln!("Agent disconnected; retrying with bounded backoff");
        tokio::select! { _ = shutdown.changed() => return Ok(()), _ = tokio::time::sleep(Duration::from_millis(seconds * 1000 + jitter)) => () }
    }
}

async fn session(
    store: &Store,
    url: &str,
    node_id: &str,
    credential: &str,
    connector: Connector,
    mut shutdown: watch::Receiver<bool>,
) -> Result<(), AgentError> {
    let mut socket = connect(url, "Bearer", credential, connector).await?;
    send(&mut socket, node_id, Message::Hello(AgentInfo::current())).await?;
    let welcome =
        tokio::time::timeout(Duration::from_secs(10), receive(&mut socket, Some(node_id)))
            .await
            .map_err(|_| AgentError::Connection)??;
    if !matches!(welcome.message, Message::Welcome) {
        return Err(AgentError::Protocol);
    }
    println!("Agent connected: {node_id}");
    let mut heartbeat = tokio::time::interval(Duration::from_secs(HEARTBEAT_SECONDS));
    heartbeat.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    let mut seen = Instant::now();
    type Work<'a> = std::pin::Pin<
        Box<
            dyn std::future::Future<Output = (String, Result<TaskOutcome, AgentError>)> + Send + 'a,
        >,
    >;
    let mut work: Option<Work<'_>> = None;
    let mut work_task: Option<TaskCommand> = None;
    let mut collector = crate::metrics::Collector::default();
    let mut sampling = tokio::time::interval(Duration::from_secs(
        panel_common::telemetry::METRICS_SECONDS,
    ));
    sampling.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    let mut traffic =
        tokio::time::interval(Duration::from_secs(panel_common::traffic::TRAFFIC_SECONDS));
    traffic.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    loop {
        tokio::select! {
            _ = traffic.tick() => {
                send(&mut socket, node_id, Message::Traffic(crate::traffic::sample().await)).await?;
            }
            completed = async { match work.as_mut() { Some(work) => work.await, None => std::future::pending().await } } => {
                work = None; work_task = None;
                let (task_id, result) = completed;
                let result = result?;
                send(&mut socket, node_id, Message::TaskAck { task_id: task_id.clone() }).await?;
                send(&mut socket, node_id, Message::TaskResult { task_id, result }).await?;
            }
            _ = shutdown.changed() => { let _ = socket.close(None).await; return Ok(()); },
            _ = sampling.tick() => {
                let (next, snapshot) = tokio::task::spawn_blocking(move || { let snapshot = collector.sample(); (collector, snapshot) }).await.map_err(|_| AgentError::Protocol)?;
                collector = next;
                send(&mut socket, node_id, Message::Metrics(snapshot)).await?;
            }
            _ = heartbeat.tick() => {
                if seen.elapsed() > Duration::from_secs(OFFLINE_SECONDS) { return Err(AgentError::Connection); }
                send(&mut socket, node_id, Message::Heartbeat).await?;
            }
            incoming = socket.next() => {
                let Some(Ok(incoming)) = incoming else { return Err(AgentError::Connection); };
                seen = Instant::now();
                match incoming {
                    WsMessage::Text(text) => {
                        let frame = Envelope::parse(&text, Some(node_id)).map_err(|_| AgentError::Protocol)?;
                        match frame.message {
                            Message::Task(task) => {
                                if work_task.as_ref().is_some_and(|current| current != &task) { return Err(AgentError::Protocol); }
                                if work.is_none() {
                                    work_task = Some(task.clone());
                                    work = Some(Box::pin(async move {
                                        let result = store.execute_async(&task, || async { match &task.operation {
                                            TaskOperation::AgentStatus => TaskOutcome::Succeeded(AgentInfo::current()),
                                            TaskOperation::ServiceStatus(query) => crate::systemd::status(query.service.clone()).await,
                                            TaskOperation::Xray(operation) => crate::xray::execute(&task, operation.clone()).await,
                                            TaskOperation::ApplyConfig(reference) => crate::xray::apply_config(&task, reference).await,
                                        } }).await;
                                        (task.id, result)
                                    }));
                                }
                            }
                            Message::ResultAck { task_id } if uuid::Uuid::parse_str(&task_id).is_ok() => (),
                            _ => return Err(AgentError::Protocol),
                        }
                    }
                    WsMessage::Ping(data) => {
                        tokio::time::timeout(Duration::from_secs(5), socket.send(WsMessage::Pong(data))).await.map_err(|_| AgentError::Connection)?.map_err(|_| AgentError::Connection)?;
                    }
                    WsMessage::Pong(_) => (),
                    WsMessage::Close(_) => return Err(AgentError::Connection),
                    WsMessage::Binary(_) | WsMessage::Frame(_) => return Err(AgentError::Protocol),
                }
            }
        }
    }
}

pub async fn retire(
    store: &Store,
    allow_local_ws: bool,
    connector: Connector,
) -> Result<(), AgentError> {
    crate::security::require_unprivileged()?;
    let identity = store.identity().await?.ok_or(AgentError::Configuration)?;
    let url = config::server_url(&identity.server_url, allow_local_ws)?;
    let mut socket = match connect(&url, "Bearer", &identity.credential, connector).await {
        Ok(socket) => socket,
        Err(AgentError::Authentication) => return Ok(()),
        Err(error) => return Err(error),
    };
    tokio::time::timeout(Duration::from_secs(10), async {
        send(
            &mut socket,
            &identity.node_id,
            Message::Hello(AgentInfo::current()),
        )
        .await?;
        if !matches!(
            receive(&mut socket, Some(&identity.node_id)).await?.message,
            Message::Welcome
        ) {
            return Err(AgentError::Protocol);
        }
        send(&mut socket, &identity.node_id, Message::Retire).await?;
        loop {
            match receive(&mut socket, Some(&identity.node_id)).await?.message {
                Message::Retired => return Ok(()),
                Message::Task(_) => (),
                _ => return Err(AgentError::Protocol),
            }
        }
    })
    .await
    .map_err(|_| AgentError::Connection)?
}
