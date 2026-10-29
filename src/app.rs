//! 编排配置变更、进程控制与中断恢复，协调 SQLite、启动文件和 Xray 运行时。
//! 普通动态变更不重启；完整配置替换、版本切换和显式重启使用各自的恢复流程。

use crate::{
    config::{Paths, Settings, atomic_write, initial_xray},
    error::{Error, Result},
    grpc::{Grpc, XrayRpc},
    mutation::Change,
    process::{Compiler, ConfigCompiler, ServiceControl, Systemd},
    store::{Operation, Store},
};
use serde_json::{Value, json};
use std::{collections::HashSet, sync::Arc, time::Duration};
use tokio::sync::{Mutex, MutexGuard};

pub struct App {
    pub settings: Settings,
    pub paths: Paths,
    pub store: Store,
    pub rpc: Arc<dyn XrayRpc>,
    pub compiler: Arc<dyn ConfigCompiler>,
    pub system: Arc<dyn ServiceControl>,
    // 需要改变受管状态的操作共用此锁，防止配置提交与版本切换交叉执行。
    pub operations: Mutex<()>,
    // 变更任务独立于 HTTP 连接存活，退出服务时统一等待它们完成。
    pub tasks: tokio_util::task::TaskTracker,
}

#[cfg(test)]
mod tests {
    // 使用模拟运行时验证提交顺序及异常分支，不启动真实 Xray 或 systemd。
    use super::*;
    use crate::testsupport::{Outcome, fixture};
    use std::sync::atomic::Ordering;

    fn new_inbound() -> Change {
        Change::AddInbound(
            json!({"tag":"new","protocol":"dokodemo-door","listen":"127.0.0.1","port":10086,"settings":{"address":"127.0.0.1","port":80}}),
        )
    }

    #[tokio::test]
    async fn successful_mutation_persists_without_restarting() {
        let temporary = tempfile::tempdir().unwrap();
        let (app, runtime) = fixture(temporary.path(), Outcome::Success);
        let result = app.mutation(new_inbound()).await.unwrap();
        let persisted: Value =
            serde_json::from_slice(&std::fs::read(&app.paths.config).unwrap()).unwrap();
        assert_eq!(persisted, app.store.committed().unwrap().config);
        assert_eq!(persisted["inbounds"][0]["tag"], "new");
        assert_eq!(result["revision"], app.store.committed().unwrap().revision);
        assert_eq!(runtime.calls.load(Ordering::SeqCst), 1);
        assert!(runtime.actions.lock().unwrap().is_empty());
        assert!(app.store.ensure_consistent().is_ok());
    }

    #[tokio::test]
    async fn timeout_blocks_writes_until_an_explicit_restart() {
        let temporary = tempfile::tempdir().unwrap();
        let (app, runtime) = fixture(temporary.path(), Outcome::Timeout);
        let previous = app.store.committed().unwrap().revision;
        assert!(matches!(
            app.mutation(new_inbound()).await,
            Err(Error::Grpc {
                code: tonic::Code::DeadlineExceeded,
                ..
            })
        ));
        assert_eq!(app.store.committed().unwrap().revision, previous);
        assert!(matches!(
            app.mutation(new_inbound()).await,
            Err(Error::Conflict(_))
        ));
        assert_eq!(runtime.calls.load(Ordering::SeqCst), 1);
        assert!(runtime.actions.lock().unwrap().is_empty());
        app.control("restart").await.unwrap();
        assert_eq!(runtime.actions.lock().unwrap().as_slice(), ["restart"]);
        assert!(app.store.ensure_consistent().is_ok());
    }

    #[tokio::test]
    async fn preflight_and_definite_rejection_do_not_commit() {
        for outcome in [Outcome::CompilationFailure, Outcome::Rejected] {
            let temporary = tempfile::tempdir().unwrap();
            let (app, runtime) = fixture(temporary.path(), outcome);
            let before = std::fs::read(&app.paths.config).unwrap();
            assert!(app.mutation(new_inbound()).await.is_err());
            assert_eq!(before, std::fs::read(&app.paths.config).unwrap());
            assert!(app.store.ensure_consistent().is_ok());
            assert!(runtime.actions.lock().unwrap().is_empty());
            assert_eq!(
                runtime.calls.load(Ordering::SeqCst),
                if outcome == Outcome::CompilationFailure {
                    0
                } else {
                    1
                }
            );
        }
    }

    #[tokio::test]
    async fn publication_failure_is_not_reported_as_success_or_restarted() {
        let temporary = tempfile::tempdir().unwrap();
        let (app, runtime) = fixture(temporary.path(), Outcome::PublicationFailure);
        assert!(matches!(
            app.mutation(new_inbound()).await,
            Err(Error::Uncertain(_))
        ));
        assert_eq!(
            app.store.committed().unwrap().config["inbounds"][0]["tag"],
            "new"
        );
        assert_eq!(app.store.sync_state().unwrap(), "needs_confirmation");
        assert!(runtime.actions.lock().unwrap().is_empty());
    }

    #[tokio::test]
    async fn database_commit_failure_after_grpc_keeps_previous_snapshot() {
        let temporary = tempfile::tempdir().unwrap();
        let (app, runtime) = fixture(temporary.path(), Outcome::Success);
        let previous = app.store.committed().unwrap().revision;
        let connection = rusqlite::Connection::open(&app.paths.database).unwrap();
        // 仅阻断权威修订指针的提交，保留操作日志更新能力，模拟 RPC 成功后的落盘故障。
        connection.execute_batch("CREATE TRIGGER injected_commit_failure BEFORE UPDATE OF revision ON state BEGIN SELECT RAISE(FAIL,'injected disk failure'); END;").unwrap();
        assert!(matches!(
            app.mutation(new_inbound()).await,
            Err(Error::Uncertain(_))
        ));
        assert_eq!(runtime.calls.load(Ordering::SeqCst), 1);
        assert_eq!(app.store.committed().unwrap().revision, previous);
        assert!(app.store.ensure_consistent().is_err());
        assert!(runtime.actions.lock().unwrap().is_empty());
    }

    #[tokio::test]
    async fn full_configuration_restart_failure_restores_committed_revision() {
        let temporary = tempfile::tempdir().unwrap();
        let (app, runtime) = fixture(temporary.path(), Outcome::RestartFailureOnce);
        let before = app.store.committed().unwrap();
        let mut candidate = before.config.clone();
        candidate["log"]["loglevel"] = json!("info");
        assert!(app.replace_config(candidate).await.is_err());
        assert_eq!(app.store.committed().unwrap().revision, before.revision);
        assert_eq!(
            serde_json::from_slice::<Value>(&std::fs::read(&app.paths.config).unwrap()).unwrap(),
            before.config
        );
        assert_eq!(
            runtime.actions.lock().unwrap().as_slice(),
            ["restart", "restart"]
        );
        assert!(app.store.ensure_consistent().is_ok());
    }
}

impl App {
    pub fn open(settings: Settings, paths: Paths) -> Result<Arc<Self>> {
        paths.prepare()?;
        let store = Store::open(&paths.database, &initial_xray(&settings))?;
        let rpc = Arc::new(Grpc::new(settings.grpc_listen)?);
        Ok(Arc::new(Self {
            settings,
            compiler: Arc::new(Compiler {
                paths: paths.clone(),
            }),
            paths,
            store,
            rpc,
            system: Arc::new(Systemd),
            operations: Mutex::new(()),
            tasks: tokio_util::task::TaskTracker::new(),
        }))
    }

    pub fn lock(&self) -> Result<MutexGuard<'_, ()>> {
        // 立即拒绝冲突而非排队，避免旧请求在另一项变更结束后意外执行。
        self.operations
            .try_lock()
            .map_err(|_| Error::Conflict("another management operation is in progress".into()))
    }

    pub fn validate_management(&self, config: &Value) -> Result<()> {
        // 业务配置可以修改，但不能关闭 Agent 依赖的管理监听、统计和必需服务。
        if !config.is_object() {
            return Err(Error::Validation(
                "configuration must be a JSON object".into(),
            ));
        }
        if config.pointer("/api/listen").and_then(Value::as_str)
            != Some(self.settings.grpc_listen.to_string().as_str())
        {
            return Err(Error::Validation(
                "api.listen must match the agent's loopback gRPC address".into(),
            ));
        }
        if config
            .pointer("/api/tag")
            .and_then(Value::as_str)
            .is_none_or(str::is_empty)
        {
            return Err(Error::Validation("api.tag is required".into()));
        }
        let services = config
            .pointer("/api/services")
            .and_then(Value::as_array)
            .ok_or_else(|| Error::Validation("api.services is required".into()))?;
        for service in [
            "HandlerService",
            "StatsService",
            "RoutingService",
            "LoggerService",
        ] {
            if !services.iter().any(|entry| {
                entry
                    .as_str()
                    .is_some_and(|value| value.eq_ignore_ascii_case(service))
            }) {
                return Err(Error::Validation(format!(
                    "api.services must include {service}"
                )));
            }
        }
        if !config.get("stats").is_some_and(Value::is_object) {
            return Err(Error::Validation(
                "stats must be enabled for readiness checks".into(),
            ));
        }
        for key in ["inbounds", "outbounds"] {
            let entries = config
                .get(key)
                .and_then(Value::as_array)
                .ok_or_else(|| Error::Validation(format!("{key} must be an array")))?;
            let mut tags = HashSet::new();
            for entry in entries {
                let tag = entry
                    .get("tag")
                    .and_then(Value::as_str)
                    .filter(|tag| !tag.is_empty())
                    .ok_or_else(|| Error::Validation(format!("every {key} entry needs a tag")))?;
                if !tags.insert(tag) {
                    return Err(Error::Validation(format!("duplicate {key} tag: {tag}")));
                }
            }
        }
        Ok(())
    }

    pub fn materialize(&self) -> Result<()> {
        // config.json 始终由已提交快照生成，未确认的候选不能成为启动配置。
        atomic_write(
            &self.paths.config,
            &serde_json::to_vec_pretty(&self.store.committed()?.config)?,
        )
    }

    pub async fn mutation(&self, change: Change) -> Result<Value> {
        let _guard = self.lock()?;
        self.store.ensure_consistent()?;
        self.require_installed()?;
        // 先在快照副本上构造和校验完整结果，所有前置失败都发生在运行时调用之前。
        let snapshot = self.store.committed()?;
        let candidate = change.apply(&snapshot)?;
        self.validate_management(&candidate.config)?;
        let compiled = self
            .compiler
            .compile(&self.paths.binary(), &self.paths.current, &candidate.config)
            .await?;
        let request = change.request(&compiled)?;
        // Validate the exact request before crossing the durable dispatch boundary.
        crate::grpc::from_json(
            crate::grpc::method(change.method())?.input(),
            request.clone(),
        )?;
        // 提前写好启动文件临时副本，尽量在调用 gRPC 之前发现磁盘与权限问题。
        let mut staged = tempfile::NamedTempFile::new_in(self.paths.config.parent().unwrap())?;
        use std::io::Write;
        staged.write_all(&serde_json::to_vec_pretty(&candidate.config)?)?;
        staged.as_file().sync_all()?;
        let operation = self.store.begin(
            "mutation",
            Some((&candidate.config, &candidate.overrides)),
            json!({"method":change.method()}),
        )?;
        let before = match self.system.status().await {
            Ok(status) if status.active_state == "active" => status,
            Ok(_) => {
                self.store
                    .fail(&operation.id, "Xray is not active", false)?;
                return Err(Error::Unavailable("Xray is not active".into()));
            }
            Err(error) => {
                self.store.fail(&operation.id, &error.to_string(), false)?;
                return Err(error);
            }
        };
        self.store.details(&operation.id,&json!({
            "method":change.method(),"invocation_id":before.invocation_id,"pid":before.main_pid,
            "tag":request.get("tag").or_else(||request.pointer("/inbound/tag")).or_else(||request.pointer("/outbound/tag")),
            "rule_tag":request.get("ruleTag"),"balancer_tag":request.get("balancerTag")
        }))?;
        // 必须先落盘 dispatched 再发送请求；此后中断就不能再假定运行时未改变。
        self.store.phase(&operation.id, "dispatched")?;
        let response = match self.rpc.unary(change.method(), request).await {
            Ok(response) => response,
            Err(error) => {
                let definitely_not_applied = error.definitely_not_applied();
                self.store
                    .fail(&operation.id, &error.to_string(), !definitely_not_applied)?;
                return if definitely_not_applied {
                    Err(error)
                } else {
                    let context = format!(
                        "operation {} needs confirmation; inspect /status before explicitly restarting",
                        operation.id
                    );
                    match error {
                        Error::Grpc { code, message } => Err(Error::Grpc {
                            code,
                            message: format!("{message}; {context}"),
                        }),
                        _ => Err(Error::Uncertain(context)),
                    }
                };
            }
        };
        // 即使 RPC 返回成功，期间发生的 Xray 重启也可能使本次运行时结果失效。
        let same_generation = self.system.status().await.is_ok_and(|after| {
            after.active_state == "active"
                && after.invocation_id == before.invocation_id
                && after.main_pid == before.main_pid
        });
        if !same_generation {
            self.store
                .fail(&operation.id, "Xray restarted during the mutation", true)?;
            return Err(Error::Uncertain(format!(
                "Xray generation changed during operation {}; explicit restart required",
                operation.id
            )));
        }
        // 从这里开始运行时已改变；提交失败必须保留待确认状态，不能自动重发 RPC。
        if let Err(error) = self.store.commit_candidate(&operation) {
            let _ = self.store.fail(
                &operation.id,
                "runtime changed but database commit failed",
                true,
            );
            return Err(Error::Uncertain(format!(
                "runtime changed but persistence failed: {error}"
            )));
        }
        // 数据库提交后再替换启动文件；失败时新快照仍是权威配置，需恢复发布或显式重启。
        let publication = staged
            .persist(&self.paths.config)
            .map_err(|error| Error::Io(error.error))
            .and_then(|_| {
                std::fs::File::open(self.paths.config.parent().unwrap())?
                    .sync_all()
                    .map_err(Error::from)
            });
        if let Err(error) = publication {
            self.store.fail(
                &operation.id,
                "configuration committed but startup file publication failed",
                true,
            )?;
            return Err(Error::Uncertain(format!(
                "configuration committed but startup file publication failed: {error}"
            )));
        }
        // 发布文件也存在时间窗口，再次检查启动代次，防止将旧进程的成功误报为已同步。
        if !self.system.status().await.is_ok_and(|after| {
            after.active_state == "active"
                && after.invocation_id == before.invocation_id
                && after.main_pid == before.main_pid
        }) {
            self.store.fail(
                &operation.id,
                "Xray restarted while publishing configuration",
                true,
            )?;
            return Err(Error::Uncertain("configuration committed but Xray changed generation during publication; explicitly restart to restore it".into()));
        }
        if let Err(error) = self.store.complete(&operation.id) {
            return Err(Error::Uncertain(format!(
                "configuration published but completion could not be recorded: {error}"
            )));
        }
        Ok(json!({"revision":operation.candidate_revision,"response":response}))
    }

    pub fn require_installed(&self) -> Result<()> {
        if !self.paths.binary().is_file() {
            return Err(Error::Conflict("Xray has not been installed".into()));
        }
        Ok(())
    }

    pub async fn restore_overrides(&self) -> Result<()> {
        // override 没有普通 Xray JSON 字段，启动后单独通过 gRPC 恢复。
        let snapshot = self.store.committed()?;
        if let Some(overrides) = snapshot.overrides.as_object() {
            for (tag, target) in overrides {
                self.rpc
                    .unary(
                        "RoutingService/OverrideBalancerTarget",
                        json!({"balancerTag":tag,"target":target}),
                    )
                    .await?;
            }
        }
        Ok(())
    }

    pub async fn restore_after_start(&self) -> Result<()> {
        // 本方法由 ExecStartPost 调用；此时 unit 仍可能在 activating，直接等待 gRPC。
        tokio::time::timeout(Duration::from_secs(30), async {
            loop {
                if self
                    .rpc
                    .unary("StatsService/GetSysStats", json!({}))
                    .await
                    .is_ok()
                {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(300)).await;
            }
            self.restore_overrides().await
        })
        .await
        .map_err(|_| Error::Unavailable("Xray readiness/override restoration timed out".into()))?
    }

    pub async fn wait_ready(&self, stable: Duration) -> Result<()> {
        // 活跃状态与 RPC 可用必须同时满足；安装时再观察稳定期，识别反复崩溃重启。
        let generation = tokio::time::timeout(Duration::from_secs(30), async {
            loop {
                let status = self.system.status().await?;
                if status.active_state == "failed" {
                    return Err(Error::Unavailable("xray.service failed".into()));
                }
                if status.active_state == "active"
                    && self
                        .rpc
                        .unary("StatsService/GetSysStats", json!({}))
                        .await
                        .is_ok()
                {
                    return Ok(status.invocation_id);
                }
                tokio::time::sleep(Duration::from_millis(500)).await;
            }
        })
        .await
        .map_err(|_| Error::Unavailable("Xray did not become ready within 30 seconds".into()))??;
        let until = tokio::time::Instant::now() + stable;
        while tokio::time::Instant::now() < until {
            tokio::time::sleep(Duration::from_secs(1)).await;
            let status = self.system.status().await?;
            if status.active_state != "active" || status.invocation_id != generation {
                return Err(Error::Unavailable(
                    "Xray changed generation during stability observation".into(),
                ));
            }
            self.rpc
                .unary("StatsService/GetSysStats", json!({}))
                .await?;
        }
        Ok(())
    }

    pub async fn control(&self, action: &str) -> Result<Value> {
        let _guard = self.lock()?;
        self.require_installed()?;
        if action == "stop" {
            // 即使存在待确认变更，也允许操作者先停止服务。
            self.system.action("stop").await?;
            return Ok(json!({"stopped":true}));
        }
        if action != "restart" {
            // 显式 restart 是恢复配置一致性的入口，start 不能绕过待确认状态。
            self.store.ensure_consistent()?;
        }
        if self
            .store
            .pending()?
            .iter()
            .any(|operation| operation.kind == "install" || operation.kind == "replace_config")
        {
            return Err(Error::Conflict(
                "a version/configuration rollback is unfinished; inspect the agent journal".into(),
            ));
        }
        self.materialize()?;
        self.system.action(action).await?;
        self.wait_ready(Duration::ZERO).await?;
        self.restore_overrides().await?;
        if action == "restart" {
            self.store.reconciled()?;
        }
        Ok(json!({"revision":self.store.committed()?.revision,"state":"ready"}))
    }

    pub async fn replace_config(&self, config: Value) -> Result<Value> {
        let _guard = self.lock()?;
        self.store.ensure_consistent()?;
        self.require_installed()?;
        self.validate_management(&config)?;
        self.compiler
            .compile(&self.paths.binary(), &self.paths.current, &config)
            .await?;
        let previous = self.store.committed()?;
        let mut overrides = previous.overrides.clone();
        if let Some(entries) = overrides.as_object_mut() {
            // 完整替换后，仅保留 balancer 和目标出站都仍然存在的 override。
            entries.retain(|tag, target| {
                config
                    .pointer("/routing/balancers")
                    .and_then(Value::as_array)
                    .is_some_and(|entries| {
                        entries.iter().any(|entry| {
                            entry.get("tag").and_then(Value::as_str) == Some(tag.as_str())
                        })
                    })
                    && config
                        .get("outbounds")
                        .and_then(Value::as_array)
                        .is_some_and(|entries| {
                            entries.iter().any(|entry| {
                                entry.get("tag").and_then(Value::as_str) == target.as_str()
                            })
                        })
            });
        }
        let operation =
            self.store
                .begin("replace_config", Some((&config, &overrides)), json!({}))?;
        // 先提交新快照，让 systemd 的 ExecStartPre 在重启时读取到新配置。
        self.store.commit_candidate(&operation)?;
        let result = async {
            self.materialize()?;
            self.system.action("restart").await?;
            self.wait_ready(Duration::ZERO).await?;
            self.restore_overrides().await?;
            self.store.complete(&operation.id)?;
            Ok(json!({"revision":operation.candidate_revision,"state":"ready"}))
        }
        .await;
        match result {
            Ok(result) => Ok(result),
            Err(error) => {
                self.rollback_configuration(&operation).await?;
                self.store.fail(&operation.id, &error.to_string(), false)?;
                Err(error)
            }
        }
    }

    pub async fn rollback_configuration(&self, operation: &Operation) -> Result<()> {
        // 数据库指针、启动文件与运行时依次恢复；全部完成前不能解除待确认状态。
        let rollback = async {
            self.store.restore_revision(operation.previous_revision)?;
            self.materialize()?;
            self.system.action("restart").await?;
            self.wait_ready(Duration::ZERO).await?;
            self.restore_overrides().await
        }
        .await;
        if let Err(error) = rollback {
            self.store
                .fail(&operation.id, "configuration rollback failed", true)?;
            return Err(Error::Uncertain(format!(
                "configuration rollback failed: {error}"
            )));
        }
        Ok(())
    }

    pub async fn recover(&self) -> Result<()> {
        // Download staging has no runtime effects and lives on the versions filesystem.
        for entry in std::fs::read_dir(&self.paths.versions)? {
            let entry = entry?;
            if entry.file_name().to_string_lossy().starts_with(".staging-")
                && entry.file_type()?.is_dir()
            {
                std::fs::remove_dir_all(entry.path())?;
            }
        }
        self.store.recover_mutations()?;
        // 不同 kind 的提交点不同，不能对所有遗留操作统一重试或统一回滚。
        for operation in self.store.pending()? {
            match operation.kind.as_str() {
                "install" => self.recover_install(&operation).await?,
                "replace_config" => {
                    if operation.phase == "prepared" {
                        self.store.fail(
                            &operation.id,
                            "configuration replacement was not committed",
                            false,
                        )?;
                    } else {
                        self.rollback_configuration(&operation).await?;
                        self.store.fail(
                            &operation.id,
                            "interrupted configuration replacement rolled back",
                            false,
                        )?;
                    }
                }
                "mutation" if operation.phase == "committed" => {
                    self.materialize()?;
                    self.store.complete(&operation.id)?;
                }
                "mutation" if operation.phase == "uncertain" => {
                    if self.try_confirm(&operation).await.is_err() {
                        tracing::warn!(operation_id=%operation.id,"runtime outcome could not be verified; explicit restart is required");
                    }
                }
                _ => {}
            }
        }
        Ok(())
    }

    /// Read-only probes may resolve an interrupted call. Routing conditions cannot be read back
    /// completely, so AddRule deliberately remains uncertain instead of comparing tags alone.
    async fn try_confirm(&self, operation: &Operation) -> Result<()> {
        let before = self.system.status().await?;
        // 只有同一启动代次的读回结果才能解释之前那次 RPC 的执行结果。
        if before.active_state != "active"
            || operation
                .details
                .get("invocation_id")
                .and_then(Value::as_str)
                != Some(before.invocation_id.as_str())
            || operation.details.get("pid").and_then(Value::as_u64) != Some(before.main_pid as u64)
        {
            return Ok(());
        }
        let Some(revision) = operation.candidate_revision else {
            return Ok(());
        };
        let candidate = self.store.snapshot(revision)?;
        let previous = self.store.snapshot(operation.previous_revision)?;
        let method = operation
            .details
            .get("method")
            .and_then(Value::as_str)
            .unwrap_or_default();
        let tag = operation
            .details
            .get("tag")
            .and_then(Value::as_str)
            .unwrap_or_default();
        // true 表示候选已生效，false 表示确认未生效，None 表示信息不足，继续等待人工恢复。
        let observed: Option<bool> = match method {
            "HandlerService/AddInbound"
            | "HandlerService/RemoveInbound"
            | "HandlerService/AddOutbound"
            | "HandlerService/RemoveOutbound" => {
                let inbound = method.ends_with("Inbound");
                let list_method = if inbound {
                    "HandlerService/ListInbounds"
                } else {
                    "HandlerService/ListOutbounds"
                };
                let key = if inbound { "inbounds" } else { "outbounds" };
                let live = self.rpc.unary(list_method, json!({})).await?;
                let actual = live.get(key).and_then(Value::as_array).and_then(|items| {
                    items
                        .iter()
                        .find(|item| item.get("tag").and_then(Value::as_str) == Some(tag))
                });
                if method.contains("Remove") {
                    Some(actual.is_none())
                } else if let Some(actual) = actual {
                    let compiled = self
                        .compiler
                        .compile(&self.paths.binary(), &self.paths.current, &candidate.config)
                        .await?;
                    let expected = compiled
                        .get(if inbound { "inbound" } else { "outbound" })
                        .and_then(Value::as_array)
                        .and_then(|items| {
                            items
                                .iter()
                                .find(|item| item.get("tag").and_then(Value::as_str) == Some(tag))
                        });
                    if expected == Some(actual) {
                        Some(true)
                    } else {
                        None
                    }
                } else {
                    Some(false)
                }
            }
            "HandlerService/AlterInbound" => {
                let live = self
                    .rpc
                    .unary("HandlerService/GetInboundUsers", json!({"tag":tag}))
                    .await?;
                let actual = sorted_users(live.get("users"));
                let mut outcome = None;
                for (snapshot, is_candidate) in [(&candidate, true), (&previous, false)] {
                    let compiled = self
                        .compiler
                        .compile(&self.paths.binary(), &self.paths.current, &snapshot.config)
                        .await?;
                    let settings = compiled
                        .get("inbound")
                        .and_then(Value::as_array)
                        .and_then(|items| {
                            items
                                .iter()
                                .find(|item| item.get("tag").and_then(Value::as_str) == Some(tag))
                        })
                        .and_then(|item| item.pointer("/proxySettings/value"));
                    if let Some(settings) = settings {
                        if sorted_users(settings.get("users").or_else(|| settings.get("user")))
                            == actual
                        {
                            outcome = Some(is_candidate);
                            break;
                        }
                    }
                }
                outcome
            }
            "RoutingService/RemoveRule" => {
                let rule = operation
                    .details
                    .get("rule_tag")
                    .and_then(Value::as_str)
                    .unwrap_or_default();
                let live = self.rpc.unary("RoutingService/ListRule", json!({})).await?;
                Some(
                    !live
                        .get("rules")
                        .and_then(Value::as_array)
                        .is_some_and(|items| {
                            items.iter().any(|item| {
                                item.get("ruleTag").and_then(Value::as_str) == Some(rule)
                            })
                        }),
                )
            }
            "RoutingService/OverrideBalancerTarget" => {
                let balancer = operation
                    .details
                    .get("balancer_tag")
                    .and_then(Value::as_str)
                    .unwrap_or_default();
                let live = self
                    .rpc
                    .unary("RoutingService/GetBalancerInfo", json!({"tag":balancer}))
                    .await?;
                let actual = live
                    .pointer("/balancer/override/target")
                    .and_then(Value::as_str)
                    .unwrap_or_default();
                if actual
                    == candidate
                        .overrides
                        .get(balancer)
                        .and_then(Value::as_str)
                        .unwrap_or_default()
                {
                    Some(true)
                } else if actual
                    == previous
                        .overrides
                        .get(balancer)
                        .and_then(Value::as_str)
                        .unwrap_or_default()
                {
                    Some(false)
                } else {
                    None
                }
            }
            _ => None,
        };
        let after = self.system.status().await?;
        if after.invocation_id != before.invocation_id
            || after.main_pid != before.main_pid
            || after.active_state != "active"
        {
            return Ok(());
        }
        if observed == Some(true) {
            self.store.commit_candidate(operation)?;
            self.materialize()?;
            self.store.complete(&operation.id)?;
        } else if observed == Some(false)
            && self.store.committed()?.revision == operation.previous_revision
        {
            // 已提交的新快照不能仅因运行时不同就被丢弃；这里只结束尚未提交的失败候选。
            self.store.fail(
                &operation.id,
                "readback confirmed the operation was not applied",
                false,
            )?;
        }
        Ok(())
    }
}

fn sorted_users(value: Option<&Value>) -> Vec<String> {
    // 用户列表返回顺序不稳定，核对配置时比较内容集合而非原始数组顺序。
    let mut users: Vec<_> = value
        .and_then(Value::as_array)
        .map(|users| users.iter().map(Value::to_string).collect())
        .unwrap_or_default();
    users.sort();
    users
}
