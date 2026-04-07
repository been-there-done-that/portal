pub mod banner;
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
            let cwd = std::env::current_dir()?;
            let config = crate::config::Config::load(&cwd)?;
            let mut setup = banner::SetupPrinter::new();
            ensure_daemon_running(&config, &mut setup).await?;
            setup.done();
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
            // Load cwd + config first (needed for port range and hostname resolution)
            let cwd = std::env::current_dir()?;
            let config = crate::config::Config::load(&cwd)?;
            let mut setup = banner::SetupPrinter::new();
            ensure_daemon_running(&config, &mut setup).await?;
            ensure_cert_trusted(&mut setup).await?;
            setup.done();
            let hostname =
                crate::detect::resolve_hostname(&cwd, hostname.as_deref(), &config.proxy.tld);

            // Check for an existing live route for this hostname (replace-by-default)
            let reuse_port: Option<u16> = {
                let mut stream = ipc_connect().await?;
                write_frame(&mut stream, &Command::Ls).await?;
                let resp: crate::proto::Response = read_frame(&mut stream).await?;
                if let Some(serde_json::Value::Array(routes)) = resp.data {
                    routes
                        .iter()
                        .find(|r| r["hostname"].as_str() == Some(&hostname))
                        .and_then(|r| r["port"].as_u64())
                        .and_then(|p| u16::try_from(p).ok())
                } else {
                    None
                }
            };

            // Determine backend port:
            //   1. User pinned --port  → use it (stop old if exists)
            //   2. Existing route      → stop old, reuse its port
            //   3. No existing route   → find a free port
            let port = if let Some(explicit_port) = port {
                if let Some(old_port) = reuse_port {
                    let mut s = ipc_connect().await?;
                    write_frame(&mut s, &Command::Stop { hostname: hostname.clone() }).await?;
                    let _: crate::proto::Response = read_frame(&mut s).await?;
                    eprintln!("  replaced existing instance (port {})", old_port);
                    crate::ports::wait_for_port_free(
                        explicit_port,
                        std::time::Duration::from_secs(2),
                    )
                    .await;
                }
                explicit_port
            } else if let Some(old_port) = reuse_port {
                // Replace-by-default: stop old, reuse its port
                let mut s = ipc_connect().await?;
                write_frame(&mut s, &Command::Stop { hostname: hostname.clone() }).await?;
                let _: crate::proto::Response = read_frame(&mut s).await?;
                eprintln!("  replaced existing instance (port {})", old_port);
                crate::ports::wait_for_port_free(old_port, std::time::Duration::from_secs(2))
                    .await;
                old_port
            } else {
                crate::ports::find_free_port(
                    config.proxy.port_range.0,
                    config.proxy.port_range.1,
                )?
            };

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

            // Don't send Rm here — rely on remove_stale() (called by `portal ls`)
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

async fn ensure_daemon_running(
    config: &crate::config::Config,
    setup: &mut banner::SetupPrinter,
) -> Result<()> {
    let sock = crate::config::dirs_for_state().join("portal.sock");
    if sock.exists() && tokio::net::UnixStream::connect(&sock).await.is_ok() {
        return Ok(());
    }

    // If ca.pem doesn't exist yet, the daemon will generate it on first start.
    // Show a cert step so the user knows something is happening.
    let ca_pem_path = crate::config::dirs_for_state().join("ca.pem");
    let mut cert_pb: Option<indicatif::ProgressBar> = if !ca_pem_path.exists() {
        Some(setup.begin_step("cert", "generating CA certificate…"))
    } else {
        None
    };

    let daemon_pb = setup.begin_step("daemon", "starting…");

    let exe = match std::env::current_exe() {
        Ok(e) => e,
        Err(err) => {
            if let Some(pb) = cert_pb.take() {
                pb.abandon_with_message(format!("{} cert    failed", console::style("✗").red()));
            }
            daemon_pb.abandon_with_message(format!("{} daemon  failed to start", console::style("✗").red()));
            return Err(err.into());
        }
    };
    let needs_sudo = config.proxy.http_port < 1024 || config.proxy.https_port < 1024;
    let spawn_result = if needs_sudo {
        tokio::process::Command::new("sudo")
            .arg(&exe)
            .arg("daemon")
            .env("PORTAL_IS_DAEMON", "1")
            .spawn()
    } else {
        tokio::process::Command::new(&exe)
            .arg("daemon")
            .env("PORTAL_IS_DAEMON", "1")
            .spawn()
    };
    if let Err(err) = spawn_result {
        if let Some(pb) = cert_pb.take() {
            pb.abandon_with_message(format!("{} cert    failed", console::style("✗").red()));
        }
        daemon_pb.abandon_with_message(format!("{} daemon  failed to start", console::style("✗").red()));
        return Err(err.into());
    }

    for _ in 0..20 {
        tokio::time::sleep(std::time::Duration::from_millis(150)).await;
        if tokio::net::UnixStream::connect(&sock).await.is_ok() {
            if let Some(pb) = cert_pb.take() {
                pb.finish_with_message(format!(
                    "{} cert    generated",
                    console::style("✓").green()
                ));
            }
            daemon_pb.finish_with_message(format!(
                "{} daemon  started  on :{}/:{}",
                console::style("✓").green(),
                config.proxy.http_port,
                config.proxy.https_port,
            ));
            return Ok(());
        }
    }

    if let Some(pb) = cert_pb.take() {
        pb.abandon_with_message(format!(
            "{} cert    failed",
            console::style("✗").red()
        ));
    }
    daemon_pb.abandon_with_message(format!(
        "{} daemon  failed to start",
        console::style("✗").red()
    ));
    Err(crate::error::Error::DaemonNotRunning)
}

async fn ensure_cert_trusted(setup: &mut banner::SetupPrinter) -> Result<()> {
    if crate::certs::is_ca_trusted() {
        return Ok(());
    }

    let trust_pb = setup.begin_step("trust", "installing CA certificate…  (sudo required)");

    let exe = std::env::current_exe()?;
    let status = tokio::process::Command::new("sudo")
        .arg(&exe)
        .arg("cert")
        .arg("install")
        .status()
        .await?;

    if !status.success() {
        trust_pb.abandon_with_message(format!(
            "{} trust   failed  (run `sudo portal cert install` manually)",
            console::style("✗").red()
        ));
        return Err(crate::error::Error::Cert(
            "Failed to install CA certificate. Run `sudo portal cert install` manually."
                .to_string(),
        ));
    }

    trust_pb.finish_with_message(format!(
        "{} trust   installed  (sudo)",
        console::style("✓").green()
    ));
    Ok(())
}
