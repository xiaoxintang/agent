use crate::{
    cli::{Component, Input, InstallArgs, Settings},
    platform::Result,
};
use dialoguer::{Confirm, Input as Prompt, Password, Select};
use std::{io::IsTerminal, path::PathBuf};

async fn public_ipv4() -> Option<std::net::Ipv4Addr> {
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(3))
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .ok()?;
    let mut response = client.get("https://api.ipify.org").send().await.ok()?;
    if !response.status().is_success() {
        return None;
    }
    let mut body = Vec::new();
    while let Some(chunk) = response.chunk().await.ok()? {
        if body.len() + chunk.len() > 64 {
            return None;
        }
        body.extend_from_slice(&chunk);
    }
    parse_public_ipv4(&body)
}

fn parse_public_ipv4(body: &[u8]) -> Option<std::net::Ipv4Addr> {
    let ip: std::net::Ipv4Addr = std::str::from_utf8(body).ok()?.trim().parse().ok()?;
    if ip.is_private()
        || ip.is_loopback()
        || ip.is_link_local()
        || ip.is_unspecified()
        || ip.is_broadcast()
        || ip.is_multicast()
    {
        return None;
    }
    Some(ip)
}

fn http_origin_default(bind: std::net::SocketAddr) -> Option<String> {
    let preferred = || {
        let socket = std::net::UdpSocket::bind("0.0.0.0:0").ok()?;
        socket.connect("192.0.2.1:80").ok()?;
        match socket.local_addr().ok()?.ip() {
            std::net::IpAddr::V4(ip) if !ip.is_unspecified() && !ip.is_loopback() => Some(ip),
            _ => None,
        }
    };
    http_origin_from(bind, preferred)
}

fn http_origin_from(
    bind: std::net::SocketAddr,
    preferred: impl FnOnce() -> Option<std::net::Ipv4Addr>,
) -> Option<String> {
    let ip = match bind.ip() {
        std::net::IpAddr::V4(ip) if !ip.is_unspecified() => ip,
        std::net::IpAddr::V4(_) => preferred()?,
        std::net::IpAddr::V6(ip) if ip.is_unspecified() => preferred()?,
        _ => return None,
    };
    Some(if bind.port() == 80 {
        format!("http://{ip}")
    } else {
        format!("http://{ip}:{}", bind.port())
    })
}

pub async fn install(args: &mut InstallArgs) -> Result<Input> {
    if !std::io::stdin().is_terminal() {
        return Err(
            "Interactive install requires a TTY; use --non-interactive --config FILE --bundle DIR"
                .into(),
        );
    }
    let component = Select::new()
        .with_prompt("Component")
        .items(["Server + Agent", "Server", "Agent"])
        .default(match args.component {
            Component::All => 0,
            Component::Server => 1,
            Component::Agent => 2,
        })
        .interact()?;
    args.component = [Component::All, Component::Server, Component::Agent][component];
    let retained = crate::deployment::retained_settings(args.component)?;
    if retained.is_some() {
        eprintln!("Retained installation settings found; using them as defaults.");
    }
    let custom = Select::new()
        .with_prompt("Setup mode")
        .items([
            "Quick: default data path and automatic startup",
            "Custom: choose data path and startup",
        ])
        .default(0)
        .interact()?
        == 1;
    let mut settings = retained.clone().unwrap_or_default();
    let mut password = None;
    let mut token = None;
    if args.component.server() {
        settings.local_http = !Confirm::new()
            .with_prompt("Enable HTTPS? No allows unencrypted IP access (passwords and sessions can be intercepted)")
            .default(retained.as_ref().is_none_or(|saved| !saved.local_http))
            .interact()?;
        if settings.local_http {
            settings.tls_cert = None;
            settings.tls_key = None;
            settings.acme_domain = None;
        }
        settings.server_bind = Prompt::new()
            .with_prompt("Server bind IP:port")
            .default(if let Some(saved) = &retained {
                saved.server_bind.clone()
            } else if settings.local_http {
                "0.0.0.0:8080".into()
            } else {
                "0.0.0.0:8443".into()
            })
            .interact_text()?;
        let server_bind = settings.server_bind.parse::<std::net::SocketAddr>()?;
        let server_port = server_bind.port();
        let default_origin = if server_port == 443 {
            "https://panel.example".into()
        } else {
            format!("https://panel.example:{server_port}")
        };
        settings.server_origin = if settings.local_http {
            let prompt = Prompt::new().with_prompt("Browser HTTP origin (NAT public IP or external port may differ; enter the actual browser address)");
            let default = if let Some(saved) = &retained
                && saved.local_http
                && saved.server_bind == settings.server_bind
            {
                Some(saved.server_origin.clone())
            } else if server_bind.ip().is_loopback() {
                http_origin_default(server_bind)
            } else {
                eprintln!("Looking up public IPv4 address (up to 3 seconds)...");
                public_ipv4()
                    .await
                    .and_then(|ip| {
                        http_origin_from(
                            std::net::SocketAddr::from(([0, 0, 0, 0], server_port)),
                            || Some(ip),
                        )
                    })
                    .or_else(|| http_origin_default(server_bind))
            };
            match default {
                Some(default) => prompt.default(default).interact_text()?,
                None => prompt.interact_text()?,
            }
        } else {
            Prompt::new()
                .with_prompt("Exact browser origin (no trailing slash)")
                .default(
                    retained
                        .as_ref()
                        .filter(|saved| !saved.local_http)
                        .map_or(default_origin, |saved| saved.server_origin.clone()),
                )
                .interact_text()?
        };
        if !settings.local_http {
            if Confirm::new()
                .with_prompt("Automatically obtain and renew a Let's Encrypt certificate?")
                .default(
                    retained
                        .as_ref()
                        .is_none_or(|saved| saved.acme_domain.is_some()),
                )
                .interact()?
            {
                settings.tls_cert = None;
                settings.tls_key = None;
                let origin = url::Url::parse(&settings.server_origin)?;
                settings.acme_domain = Some(
                    origin
                        .host_str()
                        .ok_or("HTTPS origin needs a domain")?
                        .to_owned(),
                );
            } else {
                settings.acme_domain = None;
                settings.tls_cert = Some(
                    Prompt::<String>::new()
                        .with_prompt("TLS certificate PEM path")
                        .with_initial_text(
                            settings
                                .tls_cert
                                .as_ref()
                                .map_or_else(String::new, |path| path.display().to_string()),
                        )
                        .interact_text()?
                        .into(),
                );
                settings.tls_key = Some(
                    Prompt::<String>::new()
                        .with_prompt("TLS private key PEM path")
                        .with_initial_text(
                            settings
                                .tls_key
                                .as_ref()
                                .map_or_else(String::new, |path| path.display().to_string()),
                        )
                        .interact_text()?
                        .into(),
                );
            }
        }
        if custom {
            settings.server_data = PathBuf::from(
                Prompt::<String>::new()
                    .with_prompt("Server data path under /var/lib/panel")
                    .default(settings.server_data.display().to_string())
                    .interact_text()?,
            );
        }
        settings.admin_username = Prompt::new()
            .with_prompt("Administrator username")
            .default(settings.admin_username.clone())
            .interact_text()?;
        password = Some(
            Password::new()
                .with_prompt(
                    "First administrator password (12-128 bytes); existing account is preserved",
                )
                .with_confirmation("Confirm password", "Passwords do not match")
                .interact()?,
        );
    }
    if args.component.agent() {
        settings.agent_name = Prompt::new()
            .with_prompt("Node display name (used when creating a local node)")
            .default(settings.agent_name.clone())
            .interact_text()?;
        if args.component == Component::Agent {
            settings.agent_server = Prompt::new()
                .with_prompt("Server WSS URL including /agent/v1/connect")
                .with_initial_text(
                    retained
                        .as_ref()
                        .map_or_else(String::new, |saved| saved.agent_server.clone()),
                )
                .interact_text()?;
            token = Some(
                Password::new()
                    .with_prompt("One-time enrollment token from panel")
                    .interact()?,
            );
        } else {
            settings.agent_server = format!(
                "{}/agent/v1/connect",
                if settings.local_http {
                    crate::deployment::local_http_origin(&settings)?.replacen("http", "ws", 1)
                } else {
                    settings.server_origin.replacen("http", "ws", 1)
                }
            );
        }
        if args.component == Component::Agent || !settings.local_http {
            let ca: String = Prompt::new()
                .with_prompt("Additional trusted CA PEM path (empty for public CA)")
                .with_initial_text(
                    settings
                        .agent_ca
                        .as_ref()
                        .map_or_else(String::new, |path| path.display().to_string()),
                )
                .allow_empty(true)
                .interact_text()?;
            settings.agent_ca = if ca.is_empty() { None } else { Some(ca.into()) };
        }
        settings.install_xray = Confirm::new()
            .with_prompt("Install verified Xray 26.10.31 with no business listening ports?")
            .default(settings.install_xray)
            .interact()?;
    }
    if custom {
        settings.autostart = Confirm::new()
            .with_prompt("Enable services at boot?")
            .default(settings.autostart)
            .interact()?;
    }
    if args.bundle.is_none() {
        args.bundle = Some(PathBuf::from(
            Prompt::<String>::new()
                .with_prompt("Verified bundle directory")
                .interact_text()?,
        ));
    }
    Ok(Input {
        settings,
        admin_password: password,
        enrollment_token: token,
    })
}
pub fn confirm_plan(component: Component, settings: &Settings) -> Result<()> {
    println!(
        "Install {component:?}; Server bind {}, origin {}, data {}; Agent server {}; autostart {}; Xray {}. Existing unowned files will be refused.",
        settings.server_bind,
        settings.server_origin,
        settings.server_data.display(),
        settings.agent_server,
        settings.autostart,
        settings.install_xray
    );
    if !Confirm::new()
        .with_prompt("Apply this plan?")
        .default(false)
        .interact()?
    {
        return Err("Installation canceled".into());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{http_origin_default, http_origin_from, parse_public_ipv4};

    #[test]
    fn public_ipv4_response_requires_a_usable_ipv4() {
        assert_eq!(
            parse_public_ipv4(b"8.8.8.8\n"),
            Some("8.8.8.8".parse().unwrap())
        );
        for response in ["invalid", "10.0.0.1", "127.0.0.1", "::1", "0.0.0.0"] {
            assert_eq!(parse_public_ipv4(response.as_bytes()), None);
        }
    }

    #[test]
    fn http_default_uses_explicit_ipv4_and_port() {
        assert_eq!(
            http_origin_default("192.168.1.5:8080".parse().unwrap()).as_deref(),
            Some("http://192.168.1.5:8080")
        );
        assert_eq!(
            http_origin_default("192.168.1.5:80".parse().unwrap()).as_deref(),
            Some("http://192.168.1.5")
        );
        assert!(http_origin_default("[::1]:8080".parse().unwrap()).is_none());
        assert_eq!(
            http_origin_from("0.0.0.0:8080".parse().unwrap(), || Some(
                "192.168.1.7".parse().unwrap()
            ))
            .as_deref(),
            Some("http://192.168.1.7:8080")
        );
        assert!(http_origin_from("0.0.0.0:8080".parse().unwrap(), || None).is_none());
        assert_eq!(
            http_origin_from("[::]:8080".parse().unwrap(), || Some(
                "192.168.1.7".parse().unwrap()
            ))
            .as_deref(),
            Some("http://192.168.1.7:8080")
        );
    }
}
