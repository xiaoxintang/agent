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
    #[arg(short = 'v', long = "version", visible_short_alias = 'V', action = clap::ArgAction::Version, global = true)]
    _version: bool,
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// Print the agent version without requiring root or initialization.
    Version,
    /// Initialize credentials and SQLite; print a new token only on first initialization.
    Init,
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
    let config_path = Path::new(config::DEFAULT_CONFIG);
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
        Commands::Serve => {
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
        Commands::Init | Commands::Openapi { .. } | Commands::Version => unreachable!(),
    }
}

#[cfg(test)]
mod cli_tests {
    use super::*;

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
