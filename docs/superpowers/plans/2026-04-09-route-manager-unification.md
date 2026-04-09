# RouteManager Unification Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Replace the dual-store pattern (`StateStore` + `TcpRouteManager` coordinated at 6+ call sites) with a single `RouteManager` wrapper that automatically handles TCP listener lifecycle on every route mutation.

**Architecture:** New file `src/route_manager.rs` wraps both existing types. `daemon/ipc.rs` and `daemon/mod.rs` switch from passing two stores to passing one `RouteManager`. `StateStore` and `TcpRouteManager` stay unchanged internally.

**Tech Stack:** Rust, tokio (async), existing crate types

---

## File Map

| File | Change |
|---|---|
| `src/route_manager.rs` | New — `RouteManager` struct + unified API + tests |
| `src/lib.rs` | Add `pub mod route_manager;` |
| `src/daemon/ipc.rs` | Replace `routes: StateStore` + `tcp_routes: TcpRouteManager` with `manager: RouteManager` |
| `src/daemon/mod.rs` | Construct `RouteManager`, replace startup stale/restore logic |

---

## Task 1: `RouteManager` — new `src/route_manager.rs`

**Files:**
- Create: `src/route_manager.rs`
- Modify: `src/lib.rs`

### Background

`RouteManager` wraps `StateStore` and `TcpRouteManager`. On `insert`, if the route is TCP, it starts a listener via `TcpRouteManager` before persisting. On `remove`, it tears down the listener before removing from the store. On `remove_stale`, it automatically tears down TCP listeners for any removed TCP routes.

Both `StateStore` and `TcpRouteManager` are `Clone` (Arc-backed), so `RouteManager` is also `Clone`.

- [ ] **Step 1: Add `pub mod route_manager;` to `src/lib.rs`**

Find the module declarations in `src/lib.rs` and add:
```rust
pub mod route_manager;
```

- [ ] **Step 2: Create `src/route_manager.rs` with stub + failing tests**

```rust
use crate::routes::{Route, RouteProtocol, StateStore};
use crate::tcp::TcpRouteManager;
use crate::error::Result;

#[derive(Clone)]
pub struct RouteManager {
    store: StateStore,
    tcp: TcpRouteManager,
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

    pub async fn insert(&self, _route: Route) -> Result<()> {
        todo!()
    }

    pub async fn remove(&self, _hostname: &str) -> Result<()> {
        todo!()
    }

    pub async fn remove_stale(&self) -> Result<Vec<Route>> {
        todo!()
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

        // Verify public port is now occupied by the TCP listener
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

        // Give tokio time to abort the listener task
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;

        // Verify public port is now free
        let probe = std::net::TcpListener::bind(("127.0.0.1", public_port));
        assert!(probe.is_ok(), "expected public port to be free after remove");
    }

    #[tokio::test]
    async fn remove_stale_tears_down_tcp_listeners() {
        let temp = TempDir::new().unwrap();
        let mgr = make_manager(&temp);

        let reserved = std::net::TcpListener::bind(("127.0.0.1", 0)).unwrap();
        let public_port = reserved.local_addr().unwrap().port();
        drop(reserved);

        // Use dead PID so remove_stale considers it stale
        let mut route = make_tcp_route("redis.localhost", 9999, public_port);
        route.pid = u32::MAX;
        route.owner_pid = u32::MAX;

        // Insert directly into both stores (simulating daemon startup)
        mgr.store.insert(route.clone()).await.unwrap();
        mgr.tcp.ensure_route(&route).await.unwrap();

        let removed = mgr.remove_stale().await.unwrap();
        assert_eq!(removed.len(), 1);
        assert_eq!(removed[0].hostname, "redis.localhost");

        // Listener should be torn down
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
```

- [ ] **Step 3: Run tests to verify they fail**

```bash
cargo test route_manager 2>&1 | grep -E "FAILED|error\[|^test result" | head -10
```

Expected: panics from `todo!()`.

- [ ] **Step 4: Implement all methods**

Replace the `todo!()` stubs with:

```rust
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
```

- [ ] **Step 5: Run tests**

```bash
cargo test route_manager 2>&1 | grep -E "^test result|FAILED"
```

Expected: all route_manager tests pass.

- [ ] **Step 6: Run full test suite**

```bash
cargo test 2>&1 | grep -E "^test result|FAILED"
```

Expected: all tests pass (new module is not yet wired into daemon).

- [ ] **Step 7: Commit**

```bash
git add src/route_manager.rs src/lib.rs
git commit -m "feat: add RouteManager wrapping StateStore + TcpRouteManager"
```

---

## Task 2: Wire `RouteManager` into `daemon/ipc.rs`

**Files:**
- Modify: `src/daemon/ipc.rs`

### Background

`IpcServer` currently holds `routes: StateStore` and `tcp_routes: TcpRouteManager` as separate fields. Replace both with `manager: RouteManager`. The `dispatch` function currently takes both stores — replace with `manager: RouteManager`. All 6 dual-store call patterns become single `manager.method()` calls.

`user_hostnames` currently takes `&StateStore` — change to `&RouteManager`.

- [ ] **Step 1: Replace `IpcServer` fields and constructor**

In `src/daemon/ipc.rs`, replace the struct and `new()`:

```rust
use crate::route_manager::RouteManager;
```

Remove these imports (no longer directly used):
```rust
// REMOVE: use crate::routes::{Route, RouteProtocol, StateStore};
// REMOVE: use crate::tcp::TcpRouteManager;
// KEEP Route and RouteProtocol for display functions:
use crate::routes::{Route, RouteProtocol};
```

Replace `IpcServer`:
```rust
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
```

- [ ] **Step 2: Update `serve()` — replace two clones with one**

In `serve()`, replace:
```rust
let routes = self.routes.clone();
let tcp_routes = self.tcp_routes.clone();
```
With:
```rust
let manager = self.manager.clone();
```

Update the `tokio::spawn` closure to pass `manager` instead of `routes` + `tcp_routes`:
```rust
let manager = manager.clone();
```

And the `handle_connection` call:
```rust
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
```

- [ ] **Step 3: Update `handle_connection` signature**

```rust
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
```

Update the `dispatch` call inside:
```rust
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
```

- [ ] **Step 4: Update `dispatch` — replace all dual-store patterns**

Signature:
```rust
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
```

Replace each command handler:

**`Command::Ls`** — replace stale cleanup + list:
```rust
Command::Ls => {
    let _ = manager.remove_stale().await;
    let list: Vec<_> = manager
        .list()
        .into_iter()
        .filter(|r| r.hostname != "_.localhost")
        .map(|route| route_response_value(&route, https_enabled, http_port, https_port))
        .collect();
    Response::ok(serde_json::Value::Array(list))
}
```

**`Command::Status`** — replace `routes.list()` with `manager.list()`:
```rust
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
```

**`Command::Stop`** — replace dual remove:
```rust
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
            let _ = manager.remove(&hostname).await;
            Response::ok_empty()
        }
    }
}
```

**`Command::Rm`** — replace dual remove:
```rust
Command::Rm { hostname } => {
    let _ = manager.remove(&hostname).await;
    Response::ok_empty()
}
```

**`Command::Shutdown`** — replace `tcp_routes.shutdown_all()`:
```rust
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
```

**`Command::RegisterRoute`** — replace dual insert + rollback:
```rust
Command::RegisterRoute {
    hostname,
    port,
    public_port,
    protocol,
    pid,
    cwd,
} => {
    let route = Route {
        hostname,
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
```

**`Command::HostsSync`** — replace `user_hostnames(&routes)`:
```rust
Command::HostsSync => {
    let hostnames = user_hostnames(&manager);
    // ... rest unchanged
}
```

- [ ] **Step 5: Update `user_hostnames` to take `&RouteManager`**

```rust
fn user_hostnames(manager: &RouteManager) -> Vec<String> {
    manager
        .list()
        .into_iter()
        .filter(|r| r.hostname != "_.localhost" && r.protocol == RouteProtocol::Http)
        .map(|r| r.hostname)
        .collect()
}
```

- [ ] **Step 6: Update tests in `daemon/ipc.rs`**

Update the `stale_tests` module to construct a `RouteManager` instead of separate stores:

```rust
#[cfg(test)]
mod stale_tests {
    use super::*;
    use chrono::Utc;
    use tempfile::TempDir;
    use crate::route_manager::RouteManager;
    use crate::routes::{StateStore, Route, RouteProtocol};
    use crate::tcp::TcpRouteManager;

    #[tokio::test]
    async fn ls_removes_stale_tcp_routes_and_releases_public_port() {
        let temp = TempDir::new().unwrap();
        let store = StateStore::new(temp.path().join("routes.json")).unwrap();
        let tcp = TcpRouteManager::default();
        let manager = RouteManager::new(store, tcp);

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
        // Insert via manager to start TCP listener
        manager.insert(route).await.unwrap();

        let response = dispatch(
            Command::Ls,
            manager.clone(),
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
        assert!(manager.get("redis.localhost").is_none());

        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        let rebound = std::net::TcpListener::bind(("127.0.0.1", public_port));
        assert!(rebound.is_ok());
    }
}
```

Update `user_hostnames_excludes_inspector` test similarly — use `RouteManager` wrapper:

```rust
#[tokio::test]
async fn user_hostnames_excludes_inspector() {
    let dir = tempfile::tempdir().unwrap();
    let store = crate::routes::StateStore::new(dir.path().join("routes.json")).unwrap();
    let tcp = crate::tcp::TcpRouteManager::default();
    let manager = crate::route_manager::RouteManager::new(store, tcp);

    manager
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

    manager
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
}
```

- [ ] **Step 7: Run all tests**

```bash
cargo test 2>&1 | grep -E "^test result|FAILED"
```

Expected: all tests pass.

- [ ] **Step 8: Commit**

```bash
git add src/daemon/ipc.rs
git commit -m "refactor(ipc): replace dual StateStore+TcpRouteManager with unified RouteManager"
```

---

## Task 3: Wire `RouteManager` into `daemon/mod.rs`

**Files:**
- Modify: `src/daemon/mod.rs`

### Background

`daemon/mod.rs` currently constructs `StateStore` and `TcpRouteManager` separately, does startup stale cleanup and TCP restoration with manual coordination, then passes both to `IpcServer::new()`. Replace with: construct `RouteManager`, call `manager.remove_stale()` (handles TCP automatically), restore TCP routes via `manager.insert()`, and pass `manager` to `IpcServer::new()`.

- [ ] **Step 1: Update imports in `daemon/mod.rs`**

Add:
```rust
use crate::route_manager::RouteManager;
```

Remove (if no longer used directly after the changes):
```rust
// Keep Route and RouteProtocol for inspector route insertion
use crate::routes::{Route, RouteProtocol, StateStore};
use crate::tcp::TcpRouteManager;
```

- [ ] **Step 2: Replace route store construction + startup cleanup**

Find the section (around lines 161-188) that does:
```rust
let routes = match StateStore::new(...) { ... };
let tcp_routes = TcpRouteManager::default();
let removed_stale = routes.remove_stale().await...;
// manual TCP cleanup loop
// manual TCP restore loop
```

Replace with:
```rust
let store = match StateStore::new(state_dir.join("routes.json")) {
    Ok(r) => r,
    Err(e) => {
        eprintln!("portal: failed to load route store: {e}");
        return Err(e);
    }
};
let manager = RouteManager::new(store, TcpRouteManager::default());

// Clean up stale routes (automatically tears down TCP listeners)
let _ = manager.remove_stale().await;

// Restore surviving TCP routes
for route in manager.list().iter().filter(|r| r.protocol == RouteProtocol::Tcp) {
    // Re-insert to start TCP listeners (ensure_route is idempotent)
    if let Err(e) = manager.insert(route.clone()).await {
        tracing::warn!("failed to restore TCP route {}: {e}", route.hostname);
        let _ = manager.remove(&route.hostname).await;
    }
}
```

- [ ] **Step 3: Update inspector route insertion**

Find where the inspector route is inserted (around line 201):
```rust
let _ = routes.insert(crate::routes::Route { ... }).await;
```

Replace `routes` with `manager`:
```rust
let _ = manager.insert(crate::routes::Route { ... }).await;
```

- [ ] **Step 4: Update `IpcServer::new()` call**

Find the `IpcServer::new(...)` call (around line 260). Replace the two-store arguments with `manager`:

```rust
let ipc = ipc::IpcServer::new(
    sock_path,
    pid_path,
    manager,
    mode,
    config.proxy.https,
    config.proxy.http_port,
    config.proxy.https_port,
);
```

- [ ] **Step 5: Run all tests**

```bash
cargo test 2>&1 | grep -E "^test result|FAILED"
```

Expected: all tests pass.

- [ ] **Step 6: Commit**

```bash
git add src/daemon/mod.rs
git commit -m "refactor(daemon): use RouteManager for startup and IPC server construction"
```

---

## Self-Review

**Spec coverage:**

- ✅ `RouteManager` wraps both stores — Task 1
- ✅ `insert()` with TCP auto-start + rollback — Task 1 implementation + `insert_tcp_route_starts_listener` test
- ✅ `remove()` with TCP auto-stop — Task 1 `remove_tcp_route_stops_listener` test
- ✅ `remove_stale()` with TCP teardown — Task 1 `remove_stale_tears_down_tcp_listeners` test
- ✅ `shutdown_all_tcp()` — Task 1 implementation
- ✅ `get/list` delegate to store — Task 1 `get_and_list_delegate_to_store` test
- ✅ IPC uses single `RouteManager` — Task 2 replaces all 6 dual-store patterns
- ✅ Daemon startup uses `RouteManager` — Task 3 replaces manual coordination
- ✅ Existing IPC tests updated — Task 2 Step 6

**No placeholders found.**

**Type consistency:** `RouteManager` constructor is `new(store, tcp)` consistently in Task 1, Task 2 tests, and Task 3 daemon setup. Method signatures match across all tasks.
