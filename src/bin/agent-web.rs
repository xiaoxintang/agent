use agent::{error::Result, web, web_config};
use clap::Parser;
use std::path::PathBuf;

#[derive(Parser)]
#[command(
    name = "agent-web",
    version,
    about = "Optional Xray Web management service"
)]
struct Cli {
    #[arg(long, default_value = web_config::SETTINGS)]
    config: PathBuf,
    #[arg(long, default_value = web_config::DATABASE)]
    database: PathBuf,
    #[arg(long, default_value = web_config::CREDENTIALS)]
    credentials: PathBuf,
    #[arg(long, default_value = web_config::LOCAL_AGENT)]
    local_agent: PathBuf,
    #[arg(long, default_value = web_config::FRONTEND)]
    frontend: PathBuf,
    /// Export the Web API contract without starting the service.
    #[arg(long)]
    openapi: bool,
}
#[tokio::main]
async fn main() {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "agent=info".into()),
        )
        .init();
    if let Err(error) = run(Cli::parse()).await {
        eprintln!("{error}");
        std::process::exit(1);
    }
}
async fn run(cli: Cli) -> Result<()> {
    if cli.openapi {
        println!("{}", serde_json::to_string_pretty(&web::contract())?);
        return Ok(());
    }
    let settings = web_config::Settings::load(&cli.config)?;
    if !settings.enabled {
        return Err(agent::error::Error::Validation("Web is disabled".into()));
    }
    web::serve(
        settings,
        cli.database,
        cli.credentials,
        cli.local_agent,
        cli.frontend,
    )
    .await
}
