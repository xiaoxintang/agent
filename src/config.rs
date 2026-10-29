//! 管理 Agent 的凭据、部署路径及首次生成的 Xray 配置。
//! SQLite 保存权威配置；这里的文件写入工具负责安全地发布配置副本。

use crate::error::{Error, Result};
use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use rand::TryRngCore;
use serde::{Deserialize, Serialize};
use std::{
    fs,
    io::Write,
    net::SocketAddr,
    path::{Path, PathBuf},
};

pub const BASELINE: &str = "v26.10.21";
pub const DEFAULT_CONFIG: &str = "/etc/xray-agent/agent.toml";

// Token 只用于 HTTP 鉴权；gRPC 地址必须与 Xray 配置中的本机监听地址一致。
#[derive(Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Settings {
    pub listen: SocketAddr,
    pub token: String,
    pub grpc_listen: SocketAddr,
}

#[derive(Clone)]
pub struct Paths {
    // 数据库和启动配置独立于版本目录，升级与清理二进制时不会删除它们。
    pub database: PathBuf,
    pub config: PathBuf,
    pub versions: PathBuf,
    pub current: PathBuf,
    // 配置转换可能包含账户和私钥，工作目录仅允许 root 访问。
    pub work: PathBuf,
    pub unit: PathBuf,
}

impl Default for Paths {
    fn default() -> Self {
        Self {
            database: "/var/lib/xray-agent/state.db".into(),
            config: "/etc/xray/config.json".into(),
            versions: "/opt/xray/versions".into(),
            current: "/opt/xray/current".into(),
            work: "/var/lib/xray-agent/work".into(),
            unit: "/etc/systemd/system/xray.service".into(),
        }
    }
}

impl Paths {
    pub fn binary(&self) -> PathBuf {
        self.current.join("xray")
    }

    pub fn prepare(&self) -> Result<()> {
        for path in [
            self.database.parent().unwrap(),
            self.config.parent().unwrap(),
            &self.versions,
            &self.work,
        ] {
            fs::create_dir_all(path)?;
        }
        private_directory(self.database.parent().unwrap())?;
        private_directory(&self.work)?;
        Ok(())
    }

    #[cfg(test)]
    pub fn temporary(root: &Path) -> Self {
        // 测试使用完整的临时路径，避免写入真实的系统部署目录。
        Self {
            database: root.join("state/state.db"),
            config: root.join("etc/config.json"),
            versions: root.join("xray/versions"),
            current: root.join("xray/current"),
            work: root.join("state/work"),
            unit: root.join("systemd/xray.service"),
        }
    }
}

pub fn private_directory(path: &Path) -> Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(path, fs::Permissions::from_mode(0o700))?;
    }
    Ok(())
}

pub fn private_file(path: &Path) -> Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        // systemd 辅助命令也会读取凭据；权限已正确时不 chmod，兼容只读挂载。
        if fs::metadata(path)?.permissions().mode() & 0o777 != 0o600 {
            fs::set_permissions(path, fs::Permissions::from_mode(0o600))?;
        }
    }
    Ok(())
}

pub fn atomic_write(path: &Path, contents: &[u8]) -> Result<()> {
    let parent = path
        .parent()
        .ok_or_else(|| Error::Internal("file has no parent".into()))?;
    fs::create_dir_all(parent)?;
    // 临时文件与目标处于同一目录，确保 persist 使用同一文件系统内的原子替换。
    let mut file = tempfile::NamedTempFile::new_in(parent)?;
    file.write_all(contents)?;
    file.as_file().sync_all()?;
    file.persist(path).map_err(|error| error.error)?;
    // 文件内容和目录项分别落盘，减少中途断电后丢失替换结果的风险。
    fs::File::open(parent)?.sync_all()?;
    Ok(())
}

pub fn load(path: &Path) -> Result<Settings> {
    let settings: Settings = toml::from_str(&fs::read_to_string(path)?)
        .map_err(|error| Error::Validation(format!("invalid agent config: {error}")))?;
    if settings.token.len() < 32
        || !settings.grpc_listen.ip().is_loopback()
        || settings.grpc_listen.port() == 0
    {
        return Err(Error::Validation(
            "token must have at least 32 characters and gRPC must listen on loopback".into(),
        ));
    }
    private_file(path)?;
    Ok(settings)
}

/// Returns the new token only on first initialization. Existing credentials are never printed.
pub fn initialize(path: &Path) -> Result<Option<String>> {
    if path.exists() {
        load(path)?;
        return Ok(None);
    }
    let parent = path
        .parent()
        .ok_or_else(|| Error::BadRequest("config needs a parent directory".into()))?;
    fs::create_dir_all(parent)?;
    private_directory(parent)?;
    // 凭据来自系统安全随机源，不能使用普通伪随机数替代。
    let mut random = [0u8; 32];
    rand::rngs::OsRng
        .try_fill_bytes(&mut random)
        .map_err(|error| Error::Internal(format!("OS random source failed: {error}")))?;
    let token = URL_SAFE_NO_PAD.encode(random);
    let settings = Settings {
        listen: "0.0.0.0:8080".parse().unwrap(),
        grpc_listen: "127.0.0.1:10085".parse().unwrap(),
        token: token.clone(),
    };
    let mut file = tempfile::NamedTempFile::new_in(parent)?;
    let contents =
        toml::to_string_pretty(&settings).map_err(|error| Error::Internal(error.to_string()))?;
    file.write_all(contents.as_bytes())?;
    file.as_file().sync_all()?;
    // 并发初始化时只允许一个进程创建配置，后来者复用已经落盘的凭据。
    match file.persist_noclobber(path) {
        Ok(_) => {}
        Err(error) if error.error.kind() == std::io::ErrorKind::AlreadyExists => {
            load(path)?;
            return Ok(None);
        }
        Err(error) => return Err(error.error.into()),
    }
    fs::File::open(parent)?.sync_all()?;
    Ok(Some(token))
}

pub fn initial_xray(settings: &Settings) -> serde_json::Value {
    // 默认只启用本机管理服务和统计，不开放业务入站或指定外部探测目标。
    serde_json::json!({
        "log": {"loglevel":"warning"},
        "api": {"tag":"agent-api", "listen":settings.grpc_listen.to_string(), "services":["HandlerService","StatsService","RoutingService","LoggerService","ReflectionService"]},
        "stats": {},
        "policy": {"levels":{"0":{"statsUserUplink":true,"statsUserDownlink":true,"statsUserOnline":true}},"system":{"statsInboundUplink":true,"statsInboundDownlink":true,"statsOutboundUplink":true,"statsOutboundDownlink":true}},
        "inbounds": [], "outbounds":[{"tag":"direct","protocol":"freedom","settings":{}}],
        "routing":{"rules":[]}
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn initialization_preserves_credentials() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("agent/agent.toml");
        let token = initialize(&path).unwrap().unwrap();
        assert_eq!(URL_SAFE_NO_PAD.decode(&token).unwrap().len(), 32);
        assert!(initialize(&path).unwrap().is_none());
        assert_eq!(load(&path).unwrap().token, token);
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                fs::metadata(&path).unwrap().permissions().mode() & 0o777,
                0o600
            );
        }
    }
}
