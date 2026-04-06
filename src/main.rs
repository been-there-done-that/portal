mod certs;
mod cli;
mod config;
mod daemon;
mod detect;
mod error;
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
    let cli_args = cli::Cli::parse();
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_env("PORTLESS_LOG")
                .unwrap_or_else(|_| "portless=info".into()),
        )
        .init();
    cli::run(cli_args).await
}
