//! Agent 自更新：复用发行包校验和部署，HTTP 请求将执行交给独立 systemd 服务。

use crate::{
    config::{Paths, private_directory, private_file},
    error::{Error, Result},
};
use clap::Args;
use serde::{Deserialize, Serialize};
use std::{fs, process::Stdio, time::Duration};
use tokio::process::Command;
use utoipa::ToSchema;

const UNIT: &str = "xray-agent-update.service";

#[derive(Args, Default, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct UpdateRequest {
    /// Agent release tag; omitted values use the latest stable GitHub Release.
    #[arg(long = "target-version")]
    #[schema(default = "latest", example = "v0.1.2")]
    pub version: Option<String>,
}

impl UpdateRequest {
    fn version(&self) -> &str {
        self.version.as_deref().unwrap_or("latest")
    }

    fn validate(&self) -> Result<()> {
        let version = self.version();
        if version != "latest"
            && (!version.starts_with('v')
                || !version.as_bytes().get(1).is_some_and(u8::is_ascii_digit)
                || !version.bytes().all(|byte| {
                    byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'+' | b'-')
                }))
        {
            return Err(Error::BadRequest(
                "Agent version must be latest or a release tag such as v0.1.2".into(),
            ));
        }
        Ok(())
    }
}

#[derive(Serialize, ToSchema)]
pub struct UpdateAccepted {
    pub current_version: String,
    /// Requested release tag; latest is resolved by the background updater.
    pub requested_version: String,
    /// Independent systemd service; acceptance does not mean the update has completed.
    pub unit: String,
}

pub fn lock_update() -> Result<fs::File> {
    let lock_path = Paths::default().database.with_file_name("update.lock");
    let directory = lock_path.parent().unwrap();
    fs::create_dir_all(directory)?;
    private_directory(directory)?;
    let lock = fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .open(&lock_path)?;
    private_file(&lock_path)?;
    // 与 Agent 服务锁独立，下载时管理接口仍可工作，CLI 与 HTTP 更新不能重叠。
    fs2::FileExt::try_lock_exclusive(&lock)
        .map_err(|_| Error::Conflict("an Agent update is already running".into()))?;
    Ok(lock)
}

pub async fn run(request: UpdateRequest) -> Result<()> {
    request.validate()?;
    let _lock = lock_update()?;
    let status = Command::new("/bin/sh")
        .args([
            "-c",
            include_str!("../install.sh"),
            "agent-update",
            "--update",
            "--version",
            request.version(),
        ])
        .stdin(Stdio::null())
        .kill_on_drop(true)
        .status()
        .await?;
    if !status.success() {
        return Err(Error::Unavailable(format!(
            "Agent update failed ({status}); inspect the installer output"
        )));
    }
    Ok(())
}

pub async fn schedule(request: UpdateRequest) -> Result<UpdateAccepted> {
    request.validate()?;
    // 独立 cgroup 不会随 xray-agent.service 停止；固定 unit 名阻止重复提交。
    let output = tokio::time::timeout(
        Duration::from_secs(15),
        Command::new("systemd-run")
            .args([
                "--unit",
                UNIT,
                "--collect",
                "--property=Type=exec",
                "--property=UMask=0077",
                "--",
                "/usr/local/bin/agent",
                "update",
                "--target-version",
                request.version(),
            ])
            .stdin(Stdio::null())
            .kill_on_drop(true)
            .output(),
    )
    .await
    .map_err(|_| Error::Unavailable("Agent update scheduling timed out".into()))??;
    if !output.status.success() {
        return Err(Error::Unavailable(format!(
            "could not schedule Agent update: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        )));
    }
    Ok(UpdateAccepted {
        current_version: env!("CARGO_PKG_VERSION").into(),
        requested_version: request.version().into(),
        unit: UNIT.into(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn update_versions_match_release_tag_rules() {
        assert_eq!(UpdateRequest::default().version(), "latest");
        for version in ["latest", "v0.1.2", "v0.2.0-rc.1", "v0.1.2+build"] {
            UpdateRequest {
                version: Some(version.into()),
            }
            .validate()
            .unwrap();
        }
        for version in ["", "v", "0.1.2", "--help", "v1/../../etc", "v1;id", "v1\n"] {
            assert!(
                UpdateRequest {
                    version: Some(version.into())
                }
                .validate()
                .is_err()
            );
        }
    }
}
