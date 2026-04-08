mod certs;
mod cli;
mod config;
mod daemon;
mod detect;
mod error;
mod hosts;
mod inspector;
mod pages;
mod ports;
mod process;
mod proto;
mod proxy;
mod routes;

use clap::Parser;
use error::Result;

#[tokio::main]
async fn main() -> Result<()> {
    // Install the ring crypto provider for rustls (must happen before any TLS use)
    let _ = rustls::crypto::ring::default_provider().install_default();

    let cli_args = cli::Cli::parse();
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_env("PORTAL_LOG")
                .unwrap_or_else(|_| "portal=info".into()),
        )
        .init();
    cli::run(cli_args).await
}
