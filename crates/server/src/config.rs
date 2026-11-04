use std::{env, error::Error, net::SocketAddr, path::PathBuf};

pub fn data_directory() -> PathBuf {
    env::var_os("PANEL_DATA_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("data"))
}

pub fn secure_cookie() -> Result<bool, Box<dyn Error>> {
    let public_http = public_http()?;
    match env::var("PANEL_DEV_HTTP") {
        Ok(value) if value == "1" && !public_http => Ok(false),
        Err(env::VarError::NotPresent) => Ok(!public_http),
        _ => Err("PANEL_DEV_HTTP must be 1 and cannot be combined with PANEL_PUBLIC_HTTP".into()),
    }
}

fn public_http() -> Result<bool, Box<dyn Error>> {
    match env::var("PANEL_PUBLIC_HTTP") {
        Ok(value) if value == "1" => Ok(true),
        Err(env::VarError::NotPresent) => Ok(false),
        _ => Err("PANEL_PUBLIC_HTTP, if set, must be exactly 1".into()),
    }
}

pub fn origin(address: SocketAddr) -> Result<String, Box<dyn Error>> {
    let value = match env::var("PANEL_ORIGIN") {
        Ok(value) => value,
        Err(env::VarError::NotPresent) if !public_http()? => format!("http://{address}"),
        Err(env::VarError::NotPresent) => {
            return Err("Public HTTP requires an explicit PANEL_ORIGIN".into());
        }
        Err(error) => return Err(error.into()),
    };
    validate_origin(&value, public_http()?)?;
    Ok(value)
}

fn validate_origin(value: &str, public_http: bool) -> Result<(), Box<dyn Error>> {
    let uri: axum::http::Uri = value.parse()?;
    let host = uri.host().ok_or("PANEL_ORIGIN must contain a host")?;
    let ip: std::net::IpAddr = host.trim_matches(['[', ']']).parse()?;
    if uri.scheme_str() != Some("http")
        || ip.is_unspecified()
        || (!public_http && !ip.is_loopback())
        || uri
            .authority()
            .is_none_or(|authority| authority.as_str().contains('@'))
        || uri
            .path_and_query()
            .is_some_and(|path| path.as_str() != "/")
        || value.ends_with('/')
    {
        return Err("PANEL_ORIGIN must be an HTTP IP origin without path or trailing slash".into());
    }
    Ok(())
}

pub fn bind_address() -> Result<SocketAddr, Box<dyn Error>> {
    listen_address(false)
}

pub fn listen_address(tls: bool) -> Result<SocketAddr, Box<dyn Error>> {
    let value = match env::var("PANEL_BIND") {
        Ok(value) => value,
        Err(env::VarError::NotPresent) => "127.0.0.1:8080".into(),
        Err(error) => return Err(error.into()),
    };
    if tls {
        Ok(value.parse()?)
    } else {
        validate_bind_address(&value, public_http()?)
    }
}

pub fn tls_paths() -> Result<Option<(PathBuf, PathBuf)>, Box<dyn Error>> {
    match (env::var_os("PANEL_TLS_CERT"), env::var_os("PANEL_TLS_KEY")) {
        (None, None) => Ok(None),
        (Some(cert), Some(key)) => {
            let metadata = std::fs::metadata(&key)?;
            if !metadata.is_file() {
                return Err("TLS key must be a regular private file".into());
            }
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                if metadata.permissions().mode() & 0o077 != 0 {
                    return Err(
                        "TLS private key must not be readable by group or other users".into(),
                    );
                }
            }
            Ok(Some((cert.into(), key.into())))
        }
        _ => Err("PANEL_TLS_CERT and PANEL_TLS_KEY must be provided together".into()),
    }
}

pub fn tls_origin(address: SocketAddr) -> Result<String, Box<dyn Error>> {
    let value = match env::var("PANEL_ORIGIN") {
        Ok(value) => value,
        Err(env::VarError::NotPresent) if address.ip().is_loopback() => {
            format!("https://{address}")
        }
        _ => return Err("HTTPS on a public bind requires an explicit PANEL_ORIGIN".into()),
    };
    let uri: axum::http::Uri = value.parse()?;
    if uri.scheme_str() != Some("https")
        || uri.host().is_none()
        || uri
            .authority()
            .is_none_or(|authority| authority.as_str().contains('@'))
        || uri
            .path_and_query()
            .is_some_and(|path| path.as_str() != "/")
        || value.ends_with('/')
    {
        return Err(
            "TLS PANEL_ORIGIN must be an HTTPS origin without path, credentials or query".into(),
        );
    }
    Ok(value)
}

fn validate_bind_address(value: &str, public_http: bool) -> Result<SocketAddr, Box<dyn Error>> {
    let address: SocketAddr = value.parse()?;
    if !public_http && !address.ip().is_loopback() {
        return Err("HTTP must bind to loopback; public access requires TLS".into());
    }
    Ok(address)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn http_bind_is_limited_to_loopback() {
        for value in ["127.0.0.1:8080", "[::1]:8080"] {
            assert!(validate_bind_address(value, false).is_ok());
        }
        for value in ["0.0.0.0:8080", "[::]:8080", "192.168.1.2:8080", "invalid"] {
            assert!(validate_bind_address(value, false).is_err());
        }
        assert!(validate_bind_address("0.0.0.0:8080", true).is_ok());
        assert!(validate_bind_address("invalid", true).is_err());
    }

    #[test]
    fn origin_must_be_an_exact_loopback_origin() {
        for value in ["http://127.0.0.1:8080", "http://[::1]:5173"] {
            assert!(validate_origin(value, false).is_ok(), "{value}");
        }
        for value in [
            "http://evil.example",
            "http://0.0.0.0:8080",
            "http://127.0.0.1:8080/",
            "http://127.0.0.1:8080/path",
            "http://user@127.0.0.1:8080",
            "http://127.0.0.1:8080?query",
        ] {
            assert!(validate_origin(value, false).is_err(), "{value}");
        }
        assert!(validate_origin("http://192.168.1.2:8080", true).is_ok());
        assert!(validate_origin("http://0.0.0.0:8080", true).is_err());
        assert!(validate_origin("http://example.com:8080", true).is_err());
    }
}
