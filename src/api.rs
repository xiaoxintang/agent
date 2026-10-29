//! HTTP 接口与鉴权入口；接口清单同时用于注册路由和生成 OpenAPI。
//! 这里只转换传输参数，配置持久化及恢复逻辑统一交给 App。

use crate::{
    app::App,
    error::{Error, ErrorBody, Result},
    grpc,
    mutation::Change,
    process::SystemStatus,
    store::{Operation, Snapshot},
    versions::VersionInfo,
};
use axum::{
    Json, Router,
    body::Bytes,
    extract::{
        DefaultBodyLimit, MatchedPath, Path, Query, Request, State,
        rejection::{BytesRejection, PathRejection, QueryRejection},
    },
    http::{Method, StatusCode, header},
    middleware::{self, Next},
    response::{
        IntoResponse, Response, Sse,
        sse::{Event, KeepAlive},
    },
    routing::{MethodFilter, on},
};
use futures_util::StreamExt;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{collections::HashMap, convert::Infallible, sync::Arc};
use subtle::ConstantTimeEq;
use utoipa::ToSchema;
use utoipa_swagger_ui::SwaggerUi;

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Install,
    Versions,
    DeleteVersion,
    Status,
    Capabilities,
    Start,
    Stop,
    Restart,
    GetConfig,
    PutConfig,
    AddInbound,
    RemoveInbound,
    PatchInbound,
    AddOutbound,
    RemoveOutbound,
    PatchOutbound,
    AddUser,
    RemoveUser,
    AddRules,
    RemoveRule,
    Override,
    Events,
    Rpc,
}

#[derive(Clone, Copy)]
pub struct Endpoint {
    pub verb: &'static str,
    pub path: &'static str,
    pub summary: &'static str,
    pub kind: Kind,
    pub rpc: Option<&'static str>,
    // 显式填写业务 schema 名称；未填写的原生 RPC 类型由描述符补全。
    pub request: Option<&'static str>,
    pub response: Option<&'static str>,
    // 包括清零统计等运行时副作用，不等同于“需要持久化配置”。
    pub mutating: bool,
}

macro_rules! endpoint {
    ($verb:literal,$path:literal,$summary:literal,$kind:ident,$rpc:expr,$request:expr,$response:expr,$mutating:expr) => {
        Endpoint {
            verb: $verb,
            path: concat!("/api/v1/xray", $path),
            summary: $summary,
            kind: Kind::$kind,
            rpc: $rpc,
            request: $request,
            response: $response,
            mutating: $mutating,
        }
    };
}

// 路径、文档和管理 RPC 的关联只有这一处定义，新增接口必须同时经过覆盖校验。
pub const ENDPOINTS: &[Endpoint] = &[
    endpoint!(
        "post",
        "/install",
        "Install and activate an official version",
        Install,
        None,
        Some("InstallRequest"),
        Some("InstallResult"),
        true
    ),
    endpoint!(
        "get",
        "/versions",
        "List locally installed versions",
        Versions,
        None,
        None,
        Some("VersionList"),
        false
    ),
    endpoint!(
        "delete",
        "/versions/{version}",
        "Delete an inactive local version",
        DeleteVersion,
        None,
        None,
        Some("DeleteResult"),
        true
    ),
    endpoint!(
        "get",
        "/status",
        "Inspect process and configuration consistency",
        Status,
        None,
        None,
        Some("AgentStatus"),
        false
    ),
    endpoint!(
        "get",
        "/capabilities",
        "List management RPCs and upstream limitations",
        Capabilities,
        None,
        None,
        Some("Capabilities"),
        false
    ),
    endpoint!(
        "post",
        "/start",
        "Start the active Xray version",
        Start,
        None,
        None,
        Some("ControlResult"),
        true
    ),
    endpoint!(
        "post",
        "/stop",
        "Stop Xray",
        Stop,
        None,
        None,
        Some("ControlResult"),
        true
    ),
    endpoint!(
        "post",
        "/restart",
        "Restart using committed configuration and reconcile",
        Restart,
        None,
        None,
        Some("ControlResult"),
        true
    ),
    endpoint!(
        "get",
        "/config",
        "Read the authoritative configuration snapshot",
        GetConfig,
        None,
        None,
        Some("Snapshot"),
        false
    ),
    endpoint!(
        "put",
        "/config",
        "Replace the complete configuration and restart",
        PutConfig,
        None,
        Some("XrayConfig"),
        Some("ControlResult"),
        true
    ),
    endpoint!(
        "get",
        "/inbounds",
        "List live inbounds",
        Rpc,
        Some("HandlerService/ListInbounds"),
        None,
        None,
        false
    ),
    endpoint!(
        "post",
        "/inbounds",
        "Add an inbound and persist without restart",
        AddInbound,
        Some("HandlerService/AddInbound"),
        Some("InboundConfig"),
        None,
        true
    ),
    endpoint!(
        "delete",
        "/inbounds/{tag}",
        "Remove an inbound and persist",
        RemoveInbound,
        Some("HandlerService/RemoveInbound"),
        None,
        None,
        true
    ),
    endpoint!(
        "patch",
        "/inbounds/{tag}",
        "Apply a supported inbound user operation",
        PatchInbound,
        Some("HandlerService/AlterInbound"),
        Some("InboundOperation"),
        None,
        true
    ),
    endpoint!(
        "get",
        "/inbounds/{tag}/users",
        "List inbound users or query an email",
        Rpc,
        Some("HandlerService/GetInboundUsers"),
        None,
        None,
        false
    ),
    endpoint!(
        "get",
        "/inbounds/{tag}/users/count",
        "Count live inbound users",
        Rpc,
        Some("HandlerService/GetInboundUsersCount"),
        None,
        None,
        false
    ),
    endpoint!(
        "post",
        "/inbounds/{tag}/users",
        "Add a protocol-native user and persist",
        AddUser,
        Some("HandlerService/AlterInbound"),
        Some("NativeUser"),
        None,
        true
    ),
    endpoint!(
        "delete",
        "/inbounds/{tag}/users/{email}",
        "Remove a user and persist",
        RemoveUser,
        Some("HandlerService/AlterInbound"),
        None,
        None,
        true
    ),
    endpoint!(
        "get",
        "/outbounds",
        "List live outbounds",
        Rpc,
        Some("HandlerService/ListOutbounds"),
        None,
        None,
        false
    ),
    endpoint!(
        "post",
        "/outbounds",
        "Add an outbound and persist without restart",
        AddOutbound,
        Some("HandlerService/AddOutbound"),
        Some("OutboundConfig"),
        None,
        true
    ),
    endpoint!(
        "delete",
        "/outbounds/{tag}",
        "Remove an outbound and persist",
        RemoveOutbound,
        Some("HandlerService/RemoveOutbound"),
        None,
        None,
        true
    ),
    endpoint!(
        "patch",
        "/outbounds/{tag}",
        "AlterOutbound: no concrete operation in the pinned upstream",
        PatchOutbound,
        Some("HandlerService/AlterOutbound"),
        Some("OutboundOperation"),
        None,
        true
    ),
    endpoint!(
        "post",
        "/stats/read",
        "Read a counter, optionally resetting it",
        Rpc,
        Some("StatsService/GetStats"),
        None,
        None,
        true
    ),
    endpoint!(
        "post",
        "/stats/query",
        "Query counters, optionally resetting them",
        Rpc,
        Some("StatsService/QueryStats"),
        None,
        None,
        true
    ),
    endpoint!(
        "get",
        "/stats/online",
        "Get online count for a counter name",
        Rpc,
        Some("StatsService/GetStatsOnline"),
        None,
        None,
        false
    ),
    endpoint!(
        "get",
        "/stats/online-ips",
        "Get online IPs for a counter name",
        Rpc,
        Some("StatsService/GetStatsOnlineIpList"),
        None,
        None,
        false
    ),
    endpoint!(
        "get",
        "/stats/online-users",
        "Get all online users",
        Rpc,
        Some("StatsService/GetAllOnlineUsers"),
        None,
        None,
        false
    ),
    endpoint!(
        "post",
        "/stats/users/query",
        "Query online users and optional traffic counters",
        Rpc,
        Some("StatsService/GetUsersStats"),
        None,
        None,
        true
    ),
    endpoint!(
        "get",
        "/stats/system",
        "Get Xray runtime statistics",
        Rpc,
        Some("StatsService/GetSysStats"),
        None,
        None,
        false
    ),
    endpoint!(
        "get",
        "/routing/rules",
        "List live routing rule tags",
        Rpc,
        Some("RoutingService/ListRule"),
        None,
        None,
        false
    ),
    endpoint!(
        "post",
        "/routing/rules",
        "Append or replace rules and balancers without restart",
        AddRules,
        Some("RoutingService/AddRule"),
        Some("RulesRequest"),
        None,
        true
    ),
    endpoint!(
        "delete",
        "/routing/rules/{ruleTag}",
        "Remove a routing rule and persist",
        RemoveRule,
        Some("RoutingService/RemoveRule"),
        None,
        None,
        true
    ),
    endpoint!(
        "post",
        "/routing/test",
        "Test routing with a protobuf JSON context",
        Rpc,
        Some("RoutingService/TestRoute"),
        None,
        None,
        true
    ),
    endpoint!(
        "get",
        "/routing/balancers/{tag}",
        "Inspect a balancer",
        Rpc,
        Some("RoutingService/GetBalancerInfo"),
        None,
        None,
        false
    ),
    endpoint!(
        "put",
        "/routing/balancers/{tag}/target",
        "Set and persist a balancer override",
        Override,
        Some("RoutingService/OverrideBalancerTarget"),
        Some("OverrideRequest"),
        None,
        true
    ),
    endpoint!(
        "get",
        "/routing/events",
        "Subscribe to routing events (upstream limitation applies)",
        Events,
        Some("RoutingService/SubscribeRoutingStats"),
        None,
        None,
        false
    ),
    endpoint!(
        "post",
        "/logger/restart",
        "Reopen Xray logs",
        Rpc,
        Some("LoggerService/RestartLogger"),
        None,
        None,
        true
    ),
    endpoint!(
        "get",
        "/observatory/outbounds",
        "Query configured outbound observations",
        Rpc,
        Some("ObservatoryService/GetOutboundStatus"),
        None,
        None,
        false
    ),
];

#[derive(Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct InstallRequest {
    /// Official release tag, default v26.10.21.
    #[schema(example = "v26.10.21")]
    pub version: Option<String>,
}

#[derive(Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct OverrideRequest {
    /// Empty string clears the override.
    pub target: String,
}

#[derive(Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct RulesRequest {
    #[schema(value_type = Object)]
    pub routing: Value,
    #[serde(default, rename = "shouldAppend")]
    pub append: bool,
}

#[derive(Deserialize, ToSchema)]
#[serde(tag = "operation", rename_all = "snake_case", deny_unknown_fields)]
pub enum InboundOperation {
    AddUser {
        #[schema(value_type = Object)]
        user: Value,
    },
    RemoveUser {
        email: String,
    },
}

#[derive(Serialize, ToSchema)]
pub struct AgentStatus {
    pub installed: bool,
    pub active_version: Option<String>,
    pub system: Option<SystemStatus>,
    pub system_error: Option<String>,
    pub grpc_available: bool,
    pub revision: i64,
    pub sync_state: String,
    pub operation_in_progress: bool,
    pub pending_operations: Vec<Operation>,
}

pub fn router(app: Arc<App>) -> Router {
    let mut management = Router::<Arc<App>>::new();
    for endpoint in ENDPOINTS {
        let filter = match endpoint.verb {
            "get" => MethodFilter::GET,
            "post" => MethodFilter::POST,
            "put" => MethodFilter::PUT,
            "patch" => MethodFilter::PATCH,
            "delete" => MethodFilter::DELETE,
            _ => unreachable!(),
        };
        management = management.route(endpoint.path, on(filter, dispatch));
    }
    management
        .layer(DefaultBodyLimit::max(4 * 1024 * 1024))
        .layer(middleware::from_fn_with_state(app.clone(), authenticate))
        .with_state(app)
        // 文档只包含接口定义，不含主机配置和凭据，独立于管理接口的鉴权中间件。
        .merge(SwaggerUi::new("/docs").url("/openapi.json", crate::openapi::document()))
}

async fn authenticate(State(app): State<Arc<App>>, request: Request, next: Next) -> Response {
    let supplied = request
        .headers()
        .get(header::AUTHORIZATION)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.strip_prefix("Bearer "));
    // 先散列为固定长度，再做常量时间比较，避免直接比较可变长 Token。
    let authorized = supplied.is_some_and(|token| {
        bool::from(
            Sha256::digest(token.as_bytes()).ct_eq(&Sha256::digest(app.settings.token.as_bytes())),
        )
    });
    if !authorized {
        return (
            StatusCode::UNAUTHORIZED,
            [(header::WWW_AUTHENTICATE, "Bearer")],
            Json(ErrorBody {
                error: "valid Bearer token required".into(),
                grpc_code: None,
            }),
        )
            .into_response();
    }
    next.run(request).await
}

async fn dispatch(
    State(app): State<Arc<App>>,
    matched: MatchedPath,
    parameters: std::result::Result<Path<HashMap<String, String>>, PathRejection>,
    method: Method,
    query: std::result::Result<Query<HashMap<String, String>>, QueryRejection>,
    body: std::result::Result<Bytes, BytesRejection>,
) -> Result<Response> {
    // 把 Axum 提取器错误也转换为统一 JSON，保持与 OpenAPI 的错误响应一致。
    let Path(parameters) = parameters.map_err(|error| Error::BadRequest(error.to_string()))?;
    let Query(query) = query.map_err(|error| Error::BadRequest(error.to_string()))?;
    let body = body.map_err(|error| {
        if error.status() == StatusCode::PAYLOAD_TOO_LARGE {
            Error::PayloadTooLarge
        } else {
            Error::BadRequest(error.to_string())
        }
    })?;
    // 使用匹配到的路由模板，而非包含 tag/email 实际值的 URL 查找接口定义。
    let endpoint = *ENDPOINTS
        .iter()
        .find(|endpoint| {
            endpoint.path == matched.as_str() && endpoint.verb.eq_ignore_ascii_case(method.as_str())
        })
        .ok_or_else(|| Error::NotFound("endpoint not found".into()))?;
    let body = if body.is_empty() {
        json!({})
    } else {
        serde_json::from_slice(&body)
            .map_err(|error| Error::BadRequest(format!("invalid JSON: {error}")))?
    };
    if endpoint.mutating {
        // An HTTP disconnect must not cancel a mutation between gRPC dispatch and durable commit.
        let tasks = app.tasks.clone();
        tasks
            .spawn(execute(app, endpoint, parameters, query, body))
            .await
            .map_err(|error| Error::Internal(format!("management task failed: {error}")))?
    } else {
        execute(app, endpoint, parameters, query, body).await
    }
}

fn required(parameters: &HashMap<String, String>, name: &str) -> Result<String> {
    parameters
        .get(name)
        .filter(|value| !value.is_empty())
        .cloned()
        .ok_or_else(|| Error::BadRequest(format!("missing {name}")))
}

fn boolean(query: &HashMap<String, String>, name: &str) -> Result<bool> {
    match query.get(name).map(String::as_str) {
        None | Some("false") => Ok(false),
        Some("true") => Ok(true),
        _ => Err(Error::BadRequest(format!("{name} must be true or false"))),
    }
}

fn deserialize<T: serde::de::DeserializeOwned>(value: Value) -> Result<T> {
    serde_json::from_value(value).map_err(|error| Error::BadRequest(error.to_string()))
}

async fn execute(
    app: Arc<App>,
    endpoint: Endpoint,
    parameters: HashMap<String, String>,
    query: HashMap<String, String>,
    body: Value,
) -> Result<Response> {
    let value = match endpoint.kind {
        Kind::Install => app.install(deserialize::<InstallRequest>(body)?.version).await?,
        Kind::Versions => serde_json::to_value(app.list_versions()?)?,
        Kind::DeleteVersion => app.delete_version(&required(&parameters,"version")?).await?,
        Kind::Status => {
            let active_version = app.active_version()?;
            let (system,system_error) = match app.system.status().await {Ok(status)=>(Some(status),None),Err(error)=>(None,Some(error.to_string()))};
            let grpc_available = if active_version.is_some() {app.rpc.unary("StatsService/GetSysStats",json!({})).await.is_ok()} else {false};
            serde_json::to_value(AgentStatus {installed:active_version.is_some(),active_version,system,system_error,grpc_available,revision:app.store.committed()?.revision,sync_state:app.store.sync_state()?,operation_in_progress:app.operations.try_lock().is_err(),pending_operations:app.store.pending()?})?
        }
        Kind::Capabilities => json!({
            "baseline":crate::config::BASELINE,"active_version":app.active_version()?,"rpcs":grpc::methods(),
            "configuration_format":"native Xray JSON","persistence":"SQLite snapshots + generated config.json",
            "limitations":["Other Xray versions may not implement every baseline method.","v26.10.21 SubscribeRoutingStats is not connected to a statistics channel.","v26.10.21 has no concrete AlterOutbound operation.","Observatory requires configuration and ObservatoryService in api.services.","Balancer overrides are restored after startup; statistics resets are not persisted."]
        }),
        Kind::Start=>app.control("start").await?,
        Kind::Stop=>app.control("stop").await?,
        Kind::Restart=>app.control("restart").await?,
        Kind::GetConfig=>serde_json::to_value(app.store.committed()?)?,
        Kind::PutConfig=>app.replace_config(body).await?,
        Kind::AddInbound=>app.mutation(Change::AddInbound(body)).await?,
        Kind::RemoveInbound=>app.mutation(Change::RemoveInbound(required(&parameters,"tag")?)).await?,
        Kind::AddOutbound=>app.mutation(Change::AddOutbound(body)).await?,
        Kind::RemoveOutbound=>app.mutation(Change::RemoveOutbound(required(&parameters,"tag")?)).await?,
        Kind::AddUser=>app.mutation(Change::AddUser {tag:required(&parameters,"tag")?,user:body}).await?,
        Kind::RemoveUser=>app.mutation(Change::RemoveUser {tag:required(&parameters,"tag")?,email:required(&parameters,"email")?}).await?,
        Kind::PatchInbound=>{
            // PATCH 只映射上游支持的动态操作，不把任意入站字段变更伪装成热更新。
            let tag = required(&parameters,"tag")?;
            let change = match deserialize::<InboundOperation>(body)? {InboundOperation::AddUser {user}=>Change::AddUser {tag,user},InboundOperation::RemoveUser {email}=>Change::RemoveUser {tag,email}};
            app.mutation(change).await?
        }
        Kind::PatchOutbound=>return Err(Error::Unsupported("v26.10.21 defines AlterOutbound but implements no concrete OutboundOperation; arbitrary outbound patching is not supported".into())),
        Kind::AddRules=>{
            let request: RulesRequest = deserialize(body)?;
            app.mutation(Change::AddRules {routing:request.routing,append:request.append}).await?
        }
        Kind::RemoveRule=>app.mutation(Change::RemoveRule(required(&parameters,"ruleTag")?)).await?,
        Kind::Override=>{
            let request: OverrideRequest = deserialize(body)?;
            app.mutation(Change::Override {tag:required(&parameters,"tag")?,target:request.target}).await?
        }
        Kind::Events=>{
            let selectors: Vec<_> = query.get("fieldSelectors").map(|value| value.split(',').filter(|value| !value.is_empty()).collect()).unwrap_or_default();
            let mut source = app.rpc.subscribe(json!({"FieldSelectors":selectors})).await?;
            // 已发出 SSE 响应后不能改 HTTP 状态，后续上游错误通过 error 事件通知并结束。
            // HTTP 连接断开时流会被丢弃，从而结束对应的上游订阅。
            let stream = async_stream::stream! {
                while let Some(item) = source.next().await {
                    match item {
                        Ok(value)=>yield Ok::<Event,Infallible>(Event::default().event("route").json_data(value).expect("JSON event")),
                        Err(error)=>{yield Ok(Event::default().event("error").json_data(json!({"error":error.to_string()})).expect("JSON error"));break;}
                    }
                }
            };
            return Ok(Sse::new(stream).keep_alive(KeepAlive::default()).into_response());
        }
        Kind::Rpc=>{
            let rpc = endpoint.rpc.unwrap();
            let input = match rpc {
                "HandlerService/ListInbounds"=>json!({"isOnlyTags":boolean(&query,"isOnlyTags")?}),
                "HandlerService/GetInboundUsers"|"HandlerService/GetInboundUsersCount"=>json!({"tag":required(&parameters,"tag")?,"email":query.get("email").cloned().unwrap_or_default()}),
                "StatsService/GetStatsOnline"|"StatsService/GetStatsOnlineIpList"=>json!({"name":required(&query,"name")?}),
                "RoutingService/GetBalancerInfo"=>json!({"tag":required(&parameters,"tag")?}),
                _=>body,
            };
            // Counter resets and logger/routing actions are serialized, but aren't persisted config changes.
            let _guard = if endpoint.mutating {Some(app.lock()?)} else {None};
            app.rpc.unary(rpc,input).await?
        }
    };
    Ok(Json(value).into_response())
}

#[derive(utoipa::OpenApi)]
#[openapi(components(schemas(
    ErrorBody,
    InstallRequest,
    OverrideRequest,
    RulesRequest,
    InboundOperation,
    AgentStatus,
    Snapshot,
    Operation,
    SystemStatus,
    crate::versions::Manifest,
    VersionInfo
)))]
pub struct ApiComponents;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        config::{Paths, Settings},
        grpc::{EventStream, XrayRpc},
        process::{ConfigCompiler, ServiceControl},
        store::Store,
    };
    use async_trait::async_trait;
    use tower::ServiceExt;

    struct Fake;
    #[async_trait]
    impl XrayRpc for Fake {
        async fn unary(&self, _: &str, _: Value) -> Result<Value> {
            Ok(json!({}))
        }
        async fn subscribe(&self, _: Value) -> Result<EventStream> {
            Ok(Box::pin(futures_util::stream::empty()))
        }
    }
    #[async_trait]
    impl ConfigCompiler for Fake {
        async fn compile(
            &self,
            _: &std::path::Path,
            _: &std::path::Path,
            _: &Value,
        ) -> Result<Value> {
            Ok(json!({}))
        }
    }
    #[async_trait]
    impl ServiceControl for Fake {
        async fn status(&self) -> Result<SystemStatus> {
            Ok(SystemStatus::default())
        }
        async fn action(&self, _: &str) -> Result<()> {
            Ok(())
        }
        async fn daemon_reload(&self) -> Result<()> {
            Ok(())
        }
        async fn enable(&self) -> Result<()> {
            Ok(())
        }
        async fn disable(&self) -> Result<()> {
            Ok(())
        }
    }

    #[tokio::test]
    async fn management_requires_token_and_documentation_matches_routes() {
        let temporary = tempfile::tempdir().unwrap();
        let paths = Paths::temporary(temporary.path());
        paths.prepare().unwrap();
        let settings = Settings {
            listen: "127.0.0.1:0".parse().unwrap(),
            grpc_listen: "127.0.0.1:10085".parse().unwrap(),
            token: "a".repeat(43),
        };
        let app = Arc::new(App {
            store: Store::open(&paths.database, &crate::config::initial_xray(&settings)).unwrap(),
            settings,
            paths,
            rpc: Arc::new(Fake),
            compiler: Arc::new(Fake),
            system: Arc::new(Fake),
            operations: tokio::sync::Mutex::new(()),
            tasks: tokio_util::task::TaskTracker::new(),
        });
        let router = router(app);
        let unauthorized = router
            .clone()
            .oneshot(
                axum::http::Request::builder()
                    .uri("/api/v1/xray/config")
                    .body(axum::body::Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(unauthorized.status(), StatusCode::UNAUTHORIZED);
        let request = axum::http::Request::builder()
            .uri("/api/v1/xray/config")
            .header("Authorization", format!("Bearer {}", "a".repeat(43)))
            .body(axum::body::Body::empty())
            .unwrap();
        assert_eq!(
            router.clone().oneshot(request).await.unwrap().status(),
            StatusCode::OK
        );
        assert_eq!(
            router
                .oneshot(
                    axum::http::Request::builder()
                        .uri("/openapi.json")
                        .body(axum::body::Body::empty())
                        .unwrap()
                )
                .await
                .unwrap()
                .status(),
            StatusCode::OK
        );
        let document = serde_json::to_value(crate::openapi::document()).unwrap();
        for endpoint in ENDPOINTS {
            assert!(document["paths"][endpoint.path][endpoint.verb].is_object());
        }
        let covered: std::collections::HashSet<_> = ENDPOINTS
            .iter()
            .filter_map(|endpoint| endpoint.rpc.map(str::to_owned))
            .collect();
        assert_eq!(covered, grpc::methods().into_iter().collect());
    }

    #[tokio::test]
    async fn client_disconnect_does_not_cancel_durable_mutation() {
        let temporary = tempfile::tempdir().unwrap();
        let (app, runtime) = crate::testsupport::fixture(
            temporary.path(),
            crate::testsupport::Outcome::DelayedSuccess,
        );
        let request = axum::http::Request::builder()
            .method("POST")
            .uri("/api/v1/xray/inbounds")
            .header("Authorization", format!("Bearer {}", app.settings.token))
            .header("Content-Type", "application/json")
            .body(axum::body::Body::from(
                json!({"tag":"new","protocol":"dokodemo-door","port":10086,"settings":{}})
                    .to_string(),
            ))
            .unwrap();
        let client = tokio::spawn(router(app.clone()).oneshot(request));
        tokio::time::timeout(std::time::Duration::from_secs(2), async {
            while runtime.calls.load(std::sync::atomic::Ordering::SeqCst) == 0 {
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();
        client.abort();
        app.tasks.close();
        app.tasks.wait().await;
        assert_eq!(
            app.store.committed().unwrap().config["inbounds"][0]["tag"],
            "new"
        );
        assert!(app.store.ensure_consistent().is_ok());
        assert!(runtime.actions.lock().unwrap().is_empty());
    }
}
