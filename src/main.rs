mod error;
mod config;
mod proto;
mod routes;
mod ports;
mod pages;
mod certs;
mod detect;
mod process;
mod proxy;
mod daemon;
mod cli;

use clap::Parser;
use error::Result;

#[tokio::main]
async fn main() -> Result<()> {
    let cli_args = cli::Cli::parse();
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_env("PORTLESS_LOG")
                .unwrap_or_else(|_| "portless=info".into()),
        )
        .init();
    cli::run(cli_args).await
}
