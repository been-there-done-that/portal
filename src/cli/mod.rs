pub mod output;

use clap::{Parser, Subcommand};

use crate::error::Result;
use crate::proto::{read_frame, write_frame, Command};

#[derive(Parser)]
#[command(
    name = "portal",
    version,
    about = "Named .localhost URLs for local development"
)]
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
        /// Kill any existing instance for this hostname before starting
        #[arg(long)]
        force: bool,
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
            force,
            args,
        } => {
            // Allow --force anywhere in the arg list (clap trailing_var_arg swallows flags
            // that appear after the first positional argument).
            let force = force || args.iter().any(|a| a == "--force");
            let args: Vec<String> = args.into_iter().filter(|a| a != "--force").collect();

            ensure_daemon_running().await?;
            let cwd = std::env::current_dir()?;
            let config = crate::config::Config::load(&cwd)?;
            let hostname =
                crate::detect::resolve_hostname(&cwd, hostname.as_deref(), &config.proxy.tld);

            // Check for an existing live route for this hostname
            {
                let mut stream = ipc_connect().await?;
                write_frame(&mut stream, &Command::Ls).await?;
                let resp: crate::proto::Response = read_frame(&mut stream).await?;
                if let Some(serde_json::Value::Array(routes)) = resp.data {
                    if let Some(existing) = routes.iter().find(|r| {
                        r["hostname"].as_str() == Some(&hostname)
                    }) {
                        if force {
                            // Kill the existing instance via Stop
                            let mut s = ipc_connect().await?;
                            write_frame(&mut s, &Command::Stop { hostname: hostname.clone() }).await?;
                            let _: crate::proto::Response = read_frame(&mut s).await?;
                            eprintln!("  stopped existing instance on port {}", existing["port"].as_u64().unwrap_or(0));
                        } else {
                            eprintln!(
                                "error: {hostname} is already running on port {} (use --force to replace it)",
                                existing["port"].as_u64().unwrap_or(0)
                            );
                            std::process::exit(1);
                        }
                    }
                }
            }

            let port = port.map(Ok).unwrap_or_else(|| {
                crate::ports::find_free_port(config.proxy.port_range.0, config.proxy.port_range.1)
            })?;

            let my_pid = std::process::id();
            let mut child = crate::process::spawn_child(&cwd, &args, port, &hostname).await?;

            eprintln!("  https://{hostname}  ->  port {port}");

            // Register the route in the daemon's live in-memory store via IPC
            {
                let child_pid = child.id().unwrap_or(my_pid);
                if let Ok(mut stream) = ipc_connect().await {
                    let _ = write_frame(
                        &mut stream,
                        &Command::RegisterRoute {
                            hostname: hostname.clone(),
                            port,
                            pid: child_pid,
                            cwd: cwd.to_string_lossy().to_string(),
                        },
                    )
                    .await;
                    let _: crate::proto::Response = read_frame(&mut stream)
                        .await
                        .unwrap_or(crate::proto::Response::ok_empty());
                }
            }

            child.wait().await?;

            // Don't send Rm here — if --force replaced us, our Rm would wipe the
            // new route. Instead, rely on remove_stale() (called by `portless ls`)
            // to clean up dead routes automatically.
        }
    }

    Ok(())
}

async fn ipc_connect() -> Result<tokio::net::UnixStream> {
    let sock = crate::config::dirs_for_state().join("portal.sock");
    tokio::net::UnixStream::connect(&sock)
        .await
        .map_err(|_| crate::error::Error::DaemonNotRunning)
}

async fn ensure_daemon_running() -> Result<()> {
    let sock = crate::config::dirs_for_state().join("portal.sock");
    if sock.exists() && tokio::net::UnixStream::connect(&sock).await.is_ok() {
        return Ok(());
    }
    // Auto-start daemon
    let exe = std::env::current_exe()?;
    tokio::process::Command::new(exe)
        .arg("daemon")
        .env("PORTAL_IS_DAEMON", "1")
        .spawn()?;
    for _ in 0..20 {
        tokio::time::sleep(std::time::Duration::from_millis(150)).await;
        if tokio::net::UnixStream::connect(&sock).await.is_ok() {
            return Ok(());
        }
    }
    Err(crate::error::Error::DaemonNotRunning)
}
