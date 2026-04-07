pub mod daemonize;
pub mod ipc;

use crate::certs::CertStore;
use crate::config::{dirs_for_state, Config};
use crate::error::Result;
use crate::proxy::serve_http_redirect;
use crate::routes::RouteStore;

/// Entry point called by `portless daemon`.
///
/// Uses the re-spawn approach to avoid fork-in-async-runtime problems:
/// - If PORTAL_IS_DAEMON=1 is set, we are already in the daemon process:
///   run the daemon loop directly.
/// - Otherwise, spawn a fresh copy of the binary with PORTAL_IS_DAEMON=1,
///   then exit.
pub async fn start() -> Result<()> {
    if std::env::var("PORTAL_IS_DAEMON").as_deref() == Ok("1") {
        run_daemon_loop().await
    } else {
        let state_dir = dirs_for_state();
        std::fs::create_dir_all(&state_dir)?;
        let pid_path = state_dir.join("daemon.pid");

        if daemonize::daemon_already_running(&pid_path) {
            eprintln!("portal daemon already running");
            return Ok(());
        }

        // Spawn a detached copy of ourselves as the real daemon.
        // Forward SUDO_* vars so the child can chown state files correctly.
        let exe = std::env::current_exe()?;
        let mut cmd = tokio::process::Command::new(exe);
        cmd.arg("daemon")
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

async fn run_daemon_loop() -> Result<()> {
    let state_dir = dirs_for_state();
    std::fs::create_dir_all(&state_dir)?;
    std::fs::create_dir_all(state_dir.join("logs"))?;

    // Chown the state directory to the invoking user when running under sudo,
    // so CLI commands (run as the real user) can also write to it.
    #[cfg(unix)]
    if let Some((uid, gid)) = crate::config::sudo_uid_gid() {
        for path in [
            state_dir.as_path(),
            &state_dir.join("logs"),
            &state_dir.join("certs"),
        ] {
            unsafe {
                let p = std::ffi::CString::new(path.to_string_lossy().as_bytes()).unwrap();
                nix::libc::chown(p.as_ptr(), uid, gid);
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

    // Init route store
    let routes = match RouteStore::new(state_dir.join("routes.json")) {
        Ok(r) => r,
        Err(e) => {
            eprintln!("portal: failed to load route store: {e}");
            return Err(e);
        }
    };

    // Init cert store
    let cert_store = CertStore::new(state_dir.join("certs"));
    cert_store.ensure_ca()?;

    // Start inspector (background worker + axum server at _.localhost)
    let inspector = match crate::inspector::Inspector::start(state_dir.join("inspector.db")).await {
        Ok(insp) => {
            // Register _.localhost in the route table
            let _ = routes.insert(crate::routes::Route {
                hostname: "_.localhost".to_string(),
                port: insp.port,
                pid: std::process::id(),
                owner_pid: std::process::id(),
                cwd: String::new(),
                created_at: chrono::Utc::now(),
            });
            tracing::info!("portal inspector started at _.localhost (internal port {})", insp.port);
            Some(insp.sender)
        }
        Err(e) => {
            tracing::warn!("portal inspector failed to start: {e}");
            None
        }
    };

    // Bind listeners
    let http_bind = format!("0.0.0.0:{}", config.proxy.http_port);
    let https_bind = format!("0.0.0.0:{}", config.proxy.https_port);

    let http_listener = tokio::net::TcpListener::bind(&http_bind).await?;
    let https_listener = tokio::net::TcpListener::bind(&https_bind).await?;

    tracing::info!(
        "portal daemon started (pid={}, http={}, https={})",
        std::process::id(),
        config.proxy.http_port,
        config.proxy.https_port
    );

    // Start HTTP redirect listener
    let http_https_port = config.proxy.https_port;
    tokio::spawn(serve_http_redirect(http_listener, http_https_port));

    // Start HTTPS proxy listener
    {
        let cs = cert_store.clone();
        let rt = routes.clone();
        tokio::spawn(serve_https(https_listener, cs, rt, inspector.clone()));
    }

    // Start IPC server (blocks)
    let sock_path = state_dir.join("portal.sock");
    let ipc = ipc::IpcServer::new(sock_path, pid_path, routes.clone(), config.proxy.http_port, config.proxy.https_port);
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
    routes: RouteStore,
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
            let first = match crate::proxy::peek_first_byte(&tcp_stream).await {
                Ok(b) => b,
                Err(_) => return,
            };
            if !crate::proxy::is_tls_client_hello(first) {
                return;
            }

            let Ok(tls_stream) = acceptor.accept(tcp_stream).await else {
                return;
            };
            let io = TokioIo::new(tls_stream);
            http1::Builder::new()
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
        });
    }
}
