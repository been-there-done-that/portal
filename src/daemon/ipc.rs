use std::path::PathBuf;

use crate::config::dirs_for_state;
use crate::daemon::DaemonMode;
use crate::proto::{read_frame, write_frame, Command, Response};
use crate::route_manager::RouteManager;
use crate::routes::{Route, RouteProtocol};

pub struct IpcServer {
    sock_path: PathBuf,
    pid_path: PathBuf,
    manager: RouteManager,
    start_time: std::time::Instant,
    mode: DaemonMode,
    https_enabled: bool,
    http_port: u16,
    https_port: u16,
}

impl IpcServer {
    pub fn new(
        sock_path: PathBuf,
        pid_path: PathBuf,
        manager: RouteManager,
        mode: DaemonMode,
        https_enabled: bool,
        http_port: u16,
        https_port: u16,
    ) -> Self {
        // Remove stale socket file if it exists
        std::fs::remove_file(&sock_path).ok();
        IpcServer {
            sock_path,
            pid_path,
            manager,
            start_time: std::time::Instant::now(),
            mode,
            https_enabled,
            http_port,
            https_port,
        }
    }

    pub async fn serve(self) {
        use tokio::net::UnixListener;

        let listener = match UnixListener::bind(&self.sock_path) {
            Ok(l) => l,
            Err(e) => {
                eprintln!("portal: failed to bind IPC socket: {e}");
                return;
            }
        };

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            // When running under sudo, allow the invoking user to connect by
            // setting 0o660 and chown-ing the socket to SUDO_UID:SUDO_GID.
            let (mode, uid_gid) = match crate::config::sudo_uid_gid() {
                Some(ug) => (0o660, Some(ug)),
                None => (0o600, None),
            };
            if let Err(e) =
                std::fs::set_permissions(&self.sock_path, std::fs::Permissions::from_mode(mode))
            {
                eprintln!("portal: failed to set socket permissions: {e}");
            }
            if let Some((uid, gid)) = uid_gid {
                unsafe {
                    let path = std::ffi::CString::new(self.sock_path.to_string_lossy().as_bytes())
                        .unwrap();
                    nix::libc::chown(path.as_ptr(), uid, gid);
                }
            }
        }

        let manager = self.manager.clone();
        let start_time = self.start_time;
        let mode = self.mode;
        let sock_path = self.sock_path.clone();
        let pid_path = self.pid_path.clone();
        let https_enabled = self.https_enabled;
        let http_port = self.http_port;
        let https_port = self.https_port;

        loop {
            match listener.accept().await {
                Ok((stream, _)) => {
                    let manager = manager.clone();
                    let sock = sock_path.clone();
                    let pid = pid_path.clone();
                    tokio::spawn(async move {
                        handle_connection(
                            stream,
                            manager,
                            start_time,
                            mode,
                            sock,
                            pid,
                            https_enabled,
                            http_port,
                            https_port,
                        )
                        .await;
                    });
                }
                Err(e) => {
                    eprintln!("portal: IPC accept error: {e}");
                }
            }
        }
    }
}

async fn handle_connection(
    mut stream: tokio::net::UnixStream,
    manager: RouteManager,
    start_time: std::time::Instant,
    mode: DaemonMode,
    sock_path: PathBuf,
    pid_path: PathBuf,
    https_enabled: bool,
    http_port: u16,
    https_port: u16,
) {
    let cmd: Command = match read_frame(&mut stream).await {
        Ok(c) => c,
        Err(_) => return,
    };

    let response = dispatch(
        cmd,
        manager,
        start_time,
        mode,
        sock_path,
        pid_path,
        https_enabled,
        http_port,
        https_port,
    )
    .await;

    write_frame(&mut stream, &response).await.ok();
}

/// Collect hostnames of all user-registered routes (excludes internal `_.localhost`).
fn user_hostnames(manager: &RouteManager) -> Vec<String> {
    manager.list().into_iter()
        .filter(|r| r.hostname != "_.localhost" && r.protocol == RouteProtocol::Http)
        .map(|r| r.hostname)
        .collect()
}

fn public_url(https_enabled: bool, hostname: &str, http_port: u16, https_port: u16) -> String {
    if https_enabled {
        if https_port == 443 {
            format!("https://{hostname}")
        } else {
            format!("https://{hostname}:{https_port}")
        }
    } else if http_port == 80 {
        format!("http://{hostname}")
    } else {
        format!("http://{hostname}:{http_port}")
    }
}

fn display_target_for_route(
    route: &Route,
    https_enabled: bool,
    http_port: u16,
    https_port: u16,
) -> String {
    match route.protocol {
        RouteProtocol::Http => public_url(https_enabled, &route.hostname, http_port, https_port),
        RouteProtocol::Tcp => format!("localhost:{}", route.public_port.unwrap_or(route.port)),
    }
}

fn route_response_value(
    route: &Route,
    https_enabled: bool,
    http_port: u16,
    https_port: u16,
) -> serde_json::Value {
    let mut value = serde_json::to_value(route).unwrap_or_else(|_| serde_json::json!({}));
    if let serde_json::Value::Object(ref mut obj) = value {
        obj.insert(
            "display_target".to_string(),
            serde_json::Value::String(display_target_for_route(
                route,
                https_enabled,
                http_port,
                https_port,
            )),
        );
    }
    value
}

async fn dispatch(
    cmd: Command,
    manager: RouteManager,
    start_time: std::time::Instant,
    mode: DaemonMode,
    sock_path: PathBuf,
    pid_path: PathBuf,
    https_enabled: bool,
    http_port: u16,
    https_port: u16,
) -> Response {
    match cmd {
        Command::Ls => {
            if let Err(e) = manager.remove_stale().await {
                tracing::warn!("stale route cleanup failed: {e}");
            }
            let list: Vec<_> = manager
                .list()
                .into_iter()
                .filter(|r| r.hostname != "_.localhost")
                .map(|route| route_response_value(&route, https_enabled, http_port, https_port))
                .collect();
            Response::ok(serde_json::Value::Array(list))
        }

        Command::Status => {
            let uptime_secs = start_time.elapsed().as_secs();
            let routes_count = manager
                .list()
                .iter()
                .filter(|r| r.hostname != "_.localhost")
                .count();
            Response::ok(serde_json::json!({
                "version": env!("CARGO_PKG_VERSION"),
                "pid": std::process::id(),
                "uptime_secs": uptime_secs,
                "mode": mode.as_str(),
                "https": https_enabled,
                "http_port": http_port,
                "https_port": https_port,
                "routes_count": routes_count,
            }))
        }

        Command::Stop { hostname } => {
            if hostname.is_empty() {
                return Response::err("hostname required for stop");
            }
            match manager.get(&hostname) {
                None => Response::err(format!("no route for {hostname}")),
                Some(route) => {
                    #[cfg(unix)]
                    {
                        use nix::sys::signal::{killpg, Signal};
                        use nix::unistd::Pid;
                        killpg(Pid::from_raw(route.pid as i32), Signal::SIGTERM).ok();
                    }
                    if let Err(e) = manager.remove(&hostname).await {
                        tracing::warn!("failed to remove route {hostname}: {e}");
                    }
                    Response::ok_empty()
                }
            }
        }

        Command::Rm { hostname } => {
            if let Err(e) = manager.remove(&hostname).await {
                tracing::warn!("failed to remove route {hostname}: {e}");
            }
            Response::ok_empty()
        }

        Command::Shutdown => {
            let shutdown_manager = manager.clone();
            let sock = sock_path.clone();
            let pid = pid_path.clone();
            tokio::spawn(async move {
                shutdown_manager.shutdown_all_tcp().await;
                tokio::time::sleep(std::time::Duration::from_millis(100)).await;
                if let Err(e) = crate::hosts::clean_hosts_file() {
                    tracing::warn!("hosts cleanup on shutdown failed: {e}");
                }
                let _ = std::fs::remove_file(&sock);
                let _ = std::fs::remove_file(&pid);
                std::process::exit(0);
            });
            Response::ok_empty()
        }

        Command::CertInstall => {
            let cert_store = crate::certs::CertStore::new(dirs_for_state().join("certs"));
            match cert_store.install_system_trust() {
                Ok(_) => Response::ok_empty(),
                Err(e) => Response::err(e.to_string()),
            }
        }

        Command::CertReset => {
            let certs_dir = dirs_for_state().join("certs");
            let _ = std::fs::remove_dir_all(&certs_dir);
            let cert_store = crate::certs::CertStore::new(certs_dir);
            match cert_store.ensure_ca() {
                Ok(_) => {}
                Err(e) => return Response::err(e.to_string()),
            }
            match cert_store.install_system_trust() {
                Ok(_) => Response::ok_empty(),
                Err(e) => Response::err(e.to_string()),
            }
        }

        Command::RegisterRoute {
            hostname,
            port,
            public_port,
            protocol,
            pid,
            cwd,
        } => {
            let route = crate::routes::Route {
                hostname: hostname.clone(),
                port,
                public_port,
                protocol,
                pid,
                owner_pid: pid,
                cwd,
                created_at: chrono::Utc::now(),
            };
            match manager.insert(route).await {
                Ok(_) => Response::ok_empty(),
                Err(e) => Response::err(e.to_string()),
            }
        }

        // Note: explicit user command — bypasses PORTAL_SYNC_HOSTS opt-out intentionally.
        Command::HostsSync => {
            let hostnames = user_hostnames(&manager);
            let refs: Vec<&str> = hostnames.iter().map(|s| s.as_str()).collect();
            match crate::hosts::sync_hosts_file(&refs) {
                Ok(_) => {
                    let entries: Vec<serde_json::Value> = refs
                        .iter()
                        .map(|h| serde_json::Value::String(format!("127.0.0.1 {h}")))
                        .collect();
                    Response::ok(serde_json::Value::Array(entries))
                }
                Err(e) => Response::err(e.to_string()),
            }
        }

        Command::HostsClean => match crate::hosts::clean_hosts_file() {
            Ok(_) => Response::ok_empty(),
            Err(e) => Response::err(e.to_string()),
        },

        Command::Run { .. } => Response::err("use portal run from CLI"),
    }
}

#[cfg(test)]
mod stale_tests {
    use super::*;
    use chrono::Utc;
    use tempfile::TempDir;
    use crate::routes::StateStore;
    use crate::tcp::TcpRouteManager;
    use crate::route_manager::RouteManager;

    #[tokio::test]
    async fn ls_removes_stale_tcp_routes_and_releases_public_port() {
        let temp = TempDir::new().unwrap();
        let routes = StateStore::new(temp.path().join("routes.json")).unwrap();
        let tcp_routes = TcpRouteManager::default();
        let manager = RouteManager::new(routes.clone(), tcp_routes.clone());

        let reserved = std::net::TcpListener::bind(("127.0.0.1", 0)).unwrap();
        let public_port = reserved.local_addr().unwrap().port();
        drop(reserved);

        let route = Route {
            hostname: "redis.localhost".to_string(),
            port: public_port + 1,
            public_port: Some(public_port),
            protocol: RouteProtocol::Tcp,
            pid: u32::MAX,
            owner_pid: u32::MAX,
            cwd: "/tmp".to_string(),
            created_at: Utc::now(),
        };
        routes.insert(route.clone()).await.unwrap();
        tcp_routes.ensure_route(&route).await.unwrap();

        let response = dispatch(
            Command::Ls,
            manager,
            std::time::Instant::now(),
            DaemonMode::TcpOnly,
            temp.path().join("portal.sock"),
            temp.path().join("daemon.pid"),
            false,
            80,
            443,
        )
        .await;

        assert!(response.ok);
        assert!(routes.get("redis.localhost").is_none());

        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        let rebound = std::net::TcpListener::bind(("127.0.0.1", public_port));
        assert!(rebound.is_ok());
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::routes::StateStore;
    use crate::tcp::TcpRouteManager;
    use crate::route_manager::RouteManager;

    #[test]
    fn ls_hides_inspector_route() {
        let routes: Vec<String> = vec![
            "myapp.localhost".to_string(),
            "_.localhost".to_string(),
            "api.localhost".to_string(),
        ];
        let filtered: Vec<&String> = routes.iter().filter(|h| *h != "_.localhost").collect();
        assert_eq!(filtered.len(), 2);
        assert!(!filtered.iter().any(|h| *h == "_.localhost"));
    }

    #[tokio::test]
    async fn user_hostnames_excludes_inspector() {
        let dir = tempfile::tempdir().unwrap();
        let store = StateStore::new(dir.path().join("routes.json")).unwrap();
        let tcp_routes = TcpRouteManager::default();
        let manager = RouteManager::new(store.clone(), tcp_routes);

        // Insert a real user route
        store
            .insert(crate::routes::Route {
                hostname: "myapp.localhost".to_string(),
                port: 4000,
                public_port: None,
                protocol: crate::routes::RouteProtocol::Http,
                pid: std::process::id(),
                owner_pid: std::process::id(),
                cwd: "/tmp".to_string(),
                created_at: chrono::Utc::now(),
            })
            .await
            .unwrap();

        // Insert the internal inspector route
        store
            .insert(crate::routes::Route {
                hostname: "_.localhost".to_string(),
                port: 9999,
                public_port: None,
                protocol: crate::routes::RouteProtocol::Http,
                pid: std::process::id(),
                owner_pid: std::process::id(),
                cwd: String::new(),
                created_at: chrono::Utc::now(),
            })
            .await
            .unwrap();

        let result = user_hostnames(&manager);
        assert_eq!(result.len(), 1);
        assert_eq!(result[0], "myapp.localhost");
        assert!(!result.contains(&"_.localhost".to_string()));
    }

    #[test]
    fn display_target_uses_non_default_https_port_for_http_routes() {
        let route = Route {
            hostname: "myapp.localhost".to_string(),
            port: 4000,
            public_port: None,
            protocol: RouteProtocol::Http,
            pid: 1,
            owner_pid: 1,
            cwd: "/tmp".to_string(),
            created_at: chrono::Utc::now(),
        };
        assert_eq!(
            display_target_for_route(&route, true, 80, 4443),
            "https://myapp.localhost:4443"
        );
    }

    #[test]
    fn display_target_uses_public_port_for_tcp_routes() {
        let route = Route {
            hostname: "redis.localhost".to_string(),
            port: 6379,
            public_port: Some(46379),
            protocol: RouteProtocol::Tcp,
            pid: 1,
            owner_pid: 1,
            cwd: "/tmp".to_string(),
            created_at: chrono::Utc::now(),
        };
        assert_eq!(
            display_target_for_route(&route, true, 80, 443),
            "localhost:46379"
        );
    }
}
