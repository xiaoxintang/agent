use crate::{
    bundle::{self, Bundle},
    cli::{Component, Input, Settings},
    platform::Result,
    xray::lifecycle,
};
use serde::{Deserialize, Serialize};
use std::{
    collections::BTreeMap,
    fs,
    path::{Path, PathBuf},
    time::Duration,
};
use tokio::{io::AsyncWriteExt, process::Command};

const CONTROL: &str = "/var/lib/panel-installer";
const MANIFEST: &str = "/var/lib/panel-installer/manifest.json";
const AGENT_DATA: &str = "/var/lib/panel/agent";
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Account {
    uid: u32,
    gid: u32,
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Manifest {
    format: u8,
    id: String,
    settings: Settings,
    files: BTreeMap<PathBuf, String>,
    accounts: BTreeMap<String, Account>,
    versions: BTreeMap<Component, String>,
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Backup {
    path: PathBuf,
    existed: bool,
    mode: u32,
    uid: u32,
    gid: u32,
    index: usize,
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Journal {
    previous: Manifest,
    files: Vec<Backup>,
    active: Vec<String>,
    enabled: Vec<String>,
    component: Component,
}
fn io(error: panel_common::XrayError) -> Box<dyn std::error::Error> {
    format!("Owned Xray storage/lifecycle failed: {error:?}").into()
}
fn atomic(path: &Path, bytes: &[u8], mode: u32) -> Result<()> {
    lifecycle::atomic(path, bytes, mode).map_err(io)
}
fn directory(path: &Path, mode: u32) -> Result<()> {
    if !path.exists() {
        fs::create_dir(path)?;
        lifecycle::mode(path, mode).map_err(io)?;
    }
    bundle::secure(path, mode == 0o700)?;
    Ok(())
}
fn store(state: &Manifest) -> Result<()> {
    atomic(Path::new(MANIFEST), &serde_json::to_vec(state)?, 0o600)
}
fn load() -> Result<Option<Manifest>> {
    if !Path::new(MANIFEST).exists() {
        return Ok(None);
    }
    let state: Manifest = serde_json::from_slice(&bundle::read(Path::new(MANIFEST), 65536, true)?)?;
    if state.format != 1
        || uuid::Uuid::parse_str(&state.id).is_err()
        || state.versions.keys().any(|role| *role == Component::All)
        || state.versions.values().any(|version| {
            version.is_empty()
                || !version
                    .bytes()
                    .all(|byte| byte.is_ascii_digit() || b".-".contains(&byte))
        })
    {
        return Err("Invalid ownership manifest".into());
    }
    validate(
        &state.settings,
        if state.versions.contains_key(&Component::Server)
            && state.versions.contains_key(&Component::Agent)
        {
            Component::All
        } else if state.versions.contains_key(&Component::Agent) {
            Component::Agent
        } else {
            Component::Server
        },
    )?;
    for path in state.files.keys() {
        if !managed_path(path) {
            return Err("Manifest contains a path outside the fixed deployment".into());
        }
    }
    for (name, account) in &state.accounts {
        if !matches!(name.as_str(), "panel-server" | "panel-agent")
            || account.uid == 0
            || account.gid == 0
        {
            return Err("Invalid service account manifest".into());
        }
    }
    Ok(Some(state))
}
fn managed_path(path: &Path) -> bool {
    let Some(path) = path.to_str() else {
        return false;
    };
    matches!(
        path,
        "/usr/local/bin/panel-server"
            | "/usr/local/bin/panel-agent"
            | "/usr/local/bin/panel-installer"
            | "/etc/panel/server.env"
            | "/etc/panel/server-cert.pem"
            | "/etc/panel/server-key.pem"
            | "/etc/panel/agent-ca.pem"
            | "/etc/panel/xray-helper.json"
            | "/etc/systemd/system/panel-server.service"
            | "/etc/systemd/system/panel-agent.service"
            | "/etc/systemd/system/panel-xray-helper.socket"
            | "/etc/systemd/system/panel-xray-helper@.service"
    )
}
pub fn validate(settings: &Settings, component: Component) -> Result<()> {
    let data = &settings.server_data;
    if !data.is_absolute()
        || !data.starts_with("/var/lib/panel")
        || data == Path::new("/var/lib/panel")
        || data == Path::new(AGENT_DATA)
        || data.components().any(|part| {
            !matches!(
                part,
                std::path::Component::RootDir | std::path::Component::Normal(_)
            )
        })
        || data.to_str().is_none_or(|text| {
            !text
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || b"/_-".contains(&byte))
        })
        || data.components().count() != 5
    {
        return Err("Server data must be one named directory directly under /var/lib/panel (excluding agent)".into());
    }
    if component.server() {
        let bind: std::net::SocketAddr = settings.server_bind.parse()?;
        if settings
            .server_origin
            .chars()
            .any(|ch| ch.is_whitespace() || ch.is_control() || matches!(ch, '"' | '\\' | '%'))
        {
            return Err("Invalid characters in server origin".into());
        }
        let origin = url::Url::parse(&settings.server_origin)?;
        if !origin.username().is_empty()
            || origin.password().is_some()
            || origin.query().is_some()
            || origin.fragment().is_some()
            || origin.path() != "/"
            || settings.server_origin.ends_with('/')
            || origin.host_str().is_none()
        {
            return Err("Invalid server origin".into());
        }
        if settings.local_http {
            if !bind.ip().is_loopback()
                || origin.scheme() != "http"
                || origin
                    .host_str()
                    .and_then(|host| {
                        host.trim_matches(['[', ']'])
                            .parse::<std::net::IpAddr>()
                            .ok()
                    })
                    .is_none_or(|ip| !ip.is_loopback())
                || settings.tls_key.is_some()
                || settings.tls_cert.is_some()
            {
                return Err("Plain HTTP must be explicitly local-only, without TLS paths".into());
            }
        } else if origin.scheme() != "https"
            || settings.tls_key.is_none()
            || settings.tls_cert.is_none()
        {
            return Err("Public installation requires HTTPS and certificate/key".into());
        }
        if settings.admin_username.is_empty()
            || settings.admin_username.len() > 32
            || !settings
                .admin_username
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || b"_-".contains(&byte))
        {
            return Err("Invalid administrator username".into());
        }
    }
    if component.agent() {
        let url = url::Url::parse(&settings.agent_server)?;
        if !url.username().is_empty()
            || url.password().is_some()
            || url.query().is_some()
            || url.fragment().is_some()
            || url.path() != "/agent/v1/connect"
            || url.host_str().is_none()
        {
            return Err("Invalid Agent URL".into());
        }
        if url.scheme() != "wss"
            && !(settings.local_http
                && url.scheme() == "ws"
                && url
                    .host_str()
                    .and_then(|host| {
                        host.trim_matches(['[', ']'])
                            .parse::<std::net::IpAddr>()
                            .ok()
                    })
                    .is_some_and(|ip| ip.is_loopback()))
        {
            return Err("Agent requires WSS, or explicit literal loopback development WS".into());
        }
        if settings.agent_name.is_empty()
            || settings.agent_name.chars().count() > 64
            || settings.agent_name.chars().any(char::is_control)
        {
            return Err("Invalid node name".into());
        }
    }
    Ok(())
}
fn cmd(path: &str) -> Command {
    let mut command = Command::new(path);
    command
        .env_clear()
        .env("PATH", "/usr/sbin:/usr/bin:/sbin:/bin")
        .env("LC_ALL", "C")
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .kill_on_drop(true);
    command
}
async fn status(mut command: Command) -> Result<()> {
    if !tokio::time::timeout(Duration::from_secs(40), command.status())
        .await??
        .success()
    {
        return Err("Fixed installer subprocess failed (arguments and secrets redacted)".into());
    }
    Ok(())
}
async fn system(action: &str, unit: &str) -> Result<()> {
    if !matches!(
        action,
        "start" | "stop" | "restart" | "enable" | "disable" | "reset-failed"
    ) || !matches!(
        unit,
        "panel-server.service"
            | "panel-agent.service"
            | "panel-xray-helper.socket"
            | "panel-xray-helper@*.service"
    ) {
        return Err("Unsupported systemd operation".into());
    }
    let mut command = cmd("/usr/bin/systemctl");
    command.args([action, unit]);
    status(command).await
}
async fn reload() -> Result<()> {
    let mut command = cmd("/usr/bin/systemctl");
    command.arg("daemon-reload");
    status(command).await
}
async fn active(unit: &str) -> bool {
    let mut command = cmd("/usr/bin/systemctl");
    command.args(["is-active", "--quiet", unit]);
    matches!(tokio::time::timeout(Duration::from_secs(5),command.status()).await,Ok(Ok(status)) if status.success())
}
async fn lookup(name: &str) -> Result<Option<Account>> {
    if !matches!(name, "panel-server" | "panel-agent") {
        return Err("Unknown service account".into());
    }
    let mut values = Vec::new();
    for flag in ["-u", "-g"] {
        let mut command = cmd("/usr/bin/id");
        command
            .args([flag, name])
            .stdout(std::process::Stdio::piped());
        let output = command.output().await?;
        if !output.status.success() {
            return Ok(None);
        }
        values.push(String::from_utf8(output.stdout)?.trim().parse()?);
    }
    Ok(Some(Account {
        uid: values[0],
        gid: values[1],
    }))
}
async fn account(state: &mut Manifest, name: &str) -> Result<Account> {
    match (state.accounts.get(name), lookup(name).await?) {
        (Some(saved), Some(actual)) if saved.uid == actual.uid && saved.gid == actual.gid => {
            return Ok(actual);
        }
        (None, None) => (),
        _ => return Err("Existing or changed unowned service account refused".into()),
    }
    let mut command = cmd("/usr/sbin/useradd");
    command.args([
        "--system",
        "--user-group",
        "--no-create-home",
        "--home-dir",
        "/nonexistent",
        "--shell",
        "/usr/sbin/nologin",
        name,
    ]);
    status(command).await?;
    let created = lookup(name)
        .await?
        .ok_or("Service account creation failed")?;
    state.accounts.insert(name.into(), created.clone());
    store(state)?;
    Ok(created)
}
fn chown(path: &Path, account: &Account) -> Result<()> {
    #[cfg(unix)]
    std::os::unix::fs::chown(path, Some(account.uid), Some(account.gid))?;
    Ok(())
}
fn data_directory(path: &Path, id: &str, account: &Account) -> Result<()> {
    if path.exists() {
        let metadata = fs::symlink_metadata(path)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::MetadataExt;
            if metadata.uid() != account.uid
                || metadata.mode() & 0o077 != 0
                || metadata.file_type().is_symlink()
            {
                return Err("Unowned data directory refused".into());
            }
        }
        if bundle::read_regular(&path.join(".panel-owned"), 128, 0)? != id.as_bytes() {
            return Err("Data ownership marker mismatch".into());
        }
    } else {
        fs::create_dir(path)?;
        lifecycle::mode(path, 0o700).map_err(io)?;
        atomic(&path.join(".panel-owned"), id.as_bytes(), 0o600)?;
        chown(path, account)?;
    }
    Ok(())
}
fn verify_files(state: &Manifest) -> Result<()> {
    for (path, digest) in &state.files {
        if lifecycle::hash(&bundle::read(path, 128 * 1024 * 1024, false)?) != *digest {
            return Err(format!(
                "Owned file changed; inspect before explicit repair: {}",
                path.display()
            )
            .into());
        }
    }
    Ok(())
}
fn write(state: &mut Manifest, path: &str, bytes: &[u8], mode: u32) -> Result<()> {
    let path = Path::new(path);
    if path.exists() && !state.files.contains_key(path) {
        return Err(format!("Unowned destination refused: {}", path.display()).into());
    }
    atomic(path, bytes, mode)?;
    state.files.insert(path.into(), lifecycle::hash(bytes));
    Ok(())
}
fn agent_args(settings: &Settings) -> Vec<String> {
    let mut args = vec!["--data-dir".into(), AGENT_DATA.into()];
    if settings.agent_ca.is_some() {
        args.extend(["--ca-cert".into(), "/etc/panel/agent-ca.pem".into()]);
    }
    if settings.local_http && settings.agent_server.starts_with("ws:") {
        args.push("--allow-local-ws".into());
    }
    args
}
fn server_unit(settings: &Settings) -> String {
    let mut unit = include_str!("../../../deploy/systemd/panel-server.service").replace(
        "/var/lib/panel/server",
        settings.server_data.to_str().expect("validated data path"),
    );
    if settings
        .server_bind
        .parse::<std::net::SocketAddr>()
        .is_ok_and(|bind| bind.port() >= 1024)
    {
        unit = unit.replace("CAP_NET_BIND_SERVICE", "");
    }
    if settings.local_http {
        unit = unit
            .lines()
            .filter(|line| {
                !line.starts_with("LoadCredential=")
                    && !line.starts_with("ExecStartPre=")
                    && !line.starts_with("Environment=PANEL_TLS_")
            })
            .collect::<Vec<_>>()
            .join("\n")
            + "\n";
    }
    unit
}

fn agent_unit(settings: &Settings) -> String {
    let mut unit = include_str!("../../../deploy/systemd/panel-agent.service").to_owned();
    let command = format!(
        "ExecStart=/usr/local/bin/panel-agent {} run",
        agent_args(settings).join(" ")
    );
    unit = unit.replace(
        "ExecStart=/usr/local/bin/panel-agent --data-dir /var/lib/panel/agent run",
        &command,
    );
    unit
}
async fn foreign_units(state: &Manifest, component: Component) -> Result<()> {
    for unit in [
        "panel-server.service",
        "panel-agent.service",
        "panel-xray-helper.socket",
        "panel-xray-helper@.service",
    ] {
        if (unit == "panel-server.service" && !component.server())
            || (unit != "panel-server.service" && !component.agent())
        {
            continue;
        }
        for base in ["/etc/systemd/system", "/run/systemd/system"] {
            if Path::new(base).join(format!("{unit}.d")).exists() {
                return Err("Unowned systemd drop-ins refused".into());
            }
        }
        let mut command = cmd("/usr/bin/systemctl");
        command
            .args(["show", unit, "--property=FragmentPath", "--value"])
            .stdout(std::process::Stdio::piped());
        let output = command.output().await?;
        let path = String::from_utf8(output.stdout)?;
        let path = path.trim();
        if !path.is_empty() && !state.files.contains_key(Path::new(path)) {
            return Err("Pre-existing unowned systemd unit refused".into());
        }
    }
    Ok(())
}
fn file_paths(state: &Manifest) -> Vec<PathBuf> {
    let mut paths = state.files.keys().cloned().collect::<Vec<_>>();
    for path in [
        "/usr/local/bin/panel-server",
        "/usr/local/bin/panel-agent",
        "/usr/local/bin/panel-installer",
        "/etc/panel/server.env",
        "/etc/panel/server-cert.pem",
        "/etc/panel/server-key.pem",
        "/etc/panel/agent-ca.pem",
        "/etc/panel/xray-helper.json",
        "/etc/systemd/system/panel-server.service",
        "/etc/systemd/system/panel-agent.service",
        "/etc/systemd/system/panel-xray-helper.socket",
        "/etc/systemd/system/panel-xray-helper@.service",
    ] {
        paths.push(path.into());
    }
    for (directory, name) in [
        (&state.settings.server_data, "panel.sqlite3"),
        (&PathBuf::from(AGENT_DATA), "agent.sqlite3"),
    ] {
        for suffix in ["", "-wal", "-shm"] {
            paths.push(directory.join(format!("{name}{suffix}")));
        }
    }
    paths.sort();
    paths.dedup();
    paths
}
fn selected_path(path: &Path, settings: &Settings, component: Component) -> bool {
    let value = path.to_str().unwrap_or("");
    value == "/usr/local/bin/panel-installer"
        || (component.server()
            && (path.starts_with(&settings.server_data)
                || value.contains("panel-server")
                || value.ends_with("server.env")
                || value.ends_with("server-cert.pem")
                || value.ends_with("server-key.pem")))
        || (component.agent()
            && (path.starts_with(AGENT_DATA)
                || value.contains("panel-agent")
                || value.contains("panel-xray-helper")
                || value.ends_with("agent-ca.pem")
                || value.ends_with("xray-helper.json")))
}
async fn checkpoint(state: &Manifest, component: Component) -> Result<()> {
    let mut live = Vec::new();
    let mut enabled = Vec::new();
    for unit in [
        "panel-server.service",
        "panel-agent.service",
        "panel-xray-helper.socket",
    ] {
        if (unit == "panel-server.service" && !component.server())
            || (unit != "panel-server.service" && !component.agent())
        {
            continue;
        }
        if active(unit).await {
            live.push(unit.into());
        }
        let mut command = cmd("/usr/bin/systemctl");
        command.args(["is-enabled", "--quiet", unit]);
        if command.status().await?.success() {
            enabled.push(unit.into());
        }
        let _ = system("stop", unit).await;
    }
    if component.agent() {
        let _ = system("stop", "panel-xray-helper@*.service").await;
    }
    let backup = Path::new(CONTROL).join("transaction");
    directory(&backup, 0o700)?;
    let mut files = Vec::new();
    for (index, path) in file_paths(state).into_iter().enumerate() {
        if !selected_path(&path, &state.settings, component) {
            continue;
        }
        let mut entry = Backup {
            path: path.clone(),
            existed: false,
            mode: 0o600,
            uid: 0,
            gid: 0,
            index,
        };
        if let Ok(metadata) = fs::symlink_metadata(&path) {
            if !metadata.is_file() || metadata.file_type().is_symlink() {
                return Err("Refusing nonregular transaction path".into());
            }
            entry.existed = true;
            #[cfg(unix)]
            {
                use std::os::unix::fs::MetadataExt;
                entry.mode = metadata.mode() & 0o777;
                entry.uid = metadata.uid();
                entry.gid = metadata.gid();
            }
            let destination = backup.join(index.to_string());
            fs::copy(&path, &destination)?;
            lifecycle::mode(&destination, 0o600).map_err(io)?;
            fs::File::open(destination)?.sync_all()?;
        }
        files.push(entry);
    }
    fs::File::open(&backup)?.sync_all()?;
    atomic(
        &Path::new(CONTROL).join("journal.json"),
        &serde_json::to_vec(&Journal {
            previous: state.clone(),
            files,
            active: live,
            enabled,
            component,
        })?,
        0o600,
    )
}
async fn recover() -> Result<bool> {
    let path = Path::new(CONTROL).join("journal.json");
    if !path.exists() {
        return Ok(false);
    }
    let journal: Journal = serde_json::from_slice(&bundle::read(&path, 65536, true)?)?;
    validate(&journal.previous.settings, journal.component)?;
    let allowed = file_paths(&journal.previous);
    if journal
        .files
        .iter()
        .any(|entry| !allowed.contains(&entry.path))
    {
        return Err("Invalid recovery inventory".into());
    }
    for unit in [
        "panel-server.service",
        "panel-agent.service",
        "panel-xray-helper.socket",
    ] {
        if (unit == "panel-server.service" && !journal.component.server())
            || (unit != "panel-server.service" && !journal.component.agent())
        {
            continue;
        }
        let _ = system("stop", unit).await;
        let _ = system("disable", unit).await;
    }
    for entry in &journal.files {
        if !allowed.contains(&entry.path) {
            return Err("Invalid recovery path".into());
        }
        if entry.existed {
            let source = Path::new(CONTROL)
                .join("transaction")
                .join(entry.index.to_string());
            bundle::secure(&source, true)?;
            let parent = entry.path.parent().ok_or("Invalid recovery parent")?;
            let temp = tempfile::NamedTempFile::new_in(parent)?;
            fs::copy(source, temp.path())?;
            lifecycle::mode(temp.path(), entry.mode).map_err(io)?;
            chown(
                temp.path(),
                &Account {
                    uid: entry.uid,
                    gid: entry.gid,
                },
            )?;
            temp.as_file().sync_all()?;
            temp.persist(&entry.path)?;
        } else if entry.path.exists() {
            fs::remove_file(&entry.path)?;
        }
    }
    let mut previous = journal.previous.clone();
    if let Some(current) = load()? {
        if current.id != previous.id {
            return Err("Recovery ownership mismatch".into());
        }
        for (name, account) in current.accounts {
            previous.accounts.entry(name).or_insert(account);
        }
    }
    store(&previous)?;
    reload().await?;
    for unit in &journal.enabled {
        system("enable", unit).await?;
    }
    for unit in &journal.active {
        // An inactive unit may have been unloaded during daemon-reload.
        let _ = system("reset-failed", unit).await;
        system("start", unit).await?;
    }
    finish_transaction()?;
    Ok(true)
}
fn finish_transaction() -> Result<()> {
    let path = Path::new(CONTROL).join("journal.json");
    if path.exists() {
        fs::remove_file(path)?;
    }
    let backup = Path::new(CONTROL).join("transaction");
    if backup.exists() {
        fs::remove_dir_all(backup)?;
    }
    Ok(())
}
fn lock() -> Result<fs::File> {
    directory(Path::new(CONTROL), 0o700)?;
    let lock = lifecycle::private_file(&Path::new(CONTROL).join("installer.lock")).map_err(io)?;
    lock.try_lock()?;
    Ok(lock)
}
async fn initialize(state: &Manifest, input: &Input) -> Result<()> {
    if !state.settings.server_data.join("panel.sqlite3").exists() {
        let password = input
            .admin_password
            .as_ref()
            .ok_or("First server install requires an admin password in protected config or TTY")?;
        if !(12..=128).contains(&password.len()) {
            return Err("Administrator password must be 12-128 bytes".into());
        }
        let account = state
            .accounts
            .get("panel-server")
            .ok_or("Missing service account")?;
        let mut command = cmd("/usr/local/bin/panel-server");
        command
            .env("PANEL_DATA_DIR", &state.settings.server_data)
            .args([
                "init-admin",
                "--username",
                &state.settings.admin_username,
                "--password-stdin",
            ])
            .stdin(std::process::Stdio::piped());
        #[cfg(unix)]
        {
            command.uid(account.uid).gid(account.gid);
        }
        let mut child = command.spawn()?;
        let mut pipe = child.stdin.take().ok_or("Missing password pipe")?;
        pipe.write_all(password.as_bytes()).await?;
        pipe.shutdown().await?;
        drop(pipe);
        if !tokio::time::timeout(Duration::from_secs(30), child.wait())
            .await??
            .success()
        {
            return Err("Administrator initialization failed; credentials redacted".into());
        }
    }
    Ok(())
}
async fn request(
    client: &reqwest::Client,
    method: reqwest::Method,
    url: &str,
    body: serde_json::Value,
    session: Option<(&str, &str)>,
) -> Result<(reqwest::header::HeaderMap, serde_json::Value)> {
    let parsed = url::Url::parse(url)?;
    let origin = parsed.origin().ascii_serialization();
    let mut request = client
        .request(method, url)
        .header("Origin", origin)
        .json(&body);
    if let Some((cookie, csrf)) = session {
        request = request
            .header("Cookie", cookie)
            .header("X-CSRF-Token", csrf);
    }
    let response = request.send().await?;
    if !response.status().is_success() {
        return Err("Authenticated setup API failed; credentials redacted".into());
    }
    let headers = response.headers().clone();
    let mut response = response;
    let mut bytes = Vec::new();
    while let Some(chunk) = response.chunk().await? {
        if bytes.len() + chunk.len() > 65536 {
            return Err("Setup response exceeds limit".into());
        }
        bytes.extend_from_slice(&chunk);
    }
    Ok((headers, serde_json::from_slice(&bytes)?))
}
async fn client(settings: &Settings) -> Result<reqwest::Client> {
    let mut builder = reqwest::Client::builder()
        .no_proxy()
        .redirect(reqwest::redirect::Policy::none())
        .timeout(Duration::from_secs(10));
    if let Some(path) = &settings.agent_ca {
        builder = builder.add_root_certificate(reqwest::Certificate::from_pem(&bundle::read(
            path, 65536, false,
        )?)?);
    } else if let Some(path) = &settings.tls_cert {
        builder = builder.add_root_certificate(reqwest::Certificate::from_pem(&bundle::read(
            path, 65536, false,
        )?)?);
    }
    Ok(builder.build()?)
}
async fn enrollment(state: &Manifest, input: &Input) -> Result<String> {
    if let Some(token) = &input.enrollment_token {
        return Ok(token.clone());
    }
    if !state.versions.contains_key(&Component::Server) {
        return Err("Agent-only install requires a one-time enrollment token".into());
    }
    let password = input
        .admin_password
        .as_ref()
        .ok_or("Local automatic enrollment requires the administrator password")?;
    let client = client(&state.settings).await?;
    let origin = &state.settings.server_origin;
    let (headers, session) = request(
        &client,
        reqwest::Method::POST,
        &format!("{origin}/api/v1/auth/login"),
        serde_json::json!({"username":state.settings.admin_username,"password":password}),
        None,
    )
    .await?;
    let cookie = headers
        .get("set-cookie")
        .ok_or("Missing setup session")?
        .to_str()?
        .split(';')
        .next()
        .ok_or("Missing setup cookie")?;
    let csrf = session["csrf_token"].as_str().ok_or("Missing setup CSRF")?;
    let (_, node) = request(
        &client,
        reqwest::Method::POST,
        &format!("{origin}/api/v1/nodes"),
        serde_json::json!({"name":state.settings.agent_name}),
        Some((cookie, csrf)),
    )
    .await?;
    let (_, token) = request(
        &client,
        reqwest::Method::POST,
        &format!("{origin}/api/v1/nodes/enrollments"),
        serde_json::json!({"node_id":node["id"]}),
        Some((cookie, csrf)),
    )
    .await?;
    let _ = client
        .post(format!("{origin}/api/v1/auth/logout"))
        .header("Origin", origin)
        .header("Cookie", cookie)
        .header("X-CSRF-Token", csrf)
        .send()
        .await;
    Ok(token["token"]
        .as_str()
        .ok_or("Missing enrollment token")?
        .into())
}
async fn enroll(state: &Manifest, input: &Input) -> Result<()> {
    if Path::new(AGENT_DATA).join("agent.sqlite3").exists() && input.enrollment_token.is_none() {
        return Ok(());
    }
    let token = enrollment(state, input).await?;
    let account = state
        .accounts
        .get("panel-agent")
        .ok_or("Missing Agent account")?;
    let mut command = cmd("/usr/local/bin/panel-agent");
    command
        .args(agent_args(&state.settings))
        .args([
            "enroll",
            "--server",
            &state.settings.agent_server,
            "--token-stdin",
        ])
        .stdin(std::process::Stdio::piped());
    #[cfg(unix)]
    {
        command.uid(account.uid).gid(account.gid);
    }
    let mut child = command.spawn()?;
    let mut pipe = child.stdin.take().ok_or("Missing enrollment pipe")?;
    pipe.write_all(token.as_bytes()).await?;
    pipe.shutdown().await?;
    drop(pipe);
    if !tokio::time::timeout(Duration::from_secs(35), child.wait())
        .await??
        .success()
    {
        return Err("Agent enrollment failed; token redacted".into());
    }
    Ok(())
}
async fn health(state: &Manifest, component: Component) -> Result<()> {
    for (enabled, unit) in [
        (component.server(), "panel-server.service"),
        (component.agent(), "panel-agent.service"),
    ] {
        if enabled {
            tokio::time::sleep(Duration::from_millis(700)).await;
            if !active(unit).await {
                return Err(format!("Owned service health check failed: {unit}").into());
            }
        }
    }
    if component.server() {
        let response = client(&state.settings)
            .await?
            .get(format!("{}/api/v1/health", state.settings.server_origin))
            .send()
            .await?;
        if !response.status().is_success() {
            return Err("Server HTTPS/HTTP health check failed".into());
        }
    }
    Ok(())
}

async fn cache(bundle: &Bundle) -> Result<PathBuf> {
    let path = bundle::cache_path(&bundle.metadata.version);
    directory(&path, 0o700)?;
    let metadata = serde_json::to_vec(&bundle.metadata)?;
    if path.join("bundle.json").exists()
        && bundle::read(&path.join("bundle.json"), 8192, false)? != metadata
    {
        return Err("A different build already owns this version cache".into());
    }
    for (name, bytes) in &bundle.bytes {
        let artifact = path.join(name);
        if artifact.exists() {
            if lifecycle::hash(&bundle::read(&artifact, 128 * 1024 * 1024, false)?)
                != bundle.metadata.files[name]
            {
                return Err("Cached artifact was modified".into());
            }
        } else {
            atomic(&artifact, bytes, 0o700)?;
        }
        let mut command = cmd(artifact.to_str().ok_or("Invalid cache path")?);
        command
            .arg("--version")
            .stdout(std::process::Stdio::piped());
        let output = tokio::time::timeout(Duration::from_secs(10), command.output()).await??;
        if !output.status.success()
            || String::from_utf8(output.stdout)?.trim()
                != format!("{name} {}", bundle.metadata.version)
        {
            return Err("Verified artifact version smoke check failed".into());
        }
    }
    atomic(&path.join("bundle.json"), &metadata, 0o600)?;
    Ok(path)
}
fn check_destinations(state: &Manifest) -> Result<()> {
    for path in file_paths(state) {
        if managed_path(&path) {
            for parent in path.ancestors().skip(1) {
                if parent.exists() {
                    bundle::secure(parent, false).map_err(|_| {
                        format!(
                            "Managed destination has an untrusted parent: {}",
                            parent.display()
                        )
                    })?;
                }
            }
        }
        if managed_path(&path)
            && fs::symlink_metadata(&path).is_ok()
            && !state.files.contains_key(&path)
        {
            return Err(format!(
                "Pre-existing unowned destination refused: {}",
                path.display()
            )
            .into());
        }
    }
    Ok(())
}
async fn activate(
    state: &mut Manifest,
    input: &Input,
    component: Component,
    bundle: &Bundle,
    initial: bool,
) -> Result<()> {
    let mut saved = input.settings.clone();
    // Never retain source paths or credentials from a removable bootstrap directory.
    if component.server() {
        let account = account(state, "panel-server").await?;
        data_directory(&saved.server_data, &state.id, &account)?;
        write(
            state,
            "/usr/local/bin/panel-server",
            &bundle.bytes["panel-server"],
            0o755,
        )?;
        let environment = format!(
            "PANEL_BIND={}\nPANEL_ORIGIN={}\nPANEL_DATA_DIR={}\n{}",
            saved.server_bind,
            saved.server_origin,
            saved.server_data.display(),
            if saved.local_http {
                "PANEL_DEV_HTTP=1\n"
            } else {
                ""
            }
        );
        write(
            state,
            "/etc/panel/server.env",
            environment.as_bytes(),
            0o600,
        )?;
        if !saved.local_http {
            let cert = bundle::read(
                saved.tls_cert.as_deref().ok_or("Missing TLS cert")?,
                65536,
                false,
            )?;
            let key = bundle::read(
                saved.tls_key.as_deref().ok_or("Missing TLS key")?,
                65536,
                true,
            )?;
            write(state, "/etc/panel/server-cert.pem", &cert, 0o600)?;
            write(state, "/etc/panel/server-key.pem", &key, 0o600)?;
            saved.tls_cert = Some("/etc/panel/server-cert.pem".into());
            saved.tls_key = Some("/etc/panel/server-key.pem".into());
        }
        write(
            state,
            "/etc/systemd/system/panel-server.service",
            server_unit(&saved).as_bytes(),
            0o644,
        )?;
        state.settings = saved.clone();
        state
            .versions
            .insert(Component::Server, bundle.metadata.version.clone());
        if initial {
            initialize(state, input).await?;
        }
    }
    if component.agent() {
        let account = account(state, "panel-agent").await?;
        data_directory(Path::new(AGENT_DATA), &state.id, &account)?;
        write(
            state,
            "/usr/local/bin/panel-agent",
            &bundle.bytes["panel-agent"],
            0o755,
        )?;
        if let Some(path) = &saved.agent_ca {
            let ca = bundle::read(path, 65536, false)?;
            write(state, "/etc/panel/agent-ca.pem", &ca, 0o644)?;
            saved.agent_ca = Some("/etc/panel/agent-ca.pem".into());
        }
        write(
            state,
            "/etc/panel/xray-helper.json",
            &serde_json::to_vec(&serde_json::json!({"agent_uid":account.uid}))?,
            0o600,
        )?;
        write(
            state,
            "/etc/systemd/system/panel-agent.service",
            agent_unit(&saved).as_bytes(),
            0o644,
        )?;
        write(
            state,
            "/etc/systemd/system/panel-xray-helper.socket",
            include_bytes!("../../../deploy/systemd/panel-xray-helper.socket"),
            0o644,
        )?;
        write(
            state,
            "/etc/systemd/system/panel-xray-helper@.service",
            include_bytes!("../../../deploy/systemd/panel-xray-helper@.service"),
            0o644,
        )?;
        state.settings = saved.clone();
        state
            .versions
            .insert(Component::Agent, bundle.metadata.version.clone());
    }
    // First install needs the helper executable before starting socket activation.
    write(
        state,
        "/usr/local/bin/panel-installer",
        &bundle.bytes["panel-installer"],
        0o755,
    )?;
    reload().await?;
    if component.server() {
        let _ = system("reset-failed", "panel-server.service").await;
        system("start", "panel-server.service").await?;
        health(state, Component::Server).await?;
    }
    if component.agent() {
        if initial {
            enroll(state, input).await?;
        }
        system("start", "panel-xray-helper.socket").await?;
        let _ = system("reset-failed", "panel-agent.service").await;
        system("start", "panel-agent.service").await?;
    }
    health(state, component).await?;
    if saved.install_xray && component.agent() {
        let manager = lifecycle::Manager {
            layout: lifecycle::Layout::production(),
            service: lifecycle::Systemd::production(),
            user_scope: false,
        };
        let observed = manager
            .operate(panel_common::XrayOperation::Status)
            .await
            .map_err(io)?;
        if !observed.installed {
            manager
                .operate(panel_common::XrayOperation::Install(
                    panel_common::XrayRelease {
                        version: panel_common::XrayVersion::V26_10_31,
                    },
                ))
                .await
                .map_err(io)?;
        }
    }
    if saved.autostart {
        if component.server() {
            system("enable", "panel-server.service").await?;
        }
        if component.agent() {
            system("enable", "panel-agent.service").await?;
            system("enable", "panel-xray-helper.socket").await?;
        }
    }
    Ok(())
}
pub async fn install(mut input: Input, component: Component, bundle: Bundle) -> Result<()> {
    crate::platform::detect()?;
    validate(&input.settings, component)?;
    if input
        .admin_password
        .as_ref()
        .is_some_and(|password| !(12..=128).contains(&password.len()))
    {
        return Err("Administrator password must be 12-128 bytes".into());
    }
    if input.enrollment_token.as_ref().is_some_and(|token| {
        token.len() != 64 || !token.bytes().all(|byte| byte.is_ascii_hexdigit())
    }) {
        return Err("Invalid one-time enrollment token".into());
    }
    directory(Path::new("/var/lib/panel"), 0o755)?;
    directory(Path::new("/etc/panel"), 0o755)?;
    let _lock = lock()?;
    if recover().await? {
        return Err(
            "An interrupted transaction was restored; inspect status before retrying".into(),
        );
    }
    let mut state = load()?.unwrap_or(Manifest {
        format: 1,
        id: uuid::Uuid::new_v4().to_string(),
        settings: input.settings.clone(),
        files: BTreeMap::new(),
        accounts: BTreeMap::new(),
        versions: BTreeMap::new(),
    });
    if component.server() && state.versions.contains_key(&Component::Server)
        || component.agent() && state.versions.contains_key(&Component::Agent)
    {
        return Err("Selected component is already installed; use upgrade or repair".into());
    }
    if !component.server() && state.versions.contains_key(&Component::Server) {
        input.settings.server_bind = state.settings.server_bind.clone();
        input.settings.server_origin = state.settings.server_origin.clone();
        input.settings.local_http = state.settings.local_http;
        input.settings.server_data = state.settings.server_data.clone();
        input.settings.tls_cert = state.settings.tls_cert.clone();
        input.settings.tls_key = state.settings.tls_key.clone();
        input.settings.admin_username = state.settings.admin_username.clone();
    }
    if !component.agent() && state.versions.contains_key(&Component::Agent) {
        input.settings.agent_server = state.settings.agent_server.clone();
        input.settings.agent_name = state.settings.agent_name.clone();
        input.settings.agent_ca = state.settings.agent_ca.clone();
        input.settings.install_xray = state.settings.install_xray;
    }
    validate(
        &input.settings,
        if state.versions.is_empty() {
            component
        } else {
            Component::All
        },
    )?;
    if component.server()
        && !input.settings.server_data.join("panel.sqlite3").exists()
        && input.admin_password.is_none()
    {
        return Err("First server install requires an administrator password".into());
    }
    if !state.accounts.is_empty() && state.settings.server_data != input.settings.server_data {
        return Err(
            "Existing data path is immutable; use documented backup/restore to migrate".into(),
        );
    }
    verify_files(&state)?;
    check_destinations(&state)?;
    foreign_units(&state, component).await?;
    cache(&bundle).await?;
    store(&state)?;
    checkpoint(&state, component).await?;
    let result = activate(&mut state, &input, component, &bundle, true).await;
    if let Err(error) = result {
        recover().await?;
        return Err(error);
    }
    store(&state)?;
    finish_transaction()?;
    println!(
        "Installed {component:?} {}. Data and credentials are private; inspect panel-installer status.",
        bundle.metadata.version
    );
    Ok(())
}
pub async fn upgrade(component: Component, bundle: Bundle, version: &str) -> Result<()> {
    crate::platform::detect()?;
    if bundle.metadata.version != version {
        return Err("Requested version does not match verified bundle".into());
    }
    let _lock = lock()?;
    if recover().await? {
        return Err("Interrupted transaction restored; inspect before retrying".into());
    }
    let mut state = load()?.ok_or("No owned installation")?;
    require_components(&state, component)?;
    verify_files(&state)?;
    check_destinations(&state)?;
    foreign_units(&state, component).await?;
    cache(&bundle).await?;
    let input = Input {
        settings: state.settings.clone(),
        admin_password: None,
        enrollment_token: None,
    };
    checkpoint(&state, component).await?;
    if let Err(error) = activate(&mut state, &input, component, &bundle, false).await {
        recover().await?;
        return Err(error);
    }
    store(&state)?;
    finish_transaction()?;
    println!(
        "Upgraded {component:?} to {version}; previous files/database were retained until health checks passed"
    );
    Ok(())
}
fn require_components(state: &Manifest, component: Component) -> Result<()> {
    if component.server() && !state.versions.contains_key(&Component::Server)
        || component.agent() && !state.versions.contains_key(&Component::Agent)
    {
        return Err("Selected component is not installed".into());
    }
    Ok(())
}
pub async fn repair(component: Component) -> Result<()> {
    crate::platform::detect()?;
    let _lock = lock()?;
    if recover().await? {
        println!("Recovered interrupted deployment; inspect status");
        return Ok(());
    }
    let mut state = load()?.ok_or("No owned installation")?;
    require_components(&state, component)?;
    foreign_units(&state, component).await?;
    let selected = [
        (component.server(), Component::Server),
        (component.agent(), Component::Agent),
    ];
    for (enabled, role) in selected {
        if !enabled {
            continue;
        }
        let version = state.versions.get(&role).ok_or("Missing version")?.clone();
        let path = bundle::cache_path(&version);
        let digest = lifecycle::hash(&bundle::read(&path.join("bundle.json"), 8192, true)?);
        let bundle = bundle::load(&path, &digest)?;
        // Repair explicitly restores only manifest-owned static files; data is untouched.
        for path in state.files.keys() {
            if path.exists() {
                bundle::secure(path, false)?;
            }
        }
        checkpoint(&state, role).await?;
        let input = Input {
            settings: state.settings.clone(),
            admin_password: None,
            enrollment_token: None,
        };
        if let Err(error) = activate(&mut state, &input, role, &bundle, false).await {
            recover().await?;
            return Err(error);
        }
        store(&state)?;
        finish_transaction()?;
    }
    println!("Repaired selected owned binaries/units/configuration from the verified local cache");
    Ok(())
}
pub async fn report() -> Result<()> {
    crate::platform::detect()?;
    if !Path::new(MANIFEST).exists() && !Path::new(CONTROL).join("journal.json").exists() {
        println!("No owned Panel installation");
        return Ok(());
    }
    let _lock = lock()?;
    let recovered = recover().await?;
    let Some(state) = load()? else {
        println!("No owned Panel installation");
        return Ok(());
    };
    let integrity = verify_files(&state).is_ok();
    println!(
        "{}",
        serde_json::to_string_pretty(
            &serde_json::json!({"installation_id":state.id,"versions":state.versions,"integrity_ok":integrity,"recovered_interrupted_transaction":recovered,"server_active":active("panel-server.service").await,"agent_active":active("panel-agent.service").await,"helper_socket_active":active("panel-xray-helper.socket").await})
        )?
    );
    Ok(())
}
async fn revoke(state: &Manifest) -> Result<()> {
    let account = state
        .accounts
        .get("panel-agent")
        .ok_or("Missing service account")?;
    let mut command = cmd("/usr/local/bin/panel-agent");
    command.args(agent_args(&state.settings)).arg("revoke");
    #[cfg(unix)]
    {
        command.uid(account.uid).gid(account.gid);
    }
    status(command).await
}
fn remove_file(state: &mut Manifest, path: &Path) -> Result<()> {
    if let Some(digest) = state.files.get(path) {
        if lifecycle::hash(&bundle::read(path, 128 * 1024 * 1024, false)?) != *digest {
            return Err("Changed file refused during uninstall".into());
        }
        fs::remove_file(path)?;
        state.files.remove(path);
    }
    Ok(())
}
fn purge_data(path: &Path, id: &str, account: &Account, name: &str) -> Result<()> {
    if !path.exists() {
        return Ok(());
    }
    data_directory(path, id, account)?;
    let allowed = [
        ".panel-owned".to_owned(),
        "agent.lock".to_owned(),
        "server.lock".to_owned(),
        name.into(),
        format!("{name}-wal"),
        format!("{name}-shm"),
    ];
    for entry in fs::read_dir(path)? {
        let entry = entry?;
        let filename = entry.file_name();
        if !allowed.iter().any(|value| filename == value.as_str()) || !entry.file_type()?.is_file()
        {
            return Err("Unexpected unowned data prevents purge".into());
        }
    }
    for entry in fs::read_dir(path)? {
        fs::remove_file(entry?.path())?;
    }
    fs::remove_dir(path)?;
    Ok(())
}
pub async fn uninstall(
    component: Component,
    purge: bool,
    remove_xray: bool,
    confirmation: Option<&str>,
) -> Result<()> {
    crate::platform::detect()?;
    let _lock = lock()?;
    if recover().await? {
        return Err("Interrupted transaction restored; inspect before uninstalling".into());
    }
    let mut state = load()?.ok_or("No owned installation")?;
    if purge && confirmation != Some("PURGE PANEL DATA") {
        return Err(
            "Destructive purge requires exact confirmation: --confirm 'PURGE PANEL DATA'".into(),
        );
    }
    verify_files(&state)?;
    foreign_units(&state, component).await?;
    if remove_xray && !component.agent() {
        return Err("--remove-xray requires the Agent component".into());
    }
    if component.agent() {
        let _ = system("stop", "panel-agent.service").await;
        if state.versions.contains_key(&Component::Agent) && revoke(&state).await.is_err() {
            eprintln!(
                "Agent credential could not be revoked online; revoke this node in the panel before reusing the host"
            );
        }
        let _ = system("stop", "panel-xray-helper.socket").await;
        let _ = system("stop", "panel-xray-helper@*.service").await;
        if remove_xray {
            let manager = lifecycle::Manager {
                layout: lifecycle::Layout::production(),
                service: lifecycle::Systemd::production(),
                user_scope: false,
            };
            let observed = manager
                .operate(panel_common::XrayOperation::Status)
                .await
                .map_err(io)?;
            if observed.installed {
                manager
                    .operate(panel_common::XrayOperation::Uninstall)
                    .await
                    .map_err(io)?;
            }
        }
        for unit in ["panel-agent.service", "panel-xray-helper.socket"] {
            let _ = system("disable", unit).await;
        }
        let paths = state
            .files
            .keys()
            .filter(|path| {
                path.to_str().is_some_and(|path| {
                    path.contains("panel-agent")
                        || path.contains("panel-xray-helper")
                        || (purge && path.ends_with("agent-ca.pem"))
                        || path.ends_with("xray-helper.json")
                })
            })
            .cloned()
            .collect::<Vec<_>>();
        for path in paths {
            remove_file(&mut state, &path)?;
        }
        state.versions.remove(&Component::Agent);
    }
    if component.server() {
        let _ = system("stop", "panel-server.service").await;
        let _ = system("disable", "panel-server.service").await;
        let paths = state
            .files
            .keys()
            .filter(|path| {
                path.to_str().is_some_and(|path| {
                    path.contains("panel-server")
                        || (purge
                            && (path.ends_with("server.env")
                                || path.ends_with("server-cert.pem")
                                || path.ends_with("server-key.pem")))
                })
            })
            .cloned()
            .collect::<Vec<_>>();
        for path in paths {
            remove_file(&mut state, &path)?;
        }
        state.versions.remove(&Component::Server);
    }
    if state.versions.is_empty() {
        remove_file(&mut state, Path::new("/usr/local/bin/panel-installer"))?;
    }
    reload().await?;
    store(&state)?;
    if purge {
        if component.server()
            && let Some(account) = state.accounts.get("panel-server")
        {
            purge_data(
                &state.settings.server_data,
                &state.id,
                account,
                "panel.sqlite3",
            )?;
        }
        if component.agent()
            && let Some(account) = state.accounts.get("panel-agent")
        {
            purge_data(Path::new(AGENT_DATA), &state.id, account, "agent.sqlite3")?;
        }
        for (enabled, name) in [
            (component.server(), "panel-server"),
            (component.agent(), "panel-agent"),
        ] {
            if enabled && let Some(account) = state.accounts.get(name) {
                let actual = lookup(name).await?.ok_or("Service account disappeared")?;
                if actual.uid != account.uid || actual.gid != account.gid {
                    return Err("Changed service account refused during purge".into());
                }
                let mut command = cmd("/usr/sbin/userdel");
                command.arg(name);
                status(command).await?;
                state.accounts.remove(name);
            }
        }
        store(&state)?;
        if remove_xray {
            purge_xray()?;
        }
        if state.versions.is_empty() && state.accounts.is_empty() {
            purge_control()?;
        }
    }
    println!(
        "Uninstalled {component:?}; {}. Xray {}.",
        if purge {
            "selected Panel data purged"
        } else {
            "data and ownership metadata retained"
        },
        if remove_xray && purge {
            "owned service/binary/configuration removed"
        } else if remove_xray {
            "owned service/binary removed; configuration retained"
        } else {
            "left running with its last configuration"
        }
    );
    Ok(())
}
/// Re-exec from a private temporary copy so an external maintenance process replaces/removes the installed CLI.
pub fn maintenance_worker() -> Result<Option<std::process::ExitStatus>> {
    let current = std::env::current_exe()?;
    if current != Path::new("/usr/local/bin/panel-installer") {
        return Ok(None);
    }
    let args = std::env::args_os().skip(1).collect::<Vec<_>>();
    if !args.first().is_some_and(|arg| {
        matches!(
            arg.to_str(),
            Some("install" | "upgrade" | "repair" | "uninstall")
        )
    }) {
        return Ok(None);
    }
    crate::platform::detect()?;
    bundle::secure(&current, false)?;
    let temp = tempfile::Builder::new()
        .prefix("panel-maintenance-")
        .tempdir_in("/run")?;
    lifecycle::mode(temp.path(), 0o700).map_err(io)?;
    let executable = temp.path().join("panel-installer");
    fs::copy(current, &executable)?;
    lifecycle::mode(&executable, 0o700).map_err(io)?;
    Ok(Some(
        std::process::Command::new(executable).args(args).status()?,
    ))
}

fn purge_xray() -> Result<()> {
    let root = Path::new("/opt/panel/xray");
    let control = Path::new("/var/lib/panel/xray-helper");
    if root.exists() {
        let manifest: serde_json::Value =
            serde_json::from_slice(&bundle::read(&root.join("manifest.json"), 16384, true)?)?;
        if manifest["installed"] != false
            || manifest["hashes"]
                .as_object()
                .is_none_or(|map| !map.is_empty())
            || manifest["config_hash"].as_str()
                != Some(&lifecycle::hash(&bundle::read(
                    &root.join("config.json"),
                    65536,
                    true,
                )?))
        {
            return Err("Xray ownership/state does not permit data purge".into());
        }
        for entry in fs::read_dir(root)? {
            let entry = entry?;
            bundle::secure(&entry.path(), false)?;
            let name = entry.file_name();
            if name == "versions" {
                if !entry.file_type()?.is_dir() || fs::read_dir(entry.path())?.next().is_some() {
                    return Err("Unexpected Xray versions prevent purge".into());
                }
            } else if (name != "config.json" && name != "manifest.json")
                || !entry.file_type()?.is_file()
            {
                return Err("Unowned Xray data prevents purge".into());
            }
        }
    }
    if control.exists() {
        for entry in fs::read_dir(control)? {
            let entry = entry?;
            bundle::secure(&entry.path(), true)?;
            let name = entry.file_name();
            let name = name.to_str().ok_or("Invalid helper filename")?;
            let receipt = name
                .strip_suffix(".json")
                .is_some_and(|id| uuid::Uuid::parse_str(id).is_ok());
            if !entry.file_type()?.is_file()
                || (!receipt
                    && !matches!(
                        name,
                        "lifecycle.lock" | "requests.lock" | "config.backup.json"
                    ))
            {
                return Err("Unowned helper data prevents purge".into());
            }
        }
    }
    if root.exists() {
        fs::remove_file(root.join("config.json"))?;
        fs::remove_file(root.join("manifest.json"))?;
        fs::remove_dir(root.join("versions"))?;
        fs::remove_dir(root)?;
    }
    if control.exists() {
        for entry in fs::read_dir(control)? {
            fs::remove_file(entry?.path())?;
        }
        fs::remove_dir(control)?;
    }
    Ok(())
}
fn purge_control() -> Result<()> {
    let control = Path::new(CONTROL);
    let mut caches = Vec::new();
    for entry in fs::read_dir(control)? {
        let entry = entry?;
        bundle::secure(&entry.path(), false)?;
        let name = entry.file_name();
        let name = name.to_str().ok_or("Invalid installer filename")?;
        if matches!(name, "manifest.json" | "installer.lock") && entry.file_type()?.is_file() {
            continue;
        }
        let version = name
            .strip_prefix("bundle-")
            .ok_or("Unowned installer data prevents purge")?;
        if !entry.file_type()?.is_dir()
            || version.is_empty()
            || !version
                .bytes()
                .all(|byte| byte.is_ascii_digit() || b".-".contains(&byte))
        {
            return Err("Invalid cache path prevents purge".into());
        }
        let path = entry.path();
        let bytes = bundle::read(&path.join("bundle.json"), 8192, true)?;
        let cached = bundle::load(&path, &lifecycle::hash(&bytes))?;
        if cached.metadata.version != version {
            return Err("Cache ownership mismatch".into());
        }
        for file in fs::read_dir(&path)? {
            let file = file?;
            if !file.file_type()?.is_file()
                || !file
                    .file_name()
                    .to_str()
                    .is_some_and(|name| bundle::BINARIES.contains(&name) || name == "bundle.json")
            {
                return Err("Unowned cache file prevents purge".into());
            }
        }
        caches.push(path);
    }
    for path in caches {
        for name in bundle::BINARIES {
            fs::remove_file(path.join(name))?;
        }
        fs::remove_file(path.join("bundle.json"))?;
        fs::remove_dir(path)?;
    }
    fs::remove_file(MANIFEST)?;
    fs::remove_file(control.join("installer.lock"))?;
    fs::remove_dir(control)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn deployment_rejects_public_http_injection_traversal_and_root_data_paths() {
        let mut settings = Settings::default();
        assert!(validate(&settings, Component::All).is_ok());
        settings.server_bind = "0.0.0.0:8080".into();
        assert!(validate(&settings, Component::Server).is_err());
        settings = Settings::default();
        settings.server_origin = "http://127.0.0.1:8080\n".into();
        assert!(validate(&settings, Component::Server).is_err());
        for path in [
            "/",
            "/var/lib/panel",
            "/var/lib/panel/../foreign",
            "/var/lib/panel/agent",
            "/etc/panel/server",
            "/var/lib/panel/server/child",
        ] {
            settings = Settings::default();
            settings.server_data = path.into();
            assert!(validate(&settings, Component::Server).is_err(), "{path}");
        }
        settings = Settings::default();
        settings.agent_server = "wss://user:credential@panel.example/agent/v1/connect".into();
        assert!(validate(&settings, Component::Agent).is_err());
        assert!(!managed_path(Path::new("/etc/shadow")));
        assert!(!managed_path(Path::new("/etc/systemd/system/ssh.service")));
    }
}
