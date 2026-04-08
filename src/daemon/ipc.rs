use std::path::PathBuf;

use crate::config::dirs_for_state;
use crate::proto::{read_frame, write_frame, Command, Response};
use crate::routes::RouteStore;

pub struct IpcServer {
    sock_path: PathBuf,
    pid_path: PathBuf,
    routes: RouteStore,
    start_time: std::time::Instant,
    http_port: u16,
    https_port: u16,
}

impl IpcServer {
    pub fn new(
        sock_path: PathBuf,
        pid_path: PathBuf,
        routes: RouteStore,
        http_port: u16,
        https_port: u16,
    ) -> Self {
        // Remove stale socket file if it exists
        std::fs::remove_file(&sock_path).ok();
        IpcServer {
            sock_path,
            pid_path,
            routes,
            start_time: std::time::Instant::now(),
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
            if let Err(e) = std::fs::set_permissions(
                &self.sock_path,
                std::fs::Permissions::from_mode(mode),
            ) {
                eprintln!("portal: failed to set socket permissions: {e}");
            }
            if let Some((uid, gid)) = uid_gid {
                unsafe {
                    let path = std::ffi::CString::new(
                        self.sock_path.to_string_lossy().as_bytes(),
                    )
                    .unwrap();
                    nix::libc::chown(path.as_ptr(), uid, gid);
                }
            }
        }

        let routes = self.routes.clone();
        let start_time = self.start_time;
        let sock_path = self.sock_path.clone();
        let pid_path = self.pid_path.clone();
        let http_port = self.http_port;
        let https_port = self.https_port;

        loop {
            match listener.accept().await {
                Ok((stream, _)) => {
                    let routes = routes.clone();
                    let sock = sock_path.clone();
                    let pid = pid_path.clone();
                    tokio::spawn(async move {
                        handle_connection(stream, routes, start_time, sock, pid, http_port, https_port).await;
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
    routes: RouteStore,
    start_time: std::time::Instant,
    sock_path: PathBuf,
    pid_path: PathBuf,
    http_port: u16,
    https_port: u16,
) {
    let cmd: Command = match read_frame(&mut stream).await {
        Ok(c) => c,
        Err(_) => return,
    };

    let response = dispatch(cmd, routes, start_time, sock_path, pid_path, http_port, https_port).await;

    write_frame(&mut stream, &response).await.ok();
}

/// Collect hostnames of all user-registered routes (excludes internal `_.localhost`).
fn user_hostnames(routes: &RouteStore) -> Vec<String> {
    routes
        .list()
        .into_iter()
        .filter(|r| r.hostname != "_.localhost")
        .map(|r| r.hostname)
        .collect()
}

/// Sync /etc/hosts with current user routes. Logs a warning on failure, never panics.
fn sync_hosts(routes: &RouteStore) {
    if !crate::hosts::should_sync() {
        return;
    }
    let hostnames = user_hostnames(routes);
    let refs: Vec<&str> = hostnames.iter().map(|s| s.as_str()).collect();
    if let Err(e) = crate::hosts::sync_hosts_file(&refs) {
        tracing::warn!("hosts sync failed: {e}");
    }
}

async fn dispatch(
    cmd: Command,
    routes: RouteStore,
    start_time: std::time::Instant,
    sock_path: PathBuf,
    pid_path: PathBuf,
    http_port: u16,
    https_port: u16,
) -> Response {
    match cmd {
        Command::Ls => {
            let _ = routes.remove_stale();
            let list: Vec<_> = routes
                .list()
                .into_iter()
                .filter(|r| r.hostname != "_.localhost")
                .collect();
            Response::ok(serde_json::to_value(&list).unwrap_or(serde_json::Value::Array(vec![])))
        }

        Command::Status => {
            let uptime_secs = start_time.elapsed().as_secs();
            let routes_count = routes.list().iter().filter(|r| r.hostname != "_.localhost").count();
            Response::ok(serde_json::json!({
                "version": env!("CARGO_PKG_VERSION"),
                "pid": std::process::id(),
                "uptime_secs": uptime_secs,
                "http_port": http_port,
                "https_port": https_port,
                "routes_count": routes_count,
            }))
        }

        Command::Stop { hostname } => {
            if hostname.is_empty() {
                return Response::err("hostname required for stop");
            }
            match routes.get(&hostname) {
                None => Response::err(format!("no route for {hostname}")),
                Some(route) => {
                    #[cfg(unix)]
                    {
                        use nix::sys::signal::{killpg, Signal};
                        use nix::unistd::Pid;
                        killpg(Pid::from_raw(route.pid as i32), Signal::SIGTERM).ok();
                    }
                    let _ = routes.remove(&hostname);
                    sync_hosts(&routes);
                    Response::ok_empty()
                }
            }
        }

        Command::Rm { hostname } => {
            let _ = routes.remove(&hostname);
            sync_hosts(&routes);
            Response::ok_empty()
        }

        Command::Shutdown => {
            let sock = sock_path.clone();
            let pid = pid_path.clone();
            tokio::spawn(async move {
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
            pid,
            cwd,
        } => {
            let route = crate::routes::Route {
                hostname: hostname.clone(),
                port,
                pid,
                owner_pid: pid,
                cwd,
                created_at: chrono::Utc::now(),
            };
            match routes.insert(route) {
                Ok(_) => {
                    sync_hosts(&routes);
                    Response::ok_empty()
                }
                Err(e) => Response::err(e.to_string()),
            }
        }

        // Note: explicit user command — bypasses PORTAL_SYNC_HOSTS opt-out intentionally.
        Command::HostsSync => {
            let hostnames = user_hostnames(&routes);
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
mod tests {
    use super::*;

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

    #[test]
    fn user_hostnames_excludes_inspector() {
        let dir = tempfile::tempdir().unwrap();
        let store = crate::routes::RouteStore::new(dir.path().join("routes.json")).unwrap();

        // Insert a real user route
        store.insert(crate::routes::Route {
            hostname: "myapp.localhost".to_string(),
            port: 4000,
            pid: std::process::id(),
            owner_pid: std::process::id(),
            cwd: "/tmp".to_string(),
            created_at: chrono::Utc::now(),
        }).unwrap();

        // Insert the internal inspector route
        store.insert(crate::routes::Route {
            hostname: "_.localhost".to_string(),
            port: 9999,
            pid: std::process::id(),
            owner_pid: std::process::id(),
            cwd: String::new(),
            created_at: chrono::Utc::now(),
        }).unwrap();

        let result = user_hostnames(&store);
        assert_eq!(result.len(), 1);
        assert_eq!(result[0], "myapp.localhost");
        assert!(!result.contains(&"_.localhost".to_string()));
    }
}
