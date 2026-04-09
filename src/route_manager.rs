use crate::routes::{Route, RouteProtocol, StateStore};
use crate::tcp::TcpRouteManager;
use crate::error::Result;

#[derive(Clone)]
pub struct RouteManager {
    pub(crate) store: StateStore,
    pub(crate) tcp: TcpRouteManager,
}

impl RouteManager {
    pub fn new(store: StateStore, tcp: TcpRouteManager) -> Self {
        Self { store, tcp }
    }

    pub fn get(&self, hostname: &str) -> Option<Route> {
        self.store.get(hostname)
    }

    pub fn list(&self) -> Vec<Route> {
        self.store.list()
    }

    pub async fn insert(&self, route: Route) -> Result<()> {
        if route.protocol == RouteProtocol::Tcp {
            self.tcp.ensure_route(&route).await?;
        }
        if let Err(e) = self.store.insert(route.clone()).await {
            if route.protocol == RouteProtocol::Tcp {
                self.tcp.remove(&route.hostname).await;
            }
            return Err(e);
        }
        Ok(())
    }

    pub async fn remove(&self, hostname: &str) -> Result<()> {
        self.tcp.remove(hostname).await;
        self.store.remove(hostname).await
    }

    pub async fn remove_stale(&self) -> Result<Vec<Route>> {
        let removed = self.store.remove_stale().await?;
        for route in &removed {
            if route.protocol == RouteProtocol::Tcp {
                self.tcp.remove(&route.hostname).await;
            }
        }
        Ok(removed)
    }

    pub async fn shutdown_all_tcp(&self) {
        self.tcp.shutdown_all().await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Utc;
    use tempfile::TempDir;

    fn make_http_route(hostname: &str, port: u16) -> Route {
        Route {
            hostname: hostname.to_string(),
            port,
            public_port: None,
            protocol: RouteProtocol::Http,
            pid: std::process::id(),
            owner_pid: std::process::id(),
            cwd: "/tmp".to_string(),
            created_at: Utc::now(),
        }
    }

    fn make_tcp_route(hostname: &str, backend_port: u16, public_port: u16) -> Route {
        Route {
            hostname: hostname.to_string(),
            port: backend_port,
            public_port: Some(public_port),
            protocol: RouteProtocol::Tcp,
            pid: std::process::id(),
            owner_pid: std::process::id(),
            cwd: "/tmp".to_string(),
            created_at: Utc::now(),
        }
    }

    fn make_manager(temp: &TempDir) -> RouteManager {
        let store = StateStore::new(temp.path().join("routes.json")).unwrap();
        let tcp = TcpRouteManager::default();
        RouteManager::new(store, tcp)
    }

    #[tokio::test]
    async fn insert_http_route_persists() {
        let temp = TempDir::new().unwrap();
        let mgr = make_manager(&temp);
        mgr.insert(make_http_route("app.localhost", 4000)).await.unwrap();
        assert!(mgr.get("app.localhost").is_some());
    }

    #[tokio::test]
    async fn insert_tcp_route_starts_listener() {
        let temp = TempDir::new().unwrap();
        let mgr = make_manager(&temp);

        let reserved = std::net::TcpListener::bind(("127.0.0.1", 0)).unwrap();
        let public_port = reserved.local_addr().unwrap().port();
        drop(reserved);

        let backend = tokio::net::TcpListener::bind(("127.0.0.1", 0)).await.unwrap();
        let backend_port = backend.local_addr().unwrap().port();

        mgr.insert(make_tcp_route("redis.localhost", backend_port, public_port))
            .await
            .unwrap();

        let probe = std::net::TcpListener::bind(("127.0.0.1", public_port));
        assert!(probe.is_err(), "expected public port to be in use by TCP listener");
        assert!(mgr.get("redis.localhost").is_some());
    }

    #[tokio::test]
    async fn remove_tcp_route_stops_listener() {
        let temp = TempDir::new().unwrap();
        let mgr = make_manager(&temp);

        let reserved = std::net::TcpListener::bind(("127.0.0.1", 0)).unwrap();
        let public_port = reserved.local_addr().unwrap().port();
        drop(reserved);

        let backend = tokio::net::TcpListener::bind(("127.0.0.1", 0)).await.unwrap();
        let backend_port = backend.local_addr().unwrap().port();

        mgr.insert(make_tcp_route("redis.localhost", backend_port, public_port))
            .await
            .unwrap();
        mgr.remove("redis.localhost").await.unwrap();

        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        let probe = std::net::TcpListener::bind(("127.0.0.1", public_port));
        assert!(probe.is_ok(), "expected public port to be free after remove");
    }

    #[tokio::test]
    async fn remove_stale_tears_down_tcp_listeners() {
        let temp = TempDir::new().unwrap();
        let store = StateStore::new(temp.path().join("routes.json")).unwrap();
        let tcp = TcpRouteManager::default();
        let mgr = RouteManager::new(store.clone(), tcp.clone());

        let reserved = std::net::TcpListener::bind(("127.0.0.1", 0)).unwrap();
        let public_port = reserved.local_addr().unwrap().port();
        drop(reserved);

        let mut route = make_tcp_route("redis.localhost", 9999, public_port);
        route.pid = u32::MAX;
        route.owner_pid = u32::MAX;

        // Insert directly into both stores (simulating daemon startup state)
        store.insert(route.clone()).await.unwrap();
        tcp.ensure_route(&route).await.unwrap();

        let removed = mgr.remove_stale().await.unwrap();
        assert_eq!(removed.len(), 1);
        assert_eq!(removed[0].hostname, "redis.localhost");

        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        let probe = std::net::TcpListener::bind(("127.0.0.1", public_port));
        assert!(probe.is_ok(), "expected public port to be free after stale cleanup");
    }

    #[tokio::test]
    async fn get_and_list_delegate_to_store() {
        let temp = TempDir::new().unwrap();
        let mgr = make_manager(&temp);
        mgr.insert(make_http_route("a.localhost", 4000)).await.unwrap();
        mgr.insert(make_http_route("b.localhost", 4001)).await.unwrap();
        assert_eq!(mgr.list().len(), 2);
        assert!(mgr.get("a.localhost").is_some());
        assert!(mgr.get("nonexistent").is_none());
    }
}
