use clap::Parser;
use panel_installer::{
    bundle,
    cli::{Action, Cli, InstallArgs},
    deployment,
    platform::Result,
    wizard,
};
use std::io::IsTerminal;

#[tokio::main]
async fn main() -> Result<()> {
    panel_common::storage::disable_core_dumps()?;
    let cli = Cli::parse();
    if let Some(status) = deployment::maintenance_worker()? {
        std::process::exit(status.code().unwrap_or(1));
    }
    let action = match cli.command {
        Some(action) => action,
        None => {
            if !std::io::stdin().is_terminal() {
                use clap::CommandFactory;
                Cli::command().print_help()?;
                return Ok(());
            }
            let selected = dialoguer::Select::new()
                .with_prompt("Panel maintenance")
                .items(["Install", "Upgrade", "Repair", "Uninstall", "Status"])
                .default(0)
                .interact()?;
            match selected {
                0 => Action::Install(InstallArgs {
                    component: panel_installer::cli::Component::All,
                    bundle: None,
                    bundle_sha256: None,
                    non_interactive: false,
                    config: None,
                }),
                1 => {
                    let directory: String = dialoguer::Input::new()
                        .with_prompt("Verified bundle directory")
                        .interact_text()?;
                    let digest: String = dialoguer::Input::new()
                        .with_prompt("Trusted bundle.json SHA-256")
                        .interact_text()?;
                    let version: String = dialoguer::Input::new()
                        .with_prompt("Exact version")
                        .interact_text()?;
                    Action::Upgrade(panel_installer::cli::UpgradeArgs {
                        component: panel_installer::cli::Component::All,
                        bundle: directory.into(),
                        bundle_sha256: digest,
                        version,
                    })
                }
                2 => Action::Repair {
                    component: panel_installer::cli::Component::All,
                },
                3 => {
                    let purge = dialoguer::Confirm::new()
                        .with_prompt("Purge Panel data permanently?")
                        .default(false)
                        .interact()?;
                    let confirm = if purge {
                        Some(
                            dialoguer::Input::<String>::new()
                                .with_prompt("Type PURGE PANEL DATA")
                                .interact_text()?,
                        )
                    } else {
                        None
                    };
                    let remove_xray = dialoguer::Confirm::new()
                        .with_prompt("Remove the owned Xray service/binary too?")
                        .default(false)
                        .interact()?;
                    Action::Uninstall {
                        component: panel_installer::cli::Component::All,
                        purge,
                        remove_xray,
                        confirm,
                    }
                }
                _ => Action::Status,
            }
        }
    };
    match action {
        Action::XrayHelper => panel_installer::xray::helper::serve_once()
            .await
            .map_err(|error| format!("Xray helper rejected or failed: {error:?}"))?,
        Action::Install(mut args) => {
            panel_installer::platform::detect()?;
            let input = if args.non_interactive {
                let config = args
                    .config
                    .as_deref()
                    .ok_or("--non-interactive requires --config FILE")?;
                serde_json::from_slice(&bundle::read(config, 16384, true)?)?
            } else if let Some(config) = &args.config {
                serde_json::from_slice(&bundle::read(config, 16384, true)?)?
            } else {
                wizard::install(&mut args)?
            };
            deployment::validate(&input.settings, args.component)?;
            if !args.non_interactive {
                wizard::confirm_plan(args.component, &input.settings)?;
            }
            let artifacts = bundle::load(
                args.bundle.as_deref().ok_or("--bundle is required")?,
                args.bundle_sha256
                    .as_deref()
                    .ok_or("--bundle-sha256 is required")?,
            )?;
            deployment::install(input, args.component, artifacts).await?;
        }
        Action::Upgrade(args) => {
            deployment::upgrade(
                args.component,
                bundle::load(&args.bundle, &args.bundle_sha256)?,
                &args.version,
            )
            .await?
        }
        Action::Repair { component } => deployment::repair(component).await?,
        Action::Uninstall {
            component,
            purge,
            remove_xray,
            confirm,
        } => deployment::uninstall(component, purge, remove_xray, confirm.as_deref()).await?,
        Action::Status => deployment::report().await?,
    }
    Ok(())
}
