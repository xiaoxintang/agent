//! 命令行入口：区分无需初始化的查询命令、HTTP 常驻服务和 systemd 辅助命令。

mod api;
mod app;
mod config;
mod error;
mod grpc;
mod mutation;
mod openapi;
mod process;
mod store;
#[cfg(test)]
mod testsupport;
mod update;
mod versions;

use clap::{Parser, Subcommand};
use error::{Error, Result};
use std::{
    fs,
    path::{Path, PathBuf},
};

#[derive(Parser)]
#[command(
    version,
    disable_version_flag = true,
    propagate_version = true,
    about = "Linux/systemd Xray agent with durable configuration and OpenAPI"
)]
struct Cli {
    // 自定义小写 -v，同时保留常见的 -V；版本参数会在执行业务命令前退出。
    // Version 动作不生成默认布尔值，使用 Option 避免普通子命令缺少此字段。
    #[arg(short = 'v', long = "version", visible_short_alias = 'V', action = clap::ArgAction::Version, global = true)]
    _version: Option<bool>,
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// Print the agent version without requiring root or initialization.
    Version,
    /// Initialize credentials and SQLite; print a new token only on first initialization.
    Init,
    /// Print the current Bearer token from the initialized configuration.
    Token,
    /// Save and print a new Bearer token, then restart the Agent to apply it.
    ResetToken,
    /// Install the configured default Xray version if no managed version is active.
    InstallXray,
    /// Update the Agent from GitHub Releases, preserving credentials and Xray state.
    Update(update::UpdateRequest),
    /// Uninstall the Agent and managed Xray, deleting all configuration and state.
    Uninstall,
    /// Run the HTTP management service.
    Serve,
    /// systemd ExecStartPre: publish the authoritative committed Xray JSON.
    PrepareXray,
    /// systemd ExecStartPost: wait for gRPC and restore persisted overrides.
    RestoreXray,
    /// Export the complete OpenAPI document without requiring root or initialization.
    Openapi {
        #[arg(long)]
        output: Option<PathBuf>,
    },
}

fn require_linux_root() -> Result<()> {
    #[cfg(target_os = "linux")]
    {
        use std::os::unix::fs::MetadataExt;
        if fs::metadata("/proc/self")?.uid() == 0 {
            return Ok(());
        }
        Err(Error::Validation("this command requires root".into()))
    }
    #[cfg(not(target_os = "linux"))]
    {
        Err(Error::Unsupported(
            "this command requires Linux with systemd".into(),
        ))
    }
}

#[tokio::main]
async fn main() {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "agent=info".into()),
        )
        .with_writer(std::io::stderr)
        .init();
    if let Err(error) = run(Cli::parse()).await {
        eprintln!("{error}");
        std::process::exit(1);
    }
}

async fn run(cli: Cli) -> Result<()> {
    // 版本和 OpenAPI 导出不依赖 root、凭据文件或已经安装的 Xray。
    if matches!(cli.command, Commands::Version) {
        println!("agent {}", env!("CARGO_PKG_VERSION"));
        return Ok(());
    }
    if let Commands::Openapi { output } = cli.command {
        let bytes = serde_json::to_vec_pretty(&openapi::document())?;
        if let Some(path) = output {
            fs::write(path, bytes)?;
        } else {
            use std::io::Write;
            std::io::stdout().lock().write_all(&bytes)?;
        }
        return Ok(());
    }
    require_linux_root()?;
    if let Commands::Update(request) = cli.command {
        return update::run(request).await;
    }
    if matches!(cli.command, Commands::Uninstall) {
        let _lock = update::lock_update()?;
        let status = tokio::process::Command::new("/bin/sh")
            .args([
                "-c",
                include_str!("../deploy/uninstall.sh"),
                "agent-uninstall",
            ])
            .stdin(std::process::Stdio::null())
            .kill_on_drop(true)
            .status()
            .await?;
        if !status.success() {
            return Err(Error::Unavailable(format!(
                "Uninstall failed ({status}); inspect the uninstaller output"
            )));
        }
        return Ok(());
    }
    let config_path = Path::new(config::DEFAULT_CONFIG);
    match cli.command {
        Commands::Token => {
            println!("{}", config::load(config_path)?.token);
            return Ok(());
        }
        Commands::ResetToken => {
            println!("{}", config::reset_token(config_path)?);
            process::Systemd::restart_agent().await.map_err(|error| {
                Error::Unavailable(format!(
                    "New token saved, but restarting the Agent failed: {error}. Retry with: sudo systemctl restart xray-agent.service"
                ))
            })?;
            return Ok(());
        }
        _ => {}
    }
    if matches!(cli.command, Commands::Init) {
        let token = config::initialize(config_path)?;
        let settings = config::load(config_path)?;
        let paths = config::Paths::default();
        paths.prepare()?;
        store::Store::open(&paths.database, &config::initial_xray(&settings))?;
        if let Some(token) = token {
            println!("Bearer token (shown once): {token}");
        } else {
            println!("Already initialized; existing credentials preserved.");
        }
        return Ok(());
    }
    let settings = config::load(config_path)?;
    let app = app::App::open(settings, config::Paths::default())?;
    match cli.command {
        Commands::PrepareXray => app.materialize(),
        Commands::RestoreXray => app.restore_after_start().await,
        Commands::Serve | Commands::InstallXray => {
            let lock_path = app.paths.database.with_file_name("agent.lock");
            let lock = fs::OpenOptions::new()
                .create(true)
                .truncate(false)
                .read(true)
                .write(true)
                .open(&lock_path)?;
            config::private_file(&lock_path)?;
            // 文件锁约束多个 Agent 进程，App 内的异步锁约束单个进程内的并发操作。
            fs2::FileExt::try_lock_exclusive(&lock)
                .map_err(|_| Error::Conflict("another agent instance is running".into()))?;
            if matches!(cli.command, Commands::InstallXray) {
                app.recover().await?;
                if let Some(version) = app.active_version()? {
                    println!("Xray {version} already installed; existing version preserved.");
                } else {
                    println!("{}", app.install(None).await?);
                }
                return Ok(());
            }
            if let Err(error) = app.recover().await {
                // 恢复失败仍启动 HTTP，管理端才能查询未完成事务和故障状态。
                tracing::error!(error=%error,"startup recovery incomplete; inspect /status");
            }
            let listener = tokio::net::TcpListener::bind(app.settings.listen).await?;
            tracing::info!(address=%app.settings.listen,"HTTP agent started");
            axum::serve(listener, api::router(app.clone()))
                .with_graceful_shutdown(shutdown())
                .await?;
            // HTTP 停止后仍等待已分离的变更任务，避免打断 gRPC 调用后的持久化提交。
            app.tasks.close();
            app.tasks.wait().await;
            drop(lock);
            Ok(())
        }
        Commands::Init
        | Commands::Token
        | Commands::ResetToken
        | Commands::Openapi { .. }
        | Commands::Version
        | Commands::Uninstall
        | Commands::Update(_) => {
            unreachable!()
        }
    }
}

#[cfg(test)]
mod cli_tests {
    use super::*;

    #[test]
    fn ordinary_commands_do_not_require_a_version_flag() {
        for command in [
            "version",
            "init",
            "token",
            "reset-token",
            "install-xray",
            "update",
            "uninstall",
            "serve",
            "prepare-xray",
            "restore-xray",
            "openapi",
        ] {
            let cli = Cli::try_parse_from(["agent", command]).unwrap_or_else(|error| {
                panic!("{command} must parse without a version flag: {error}")
            });
            assert!(cli._version.is_none());
        }
        let cli = Cli::try_parse_from(["agent", "openapi", "--output", "openapi.json"]).unwrap();
        assert!(
            matches!(cli.command, Commands::Openapi { output: Some(path) } if path == PathBuf::from("openapi.json"))
        );
    }

    #[test]
    fn update_accepts_a_target_version_without_shadowing_the_version_flag() {
        let cli = Cli::try_parse_from(["agent", "update", "--target-version", "v0.1.2"]).unwrap();
        assert!(
            matches!(cli.command, Commands::Update(request) if request.version.as_deref() == Some("v0.1.2"))
        );
        let error = Cli::try_parse_from(["agent", "update", "--version"])
            .err()
            .unwrap();
        assert_eq!(error.kind(), clap::error::ErrorKind::DisplayVersion);
    }

    #[test]
    fn version_flags_exit_successfully_without_a_subcommand() {
        for flag in ["-v", "-V", "--version"] {
            let error = match Cli::try_parse_from(["agent", flag]) {
                Err(error) => error,
                Ok(_) => panic!("version must exit before running commands"),
            };
            assert_eq!(error.kind(), clap::error::ErrorKind::DisplayVersion);
            assert_eq!(error.exit_code(), 0);
            assert!(error.to_string().contains(env!("CARGO_PKG_VERSION")));
        }
        assert!(matches!(
            Cli::try_parse_from(["agent", "version"]).unwrap().command,
            Commands::Version
        ));
        let error = match Cli::try_parse_from(["agent", "init", "--version"]) {
            Err(error) => error,
            Ok(_) => panic!("version must not initialize the agent"),
        };
        assert_eq!(error.kind(), clap::error::ErrorKind::DisplayVersion);
    }
}

async fn shutdown() {
    // 同时响应终端 Ctrl-C 和 systemd 发送的 SIGTERM，交由 HTTP 层优雅退出。
    #[cfg(unix)]
    {
        match tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate()) {
            Ok(mut terminate) => {
                tokio::select! {_=tokio::signal::ctrl_c()=>{},_=terminate.recv()=>{}}
            }
            Err(_) => {
                let _ = tokio::signal::ctrl_c().await;
            }
        }
    }
    #[cfg(not(unix))]
    {
        let _ = tokio::signal::ctrl_c().await;
    }
}
