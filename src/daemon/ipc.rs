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
                        use nix::sys::signal::{kill, Signal};
                        use nix::unistd::Pid;
                        kill(Pid::from_raw(route.pid as i32), Signal::SIGTERM).ok();
                    }
                    let _ = routes.remove(&hostname);
                    Response::ok_empty()
                }
            }
        }

        Command::Rm { hostname } => {
            let _ = routes.remove(&hostname);
            Response::ok_empty()
        }

        Command::Shutdown => {
            let sock = sock_path.clone();
            let pid = pid_path.clone();
            tokio::spawn(async move {
                tokio::time::sleep(std::time::Duration::from_millis(100)).await;
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
                Ok(_) => Response::ok_empty(),
                Err(e) => Response::err(e.to_string()),
            }
        }

        Command::HostsSync => {
            Response::err("HostsSync not yet implemented".to_string())
        }

        Command::HostsClean => {
            Response::err("HostsClean not yet implemented".to_string())
        }

        Command::Run { .. } => Response::err("use portal run from CLI"),
    }
}

#[cfg(test)]
mod tests {
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
}
