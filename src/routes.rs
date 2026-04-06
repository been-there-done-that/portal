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
    pub owner_pid: u32,
    pub cwd: String,
    pub created_at: DateTime<Utc>,
}

/// Thread-safe store backed by DashMap with atomic JSON persistence.
#[derive(Clone)]
pub struct RouteStore {
    map: Arc<DashMap<String, Route>>,
    path: PathBuf,
}

impl RouteStore {
    /// Create a new RouteStore. Loads from disk if file exists.
    pub fn new(path: PathBuf) -> Result<Self> {
        let map = Arc::new(DashMap::new());

        // Load from disk if file exists
        if path.exists() {
            let contents = std::fs::read_to_string(&path)?;
            if !contents.is_empty() {
                let routes: Vec<Route> = serde_json::from_str(&contents)?;
                for route in routes {
                    map.insert(route.hostname.clone(), route);
                }
            }
        }

        Ok(Self { map, path })
    }

    /// Insert a route and persist to disk.
    pub fn insert(&self, route: Route) -> Result<()> {
        self.map.insert(route.hostname.clone(), route);
        self.persist()?;
        Ok(())
    }

    /// Remove a route by hostname and persist to disk.
    pub fn remove(&self, hostname: &str) -> Result<()> {
        self.map.remove(hostname);
        self.persist()?;
        Ok(())
    }

    /// Get a route by hostname.
    pub fn get(&self, hostname: &str) -> Option<Route> {
        self.map.get(hostname).map(|entry| entry.clone())
    }

    /// List all routes.
    pub fn list(&self) -> Vec<Route> {
        self.map.iter().map(|entry| entry.value().clone()).collect()
    }

    /// Remove routes with dead PIDs.
    pub fn remove_stale(&self) -> Result<()> {
        let mut to_remove = Vec::new();

        for entry in self.map.iter() {
            let route = entry.value();
            if !pid_alive_check(route.pid) {
                to_remove.push(route.hostname.clone());
            }
        }

        let had_removals = !to_remove.is_empty();

        for hostname in to_remove {
            self.map.remove(&hostname);
        }

        if had_removals {
            self.persist()?;
        }

        Ok(())
    }

    /// Persist all routes to disk atomically.
    fn persist(&self) -> Result<()> {
        let routes: Vec<Route> = self.map.iter().map(|entry| entry.value().clone()).collect();

        let json = serde_json::to_string_pretty(&routes)?;
        let tmp_path = format!("{}.tmp", self.path.display());

        // Write to temporary file
        std::fs::write(&tmp_path, json)?;

        // Atomic rename
        std::fs::rename(&tmp_path, &self.path)?;

        Ok(())
    }
}

/// Check if a process with the given PID is alive.
pub fn pid_alive_check(pid: u32) -> bool {
    #[cfg(unix)]
    {
        use std::process::Command;

        // Use `kill -0` to check if process exists without sending a signal
        let output = Command::new("kill").arg("-0").arg(pid.to_string()).output();

        matches!(output, Ok(output) if output.status.success())
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

    #[test]
    fn insert_and_get() {
        let temp = TempDir::new().unwrap();
        let store_path = temp.path().join("routes.json");
        let store = RouteStore::new(store_path).unwrap();

        let route = Route {
            hostname: "myapp.localhost".to_string(),
            port: 4000,
            pid: std::process::id(),
            owner_pid: std::process::id(),
            cwd: "/tmp".to_string(),
            created_at: Utc::now(),
        };

        store.insert(route.clone()).unwrap();

        let retrieved = store.get("myapp.localhost").unwrap();
        assert_eq!(retrieved.port, 4000);
        assert_eq!(retrieved.hostname, "myapp.localhost");
    }

    #[test]
    fn persists_across_reload() {
        let temp = TempDir::new().unwrap();
        let store_path = temp.path().join("routes.json");

        {
            let store = RouteStore::new(store_path.clone()).unwrap();
            let route = Route {
                hostname: "myapp.localhost".to_string(),
                port: 4000,
                pid: std::process::id(),
                owner_pid: std::process::id(),
                cwd: "/tmp".to_string(),
                created_at: Utc::now(),
            };
            store.insert(route).unwrap();
        }

        // Create new store from same path
        let store = RouteStore::new(store_path).unwrap();
        let retrieved = store.get("myapp.localhost").unwrap();
        assert_eq!(retrieved.port, 4000);
    }

    #[test]
    fn remove_works() {
        let temp = TempDir::new().unwrap();
        let store_path = temp.path().join("routes.json");
        let store = RouteStore::new(store_path).unwrap();

        let route = Route {
            hostname: "myapp.localhost".to_string(),
            port: 4000,
            pid: std::process::id(),
            owner_pid: std::process::id(),
            cwd: "/tmp".to_string(),
            created_at: Utc::now(),
        };

        store.insert(route).unwrap();
        assert!(store.get("myapp.localhost").is_some());

        store.remove("myapp.localhost").unwrap();
        assert!(store.get("myapp.localhost").is_none());
    }

    #[test]
    fn stale_cleanup_removes_dead_pids() {
        let temp = TempDir::new().unwrap();
        let store_path = temp.path().join("routes.json");
        let store = RouteStore::new(store_path).unwrap();

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

        store.insert(route_alive).unwrap();
        store.insert(route_dead).unwrap();

        // Run cleanup
        store.remove_stale().unwrap();

        // Alive route should remain
        assert!(store.get("alive.localhost").is_some());
        // Dead route should be removed
        assert!(store.get("dead.localhost").is_none());
    }
}
