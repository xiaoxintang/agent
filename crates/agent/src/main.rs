use clap::{Parser, Subcommand};
use panel_agent::{config, connection, store::Store};
use std::{
    error::Error,
    io::{IsTerminal, Read},
    path::PathBuf,
};

#[derive(Parser)]
#[command(
    version,
    about = "Panel agent — authenticated outbound connection, no privileged operations"
)]
struct Cli {
    #[arg(long, global = true, default_value = "agent-data")]
    data_dir: PathBuf,
    #[arg(long, global = true)]
    ca_cert: Option<PathBuf>,
    #[arg(
        long,
        global = true,
        help = "Allow plaintext WS only to a literal loopback address for development"
    )]
    allow_local_ws: bool,
    #[command(subcommand)]
    command: Option<Command>,
}

#[derive(Subcommand)]
enum Command {
    Enroll {
        #[arg(long)]
        server: String,
        #[arg(long)]
        token_stdin: bool,
    },
    Run,
    /// Revoke this node credential on its authenticated server before local uninstall.
    Revoke,
}

fn main() -> Result<(), Box<dyn Error>> {
    panel_common::storage::disable_core_dumps()?;
    let cli = Cli::parse();
    if cli.command.is_none() {
        println!("panel-agent {}", panel_common::VERSION);
        return Ok(());
    }
    panel_agent::security::harden_process()?;
    tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()?
        .block_on(run(cli))
}

async fn run(cli: Cli) -> Result<(), Box<dyn Error>> {
    let command = cli
        .command
        .expect("command was checked before runtime creation");
    let connector = config::connector(cli.ca_cert.as_deref())?;
    let store = Store::open(&cli.data_dir).await?;
    let result = match command {
        Command::Enroll {
            server,
            token_stdin,
        } => {
            let token = if token_stdin {
                let mut input = String::new();
                std::io::stdin().take(67).read_to_string(&mut input)?;
                if input.ends_with('\n') {
                    input.pop();
                    if input.ends_with('\r') {
                        input.pop();
                    }
                }
                input
            } else {
                if !std::io::stdin().is_terminal() {
                    return Err("Enrollment requires a TTY or explicit --token-stdin".into());
                }
                rpassword::prompt_password("One-time enrollment token: ")?
            };
            connection::enroll(&store, &server, &token, cli.allow_local_ws, connector)
                .await
                .map(|node| println!("Agent enrolled: {node}"))
        }
        Command::Revoke => connection::retire(&store, cli.allow_local_ws, connector).await,
        Command::Run => {
            let (shutdown, receiver) = tokio::sync::watch::channel(false);
            tokio::spawn(async move {
                let _ = tokio::signal::ctrl_c().await;
                let _ = shutdown.send(true);
            });
            connection::run(&store, cli.allow_local_ws, connector, receiver).await
        }
    };
    store.close().await;
    result.map_err(Into::into)
}
