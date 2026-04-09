# Route Manager Unification Design

**Goal:** Replace the dual-store pattern (`StateStore` + `TcpRouteManager` coordinated at every call site) with a single `RouteManager` that wraps both and exposes a unified API.

**Architecture:** New file `src/route_manager.rs` with a `RouteManager` struct that owns both `StateStore` and `TcpRouteManager`. All IPC handlers call `RouteManager` methods instead of touching two stores. `StateStore` and `TcpRouteManager` remain unchanged internally — `RouteManager` is a thin coordinator, not a rewrite.

---

## Problem

Currently every route operation must touch two stores:

| Operation | StateStore | TcpRouteManager |
|---|---|---|
| Register | `routes.insert()` | `tcp_routes.ensure_route()` |
| Stop | `routes.remove()` | `tcp_routes.remove()` |
| Rm | `routes.remove()` | `tcp_routes.remove()` |
| Stale cleanup | `routes.remove_stale()` | `tcp_routes.remove()` per TCP route |
| Shutdown | — | `tcp_routes.shutdown_all()` |

This coordination is repeated in 6+ call sites in `daemon/ipc.rs` and `daemon/mod.rs`. Forgetting either side causes bugs (stale TCP listeners, orphaned routes in `routes.json`).

## Solution

### `RouteManager` struct

```rust
#[derive(Clone)]
pub struct RouteManager {
    store: StateStore,
    tcp: TcpRouteManager,
}
```

Both inner types are already `Clone` (Arc-backed). `RouteManager` is also `Clone` for the same reason.

### API

**`insert(route: Route) -> Result<()>`**
1. If route is TCP: call `tcp.ensure_route(&route)`. On failure, return error.
2. Call `store.insert(route)`. On failure, roll back TCP listener via `tcp.remove()`, return error.

**`remove(hostname: &str) -> Result<()>`**
1. Call `tcp.remove(hostname)` (no-op for HTTP routes — DashMap lookup misses).
2. Call `store.remove(hostname)`.

**`remove_stale() -> Result<Vec<Route>>`**
1. Call `store.remove_stale()` to get removed routes.
2. For each removed route with `protocol == Tcp`, call `tcp.remove()`.
3. Return removed routes (caller may need them to kill processes).

**`shutdown_all_tcp()`**
1. Call `tcp.shutdown_all()`.

**`get(hostname: &str) -> Option<Route>`** — delegate to `store.get()`.

**`list() -> Vec<Route>`** — delegate to `store.list()`.

### Rollback on insert failure

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
```

## Files Changed

| File | Change |
|---|---|
| `src/route_manager.rs` | New — `RouteManager` struct, unified API, unit tests |
| `src/routes.rs` | No change |
| `src/tcp.rs` | No change |
| `src/daemon/ipc.rs` | Replace `routes: StateStore` + `tcp_routes: TcpRouteManager` params with `manager: RouteManager`. Replace all dual-store call patterns with single `manager.insert/remove/remove_stale` calls |
| `src/daemon/mod.rs` | Construct `RouteManager::new(routes, tcp_routes)` and pass to IPC server. Replace startup stale cleanup + TCP restore with `manager.remove_stale()` + `manager.restore_tcp_routes()` |
| `src/lib.rs` | Add `pub mod route_manager;` |

## Testing

**`route_manager.rs` unit tests:**
- `insert_http_route_does_not_start_tcp_listener` — insert HTTP route, verify TCP manager has no handles
- `insert_tcp_route_starts_listener` — insert TCP route with public_port, verify TCP listener is reachable
- `remove_tcp_route_stops_listener` — insert then remove TCP route, verify public port is freed
- `remove_stale_tears_down_tcp_listeners` — insert TCP route with dead PID, call `remove_stale`, verify listener stopped
- `insert_rollback_on_store_failure` — if store.insert fails, TCP listener is cleaned up
- `get_and_list_delegate_to_store` — basic read delegation

**`daemon/ipc.rs` tests:**
- Existing tests updated to use `RouteManager` instead of separate stores
- No new IPC tests needed — the behavior is identical, just the plumbing changes
