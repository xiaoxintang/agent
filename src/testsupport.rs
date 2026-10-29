//! 测试专用的配置编译器、RPC 和 systemd 替身，隔离真实网络和系统目录。

use crate::{
    app::App,
    config::{Paths, Settings, initial_xray},
    error::{Error, Result},
    grpc::{EventStream, XrayRpc},
    process::{ConfigCompiler, ServiceControl, SystemStatus},
    store::Store,
};
use async_trait::async_trait;
use serde_json::{Value, json};
use std::{
    path::Path,
    sync::{
        Arc, Mutex,
        atomic::{AtomicUsize, Ordering},
    },
};

#[derive(Clone, Copy, PartialEq)]
pub enum Outcome {
    // 每种模式对应一个事务边界，验证错误不会被误报为成功或触发自动重启。
    Success,
    Timeout,
    Rejected,
    PublicationFailure,
    CompilationFailure,
    RestartFailureOnce,
    DelayedSuccess,
}

pub struct FakeRuntime {
    pub outcome: Outcome,
    pub calls: AtomicUsize,
    // 记录所有服务控制动作，测试据此确认动态变更没有偷偷重启 Xray。
    pub actions: Mutex<Vec<String>>,
    config_path: std::path::PathBuf,
}

#[async_trait]
impl XrayRpc for FakeRuntime {
    async fn unary(&self, method: &str, _: Value) -> Result<Value> {
        if method == "StatsService/GetSysStats" {
            return Ok(json!({"Uptime":1}));
        }
        self.calls.fetch_add(1, Ordering::SeqCst);
        if self.outcome == Outcome::DelayedSuccess {
            // 为客户端先断开连接留出窗口，验证后台持久化任务仍会完成。
            tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        }
        match self.outcome {
            Outcome::Timeout => Err(Error::Grpc {
                code: tonic::Code::DeadlineExceeded,
                message: "injected timeout".into(),
            }),
            Outcome::Rejected => Err(Error::Grpc {
                code: tonic::Code::InvalidArgument,
                message: "injected rejection".into(),
            }),
            Outcome::PublicationFailure => {
                // RPC 已返回成功，但目标文件被目录占据，强制触发发布失败分支。
                if self.config_path.is_file() {
                    std::fs::remove_file(&self.config_path)?;
                }
                std::fs::create_dir(&self.config_path)?;
                Ok(json!({}))
            }
            _ => Ok(json!({})),
        }
    }
    async fn subscribe(&self, _: Value) -> Result<EventStream> {
        Ok(Box::pin(futures_util::stream::empty()))
    }
}

#[async_trait]
impl ConfigCompiler for FakeRuntime {
    async fn compile(&self, _: &Path, _: &Path, config: &Value) -> Result<Value> {
        if self.outcome == Outcome::CompilationFailure {
            return Err(Error::Validation("injected compiler failure".into()));
        }
        let inbounds: Vec<_> = config["inbounds"]
            .as_array()
            .unwrap()
            .iter()
            .map(|inbound| json!({"tag":inbound["tag"]}))
            .collect();
        Ok(json!({"inbound":inbounds}))
    }
}

#[async_trait]
impl ServiceControl for FakeRuntime {
    async fn status(&self) -> Result<SystemStatus> {
        // 固定 PID 和 InvocationID，避免无关的启动代次变化干扰事务测试。
        Ok(SystemStatus {
            load_state: "loaded".into(),
            active_state: "active".into(),
            sub_state: "running".into(),
            main_pid: 42,
            invocation_id: "same-generation".into(),
            unit_file_state: "enabled".into(),
        })
    }
    async fn action(&self, action: &str) -> Result<()> {
        let mut actions = self.actions.lock().unwrap();
        actions.push(action.to_owned());
        if self.outcome == Outcome::RestartFailureOnce && action == "restart" && actions.len() == 1
        {
            return Err(Error::Unavailable("injected restart failure".into()));
        }
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

pub fn fixture(root: &Path, outcome: Outcome) -> (Arc<App>, Arc<FakeRuntime>) {
    // 每个测试独立使用真实 SQLite 和生成文件；只有外部进程、网络调用被替换。
    let settings = Settings {
        listen: "127.0.0.1:0".parse().unwrap(),
        grpc_listen: "127.0.0.1:10085".parse().unwrap(),
        token: "a".repeat(43),
    };
    let paths = Paths::temporary(root);
    paths.prepare().unwrap();
    std::fs::create_dir_all(&paths.current).unwrap();
    std::fs::write(paths.binary(), "fake binary").unwrap();
    let runtime = Arc::new(FakeRuntime {
        outcome,
        calls: AtomicUsize::new(0),
        actions: Mutex::new(vec![]),
        config_path: paths.config.clone(),
    });
    let app = Arc::new(App {
        store: Store::open(&paths.database, &initial_xray(&settings)).unwrap(),
        settings,
        paths,
        rpc: runtime.clone(),
        compiler: runtime.clone(),
        system: runtime.clone(),
        operations: tokio::sync::Mutex::new(()),
        tasks: tokio_util::task::TaskTracker::new(),
    });
    app.materialize().unwrap();
    (app, runtime)
}
