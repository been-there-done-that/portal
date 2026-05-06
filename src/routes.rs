use crate::error::Result;
use chrono::{DateTime, Utc};
use dashmap::DashMap;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use std::sync::Arc;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum RouteProtocol {
    #[default]
    Http,
    Tcp,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Route {
    pub hostname: String,
    pub port: u16,
    #[serde(default)]
    pub public_port: Option<u16>,
    #[serde(default)]
    pub protocol: RouteProtocol,
    pub pid: u32,
    #[serde(default)]
    pub owner_pid: u32,
    #[serde(default)]
    pub cwd: String,
    #[serde(default = "chrono::Utc::now")]
    pub created_at: DateTime<Utc>,
    // Multiplexed routing
    #[serde(default)]
    pub slot: u32,
    #[serde(default)]
    pub label: Option<String>,
    // Tailscale
    #[serde(default)]
    pub tailscale_url: Option<String>,
    #[serde(default)]
    pub tailscale_https_port: Option<u16>,
    #[serde(default)]
    pub tailscale_funnel: bool,
}

/// Thread-safe store backed by DashMap.
/// Reads (get, list) are lock-free.
/// Writes (insert, remove, remove_stale) are serialised under a tokio Mutex
/// and atomically update routes.json + /etc/hosts in one locked transaction.
#[derive(Clone)]
pub struct StateStore {
    map: Arc<DashMap<String, Vec<Route>>>,
    write_lock: Arc<tokio::sync::Mutex<()>>,
    path: PathBuf,
}

impl StateStore {
    /// Create a new StateStore. Loads existing routes from disk if file exists.
    pub fn new(path: PathBuf) -> Result<Self> {
        let map: Arc<DashMap<String, Vec<Route>>> = Arc::new(DashMap::new());
        if path.exists() {
            let contents = std::fs::read_to_string(&path)?;
            if !contents.is_empty() {
                let routes: Vec<Route> = serde_json::from_str(&contents)?;
                for route in routes {
                    map.entry(route.hostname.clone()).or_default().push(route);
                }
            }
        }
        Ok(Self {
            map,
            write_lock: Arc::new(tokio::sync::Mutex::new(())),
            path,
        })
    }

    // ── Read API (lock-free) ──────────────────────────────────────────────

    pub fn get(&self, hostname: &str) -> Option<Route> {
        self.map.get(hostname).and_then(|v| v.first().cloned())
    }

    pub fn get_slot(&self, hostname: &str, slot: u32) -> Option<Route> {
        self.map
            .get(hostname)
            .and_then(|v| v.iter().find(|r| r.slot == slot).cloned())
    }

    pub fn list_slots(&self, hostname: &str) -> Vec<Route> {
        self.map
            .get(hostname)
            .map(|v| v.clone())
            .unwrap_or_default()
    }

    pub fn list(&self) -> Vec<Route> {
        self.map
            .iter()
            .flat_map(|e| e.value().clone())
            .collect()
    }

    // ── Write API (serialised) ────────────────────────────────────────────

    pub async fn insert(&self, mut route: Route) -> Result<()> {
        let _guard = self.write_lock.lock().await;
        let mut entry = self.map.entry(route.hostname.clone()).or_default();
        let slots = entry.value_mut();
        if slots.is_empty() {
            // First route for this hostname — slot stays as-is (0 or what was set)
        } else if route.slot == 0 {
            // Auto-assign next slot (caller didn't specify one)
            let max_slot = slots.iter().map(|r| r.slot).max().unwrap_or(0);
            route.slot = max_slot + 1;
        } else {
            // Non-zero explicit slot: replace if exists, otherwise push
            if let Some(existing) = slots.iter_mut().find(|r| r.slot == route.slot) {
                *existing = route;
                drop(entry);
                self.persist_locked()?;
                self.sync_hosts_locked();
                return Ok(());
            }
        }
        slots.push(route);
        // Sort by slot number to maintain ordering
        slots.sort_by_key(|r| r.slot);
        drop(entry);
        self.persist_locked()?;
        self.sync_hosts_locked();
        Ok(())
    }

    /// Replace a specific slot in-place without auto-slot-assignment.
    /// Returns an error if the hostname or slot does not exist.
    pub async fn update_slot(&self, route: Route) -> Result<()> {
        let _guard = self.write_lock.lock().await;
        let mut entry = match self.map.get_mut(&route.hostname) {
            Some(e) => e,
            None => return Err(crate::error::Error::HostNotFound(route.hostname.clone())),
        };
        let slots = entry.value_mut();
        match slots.iter_mut().find(|r| r.slot == route.slot) {
            Some(existing) => {
                *existing = route;
                drop(entry);
                self.persist_locked()?;
                self.sync_hosts_locked();
                Ok(())
            }
            None => Err(crate::error::Error::Ipc(
                format!("slot {} not found for \"{}\"", route.slot, route.hostname)
            )),
        }
    }

    pub async fn remove(&self, hostname: &str) -> Result<()> {
        let _guard = self.write_lock.lock().await;
        self.map.remove(hostname);
        self.persist_locked()?;
        self.sync_hosts_locked();
        Ok(())
    }

    pub async fn remove_slot(&self, hostname: &str, slot: u32) -> Result<()> {
        let _guard = self.write_lock.lock().await;
        let mut changed = false;
        {
            let mut entry = match self.map.get_mut(hostname) {
                Some(e) => e,
                None => return Ok(()),
            };
            let slots = entry.value_mut();
            let before = slots.len();
            slots.retain(|r| r.slot != slot);
            changed = slots.len() != before;
            // If primary (slot 0) was removed, promote slot 1 to slot 0
            if changed && !slots.is_empty() && !slots.iter().any(|r| r.slot == 0) {
                if let Some(first) = slots.iter_mut().min_by_key(|r| r.slot) {
                    first.slot = 0;
                }
                slots.sort_by_key(|r| r.slot);
            }
            // Remove the hostname entirely if no slots remain
            if slots.is_empty() {
                drop(entry);
                self.map.remove(hostname);
            }
        }
        if changed {
            self.persist_locked()?;
            self.sync_hosts_locked();
        }
        Ok(())
    }

    pub async fn remove_stale(&self) -> Result<Vec<Route>> {
        let _guard = self.write_lock.lock().await;

        let mut removed: Vec<Route> = Vec::new();
        let mut hostnames_to_remove: Vec<String> = Vec::new();

        for mut entry in self.map.iter_mut() {
            let slots = entry.value_mut();
            let dead: Vec<Route> = slots
                .iter()
                .filter(|r| !pid_alive_check(r.pid))
                .cloned()
                .collect();
            if dead.is_empty() {
                continue;
            }
            slots.retain(|r| pid_alive_check(r.pid));
            removed.extend(dead);
            if slots.is_empty() {
                hostnames_to_remove.push(entry.key().clone());
            }
        }

        for hostname in &hostnames_to_remove {
            self.map.remove(hostname);
        }

        if !removed.is_empty() {
            self.persist_locked()?;
            self.sync_hosts_locked();
        }
        Ok(removed)
    }

    // ── Private helpers (called while write_lock is held) ─────────────────

    fn persist_locked(&self) -> Result<()> {
        let routes: Vec<Route> = self
            .map
            .iter()
            .flat_map(|e| e.value().clone())
            .collect();
        let json = serde_json::to_string_pretty(&routes)?;
        let tmp_path = format!("{}.tmp", self.path.display());
        std::fs::write(&tmp_path, &json)?;
        std::fs::rename(&tmp_path, &self.path)?;

        #[cfg(unix)]
        if let Some((uid, gid)) = crate::config::sudo_uid_gid() {
            unsafe {
                let p = std::ffi::CString::new(self.path.to_string_lossy().as_bytes()).unwrap();
                let ret = nix::libc::chown(p.as_ptr(), uid, gid);
                if ret != 0 {
                    tracing::warn!(
                        "chown failed for {}: {}",
                        self.path.display(),
                        std::io::Error::last_os_error()
                    );
                }
            }
        }
        Ok(())
    }

    fn sync_hosts_locked(&self) {
        if !crate::hosts::should_sync() {
            return;
        }
        let hostnames: Vec<String> = self
            .map
            .iter()
            .filter(|e| {
                e.value()
                    .first()
                    .map(|r| r.protocol == RouteProtocol::Http)
                    .unwrap_or(false)
            })
            .map(|e| e.key().clone())
            .collect();
        let refs: Vec<&str> = hostnames.iter().map(|s| s.as_str()).collect();
        if let Err(e) = crate::hosts::sync_hosts_file(&refs) {
            tracing::warn!("hosts sync failed: {e}");
        }
    }
}

/// Check if a process with the given PID is alive.
pub fn pid_alive_check(pid: u32) -> bool {
    // pid=0 is the alias sentinel — aliases are never stale
    if pid == 0 {
        return true;
    }
    #[cfg(unix)]
    {
        use nix::sys::signal::kill;
        use nix::unistd::Pid;
        // Reject PIDs that would overflow i32 (e.g. u32::MAX wraps to -1,
        // which has special kill(2) semantics and does not represent a real PID).
        let raw = match i32::try_from(pid) {
            Ok(v) if v > 0 => v,
            _ => return false,
        };
        match kill(Pid::from_raw(raw), None) {
            Ok(_) => true,                         // process exists and we can signal it
            Err(nix::errno::Errno::EPERM) => true, // exists but owned by another user
            Err(_) => false,                       // ESRCH = no such process, or other error
        }
    }

    #[cfg(windows)]
    {
        use windows_sys::Win32::Foundation::CloseHandle;
        use windows_sys::Win32::System::Threading::OpenProcess;
        use windows_sys::Win32::System::Threading::PROCESS_QUERY_INFORMATION;

        unsafe {
            // Try to open the process with query information access
            let handle = OpenProcess(PROCESS_QUERY_INFORMATION, 0, pid);
            if handle.is_null() {
                return false;
            }
            // Close the handle
            let _ = CloseHandle(handle);
            true
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[tokio::test]
    async fn insert_and_get() {
        let temp = TempDir::new().unwrap();
        let store_path = temp.path().join("routes.json");
        let store = StateStore::new(store_path).unwrap();

        let route = Route {
            hostname: "myapp.localhost".to_string(),
            port: 4000,
            public_port: None,
            protocol: RouteProtocol::Http,
            pid: std::process::id(),
            owner_pid: std::process::id(),
            cwd: "/tmp".to_string(),
            created_at: Utc::now(),
            slot: 0,
            label: None,
            tailscale_url: None,
            tailscale_https_port: None,
            tailscale_funnel: false,
        };

        store.insert(route.clone()).await.unwrap();

        let retrieved = store.get("myapp.localhost").unwrap();
        assert_eq!(retrieved.port, 4000);
        assert_eq!(retrieved.hostname, "myapp.localhost");
    }

    #[tokio::test]
    async fn persists_across_reload() {
        let temp = TempDir::new().unwrap();
        let store_path = temp.path().join("routes.json");

        {
            let store = StateStore::new(store_path.clone()).unwrap();
            let route = Route {
                hostname: "myapp.localhost".to_string(),
                port: 4000,
                public_port: None,
                protocol: RouteProtocol::Http,
                pid: std::process::id(),
                owner_pid: std::process::id(),
                cwd: "/tmp".to_string(),
                created_at: Utc::now(),
                slot: 0,
                label: None,
                tailscale_url: None,
                tailscale_https_port: None,
                tailscale_funnel: false,
            };
            store.insert(route).await.unwrap();
        }

        // Create new store from same path
        let store = StateStore::new(store_path).unwrap();
        let retrieved = store.get("myapp.localhost").unwrap();
        assert_eq!(retrieved.port, 4000);
    }

    #[tokio::test]
    async fn remove_works() {
        let temp = TempDir::new().unwrap();
        let store_path = temp.path().join("routes.json");
        let store = StateStore::new(store_path).unwrap();

        let route = Route {
            hostname: "myapp.localhost".to_string(),
            port: 4000,
            public_port: None,
            protocol: RouteProtocol::Http,
            pid: std::process::id(),
            owner_pid: std::process::id(),
            cwd: "/tmp".to_string(),
            created_at: Utc::now(),
            slot: 0,
            label: None,
            tailscale_url: None,
            tailscale_https_port: None,
            tailscale_funnel: false,
        };

        store.insert(route).await.unwrap();
        assert!(store.get("myapp.localhost").is_some());

        store.remove("myapp.localhost").await.unwrap();
        assert!(store.get("myapp.localhost").is_none());
    }

    #[tokio::test]
    async fn stale_cleanup_removes_dead_pids() {
        let temp = TempDir::new().unwrap();
        let store_path = temp.path().join("routes.json");
        let store = StateStore::new(store_path).unwrap();

        // Insert route with current process PID (definitely alive)
        let current_pid = std::process::id();
        let route_alive = Route {
            hostname: "alive.localhost".to_string(),
            port: 4000,
            public_port: None,
            protocol: RouteProtocol::Http,
            pid: current_pid,
            owner_pid: current_pid,
            cwd: "/tmp".to_string(),
            created_at: Utc::now(),
            slot: 0,
            label: None,
            tailscale_url: None,
            tailscale_https_port: None,
            tailscale_funnel: false,
        };

        // Insert route with invalid PID
        let route_dead = Route {
            hostname: "dead.localhost".to_string(),
            port: 4001,
            public_port: None,
            protocol: RouteProtocol::Http,
            pid: u32::MAX, // Invalid PID, definitely dead
            owner_pid: u32::MAX,
            cwd: "/tmp".to_string(),
            created_at: Utc::now(),
            slot: 0,
            label: None,
            tailscale_url: None,
            tailscale_https_port: None,
            tailscale_funnel: false,
        };

        store.insert(route_alive).await.unwrap();
        store.insert(route_dead).await.unwrap();

        // Run cleanup
        let removed = store.remove_stale().await.unwrap();

        // Alive route should remain
        assert!(store.get("alive.localhost").is_some());
        // Dead route should be removed
        assert!(store.get("dead.localhost").is_none());
        assert_eq!(removed.len(), 1);
        assert_eq!(removed[0].hostname, "dead.localhost");
    }

    #[tokio::test]
    async fn stale_cleanup_returns_removed_tcp_routes() {
        let temp = TempDir::new().unwrap();
        let store_path = temp.path().join("routes.json");
        let store = StateStore::new(store_path).unwrap();

        store
            .insert(Route {
                hostname: "redis.localhost".to_string(),
                port: 6379,
                public_port: Some(46379),
                protocol: RouteProtocol::Tcp,
                pid: u32::MAX,
                owner_pid: u32::MAX,
                cwd: "/tmp".to_string(),
                created_at: Utc::now(),
                slot: 0,
                label: None,
                tailscale_url: None,
                tailscale_https_port: None,
                tailscale_funnel: false,
            })
            .await
            .unwrap();

        let removed = store.remove_stale().await.unwrap();
        assert_eq!(removed.len(), 1);
        assert_eq!(removed[0].protocol, RouteProtocol::Tcp);
        assert_eq!(removed[0].public_port, Some(46379));
    }

    #[tokio::test]
    async fn concurrent_inserts_no_data_loss() {
        use std::sync::Arc;
        let temp = tempfile::TempDir::new().unwrap();
        let store = Arc::new(StateStore::new(temp.path().join("routes.json")).unwrap());

        let mut handles = vec![];
        for i in 0u32..20 {
            let s = Arc::clone(&store);
            handles.push(tokio::spawn(async move {
                s.insert(crate::routes::Route {
                    hostname: format!("app{i}.localhost"),
                    port: 4000 + i as u16,
                    public_port: None,
                    protocol: RouteProtocol::Http,
                    pid: std::process::id(),
                    owner_pid: std::process::id(),
                    cwd: "/tmp".to_string(),
                    created_at: chrono::Utc::now(),
                    slot: 0,
                    label: None,
                    tailscale_url: None,
                    tailscale_https_port: None,
                    tailscale_funnel: false,
                })
                .await
                .unwrap();
            }));
        }
        for h in handles {
            h.await.unwrap();
        }

        assert_eq!(store.list().len(), 20);

        let store2 = StateStore::new(temp.path().join("routes.json")).unwrap();
        assert_eq!(store2.list().len(), 20);
    }

    #[tokio::test]
    async fn old_http_routes_deserialize_without_protocol_fields() {
        let temp = tempfile::TempDir::new().unwrap();
        let store_path = temp.path().join("routes.json");
        std::fs::write(
            &store_path,
            r#"[{"hostname":"legacy.localhost","port":4000,"pid":1,"owner_pid":1,"cwd":"/tmp","created_at":"2026-04-09T00:00:00Z"}]"#,
        )
        .unwrap();

        let store = StateStore::new(store_path).unwrap();
        let route = store.get("legacy.localhost").unwrap();
        assert_eq!(route.protocol, RouteProtocol::Http);
        assert_eq!(route.public_port, None);
    }

    #[test]
    fn pid_alive_check_returns_true_for_zero_alias_sentinel() {
        assert!(pid_alive_check(0), "pid 0 (alias sentinel) should always be considered alive");
    }

    #[tokio::test]
    async fn insert_second_route_auto_assigns_slot() {
        let temp = TempDir::new().unwrap();
        let store = StateStore::new(temp.path().join("routes.json")).unwrap();

        let route1 = Route {
            hostname: "myapp.localhost".to_string(),
            port: 4000,
            public_port: None,
            protocol: RouteProtocol::Http,
            pid: std::process::id(),
            owner_pid: std::process::id(),
            cwd: "/tmp".to_string(),
            created_at: Utc::now(),
            slot: 0,
            label: None,
            tailscale_url: None,
            tailscale_https_port: None,
            tailscale_funnel: false,
        };
        let mut route2 = route1.clone();
        route2.port = 4001;
        route2.slot = 0; // will be auto-assigned to 1

        store.insert(route1).await.unwrap();
        store.insert(route2).await.unwrap();

        let slots = store.list_slots("myapp.localhost");
        assert_eq!(slots.len(), 2);
        assert_eq!(slots[0].slot, 0);
        assert_eq!(slots[1].slot, 1);
        assert_eq!(slots[0].port, 4000);
        assert_eq!(slots[1].port, 4001);
    }

    #[tokio::test]
    async fn get_returns_primary_slot() {
        let temp = TempDir::new().unwrap();
        let store = StateStore::new(temp.path().join("routes.json")).unwrap();

        let route = Route {
            hostname: "myapp.localhost".to_string(),
            port: 4000,
            public_port: None,
            protocol: RouteProtocol::Http,
            pid: std::process::id(),
            owner_pid: std::process::id(),
            cwd: "/tmp".to_string(),
            created_at: Utc::now(),
            slot: 0,
            label: None,
            tailscale_url: None,
            tailscale_https_port: None,
            tailscale_funnel: false,
        };
        let mut route2 = route.clone();
        route2.port = 4001;

        store.insert(route).await.unwrap();
        store.insert(route2).await.unwrap();

        let primary = store.get("myapp.localhost").unwrap();
        assert_eq!(primary.port, 4000, "get() should return slot 0 (primary)");
    }

    #[tokio::test]
    async fn remove_slot_promotes_next() {
        let temp = TempDir::new().unwrap();
        let store = StateStore::new(temp.path().join("routes.json")).unwrap();

        let route = Route {
            hostname: "myapp.localhost".to_string(),
            port: 4000,
            public_port: None,
            protocol: RouteProtocol::Http,
            pid: std::process::id(),
            owner_pid: std::process::id(),
            cwd: "/tmp".to_string(),
            created_at: Utc::now(),
            slot: 0,
            label: None,
            tailscale_url: None,
            tailscale_https_port: None,
            tailscale_funnel: false,
        };
        let mut route2 = route.clone();
        route2.port = 4001;

        store.insert(route).await.unwrap();
        store.insert(route2).await.unwrap();

        // Remove primary (slot 0), slot 1 should become new primary (slot 0)
        store.remove_slot("myapp.localhost", 0).await.unwrap();

        let slots = store.list_slots("myapp.localhost");
        assert_eq!(slots.len(), 1);
        assert_eq!(slots[0].slot, 0, "remaining slot should be promoted to slot 0");
        assert_eq!(slots[0].port, 4001);
    }

    #[tokio::test]
    async fn multi_slot_persists_and_reloads() {
        let temp = TempDir::new().unwrap();
        let path = temp.path().join("routes.json");

        {
            let store = StateStore::new(path.clone()).unwrap();
            let route = Route {
                hostname: "myapp.localhost".to_string(),
                port: 4000,
                public_port: None,
                protocol: RouteProtocol::Http,
                pid: std::process::id(),
                owner_pid: std::process::id(),
                cwd: "/tmp".to_string(),
                created_at: Utc::now(),
                slot: 0,
                label: Some("main".to_string()),
                tailscale_url: None,
                tailscale_https_port: None,
                tailscale_funnel: false,
            };
            let mut route2 = route.clone();
            route2.port = 4001;
            route2.label = Some("dev".to_string());

            store.insert(route).await.unwrap();
            store.insert(route2).await.unwrap();
        }

        let store = StateStore::new(path).unwrap();
        let slots = store.list_slots("myapp.localhost");
        assert_eq!(slots.len(), 2);
        assert_eq!(slots[0].label, Some("main".to_string()));
        assert_eq!(slots[1].label, Some("dev".to_string()));
    }

    #[tokio::test]
    async fn alias_route_survives_remove_stale() {
        let temp = TempDir::new().unwrap();
        let store = StateStore::new(temp.path().join("routes.json")).unwrap();

        store.insert(Route {
            hostname: "my-postgres.localhost".to_string(),
            port: 5432,
            public_port: None,
            protocol: RouteProtocol::Http,
            pid: 0,
            owner_pid: 0,
            cwd: String::new(),
            created_at: Utc::now(),
            slot: 0,
            label: None,
            tailscale_url: None,
            tailscale_https_port: None,
            tailscale_funnel: false,
        }).await.unwrap();

        store.insert(Route {
            hostname: "dead.localhost".to_string(),
            port: 4000,
            public_port: None,
            protocol: RouteProtocol::Http,
            pid: u32::MAX,
            owner_pid: u32::MAX,
            cwd: "/tmp".to_string(),
            created_at: Utc::now(),
            slot: 0,
            label: None,
            tailscale_url: None,
            tailscale_https_port: None,
            tailscale_funnel: false,
        }).await.unwrap();

        let removed = store.remove_stale().await.unwrap();

        assert_eq!(removed.len(), 1);
        assert_eq!(removed[0].hostname, "dead.localhost");
        assert!(store.get("my-postgres.localhost").is_some(), "alias should survive stale cleanup");
    }
}
