use clap::{Parser, Subcommand};
use panel_server::{app, auth, config, db, state::AppState};
use std::{
    error::Error,
    io::{IsTerminal, Read},
};

#[derive(Parser)]
#[command(version, about = "Linux Panel server")]
struct Cli {
    #[command(subcommand)]
    command: Option<Command>,
}

#[derive(Subcommand)]
enum Command {
    /// Initialize the first administrator locally; never overwrites an existing account.
    InitAdmin {
        #[arg(long)]
        username: String,
        /// Read one password from stdin instead of prompting (never pass it as an argument).
        #[arg(long)]
        password_stdin: bool,
    },
}

fn read_password(stdin: bool) -> Result<String, Box<dyn Error>> {
    if stdin {
        let mut password = String::new();
        std::io::stdin().take(131).read_to_string(&mut password)?;
        if password.len() > 130 {
            return Err("Password input exceeds 128 bytes plus a line ending".into());
        }
        if password.ends_with('\n') {
            password.pop();
            if password.ends_with('\r') {
                password.pop();
            }
        }
        Ok(password)
    } else {
        if !std::io::stdin().is_terminal() {
            return Err(
                "Interactive initialization requires a TTY; use --password-stdin for automation"
                    .into(),
            );
        }
        let password = rpassword::prompt_password("Administrator password (12–128 bytes): ")?;
        let confirm = rpassword::prompt_password("Confirm password: ")?;
        if password != confirm {
            return Err("Passwords do not match".into());
        }
        Ok(password)
    }
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn Error>> {
    panel_common::storage::disable_core_dumps()?;
    let cli = Cli::parse();
    let data = config::data_directory();
    panel_common::storage::private_directory(&data)?;
    let instance = panel_common::storage::private_file(&data.join("server.lock"))?;
    instance
        .try_lock()
        .map_err(|_| "Server data is already in use by another process")?;
    if let Some(Command::InitAdmin {
        username,
        password_stdin,
    }) = cli.command
    {
        let password = read_password(password_stdin)?;
        let pool = db::open(&config::data_directory()).await?;
        auth::bootstrap_admin(&pool, &username, password).await?;
        pool.close().await;
        println!("Administrator initialized");
        return Ok(());
    }
    let tls_paths = config::tls_paths()?;
    let acme_domain = match std::env::var("PANEL_ACME_DOMAIN") {
        Ok(domain) => Some(domain),
        Err(std::env::VarError::NotPresent) => None,
        Err(error) => return Err(error.into()),
    };
    if acme_domain.is_some() && tls_paths.is_some() {
        return Err("ACME and manual TLS certificates cannot be combined".into());
    }
    let tls_enabled = tls_paths.is_some() || acme_domain.is_some();
    let listener = tokio::net::TcpListener::bind(config::listen_address(tls_enabled)?).await?;
    let address = listener.local_addr()?;
    let secure_cookie = config::secure_cookie()?;
    if tls_enabled && !secure_cookie {
        return Err("HTTP mode cannot be used with TLS".into());
    }
    let origin = if tls_enabled {
        config::tls_origin(address)?
    } else {
        config::origin(address)?
    };
    if let Some(domain) = &acme_domain {
        let uri: axum::http::Uri = origin.parse()?;
        if uri.host() != Some(domain.as_str())
            || uri.port_u16().unwrap_or(443) != address.port()
            || !domain.contains('.')
            || domain.parse::<std::net::IpAddr>().is_ok()
            || !domain.bytes().all(|byte| {
                byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-' || byte == b'.'
            })
            || address.port() == 80
        {
            return Err("ACME requires a matching DNS origin and reserves port 80".into());
        }
    }
    let mut state =
        AppState::new(db::open(&config::data_directory()).await?, secure_cookie).await?;
    tokio::spawn(panel_server::traffic::run(
        state.pool.clone(),
        state.agents_shutdown.subscribe(),
    ));
    let shutdown = state.agents_shutdown.clone();
    if tls_enabled {
        state.tls_enabled = true;
        let tls = if let Some(domain) = acme_domain {
            panel_server::acme::start(domain, &data)
                .await
                .map_err(|error| format!("ACME startup failed: {error}"))?
        } else {
            let (cert, key) = tls_paths.ok_or("Missing TLS certificate")?;
            axum_server::tls_rustls::RustlsConfig::from_pem_file(cert, key).await?
        };
        let handle = axum_server::Handle::new();
        let stop = handle.clone();
        tokio::spawn(async move {
            let _ = tokio::signal::ctrl_c().await;
            let _ = shutdown.send(true);
            stop.graceful_shutdown(Some(std::time::Duration::from_secs(5)));
        });
        println!(
            "panel-server {} listening on https://{address}",
            panel_common::VERSION
        );
        axum_server::from_tcp_rustls(listener.into_std()?, tls)?
            .handle(handle)
            .serve(app::router(state).into_make_service())
            .await?;
        return Ok(());
    }
    println!(
        "panel-server {} listening on http://{address}",
        panel_common::VERSION
    );
    axum::serve(listener, app::router(state))
        .with_graceful_shutdown(async move {
            if let Err(error) = tokio::signal::ctrl_c().await {
                eprintln!("Cannot listen for shutdown: {error}");
            }
            let _ = shutdown.send(true);
        })
        .await?;
    Ok(())
}
