//! 按版本管理 Xray 二进制与资源，负责下载校验、原子切换、失败回滚和历史清理。
//! current 链接决定活动版本，SQLite 操作阶段决定中断后应回滚还是继续清理。

use crate::{
    app::App,
    config::{atomic_write, default_xray_version},
    error::{Error, Result},
    store::{Operation, now},
};
use futures_util::StreamExt;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{
    fs,
    io::{Read, Write},
    path::{Path, PathBuf},
    time::Duration,
};
use utoipa::ToSchema;

const MAX_DOWNLOAD: usize = 128 * 1024 * 1024;
const MAX_EXPANDED: u64 = 512 * 1024 * 1024;
// unit 模板嵌入二进制，一键安装 Agent 后无需再次下载 Xray 的服务定义。
pub const UNIT: &str = include_str!("../deploy/xray.service");

#[derive(Clone, Debug, Deserialize, Serialize, ToSchema)]
pub struct Manifest {
    pub version: String,
    pub architecture: String,
    pub installed_at: i64,
    pub sha256: String,
    // available 只表示安装包已就绪；实际启动结果由 ready/failed 和 last_error 记录。
    pub status: String,
    pub last_error: Option<String>,
}

#[derive(Clone, Debug, Serialize, ToSchema)]
pub struct VersionInfo {
    #[serde(flatten)]
    pub manifest: Manifest,
    pub active: bool,
}

pub fn validate_version(version: &str) -> Result<()> {
    // 同一版本号会进入下载 URL 和本地目录名，必须拒绝分隔符及路径穿越输入。
    if version.len() > 64
        || !version.starts_with('v')
        || !version.as_bytes().get(1).is_some_and(u8::is_ascii_digit)
        || !version
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'-' | b'_'))
    {
        return Err(Error::BadRequest(
            "version must be an official tag such as v26.10.21, without path separators".into(),
        ));
    }
    Ok(())
}

pub fn asset(architecture: &str) -> Result<&'static str> {
    match architecture {
        "x86_64" => Ok("Xray-linux-64.zip"),
        "aarch64" => Ok("Xray-linux-arm64-v8a.zip"),
        _ => Err(Error::Unsupported(format!(
            "unsupported Linux architecture: {architecture}"
        ))),
    }
}

fn read_manifest(directory: &Path) -> Result<Manifest> {
    // 只接受有匹配清单的真实版本目录，删除时不能跟随任意符号链接。
    let metadata = fs::symlink_metadata(directory)?;
    if !metadata.is_dir() || metadata.file_type().is_symlink() {
        return Err(Error::Conflict(
            "version directory must be a real managed directory".into(),
        ));
    }
    let manifest: Manifest = serde_json::from_slice(&fs::read(directory.join("manifest.json"))?)?;
    validate_version(&manifest.version)?;
    if directory.file_name().and_then(|name| name.to_str()) != Some(manifest.version.as_str()) {
        return Err(Error::Conflict(
            "version manifest does not match directory".into(),
        ));
    }
    Ok(manifest)
}

fn write_manifest(directory: &Path, manifest: &Manifest) -> Result<()> {
    atomic_write(
        &directory.join("manifest.json"),
        &serde_json::to_vec_pretty(manifest)?,
    )
}

pub fn sha256_from_digest(text: &str) -> Result<String> {
    // 官方 .dgst 同时提供多种算法，这里只接受完整的 SHA2-256 摘要。
    for line in text.lines() {
        if let Some((algorithm, value)) = line.split_once('=') {
            if algorithm.trim() == "SHA2-256" {
                let digest = value.trim();
                if digest.len() == 64 && digest.bytes().all(|byte| byte.is_ascii_hexdigit()) {
                    return Ok(digest.to_ascii_lowercase());
                }
            }
        }
    }
    Err(Error::Validation(
        "official digest has no valid SHA2-256 value".into(),
    ))
}

fn extract(bytes: &[u8], directory: &Path) -> Result<()> {
    let mut archive = zip::ZipArchive::new(std::io::Cursor::new(bytes))
        .map_err(|error| Error::Validation(format!("invalid Xray ZIP: {error}")))?;
    let mut extracted = std::collections::HashSet::new();
    let mut total = 0u64;
    for index in 0..archive.len() {
        let mut entry = archive
            .by_index(index)
            .map_err(|error| Error::Validation(error.to_string()))?;
        let name = entry.name().to_owned();
        // Never join archive-provided paths. Only these exact top-level filenames are accepted.
        if !matches!(
            name.as_str(),
            "xray" | "geoip.dat" | "geosite.dat" | "LICENSE"
        ) {
            continue;
        }
        if entry.is_dir()
            || entry
                .unix_mode()
                .is_some_and(|mode| mode & 0o170000 == 0o120000)
            || !extracted.insert(name.clone())
        {
            return Err(Error::Validation(
                "archive contains duplicate or nonregular expected files".into(),
            ));
        }
        total = total
            .checked_add(entry.size())
            .ok_or_else(|| Error::Validation("archive is too large".into()))?;
        if total > MAX_EXPANDED {
            return Err(Error::Validation("expanded archive exceeds 512 MiB".into()));
        }
        let mut destination = fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(directory.join(&name))?;
        let written = std::io::copy(
            // 实际读取也设置上限，避免完全依赖 ZIP 中声明的解压大小。
            &mut Read::by_ref(&mut entry).take(MAX_EXPANDED + 1),
            &mut destination,
        )?;
        if written > MAX_EXPANDED {
            return Err(Error::Validation("expanded entry is too large".into()));
        }
        destination.flush()?;
        destination.sync_all()?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(
                directory.join(&name),
                fs::Permissions::from_mode(if name == "xray" { 0o755 } else { 0o644 }),
            )?;
        }
    }
    if !extracted.contains("xray") {
        return Err(Error::Validation(
            "archive contains no xray executable".into(),
        ));
    }
    Ok(())
}

async fn download(client: &reqwest::Client, url: &str, limit: usize) -> Result<Vec<u8>> {
    let response = client.get(url).send().await.map_err(|error| {
        Error::Unavailable(format!("official release download failed: {error}"))
    })?;
    if response.status() == reqwest::StatusCode::NOT_FOUND {
        return Err(Error::NotFound(
            "official version or architecture asset not found".into(),
        ));
    }
    let response = response.error_for_status().map_err(|error| {
        Error::Unavailable(format!("official release download failed: {error}"))
    })?;
    if response
        .content_length()
        .is_some_and(|length| length > limit as u64)
    {
        return Err(Error::Validation("download is too large".into()));
    }
    let mut stream = response.bytes_stream();
    let mut bytes = Vec::new();
    // 分块累计限制同时覆盖缺少或不可信 Content-Length 的响应。
    while let Some(chunk) = stream.next().await {
        let chunk = chunk.map_err(|error| Error::Unavailable(error.to_string()))?;
        if bytes.len() + chunk.len() > limit {
            return Err(Error::Validation("download exceeds size limit".into()));
        }
        bytes.extend_from_slice(&chunk);
    }
    Ok(bytes)
}

impl App {
    pub fn active_version(&self) -> Result<Option<String>> {
        let target = match fs::read_link(&self.paths.current) {
            Ok(target) => target,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(error) => {
                return Err(Error::Conflict(format!(
                    "current is not a managed symlink: {error}"
                )));
            }
        };
        let resolved = if target.is_absolute() {
            target
        } else {
            self.paths.current.parent().unwrap().join(target)
        };
        let canonical = fs::canonicalize(&resolved)?;
        // 解析链接后的真实父目录必须仍是受管目录，不能仅按路径文本前缀判断。
        if canonical.parent() != Some(fs::canonicalize(&self.paths.versions)?.as_path()) {
            return Err(Error::Conflict(
                "current points outside managed versions".into(),
            ));
        }
        let manifest = read_manifest(&canonical)?;
        Ok(Some(manifest.version))
    }

    pub fn list_versions(&self) -> Result<Vec<VersionInfo>> {
        let active = self.active_version()?;
        let mut versions = Vec::new();
        for entry in fs::read_dir(&self.paths.versions)? {
            let entry = entry?;
            // 安装暂存目录尚无正式 manifest，不能作为已安装版本暴露。
            if entry.file_name().to_string_lossy().starts_with('.') {
                continue;
            }
            if !entry.file_type()?.is_dir() {
                continue;
            }
            let manifest = read_manifest(&entry.path())?;
            versions.push(VersionInfo {
                active: active.as_deref() == Some(manifest.version.as_str()),
                manifest,
            });
        }
        versions.sort_by(|left, right| {
            right
                .manifest
                .installed_at
                .cmp(&left.manifest.installed_at)
                .then(left.manifest.version.cmp(&right.manifest.version))
        });
        Ok(versions)
    }

    fn switch_version(&self, version: Option<&str>) -> Result<()> {
        // 先创建新链接再 rename，避免 current 在切换过程中短暂指向半成品。
        if let Some(version) = version {
            validate_version(version)?;
            read_manifest(&self.paths.versions.join(version))?;
            let temporary = self
                .paths
                .current
                .with_file_name(format!(".current-{}", uuid::Uuid::new_v4()));
            #[cfg(unix)]
            std::os::unix::fs::symlink(self.paths.versions.join(version), &temporary)?;
            #[cfg(not(unix))]
            return Err(Error::Unsupported("Unix symlinks are required".into()));
            fs::rename(&temporary, &self.paths.current)?;
        } else {
            match fs::remove_file(&self.paths.current) {
                Ok(()) => {}
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                Err(error) => return Err(error.into()),
            }
        }
        fs::File::open(self.paths.current.parent().unwrap())?.sync_all()?;
        Ok(())
    }

    async fn fetch_version(&self, version: &str) -> Result<PathBuf> {
        let directory = self.paths.versions.join(version);
        if directory.exists() {
            let manifest = read_manifest(&directory)?;
            if manifest.architecture != std::env::consts::ARCH {
                return Err(Error::Conflict(
                    "cached version has a different architecture".into(),
                ));
            }
            return Ok(directory);
        }
        let asset_name = asset(std::env::consts::ARCH)?;
        let base =
            format!("https://github.com/XTLS/Xray-core/releases/download/{version}/{asset_name}");
        let client = reqwest::Client::builder()
            .https_only(true)
            .timeout(Duration::from_secs(180))
            .connect_timeout(Duration::from_secs(15))
            .user_agent("xray-agent/0.1")
            .build()
            .map_err(|error| Error::Internal(error.to_string()))?;
        let digest = download(&client, &format!("{base}.dgst"), 16 * 1024).await?;
        let expected = sha256_from_digest(
            std::str::from_utf8(&digest)
                .map_err(|_| Error::Validation("digest must be UTF-8".into()))?,
        )?;
        let bytes = download(&client, &base, MAX_DOWNLOAD).await?;
        let actual = hex::encode(Sha256::digest(&bytes));
        if actual != expected {
            return Err(Error::Validation("Xray archive SHA-256 mismatch".into()));
        }
        // 暂存于版本目录所在文件系统，确保最终移动不会因跨分区而失败。
        let temporary = tempfile::Builder::new()
            .prefix(".staging-")
            .tempdir_in(&self.paths.versions)?;
        let extraction = temporary.path().join("version");
        fs::create_dir(&extraction)?;
        let extraction_task = extraction.clone();
        // ZIP 解压是阻塞 I/O 与 CPU 工作，不占用 Tokio 的异步执行线程。
        tokio::task::spawn_blocking(move || extract(&bytes, &extraction_task))
            .await
            .map_err(|error| Error::Internal(error.to_string()))??;
        let manifest = Manifest {
            version: version.into(),
            architecture: std::env::consts::ARCH.into(),
            installed_at: now(),
            sha256: actual,
            status: "available".into(),
            last_error: None,
        };
        write_manifest(&extraction, &manifest)?;
        fs::rename(&extraction, &directory)?;
        fs::File::open(&self.paths.versions)?.sync_all()?;
        Ok(directory)
    }

    fn mark_version(&self, version: &str, status: &str, error: Option<String>) -> Result<()> {
        let directory = self.paths.versions.join(version);
        let mut manifest = read_manifest(&directory)?;
        manifest.status = status.into();
        manifest.last_error = error;
        write_manifest(&directory, &manifest)
    }

    pub fn cleanup_versions(&self) -> Result<Vec<Value>> {
        // 必须先确认活动版本；清理失败汇总到响应，不能回滚已健康运行的新版本。
        let active = self.active_version()?.ok_or_else(|| {
            Error::Conflict("cannot clean versions without an active version".into())
        })?;
        let mut failures = Vec::new();
        for version in self.list_versions()? {
            if version.manifest.version == active {
                continue;
            }
            if let Err(error) =
                fs::remove_dir_all(self.paths.versions.join(&version.manifest.version))
            {
                failures
                    .push(json!({"version":version.manifest.version,"error":error.to_string()}));
            }
        }
        fs::File::open(&self.paths.versions)?.sync_all()?;
        Ok(failures)
    }

    pub async fn delete_version(&self, version: &str) -> Result<Value> {
        let _guard = self.lock()?;
        validate_version(version)?;
        if self.active_version()?.as_deref() == Some(version) {
            return Err(Error::Conflict("cannot delete the active version".into()));
        }
        for operation in self.store.pending()? {
            // 候选版本和回滚目标都可能还会被事务使用，即使当前不活动也不能删除。
            if ["version", "previous"]
                .iter()
                .any(|key| operation.details.get(key).and_then(Value::as_str) == Some(version))
            {
                return Err(Error::Conflict(
                    "version is referenced by an unfinished operation".into(),
                ));
            }
        }
        let directory = self.paths.versions.join(version);
        if !directory.exists() {
            return Err(Error::NotFound("local version not found".into()));
        }
        read_manifest(&directory)?;
        fs::remove_dir_all(directory)?;
        fs::File::open(&self.paths.versions)?.sync_all()?;
        Ok(json!({"deleted":version}))
    }

    pub async fn install(&self, version: Option<String>) -> Result<Value> {
        let _guard = self.lock()?;
        if !cfg!(target_os = "linux") {
            return Err(Error::Unsupported(
                "Xray installation requires Linux with systemd".into(),
            ));
        }
        self.store.ensure_consistent()?;
        let version = version.unwrap_or_else(default_xray_version);
        let version = version.as_str();
        validate_version(version)?;
        let previous = self.active_version()?;
        if previous.as_deref() == Some(version) {
            self.wait_ready(Duration::ZERO).await?;
            return Ok(
                json!({"version":version,"already_installed":true,"cleanup_pending":self.cleanup_versions()?}),
            );
        }
        // 只有明确由本 Agent 管理的部署才允许覆盖，避免接管机器上的既有服务。
        if self.paths.unit.exists()
            && !fs::read_to_string(&self.paths.unit)?.starts_with("# Managed by xray-agent")
        {
            return Err(Error::Conflict(
                "existing xray.service is not managed by this agent".into(),
            ));
        }
        if previous.is_none()
            && (Path::new("/usr/local/bin/xray").exists()
                || (self.paths.config.exists() && !self.store.has_history()?))
        {
            return Err(Error::Conflict(
                "existing Xray installation/configuration is not managed by this agent".into(),
            ));
        }
        let previous_status = self.system.status().await?;
        if previous_status.load_state == "loaded" && !self.paths.unit.exists() {
            return Err(Error::Conflict(
                "xray.service exists outside the agent's managed unit path".into(),
            ));
        }
        let directory = self.fetch_version(version).await?;
        let snapshot = self.store.committed()?;
        // 候选程序先验证现有配置，失败时仍保留当前版本和进程。
        if let Err(error) = self
            .compiler
            .compile(&directory.join("xray"), &directory, &snapshot.config)
            .await
        {
            self.mark_version(version, "failed", Some(error.to_string()))?;
            return Err(error);
        }
        // 记录原始运行、启用状态和本次新建文件，回滚时只撤销本次安装造成的变化。
        let operation = self.store.begin("install",None,json!({"version":version,"previous":previous,"unit_created":!self.paths.unit.exists(),"config_created":!self.paths.config.exists(),"was_active":previous_status.active_state=="active","was_enabled":previous_status.unit_file_state=="enabled"}))?;
        let activation: Result<()> = async {
            self.materialize()?;
            if !self.paths.unit.exists() {
                atomic_write(&self.paths.unit, UNIT.as_bytes())?;
                #[cfg(unix)]
                {
                    use std::os::unix::fs::PermissionsExt;
                    fs::set_permissions(&self.paths.unit, fs::Permissions::from_mode(0o644))?;
                }
            }
            self.system.daemon_reload().await?;
            self.store.phase(&operation.id, "switching")?;
            self.switch_version(Some(version))?;
            self.system.enable().await?;
            self.system.action("restart").await?;
            self.wait_ready(Duration::from_secs(10)).await?;
            self.restore_overrides().await?;
            self.mark_version(version, "ready", None)?;
            // activated 是清理前的持久提交点；此后即使 Agent 中断，也不能再回滚旧版本。
            self.store.phase(&operation.id, "activated")?;
            Ok(())
        }
        .await;
        if let Err(error) = activation {
            self.rollback_install(&operation).await?;
            self.mark_version(version, "failed", Some(error.to_string()))?;
            self.store.fail(&operation.id, &error.to_string(), false)?;
            return Err(error);
        }
        let cleanup = match self.cleanup_versions() {
            Ok(failures) => failures,
            Err(error) => vec![json!({"error":error.to_string()})],
        };
        self.store.complete(&operation.id)?;
        Ok(json!({"version":version,"already_installed":false,"cleanup_pending":cleanup}))
    }

    async fn rollback_install(&self, operation: &Operation) -> Result<()> {
        // 先停候选再切回旧链接；只有旧版本原本运行时才重新启动它。
        let rollback: Result<()> = async {
            let previous = operation.details.get("previous").and_then(Value::as_str);
            if self.paths.unit.exists() || previous.is_some() {
                self.system.action("stop").await?;
            }
            self.switch_version(previous)?;
            if previous.is_some() {
                self.materialize()?;
                if operation.details.get("was_active").and_then(Value::as_bool) == Some(true) {
                    self.system.action("start").await?;
                    self.wait_ready(Duration::ZERO).await?;
                    self.restore_overrides().await?;
                }
            }
            if self.paths.unit.exists()
                && operation
                    .details
                    .get("was_enabled")
                    .and_then(Value::as_bool)
                    != Some(true)
            {
                self.system.disable().await?;
            }
            if previous.is_none() {
                if operation
                    .details
                    .get("unit_created")
                    .and_then(Value::as_bool)
                    == Some(true)
                    && self.paths.unit.exists()
                {
                    fs::remove_file(&self.paths.unit)?;
                    self.system.daemon_reload().await?;
                }
                if operation
                    .details
                    .get("config_created")
                    .and_then(Value::as_bool)
                    == Some(true)
                    && self.paths.config.exists()
                {
                    fs::remove_file(&self.paths.config)?;
                }
            }
            Ok(())
        }
        .await;
        if let Err(error) = rollback {
            self.store
                .fail(&operation.id, "version rollback failed", true)?;
            return Err(Error::Uncertain(format!(
                "version rollback failed: {error}"
            )));
        }
        Ok(())
    }

    pub async fn recover_install(&self, operation: &Operation) -> Result<()> {
        if operation.phase == "activated" {
            // Activation was durably committed. Never roll back after deleting an old version.
            let failures = self.cleanup_versions()?;
            if !failures.is_empty() {
                tracing::warn!("some inactive version directories could not be removed");
            }
            self.store.complete(&operation.id)?;
        } else {
            self.rollback_install(operation).await?;
            if let Some(version) = operation.details.get("version").and_then(Value::as_str) {
                self.mark_version(
                    version,
                    "failed",
                    Some("interrupted installation rolled back".into()),
                )?;
            }
            self.store
                .fail(&operation.id, "interrupted installation rolled back", false)?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    // 使用临时目录和模拟服务验证删除保护及恢复阶段，不安装真实的系统服务。
    use super::*;

    #[test]
    fn paths_and_unsupported_architectures_are_rejected() {
        for version in ["../v1", "v1/../../etc", "v1?x=1", "v1\\evil", "latest", "v"] {
            assert!(validate_version(version).is_err());
        }
        assert!(validate_version("v26.10.21").is_ok());
        assert_eq!(asset("aarch64").unwrap(), "Xray-linux-arm64-v8a.zip");
        assert!(asset("mips").is_err());
    }

    #[test]
    fn digest_parser_requires_the_full_sha256() {
        let expected = "a".repeat(64);
        assert_eq!(
            sha256_from_digest(&format!("MD5= bad\nSHA2-256= {expected}\n")).unwrap(),
            expected
        );
        assert!(sha256_from_digest("SHA2-256= short").is_err());
    }

    fn version(app: &App, name: &str) {
        let directory = app.paths.versions.join(name);
        fs::create_dir_all(&directory).unwrap();
        fs::write(directory.join("xray"), "fixture").unwrap();
        write_manifest(
            &directory,
            &Manifest {
                version: name.into(),
                architecture: std::env::consts::ARCH.into(),
                installed_at: now(),
                sha256: "a".repeat(64),
                status: "available".into(),
                last_error: None,
            },
        )
        .unwrap();
    }

    #[tokio::test]
    async fn active_and_transaction_versions_are_protected_then_all_inactive_versions_are_cleaned()
    {
        let temporary = tempfile::tempdir().unwrap();
        let (app, _) =
            crate::testsupport::fixture(temporary.path(), crate::testsupport::Outcome::Success);
        fs::remove_dir_all(&app.paths.current).unwrap();
        version(&app, "v26.10.21");
        version(&app, "v26.9.9");
        version(&app, "v26.9.30");
        app.switch_version(Some("v26.10.21")).unwrap();
        assert_eq!(app.list_versions().unwrap().len(), 3);
        assert!(matches!(
            app.delete_version("v26.10.21").await,
            Err(Error::Conflict(_))
        ));
        let operation = app
            .store
            .begin(
                "install",
                None,
                json!({"version":"v26.9.30","previous":"v26.9.9"}),
            )
            .unwrap();
        assert!(matches!(
            app.delete_version("v26.9.9").await,
            Err(Error::Conflict(_))
        ));
        app.store
            .fail(&operation.id, "fixture completed", false)
            .unwrap();
        assert!(app.cleanup_versions().unwrap().is_empty());
        let remaining = app.list_versions().unwrap();
        assert_eq!(remaining.len(), 1);
        assert!(remaining[0].active);
    }

    #[test]
    fn extraction_does_not_follow_archive_paths() {
        let temporary = tempfile::tempdir().unwrap();
        let destination = temporary.path().join("version");
        fs::create_dir(&destination).unwrap();
        let mut writer = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
        let options = zip::write::SimpleFileOptions::default();
        writer.start_file("../escaped", options).unwrap();
        writer.write_all(b"bad").unwrap();
        writer.start_file("xray", options).unwrap();
        writer.write_all(b"expected binary").unwrap();
        let bytes = writer.finish().unwrap().into_inner();
        extract(&bytes, &destination).unwrap();
        assert!(!temporary.path().join("escaped").exists());
        assert_eq!(
            fs::read(destination.join("xray")).unwrap(),
            b"expected binary"
        );
    }

    #[tokio::test]
    async fn interrupted_switch_rolls_back_and_retains_failed_candidate() {
        let temporary = tempfile::tempdir().unwrap();
        let (app, runtime) =
            crate::testsupport::fixture(temporary.path(), crate::testsupport::Outcome::Success);
        fs::remove_dir_all(&app.paths.current).unwrap();
        version(&app, "v26.9.9");
        version(&app, "v26.10.21");
        app.switch_version(Some("v26.9.9")).unwrap();
        let operation=app.store.begin("install",None,json!({"version":"v26.10.21","previous":"v26.9.9","was_active":true,"was_enabled":true})).unwrap();
        app.store.phase(&operation.id, "switching").unwrap();
        app.switch_version(Some("v26.10.21")).unwrap();
        let operation = app.store.pending().unwrap().pop().unwrap();
        app.recover_install(&operation).await.unwrap();
        assert_eq!(app.active_version().unwrap().as_deref(), Some("v26.9.9"));
        assert_eq!(
            read_manifest(&app.paths.versions.join("v26.10.21"))
                .unwrap()
                .status,
            "failed"
        );
        assert_eq!(
            runtime.actions.lock().unwrap().as_slice(),
            ["stop", "start"]
        );
        assert!(app.store.ensure_consistent().is_ok());
    }

    #[tokio::test]
    async fn committed_activation_only_cleans_history_on_recovery() {
        let temporary = tempfile::tempdir().unwrap();
        let (app, runtime) =
            crate::testsupport::fixture(temporary.path(), crate::testsupport::Outcome::Success);
        fs::remove_dir_all(&app.paths.current).unwrap();
        version(&app, "v26.9.9");
        version(&app, "v26.10.21");
        app.switch_version(Some("v26.10.21")).unwrap();
        let operation = app
            .store
            .begin(
                "install",
                None,
                json!({"version":"v26.10.21","previous":"v26.9.9"}),
            )
            .unwrap();
        app.store.phase(&operation.id, "activated").unwrap();
        let operation = app.store.pending().unwrap().pop().unwrap();
        app.recover_install(&operation).await.unwrap();
        assert_eq!(app.list_versions().unwrap().len(), 1);
        assert!(runtime.actions.lock().unwrap().is_empty());
        assert!(app.store.pending().unwrap().is_empty());
    }
}
