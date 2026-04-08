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
    /// Open the request inspector in the browser
    Inspect,
    /// Generate portal.toml for this project
    Init,
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
            let registry = crate::detect::DriverRegistry::new(&config);

            let driver = match registry.detect(&cwd) {
                Some(d) => d,
                None => {
                    eprintln!("No supported project detected. Run `portal init` to set up this project.");
                    std::process::exit(1);
                }
            };

            let raw_cmd = match driver.start_command(&cwd) {
                Some(cmd) => cmd,
                None => {
                    eprintln!("Detected {} but couldn't determine a start command. Run `portal init`.", driver.name());
                    std::process::exit(1);
                }
            };

            let hostname_override = config.project.name.clone()
                .or_else(|| driver.project_name(&cwd));

            let args: Vec<String> = raw_cmd
                .split_whitespace()
                .map(String::from)
                .collect();

            do_run(cwd, config, args, hostname_override, None, true).await?;
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
            let cwd = std::env::current_dir()?;
            let config = crate::config::Config::load(&cwd)?;
            let mut setup = banner::SetupPrinter::new();
            ensure_daemon_running(&config, &mut setup).await?;
            setup.done();

            let mut stream = ipc_connect().await?;
            write_frame(&mut stream, &Command::Status).await?;
            let status_resp = read_frame(&mut stream).await?;

            let mut stream2 = ipc_connect().await?;
            write_frame(&mut stream2, &Command::Ls).await?;
            let ls_resp = read_frame(&mut stream2).await?;

            output::print_status(&status_resp, &ls_resp);
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
            do_run(cwd, config, resolved_args, hostname, port, false).await?;
        }

        CliCommand::Inspect => {
            let url = "https://_.localhost";
            #[cfg(target_os = "macos")]
            {
                std::process::Command::new("open").arg(url).spawn().ok();
            }
            #[cfg(target_os = "linux")]
            {
                std::process::Command::new("xdg-open").arg(url).spawn().ok();
            }
            println!("Opening {url}");
        }

        CliCommand::Init => {
            let cwd = std::env::current_dir()?;

            if cwd.join("portal.toml").exists() {
                eprintln!("portal.toml already exists. Remove it first to reinitialise.");
                std::process::exit(1);
            }

            let config = crate::config::Config::load(&cwd)?;
            let registry = crate::detect::DriverRegistry::new(&config);
            let detected = registry.detect_language(&cwd);

            use std::io::IsTerminal;
            let is_tty = std::io::stdin().is_terminal();

            let (start_command, port_arg, host_arg, port_position, name) =
                if let Some(driver) = detected {
                    let raw_cmd = driver.start_command(&cwd).unwrap_or_default();
                    let proj_name = driver.project_name(&cwd)
                        .unwrap_or_else(|| {
                            cwd.file_name()
                                .and_then(|n| n.to_str())
                                .map(crate::detect::sanitize_hostname)
                                .unwrap_or_else(|| "app".to_string())
                        });

                    if is_tty {
                        println!("\n  {} Detected  {}", console::style("✓").green(), driver.name());
                        println!("  {} command   {}", console::style(" ").dim(), raw_cmd);
                        println!("  {} name      {}\n", console::style(" ").dim(), proj_name);

                        let confirmed: bool = dialoguer::Confirm::new()
                            .with_prompt("Does this look right?")
                            .default(true)
                            .interact()
                            .unwrap_or(true);

                        if confirmed {
                            let (pa, ha, pp) = injection_toml_fields(&driver.port_injection(&cwd, 0));
                            (raw_cmd, pa, ha, pp, Some(proj_name))
                        } else {
                            prompt_manual_config()?
                        }
                    } else {
                        let (pa, ha, pp) = injection_toml_fields(&driver.port_injection(&cwd, 0));
                        (raw_cmd, pa, ha, pp, Some(proj_name))
                    }
                } else if is_tty {
                    prompt_manual_config()?
                } else {
                    write_placeholder_toml(&cwd)?;
                    println!("portal.toml created with placeholder. Edit it to configure your project.");
                    return Ok(());
                };

            write_portal_toml(&cwd, &name, &start_command, &port_arg, &host_arg, &port_position)?;
            println!("{} portal.toml created", console::style("✓").green());
            println!("  Run: portal run {}",
                start_command.split_whitespace().next().unwrap_or("your-server"));
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
    use_full_registry: bool,
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

    let injection = {
        let registry = crate::detect::DriverRegistry::new(&config);
        let driver = if use_full_registry {
            registry.detect(&cwd)
        } else {
            registry.detect_language(&cwd)
        };
        driver
            .map(|d| d.port_injection(&cwd, port))
            .unwrap_or(crate::detect::PortInjection::EnvOnly)
    };

    let my_pid = std::process::id();
    let mut child = crate::process::spawn_child(&cwd, &args, port, &hostname, injection).await?;

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
    if tokio::net::UnixStream::connect(&sock).await.is_ok() {
        return Ok(());
    }

    let exe = std::env::current_exe()?;
    let needs_sudo = !cfg!(windows)
        && (config.proxy.http_port < 1024 || config.proxy.https_port < 1024);
    let ca_missing = !crate::config::dirs_for_state().join("ca.pem").exists();

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

        // Check for port conflicts before attempting daemon start.
        let conflicting = check_ports_free(&[config.proxy.http_port, config.proxy.https_port]);
        if !conflicting.is_empty() {
            let occupiers = discover_port_occupiers(&conflicting);
            if occupiers.is_empty() {
                eprintln!(
                    "error: ports {:?} are already in use (cannot identify processes — try: sudo lsof -iTCP:{} -sTCP:LISTEN)",
                    conflicting, conflicting[0]
                );
                return Err(crate::error::Error::DaemonNotRunning);
            }
            match show_conflict_menu(&occupiers)? {
                ConflictAction::KillAndRetry(pids) => {
                    kill_occupiers(&pids, &conflicting).await?;
                }
                ConflictAction::Cancel => {
                    return Err(crate::error::Error::DaemonNotRunning);
                }
            }
        }

        // Plain-text path — no indicatif spinners that could corrupt the TTY that sudo needs.
        if ca_missing {
            setup.plain_step("cert     generating CA certificate…");
        }
        setup.plain_step("daemon   starting (sudo may ask for your password)…");

        // Single blocking call — gives sudo full TTY access so the password prompt
        // and Touch ID (if configured in /etc/pam.d/sudo_local) both work naturally.
        //
        // We intentionally do NOT pass PORTAL_IS_DAEMON=1 here:
        //   1. sudo strips unknown env vars, so .env() on this Command won't reach portal.
        //   2. `portal daemon` without PORTAL_IS_DAEMON enters daemon::start()'s else-branch:
        //      it forwards SUDO_USER/SUDO_UID/SUDO_GID to a grandchild, then exits quickly.
        //   3. The grandchild runs run_daemon_loop() as root with the correct state dir.
        // This mirrors how portless handles it: spawnSync("sudo", args, { stdio: "inherit" }).
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

    for _ in 0..67 {
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

    // Use plain_step (no spinner) — sudo needs raw TTY access, same as daemon start.
    setup.plain_step("trust    installing CA certificate…  (sudo required)");

    let exe = std::env::current_exe()?;
    let status = tokio::process::Command::new("sudo")
        .arg(&exe)
        .arg("cert")
        .arg("install")
        .stdin(std::process::Stdio::inherit())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::inherit())
        .status()
        .await?;

    if !status.success() {
        setup.plain_step(&format!(
            "{} trust   failed  (run `sudo portal cert install` manually)",
            console::style("✗").red()
        ));
        return Err(crate::error::Error::Cert(
            "Failed to install CA certificate. Run `sudo portal cert install` manually."
                .to_string(),
        ));
    }

    setup.plain_step(&format!(
        "{} trust   installed  (sudo)",
        console::style("✓").green()
    ));
    Ok(())
}

// ─── Port conflict resolution ────────────────────────────────────────────────

/// Returns the subset of `ports` that are already bound.
fn check_ports_free(ports: &[u16]) -> Vec<u16> {
    ports
        .iter()
        .copied()
        .filter(|&p| std::net::TcpListener::bind(("0.0.0.0", p)).is_err())
        .collect()
}

/// A process that is currently occupying one or more conflicting ports.
#[derive(Debug, Clone)]
struct PortOccupier {
    pid: u32,
    name: String,
    ports: Vec<u16>,
}

/// Parse the output of `lsof -nP -iTCP:PORT -sTCP:LISTEN -F pcn` and return
/// one `PortOccupier` per unique PID, merging ports for processes that hold
/// multiple conflicting ports.
fn parse_lsof_output(output: &str) -> Vec<PortOccupier> {
    let mut by_pid: std::collections::HashMap<u32, PortOccupier> = std::collections::HashMap::new();
    let mut cur_pid: Option<u32> = None;
    let mut cur_name: Option<String> = None;

    for line in output.lines() {
        if let Some(rest) = line.strip_prefix('p') {
            if let Ok(pid) = rest.parse::<u32>() {
                cur_pid = Some(pid);
                cur_name = None;
            }
        } else if let Some(rest) = line.strip_prefix('c') {
            cur_name = Some(rest.to_string());
        } else if let Some(rest) = line.strip_prefix('n') {
            // address looks like "*:443" or "127.0.0.1:80"
            if let Some(port_str) = rest.rsplit(':').next() {
                if let Ok(port) = port_str.parse::<u16>() {
                    if let (Some(pid), Some(ref name)) = (cur_pid, &cur_name) {
                        let entry = by_pid.entry(pid).or_insert_with(|| PortOccupier {
                            pid,
                            name: name.clone(),
                            ports: Vec::new(),
                        });
                        if !entry.ports.contains(&port) {
                            entry.ports.push(port);
                        }
                    }
                }
            }
        }
    }

    by_pid.into_values().collect()
}

/// Use `lsof` to discover which processes hold the given ports.
fn discover_port_occupiers(ports: &[u16]) -> Vec<PortOccupier> {
    let port_args: Vec<String> = ports
        .iter()
        .map(|p| format!("TCP:{p}"))
        .collect();

    // Build: lsof -nP -iTCP:80 -iTCP:443 -sTCP:LISTEN -F pcn
    let mut cmd = std::process::Command::new("lsof");
    cmd.arg("-nP");
    for a in &port_args {
        cmd.arg(format!("-i{a}"));
    }
    cmd.args(["-sTCP:LISTEN", "-F", "pcn"]);

    let output = match cmd.output() {
        Ok(o) => o,
        Err(_) => return Vec::new(),
    };

    if output.stdout.is_empty() {
        return Vec::new();
    }

    let text = match String::from_utf8(output.stdout) {
        Ok(s) => s,
        Err(_) => return Vec::new(),
    };

    parse_lsof_output(&text)
}

enum ConflictAction {
    KillAndRetry(Vec<u32>),
    Cancel,
}

/// Show an interactive multi-select menu to let the user choose which
/// port-occupying processes to kill.
fn show_conflict_menu(occupiers: &[PortOccupier]) -> crate::error::Result<ConflictAction> {
    use dialoguer::{MultiSelect, Select};

    let port_list: Vec<String> = {
        let mut all: Vec<u16> = occupiers.iter().flat_map(|o| o.ports.iter().copied()).collect();
        all.sort_unstable();
        all.dedup();
        all.iter().map(|p| format!(":{p}")).collect()
    };
    println!(
        "\n  {} {} are already in use\n",
        console::style("port").red(),
        port_list.join(" and ")
    );

    let items: Vec<String> = occupiers
        .iter()
        .map(|o| {
            let ports_str = o.ports.iter().map(|p| format!(":{p}")).collect::<Vec<_>>().join(" ");
            format!(
                "{:<14} pid {:<8} {}",
                o.name, o.pid, ports_str
            )
        })
        .collect();

    let defaults = vec![true; items.len()];
    let selected_indices = MultiSelect::new()
        .with_prompt("Select processes to kill")
        .items(&items)
        .defaults(&defaults)
        .interact_opt()
        .map_err(|e| crate::error::Error::Ipc(e.to_string()))?;

    let selected_indices = match selected_indices {
        Some(v) if !v.is_empty() => v,
        _ => {
            return Ok(ConflictAction::Cancel);
        }
    };

    let action_items = ["Kill selected & retry", "Cancel"];
    let action = Select::new()
        .with_prompt("Action")
        .items(&action_items)
        .default(0)
        .interact_opt()
        .map_err(|e| crate::error::Error::Ipc(e.to_string()))?;

    match action {
        Some(0) => {
            let pids: Vec<u32> = selected_indices.iter().map(|&i| occupiers[i].pid).collect();
            Ok(ConflictAction::KillAndRetry(pids))
        }
        _ => Ok(ConflictAction::Cancel),
    }
}

/// Kill each PID (trying direct kill first, falling back to `sudo kill` for
/// root-owned processes), then poll up to 2 s for ports to be freed.
async fn kill_occupiers(pids: &[u32], ports: &[u16]) -> crate::error::Result<()> {
    for &pid in pids {
        #[cfg(unix)]
        {
            use nix::sys::signal::{kill, Signal};
            use nix::unistd::Pid;
            match kill(Pid::from_raw(pid as i32), Signal::SIGTERM) {
                Ok(_) => {}
                Err(nix::errno::Errno::EPERM) => {
                    // Root-owned process — escalate via sudo.
                    let _ = tokio::process::Command::new("sudo")
                        .args(["kill", &pid.to_string()])
                        .stdin(std::process::Stdio::inherit())
                        .stdout(std::process::Stdio::null())
                        .stderr(std::process::Stdio::inherit())
                        .status()
                        .await;
                }
                Err(_) => {}
            }
        }
        #[cfg(not(unix))]
        {
            let _ = tokio::process::Command::new("taskkill")
                .args(["/PID", &pid.to_string(), "/F"])
                .status()
                .await;
        }
    }

    // Poll up to 2 s for ports to free.
    for _ in 0..20u32 {
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
        if check_ports_free(ports).is_empty() {
            return Ok(());
        }
    }

    eprintln!(
        "  {} warning: ports {:?} still occupied after kill — daemon start may fail",
        console::style("!").yellow(),
        ports
    );
    Ok(())
}

// ─── portal init helpers ─────────────────────────────────────────────────────

fn injection_toml_fields(
    injection: &crate::detect::PortInjection,
) -> (Option<String>, Option<String>, Option<String>) {
    match injection {
        crate::detect::PortInjection::EnvOnly => (None, None, None),
        crate::detect::PortInjection::CliArgs(args) => {
            let port_flag = args.windows(2)
                .find(|w| w[1] == "0")
                .map(|w| w[0].clone());
            let host_flag = args.windows(2)
                .find(|w| w[1] == "0.0.0.0")
                .map(|w| w[0].clone());
            (port_flag, host_flag, None)
        }
        crate::detect::PortInjection::AppendAddress(_) => {
            (None, None, Some("append".to_string()))
        }
    }
}

fn prompt_manual_config() -> crate::error::Result<
    (String, Option<String>, Option<String>, Option<String>, Option<String>)
> {
    let cmd: String = dialoguer::Input::new()
        .with_prompt("What command starts your dev server?")
        .interact_text()
        .map_err(|e| crate::error::Error::Ipc(e.to_string()))?;

    let choices = &[
        "FLAG   --port 4123",
        "FLAG   -p 4123",
        "APPEND 0.0.0.0:4123 (positional)",
        "ENV    PORT=4123 only",
        "CUSTOM I'll write the full command with {port}",
    ];
    let choice = dialoguer::Select::new()
        .with_prompt("How does it accept a port?")
        .items(choices)
        .default(0)
        .interact()
        .map_err(|e| crate::error::Error::Ipc(e.to_string()))?;

    let (pa, ha, pp) = match choice {
        0 => (Some("--port".to_string()), None, None),
        1 => (Some("-p".to_string()),     None, None),
        2 => (None, None, Some("append".to_string())),
        3 => (None, None, None),
        _ => (None, None, None),
    };

    Ok((cmd, pa, ha, pp, None))
}

fn write_portal_toml(
    cwd: &std::path::Path,
    name: &Option<String>,
    start_command: &str,
    port_arg: &Option<String>,
    host_arg: &Option<String>,
    port_position: &Option<String>,
) -> crate::error::Result<()> {
    let mut lines = vec!["[project]".to_string()];
    if let Some(n) = name {
        lines.push(format!("name = {n:?}"));
    }
    if !start_command.is_empty() {
        lines.push(format!("start_command = {start_command:?}"));
    }
    if let Some(pa) = port_arg {
        lines.push(format!("port_arg = {pa:?}"));
    }
    if let Some(ha) = host_arg {
        lines.push(format!("host_arg = {ha:?}"));
    }
    if let Some(pp) = port_position {
        lines.push(format!("port_position = {pp:?}"));
    }
    let content = lines.join("\n") + "\n";
    std::fs::write(cwd.join("portal.toml"), content)?;
    Ok(())
}

fn write_placeholder_toml(cwd: &std::path::Path) -> crate::error::Result<()> {
    let content = r#"[project]
# name = "myapp"
# start_command = "your-dev-command"
# port_arg = "--port"
# host_arg = "--host"
# See: https://github.com/been-there-done-that/portal
"#;
    std::fs::write(cwd.join("portal.toml"), content)?;
    Ok(())
}

// ─── Tests ───────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_lsof_single_process_single_port() {
        let output = "p51055\ncportless\nn*:443\n";
        let occupiers = parse_lsof_output(output);
        assert_eq!(occupiers.len(), 1);
        assert_eq!(occupiers[0].pid, 51055);
        assert_eq!(occupiers[0].name, "portless");
        assert_eq!(occupiers[0].ports, vec![443]);
    }

    #[test]
    fn test_parse_lsof_single_process_two_ports() {
        let output = "p51055\ncportless\nn*:80\nn*:443\n";
        let occupiers = parse_lsof_output(output);
        assert_eq!(occupiers.len(), 1);
        assert_eq!(occupiers[0].pid, 51055);
        let mut ports = occupiers[0].ports.clone();
        ports.sort();
        assert_eq!(ports, vec![80, 443]);
    }

    #[test]
    fn test_parse_lsof_two_processes() {
        let output = "p100\ncnginx\nn*:80\np200\ncportless\nn*:443\n";
        let mut occupiers = parse_lsof_output(output);
        occupiers.sort_by_key(|o| o.pid);
        assert_eq!(occupiers.len(), 2);
        assert_eq!(occupiers[0].pid, 100);
        assert_eq!(occupiers[0].name, "nginx");
        assert_eq!(occupiers[0].ports, vec![80]);
        assert_eq!(occupiers[1].pid, 200);
        assert_eq!(occupiers[1].name, "portless");
        assert_eq!(occupiers[1].ports, vec![443]);
    }

    #[test]
    fn test_parse_lsof_empty_output() {
        let occupiers = parse_lsof_output("");
        assert!(occupiers.is_empty());
    }
}
