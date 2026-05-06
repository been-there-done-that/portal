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
pub async fn start(mode: DaemonMode, http_port: Option<u16>, https_port: Option<u16>) -> Result<()> {
    if std::env::var("PORTAL_IS_DAEMON").as_deref() == Ok("1") {
        run_daemon_loop(mode, http_port, https_port).await
    } else {
        let state_dir = dirs_for_state();
        std::fs::create_dir_all(&state_dir)?;
        let pid_path = state_dir.join("daemon.pid");

        if daemonize::daemon_already_running(&pid_path) {
            eprintln!("portal daemon already running");
            return Ok(());
        }

        let cwd = std::env::current_dir().unwrap_or_default();
        let mut config = Config::load(&cwd)?;
        if let Some(p) = http_port { config.proxy.http_port = p; }
        if let Some(p) = https_port { config.proxy.https_port = p; }

        let port_args = port_override_args(config.proxy.http_port, config.proxy.https_port);

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
                .args(&port_args)
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
            .args(&port_args)
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

/// Build `--http-port N --https-port N` args when ports differ from protocol defaults.
pub(crate) fn port_override_args(http_port: u16, https_port: u16) -> Vec<String> {
    let mut args = Vec::new();
    if http_port != 80 {
        args.push("--http-port".to_string());
        args.push(http_port.to_string());
    }
    if https_port != 443 {
        args.push("--https-port".to_string());
        args.push(https_port.to_string());
    }
    args
}

impl DaemonMode {
    pub(crate) fn daemon_args(self) -> &'static [&'static str] {
        match self {
            Self::Full => &[],
            Self::TcpOnly => &["--tcp-only"],
        }
    }
}

async fn run_daemon_loop(mode: DaemonMode, http_port_override: Option<u16>, https_port_override: Option<u16>) -> Result<()> {
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
                let ret = nix::libc::chown(p.as_ptr(), uid, gid);
                if ret != 0 {
                    tracing::warn!(
                        "chown failed for {}: {}",
                        path.display(),
                        std::io::Error::last_os_error()
                    );
                }
            }
        }
        // Also chown any existing host certs so the non-root daemon can read them
        if let Ok(entries) = std::fs::read_dir(certs_dir.join("hosts")) {
            for entry in entries.flatten() {
                unsafe {
                    let entry_path = entry.path();
                    let p =
                        std::ffi::CString::new(entry_path.to_string_lossy().as_bytes()).unwrap();
                    let ret = nix::libc::chown(p.as_ptr(), uid, gid);
                    if ret != 0 {
                        tracing::warn!(
                            "chown failed for {}: {}",
                            entry_path.display(),
                            std::io::Error::last_os_error()
                        );
                    }
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

    // Load config, then apply any port overrides passed via CLI args.
    let mut config = Config::load(&std::env::current_dir().unwrap_or_default())?;
    if let Some(p) = http_port_override { config.proxy.http_port = p; }
    if let Some(p) = https_port_override { config.proxy.https_port = p; }

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
                            slot: 0,
                            label: None,
                            tailscale_url: None,
                            tailscale_https_port: None,
                            tailscale_funnel: false,
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

        // Use SO_REUSEADDR so the new daemon can bind immediately after the old
        // one exits (even with TIME_WAIT connections from active browser sessions).
        let http_listener = {
            let sock = tokio::net::TcpSocket::new_v4()?;
            sock.set_reuseaddr(true)?;
            sock.bind(format!("0.0.0.0:{}", config.proxy.http_port).parse().unwrap())?;
            sock.listen(1024)?
        };
        let https_listener = {
            let sock = tokio::net::TcpSocket::new_v4()?;
            sock.set_reuseaddr(true)?;
            sock.bind(format!("0.0.0.0:{}", config.proxy.https_port).parse().unwrap())?;
            sock.listen(1024)?
        };

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
            tokio::spawn(serve_https(https_listener, cs, rt, inspector.clone(), config.proxy.wildcard));
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
    wildcard: bool,
) {
    use hyper::server::conn::http1;
    use hyper_util::rt::TokioIo;
    use rustls::ServerConfig;
    use std::sync::Arc;
    use tokio_rustls::TlsAcceptor;

    let resolver = Arc::new(crate::certs::PortlessCertResolver::new(cert_store));
    let mut server_config = ServerConfig::builder()
        .with_no_client_auth()
        .with_cert_resolver(resolver);
    server_config.alpn_protocols = vec![b"h2".to_vec(), b"http/1.1".to_vec()];
    let tls_config = Arc::new(server_config);
    let acceptor = TlsAcceptor::from(tls_config);

    loop {
        let Ok((tcp_stream, _)) = listener.accept().await else {
            continue;
        };
        let acceptor = acceptor.clone();
        let routes = routes.clone();
        let inspector = inspector.clone();
        let wc = wildcard;
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
                // Use auto::Builder for HTTP/1.1 + HTTP/2 auto-negotiation via ALPN
                let prefixed = crate::proxy::PrefixedIo::new(peeked, tls_stream);
                let io = hyper_util::rt::TokioIo::new(prefixed);
                hyper_util::server::conn::auto::Builder::new(hyper_util::rt::TokioExecutor::new())
                    .serve_connection_with_upgrades(
                        io,
                        hyper::service::service_fn(move |req| {
                            let r = routes.clone();
                            let insp = inspector.clone();
                            async move { crate::proxy::handle_https_request(req, r, insp, wc, false).await }
                        }),
                    )
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

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::certs::CertStore;
    use crate::routes::{Route, RouteProtocol, StateStore};
    use tempfile::TempDir;

    /// Build a minimal in-memory StateStore backed by a temp file.
    fn temp_store(dir: &TempDir) -> StateStore {
        let path = dir.path().join("routes.json");
        StateStore::new(path).expect("StateStore::new")
    }

    /// Build a CertStore rooted at a temp directory.
    fn temp_cert_store(dir: &TempDir) -> CertStore {
        let cert_dir = dir.path().join("certs");
        std::fs::create_dir_all(&cert_dir).expect("create cert dir");
        CertStore::new(cert_dir)
    }

    /// Spawn a minimal HTTP/1.1 backend that replies 200 with the request's
    /// Host (or :authority) value as the response body.
    async fn spawn_echo_backend() -> (tokio::task::JoinHandle<()>, u16) {
        use hyper::service::service_fn;
        use hyper::Response;
        use hyper_util::rt::TokioIo;
        use http_body_util::Full;
        use bytes::Bytes;

        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind echo backend");
        let port = listener.local_addr().unwrap().port();

        let handle = tokio::spawn(async move {
            loop {
                let Ok((tcp, _)) = listener.accept().await else { break };
                tokio::spawn(async move {
                    let io = TokioIo::new(tcp);
                    hyper::server::conn::http1::Builder::new()
                        .serve_connection(
                            io,
                            service_fn(|req: hyper::Request<hyper::body::Incoming>| async move {
                                let host = req
                                    .headers()
                                    .get(http::header::HOST)
                                    .and_then(|v| v.to_str().ok())
                                    .unwrap_or("")
                                    .to_string();
                                Ok::<_, std::convert::Infallible>(
                                    Response::builder()
                                        .status(200)
                                        .body(Full::new(Bytes::from(host)))
                                        .unwrap(),
                                )
                            }),
                        )
                        .await
                        .ok();
                });
            }
        });

        (handle, port)
    }

    #[tokio::test]
    async fn http2_request_routes_via_authority_header() {
        // rustls requires a CryptoProvider to be installed.  Use ring (which
        // portal itself depends on).  Ignore errors if already installed.
        let _ = rustls::crypto::ring::default_provider().install_default();

        let tmp = TempDir::new().expect("tempdir");
        let cert_store = temp_cert_store(&tmp);
        cert_store.ensure_ca().expect("ensure_ca");

        let routes = temp_store(&tmp);

        // Spawn a backend that echoes the Host header back
        let (_backend_handle, backend_port) = spawn_echo_backend().await;

        // Register a route for the test hostname
        let hostname = "h2test.localhost";
        routes
            .insert(Route {
                hostname: hostname.to_string(),
                port: backend_port,
                public_port: None,
                protocol: RouteProtocol::Http,
                pid: std::process::id(),
                owner_pid: std::process::id(),
                cwd: String::new(),
                created_at: chrono::Utc::now(),
                slot: 0,
                label: None,
                tailscale_url: None,
                tailscale_https_port: None,
                tailscale_funnel: false,
            })
            .await
            .expect("insert route");

        // Bind on a random port and start serve_https
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind https listener");
        let https_port = listener.local_addr().unwrap().port();

        tokio::spawn(serve_https(listener, cert_store, routes, None, false));

        // Give the server a moment to start accepting
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;

        // Send an HTTPS request.  reqwest negotiates HTTP/2 via ALPN when the
        // server advertises "h2" (serve_https configures alpn_protocols).
        // danger_accept_invalid_certs bypasses trust of the self-signed test CA.
        // http2_prior_knowledge() forces HTTP/2 so the :authority pseudo-header
        // is always used instead of the Host header — this is what the test targets.
        let client = reqwest::Client::builder()
            .http2_prior_knowledge()
            .danger_accept_invalid_certs(true)
            .build()
            .expect("build reqwest client");

        let url = format!("https://{}:{}/", hostname, https_port);
        let resp = client
            .get(&url)
            .send()
            .await
            .expect("request should succeed");

        assert_eq!(
            resp.status(),
            reqwest::StatusCode::OK,
            "HTTP/2 request to {hostname} should route to backend (not 404)"
        );
    }
}
