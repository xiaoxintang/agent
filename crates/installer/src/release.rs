use crate::{bundle, platform::Result};
use std::{collections::BTreeMap, io::Read, time::Duration};

const ARCHIVE_LIMIT: usize = 150 * 1024 * 1024;
const FILE_LIMIT: u64 = 128 * 1024 * 1024;

async fn download(client: &reqwest::Client, url: &str, limit: usize) -> Result<Vec<u8>> {
    let mut response = client.get(url).send().await?.error_for_status()?;
    if response
        .content_length()
        .is_some_and(|size| size > limit as u64)
    {
        return Err("Release download exceeds size limit".into());
    }
    let mut bytes = Vec::new();
    while let Some(chunk) = response.chunk().await? {
        if bytes.len() + chunk.len() > limit {
            return Err("Release download exceeds size limit".into());
        }
        bytes.extend_from_slice(&chunk);
    }
    Ok(bytes)
}

fn release_version(tag: &str) -> Result<&str> {
    let version = tag.strip_prefix('v').ok_or("Invalid release tag")?;
    let parts: Vec<_> = version.split('.').collect();
    if parts.len() != 3
        || parts
            .iter()
            .any(|part| part.is_empty() || !part.bytes().all(|byte| byte.is_ascii_digit()))
    {
        return Err("Invalid release tag".into());
    }
    Ok(version)
}

fn extract(archive: &[u8], version: &str) -> Result<bundle::Bundle> {
    let decoder = flate2::read::GzDecoder::new(archive);
    let mut tar = tar::Archive::new(decoder);
    let mut files = BTreeMap::new();
    for entry in tar.entries()? {
        let mut entry = entry?;
        if !entry.header().entry_type().is_file() {
            return Err("Release archive contains a non-file entry".into());
        }
        let name = entry
            .path()?
            .to_str()
            .ok_or("Invalid archive path")?
            .to_owned();
        if name != "bundle.json" && !bundle::BINARIES.contains(&name.as_str()) {
            return Err("Unexpected release archive entry".into());
        }
        let limit = if name == "bundle.json" {
            8192
        } else {
            FILE_LIMIT
        };
        if entry.size() > limit || files.contains_key(&name) {
            return Err("Oversized or duplicate release archive entry".into());
        }
        let mut content = Vec::new();
        entry.read_to_end(&mut content)?;
        files.insert(name, content);
    }
    let metadata = files
        .remove("bundle.json")
        .ok_or("Missing bundle metadata")?;
    let bundle = bundle::validate_bytes(&metadata, files)?;
    if bundle.metadata.version != version {
        return Err("Release tag and bundle version differ".into());
    }
    Ok(bundle)
}

pub async fn latest() -> Result<bundle::Bundle> {
    let client = reqwest::Client::builder()
        .no_proxy()
        .https_only(true)
        .user_agent("panel-installer")
        .connect_timeout(Duration::from_secs(15))
        .timeout(Duration::from_secs(300))
        .redirect(reqwest::redirect::Policy::custom(|attempt| {
            if attempt.previous().len() < 4
                && attempt.url().scheme() == "https"
                && matches!(
                    attempt.url().host_str(),
                    Some("api.github.com" | "github.com" | "release-assets.githubusercontent.com")
                )
            {
                attempt.follow()
            } else {
                attempt.error("Untrusted release redirect")
            }
        }))
        .build()?;
    eprintln!("Checking the latest GitHub release...");
    let response = download(
        &client,
        "https://api.github.com/repos/xiaoxintang/agent/releases/latest",
        1024 * 1024,
    )
    .await?;
    let release: serde_json::Value = serde_json::from_slice(&response)?;
    let tag = release["tag_name"].as_str().ok_or("Missing release tag")?;
    let version = release_version(tag)?;
    let url = format!(
        "https://github.com/xiaoxintang/agent/releases/download/{tag}/panel-linux-{}-{tag}.tar.gz",
        std::env::consts::ARCH
    );
    eprintln!("Downloading release {tag}; signature verification is not enabled.");
    extract(&download(&client, &url, ARCHIVE_LIMIT).await?, version)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn archive(extra: Option<(&str, &[u8])>) -> Vec<u8> {
        let mut binary = vec![0; 20];
        binary[..6].copy_from_slice(b"\x7fELF\x02\x01");
        let machine: u16 = if std::env::consts::ARCH == "x86_64" {
            62
        } else {
            183
        };
        binary[18..20].copy_from_slice(&machine.to_le_bytes());
        let digest = crate::xray::lifecycle::hash(&binary);
        let metadata = serde_json::json!({
            "format": 1, "version": "1.2.3", "arch": std::env::consts::ARCH,
            "files": {"panel-server": digest, "panel-agent": digest, "panel-installer": digest}
        })
        .to_string();
        let encoder = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
        let mut tar = tar::Builder::new(encoder);
        for (name, content) in [
            ("bundle.json", metadata.as_bytes()),
            ("panel-server", binary.as_slice()),
            ("panel-agent", binary.as_slice()),
            ("panel-installer", binary.as_slice()),
        ]
        .into_iter()
        .chain(extra)
        {
            let mut header = tar::Header::new_gnu();
            header.set_size(content.len() as u64);
            header.set_mode(0o644);
            header.set_cksum();
            tar.append_data(&mut header, name, content).unwrap();
        }
        let mut encoder = tar.into_inner().unwrap();
        encoder.flush().unwrap();
        encoder.finish().unwrap()
    }

    #[test]
    fn verifies_release_archive_and_version() {
        let bytes = archive(None);
        assert!(extract(&bytes, "1.2.3").is_ok());
        assert!(extract(&bytes, "1.2.4").is_err());
        assert!(extract(&archive(Some(("panel-agent", b"duplicate"))), "1.2.3").is_err());
        assert!(extract(&archive(Some(("unexpected", b"data"))), "1.2.3").is_err());
    }

    #[test]
    fn accepts_only_stable_version_tags() {
        assert_eq!(release_version("v1.2.3").unwrap(), "1.2.3");
        for tag in ["v1.2", "v1.2.3-beta", "v1.2.3/other", "1.2.3"] {
            assert!(release_version(tag).is_err());
        }
    }
}
