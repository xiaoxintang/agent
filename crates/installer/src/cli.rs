use clap::{Args, Parser, Subcommand, ValueEnum};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, ValueEnum, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Component {
    Server,
    Agent,
    All,
}
impl Component {
    pub fn server(self) -> bool {
        matches!(self, Self::Server | Self::All)
    }
    pub fn agent(self) -> bool {
        matches!(self, Self::Agent | Self::All)
    }
}
#[derive(Parser)]
#[command(
    version,
    about = "Panel installer: owned Linux/systemd deployment and fixed Xray helper"
)]
pub struct Cli {
    #[command(subcommand)]
    pub command: Option<Action>,
}
#[derive(Subcommand)]
pub enum Action {
    Install(InstallArgs),
    Upgrade(UpgradeArgs),
    Repair {
        #[arg(long, value_enum, default_value = "all")]
        component: Component,
    },
    Uninstall {
        #[arg(long, value_enum, default_value = "all")]
        component: Component,
        #[arg(long)]
        purge: bool,
        #[arg(long)]
        remove_xray: bool,
        #[arg(long)]
        confirm: Option<String>,
    },
    Status,
    XrayHelper,
}
#[derive(Args)]
pub struct InstallArgs {
    #[arg(long, value_enum, default_value = "all")]
    pub component: Component,
    /// Directory produced by scripts/package.py; trust its checksum through an authenticated channel.
    #[arg(long)]
    pub bundle: Option<PathBuf>,
    #[arg(long)]
    pub bundle_sha256: Option<String>,
    #[arg(long)]
    pub non_interactive: bool,
    /// Root-owned 0600 JSON. Passwords and enrollment tokens never appear in process arguments.
    #[arg(long)]
    pub config: Option<PathBuf>,
}
#[derive(Args)]
pub struct UpgradeArgs {
    #[arg(long, value_enum)]
    pub component: Option<Component>,
    #[arg(long, requires = "version")]
    pub bundle: Option<PathBuf>,
    #[arg(long, requires = "bundle")]
    pub bundle_sha256: Option<String>,
    #[arg(long, requires = "bundle")]
    pub version: Option<String>,
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Settings {
    pub server_bind: String,
    pub server_origin: String,
    pub local_http: bool,
    pub server_data: PathBuf,
    pub tls_cert: Option<PathBuf>,
    pub tls_key: Option<PathBuf>,
    pub acme_domain: Option<String>,
    pub admin_username: String,
    pub agent_server: String,
    pub agent_name: String,
    pub agent_ca: Option<PathBuf>,
    pub autostart: bool,
    pub install_xray: bool,
}
impl Default for Settings {
    fn default() -> Self {
        Self {
            server_bind: "127.0.0.1:8080".into(),
            server_origin: "http://127.0.0.1:8080".into(),
            local_http: true,
            server_data: "/var/lib/panel/server".into(),
            tls_cert: None,
            tls_key: None,
            acme_domain: None,
            admin_username: "admin".into(),
            agent_server: "ws://127.0.0.1:8080/agent/v1/connect".into(),
            agent_name: "local node".into(),
            agent_ca: None,
            autostart: true,
            install_xray: false,
        }
    }
}
// Deliberately no Debug/Serialize: interactive and config credentials stay in memory only.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Input {
    #[serde(default)]
    pub settings: Settings,
    pub admin_password: Option<String>,
    pub enrollment_token: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::{Action, Cli};
    use clap::Parser;

    #[test]
    fn upgrade_accepts_optional_bundle_checksum() {
        let Action::Upgrade(latest) = Cli::try_parse_from(["panel-installer", "upgrade"])
            .unwrap()
            .command
            .unwrap()
        else {
            panic!("expected upgrade");
        };
        assert!(latest.bundle.is_none());
        assert!(latest.component.is_none());
        assert!(Cli::try_parse_from(["panel-installer", "upgrade", "--version", "1.0"]).is_err());
        assert!(
            Cli::try_parse_from(["panel-installer", "upgrade", "--bundle", "/bundle"]).is_err()
        );
        for (arguments, expected) in [
            (vec![], None),
            (vec!["--bundle-sha256", "trusted"], Some("trusted")),
        ] {
            let mut command = vec![
                "panel-installer",
                "upgrade",
                "--bundle",
                "/bundle",
                "--version",
                "1.0",
            ];
            command.extend(arguments);
            let Action::Upgrade(args) = Cli::try_parse_from(command).unwrap().command.unwrap()
            else {
                panic!("expected upgrade");
            };
            assert_eq!(args.bundle_sha256.as_deref(), expected);
        }
    }
}
