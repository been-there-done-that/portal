use std::path::PathBuf;

use crate::config::dirs_for_state;
use crate::proto::{Command, Response, read_frame, write_frame};
use crate::routes::RouteStore;

pub struct IpcServer {
    sock_path: PathBuf,
    routes: RouteStore,
    start_time: std::time::Instant,
}

impl IpcServer {
    pub fn new(sock_path: PathBuf, routes: RouteStore) -> Self {
        // Remove stale socket file if it exists
        std::fs::remove_file(&sock_path).ok();
        IpcServer {
            sock_path,
            routes,
            start_time: std::time::Instant::now(),
        }
    }

    pub async fn serve(self) {
        use tokio::net::UnixListener;

        let listener = match UnixListener::bind(&self.sock_path) {
            Ok(l) => l,
            Err(e) => {
                eprintln!("portless: failed to bind IPC socket: {e}");
                return;
            }
        };

        let routes = self.routes.clone();
        let start_time = self.start_time;

        loop {
            match listener.accept().await {
                Ok((stream, _)) => {
                    let routes = routes.clone();
                    tokio::spawn(async move {
                        handle_connection(stream, routes, start_time).await;
                    });
                }
                Err(e) => {
                    eprintln!("portless: IPC accept error: {e}");
                }
            }
        }
    }
}

async fn handle_connection(
    mut stream: tokio::net::UnixStream,
    routes: RouteStore,
    start_time: std::time::Instant,
) {
    let cmd: Command = match read_frame(&mut stream).await {
        Ok(c) => c,
        Err(_) => return,
    };

    let response = dispatch(cmd, routes, start_time).await;

    write_frame(&mut stream, &response).await.ok();
}

async fn dispatch(
    cmd: Command,
    routes: RouteStore,
    start_time: std::time::Instant,
) -> Response {
    match cmd {
        Command::Ls => {
            let _ = routes.remove_stale();
            let list = routes.list();
            Response::ok(serde_json::to_value(&list).unwrap_or(serde_json::Value::Array(vec![])))
        }

        Command::Status => {
            let uptime_secs = start_time.elapsed().as_secs();
            let routes_count = routes.list().len();
            Response::ok(serde_json::json!({
                "version": env!("CARGO_PKG_VERSION"),
                "pid": std::process::id(),
                "uptime_secs": uptime_secs,
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
                        use nix::sys::signal::{Signal, kill};
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
            // Respond first, then exit
            tokio::spawn(async {
                tokio::time::sleep(std::time::Duration::from_millis(100)).await;
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

        Command::Run { .. } => {
            Response::err("use portless run from CLI")
        }
    }
}
