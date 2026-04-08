use crate::error::Result;
use chrono::{DateTime, Utc};
use dashmap::DashMap;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use std::sync::Arc;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Route {
    pub hostname: String,
    pub port: u16,
    pub pid: u32,
    #[serde(default)]
    pub owner_pid: u32,
    #[serde(default)]
    pub cwd: String,
    #[serde(default = "chrono::Utc::now")]
    pub created_at: DateTime<Utc>,
}

/// Thread-safe store backed by DashMap.
/// Reads (get, list) are lock-free.
/// Writes (insert, remove, remove_stale) are serialised under a tokio Mutex
/// and atomically update routes.json + /etc/hosts in one locked transaction.
#[derive(Clone)]
pub struct StateStore {
    map: Arc<DashMap<String, Route>>,
    write_lock: Arc<tokio::sync::Mutex<()>>,
    path: PathBuf,
}

impl StateStore {
    /// Create a new StateStore. Loads existing routes from disk if file exists.
    pub fn new(path: PathBuf) -> Result<Self> {
        let map = Arc::new(DashMap::new());
        if path.exists() {
            let contents = std::fs::read_to_string(&path)?;
            if !contents.is_empty() {
                let routes: Vec<Route> = serde_json::from_str(&contents)?;
                for route in routes {
                    map.insert(route.hostname.clone(), route);
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
        self.map.get(hostname).map(|e| e.clone())
    }

    pub fn list(&self) -> Vec<Route> {
        self.map.iter().map(|e| e.value().clone()).collect()
    }

    // ── Write API (serialised) ────────────────────────────────────────────

    pub async fn insert(&self, route: Route) -> Result<()> {
        let _guard = self.write_lock.lock().await;
        self.map.insert(route.hostname.clone(), route);
        self.persist_locked()?;
        self.sync_hosts_locked();
        Ok(())
    }

    pub async fn remove(&self, hostname: &str) -> Result<()> {
        let _guard = self.write_lock.lock().await;
        self.map.remove(hostname);
        self.persist_locked()?;
        self.sync_hosts_locked();
        Ok(())
    }

    pub async fn remove_stale(&self) -> Result<()> {
        let _guard = self.write_lock.lock().await;
        let to_remove: Vec<String> = self.map.iter()
            .filter(|e| !pid_alive_check(e.value().pid))
            .map(|e| e.key().clone())
            .collect();
        if to_remove.is_empty() {
            return Ok(());
        }
        for h in &to_remove {
            self.map.remove(h);
        }
        self.persist_locked()?;
        self.sync_hosts_locked();
        Ok(())
    }

    // ── Private helpers (called while write_lock is held) ─────────────────

    fn persist_locked(&self) -> Result<()> {
        let routes: Vec<Route> = self.map.iter().map(|e| e.value().clone()).collect();
        let json = serde_json::to_string_pretty(&routes)?;
        let tmp_path = format!("{}.tmp", self.path.display());
        std::fs::write(&tmp_path, &json)?;
        std::fs::rename(&tmp_path, &self.path)?;

        #[cfg(unix)]
        if let Some((uid, gid)) = crate::config::sudo_uid_gid() {
            unsafe {
                let p = std::ffi::CString::new(self.path.to_string_lossy().as_bytes()).unwrap();
                nix::libc::chown(p.as_ptr(), uid, gid);
            }
        }
        Ok(())
    }

    fn sync_hosts_locked(&self) {
        if !crate::hosts::should_sync() {
            return;
        }
        let hostnames: Vec<String> = self.map.iter()
            .filter(|e| e.key() != "_.localhost")
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
            Ok(_) => true,                              // process exists and we can signal it
            Err(nix::errno::Errno::EPERM) => true,      // exists but owned by another user
            Err(_) => false,                            // ESRCH = no such process, or other error
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
            pid: std::process::id(),
            owner_pid: std::process::id(),
            cwd: "/tmp".to_string(),
            created_at: Utc::now(),
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
                pid: std::process::id(),
                owner_pid: std::process::id(),
                cwd: "/tmp".to_string(),
                created_at: Utc::now(),
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
            pid: std::process::id(),
            owner_pid: std::process::id(),
            cwd: "/tmp".to_string(),
            created_at: Utc::now(),
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
            pid: current_pid,
            owner_pid: current_pid,
            cwd: "/tmp".to_string(),
            created_at: Utc::now(),
        };

        // Insert route with invalid PID
        let route_dead = Route {
            hostname: "dead.localhost".to_string(),
            port: 4001,
            pid: u32::MAX, // Invalid PID, definitely dead
            owner_pid: u32::MAX,
            cwd: "/tmp".to_string(),
            created_at: Utc::now(),
        };

        store.insert(route_alive).await.unwrap();
        store.insert(route_dead).await.unwrap();

        // Run cleanup
        store.remove_stale().await.unwrap();

        // Alive route should remain
        assert!(store.get("alive.localhost").is_some());
        // Dead route should be removed
        assert!(store.get("dead.localhost").is_none());
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
                    pid: std::process::id(),
                    owner_pid: std::process::id(),
                    cwd: "/tmp".to_string(),
                    created_at: chrono::Utc::now(),
                }).await.unwrap();
            }));
        }
        for h in handles { h.await.unwrap(); }

        assert_eq!(store.list().len(), 20);

        let store2 = StateStore::new(temp.path().join("routes.json")).unwrap();
        assert_eq!(store2.list().len(), 20);
    }

}
