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
    /// Auto-detect and start the best dev script from package.json
    Start,
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

        CliCommand::Start => {
            let cwd = std::env::current_dir()?;
            let config = crate::config::Config::load(&cwd)?;

            let pkg_path = cwd.join("package.json");
            if !pkg_path.exists() {
                eprintln!(
                    "error: no package.json found in {}. Use 'portal run <command>' to run an arbitrary command.",
                    cwd.display()
                );
                std::process::exit(1);
            }

            let contents = std::fs::read_to_string(&pkg_path)?;
            let json: serde_json::Value = match serde_json::from_str(&contents) {
                Ok(v) => v,
                Err(e) => {
                    eprintln!("error: package.json is not valid JSON: {e}");
                    std::process::exit(1);
                }
            };

            if json.get("scripts").and_then(|s| s.as_object()).is_none() {
                eprintln!("error: package.json has no \"scripts\" field. Add a \"dev\" script or use 'portal run <command>'.");
                std::process::exit(1);
            }
            let script = match crate::detect::pick_dev_script(&json) {
                Some(s) => s,
                None => {
                    eprintln!("error: package.json \"scripts\" is empty. Add a \"dev\" script or use 'portal run <command>'.");
                    std::process::exit(1);
                }
            };

            let pm = crate::detect::detect_package_manager(&cwd);
            let args = vec![pm.to_string(), "run".to_string(), script];

            do_run(cwd, config, args, None, None).await?;
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
            let cwd = std::env::current_dir()?;
            let config = crate::config::Config::load(&cwd)?;
            let resolved_args = crate::detect::resolve_run_args(&cwd, args);
            do_run(cwd, config, resolved_args, hostname, port).await?;
        }
    }

    Ok(())
}

/// Core dev-server run logic shared by both `Run` and `Start`.
async fn do_run(
    cwd: std::path::PathBuf,
    config: crate::config::Config,
    args: Vec<String>,
    hostname_override: Option<String>,
    port_override: Option<u16>,
) -> Result<()> {
    let mut setup = banner::SetupPrinter::new();
    ensure_daemon_running(&config, &mut setup).await?;
    ensure_cert_trusted(&mut setup).await?;
    setup.done();

    let hostname =
        crate::detect::resolve_hostname(&cwd, hostname_override.as_deref(), &config.proxy.tld);

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
    let port = if let Some(explicit_port) = port_override {
        if let Some(_old_port) = reuse_port {
            let mut s = ipc_connect().await?;
            write_frame(&mut s, &Command::Stop { hostname: hostname.clone() }).await?;
            let _: crate::proto::Response = read_frame(&mut s).await?;
            crate::ports::wait_for_port_free(
                explicit_port,
                std::time::Duration::from_secs(2),
            )
            .await;
        }
        explicit_port
    } else if let Some(old_port) = reuse_port {
        let mut s = ipc_connect().await?;
        write_frame(&mut s, &Command::Stop { hostname: hostname.clone() }).await?;
        let _: crate::proto::Response = read_frame(&mut s).await?;
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

    // Register the route in the daemon's live in-memory store via IPC
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

    banner::print_banner(&hostname, port, child_pid, reuse_port.is_some());

    child.wait().await?;

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

    // Fast path: daemon is already running — return immediately, no UI.
    if sock.exists() && tokio::net::UnixStream::connect(&sock).await.is_ok() {
        return Ok(());
    }

    let exe = std::env::current_exe()?;
    let needs_sudo = !cfg!(windows)
        && (config.proxy.http_port < 1024 || config.proxy.https_port < 1024);
    let ca_missing = !crate::config::dirs_for_state().join("ca.pem").exists();

    // Non-TTY guard: sudo needs an interactive terminal for its password/Touch ID prompt.
    if needs_sudo {
        use std::io::IsTerminal;
        if !std::io::stdin().is_terminal() {
            eprintln!("error: daemon is not running and no TTY is available for sudo.");
            eprintln!("  Option 1: run portal in a terminal (will prompt for password):");
            eprintln!("    portal start");
            eprintln!("  Option 2: use unprivileged ports in portal.toml:");
            eprintln!("    [proxy]");
            eprintln!("    http_port = 8080");
            eprintln!("    https_port = 8443");
            return Err(crate::error::Error::DaemonNotRunning);
        }
    }

    if needs_sudo {
        // Plain-text path — no indicatif spinners that could corrupt the TTY that sudo needs.
        if ca_missing {
            setup.plain_step("cert     generating CA certificate…");
        }
        setup.plain_step("daemon   starting (sudo may ask for your password)…");

        // Single blocking call — gives sudo full TTY access so the password prompt
        // and Touch ID (if configured in /etc/pam.d/sudo_local) both work naturally.
        //
        // `portal daemon` without PORTAL_IS_DAEMON spawns a background grandchild
        // daemon and exits in <100 ms, so status() returns quickly after authentication.
        // The grandchild continues running as root and binds ports 80/443.
        let status = tokio::process::Command::new("sudo")
            .arg(&exe)
            .arg("daemon")
            .stdin(std::process::Stdio::inherit())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::inherit())
            .status()
            .await?;

        if !status.success() {
            eprintln!(
                "  {} daemon  sudo failed — check the error above",
                console::style("✗").red()
            );
            return Err(crate::error::Error::DaemonNotRunning);
        }

        // Poll for the IPC socket. The grandchild daemon is starting up; 10 s is plenty.
        // Print a waiting line every ~2 s so the user knows we haven't frozen.
        for i in 0..67u32 {
            tokio::time::sleep(std::time::Duration::from_millis(150)).await;
            if tokio::net::UnixStream::connect(&sock).await.is_ok() {
                setup.plain_step(&format!(
                    "{} daemon  started  on :{}/:{}",
                    console::style("✓").green(),
                    config.proxy.http_port,
                    config.proxy.https_port,
                ));
                return Ok(());
            }
            if i > 0 && i % 13 == 0 {
                setup.plain_step("         waiting for daemon…");
            }
        }

        eprintln!(
            "  {} daemon  timed out — socket not found at {}",
            console::style("✗").red(),
            sock.display()
        );
        return Err(crate::error::Error::DaemonNotRunning);
    }

    // No sudo needed: use animated spinners (unchanged behavior).
    let mut cert_pb: Option<indicatif::ProgressBar> = if ca_missing {
        Some(setup.begin_step("cert", "generating CA certificate…"))
    } else {
        None
    };
    let daemon_pb = setup.begin_step("daemon", "starting…");

    if let Err(err) = std::process::Command::new(&exe)
        .arg("daemon")
        .env("PORTAL_IS_DAEMON", "1")
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
    {
        if let Some(pb) = cert_pb.take() {
            pb.abandon_with_message(format!("{} cert    failed", console::style("✗").red()));
        }
        daemon_pb.abandon_with_message(format!(
            "{} daemon  failed to start",
            console::style("✗").red()
        ));
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
        pb.abandon_with_message(format!("{} cert    failed", console::style("✗").red()));
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
