pub mod output;

use clap::{Parser, Subcommand};

use crate::error::Result;
use crate::proto::{Command, read_frame, write_frame};

#[derive(Parser)]
#[command(name = "portless", version, about = "Named .localhost URLs for local dev")]
pub struct Cli {
    #[command(subcommand)]
    pub command: CliCommand,
}

#[derive(Subcommand)]
pub enum CliCommand {
    /// Start the background daemon
    Daemon,
    /// Run a dev server and assign it a .localhost URL
    Run {
        #[arg(long)]
        hostname: Option<String>,
        #[arg(long)]
        port: Option<u16>,
        #[arg(trailing_var_arg = true, required = true)]
        args: Vec<String>,
    },
    /// Stop a running proxy (and kill its process)
    Stop { hostname: Option<String> },
    /// List all active routes
    Ls,
    /// Show daemon status
    Status,
    /// Remove a route (without killing its process)
    Rm { hostname: String },
    /// Certificate management
    Cert {
        #[command(subcommand)]
        action: CertAction,
    },
    /// Show effective configuration
    Config,
    /// Shut down the daemon
    Shutdown,
}

#[derive(Subcommand)]
pub enum CertAction {
    /// Install the local CA into the system trust store
    Install,
    /// Regenerate the local CA and reinstall
    Reset,
}

pub async fn run(cli: Cli) -> Result<()> {
    match cli.command {
        CliCommand::Daemon => {
            crate::daemon::start().await?;
        }

        CliCommand::Ls => {
            ensure_daemon_running().await?;
            let mut stream = ipc_connect().await?;
            write_frame(&mut stream, &Command::Ls).await?;
            let resp = read_frame(&mut stream).await?;
            output::print_ls(&resp);
        }

        CliCommand::Status => {
            let mut stream = ipc_connect().await?;
            write_frame(&mut stream, &Command::Status).await?;
            let resp = read_frame(&mut stream).await?;
            output::print_status(&resp);
        }

        CliCommand::Stop { hostname } => {
            let mut stream = ipc_connect().await?;
            write_frame(
                &mut stream,
                &Command::Stop {
                    hostname: hostname.unwrap_or_default(),
                },
            )
            .await?;
            let resp = read_frame(&mut stream).await?;
            output::print_response(&resp);
        }

        CliCommand::Rm { hostname } => {
            let mut stream = ipc_connect().await?;
            write_frame(&mut stream, &Command::Rm { hostname }).await?;
            let resp = read_frame(&mut stream).await?;
            output::print_response(&resp);
        }

        CliCommand::Shutdown => {
            let mut stream = ipc_connect().await?;
            write_frame(&mut stream, &Command::Shutdown).await?;
            let resp = read_frame(&mut stream).await?;
            output::print_response(&resp);
        }

        CliCommand::Cert { action } => {
            let mut stream = ipc_connect().await?;
            let cmd = match action {
                CertAction::Install => Command::CertInstall,
                CertAction::Reset => Command::CertReset,
            };
            write_frame(&mut stream, &cmd).await?;
            let resp = read_frame(&mut stream).await?;
            output::print_response(&resp);
        }

        CliCommand::Config => {
            let cwd = std::env::current_dir()?;
            let config = crate::config::Config::load(&cwd)?;
            let json = serde_json::json!({
                "proxy": {
                    "tld": config.proxy.tld,
                    "port_range": config.proxy.port_range,
                    "https": config.proxy.https,
                    "http_port": config.proxy.http_port,
                    "https_port": config.proxy.https_port,
                },
                "daemon": {
                    "log_level": config.daemon.log_level,
                    "auto_start": config.daemon.auto_start,
                },
                "project": {
                    "name": config.project.name,
                }
            });
            println!("{}", serde_json::to_string_pretty(&json)?);
        }

        CliCommand::Run {
            hostname,
            port,
            args,
        } => {
            ensure_daemon_running().await?;
            let cwd = std::env::current_dir()?;
            let config = crate::config::Config::load(&cwd)?;
            let hostname =
                crate::detect::resolve_hostname(&cwd, hostname.as_deref(), &config.proxy.tld);
            let port = port
                .map(Ok)
                .unwrap_or_else(|| {
                    crate::ports::find_free_port(
                        config.proxy.port_range.0,
                        config.proxy.port_range.1,
                    )
                })?;

            // Register the route with the daemon via IPC
            let my_pid = std::process::id();
            {
                let mut stream = ipc_connect().await?;
                write_frame(
                    &mut stream,
                    &Command::Run {
                        hostname: hostname.clone(),
                        args: args.clone(),
                        cwd: cwd.to_string_lossy().to_string(),
                    },
                )
                .await?;
                // The daemon will respond with an error ("use portless run from CLI")
                // which we intentionally ignore here — spawn is our responsibility.
                let _: crate::proto::Response = read_frame(&mut stream).await.unwrap_or(
                    crate::proto::Response::ok_empty(),
                );
            }

            let mut child = crate::process::spawn_child(&cwd, &args, port).await?;

            println!("  https://{hostname}  ->  port {port}");

            // Register the route with the running PID
            {
                let child_pid = child.id().unwrap_or(my_pid);
                let route = crate::routes::Route {
                    hostname: hostname.clone(),
                    port,
                    pid: child_pid,
                    owner_pid: my_pid,
                    cwd: cwd.to_string_lossy().to_string(),
                    created_at: chrono::Utc::now(),
                };
                // Persist directly to the route store path so the daemon can pick it up
                let state_dir = crate::config::dirs_for_state();
                if let Ok(store) = crate::routes::RouteStore::new(state_dir.join("routes.json")) {
                    let _ = store.insert(route);
                }
            }

            child.wait().await?;

            // Clean up route on exit
            {
                let state_dir = crate::config::dirs_for_state();
                if let Ok(store) = crate::routes::RouteStore::new(state_dir.join("routes.json")) {
                    let _ = store.remove(&hostname);
                }
            }
        }
    }

    Ok(())
}

async fn ipc_connect() -> Result<tokio::net::UnixStream> {
    let sock = crate::config::dirs_for_state().join("portless.sock");
    tokio::net::UnixStream::connect(&sock)
        .await
        .map_err(|_| crate::error::Error::DaemonNotRunning)
}

async fn ensure_daemon_running() -> Result<()> {
    let sock = crate::config::dirs_for_state().join("portless.sock");
    if sock.exists() && tokio::net::UnixStream::connect(&sock).await.is_ok() {
        return Ok(());
    }
    // Auto-start daemon
    let exe = std::env::current_exe()?;
    tokio::process::Command::new(exe)
        .arg("daemon")
        .spawn()?;
    for _ in 0..20 {
        tokio::time::sleep(std::time::Duration::from_millis(150)).await;
        if tokio::net::UnixStream::connect(&sock).await.is_ok() {
            return Ok(());
        }
    }
    Err(crate::error::Error::DaemonNotRunning)
}
