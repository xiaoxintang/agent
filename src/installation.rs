//! Offline installation configuration. The installer stops services before invoking this.
use crate::{
    app::App,
    config,
    error::{Error, Result},
    store::Store,
    web_config,
};
use clap::Args;
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::{fs, path::Path};
const JOURNAL: &str = "/etc/xray-agent/installation.json";
#[derive(Args, Default)]
pub struct Options {
    #[arg(long,value_parser=clap::value_parser!(u16).range(1..))]
    pub agent_port: Option<u16>,
    #[arg(long,value_parser=clap::value_parser!(u16).range(1..))]
    pub grpc_port: Option<u16>,
    #[arg(long,value_parser=clap::value_parser!(u16).range(1..))]
    pub web_port: Option<u16>,
    #[arg(long,value_parser=["yes","no"])]
    pub web: Option<String>,
}
#[derive(Serialize, Deserialize)]
struct Journal {
    settings: config::Settings,
    web: web_config::Settings,
    revision: i64,
    sync_state: String,
}

pub fn recover() -> Result<()> {
    if !Path::new(JOURNAL).exists() {
        return Ok(());
    }
    let lock_path = config::Paths::default()
        .database
        .with_file_name("agent.lock");
    let lock = fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .open(lock_path)?;
    fs2::FileExt::try_lock_exclusive(&lock).map_err(|_| {
        Error::Conflict("installation configuration is being changed by another process".into())
    })?;
    recover_unlocked()
}
fn recover_unlocked() -> Result<()> {
    let path = Path::new(JOURNAL);
    if !path.exists() {
        return Ok(());
    }
    let old: Journal = serde_json::from_slice(&fs::read(path)?)?;
    let paths = config::Paths::default();
    let store = Store::open(&paths.database, &config::initial_xray(&old.settings))?;
    config::atomic_write(
        Path::new(config::DEFAULT_CONFIG),
        toml::to_string_pretty(&old.settings)
            .map_err(|e| Error::Internal(e.to_string()))?
            .as_bytes(),
    )?;
    store.restore_installation(old.revision, &old.sync_state)?;
    config::atomic_write(
        &paths.config,
        &serde_json::to_vec_pretty(&store.committed()?.config)?,
    )?;
    old.web.save(Path::new(web_config::SETTINGS))?;
    if Path::new(web_config::LOCAL_AGENT).exists() {
        web_config::sync_local(&old.settings)?;
    }
    fs::remove_file(path)?;
    fs::File::open(path.parent().unwrap())?.sync_all()?;
    Ok(())
}
pub fn defaults() -> Result<()> {
    let settings = if Path::new(config::DEFAULT_CONFIG).exists() {
        Some(config::load(Path::new(config::DEFAULT_CONFIG))?)
    } else {
        None
    };
    let web = web_config::Settings::load(Path::new(web_config::SETTINGS))?;
    println!(
        "agent_port={}\ngrpc_port={}\nweb_port={}\nweb_enabled={}",
        settings.as_ref().map_or(8080, |s| s.listen.port()),
        settings.as_ref().map_or(10085, |s| s.grpc_listen.port()),
        web.port,
        if web.enabled { "yes" } else { "no" }
    );
    Ok(())
}
pub async fn initialize(options: Options) -> Result<()> {
    let config_path = Path::new(config::DEFAULT_CONFIG);
    if config_path.exists()
        && config::Paths::default().database.exists()
        && options.agent_port.is_none()
        && options.grpc_port.is_none()
        && options.web_port.is_none()
        && options.web.is_none()
    {
        let settings = config::load(config_path)?;
        Store::open(
            &config::Paths::default().database,
            &config::initial_xray(&settings),
        )?;
        println!("Already initialized; existing credentials preserved.");
        return Ok(());
    }
    let token = config::initialize(config_path)?;
    let old = config::load(config_path)?;
    let mut settings = old.clone();
    if let Some(port) = options.agent_port {
        settings.listen.set_port(port);
    }
    if let Some(port) = options.grpc_port {
        settings.grpc_listen.set_port(port);
    }
    let old_web = web_config::Settings::load(Path::new(web_config::SETTINGS))?;
    let mut web = old_web.clone();
    if let Some(port) = options.web_port {
        web.port = port;
    }
    if let Some(enabled) = options.web {
        web.enabled = enabled == "yes";
    }
    if settings.listen.port() == settings.grpc_listen.port()
        || (web.enabled
            && (web.port == settings.listen.port() || web.port == settings.grpc_listen.port()))
    {
        return Err(Error::Validation(
            "enabled services must use different ports".into(),
        ));
    }
    let paths = config::Paths::default();
    paths.prepare()?;
    let lock = fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .open(paths.database.with_file_name("agent.lock"))?;
    fs2::FileExt::try_lock_exclusive(&lock).map_err(|_| {
        Error::Conflict("stop xray-agent.service before changing installation settings".into())
    })?;
    let app = App::open(settings.clone(), paths)?;
    let snapshot = app.store.committed()?;
    let mut candidate = snapshot.config.clone();
    candidate["api"]["listen"] = json!(settings.grpc_listen.to_string());
    if candidate != snapshot.config {
        app.store.ensure_consistent()?;
    }
    if settings.grpc_listen != old.grpc_listen && app.paths.binary().exists() {
        let status = app.system.status().await?;
        if matches!(
            status.active_state.as_str(),
            "active" | "activating" | "deactivating"
        ) {
            return Err(Error::Conflict(
                "stop xray.service before changing its management port".into(),
            ));
        }
        app.validate_management(&candidate)?;
        app.compiler
            .compile(&app.paths.binary(), &app.paths.current, &candidate)
            .await?;
    }
    let journal = Journal {
        settings: old,
        web: old_web,
        revision: snapshot.revision,
        sync_state: app.store.sync_state()?,
    };
    config::atomic_write(Path::new(JOURNAL), &serde_json::to_vec(&journal)?)?;
    let outcome: Result<()> = async {
        let operation = if candidate != snapshot.config {
            Some(app.store.begin(
                "installation",
                Some((&candidate, &snapshot.overrides)),
                json!({}),
            )?)
        } else {
            None
        };
        if let Some(op) = &operation {
            app.store.commit_candidate(op)?;
        }
        config::atomic_write(
            config_path,
            toml::to_string_pretty(&settings)
                .map_err(|e| Error::Internal(e.to_string()))?
                .as_bytes(),
        )?;
        app.materialize()?;
        if let Some(op) = &operation {
            app.store.complete(&op.id)?;
        }
        web.save(Path::new(web_config::SETTINGS))?;
        if web.enabled || Path::new(web_config::LOCAL_AGENT).exists() {
            web_config::sync_local(&settings)?;
        }
        if web.enabled {
            web_config::Credentials::create(Path::new(web_config::CREDENTIALS))?;
        }
        fs::remove_file(JOURNAL)?;
        fs::File::open(config_path.parent().unwrap())?.sync_all()?;
        Ok(())
    }
    .await;
    if let Err(error) = outcome {
        recover_unlocked()?;
        return Err(error);
    }
    if let Some(token) = token {
        println!("Bearer token (shown once): {token}");
    } else {
        println!("Existing credentials preserved.");
    }
    Ok(())
}

pub async fn verify_web(backend: &Path, frontend: &Path) -> Result<()> {
    let output = tokio::process::Command::new(backend)
        .arg("--version")
        .output()
        .await?;
    if !output.status.success()
        || String::from_utf8_lossy(&output.stdout).trim()
            != format!("agent-web {}", env!("CARGO_PKG_VERSION"))
    {
        return Err(Error::Validation(
            "Web backend must exactly match the Agent version".into(),
        ));
    }
    let version: serde_json::Value =
        serde_json::from_slice(&fs::read(frontend.join("version.json"))?)?;
    if version["version"] != env!("CARGO_PKG_VERSION") || !frontend.join("index.html").is_file() {
        return Err(Error::Validation(
            "Web frontend must exactly match the Agent version".into(),
        ));
    }
    Ok(())
}
pub async fn health() -> Result<()> {
    let settings = config::load(Path::new(config::DEFAULT_CONFIG))?;
    let web = web_config::Settings::load(Path::new(web_config::SETTINGS))?;
    let client = reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .timeout(std::time::Duration::from_secs(2))
        .build()
        .map_err(|e| Error::Internal(e.to_string()))?;
    let ip = if settings.listen.ip().is_unspecified() {
        if settings.listen.is_ipv6() {
            "[::1]".to_owned()
        } else {
            "127.0.0.1".to_owned()
        }
    } else if settings.listen.is_ipv6() {
        format!("[{}]", settings.listen.ip())
    } else {
        settings.listen.ip().to_string()
    };
    for _ in 0..30 {
        let agent_ok = client
            .get(format!(
                "http://{ip}:{}/api/v1/xray/status",
                settings.listen.port()
            ))
            .bearer_auth(&settings.token)
            .send()
            .await
            .is_ok_and(|r| r.status().is_success());
        let web_ok = if web.enabled {
            match client
                .get(format!("http://127.0.0.1:{}/api/meta", web.port))
                .send()
                .await
            {
                Ok(response) if response.status().is_success() => response
                    .json::<serde_json::Value>()
                    .await
                    .is_ok_and(|v| v["version"] == env!("CARGO_PKG_VERSION")),
                _ => false,
            }
        } else {
            true
        };
        if agent_ok && web_ok {
            return Ok(());
        }
        tokio::time::sleep(std::time::Duration::from_secs(1)).await;
    }
    Err(Error::Unavailable(
        "installed services did not become ready; rolling back".into(),
    ))
}

/// SQLite's backup API includes committed WAL contents in one consistent file.
pub fn backup_state(output: &Path) -> Result<()> {
    let source = rusqlite::Connection::open_with_flags(
        config::Paths::default().database,
        rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
    )?;
    source.busy_timeout(std::time::Duration::from_secs(5))?;
    source.backup(rusqlite::MAIN_DB, output, None)?;
    config::private_file(output)?;
    fs::File::open(output)?.sync_all()?;
    if let Some(parent) = output.parent() {
        fs::File::open(parent)?.sync_all()?;
    }
    Ok(())
}
