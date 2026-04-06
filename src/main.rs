mod error;

// Stub modules — will be implemented in subsequent tasks
// mod config;
// mod proto;
// mod routes;
// mod ports;
// mod pages;
// mod certs;
// mod detect;
// mod process;
// mod proxy;
// mod daemon;
// mod cli;

use error::Result;

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_env("PORTLESS_LOG")
                .unwrap_or_else(|_| "portless=info".into()),
        )
        .init();

    // Stub — will be replaced in Task 15
    println!("portless 1.0.0");
    Ok(())
}
