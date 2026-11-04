use panel_common::XrayError;
#[cfg(target_os = "linux")]
use panel_common::{XrayResult, xray::HelperRequest};
use serde::Deserialize;
#[cfg(target_os = "linux")]
use serde::Serialize;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Authorization {
    pub agent_uid: u32,
}

pub fn authorize(config: &Authorization, peer_uid: u32) -> Result<(), XrayError> {
    if config.agent_uid == 0 || peer_uid != config.agent_uid {
        return Err(XrayError::Unauthorized);
    }
    Ok(())
}

#[cfg(target_os = "linux")]
#[derive(Deserialize, Serialize)]
struct Receipt {
    request_hash: String,
    result: Option<XrayResult>,
    deadline_at: i64,
}

#[cfg(target_os = "linux")]
pub async fn serve_once() -> Result<(), XrayError> {
    use super::lifecycle::{self, Layout, Manager, Systemd};
    use std::{
        io::{BufRead, Read, Write},
        os::unix::net::UnixStream,
        time::Duration,
    };
    if rustix::process::geteuid().as_raw() != 0 {
        return Err(XrayError::Unauthorized);
    }
    let mut stream =
        UnixStream::from(rustix::io::dup(std::io::stdin()).map_err(|_| XrayError::Unauthorized)?);
    stream
        .set_read_timeout(Some(Duration::from_secs(5)))
        .map_err(|_| XrayError::Unauthorized)?;
    stream
        .set_write_timeout(Some(Duration::from_secs(5)))
        .map_err(|_| XrayError::Unauthorized)?;
    let peer =
        rustix::net::sockopt::socket_peercred(&stream).map_err(|_| XrayError::Unauthorized)?;
    for ancestor in std::path::Path::new("/etc/panel/xray-helper.json").ancestors() {
        lifecycle::owned(ancestor).map_err(|_| XrayError::Unauthorized)?;
    }
    let config: Authorization = serde_json::from_slice(&lifecycle::read(
        std::path::Path::new("/etc/panel/xray-helper.json"),
        1024,
    )?)
    .map_err(|_| XrayError::Unauthorized)?;
    authorize(&config, peer.uid.as_raw())?;
    let outcome = async {
        let mut input = String::new();
        std::io::BufReader::new((&stream).take(65537))
            .read_line(&mut input)
            .map_err(|_| XrayError::Unauthorized)?;
        if input.len() > 65536 || !input.ends_with('\n') {
            return Err(XrayError::Unauthorized);
        }
        let request: HelperRequest =
            serde_json::from_str(&input).map_err(|_| XrayError::Unauthorized)?;
        let id = uuid::Uuid::parse_str(&request.task_id)
            .map_err(|_| XrayError::Unauthorized)?
            .to_string();
        if request.deadline_at <= now() || request.deadline_at > now() + 300 {
            return Err(XrayError::Expired);
        }
        let layout = Layout::production();
        layout.validate()?;
        lifecycle::private_directory(&layout.control)?;
        let lock = lifecycle::private_file(&layout.control.join("requests.lock"))?;
        while lock.try_lock().is_err() {
            if request.deadline_at <= now() {
                return Err(XrayError::Expired);
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
        for entry in std::fs::read_dir(&layout.control).map_err(|_| XrayError::InvalidState)? {
            let entry = entry.map_err(|_| XrayError::InvalidState)?;
            let filename = entry.file_name();
            let Some(id) = filename
                .to_str()
                .and_then(|name| name.strip_suffix(".json"))
            else {
                continue;
            };
            if uuid::Uuid::parse_str(id).is_err() {
                continue;
            }
            let receipt: Receipt = serde_json::from_slice(&lifecycle::read(&entry.path(), 4096)?)
                .map_err(|_| XrayError::InvalidState)?;
            if receipt.deadline_at < now() - 86400 {
                std::fs::remove_file(entry.path()).map_err(|_| XrayError::InvalidState)?;
            }
        }
        let receipt_path = layout.control.join(format!("{id}.json"));
        let digest = lifecycle::hash(input.trim_end().as_bytes());
        let response = if receipt_path.exists() {
            let receipt: Receipt = serde_json::from_slice(&lifecycle::read(&receipt_path, 4096)?)
                .map_err(|_| XrayError::InvalidState)?;
            if receipt.request_hash != digest {
                return Err(XrayError::Unauthorized);
            }
            receipt
                .result
                .unwrap_or(XrayResult::Error(XrayError::Interrupted))
        } else {
            let receipt = Receipt {
                request_hash: digest.clone(),
                result: None,
                deadline_at: request.deadline_at,
            };
            lifecycle::atomic(
                &receipt_path,
                &serde_json::to_vec(&receipt).map_err(|_| XrayError::InvalidState)?,
                0o600,
            )?;
            let manager = Manager {
                layout,
                service: Systemd::production(),
                user_scope: false,
            };
            let result = match tokio::time::timeout(
                Duration::from_secs((request.deadline_at - now()).max(1) as u64),
                async {
                    match request.operation {
                        panel_common::xray::HelperOperation::Lifecycle(operation) => {
                            manager.operate(operation).await
                        }
                        panel_common::xray::HelperOperation::ApplyConfig(submission) => {
                            manager.apply_config(submission).await
                        }
                    }
                },
            )
            .await
            {
                Ok(Ok(report)) => XrayResult::Ok(report),
                Ok(Err(error)) => XrayResult::Error(error),
                Err(_) => XrayResult::Error(XrayError::Interrupted),
            };
            lifecycle::atomic(
                &receipt_path,
                &serde_json::to_vec(&Receipt {
                    request_hash: digest,
                    result: Some(result.clone()),
                    deadline_at: request.deadline_at,
                })
                .map_err(|_| XrayError::InvalidState)?,
                0o600,
            )?;
            result
        };
        Ok::<_, XrayError>(response)
    }
    .await;
    let response = outcome.unwrap_or_else(XrayResult::Error);
    let mut bytes = serde_json::to_vec(&response).map_err(|_| XrayError::InvalidState)?;
    bytes.push(b'\n');
    stream
        .write_all(&bytes)
        .map_err(|_| XrayError::InvalidState)?;
    Ok(())
}

#[cfg(not(target_os = "linux"))]
pub async fn serve_once() -> Result<(), XrayError> {
    Err(XrayError::UnsupportedPlatform)
}

#[cfg(target_os = "linux")]
fn now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("UTC clock")
        .as_secs() as i64
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn socket_authorization_only_accepts_the_configured_non_root_uid() {
        let config = Authorization { agent_uid: 999 };
        assert!(authorize(&config, 999).is_ok());
        assert_eq!(authorize(&config, 0), Err(XrayError::Unauthorized));
        assert_eq!(authorize(&config, 1001), Err(XrayError::Unauthorized));
        assert_eq!(
            authorize(&Authorization { agent_uid: 0 }, 0),
            Err(XrayError::Unauthorized)
        );
    }
}
