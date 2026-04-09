pub mod daemonize;
pub mod ipc;

use crate::certs::CertStore;
use crate::config::{dirs_for_state, Config};
use crate::error::Result;
use crate::proxy::serve_http_redirect;
use crate::route_manager::RouteManager;
use crate::routes::{RouteProtocol, StateStore};
use crate::tcp::TcpRouteManager;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DaemonMode {
    Full,
    TcpOnly,
}

impl DaemonMode {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Full => "full",
            Self::TcpOnly => "tcp_only",
        }
    }
}

/// Entry point called by `portless daemon`.
///
/// Uses the re-spawn approach to avoid fork-in-async-runtime problems:
/// - If PORTAL_IS_DAEMON=1 is set, we are already in the daemon process:
///   run the daemon loop directly.
/// - Otherwise, check if privileged ports need sudo, escalate if necessary,
///   then spawn a fresh copy of the binary with PORTAL_IS_DAEMON=1 and exit.
pub async fn start(mode: DaemonMode) -> Result<()> {
    if std::env::var("PORTAL_IS_DAEMON").as_deref() == Ok("1") {
        run_daemon_loop(mode).await
    } else {
        let state_dir = dirs_for_state();
        std::fs::create_dir_all(&state_dir)?;
        let pid_path = state_dir.join("daemon.pid");

        if daemonize::daemon_already_running(&pid_path) {
            eprintln!("portal daemon already running");
            return Ok(());
        }

        let cwd = std::env::current_dir().unwrap_or_default();
        let config = Config::load(&cwd)?;

        let needs_sudo = matches!(mode, DaemonMode::Full)
            && !cfg!(windows)
            && (config.proxy.http_port < 1024 || config.proxy.https_port < 1024);

        #[cfg(unix)]
        let is_root = unsafe { nix::libc::geteuid() } == 0;
        #[cfg(not(unix))]
        let is_root = true;

        let exe = std::env::current_exe()?;

        if needs_sudo && !is_root {
            // Privileged ports require root. Re-run this command under sudo so
            // the user sees the password prompt — same flow as `portal run`.
            use std::io::IsTerminal;
            if !std::io::stdin().is_terminal() {
                eprintln!("error: ports <1024 require sudo but no TTY is available.");
                return Err(crate::error::Error::DaemonNotRunning);
            }
            let status = std::process::Command::new("sudo")
                .arg(&exe)
                .arg("daemon")
                .args(mode.daemon_args())
                .stdin(std::process::Stdio::inherit())
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::inherit())
                .status()?;
            if !status.success() {
                eprintln!("portal: sudo failed — daemon not started");
                return Err(crate::error::Error::DaemonNotRunning);
            }
            return Ok(());
        }

        // Spawn a detached copy of ourselves as the real daemon.
        // Forward SUDO_* vars so the child can chown state files correctly.
        let mut cmd = tokio::process::Command::new(exe);
        cmd.arg("daemon")
            .args(mode.daemon_args())
            .env("PORTAL_IS_DAEMON", "1")
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null());
        for var in &["SUDO_USER", "SUDO_UID", "SUDO_GID"] {
            if let Ok(val) = std::env::var(var) {
                cmd.env(var, val);
            }
        }
        cmd.spawn()?;

        Ok(())
    }
}

impl DaemonMode {
    pub(crate) fn daemon_args(self) -> &'static [&'static str] {
        match self {
            Self::Full => &[],
            Self::TcpOnly => &["--tcp-only"],
        }
    }
}

async fn run_daemon_loop(mode: DaemonMode) -> Result<()> {
    let state_dir = dirs_for_state();
    std::fs::create_dir_all(&state_dir)?;
    std::fs::create_dir_all(state_dir.join("logs"))?;

    // Chown the state directory to the invoking user when running under sudo,
    // so CLI commands (run as the real user) can also write to it.
    #[cfg(unix)]
    if let Some((uid, gid)) = crate::config::sudo_uid_gid() {
        let certs_dir = state_dir.join("certs");
        let chown_paths: Vec<std::path::PathBuf> = vec![
            state_dir.clone(),
            state_dir.join("logs"),
            certs_dir.clone(),
            certs_dir.join("ca.pem"),
            certs_dir.join("ca-key.pem"),
            certs_dir.join("hosts"),
            state_dir.join("routes.json"),
        ];
        for path in &chown_paths {
            unsafe {
                let p = std::ffi::CString::new(path.to_string_lossy().as_bytes()).unwrap();
                nix::libc::chown(p.as_ptr(), uid, gid);
            }
        }
        // Also chown any existing host certs so the non-root daemon can read them
        if let Ok(entries) = std::fs::read_dir(certs_dir.join("hosts")) {
            for entry in entries.flatten() {
                unsafe {
                    let p =
                        std::ffi::CString::new(entry.path().to_string_lossy().as_bytes()).unwrap();
                    nix::libc::chown(p.as_ptr(), uid, gid);
                }
            }
        }
    }

    let pid_path = state_dir.join("daemon.pid");
    let log_path = state_dir.join("logs").join("daemon.log");

    // Redirect our own stdio to the log file (best-effort)
    redirect_stdio(&log_path);

    // Write our PID
    daemonize::write_pid_file(&pid_path, std::process::id())?;

    // Load config
    let config = Config::load(&std::env::current_dir().unwrap_or_default())?;

    // Init route store and manager
    let store = match StateStore::new(state_dir.join("routes.json")) {
        Ok(r) => r,
        Err(e) => {
            eprintln!("portal: failed to load route store: {e}");
            return Err(e);
        }
    };
    let manager = RouteManager::new(store, TcpRouteManager::default());

    // Clean up stale routes (automatically tears down TCP listeners)
    let _ = manager.remove_stale().await;

    // Restore surviving TCP routes (re-inserting starts their listeners)
    for route in manager
        .list()
        .iter()
        .filter(|r| r.protocol == RouteProtocol::Tcp)
        .cloned()
        .collect::<Vec<_>>()
    {
        if let Err(e) = manager.insert(route.clone()).await {
            tracing::warn!("failed to restore TCP route {}: {e}", route.hostname);
            let _ = manager.remove(&route.hostname).await;
        }
    }

    if matches!(mode, DaemonMode::Full) {
        let cert_store = CertStore::new(state_dir.join("certs"));
        cert_store.ensure_ca()?;
        // Pre-generate the portal.localhost cert at startup so TLS works on first connection.
        // When the daemon runs as root (normal path via ensure_daemon_running), this writes the
        // cert to disk before the chown loop hands ownership back to the user.
        let _ = cert_store.cert_for_host("_.localhost");

        let inspector =
            match crate::inspector::Inspector::start(state_dir.join("inspector.db"), manager.clone()).await {
                Ok(insp) => {
                    let _ = manager
                        .insert(crate::routes::Route {
                            hostname: "_.localhost".to_string(),
                            port: insp.port,
                            public_port: None,
                            protocol: RouteProtocol::Http,
                            pid: std::process::id(),
                            owner_pid: std::process::id(),
                            cwd: String::new(),
                            created_at: chrono::Utc::now(),
                        })
                        .await;
                    tracing::info!(
                        "portal inspector started at _.localhost (internal port {})",
                        insp.port
                    );
                    Some(insp.sender)
                }
                Err(e) => {
                    tracing::warn!("portal inspector failed to start: {e}");
                    None
                }
            };

        let http_bind = format!("0.0.0.0:{}", config.proxy.http_port);
        let https_bind = format!("0.0.0.0:{}", config.proxy.https_port);
        let http_listener = tokio::net::TcpListener::bind(&http_bind).await?;
        let https_listener = tokio::net::TcpListener::bind(&https_bind).await?;

        tracing::info!(
            "portal daemon started (pid={}, mode={}, http={}, https={})",
            std::process::id(),
            mode.as_str(),
            config.proxy.http_port,
            config.proxy.https_port
        );

        let http_https_port = config.proxy.https_port;
        tokio::spawn(serve_http_redirect(
            http_listener,
            config.proxy.http_port,
            http_https_port,
        ));

        {
            let cs = cert_store.clone();
            let rt = manager.store().clone();
            tokio::spawn(serve_https(https_listener, cs, rt, inspector.clone()));
        }
    } else {
        tracing::info!(
            "portal daemon started (pid={}, mode={})",
            std::process::id(),
            mode.as_str()
        );
    }

    // Start IPC server (blocks)
    let sock_path = state_dir.join("portal.sock");
    let ipc = ipc::IpcServer::new(
        sock_path,
        pid_path,
        manager,
        mode,
        config.proxy.https,
        config.proxy.http_port,
        config.proxy.https_port,
    );
    ipc.serve().await;

    Ok(())
}

/// Redirect our stdout/stderr to the log file (Unix only).
fn redirect_stdio(log_path: &std::path::Path) {
    #[cfg(unix)]
    {
        use std::os::unix::io::AsRawFd;
        if let Ok(log) = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(log_path)
        {
            if let Ok(devnull) = std::fs::File::open("/dev/null") {
                unsafe {
                    nix::libc::dup2(devnull.as_raw_fd(), 0);
                    nix::libc::dup2(log.as_raw_fd(), 1);
                    nix::libc::dup2(log.as_raw_fd(), 2);
                }
            }
        }
    }
    #[cfg(not(unix))]
    let _ = log_path;
}

async fn serve_https(
    listener: tokio::net::TcpListener,
    cert_store: CertStore,
    routes: StateStore,
    inspector: Option<crate::inspector::InspectorSender>,
) {
    use hyper::server::conn::http1;
    use hyper_util::rt::TokioIo;
    use rustls::ServerConfig;
    use std::sync::Arc;
    use tokio_rustls::TlsAcceptor;

    let resolver = Arc::new(crate::certs::PortlessCertResolver::new(cert_store));
    let tls_config = Arc::new(
        ServerConfig::builder()
            .with_no_client_auth()
            .with_cert_resolver(resolver),
    );
    let acceptor = TlsAcceptor::from(tls_config);

    loop {
        let Ok((tcp_stream, _)) = listener.accept().await else {
            continue;
        };
        let acceptor = acceptor.clone();
        let routes = routes.clone();
        let inspector = inspector.clone();
        tokio::spawn(async move {
            // Handle Postgres SSLRequest: if the first byte is 0x00, read the
            // 8-byte SSLRequest message and respond with 'S' (yes, use SSL).
            // Then the client sends a normal TLS ClientHello.
            let mut tcp_stream = tcp_stream;
            let first = match crate::proxy::peek_first_byte(&tcp_stream).await {
                Ok(b) => b,
                Err(_) => return,
            };
            if first == 0x00 {
                // Likely Postgres SSLRequest: 8 bytes = 4-byte length + 4-byte code (80877103)
                let mut ssl_req = [0u8; 8];
                if tokio::io::AsyncReadExt::read_exact(&mut tcp_stream, &mut ssl_req).await.is_err() {
                    return;
                }
                let code = u32::from_be_bytes([ssl_req[4], ssl_req[5], ssl_req[6], ssl_req[7]]);
                if code != 80877103 {
                    return; // Not a Postgres SSLRequest
                }
                // Respond with 'S' (yes, use SSL)
                if tokio::io::AsyncWriteExt::write_all(&mut tcp_stream, b"S").await.is_err() {
                    return;
                }
                // Now the client will send a TLS ClientHello — re-peek
                let first = match crate::proxy::peek_first_byte(&tcp_stream).await {
                    Ok(b) => b,
                    Err(_) => return,
                };
                if !crate::proxy::is_tls_client_hello(first) {
                    return;
                }
            } else if !crate::proxy::is_tls_client_hello(first) {
                return;
            }

            let Ok(mut tls_stream) = acceptor.accept(tcp_stream).await else {
                return;
            };

            // Read first bytes to detect HTTP vs raw TCP
            let mut peek_buf = [0u8; 4];
            let n = match tokio::io::AsyncReadExt::read(&mut tls_stream, &mut peek_buf).await {
                Ok(0) => return,
                Ok(n) => n,
                Err(_) => return,
            };
            let peeked = peek_buf[..n].to_vec();

            if crate::proxy::is_http_method_prefix(&peeked) {
                // HTTP path: replay peeked bytes + rest of stream → hyper
                let prefixed = crate::proxy::PrefixedIo::new(peeked, tls_stream);
                let io = hyper_util::rt::TokioIo::new(prefixed);
                hyper::server::conn::http1::Builder::new()
                    .serve_connection(
                        io,
                        hyper::service::service_fn(move |req| {
                            let r = routes.clone();
                            let insp = inspector.clone();
                            async move { crate::proxy::handle_https_request(req, r, insp).await }
                        }),
                    )
                    .with_upgrades()
                    .await
                    .ok();
            } else {
                // TCP bridge: extract SNI hostname → look up route → bridge
                let sni = tls_stream
                    .get_ref()
                    .1
                    .server_name()
                    .map(|s| s.to_string());

                let hostname = match sni {
                    Some(h) => h,
                    None => {
                        tracing::debug!("non-HTTP connection without SNI hostname, dropping");
                        return;
                    }
                };

                let route = match routes.get(&hostname) {
                    Some(r) => r,
                    None => {
                        tracing::debug!("no route for TCP connection to {hostname}");
                        return;
                    }
                };

                let mut backend = match tokio::net::TcpStream::connect(("127.0.0.1", route.port)).await {
                    Ok(s) => s,
                    Err(e) => {
                        tracing::debug!("TCP bridge: failed to connect to backend port {}: {e}", route.port);
                        return;
                    }
                };

                // Send the already-read bytes to the backend
                if tokio::io::AsyncWriteExt::write_all(&mut backend, &peeked).await.is_err() {
                    return;
                }

                // Bridge the rest bidirectionally
                let _ = tokio::io::copy_bidirectional(&mut tls_stream, &mut backend).await;
            }
        });
    }
}
