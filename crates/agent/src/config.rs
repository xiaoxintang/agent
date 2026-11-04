use crate::AgentError;
use rustls::pki_types::{CertificateDer, pem::PemObject};
use std::{path::Path, sync::Arc};
use tokio_tungstenite::Connector;

pub fn server_url(value: &str, allow_local_ws: bool) -> Result<String, AgentError> {
    let url = url::Url::parse(value).map_err(|_| AgentError::Configuration)?;
    if !url.username().is_empty()
        || url.password().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
        || url.path() != "/agent/v1/connect"
    {
        return Err(AgentError::Configuration);
    }
    match url.scheme() {
        "wss" if url.host_str().is_some() => (),
        "ws" if allow_local_ws
            && url
                .host_str()
                .and_then(|host| {
                    host.trim_matches(['[', ']'])
                        .parse::<std::net::IpAddr>()
                        .ok()
                })
                .is_some_and(|ip| ip.is_loopback()) => {}
        _ => return Err(AgentError::Configuration),
    }
    Ok(url.into())
}

pub fn connector(ca_cert: Option<&Path>) -> Result<Connector, AgentError> {
    let mut roots = rustls::RootCertStore::empty();
    roots.extend(webpki_roots::TLS_SERVER_ROOTS.iter().cloned());
    if let Some(path) = ca_cert {
        let pem = std::fs::read(path).map_err(|_| AgentError::Configuration)?;
        let certs = CertificateDer::pem_slice_iter(&pem)
            .collect::<Result<Vec<_>, _>>()
            .map_err(|_| AgentError::Configuration)?;
        if certs.is_empty() {
            return Err(AgentError::Configuration);
        }
        for cert in certs {
            roots.add(cert).map_err(|_| AgentError::Configuration)?;
        }
    }
    let config = rustls::ClientConfig::builder()
        .with_root_certificates(roots)
        .with_no_client_auth();
    Ok(Connector::Rustls(Arc::new(config)))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn only_wss_or_explicit_literal_loopback_ws_are_allowed() {
        assert!(server_url("wss://panel.example/agent/v1/connect", false).is_ok());
        assert!(server_url("ws://127.0.0.1:8080/agent/v1/connect", true).is_ok());
        for value in [
            "ws://127.0.0.1/agent/v1/connect",
            "wss://user:secret@panel.example/agent/v1/connect",
            "wss://panel.example/agent/v1/connect?token=secret",
            "https://panel.example/agent/v1/connect",
            "wss://panel.example/other",
            "wss://panel.example/agent/v1/connect#secret",
        ] {
            assert!(server_url(value, false).is_err());
        }
        for value in [
            "ws://evil.example/agent/v1/connect",
            "ws://localhost/agent/v1/connect",
            "ws://192.168.1.2/agent/v1/connect",
        ] {
            assert!(server_url(value, true).is_err());
        }
    }
}
