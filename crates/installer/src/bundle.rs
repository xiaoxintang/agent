use crate::platform::Result;
use serde::{Deserialize, Serialize};
use std::{
    collections::BTreeMap,
    fs,
    path::{Path, PathBuf},
};

pub const BINARIES: [&str; 3] = ["panel-server", "panel-agent", "panel-installer"];
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Metadata {
    pub format: u8,
    pub version: String,
    pub arch: String,
    pub files: BTreeMap<String, String>,
}
pub struct Bundle {
    pub metadata: Metadata,
    pub bytes: BTreeMap<String, Vec<u8>>,
}
pub fn secure(path: &Path, private: bool) -> Result<()> {
    let metadata = fs::symlink_metadata(path)?;
    if metadata.file_type().is_symlink() {
        return Err("Symlinked installer input/managed path refused".into());
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        if metadata.uid() != 0 || metadata.mode() & if private { 0o077 } else { 0o022 } != 0 {
            return Err(
                "Installer input/managed file must be root-owned with restrictive permissions"
                    .into(),
            );
        }
    }
    Ok(())
}
pub fn read(path: &Path, limit: u64, private: bool) -> Result<Vec<u8>> {
    let resolved = if path.is_absolute() {
        path.to_path_buf()
    } else {
        std::env::current_dir()?.join(path)
    };
    let path = resolved.as_path();
    secure(path, private)?;
    for parent in path.ancestors().skip(1) {
        let metadata = fs::symlink_metadata(parent)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::MetadataExt;
            if metadata.file_type().is_symlink()
                || metadata.uid() != 0
                || (metadata.mode() & 0o022 != 0 && metadata.mode() & 0o1000 == 0)
            {
                return Err("Installer input has an untrusted parent".into());
            }
        }
        let _ = metadata;
    }
    read_regular(path, limit, 0)
}
pub fn read_regular(path: &Path, limit: u64, owner: u32) -> Result<Vec<u8>> {
    use std::io::Read;
    #[cfg(unix)]
    let file = fs::File::from(rustix::fs::open(
        path,
        rustix::fs::OFlags::RDONLY | rustix::fs::OFlags::NOFOLLOW | rustix::fs::OFlags::CLOEXEC,
        rustix::fs::Mode::empty(),
    )?);
    #[cfg(not(unix))]
    let file = fs::File::open(path)?;
    let metadata = file.metadata()?;
    if !metadata.is_file() {
        return Err("Expected a regular file".into());
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        if metadata.uid() != owner {
            return Err("File owner changed during read".into());
        }
    }
    let _ = owner;
    let mut bytes = Vec::new();
    file.take(limit + 1).read_to_end(&mut bytes)?;
    if bytes.len() as u64 > limit {
        return Err("Installer file exceeds its limit".into());
    }
    Ok(bytes)
}

pub fn load(directory: &Path, expected: Option<&str>) -> Result<Bundle> {
    let path = if directory.is_absolute() {
        directory.to_path_buf()
    } else {
        std::env::current_dir()?.join(directory)
    };
    let bytes = read(&path.join("bundle.json"), 8192, false)?;
    let actual = crate::xray::lifecycle::hash(&bytes);
    if let Some(expected) = expected
        && (expected.len() != 64
            || !expected.bytes().all(|byte| byte.is_ascii_hexdigit())
            || actual != expected.to_ascii_lowercase())
    {
        return Err("Bundle metadata checksum mismatch".into());
    }
    let mut files = BTreeMap::new();
    for name in BINARIES {
        files.insert(
            name.to_owned(),
            read(&path.join(name), 128 * 1024 * 1024, false)?,
        );
    }
    validate_bytes(&bytes, files)
}

pub(crate) fn validate_bytes(bytes: &[u8], files: BTreeMap<String, Vec<u8>>) -> Result<Bundle> {
    let metadata: Metadata = serde_json::from_slice(bytes)?;
    if metadata.format != 1
        || metadata.arch != std::env::consts::ARCH
        || metadata.files.len() != 3
        || metadata.version.is_empty()
        || !metadata
            .version
            .bytes()
            .all(|byte| byte.is_ascii_digit() || b".-".contains(&byte))
    {
        return Err("Unsupported bundle metadata/version/architecture".into());
    }
    if files.len() != BINARIES.len() {
        return Err("Unexpected bundle files".into());
    }
    for name in BINARIES {
        let digest = metadata.files.get(name).ok_or("Missing required binary")?;
        let content = files.get(name).ok_or("Missing required binary")?;
        if crate::xray::lifecycle::hash(content) != *digest
            || content.len() < 20
            || &content[..4] != b"\x7fELF"
            || content[4] != 2
            || content[5] != 1
        {
            return Err("Artifact checksum/ELF validation failed".into());
        }
        let machine = u16::from_le_bytes([content[18], content[19]]);
        if machine != if metadata.arch == "x86_64" { 62 } else { 183 } {
            return Err("ELF architecture mismatch".into());
        }
    }
    Ok(Bundle {
        metadata,
        bytes: files,
    })
}
pub fn cache_path(version: &str) -> PathBuf {
    PathBuf::from("/var/lib/panel-installer").join(format!("bundle-{version}"))
}
