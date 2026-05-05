pub mod banner;
pub mod completion;
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
    Daemon {
        #[arg(long, hide = true)]
        tcp_only: bool,
    },
    /// Auto-detect and start the best dev script from package.json
    Start {
        #[arg(long, short = 'q', help = "Suppress startup banner and running output")]
        quiet: bool,
    },
    /// Run a dev server and assign it a .localhost URL
    Run {
        #[arg(long)]
        hostname: Option<String>,
        #[arg(long)]
        port: Option<u16>,
        #[arg(long, short = 'q', help = "Suppress startup banner and running output")]
        quiet: bool,
        /// Treat as a TCP service (skip HTTPS proxy; for databases, caches, etc.)
        #[arg(long)]
        tcp: bool,
        /// Kill any existing process registered under this hostname and replace it
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
    /// Print the public URL for a named service
    Get {
        /// App name (becomes <name>.<tld>) or full hostname
        name: String,
    },
    /// Find and kill orphaned dev servers left by crashed CLI sessions
    Prune,
    /// Stop daemon, remove CA trust, and delete all portal state
    Clean {
        /// Skip confirmation prompt (required in CI / non-interactive mode)
        #[arg(long, short = 'y')]
        yes: bool,
    },
    /// Register a static route for an already-running service
    Alias {
        /// App name (becomes <name>.localhost)
        name: String,
        /// Port the service is listening on
        #[arg(required_unless_present = "remove")]
        port: Option<u16>,
        /// Overwrite an existing route
        #[arg(long)]
        force: bool,
        /// Remove the alias instead of creating it
        #[arg(long)]
        remove: bool,
    },
    /// Certificate management
    Cert {
        #[command(subcommand)]
        action: CertAction,
    },
    /// Manage /etc/hosts entries for portless routes
    Hosts {
        #[command(subcommand)]
        action: HostsAction,
    },
    /// Show effective configuration
    Config,
    /// Shut down the daemon
    Shutdown,
    /// Open the request inspector in the browser
    Inspect,
    /// Generate portal.toml for this project
    Init,
    /// Generate shell completions for portal
    Completion {
        /// Shell to generate completions for (auto-detected if omitted)
        shell: Option<clap_complete::Shell>,
        /// Print to stdout instead of installing
        #[arg(long, short = 'p')]
        print: bool,
        /// Override the default install directory (filename is appended automatically)
        #[arg(long, value_name = "DIR")]
        path: Option<std::path::PathBuf>,
    },
}

#[derive(Subcommand)]
pub enum CertAction {
    /// Install the local CA into the system trust store
    Install,
    /// Regenerate the local CA and reinstall
    Reset,
}

#[derive(Subcommand)]
pub enum HostsAction {
    /// Force-rewrite the portless block in /etc/hosts from current routes
    Sync,
    /// Remove the portless block from /etc/hosts
    Clean,
}

pub async fn run(cli: Cli) -> Result<()> {
    match cli.command {
        CliCommand::Daemon { tcp_only } => {
            let mode = if tcp_only {
                crate::daemon::DaemonMode::TcpOnly
            } else {
                crate::daemon::DaemonMode::Full
            };

            // Check for port conflicts before starting (same picker as ensure_daemon_running)
            if matches!(mode, crate::daemon::DaemonMode::Full) {
                let cwd = std::env::current_dir().unwrap_or_default();
                let config = crate::config::Config::load(&cwd).unwrap_or_default();
                let ports_to_check = [config.proxy.http_port, config.proxy.https_port];
                let conflicting = check_ports_free(&ports_to_check);
                if !conflicting.is_empty() {
                    let occupiers = discover_port_occupiers(&conflicting);
                    if occupiers.is_empty() {
                        eprintln!(
                            "error: ports {:?} are already in use (cannot identify processes — try: sudo lsof -iTCP:{} -sTCP:LISTEN)",
                            conflicting, conflicting[0]
                        );
                        std::process::exit(1);
                    }
                    use std::io::IsTerminal;
                    if std::io::stdin().is_terminal() {
                        match show_conflict_menu(&occupiers)? {
                            ConflictAction::KillAndRetry(pids) => {
                                kill_occupiers(&pids, &ports_to_check).await?;
                            }
                            ConflictAction::Cancel => {
                                std::process::exit(0);
                            }
                        }
                    } else {
                        eprintln!(
                            "error: ports {:?} are already in use (no TTY for interactive picker)",
                            conflicting
                        );
                        std::process::exit(1);
                    }
                }
            }

            crate::daemon::start(mode).await?;
        }

        CliCommand::Start { quiet } => {
            let cwd = std::env::current_dir()?;
            let config = crate::config::Config::load(&cwd)?;

            // Try monorepo: look for workspace root, discover packages
            if let Some(root) = crate::workspace::find_workspace_root(&cwd) {
                let packages = crate::workspace::discover_workspace_packages(&root, &config);
                if packages.len() > 1 {
                    return run_monorepo(packages, &root, config, quiet).await;
                }
            }

            let registry = crate::detect::DriverRegistry::new(&config);

            let driver = match registry.detect(&cwd) {
                Some(d) => d,
                None => {
                    eprintln!(
                        "No supported project detected. Run `portal init` to set up this project."
                    );
                    std::process::exit(1);
                }
            };

            let raw_cmd = match driver.start_command(&cwd) {
                Some(cmd) => cmd,
                None => {
                    eprintln!(
                        "Detected {} but couldn't determine a start command. Run `portal init`.",
                        driver.name()
                    );
                    std::process::exit(1);
                }
            };

            let hostname_override = config
                .project
                .name
                .clone()
                .or_else(|| driver.project_name(&cwd));

            let args = parse_start_command(driver.name(), &raw_cmd)?;

            do_run(
                cwd,
                config,
                args,
                hostname_override,
                None,
                true,
                quiet,
                false,
                false,
            )
            .await?;
        }

        CliCommand::Ls => {
            let cwd = std::env::current_dir()?;
            let config = crate::config::Config::load(&cwd)?;
            let mut setup = banner::SetupPrinter::new();
            ensure_daemon_running(&config, &mut setup, DaemonRequirement::Any).await?;
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
            ensure_daemon_running(&config, &mut setup, DaemonRequirement::Any).await?;
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

        CliCommand::Get { name } => {
            let cwd = std::env::current_dir()?;
            let config = crate::config::Config::load(&cwd)?;
            let hostname = if name.contains('.') {
                name.clone()
            } else {
                format!("{}.{}", name, config.proxy.tld)
            };
            let mut stream = ipc_connect().await?;
            write_frame(&mut stream, &Command::GetUrl { hostname }).await?;
            let resp: crate::proto::Response = read_frame(&mut stream).await?;
            if resp.ok {
                if let Some(url) = resp.data.as_ref().and_then(|d| d.get("url")).and_then(|u| u.as_str()) {
                    println!("{url}");
                }
            } else {
                eprintln!("error: {}", resp.error.unwrap_or_default());
                std::process::exit(1);
            }
        }

        CliCommand::Prune => {
            let mut stream = ipc_connect().await?;
            write_frame(&mut stream, &Command::Prune).await?;
            let resp: crate::proto::Response = read_frame(&mut stream).await?;
            if resp.ok {
                let pruned = resp.data
                    .as_ref()
                    .and_then(|d| d.as_array())
                    .map(|a| a.iter().filter_map(|v| v.as_str()).collect::<Vec<_>>())
                    .unwrap_or_default();
                if pruned.is_empty() {
                    println!("nothing to prune");
                } else {
                    for h in &pruned {
                        println!("pruned {h}");
                    }
                }
            } else {
                eprintln!("error: {}", resp.error.unwrap_or_default());
                std::process::exit(1);
            }
        }

        CliCommand::Clean { yes } => {
            use std::io::IsTerminal;
            let is_ci = std::env::var("CI").map(|v| !v.is_empty()).unwrap_or(false);
            let is_tty = std::io::stdin().is_terminal();

            if !yes {
                if is_ci || !is_tty {
                    eprintln!("error: --yes required in non-interactive mode");
                    std::process::exit(1);
                }
                use dialoguer::Confirm;
                let confirmed = Confirm::new()
                    .with_prompt("This will stop the daemon and remove all portal state. Continue?")
                    .default(false)
                    .interact()
                    .unwrap_or(false);
                if !confirmed {
                    return Ok(());
                }
            }

            // 1. Try graceful shutdown (ignores error if daemon not running)
            if let Ok(mut stream) = ipc_connect().await {
                let _ = write_frame(&mut stream, &Command::Shutdown).await;
                let _: std::result::Result<crate::proto::Response, _> = read_frame(&mut stream).await;
                // Give daemon time to clean up hosts + remove socket
                tokio::time::sleep(std::time::Duration::from_millis(300)).await;
            }

            // 2. Untrust CA (best-effort)
            if let Err(e) = crate::certs::untrust_system_ca() {
                eprintln!("warning: could not untrust CA: {e}");
            }

            // 3. Remove state directory
            let state_dir = crate::config::dirs_for_state();
            if state_dir.exists() {
                if let Err(e) = std::fs::remove_dir_all(&state_dir) {
                    eprintln!("warning: could not remove state dir: {e}");
                }
            }

            println!("portal state cleared");
        }

        CliCommand::Alias { name, port, force, remove } => {
            let cwd = std::env::current_dir()?;
            let config = crate::config::Config::load(&cwd)?;
            let mut setup = banner::SetupPrinter::quiet();
            ensure_daemon_running(&config, &mut setup, DaemonRequirement::Any).await?;

            let hostname = format!("{}.{}", crate::detect::sanitize_hostname(&name), config.proxy.tld);

            if remove {
                let mut stream = ipc_connect().await?;
                write_frame(&mut stream, &Command::Rm { hostname: hostname.clone() }).await?;
                let resp: crate::proto::Response = read_frame(&mut stream).await?;
                if !resp.ok {
                    let msg = resp.error.as_deref().unwrap_or("unknown error");
                    eprintln!("{} {msg}", console::style("error:").red());
                    std::process::exit(1);
                }
                println!("{} Removed alias: {}", console::style("✓").green(), hostname);
                return Ok(());
            }

            let port = port.expect("port is required when not removing");

            if !force {
                let mut stream = ipc_connect().await?;
                write_frame(&mut stream, &Command::Ls).await?;
                let resp: crate::proto::Response = read_frame(&mut stream).await?;
                if let Some(serde_json::Value::Array(routes)) = resp.data {
                    if routes.iter().any(|r| r["hostname"].as_str() == Some(&hostname)) {
                        eprintln!(
                            "{} Route already exists for {}. Use --force to overwrite.",
                            console::style("error:").red(),
                            hostname
                        );
                        std::process::exit(1);
                    }
                }
            }

            let mut stream = ipc_connect().await?;
            write_frame(
                &mut stream,
                &Command::RegisterRoute {
                    hostname: hostname.clone(),
                    port,
                    public_port: None,
                    protocol: crate::routes::RouteProtocol::Http,
                    pid: 0,
                    cwd: String::new(),
                },
            ).await?;
            let resp: crate::proto::Response = read_frame(&mut stream).await?;
            if !resp.ok {
                let msg = resp.error.as_deref().unwrap_or("unknown error");
                eprintln!("{} {msg}", console::style("error:").red());
                std::process::exit(1);
            }

            let url = build_public_url(&config, &hostname);
            println!(
                "{} {} → localhost:{}",
                console::style("✓").green(),
                console::style(&url).bold(),
                port
            );
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

        CliCommand::Hosts { action } => {
            let (cmd, is_sync) = match action {
                HostsAction::Sync => (Command::HostsSync, true),
                HostsAction::Clean => (Command::HostsClean, false),
            };
            let mut stream = ipc_connect().await?;
            write_frame(&mut stream, &cmd).await?;
            let resp = read_frame(&mut stream).await?;
            if is_sync {
                output::print_hosts_sync(&resp);
            } else {
                output::print_hosts_clean(&resp);
            }
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
            quiet,
            tcp,
            force,
            args,
        } => {
            let cwd = std::env::current_dir()?;
            let config = crate::config::Config::load(&cwd)?;
            let resolved_args = crate::detect::resolve_run_args(&cwd, args);
            do_run(
                cwd,
                config,
                resolved_args,
                hostname,
                port,
                false,
                quiet,
                tcp,
                force,
            )
            .await?;
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

        CliCommand::Completion { shell, print, path } => {
            completion::run(shell, print, path)?;
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

            let (start_command, port_arg, host_arg, port_position, name) = if let Some(driver) =
                detected
            {
                let raw_cmd = driver.start_command(&cwd).unwrap_or_default();
                let proj_name = driver.project_name(&cwd).unwrap_or_else(|| {
                    cwd.file_name()
                        .and_then(|n| n.to_str())
                        .map(crate::detect::sanitize_hostname)
                        .unwrap_or_else(|| "app".to_string())
                });

                if is_tty {
                    println!(
                        "\n  {} Detected  {}",
                        console::style("✓").green(),
                        driver.name()
                    );
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
                println!(
                    "portal.toml created with placeholder. Edit it to configure your project."
                );
                return Ok(());
            };

            write_portal_toml(
                &cwd,
                &name,
                &start_command,
                &port_arg,
                &host_arg,
                &port_position,
            )?;
            println!("{} portal.toml created", console::style("✓").green());
            let preview_args =
                parse_command_line(&start_command).unwrap_or_else(|_| vec![start_command.clone()]);
            println!(
                "  Run: portal run {}",
                preview_args
                    .first()
                    .map(String::as_str)
                    .unwrap_or("your-server")
            );
        }
    }

    Ok(())
}

/// Monorepo orchestration: start all workspace packages concurrently.
async fn run_monorepo(
    packages: Vec<crate::workspace::WorkspacePackage>,
    root: &std::path::Path,
    config: crate::config::Config,
    quiet: bool,
) -> Result<()> {
    let mut setup = if quiet {
        banner::SetupPrinter::quiet()
    } else {
        banner::SetupPrinter::new()
    };
    ensure_daemon_running(&config, &mut setup, DaemonRequirement::Full).await?;
    ensure_cert_trusted(&mut setup).await?;
    setup.done();

    let has_turbo = crate::workspace::has_turbo_config(root);

    let mut handles = Vec::new();

    for pkg in packages {
        let _pkg_config = crate::config::Config::load(&pkg.dir).unwrap_or_else(|_| config.clone());
        let hostname = crate::detect::resolve_hostname(
            &pkg.dir,
            None,
            &config.proxy.tld,
        );
        let public_url = build_public_url(&config, &hostname);

        let args = if has_turbo {
            let pkg_name = pkg.name.clone();
            vec![
                "turbo".to_string(),
                "run".to_string(),
                "dev".to_string(),
                format!("--filter={pkg_name}"),
            ]
        } else {
            pkg.command.clone()
        };

        let port = crate::ports::find_free_port(
            config.proxy.port_range.0,
            config.proxy.port_range.1,
        )?;

        let ca_path = portal_ca_cert_path();
        let mut extra_env = vec![
            ("PORT".to_string(), port.to_string()),
            (crate::process::PORTLESS_URL_ENV.to_string(), public_url.clone()),
        ];
        if config.proxy.https && ca_path.exists() {
            extra_env.push((
                "NODE_EXTRA_CA_CERTS".to_string(),
                ca_path.to_string_lossy().into_owned(),
            ));
        }

        let label = pkg.name.clone();
        let hostname_clone = hostname.clone();
        let cwd = pkg.dir.clone();
        let injection = pkg.injection.clone();
        let config_clone = config.clone();

        let handle = tokio::spawn(async move {
            let mut child = match crate::process::spawn_child(
                &cwd,
                &args,
                port,
                injection,
                &extra_env,
            )
            .await
            {
                Ok(c) => c,
                Err(e) => {
                    eprintln!("[{label}] failed to start: {e}");
                    return;
                }
            };

            let child_pid = child.id().unwrap_or(std::process::id());
            if let Ok(mut stream) = ipc_connect().await {
                let _ = write_frame(
                    &mut stream,
                    &Command::RegisterRoute {
                        hostname: hostname_clone.clone(),
                        port,
                        public_port: None,
                        protocol: crate::routes::RouteProtocol::Http,
                        pid: child_pid,
                        cwd: cwd.to_string_lossy().to_string(),
                    },
                )
                .await;
                let _: std::result::Result<crate::proto::Response, _> = read_frame(&mut stream).await;
            }

            if config_clone.proxy.https {
                println!("[{label}] → https://{hostname_clone}");
            } else {
                println!("[{label}] → http://{hostname_clone}");
            }

            let _ = child.wait().await;
        });

        handles.push(handle);
    }

    for h in handles {
        let _ = h.await;
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
    quiet: bool,
    tcp: bool,
    force: bool,
) -> Result<()> {
    let mut setup = if quiet {
        banner::SetupPrinter::quiet()
    } else {
        banner::SetupPrinter::new()
    };
    let daemon_requirement = if tcp {
        DaemonRequirement::TcpCapable
    } else {
        DaemonRequirement::Full
    };
    ensure_daemon_running(&config, &mut setup, daemon_requirement).await?;
    if !tcp {
        ensure_cert_trusted(&mut setup).await?;
    }
    setup.done();

    let hostname =
        crate::detect::resolve_hostname(&cwd, hostname_override.as_deref(), &config.proxy.tld);
    let public_url = build_public_url(&config, &hostname);

    // Check for an existing live route for this hostname (replace-by-default)
    let existing_route: Option<crate::routes::Route> = {
        let mut stream = ipc_connect().await?;
        write_frame(&mut stream, &Command::Ls).await?;
        let resp: crate::proto::Response = read_frame(&mut stream).await?;
        if let Some(serde_json::Value::Array(routes)) = resp.data {
            routes
                .iter()
                .find(|r| r["hostname"].as_str() == Some(&hostname))
                .and_then(|r| serde_json::from_value::<crate::routes::Route>(r.clone()).ok())
        } else {
            None
        }
    };

    // Guard: error if route is live and --force not set
    if let Some(ref route) = existing_route {
        if !force && route.pid != 0 {
            let alive = {
                #[cfg(unix)]
                {
                    use nix::sys::signal::kill;
                    use nix::unistd::Pid;
                    match i32::try_from(route.pid) {
                        Ok(pid) if pid > 0 => kill(Pid::from_raw(pid), None).is_ok(),
                        _ => false,
                    }
                }
                #[cfg(not(unix))]
                { false }
            };
            if alive {
                eprintln!(
                    "error: {} is already running (PID {}). Use --force to replace it.",
                    hostname, route.pid
                );
                std::process::exit(1);
            }
        }
    }

    // Detect driver early — needed for service_port_candidates and injection.
    let registry = crate::detect::DriverRegistry::new(&config);
    let driver: Option<&dyn crate::detect::LanguageDriver> = if use_full_registry {
        registry.detect(&cwd)
    } else {
        registry.detect_language(&cwd)
    };

    // Check for service-declared port candidates (e.g., Docker Compose).
    // If candidates are present portal uses the declared port and skips pool allocation.
    let declared_port: Option<u16> = {
        let candidates: Vec<(String, u16)> = driver
            .map(|d| d.service_port_candidates(&cwd))
            .unwrap_or_default();
        match candidates.len() {
            0 => None,
            1 => Some(candidates[0].1),
            _ => {
                use std::io::IsTerminal;
                let labels: Vec<String> = candidates
                    .iter()
                    .map(|(name, port)| format!("{name} → {port}"))
                    .collect();
                let idx = if std::io::stdin().is_terminal() {
                    dialoguer::Select::new()
                        .with_prompt("Multiple services found. Which should portal proxy to?")
                        .items(&labels)
                        .default(0)
                        .interact()
                        .unwrap_or(0)
                } else {
                    eprintln!("Multiple services found, selecting first: {}", labels[0]);
                    0
                };
                Some(candidates[idx].1)
            }
        }
    };

    // Determine backend port:
    //   1. User pinned --port       → use it (stop old if exists)
    //   2. Driver declared port     → use it directly (skip pool)
    //   3. Existing route           → stop old, reuse its port
    //   4. No existing route        → find a free port
    let port = if let Some(explicit_port) = port_override {
        crate::ports::validate_app_port(explicit_port)?;
        if existing_route.is_some() {
            let mut s = ipc_connect().await?;
            write_frame(
                &mut s,
                &Command::Stop {
                    hostname: hostname.clone(),
                },
            )
            .await?;
            let _: crate::proto::Response = read_frame(&mut s).await?;
            crate::ports::wait_for_port_free(explicit_port, std::time::Duration::from_secs(2))
                .await;
        }
        explicit_port
    } else if let Some(dp) = declared_port {
        if existing_route.is_some() {
            let mut s = ipc_connect().await?;
            write_frame(
                &mut s,
                &Command::Stop {
                    hostname: hostname.clone(),
                },
            )
            .await?;
            let _: crate::proto::Response = read_frame(&mut s).await?;
            crate::ports::wait_for_port_free(dp, std::time::Duration::from_secs(2)).await;
        }
        dp
    } else if let Some(old_route) = existing_route.as_ref() {
        let mut s = ipc_connect().await?;
        write_frame(
            &mut s,
            &Command::Stop {
                hostname: hostname.clone(),
            },
        )
        .await?;
        let _: crate::proto::Response = read_frame(&mut s).await?;
        crate::ports::wait_for_port_free(old_route.port, std::time::Duration::from_secs(2)).await;
        old_route.port
    } else {
        crate::ports::find_free_port(config.proxy.port_range.0, config.proxy.port_range.1)?
    };

    let public_port = if tcp {
        match existing_route.as_ref() {
            Some(route)
                if route.protocol == crate::routes::RouteProtocol::Tcp
                    && route.public_port.is_some() =>
            {
                route.public_port
            }
            _ => Some(crate::ports::find_free_port_excluding(
                config.proxy.port_range.0,
                config.proxy.port_range.1,
                &[port],
            )?),
        }
    } else {
        None
    };

    // Skip proxy if forced by config or if command is a known build-only tool
    let is_build_only_cmd = config.project.proxy == Some(false)
        || crate::process::is_build_only(&args);

    if is_build_only_cmd {
        // Run directly without registering a proxy route
        let mut child = crate::process::spawn_child(
            &cwd,
            &args,
            0,
            crate::detect::PortInjection::EnvOnly,
            &[],
        )
        .await?;
        let _ = child.wait().await;
        return Ok(());
    }

    let injection = driver
        .map(|d| d.port_injection(&cwd, port))
        .unwrap_or(crate::detect::PortInjection::EnvOnly);

    // Build env vars for the child process
    let port_env_name = config.project.port_env.as_deref().unwrap_or("PORT");
    let mut extra_env: Vec<(String, String)> = vec![(port_env_name.to_string(), port.to_string())];
    if !tcp {
        extra_env.push((crate::process::PORTLESS_URL_ENV.to_string(), public_url.clone()));
        // Inject NODE_EXTRA_CA_CERTS so Node.js child processes trust our local CA
        if config.proxy.https {
            let ca_path = portal_ca_cert_path();
            if ca_path.exists() {
                extra_env.push((
                    "NODE_EXTRA_CA_CERTS".to_string(),
                    ca_path.to_string_lossy().into_owned(),
                ));
            }
        }
    }

    let my_pid = std::process::id();
    let mut child = crate::process::spawn_child(&cwd, &args, port, injection, &extra_env).await?;

    // Register the route in the daemon's live in-memory store via IPC
    let child_pid = child.id().unwrap_or(my_pid);
    if let Ok(mut stream) = ipc_connect().await {
        let _ = write_frame(
            &mut stream,
            &Command::RegisterRoute {
                hostname: hostname.clone(),
                port,
                public_port,
                protocol: if tcp {
                    crate::routes::RouteProtocol::Tcp
                } else {
                    crate::routes::RouteProtocol::Http
                },
                pid: child_pid,
                cwd: cwd.to_string_lossy().to_string(),
            },
        )
        .await;
        let response: crate::proto::Response = read_frame(&mut stream)
            .await
            .unwrap_or(crate::proto::Response::ok_empty());
        if !response.ok {
            let _ = crate::process::stop_child(&mut child).await;
            return Err(crate::error::Error::Ipc(
                response
                    .error
                    .unwrap_or_else(|| "failed to register route".to_string()),
            ));
        }
    }

    if !quiet {
        if tcp {
            banner::print_tcp_banner(
                &hostname,
                public_port.unwrap_or(port),
                port,
                child_pid,
                existing_route.is_some(),
            );
        } else {
            banner::print_banner(&public_url, port, child_pid, existing_route.is_some());
        }
    }

    // Wait for child to exit, or intercept Ctrl+C to stop it gracefully.
    tokio::select! {
        _ = child.wait() => {},
        _ = tokio::signal::ctrl_c() => {
            let _ = crate::process::stop_child(&mut child).await;
            // Deregister the route so the next `portal start` doesn't see a stale entry
            if let Ok(mut s) = ipc_connect().await {
                let _ = write_frame(&mut s, &Command::Stop { hostname: hostname.clone() }).await;
                let _: crate::proto::Response = read_frame(&mut s).await
                    .unwrap_or(crate::proto::Response::ok_empty());
            }
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

fn build_public_url(config: &crate::config::Config, hostname: &str) -> String {
    crate::config::public_url(
        config.proxy.https,
        hostname,
        config.proxy.http_port,
        config.proxy.https_port,
    )
}

fn portal_ca_cert_path() -> std::path::PathBuf {
    crate::config::dirs_for_state().join("certs").join("ca.pem")
}


pub(crate) fn parse_command_line(input: &str) -> Result<Vec<String>> {
    let mut args = Vec::new();
    let mut current = String::new();
    let mut chars = input.chars().peekable();
    let mut quote: Option<char> = None;
    let mut token_started = false;

    while let Some(ch) = chars.next() {
        match quote {
            Some(active_quote) => match ch {
                c if c == active_quote => quote = None,
                '\\' if active_quote == '"' => {
                    let escaped = chars.next().ok_or_else(|| {
                        crate::error::Error::Ipc(
                            "invalid trailing escape in start_command".to_string(),
                        )
                    })?;
                    current.push(escaped);
                    token_started = true;
                }
                _ => {
                    current.push(ch);
                    token_started = true;
                }
            },
            None => match ch {
                '"' | '\'' => {
                    quote = Some(ch);
                    token_started = true;
                }
                '\\' => {
                    let escaped = chars.next().ok_or_else(|| {
                        crate::error::Error::Ipc(
                            "invalid trailing escape in start_command".to_string(),
                        )
                    })?;
                    current.push(escaped);
                    token_started = true;
                }
                c if c.is_whitespace() => {
                    if token_started {
                        args.push(std::mem::take(&mut current));
                        token_started = false;
                    }
                }
                _ => {
                    current.push(ch);
                    token_started = true;
                }
            },
        }
    }

    if quote.is_some() {
        return Err(crate::error::Error::Ipc(
            "unterminated quote in start_command".to_string(),
        ));
    }

    if token_started {
        args.push(current);
    }

    if args.is_empty() {
        return Err(crate::error::Error::Ipc(
            "start_command did not produce any arguments".to_string(),
        ));
    }

    Ok(args)
}

fn parse_start_command(driver_name: &str, raw_cmd: &str) -> Result<Vec<String>> {
    parse_command_line(raw_cmd).map_err(|err| {
        if driver_name == "portal.toml" {
            crate::error::Error::Ipc(format!("invalid start_command in portal.toml: {err}"))
        } else {
            err
        }
    })
}

async fn ensure_daemon_running(
    config: &crate::config::Config,
    setup: &mut banner::SetupPrinter,
    requirement: DaemonRequirement,
) -> Result<()> {
    let sock = crate::config::dirs_for_state().join("portal.sock");

    match probe_running_daemon(&sock).await {
        Some(running_mode) if running_daemon_satisfies_requirement(running_mode, requirement) => {
            return Ok(());
        }
        Some(crate::daemon::DaemonMode::TcpOnly) if requirement == DaemonRequirement::Full => {
            shutdown_running_daemon(&sock).await?;
        }
        _ => {}
    }

    let exe = std::env::current_exe()?;
    let mode = daemon_mode_for_requirement(requirement);
    let needs_sudo = matches!(mode, crate::daemon::DaemonMode::Full)
        && !cfg!(windows)
        && (config.proxy.http_port < 1024 || config.proxy.https_port < 1024);
    let ca_missing =
        matches!(mode, crate::daemon::DaemonMode::Full) && !portal_ca_cert_path().exists();

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
            .args(mode.daemon_args())
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
        .args(mode.daemon_args())
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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum DaemonRequirement {
    Any,
    Full,
    TcpCapable,
}

fn daemon_mode_for_requirement(requirement: DaemonRequirement) -> crate::daemon::DaemonMode {
    match requirement {
        DaemonRequirement::Any | DaemonRequirement::Full => crate::daemon::DaemonMode::Full,
        DaemonRequirement::TcpCapable => crate::daemon::DaemonMode::TcpOnly,
    }
}

fn running_daemon_satisfies_requirement(
    running: crate::daemon::DaemonMode,
    requirement: DaemonRequirement,
) -> bool {
    match requirement {
        DaemonRequirement::Any => true,
        DaemonRequirement::Full => running == crate::daemon::DaemonMode::Full,
        DaemonRequirement::TcpCapable => true,
    }
}

async fn probe_running_daemon(sock: &std::path::Path) -> Option<crate::daemon::DaemonMode> {
    let mut stream = tokio::net::UnixStream::connect(sock).await.ok()?;
    write_frame(&mut stream, &Command::Status).await.ok()?;
    let response: crate::proto::Response = read_frame(&mut stream).await.ok()?;
    if !response.ok {
        return None;
    }
    Some(daemon_mode_from_status(response.data.as_ref()))
}

fn daemon_mode_from_status(data: Option<&serde_json::Value>) -> crate::daemon::DaemonMode {
    match data
        .and_then(|value| value.get("mode"))
        .and_then(serde_json::Value::as_str)
    {
        Some("tcp_only") => crate::daemon::DaemonMode::TcpOnly,
        _ => crate::daemon::DaemonMode::Full,
    }
}

async fn shutdown_running_daemon(sock: &std::path::Path) -> Result<()> {
    let mut stream = tokio::net::UnixStream::connect(sock)
        .await
        .map_err(|_| crate::error::Error::DaemonNotRunning)?;
    write_frame(&mut stream, &Command::Shutdown).await?;
    let _: crate::proto::Response = read_frame(&mut stream).await?;

    for _ in 0..40 {
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
        if tokio::net::UnixStream::connect(sock).await.is_err() {
            return Ok(());
        }
    }

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
        .filter(|&p| {
            // Check if we can connect to the port — if a server is listening, the port is occupied.
            // This avoids SO_REUSEADDR/TIME_WAIT issues that plague bind-based probes.
            std::net::TcpStream::connect_timeout(
                &format!("127.0.0.1:{p}").parse().unwrap(),
                std::time::Duration::from_millis(100),
            )
            .is_ok()
        })
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
/// Falls back to `sudo lsof` when unprivileged lsof can't see root-owned processes.
fn discover_port_occupiers(ports: &[u16]) -> Vec<PortOccupier> {
    let port_args: Vec<String> = ports.iter().map(|p| format!("TCP:{p}")).collect();

    let run_lsof = |use_sudo: bool| -> Option<std::process::Output> {
        let mut cmd = if use_sudo {
            let mut c = std::process::Command::new("sudo");
            c.arg("lsof");
            c
        } else {
            std::process::Command::new("lsof")
        };
        cmd.arg("-nP");
        for a in &port_args {
            cmd.arg(format!("-i{a}"));
        }
        cmd.args(["-sTCP:LISTEN", "-F", "pcn"]);
        if use_sudo {
            // Inherit stdin/stderr so the sudo password prompt works
            cmd.stdin(std::process::Stdio::inherit());
            cmd.stderr(std::process::Stdio::inherit());
        }
        cmd.output().ok()
    };

    // Try unprivileged first
    let output = match run_lsof(false) {
        Some(o) => o,
        None => return Vec::new(),
    };

    let result = if !output.stdout.is_empty() {
        String::from_utf8(output.stdout).ok()
            .map(|t| parse_lsof_output(&t))
            .unwrap_or_default()
    } else {
        Vec::new()
    };

    // If unprivileged lsof found nothing, try sudo lsof (will prompt for password)
    // to discover root-owned processes like a previous portal daemon.
    if result.is_empty() {
        if let Some(sudo_output) = run_lsof(true) {
            if !sudo_output.stdout.is_empty() {
                if let Ok(text) = String::from_utf8(sudo_output.stdout) {
                    return parse_lsof_output(&text);
                }
            }
        }
    }

    result
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
        let mut all: Vec<u16> = occupiers
            .iter()
            .flat_map(|o| o.ports.iter().copied())
            .collect();
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
            let ports_str = o
                .ports
                .iter()
                .map(|p| format!(":{p}"))
                .collect::<Vec<_>>()
                .join(" ");
            format!("{:<14} pid {:<8} {}", o.name, o.pid, ports_str)
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
    // Try graceful portal shutdown first (via IPC) — this cleanly shuts down
    // the daemon including its forked grandchild, hosts cleanup, and socket removal.
    let state_dir = crate::config::dirs_for_state();
    let sock = state_dir.join("portal.sock");

    // Strategy 1: IPC shutdown (clean — handles daemon + grandchild + hosts)
    if sock.exists() {
        eprintln!("  {} sending shutdown to running daemon…", console::style("→").dim());
        if let Ok(mut stream) = tokio::net::UnixStream::connect(&sock).await {
            let _ = crate::proto::write_frame(&mut stream, &crate::proto::Command::Shutdown).await;
            drop(stream);
            // Poll up to 5s — daemon needs time to clean up hosts, remove socket, exit
            for i in 0..50u32 {
                tokio::time::sleep(std::time::Duration::from_millis(100)).await;
                if check_ports_free(ports).is_empty() {
                    eprintln!("  {} old daemon stopped", console::style("✓").green());
                    return Ok(());
                }
                if i == 20 {
                    eprintln!("  {} waiting for daemon to release ports…", console::style("→").dim());
                }
            }
            eprintln!("  {} IPC shutdown sent but ports not yet free, trying kill…", console::style("!").yellow());
        } else {
            eprintln!("  {} socket exists but can't connect, trying kill…", console::style("!").yellow());
        }
    }

    // Strategy 2: sudo kill on the PID from daemon.pid
    let pid_path = state_dir.join("daemon.pid");
    if let Ok(pid_str) = std::fs::read_to_string(&pid_path) {
        if let Ok(daemon_pid) = pid_str.trim().parse::<u32>() {
            eprintln!("  {} killing daemon pid {daemon_pid}…", console::style("→").dim());
            #[cfg(unix)]
            {
                let _ = tokio::process::Command::new("sudo")
                    .args(["kill", "-9", &daemon_pid.to_string()])
                    .stdin(std::process::Stdio::inherit())
                    .stdout(std::process::Stdio::null())
                    .stderr(std::process::Stdio::inherit())
                    .status()
                    .await;
            }
            for _ in 0..20u32 {
                tokio::time::sleep(std::time::Duration::from_millis(100)).await;
                if check_ports_free(ports).is_empty() {
                    let _ = std::fs::remove_file(&pid_path);
                    let _ = std::fs::remove_file(&sock);
                    eprintln!("  {} old daemon killed", console::style("✓").green());
                    return Ok(());
                }
            }
        }
    }

    // Strategy 3: kill process groups from lsof PIDs
    for &pid in pids {
        #[cfg(unix)]
        {
            use nix::sys::signal::{killpg, kill, Signal};
            use nix::unistd::Pid;
            let raw = match i32::try_from(pid) {
                Ok(v) if v > 0 => v,
                _ => continue, // invalid PID — skip
            };
            // Try killing the process group first
            match killpg(Pid::from_raw(raw), Signal::SIGTERM) {
                Ok(_) => {}
                Err(nix::errno::Errno::EPERM) => {
                    // Root-owned — escalate via sudo kill on the process group
                    let _ = tokio::process::Command::new("sudo")
                        .args(["kill", "--", &format!("-{pid}")])
                        .stdin(std::process::Stdio::inherit())
                        .stdout(std::process::Stdio::null())
                        .stderr(std::process::Stdio::inherit())
                        .status()
                        .await;
                }
                Err(_) => {
                    // Process group kill failed — try single PID
                    match kill(Pid::from_raw(raw), Signal::SIGTERM) {
                        Ok(_) => {}
                        Err(nix::errno::Errno::EPERM) => {
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
            let port_flag = args.windows(2).find(|w| w[1] == "0").map(|w| w[0].clone());
            let host_flag = args
                .windows(2)
                .find(|w| w[1] == "0.0.0.0")
                .map(|w| w[0].clone());
            (port_flag, host_flag, None)
        }
        crate::detect::PortInjection::AppendAddress(_) => (None, None, Some("append".to_string())),
    }
}

fn prompt_manual_config() -> crate::error::Result<(
    String,
    Option<String>,
    Option<String>,
    Option<String>,
    Option<String>,
)> {
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
        1 => (Some("-p".to_string()), None, None),
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

    #[test]
    fn parse_command_line_preserves_quotes() {
        let args =
            parse_command_line(r#"python -m uvicorn "app.main:create_app()" --factory"#).unwrap();
        assert_eq!(
            args,
            vec![
                "python",
                "-m",
                "uvicorn",
                "app.main:create_app()",
                "--factory",
            ]
        );
    }

    #[test]
    fn parse_command_line_supports_escaped_spaces() {
        let args = parse_command_line(r#"python path\ with\ spaces/app.py"#).unwrap();
        assert_eq!(args, vec!["python", "path with spaces/app.py"]);
    }

    #[test]
    fn build_public_url_includes_non_default_https_port() {
        assert_eq!(
            crate::config::public_url(true, "myapp.localhost", 80, 4443),
            "https://myapp.localhost:4443"
        );
    }

    #[test]
    fn build_public_url_uses_http_when_https_disabled() {
        assert_eq!(
            crate::config::public_url(false, "myapp.localhost", 8080, 4443),
            "http://myapp.localhost:8080"
        );
    }

    #[test]
    fn portal_ca_cert_path_points_into_certs_directory() {
        let path = portal_ca_cert_path();
        assert!(path.ends_with("certs/ca.pem"));
    }

    #[test]
    fn daemon_mode_defaults_to_full_when_status_has_no_mode() {
        assert_eq!(
            daemon_mode_from_status(None),
            crate::daemon::DaemonMode::Full
        );
        assert_eq!(
            daemon_mode_from_status(Some(&serde_json::json!({ "https": true }))),
            crate::daemon::DaemonMode::Full
        );
    }

    #[test]
    fn daemon_mode_reads_tcp_only_status() {
        assert_eq!(
            daemon_mode_from_status(Some(&serde_json::json!({ "mode": "tcp_only" }))),
            crate::daemon::DaemonMode::TcpOnly
        );
    }

    #[test]
    fn tcp_requirement_accepts_full_or_tcp_only_daemon() {
        assert!(running_daemon_satisfies_requirement(
            crate::daemon::DaemonMode::Full,
            DaemonRequirement::TcpCapable
        ));
        assert!(running_daemon_satisfies_requirement(
            crate::daemon::DaemonMode::TcpOnly,
            DaemonRequirement::TcpCapable
        ));
        assert!(!running_daemon_satisfies_requirement(
            crate::daemon::DaemonMode::TcpOnly,
            DaemonRequirement::Full
        ));
    }

    #[test]
    fn run_command_has_force_arg() {
        use clap::CommandFactory;
        let cmd = Cli::command();
        let run_sub = cmd.find_subcommand("run").expect("run subcommand");
        assert!(
            run_sub.get_arguments().any(|a| a.get_id() == "force"),
            "run subcommand must have --force flag"
        );
    }

    #[test]
    fn workspace_packages_found_in_pnpm_monorepo() {
        use tempfile::TempDir;
        let root = TempDir::new().unwrap();
        std::fs::write(
            root.path().join("pnpm-workspace.yaml"),
            "packages:\n  - \"apps/*\"\n",
        ).unwrap();
        let apps = root.path().join("apps").join("web");
        std::fs::create_dir_all(&apps).unwrap();
        std::fs::write(
            apps.join("package.json"),
            r#"{"name":"web","scripts":{"dev":"vite"}}"#,
        ).unwrap();
        std::fs::write(apps.join("pnpm-lock.yaml"), "").unwrap();

        let config = crate::config::Config::default();
        let pkgs = crate::workspace::discover_workspace_packages(root.path(), &config);
        assert_eq!(pkgs.len(), 1);
    }
}
