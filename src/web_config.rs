//! Shared installation metadata and the local, recoverable administrator credential.
use crate::{
    config,
    error::{Error, Result},
};
use argon2::{
    Argon2, PasswordHasher, PasswordVerifier,
    password_hash::{PasswordHash, SaltString},
};
use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use rand::TryRngCore;
use serde::{Deserialize, Serialize};
use std::{fs, path::Path};

pub const SETTINGS: &str = "/etc/xray-agent/web.toml";
pub const CREDENTIALS: &str = "/etc/xray-agent/web-credentials.json";
pub const LOCAL_AGENT: &str = "/etc/xray-agent/web-local-agent.json";
pub const DATABASE: &str = "/var/lib/xray-web/state.db";
pub const FRONTEND: &str = "/opt/xray-web/current/frontend";

#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Settings {
    pub enabled: bool,
    pub port: u16,
    #[serde(default)]
    pub secure_cookie: bool,
}
impl Default for Settings {
    fn default() -> Self {
        Self {
            enabled: false,
            port: 3000,
            secure_cookie: false,
        }
    }
}
impl Settings {
    pub fn load(path: &Path) -> Result<Self> {
        if !path.exists() {
            return Ok(Self::default());
        }
        let value: Self = toml::from_str(&fs::read_to_string(path)?)
            .map_err(|e| Error::Validation(format!("invalid Web settings: {e}")))?;
        if value.port == 0 {
            return Err(Error::Validation("Web port must be nonzero".into()));
        }
        Ok(value)
    }
    pub fn save(&self, path: &Path) -> Result<()> {
        write_private(
            path,
            toml::to_string_pretty(self)
                .map_err(|e| Error::Internal(e.to_string()))?
                .as_bytes(),
        )
    }
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Credentials {
    pub username: String,
    // Deliberately recoverable: only the root CLI exposes this field, never HTTP.
    pub password: String,
    pub password_hash: String,
    pub epoch: String,
}

pub fn secret(bytes: usize) -> Result<String> {
    let mut data = vec![0; bytes];
    rand::rngs::OsRng
        .try_fill_bytes(&mut data)
        .map_err(|e| Error::Internal(e.to_string()))?;
    Ok(URL_SAFE_NO_PAD.encode(data))
}
impl Credentials {
    pub fn load(path: &Path) -> Result<Self> {
        Ok(serde_json::from_slice(&fs::read(path)?)?)
    }
    pub fn create(path: &Path) -> Result<Self> {
        if path.exists() {
            return Self::load(path);
        }
        let mut value = Self {
            username: format!("admin-{}", secret(6)?),
            password: String::new(),
            password_hash: String::new(),
            epoch: String::new(),
        };
        value.rotate()?;
        // Never replace an account created by another concurrent initializer.
        let parent = path
            .parent()
            .ok_or_else(|| Error::Validation("credentials need a directory".into()))?;
        fs::create_dir_all(parent)?;
        let mut temporary = tempfile::NamedTempFile::new_in(parent)?;
        use std::io::Write;
        temporary.write_all(&serde_json::to_vec_pretty(&value)?)?;
        temporary.as_file().sync_all()?;
        match temporary.persist_noclobber(path) {
            Ok(_) => {
                fs::File::open(parent)?.sync_all()?;
                Ok(value)
            }
            Err(e) if e.error.kind() == std::io::ErrorKind::AlreadyExists => Self::load(path),
            Err(e) => Err(e.error.into()),
        }
    }
    fn rotate(&mut self) -> Result<()> {
        self.password = secret(24)?;
        self.epoch = secret(24)?;
        let mut salt_bytes = [0u8; 16];
        rand::rngs::OsRng
            .try_fill_bytes(&mut salt_bytes)
            .map_err(|e| Error::Internal(e.to_string()))?;
        let salt =
            SaltString::encode_b64(&salt_bytes).map_err(|e| Error::Internal(e.to_string()))?;
        self.password_hash = Argon2::default()
            .hash_password(self.password.as_bytes(), &salt)
            .map_err(|e| Error::Internal(e.to_string()))?
            .to_string();
        Ok(())
    }
    pub fn reset(path: &Path) -> Result<Self> {
        let mut value = Self::load(path)?;
        value.rotate()?;
        write_private(path, &serde_json::to_vec_pretty(&value)?)?;
        Ok(value)
    }
    pub fn verify(&self, username: &str, password: &str) -> bool {
        let valid = PasswordHash::new(&self.password_hash)
            .ok()
            .is_some_and(|hash| {
                Argon2::default()
                    .verify_password(password.as_bytes(), &hash)
                    .is_ok()
            });
        valid && username == self.username
    }
}

#[derive(Clone, Serialize, Deserialize)]
pub struct LocalAgent {
    pub url: String,
    pub token: String,
}
pub fn sync_local(settings: &config::Settings) -> Result<()> {
    let ip = if settings.listen.ip().is_unspecified() {
        if settings.listen.is_ipv6() {
            "[::1]".into()
        } else {
            "127.0.0.1".into()
        }
    } else if settings.listen.is_ipv6() {
        format!("[{}]", settings.listen.ip())
    } else {
        settings.listen.ip().to_string()
    };
    let value = LocalAgent {
        url: format!("http://{ip}:{}", settings.listen.port()),
        token: settings.token.clone(),
    };
    write_private(Path::new(LOCAL_AGENT), &serde_json::to_vec(&value)?)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn password_rotation_preserves_name_and_invalidates_old_credentials() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("credentials.json");
        let old = Credentials::create(&path).unwrap();
        assert!(old.verify(&old.username, &old.password));
        assert_eq!(Credentials::create(&path).unwrap().epoch, old.epoch);
        let new = Credentials::reset(&path).unwrap();
        assert_eq!(old.username, new.username);
        assert_ne!(old.epoch, new.epoch);
        assert!(!new.verify(&new.username, &old.password));
        assert!(new.verify(&new.username, &new.password));
    }
}

// Atomic replacement must preserve the dedicated service group's read access.
fn write_private(path: &Path, contents: &[u8]) -> Result<()> {
    use std::io::Write;
    let previous = fs::metadata(path).ok();
    let parent = path
        .parent()
        .ok_or_else(|| Error::Validation("credential needs a directory".into()))?;
    fs::create_dir_all(parent)?;
    let mut file = tempfile::NamedTempFile::new_in(parent)?;
    if let Some(metadata) = previous {
        #[cfg(unix)]
        {
            use std::os::unix::fs::MetadataExt;
            std::os::unix::fs::chown(file.path(), Some(metadata.uid()), Some(metadata.gid()))?;
        }
        file.as_file().set_permissions(metadata.permissions())?;
    }
    file.write_all(contents)?;
    file.as_file().sync_all()?;
    file.persist(path).map_err(|e| Error::Io(e.error))?;
    fs::File::open(parent)?.sync_all()?;
    Ok(())
}
