//! 封装 Xray 配置转换与 systemd 操作，统一管理子进程超时和退出状态。

use crate::{
    config::{Paths, atomic_write},
    error::{Error, Result},
    grpc,
};
use async_trait::async_trait;
use prost_reflect::DynamicMessage;
use serde::Serialize;
use serde_json::Value;
use std::{path::Path, process::Stdio, time::Duration};
use tokio::process::Command;
use utoipa::ToSchema;

async fn execute(command: &mut Command, timeout: Duration) -> Result<std::process::Output> {
    // 禁止等待交互输入；调用超时或 future 被丢弃时结束对应子进程。
    command.stdin(Stdio::null()).kill_on_drop(true);
    tokio::time::timeout(timeout, command.output())
        .await
        .map_err(|_| Error::Unavailable("subprocess timed out".into()))?
        .map_err(Error::from)
}

#[async_trait]
pub trait ConfigCompiler: Send + Sync {
    // 返回经过校验的完整 core.Config，TypedMessage 已展开为结构化 JSON。
    async fn compile(&self, binary: &Path, assets: &Path, config: &Value) -> Result<Value>;
}

pub struct Compiler {
    pub paths: Paths,
}

#[async_trait]
impl ConfigCompiler for Compiler {
    async fn compile(&self, binary: &Path, assets: &Path, config: &Value) -> Result<Value> {
        let temporary = tempfile::TempDir::new_in(&self.paths.work)?;
        let input = temporary.path().join("config.json");
        let output = temporary.path().join("config.pb");
        atomic_write(&input, &serde_json::to_vec(config)?)?;
        // 复用候选版本自己的配置解析器，避免在 Rust 中重复实现各协议的转换规则。
        let mut command = Command::new(binary);
        command
            .args(["convert", "pb", "-outpbfile"])
            .arg(&output)
            .arg(&input)
            .env("XRAY_LOCATION_ASSET", assets);
        let result = execute(&mut command, Duration::from_secs(30)).await?;
        if !result.status.success() {
            return Err(Error::Validation(format!(
                "Xray config conversion failed: {}",
                diagnostic(&result)
            )));
        }
        let bytes = tokio::fs::read(&output).await?;
        let descriptor = grpc::pool()
            .get_message_by_name("xray.core.Config")
            .unwrap();
        let compiled = DynamicMessage::decode(descriptor, bytes.as_slice())
            .map_err(|error| Error::Validation(format!("invalid converted protobuf: {error}")))?;
        // 转换成功不代表组件能够初始化，继续执行 -test；不启动另一个代理服务。
        let mut command = Command::new(binary);
        command
            .args(["run", "-test", "-config"])
            .arg(&input)
            .env("XRAY_LOCATION_ASSET", assets);
        let result = execute(&mut command, Duration::from_secs(30)).await?;
        if !result.status.success() {
            return Err(Error::Validation(format!(
                "Xray config validation failed: {}",
                diagnostic(&result)
            )));
        }
        grpc::to_json(&compiled)
    }
}

fn diagnostic(output: &std::process::Output) -> String {
    // Xray reports validation failures on either stdout or stderr. Do not log this output.
    let mut text = String::from_utf8_lossy(&output.stderr).into_owned();
    if text.trim().is_empty() {
        text = String::from_utf8_lossy(&output.stdout).into_owned();
    }
    text.chars().take(2000).collect()
}

#[derive(Clone, Debug, Default, Serialize, ToSchema)]
pub struct SystemStatus {
    pub load_state: String,
    pub active_state: String,
    pub sub_state: String,
    pub main_pid: u32,
    // PID 可能复用；InvocationID 用于识别 systemd 的同一次服务启动。
    pub invocation_id: String,
    pub unit_file_state: String,
}

#[async_trait]
pub trait ServiceControl: Send + Sync {
    // 使用接口隔离真实 systemd，使故障测试可以记录操作而不控制本机服务。
    async fn status(&self) -> Result<SystemStatus>;
    async fn action(&self, action: &str) -> Result<()>;
    async fn daemon_reload(&self) -> Result<()>;
    async fn enable(&self) -> Result<()>;
    async fn disable(&self) -> Result<()>;
}

pub struct Systemd;

impl Systemd {
    async fn command(arguments: &[&str]) -> Result<std::process::Output> {
        if !cfg!(target_os = "linux") {
            return Err(Error::Unsupported(
                "systemd operations require Linux".into(),
            ));
        }
        execute(
            // 固定程序、服务名和参数数组，不经过 shell 解释用户输入。
            Command::new("/usr/bin/systemctl")
                .arg("--no-ask-password")
                .args(arguments),
            Duration::from_secs(60),
        )
        .await
    }

    async fn checked(arguments: &[&str]) -> Result<()> {
        let result = Self::command(arguments).await?;
        if !result.status.success() {
            return Err(Error::Unavailable(format!(
                "systemctl failed: {}",
                diagnostic(&result)
            )));
        }
        Ok(())
    }
}

#[async_trait]
impl ServiceControl for Systemd {
    async fn status(&self) -> Result<SystemStatus> {
        let output = Self::command(&[
            "show",
            "xray.service",
            "--property=LoadState,ActiveState,SubState,MainPID,InvocationID,UnitFileState",
        ])
        .await?;
        // 首次安装之前 unit 尚不存在，应返回未安装状态，而不是阻断安装流程。
        if !output.status.success()
            && !String::from_utf8_lossy(&output.stdout)
                .lines()
                .any(|line| line == "LoadState=not-found")
        {
            return Err(Error::Unavailable(format!(
                "cannot inspect xray.service: {}",
                diagnostic(&output)
            )));
        }
        let mut status = SystemStatus::default();
        for line in String::from_utf8_lossy(&output.stdout).lines() {
            let Some((key, value)) = line.split_once('=') else {
                continue;
            };
            match key {
                "LoadState" => status.load_state = value.into(),
                "ActiveState" => status.active_state = value.into(),
                "SubState" => status.sub_state = value.into(),
                "MainPID" => status.main_pid = value.parse().unwrap_or_default(),
                "InvocationID" => status.invocation_id = value.into(),
                "UnitFileState" => status.unit_file_state = value.into(),
                _ => {}
            }
        }
        Ok(status)
    }

    async fn action(&self, action: &str) -> Result<()> {
        if !matches!(action, "start" | "stop" | "restart") {
            return Err(Error::BadRequest("invalid service action".into()));
        }
        if action != "stop" {
            // 清除之前触发的启动频率限制，允许修复后启动或回滚到旧版本。
            Self::checked(&["reset-failed", "xray.service"]).await?;
        }
        Self::checked(&[action, "xray.service"]).await
    }
    async fn daemon_reload(&self) -> Result<()> {
        Self::checked(&["daemon-reload"]).await
    }
    async fn enable(&self) -> Result<()> {
        Self::checked(&["enable", "xray.service"]).await
    }
    async fn disable(&self) -> Result<()> {
        Self::checked(&["disable", "xray.service"]).await
    }
}
