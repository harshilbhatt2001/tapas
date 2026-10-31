mod api;
mod auth;
mod seal;
mod store;
mod sync;

use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Arc;

use anyhow::{Context, Result};
use clap::{Parser, Subcommand};
use tapas_sources::google::oauth::WebClient;
use tower_http::trace::TraceLayer;

#[derive(Parser)]
#[command(about = "tapas server")]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Serve the API and run the sync loop.
    Serve(Config),
    /// Print the `OpenAPI` document the Android client is generated from.
    Openapi,
}

#[derive(clap::Args)]
struct Config {
    #[arg(long, env = "TAPAS_LISTEN", default_value = "127.0.0.1:8080")]
    listen: SocketAddr,
    #[arg(long, env = "TAPAS_DATABASE")]
    database: PathBuf,
    /// The only Google account allowed to sign in.
    #[arg(long, env = "TAPAS_ALLOWED_EMAIL")]
    allowed_email: String,
    #[arg(long, env = "TAPAS_GOOGLE_WEB_CLIENT_ID")]
    google_web_client_id: String,
    #[arg(long, env = "TAPAS_GOOGLE_WEB_CLIENT_SECRET_FILE")]
    google_web_client_secret_file: PathBuf,
    /// 32 random bytes that encrypt stored refresh tokens.
    #[arg(long, env = "TAPAS_TOKEN_KEY_FILE")]
    token_key_file: PathBuf,
}

#[tokio::main]
async fn main() -> Result<()> {
    match Cli::parse().command {
        Command::Openapi => {
            println!("{}", api::router().1.to_pretty_json()?);
            Ok(())
        }
        Command::Serve(config) => serve(config).await,
    }
}

async fn serve(config: Config) -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .init();

    let read =
        |path: &PathBuf| std::fs::read(path).with_context(|| format!("reading {}", path.display()));
    let client_secret = String::from_utf8(read(&config.google_web_client_secret_file)?)
        .context("the Google client secret is not UTF-8")?;
    let store = store::Store::open(&config.database).await?;
    store.ensure_master().await?;
    let state = api::AppState {
        store,
        http: reqwest::Client::new(),
        google: Arc::new(WebClient {
            id: config.google_web_client_id.clone(),
            secret: client_secret.trim().to_owned(),
        }),
        verifier: Arc::new(auth::IdTokenVerifier::new(
            config.google_web_client_id,
            config.allowed_email,
        )),
        sealer: Arc::new(seal::Sealer::new(&read(&config.token_key_file)?)?),
        sync_now: Arc::default(),
    };

    tokio::spawn(sync::run(state.clone()));

    let app = api::router()
        .0
        .with_state(state)
        .layer(TraceLayer::new_for_http());
    let listener = tokio::net::TcpListener::bind(config.listen).await?;
    tracing::info!("listening on {}", config.listen);
    axum::serve(listener, app)
        .with_graceful_shutdown(shutdown_signal())
        .await?;
    Ok(())
}

/// Ctrl-C locally, SIGTERM from systemd.
async fn shutdown_signal() {
    let mut terminate = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
        .expect("installing the SIGTERM handler");
    tokio::select! {
        _ = tokio::signal::ctrl_c() => {}
        _ = terminate.recv() => {}
    }
}
