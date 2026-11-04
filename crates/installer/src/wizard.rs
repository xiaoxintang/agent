use crate::{
    cli::{Component, Input, InstallArgs, Settings},
    platform::Result,
};
use dialoguer::{Confirm, Input as Prompt, Password, Select};
use std::{io::IsTerminal, path::PathBuf};

pub fn install(args: &mut InstallArgs) -> Result<Input> {
    if !std::io::stdin().is_terminal() {
        return Err("Interactive install requires a TTY; use --non-interactive --config FILE --bundle DIR --bundle-sha256 HASH".into());
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
    let custom = Select::new()
        .with_prompt("Setup mode")
        .items([
            "Quick: fixed private paths",
            "Custom: bind/TLS/data and startup",
        ])
        .default(0)
        .interact()?
        == 1;
    let mut settings = Settings::default();
    let mut password = None;
    let mut token = None;
    if args.component.server() {
        settings.local_http = Confirm::new()
            .with_prompt(
                "Use local-only development HTTP? Public access requires your TLS certificate",
            )
            .default(false)
            .interact()?;
        settings.server_bind = Prompt::new()
            .with_prompt("Server bind IP:port")
            .default(
                if settings.local_http {
                    "127.0.0.1:8080"
                } else {
                    "0.0.0.0:8443"
                }
                .into(),
            )
            .interact_text()?;
        settings.server_origin = Prompt::new()
            .with_prompt("Exact browser origin (no trailing slash)")
            .default(
                if settings.local_http {
                    "http://127.0.0.1:8080"
                } else {
                    "https://panel.example:8443"
                }
                .into(),
            )
            .interact_text()?;
        if !settings.local_http {
            settings.tls_cert = Some(PathBuf::from(
                Prompt::<String>::new()
                    .with_prompt("TLS certificate PEM path")
                    .interact_text()?,
            ));
            settings.tls_key = Some(PathBuf::from(
                Prompt::<String>::new()
                    .with_prompt("TLS private key PEM path")
                    .interact_text()?,
            ));
        }
        if custom {
            settings.server_data = PathBuf::from(
                Prompt::<String>::new()
                    .with_prompt("Server data path under /var/lib/panel")
                    .default("/var/lib/panel/server".into())
                    .interact_text()?,
            );
        }
        settings.admin_username = Prompt::new()
            .with_prompt("Administrator username")
            .default("admin".into())
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
            .default("local node".into())
            .interact_text()?;
        if args.component == Component::Agent {
            settings.agent_server = Prompt::new()
                .with_prompt("Server WSS URL including /agent/v1/connect")
                .interact_text()?;
            token = Some(
                Password::new()
                    .with_prompt("One-time enrollment token from panel")
                    .interact()?,
            );
        } else {
            settings.agent_server = format!(
                "{}/agent/v1/connect",
                settings.server_origin.replacen("http", "ws", 1)
            );
        }
        let ca: String = Prompt::new()
            .with_prompt("Additional trusted CA PEM path (empty for public CA)")
            .allow_empty(true)
            .interact_text()?;
        if !ca.is_empty() {
            settings.agent_ca = Some(ca.into());
        }
        settings.install_xray = Confirm::new()
            .with_prompt("Install verified Xray 26.10.31 with no business listening ports?")
            .default(false)
            .interact()?;
    }
    if custom {
        settings.autostart = Confirm::new()
            .with_prompt("Enable services at boot?")
            .default(true)
            .interact()?;
    }
    if args.bundle.is_none() {
        args.bundle = Some(PathBuf::from(
            Prompt::<String>::new()
                .with_prompt("Verified bundle directory")
                .interact_text()?,
        ));
    }
    if args.bundle_sha256.is_none() {
        args.bundle_sha256 = Some(
            Prompt::new()
                .with_prompt("Trusted bundle.json SHA-256")
                .interact_text()?,
        );
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
