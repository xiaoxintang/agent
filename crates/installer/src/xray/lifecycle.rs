use super::download;
use panel_common::{XrayError, XrayOperation, XrayReport, XrayVersion};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    collections::BTreeMap,
    fs::{self, File},
    future::Future,
    io::{Read, Write},
    path::{Path, PathBuf},
    time::Duration,
};
use tokio::process::Command;

pub const CONFIG: &str = "{\"log\":{\"loglevel\":\"warning\"},\"inbounds\":[],\"outbounds\":[{\"protocol\":\"freedom\",\"tag\":\"direct\"}]}\n";

pub struct Layout {
    pub root: PathBuf,
    pub control: PathBuf,
    pub unit: PathBuf,
}
impl Layout {
    pub fn production() -> Self {
        Self {
            root: "/opt/panel/xray".into(),
            control: "/var/lib/panel/xray-helper".into(),
            unit: "/etc/systemd/system/xray.service".into(),
        }
    }
    pub(crate) fn validate(&self) -> Result<(), XrayError> {
        for path in [&self.root, &self.control, &self.unit] {
            if !path.is_absolute()
                || path
                    .components()
                    .any(|component| matches!(component, std::path::Component::ParentDir))
                || path.to_str().is_none_or(|value| {
                    value.chars().any(|c| c.is_whitespace() || c == '"') || value.contains("/../")
                })
            {
                return Err(XrayError::InvalidState);
            }
            for ancestor in path.ancestors() {
                match fs::symlink_metadata(ancestor) {
                    Ok(metadata) if metadata.file_type().is_symlink() => {
                        return Err(XrayError::UnownedInstallation);
                    }
                    Ok(metadata) => {
                        #[cfg(unix)]
                        {
                            use std::os::unix::fs::MetadataExt;
                            if rustix::process::geteuid().as_raw() == 0
                                && (metadata.uid() != 0 || metadata.mode() & 0o022 != 0)
                            {
                                return Err(XrayError::UnownedInstallation);
                            }
                        }
                        let _ = metadata;
                    }
                    Err(error) if error.kind() == std::io::ErrorKind::NotFound => (),
                    Err(_) => return Err(XrayError::InvalidState),
                }
            }
        }
        Ok(())
    }
}

pub struct ServiceState {
    pub active: bool,
    pub fragment: Option<PathBuf>,
}
#[derive(Clone, Copy)]
pub enum ServiceAction {
    Reload,
    Start,
    Stop,
    Restart,
}
pub trait Service {
    fn inspect(&self) -> impl Future<Output = Result<ServiceState, XrayError>> + Send;
    fn action(&self, action: ServiceAction) -> impl Future<Output = Result<(), XrayError>> + Send;
}

pub struct Systemd {
    name: String,
    user: bool,
}
impl Systemd {
    pub fn production() -> Self {
        Self {
            name: "xray.service".into(),
            user: false,
        }
    }
    /// For isolated acceptance only; not exposed by the privileged helper.
    pub fn acceptance(name: &str) -> Result<Self, XrayError> {
        if !name.starts_with("panel-phase4-")
            || !name.ends_with(".service")
            || !name
                .bytes()
                .all(|c| c.is_ascii_alphanumeric() || b"-.".contains(&c))
        {
            return Err(XrayError::Unauthorized);
        }
        Ok(Self {
            name: name.into(),
            user: true,
        })
    }
    fn command(&self) -> Command {
        let mut command = Command::new("/usr/bin/systemctl");
        command
            .env_clear()
            .env("PATH", "/usr/bin:/bin")
            .env("LC_ALL", "C")
            .kill_on_drop(true);
        if self.user {
            command.arg("--user");
            if let Some(runtime) = std::env::var_os("XDG_RUNTIME_DIR") {
                command.env("XDG_RUNTIME_DIR", runtime);
            }
        }
        command
    }
}
impl Service for Systemd {
    async fn inspect(&self) -> Result<ServiceState, XrayError> {
        let output = tokio::time::timeout(
            Duration::from_secs(10),
            self.command()
                .args([
                    "show",
                    &self.name,
                    "--property=FragmentPath",
                    "--property=ActiveState",
                    "--no-pager",
                ])
                .output(),
        )
        .await
        .map_err(|_| XrayError::ServiceFailed)?
        .map_err(|_| XrayError::ServiceFailed)?;
        if output.stdout.len() > 8192
            || !(output.status.success() || output.status.code() == Some(4))
        {
            return Err(XrayError::ServiceFailed);
        }
        let text = String::from_utf8(output.stdout).map_err(|_| XrayError::ServiceFailed)?;
        let mut fragment = None;
        let mut active = false;
        for line in text.lines() {
            if let Some(value) = line.strip_prefix("FragmentPath=")
                && !value.is_empty()
            {
                fragment = Some(value.into());
            }
            if let Some(value) = line.strip_prefix("ActiveState=") {
                active = value == "active";
            }
        }
        Ok(ServiceState { active, fragment })
    }
    async fn action(&self, action: ServiceAction) -> Result<(), XrayError> {
        if matches!(action, ServiceAction::Start | ServiceAction::Restart) {
            // A freshly installed unit may not yet be loaded, so reset-failed can return 1.
            // The actual start/restart below must still succeed.
            let _ = tokio::time::timeout(
                Duration::from_secs(10),
                self.command()
                    .args(["reset-failed", &self.name])
                    .stdout(std::process::Stdio::null())
                    .stderr(std::process::Stdio::null())
                    .status(),
            )
            .await
            .map_err(|_| XrayError::ServiceFailed)?
            .map_err(|_| XrayError::ServiceFailed)?;
        }
        let mut command = self.command();
        command.arg(match action {
            ServiceAction::Reload => "daemon-reload",
            ServiceAction::Start => "start",
            ServiceAction::Stop => "stop",
            ServiceAction::Restart => "restart",
        });
        if !matches!(action, ServiceAction::Reload) {
            command.arg(&self.name);
        }
        command
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null());
        if !tokio::time::timeout(Duration::from_secs(15), command.status())
            .await
            .map_err(|_| XrayError::ServiceFailed)?
            .map_err(|_| XrayError::ServiceFailed)?
            .success()
        {
            return Err(XrayError::ServiceFailed);
        }
        Ok(())
    }
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Manifest {
    format: u8,
    installation_id: String,
    installed: bool,
    version: Option<XrayVersion>,
    previous: Option<XrayVersion>,
    hashes: BTreeMap<String, String>,
    unit_hash: String,
    config_hash: String,
    #[serde(default)]
    applied_revision: i64,
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct ConfigSwitch {
    previous: Manifest,
    candidate_hash: String,
    was_active: bool,
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct Switch {
    previous: Manifest,
    target: XrayVersion,
    was_active: bool,
}

pub struct Manager<S> {
    pub layout: Layout,
    pub service: S,
    pub user_scope: bool,
}

impl<S: Service> Manager<S> {
    pub async fn apply_config(
        &self,
        submission: panel_common::configuration::ConfigSubmission,
    ) -> Result<XrayReport, XrayError> {
        self.layout.validate()?;
        private_directory(&self.layout.control)?;
        let lock = private_file(&self.layout.control.join("lifecycle.lock"))?;
        lock.try_lock().map_err(|_| XrayError::InvalidState)?;
        let mut state = self
            .load()
            .await?
            .filter(|state| state.installed)
            .ok_or(XrayError::InvalidState)?;
        if self.recover_config(&mut state).await? || self.recover_switch(&mut state).await? {
            return Err(XrayError::Interrupted);
        }
        let bytes = submission
            .config
            .render()
            .map_err(|_| XrayError::VerificationFailed)?;
        if hash(&bytes) != submission.reference.content_hash
            || submission.reference.revision <= 0
            || submission.reference.revision < state.applied_revision
        {
            return Err(XrayError::VerificationFailed);
        }
        if submission.reference.revision == state.applied_revision {
            if state.config_hash != submission.reference.content_hash {
                return Err(XrayError::VerificationFailed);
            }
            return self.report(&state).await;
        }
        let candidate = tempfile::NamedTempFile::new_in(&self.layout.control)
            .map_err(|_| XrayError::InvalidState)?;
        atomic(candidate.path(), &bytes, 0o600)?;
        run_binary(
            &self.layout.root.join("current/xray"),
            &["run", "-test", "-format", "json", "-config", "/dev/stdin"],
            None,
            Some(candidate.path()),
        )
        .await?;
        let backup = read(&self.layout.root.join("config.json"), 65536)?;
        atomic(
            &self.layout.control.join("config.backup.json"),
            &backup,
            0o600,
        )?;
        let pending = ConfigSwitch {
            previous: state.clone(),
            candidate_hash: hash(&bytes),
            was_active: self.service.inspect().await?.active,
        };
        atomic(
            &self.layout.control.join("config-switch.json"),
            &serde_json::to_vec(&pending).map_err(|_| XrayError::InvalidState)?,
            0o600,
        )?;
        atomic(&self.layout.root.join("config.json"), &bytes, 0o600)?;
        if self.service.action(ServiceAction::Restart).await.is_err()
            || self.health().await.is_err()
        {
            self.recover_config(&mut state).await?;
            return Err(XrayError::ServiceFailed);
        }
        state.config_hash = pending.candidate_hash;
        state.applied_revision = submission.reference.revision;
        self.save(&state)?;
        fs::remove_file(self.layout.control.join("config-switch.json"))
            .map_err(|_| XrayError::InvalidState)?;
        sync(&self.layout.control)?;
        self.report(&state).await
    }

    async fn recover_config(&self, state: &mut Manifest) -> Result<bool, XrayError> {
        let path = self.layout.control.join("config-switch.json");
        if fs::symlink_metadata(&path).is_err() {
            return Ok(false);
        }
        let pending: ConfigSwitch =
            serde_json::from_slice(&read(&path, 16384)?).map_err(|_| XrayError::InvalidState)?;
        if pending.previous.installation_id != state.installation_id
            || pending.previous.hashes != state.hashes
            || pending.previous.unit_hash != state.unit_hash
            || pending.previous.version != state.version
        {
            return Err(XrayError::UnownedInstallation);
        }
        let backup = read(&self.layout.control.join("config.backup.json"), 65536)?;
        if hash(&backup) != pending.previous.config_hash {
            return Err(XrayError::UnownedInstallation);
        }
        atomic(&self.layout.root.join("config.json"), &backup, 0o600)?;
        if pending.was_active {
            self.service
                .action(ServiceAction::Restart)
                .await
                .map_err(|_| XrayError::RollbackFailed)?;
            self.health().await.map_err(|_| XrayError::RollbackFailed)?;
        } else {
            self.service.action(ServiceAction::Stop).await?;
        }
        *state = pending.previous;
        self.save(state)?;
        fs::remove_file(path).map_err(|_| XrayError::InvalidState)?;
        sync(&self.layout.control)?;
        Ok(true)
    }

    pub async fn operate(&self, operation: XrayOperation) -> Result<XrayReport, XrayError> {
        self.layout.validate()?;
        private_directory(&self.layout.control)?;
        let lock = private_file(&self.layout.control.join("lifecycle.lock"))?;
        lock.try_lock().map_err(|_| XrayError::InvalidState)?;
        let mut manifest = self.load().await?;
        if let Some(state) = &mut manifest
            && (self.recover_config(state).await? || self.recover_switch(state).await?)
        {
            return if matches!(operation, XrayOperation::Status | XrayOperation::Rollback) {
                self.report(state).await
            } else {
                Err(XrayError::Interrupted)
            };
        }
        match operation {
            XrayOperation::Install(release) => {
                if manifest.as_ref().is_some_and(|value| value.installed) {
                    return Err(XrayError::InvalidState);
                }
                if manifest.is_none() {
                    // Do not create or touch an installation until official bytes have been verified.
                    let bytes = download::fetch(release.version, std::env::consts::ARCH).await?;
                    if let Some(parent) = self.layout.root.parent()
                        && !parent.exists()
                    {
                        fs::create_dir(parent).map_err(|_| XrayError::InvalidState)?;
                        mode(parent, 0o755)?;
                    }
                    fs::create_dir(&self.layout.root)
                        .map_err(|_| XrayError::UnownedInstallation)?;
                    mode(&self.layout.root, 0o755)?;
                    fs::create_dir(self.layout.root.join("versions"))
                        .map_err(|_| XrayError::InvalidState)?;
                    mode(&self.layout.root.join("versions"), 0o755)?;
                    let mut initial = Manifest {
                        format: 1,
                        installation_id: uuid::Uuid::new_v4().to_string(),
                        installed: false,
                        version: None,
                        previous: None,
                        hashes: BTreeMap::new(),
                        unit_hash: String::new(),
                        config_hash: hash(CONFIG.as_bytes()),
                        applied_revision: 0,
                    };
                    atomic(
                        &self.layout.root.join("config.json"),
                        CONFIG.as_bytes(),
                        0o600,
                    )?;
                    self.save(&initial)?;
                    self.stage(&mut initial, release.version, &bytes).await?;
                    self.save(&initial)?;
                    manifest = Some(initial);
                } else {
                    let bytes = download::fetch(release.version, std::env::consts::ARCH).await?;
                    self.stage(
                        manifest.as_mut().ok_or(XrayError::InvalidState)?,
                        release.version,
                        &bytes,
                    )
                    .await?;
                }
                let state = manifest.as_mut().ok_or(XrayError::InvalidState)?;
                let unit = self.unit(&state.installation_id);
                atomic(&self.layout.unit, unit.as_bytes(), 0o644)?;
                state.unit_hash = hash(unit.as_bytes());
                state.installed = true;
                state.version = Some(release.version);
                self.activate(release.version)?;
                self.save(state)?;
                if self.service.action(ServiceAction::Reload).await.is_err()
                    || self.service.action(ServiceAction::Start).await.is_err()
                    || self.health().await.is_err()
                {
                    self.service.action(ServiceAction::Stop).await?;
                    self.remove_owned(state).await?;
                    return Err(XrayError::ServiceFailed);
                }
            }
            XrayOperation::Upgrade(release) => {
                let state = manifest
                    .as_mut()
                    .filter(|value| value.installed)
                    .ok_or(XrayError::InvalidState)?;
                if state.version == Some(release.version) {
                    return self.report(state).await;
                }
                let bytes = download::fetch(release.version, std::env::consts::ARCH).await?;
                self.stage(state, release.version, &bytes).await?;
                self.test_binary(release.version).await?;
                let old = state.version.ok_or(XrayError::InvalidState)?;
                let was_active = self.service.inspect().await?.active;
                self.checkpoint(state, release.version, was_active)?;
                self.activate(release.version)?;
                if was_active
                    && (self.service.action(ServiceAction::Restart).await.is_err()
                        || self.health().await.is_err())
                {
                    self.activate(old)?;
                    if self.service.action(ServiceAction::Restart).await.is_err()
                        || self.health().await.is_err()
                    {
                        return Err(XrayError::RollbackFailed);
                    }
                    self.save(state)?;
                    self.finish_switch()?;
                    return Err(XrayError::ServiceFailed);
                }
                state.previous = Some(old);
                state.version = Some(release.version);
                self.save(state)?;
                self.finish_switch()?;
            }
            XrayOperation::Rollback => {
                let state = manifest
                    .as_mut()
                    .filter(|value| value.installed)
                    .ok_or(XrayError::InvalidState)?;
                let previous = state.previous.ok_or(XrayError::InvalidState)?;
                let current = state.version.ok_or(XrayError::InvalidState)?;
                self.test_binary(previous).await?;
                let active = self.service.inspect().await?.active;
                self.checkpoint(state, previous, active)?;
                self.activate(previous)?;
                if active
                    && (self.service.action(ServiceAction::Restart).await.is_err()
                        || self.health().await.is_err())
                {
                    self.activate(current)?;
                    if self.service.action(ServiceAction::Restart).await.is_err()
                        || self.health().await.is_err()
                    {
                        return Err(XrayError::RollbackFailed);
                    }
                    self.finish_switch()?;
                    return Err(XrayError::ServiceFailed);
                }
                state.version = Some(previous);
                state.previous = Some(current);
                self.save(state)?;
                self.finish_switch()?;
            }
            XrayOperation::Start | XrayOperation::Stop | XrayOperation::Restart => {
                let state = manifest
                    .as_ref()
                    .filter(|value| value.installed)
                    .ok_or(XrayError::InvalidState)?;
                self.test_binary(state.version.ok_or(XrayError::InvalidState)?)
                    .await?;
                self.service
                    .action(match operation {
                        XrayOperation::Start => ServiceAction::Start,
                        XrayOperation::Stop => ServiceAction::Stop,
                        _ => ServiceAction::Restart,
                    })
                    .await?;
                if !matches!(operation, XrayOperation::Stop) {
                    self.health().await?;
                }
            }
            XrayOperation::Uninstall => {
                let state = manifest
                    .as_mut()
                    .filter(|value| value.installed)
                    .ok_or(XrayError::InvalidState)?;
                self.service.action(ServiceAction::Stop).await?;
                self.remove_owned(state).await?;
            }
            XrayOperation::Status => (),
        }
        match manifest {
            Some(state) => self.report(&state).await,
            None => Ok(XrayReport {
                installed: false,
                version: None,
                previous_version: None,
                active: false,
            }),
        }
    }

    async fn load(&self) -> Result<Option<Manifest>, XrayError> {
        if !self.user_scope
            && ["/usr/local/bin/xray", "/usr/bin/xray", "/etc/xray"]
                .iter()
                .any(|path| fs::symlink_metadata(path).is_ok())
        {
            return Err(XrayError::UnownedInstallation);
        }
        if !self.user_scope
            && [
                "/etc/systemd/system/xray.service.d",
                "/run/systemd/system/xray.service.d",
            ]
            .iter()
            .any(|path| fs::symlink_metadata(path).is_ok())
        {
            return Err(XrayError::UnownedInstallation);
        }
        let service = self.service.inspect().await?;
        let root = &self.layout.root;
        if !root.exists() {
            if fs::symlink_metadata(root).is_ok()
                || self.layout.unit.exists()
                || service.fragment.is_some()
            {
                return Err(XrayError::UnownedInstallation);
            }
            return Ok(None);
        }
        owned(root)?;
        let state: Manifest = serde_json::from_slice(&read(&root.join("manifest.json"), 16384)?)
            .map_err(|_| XrayError::UnownedInstallation)?;
        if state.format != 1
            || uuid::Uuid::parse_str(&state.installation_id).is_err()
            || state.hashes.len() > 2
        {
            return Err(XrayError::InvalidState);
        }
        let actual_hash = hash(&read(&root.join("config.json"), 65536)?);
        if actual_hash != state.config_hash {
            let pending: ConfigSwitch = serde_json::from_slice(&read(
                &self.layout.control.join("config-switch.json"),
                16384,
            )?)
            .map_err(|_| XrayError::UnownedInstallation)?;
            if pending.previous.installation_id != state.installation_id
                || (actual_hash != pending.previous.config_hash
                    && actual_hash != pending.candidate_hash)
            {
                return Err(XrayError::UnownedInstallation);
            }
        }
        owned(&root.join("versions"))?;
        for (version, digest) in &state.hashes {
            let version: XrayVersion =
                serde_json::from_value(serde_json::Value::String(version.clone()))
                    .map_err(|_| XrayError::InvalidState)?;
            let directory = root.join("versions").join(version.name());
            owned(&directory)?;
            if hash(&read(&directory.join("xray"), download::BINARY_LIMIT)?) != *digest {
                return Err(XrayError::UnownedInstallation);
            }
            if fs::read_dir(directory)
                .map_err(|_| XrayError::InvalidState)?
                .count()
                != 1
            {
                return Err(XrayError::UnownedInstallation);
            }
        }
        if fs::read_dir(root.join("versions"))
            .map_err(|_| XrayError::InvalidState)?
            .count()
            != state.hashes.len()
        {
            return Err(XrayError::UnownedInstallation);
        }
        if state.installed {
            let version = state.version.ok_or(XrayError::InvalidState)?;
            if !state.hashes.contains_key(version.name())
                || (fs::read_link(root.join("current")).map_err(|_| XrayError::InvalidState)?
                    != PathBuf::from("versions").join(version.name())
                    && !self.layout.control.join("switch.json").exists())
            {
                return Err(XrayError::InvalidState);
            }
            if hash(&read(&self.layout.unit, 16384)?) != state.unit_hash
                || service
                    .fragment
                    .as_ref()
                    .is_some_and(|path| path != &self.layout.unit)
            {
                return Err(XrayError::UnownedInstallation);
            }
        } else if service.fragment.is_some()
            || fs::symlink_metadata(&self.layout.unit).is_ok()
            || fs::symlink_metadata(root.join("current")).is_ok()
        {
            return Err(XrayError::UnownedInstallation);
        }
        Ok(Some(state))
    }

    async fn stage(
        &self,
        state: &mut Manifest,
        version: XrayVersion,
        bytes: &[u8],
    ) -> Result<(), XrayError> {
        let destination = self.layout.root.join("versions").join(version.name());
        let digest = hash(bytes);
        if destination.exists() {
            if state.hashes.get(version.name()) != Some(&digest) {
                return Err(XrayError::UnownedInstallation);
            }
            return Ok(());
        }
        let stage = tempfile::Builder::new()
            .prefix(".stage-")
            .tempdir_in(self.layout.root.join("versions"))
            .map_err(|_| XrayError::InvalidState)?;
        atomic(&stage.path().join("xray"), bytes, 0o755)?;
        mode(stage.path(), 0o755)?;
        run_binary(
            &stage.path().join("xray"),
            &["version"],
            Some(version.name()),
            None,
        )
        .await?;
        run_binary(
            &stage.path().join("xray"),
            &["run", "-test", "-format", "json", "-config", "/dev/stdin"],
            None,
            Some(&self.layout.root.join("config.json")),
        )
        .await?;
        mode(stage.path(), 0o755)?;
        fs::rename(stage.path(), &destination).map_err(|_| XrayError::InvalidState)?;
        state.hashes.insert(version.name().into(), digest);
        Ok(())
    }
    async fn test_binary(&self, version: XrayVersion) -> Result<(), XrayError> {
        run_binary(
            &self
                .layout
                .root
                .join("versions")
                .join(version.name())
                .join("xray"),
            &["run", "-test", "-format", "json", "-config", "/dev/stdin"],
            None,
            Some(&self.layout.root.join("config.json")),
        )
        .await
    }
    fn activate(&self, version: XrayVersion) -> Result<(), XrayError> {
        #[cfg(unix)]
        {
            let next = self
                .layout
                .root
                .join(format!(".current-{}", uuid::Uuid::new_v4()));
            std::os::unix::fs::symlink(PathBuf::from("versions").join(version.name()), &next)
                .map_err(|_| XrayError::InvalidState)?;
            fs::rename(next, self.layout.root.join("current"))
                .map_err(|_| XrayError::InvalidState)?;
            sync(&self.layout.root)?;
            Ok(())
        }
        #[cfg(not(unix))]
        {
            let _ = version;
            Err(XrayError::UnsupportedPlatform)
        }
    }
    fn save(&self, state: &Manifest) -> Result<(), XrayError> {
        atomic(
            &self.layout.root.join("manifest.json"),
            &serde_json::to_vec(state).map_err(|_| XrayError::InvalidState)?,
            0o600,
        )
    }
    fn checkpoint(
        &self,
        state: &Manifest,
        target: XrayVersion,
        was_active: bool,
    ) -> Result<(), XrayError> {
        self.save(state)?;
        let bytes = serde_json::to_vec(&Switch {
            previous: state.clone(),
            target,
            was_active,
        })
        .map_err(|_| XrayError::InvalidState)?;
        atomic(&self.layout.control.join("switch.json"), &bytes, 0o600)
    }
    fn finish_switch(&self) -> Result<(), XrayError> {
        fs::remove_file(self.layout.control.join("switch.json"))
            .map_err(|_| XrayError::InvalidState)?;
        sync(&self.layout.control)
    }
    async fn recover_switch(&self, state: &mut Manifest) -> Result<bool, XrayError> {
        let path = self.layout.control.join("switch.json");
        if fs::symlink_metadata(&path).is_err() {
            return Ok(false);
        }
        let pending: Switch =
            serde_json::from_slice(&read(&path, 16384)?).map_err(|_| XrayError::InvalidState)?;
        let old = pending.previous.version.ok_or(XrayError::InvalidState)?;
        if pending.previous.installation_id != state.installation_id
            || pending.previous.hashes != state.hashes
            || pending.previous.unit_hash != state.unit_hash
            || pending.previous.config_hash != state.config_hash
            || !state.hashes.contains_key(pending.target.name())
        {
            return Err(XrayError::UnownedInstallation);
        }
        let link =
            fs::read_link(self.layout.root.join("current")).map_err(|_| XrayError::InvalidState)?;
        if link != PathBuf::from("versions").join(old.name())
            && link != PathBuf::from("versions").join(pending.target.name())
        {
            return Err(XrayError::UnownedInstallation);
        }
        self.test_binary(old).await?;
        self.activate(old)?;
        if pending.was_active {
            self.service
                .action(ServiceAction::Restart)
                .await
                .map_err(|_| XrayError::RollbackFailed)?;
            self.health().await.map_err(|_| XrayError::RollbackFailed)?;
        } else {
            self.service.action(ServiceAction::Stop).await?;
        }
        *state = pending.previous;
        self.save(state)?;
        self.finish_switch()?;
        Ok(true)
    }
    async fn health(&self) -> Result<(), XrayError> {
        tokio::time::sleep(Duration::from_millis(500)).await;
        if !self.service.inspect().await?.active {
            return Err(XrayError::ServiceFailed);
        }
        Ok(())
    }
    async fn report(&self, state: &Manifest) -> Result<XrayReport, XrayError> {
        Ok(XrayReport {
            installed: state.installed,
            version: state.version,
            previous_version: state.previous,
            active: self.service.inspect().await?.active,
        })
    }
    async fn remove_owned(&self, state: &mut Manifest) -> Result<(), XrayError> {
        // All these paths and bytes were checked against the ownership manifest before mutation.
        fs::remove_file(&self.layout.unit).map_err(|_| XrayError::InvalidState)?;
        fs::remove_file(self.layout.root.join("current")).map_err(|_| XrayError::InvalidState)?;
        for version in state.hashes.keys() {
            fs::remove_file(self.layout.root.join("versions").join(version).join("xray"))
                .map_err(|_| XrayError::InvalidState)?;
            fs::remove_dir(self.layout.root.join("versions").join(version))
                .map_err(|_| XrayError::InvalidState)?;
        }
        state.hashes.clear();
        state.installed = false;
        state.version = None;
        state.previous = None;
        state.unit_hash.clear();
        self.save(state)?;
        self.service.action(ServiceAction::Reload).await
    }
    fn unit(&self, id: &str) -> String {
        let privilege = if self.user_scope {
            ""
        } else {
            "DynamicUser=yes\nProtectSystem=strict\nProtectHome=true\nPrivateDevices=true\nCapabilityBoundingSet=CAP_NET_BIND_SERVICE\nAmbientCapabilities=CAP_NET_BIND_SERVICE\n"
        };
        format!(
            "# Managed by panel-xray {id}\n[Unit]\nDescription=Panel managed Xray\n[Service]\nType=simple\n{privilege}NoNewPrivileges=true\nLoadCredential=config.json:{}/config.json\nExecStart={}/current/xray run -config %d/config.json\nRestart=on-failure\nRestartSec=2\nLimitCORE=0\nUMask=0077\n[Install]\nWantedBy=multi-user.target\n",
            self.layout.root.display(),
            self.layout.root.display()
        )
    }
}

async fn run_binary(
    binary: &Path,
    args: &[&str],
    version: Option<&str>,
    config: Option<&Path>,
) -> Result<(), XrayError> {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let input = config.map(|path| read(path, 65536)).transpose()?;
    #[cfg(target_os = "linux")]
    let (mut command, credential) = if rustix::process::geteuid().as_raw() == 0 {
        // Seccomp-backed systemd hardening removes CAP_SETUID from the root helper.
        // PID 1 assigns the validator a fresh unprivileged identity instead.
        let unit = format!("panel-xray-validate-{}.service", uuid::Uuid::new_v4());
        let mut command = Command::new("/usr/bin/systemd-run");
        command
            .args(["--quiet", "--wait", "--pipe", "--collect"])
            .arg(format!("--unit={unit}"))
            .args([
                "--property=DynamicUser=yes",
                "--property=NoNewPrivileges=true",
                "--property=CapabilityBoundingSet=",
                "--property=PrivateDevices=true",
                "--property=ProtectSystem=strict",
                "--property=ProtectHome=true",
                "--property=RuntimeMaxSec=10",
                "--property=TimeoutStopSec=1",
                "--property=MemoryMax=128M",
            ]);
        if let Some(config) = config {
            // Go reopens /dev/stdin, which a DynamicUser cannot do for a root-owned pipe.
            // PID 1 grants just this bounded private file to the validation identity.
            command.arg(format!(
                "--property=LoadCredential=config.json:{}",
                config.display()
            ));
        }
        command.arg("--").arg(binary);
        (
            command,
            config.map(|_| format!("/run/credentials/{unit}/config.json")),
        )
    } else {
        (Command::new(binary), None::<String>)
    };
    #[cfg(not(target_os = "linux"))]
    let (mut command, credential) = (Command::new(binary), None::<String>);
    let input = if credential.is_some() { None } else { input };
    command
        .args(args.iter().map(|argument| {
            if *argument == "/dev/stdin" {
                credential.as_deref().unwrap_or(argument)
            } else {
                *argument
            }
        }))
        .env_clear()
        .env("PATH", "/usr/bin:/bin")
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null())
        .kill_on_drop(true);
    command.stdin(if input.is_some() {
        std::process::Stdio::piped()
    } else {
        std::process::Stdio::null()
    });
    let mut child = command.spawn().map_err(|error| {
        eprintln!(
            "Xray validator could not start: kind={:?}, os_code={:?}",
            error.kind(),
            error.raw_os_error()
        );
        XrayError::VerificationFailed
    })?;
    tokio::time::timeout(Duration::from_secs(10), async {
        if let Some(input) = input {
            let mut stdin = child.stdin.take().ok_or(XrayError::VerificationFailed)?;
            stdin
                .write_all(&input)
                .await
                .map_err(|_| XrayError::VerificationFailed)?;
            stdin
                .shutdown()
                .await
                .map_err(|_| XrayError::VerificationFailed)?;
        }
        let mut text = String::new();
        child
            .stdout
            .take()
            .ok_or(XrayError::VerificationFailed)?
            .take(8193)
            .read_to_string(&mut text)
            .await
            .map_err(|_| XrayError::VerificationFailed)?;
        if text.len() > 8192
            || !child
                .wait()
                .await
                .map_err(|_| XrayError::VerificationFailed)?
                .success()
            || version.is_some_and(|version| !text.starts_with(&format!("Xray {version} ")))
        {
            eprintln!(
                "Xray {} validation failed (bounded output, version/config test)",
                version.unwrap_or("configuration")
            );
            return Err(XrayError::VerificationFailed);
        }
        Ok(())
    })
    .await
    .map_err(|_| XrayError::VerificationFailed)?
}
pub fn hash(bytes: &[u8]) -> String {
    hex::encode(Sha256::digest(bytes))
}
pub fn owned(path: &Path) -> Result<(), XrayError> {
    let metadata = fs::symlink_metadata(path).map_err(|_| XrayError::UnownedInstallation)?;
    if metadata.file_type().is_symlink() {
        return Err(XrayError::UnownedInstallation);
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        if metadata.uid() != rustix::process::geteuid().as_raw() || metadata.mode() & 0o022 != 0 {
            return Err(XrayError::UnownedInstallation);
        }
    }
    Ok(())
}
pub fn read(path: &Path, limit: u64) -> Result<Vec<u8>, XrayError> {
    owned(path)?;
    let file = File::open(path).map_err(|_| XrayError::InvalidState)?;
    if !file
        .metadata()
        .map_err(|_| XrayError::InvalidState)?
        .is_file()
    {
        return Err(XrayError::InvalidState);
    }
    let mut bytes = Vec::new();
    file.take(limit + 1)
        .read_to_end(&mut bytes)
        .map_err(|_| XrayError::InvalidState)?;
    if bytes.len() as u64 > limit {
        return Err(XrayError::InvalidState);
    }
    Ok(bytes)
}
pub fn mode(path: &Path, value: u32) -> Result<(), XrayError> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(path, fs::Permissions::from_mode(value))
            .map_err(|_| XrayError::InvalidState)?;
    }
    #[cfg(not(unix))]
    {
        let _ = (path, value);
    }
    Ok(())
}
fn sync(path: &Path) -> Result<(), XrayError> {
    File::open(path)
        .and_then(|file| file.sync_all())
        .map_err(|_| XrayError::InvalidState)
}
pub fn atomic(path: &Path, bytes: &[u8], permissions: u32) -> Result<(), XrayError> {
    let parent = path.parent().ok_or(XrayError::InvalidState)?;
    let mut temp = tempfile::NamedTempFile::new_in(parent).map_err(|_| XrayError::InvalidState)?;
    mode(temp.path(), permissions)?;
    temp.write_all(bytes)
        .and_then(|_| temp.as_file().sync_all())
        .map_err(|_| XrayError::InvalidState)?;
    temp.persist(path).map_err(|_| XrayError::InvalidState)?;
    sync(parent)
}
pub fn private_directory(path: &Path) -> Result<(), XrayError> {
    if let Some(parent) = path.parent()
        && !parent.exists()
    {
        fs::create_dir(parent).map_err(|_| XrayError::InvalidState)?;
        mode(parent, 0o755)?;
    }
    panel_common::storage::private_directory(path).map_err(|_| XrayError::UnownedInstallation)
}
pub fn private_file(path: &Path) -> Result<File, XrayError> {
    panel_common::storage::private_file(path).map_err(|_| XrayError::UnownedInstallation)
}
