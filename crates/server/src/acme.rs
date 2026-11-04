use axum::{
    Router,
    extract::{Path as RoutePath, State},
    http::StatusCode,
    routing::get,
};
use axum_server::tls_rustls::RustlsConfig;
use instant_acme::{
    Account, AuthorizationStatus, ChallengeType, Identifier, LetsEncrypt, NewAccount, NewOrder,
    OrderStatus, RetryPolicy,
};
use serde::{Deserialize, Serialize};
use std::{
    error::Error,
    io::{Read, Write},
    path::{Path, PathBuf},
    sync::Arc,
    time::{Duration, SystemTime, UNIX_EPOCH},
};
use tokio::sync::RwLock;
use x509_parser::{extensions::GeneralName, pem::parse_x509_pem};

type Result<T> = std::result::Result<T, Box<dyn Error + Send + Sync>>;
type Challenge = Arc<RwLock<Option<(String, String)>>>;

#[derive(Serialize, Deserialize)]
struct Certificate {
    domain: String,
    certificate: String,
    private_key: String,
}

fn expiry(certificate: &Certificate, domain: &str) -> Result<i64> {
    if certificate.domain != domain {
        return Err("Cached ACME certificate belongs to another domain".into());
    }
    let (_, pem) = parse_x509_pem(certificate.certificate.as_bytes())
        .map_err(|_| "Invalid ACME certificate PEM")?;
    let parsed = pem.parse_x509().map_err(|_| "Invalid ACME certificate")?;
    if parsed.validity().not_before.timestamp() > now()? {
        return Err("ACME certificate is not yet valid".into());
    }
    let names = parsed
        .subject_alternative_name()
        .map_err(|_| "Invalid certificate names")?
        .ok_or("Missing certificate names")?;
    if !names
        .value
        .general_names
        .iter()
        .any(|name| matches!(name, GeneralName::DNSName(name) if name.eq_ignore_ascii_case(domain)))
    {
        return Err("ACME certificate does not contain the configured domain".into());
    }
    Ok(parsed.validity().not_after.timestamp())
}

fn now() -> Result<i64> {
    Ok(SystemTime::now().duration_since(UNIX_EPOCH)?.as_secs() as i64)
}

fn read_private(path: &Path) -> Result<Option<Vec<u8>>> {
    if !path.exists() {
        return Ok(None);
    }
    let mut file = panel_common::storage::private_file(path)?;
    let mut bytes = Vec::new();
    Read::by_ref(&mut file)
        .take(1024 * 1024 + 1)
        .read_to_end(&mut bytes)?;
    if bytes.len() > 1024 * 1024 {
        return Err("ACME data file exceeds limit".into());
    }
    Ok(Some(bytes))
}

fn write_private(path: &Path, bytes: &[u8]) -> Result<()> {
    let temporary = path.with_extension(format!("tmp-{}", uuid::Uuid::new_v4()));
    let result = (|| -> Result<()> {
        let mut file = panel_common::storage::private_file(&temporary)?;
        file.write_all(bytes)?;
        file.sync_all()?;
        std::fs::rename(&temporary, path)?;
        std::fs::File::open(path.parent().ok_or("Missing ACME data directory")?)?.sync_all()?;
        Ok(())
    })();
    if result.is_err() {
        let _ = std::fs::remove_file(temporary);
    }
    result
}

async fn response(
    State(challenge): State<Challenge>,
    RoutePath(token): RoutePath<String>,
) -> std::result::Result<String, StatusCode> {
    match &*challenge.read().await {
        Some((expected, value)) if token == *expected => Ok(value.clone()),
        _ => Err(StatusCode::NOT_FOUND),
    }
}

async fn account(directory: &Path) -> Result<Account> {
    let path = directory.join("account.json");
    if let Some(bytes) = read_private(&path)? {
        return Ok(Account::builder()?
            .from_credentials(serde_json::from_slice(&bytes)?)
            .await?);
    }
    let (account, credentials) = Account::builder()?
        .create(
            &NewAccount {
                contact: &[],
                terms_of_service_agreed: true,
                only_return_existing: false,
            },
            LetsEncrypt::Production.url().to_owned(),
            None,
        )
        .await?;
    write_private(&path, &serde_json::to_vec(&credentials)?)?;
    Ok(account)
}

async fn issue(domain: &str, directory: &Path, challenge: &Challenge) -> Result<Certificate> {
    let account = account(directory).await?;
    let identifiers = [Identifier::Dns(domain.to_owned())];
    let mut order = account.new_order(&NewOrder::new(&identifiers)).await?;
    {
        let mut authorizations = order.authorizations();
        while let Some(result) = authorizations.next().await {
            let mut authorization = result?;
            match authorization.status {
                AuthorizationStatus::Valid => continue,
                AuthorizationStatus::Pending => (),
                _ => return Err("ACME authorization is not pending".into()),
            }
            let mut http = authorization
                .challenge(ChallengeType::Http01)
                .ok_or("ACME HTTP-01 challenge unavailable")?;
            *challenge.write().await = Some((
                http.token.clone(),
                http.key_authorization().as_str().to_owned(),
            ));
            http.set_ready().await?;
        }
    }
    let policy = RetryPolicy::default().timeout(Duration::from_secs(120));
    let status = order.poll_ready(&policy).await;
    *challenge.write().await = None;
    if status? != OrderStatus::Ready {
        return Err("ACME order did not become ready".into());
    }
    let private_key = order.finalize().await?;
    let certificate = order.poll_certificate(&policy).await?;
    let result = Certificate {
        domain: domain.to_owned(),
        certificate,
        private_key,
    };
    if expiry(&result, domain)? <= now()? {
        return Err("ACME returned an expired certificate".into());
    }
    Ok(result)
}

pub async fn start(domain: String, data: &Path) -> Result<RustlsConfig> {
    let directory = data.join("acme");
    panel_common::storage::private_directory(&directory)?;
    let challenge: Challenge = Arc::new(RwLock::new(None));
    let listener = tokio::net::TcpListener::bind("0.0.0.0:80").await?;
    let router = Router::new()
        .route("/.well-known/acme-challenge/{token}", get(response))
        .with_state(challenge.clone());
    tokio::spawn(async move {
        if let Err(error) = axum::serve(listener, router).await {
            eprintln!("ACME challenge listener failed: {error}");
        }
    });
    let certificate_path = directory.join("certificate.json");
    let mut cached = read_private(&certificate_path)?
        .and_then(|bytes| serde_json::from_slice::<Certificate>(&bytes).ok())
        .filter(|certificate| {
            expiry(certificate, &domain)
                .is_ok_and(|expires| now().is_ok_and(|current| expires > current))
        });
    if let Some(certificate) = &cached
        && RustlsConfig::from_pem(
            certificate.certificate.as_bytes().to_vec(),
            certificate.private_key.as_bytes().to_vec(),
        )
        .await
        .is_err()
    {
        cached = None;
    }
    let certificate = match cached {
        Some(certificate) => certificate,
        None => {
            let certificate = issue(&domain, &directory, &challenge).await?;
            RustlsConfig::from_pem(
                certificate.certificate.as_bytes().to_vec(),
                certificate.private_key.as_bytes().to_vec(),
            )
            .await?;
            write_private(&certificate_path, &serde_json::to_vec(&certificate)?)?;
            certificate
        }
    };
    let tls = RustlsConfig::from_pem(
        certificate.certificate.as_bytes().to_vec(),
        certificate.private_key.as_bytes().to_vec(),
    )
    .await?;
    let reload = tls.clone();
    tokio::spawn(renew(
        domain,
        directory,
        certificate_path,
        challenge,
        reload,
        expiry(&certificate, &certificate.domain)?,
    ));
    Ok(tls)
}

async fn renew(
    domain: String,
    directory: PathBuf,
    path: PathBuf,
    challenge: Challenge,
    tls: RustlsConfig,
    mut expires: i64,
) {
    loop {
        let remaining = expires
            .saturating_sub(now().unwrap_or(0))
            .saturating_sub(30 * 24 * 60 * 60)
            .max(0);
        tokio::time::sleep(Duration::from_secs((remaining as u64).min(24 * 60 * 60))).await;
        if remaining > 24 * 60 * 60 {
            continue;
        }
        let result = async {
            let certificate = issue(&domain, &directory, &challenge).await?;
            let new_expiry = expiry(&certificate, &domain)?;
            RustlsConfig::from_pem(
                certificate.certificate.as_bytes().to_vec(),
                certificate.private_key.as_bytes().to_vec(),
            )
            .await?;
            write_private(&path, &serde_json::to_vec(&certificate)?)?;
            tls.reload_from_pem(
                certificate.certificate.into_bytes(),
                certificate.private_key.into_bytes(),
            )
            .await?;
            Ok::<i64, Box<dyn Error + Send + Sync>>(new_expiry)
        }
        .await;
        match result {
            Ok(new_expiry) => {
                expires = new_expiry;
                tokio::time::sleep(Duration::from_secs(60 * 60)).await;
            }
            Err(error) => {
                *challenge.write().await = None;
                eprintln!("ACME renewal failed; retrying in one hour: {error}");
                tokio::time::sleep(Duration::from_secs(60 * 60)).await;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn certificate_cache_checks_domain_and_expiry() {
        let generated =
            rcgen::generate_simple_self_signed(vec!["panel.example.com".into()]).unwrap();
        let certificate = Certificate {
            domain: "panel.example.com".into(),
            certificate: generated.cert.pem(),
            private_key: generated.signing_key.serialize_pem(),
        };
        assert!(expiry(&certificate, "panel.example.com").unwrap() > now().unwrap());
        assert!(expiry(&certificate, "other.example.com").is_err());
        let wrong_name = Certificate {
            domain: "other.example.com".into(),
            ..certificate
        };
        assert!(expiry(&wrong_name, "other.example.com").is_err());
    }

    #[test]
    fn private_cache_is_persisted_and_replaced() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("certificate.json");
        assert!(read_private(&path).unwrap().is_none());
        write_private(&path, b"first").unwrap();
        write_private(&path, b"second").unwrap();
        assert_eq!(read_private(&path).unwrap().unwrap(), b"second");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                std::fs::metadata(path).unwrap().permissions().mode() & 0o777,
                0o600
            );
        }
    }

    #[tokio::test]
    async fn challenge_only_serves_exact_token() {
        let challenge = Arc::new(RwLock::new(Some(("token".into(), "authorization".into()))));
        assert_eq!(
            response(State(challenge.clone()), RoutePath("token".into()))
                .await
                .unwrap(),
            "authorization"
        );
        assert_eq!(
            response(State(challenge), RoutePath("wrong".into()))
                .await
                .unwrap_err(),
            StatusCode::NOT_FOUND
        );
    }
}
