use crate::error::{Error, Result};
use crate::routes::{Route, RouteProtocol};
use dashmap::DashMap;
use std::sync::Arc;

struct TcpListenerHandle {
    task: tokio::task::JoinHandle<()>,
}

#[derive(Clone, Default)]
pub struct TcpRouteManager {
    handles: Arc<DashMap<String, TcpListenerHandle>>,
}

impl TcpRouteManager {
    pub async fn ensure_route(&self, route: &Route) -> Result<()> {
        if route.protocol != RouteProtocol::Tcp {
            return Ok(());
        }

        let public_port = route.public_port.ok_or_else(|| {
            Error::Ipc(format!(
                "TCP route {} is missing a public listener port",
                route.hostname
            ))
        })?;

        self.remove(&route.hostname).await;

        let listener = tokio::net::TcpListener::bind(("127.0.0.1", public_port)).await?;
        let backend_port = route.port;
        let hostname = route.hostname.clone();

        let task = tokio::spawn(async move {
            loop {
                let Ok((mut inbound, _)) = listener.accept().await else {
                    break;
                };
                tokio::spawn(async move {
                    let Ok(mut outbound) =
                        tokio::net::TcpStream::connect(("127.0.0.1", backend_port)).await
                    else {
                        return;
                    };
                    let _ = tokio::io::copy_bidirectional(&mut inbound, &mut outbound).await;
                });
            }
        });

        if let Some(old) = self.handles.insert(hostname, TcpListenerHandle { task }) {
            old.task.abort();
        }

        Ok(())
    }

    pub async fn remove(&self, hostname: &str) {
        if let Some((_, handle)) = self.handles.remove(hostname) {
            handle.task.abort();
        }
    }

    pub async fn shutdown_all(&self) {
        let hostnames: Vec<String> = self
            .handles
            .iter()
            .map(|entry| entry.key().clone())
            .collect();
        for hostname in hostnames {
            self.remove(&hostname).await;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Utc;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    #[tokio::test]
    async fn tcp_route_manager_forwards_bytes() {
        let backend = tokio::net::TcpListener::bind(("127.0.0.1", 0))
            .await
            .unwrap();
        let backend_port = backend.local_addr().unwrap().port();

        tokio::spawn(async move {
            let (mut stream, _) = backend.accept().await.unwrap();
            let mut buf = [0u8; 16];
            let n = stream.read(&mut buf).await.unwrap();
            stream.write_all(&buf[..n]).await.unwrap();
        });

        let public_listener = std::net::TcpListener::bind(("127.0.0.1", 0)).unwrap();
        let public_port = public_listener.local_addr().unwrap().port();
        drop(public_listener);

        let manager = TcpRouteManager::default();
        manager
            .ensure_route(&Route {
                hostname: "redis.localhost".to_string(),
                port: backend_port,
                public_port: Some(public_port),
                protocol: RouteProtocol::Tcp,
                pid: std::process::id(),
                owner_pid: std::process::id(),
                cwd: "/tmp".to_string(),
                created_at: Utc::now(),
            })
            .await
            .unwrap();

        let mut client = tokio::net::TcpStream::connect(("127.0.0.1", public_port))
            .await
            .unwrap();
        client.write_all(b"ping").await.unwrap();
        let mut echoed = [0u8; 4];
        client.read_exact(&mut echoed).await.unwrap();
        assert_eq!(&echoed, b"ping");

        manager.remove("redis.localhost").await;
    }
}
