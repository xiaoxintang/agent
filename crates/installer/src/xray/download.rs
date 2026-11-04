use panel_common::{XrayError, XrayVersion};
use sha2::{Digest, Sha256};
use std::io::{Cursor, Read};

pub const ARCHIVE_LIMIT: usize = 24 * 1024 * 1024;
pub const BINARY_LIMIT: u64 = 64 * 1024 * 1024;

pub async fn fetch(version: XrayVersion, arch: &str) -> Result<Vec<u8>, XrayError> {
    let (name, digest) = version
        .artifact(arch)
        .ok_or(XrayError::UnsupportedPlatform)?;
    let url = format!(
        "https://github.com/XTLS/Xray-core/releases/download/v{}/{name}",
        version.name()
    );
    let client = reqwest::Client::builder()
        .no_proxy()
        .https_only(true)
        .connect_timeout(std::time::Duration::from_secs(10))
        .timeout(std::time::Duration::from_secs(90))
        .redirect(reqwest::redirect::Policy::custom(|attempt| {
            if attempt.previous().len() < 4
                && attempt.url().scheme() == "https"
                && matches!(
                    attempt.url().host_str(),
                    Some("github.com" | "release-assets.githubusercontent.com")
                )
            {
                attempt.follow()
            } else {
                attempt.error("Untrusted Xray redirect")
            }
        }))
        .build()
        .map_err(|_| XrayError::DownloadFailed)?;
    let mut response = client
        .get(url)
        .send()
        .await
        .map_err(|_| XrayError::DownloadFailed)?
        .error_for_status()
        .map_err(|_| XrayError::DownloadFailed)?;
    if response
        .content_length()
        .is_some_and(|size| size > ARCHIVE_LIMIT as u64)
    {
        return Err(XrayError::VerificationFailed);
    }
    let mut archive = Vec::new();
    while let Some(chunk) = response
        .chunk()
        .await
        .map_err(|_| XrayError::DownloadFailed)?
    {
        if archive.len() + chunk.len() > ARCHIVE_LIMIT {
            return Err(XrayError::VerificationFailed);
        }
        archive.extend_from_slice(&chunk);
    }
    let binary = extract(&archive, digest)
        .inspect_err(|_| eprintln!("Xray archive integrity or bounded extraction failed"))?;
    eprintln!("Xray {} official archive verified", version.name());
    Ok(binary)
}

pub fn extract(bytes: &[u8], digest: &str) -> Result<Vec<u8>, XrayError> {
    if bytes.len() > ARCHIVE_LIMIT || hex::encode(Sha256::digest(bytes)) != digest {
        return Err(XrayError::VerificationFailed);
    }
    let mut zip =
        zip::ZipArchive::new(Cursor::new(bytes)).map_err(|_| XrayError::VerificationFailed)?;
    if zip.len() > 16 {
        return Err(XrayError::VerificationFailed);
    }
    let mut binary = zip
        .by_name("xray")
        .map_err(|_| XrayError::VerificationFailed)?;
    if binary.size() > BINARY_LIMIT
        || binary.is_dir()
        || binary
            .unix_mode()
            .is_some_and(|mode| mode & 0o170000 == 0o120000)
    {
        return Err(XrayError::VerificationFailed);
    }
    let mut output = Vec::new();
    binary
        .by_ref()
        .take(BINARY_LIMIT + 1)
        .read_to_end(&mut output)
        .map_err(|_| XrayError::VerificationFailed)?;
    if output.len() as u64 > BINARY_LIMIT || !output.starts_with(b"\x7fELF") {
        return Err(XrayError::VerificationFailed);
    }
    Ok(output)
}
