# Group A — Process & Runtime Improvements

**Goal:** Fix 7 real-world usability issues — env var injection, quiet mode, parallel write safety, and proxy compatibility — without changing the overall architecture.

**Architecture:** Small, targeted changes across `process.rs`, `config.rs`, `routes.rs`, `proxy.rs`, and `cli/mod.rs`. The biggest structural change is `RouteStore` → `StateStore`, which unifies route persistence and hosts-sync under a single write lock.

**Issues addressed:** #218, #59, #163, #182, #174, #43, #64

---

## Section 1 — Env Var Injection (#218 + #59)

### Problem

`spawn_child` hardcodes `PORT` and `PORTAL_URL`. Two real-world failures:

1. Node.js child processes don't trust the portal local CA — server-side `fetch()` to other portal-proxied services fails with `self-signed certificate in certificate chain` (#218)
2. Some frameworks use non-standard env var names (`APP_PORT`, `SERVER_PORT`) — the injected `PORT` is ignored (#59)

### Design

`spawn_child` signature gains a final `extra_env: &[(String, String)]` parameter. The function no longer hardcodes any env vars — callers provide the complete list.

```rust
pub async fn spawn_child(
    cwd: &Path,
    args: &[String],
    port: u16,
    hostname: &str,
    injection: crate::detect::PortInjection,
    extra_env: &[(String, String)],
) -> Result<tokio::process::Child>
```

`do_run` builds the env list before calling `spawn_child`:

```rust
let port_env = config.project.port_env.as_deref().unwrap_or("PORT");
let mut extra_env: Vec<(String, String)> = vec![
    (port_env.to_string(), port_str.clone()),
    ("PORTAL_URL".to_string(), format!("https://{hostname}")),
];
if config.proxy.https {
    let ca_path = crate::config::dirs_for_state().join("ca.pem");
    if ca_path.exists() {
        extra_env.push((
            "NODE_EXTRA_CA_CERTS".to_string(),
            ca_path.to_string_lossy().into_owned(),
        ));
    }
}
```

### `portal.toml` schema addition

```toml
[project]
port_env = "APP_PORT"   # optional, defaults to PORT
```

`host_env` is reserved in `ProjectConfig` for future use but not injected yet.

### Files changed

- `src/process.rs` — add `extra_env` param, remove hardcoded env vars, iterate `extra_env` with `.env(k, v)`
- `src/config.rs` — add `port_env: Option<String>` and `host_env: Option<String>` to `ProjectConfig` and `PartialProjectConfig`; wire through `apply_partial`
- `src/cli/mod.rs` — `do_run` builds `extra_env` as above; all three `spawn_child` call sites updated

---

## Section 2 — Quiet Mode (#163 + #182)

### Problem

- `portal run` / `portal start` always prints a startup banner and "Running: PORT=…" line — unwanted in CI, test harnesses, agent toolchains (#163)
- When a proxied service is unavailable, portal serves full HTML error pages. SSR fetch calls from dev servers log these as multi-hundred-line HTML dumps in the terminal (#182)

### Design

#### `--quiet` flag

Added to `CliCommand::Run` and `CliCommand::Start`. Propagated into `do_run` via a `quiet: bool` parameter.

When `quiet = true`:
- `banner::print_banner()` is skipped
- The "Running: PORT=…" line is skipped
- `stderr` errors are unaffected

#### Content-type-aware error responses

In `proxy.rs`, error response helpers (`page_502`, `page_404`, `page_508`) check the request's `Accept` header. Requests without `text/html` in `Accept` receive a short plain-text body:

```
502 Bad Gateway
myapp.localhost → port 4123 unreachable
```

Requests with `Accept: text/html` (browser navigations) continue to receive the existing styled HTML pages.

### Files changed

- `src/cli/mod.rs` — add `quiet: bool` to `CliCommand::Run` and `CliCommand::Start` variants; propagate into `do_run`; guard banner and "Running:" output behind `!quiet`
- `src/proxy.rs` — `handle_https_request` inspects `Accept` header before building error responses; plain-text path for non-browser requests

---

## Section 3 — `StateStore` (#174)

### Problem

`RouteStore::persist()` uses a fixed `routes.json.tmp` filename. Multiple concurrent tokio tasks in the daemon (one per IPC connection) can race:

- Task A: snapshot DashMap → write tmp → rename (routes.json now correct)
- Task B: snapshot DashMap *before* A's insert → write tmp → rename (overwrites with stale data)

With hosts-sync merged, a second persistent store (`/etc/hosts`) must also stay in sync with routes.json. A Mutex only around `persist()` would leave the two stores able to diverge under concurrent writes.

### Design

`RouteStore` is replaced by `StateStore`. It owns both persistent stores under one write lock.

```rust
pub struct StateStore {
    map: Arc<DashMap<String, Route>>,         // lock-free reads
    write_lock: Arc<tokio::sync::Mutex<()>>,  // serialises all writes
    path: PathBuf,
    // hosts-sync is via free functions in crate::hosts — no struct needed
    // crate::hosts::should_sync() gates whether /etc/hosts is updated
}
```

#### Read API — lock-free

```rust
pub fn get(&self, hostname: &str) -> Option<Route>
pub fn list(&self) -> Vec<Route>
```

These bypass the lock and read directly from DashMap.

#### Write API — async, serialised

```rust
pub async fn insert(&self, route: Route) -> Result<()>
pub async fn remove(&self, hostname: &str) -> Result<()>
pub async fn remove_stale(&self) -> Result<()>
```

Each write method:
1. Acquires `write_lock`
2. Mutates `map`
3. Writes `routes.json` atomically (tmp + rename)
4. Calls `crate::hosts::sync_hosts_file(&hostnames)` if `crate::hosts::should_sync()` is true

All three steps happen under the same lock. Concurrent reads are never blocked.

#### Construction

```rust
impl StateStore {
    pub fn new(path: PathBuf) -> Result<Self>
}
```

Same signature as current `RouteStore::new`. Hosts-sync behaviour is controlled by `crate::hosts::should_sync()` at runtime — no constructor parameter needed.

Loads existing routes from disk on construction (same as current `RouteStore::new`).

### Migration

`RouteStore` is deleted. All consumers updated:

| File | Change |
|---|---|
| `src/routes.rs` | `RouteStore` → `StateStore`; write methods become `async` |
| `src/daemon/ipc.rs` | `route_store: RouteStore` → `state: StateStore`; `insert`/`remove` become `.await`; local `sync_hosts()` helper deleted (absorbed into StateStore write methods) |
| `src/daemon/mod.rs` | Constructs `StateStore` instead of `RouteStore` |
| `src/proxy.rs` | Read-only usage — `get()` call unchanged |

### Files changed

- `src/routes.rs` — replace `RouteStore` struct and impl with `StateStore`
- `src/daemon/ipc.rs` — update field type and await write calls
- `src/daemon/mod.rs` — update construction
- `src/proxy.rs` — type name update only

---

## Section 4 — Proxy Compatibility (#43 + #64)

### Problem

**ngrok (#43):** ngrok rewrites the `Host` header to its own tunnel URL (`abc123.ngrok.io`). The portal proxy looks up routes by `Host`, so every request returns 404. ngrok passes the original hostname in `X-Forwarded-Host`.

**Bun HMR (#64):** WebSocket upgrade requests for Next.js HMR are proxied through portal. Bun's Next.js dev server rejects WebSocket connections where the `Host` header is `myapp.localhost` rather than `localhost:{port}` (what it's bound to). Fast refresh breaks silently.

### Design

#### ngrok fix — `X-Forwarded-Host` fallback

In `handle_https_request`, after extracting `hostname` from `Host`:

```rust
let hostname = {
    let from_host = extract_host_header(req.headers().get(http::header::HOST));
    if routes.get(&from_host).is_some() {
        from_host
    } else {
        req.headers()
            .get("x-forwarded-host")
            .and_then(|v| v.to_str().ok())
            .and_then(|v| v.split(':').next())
            .map(|s| s.to_string())
            .unwrap_or(from_host)
    }
};
```

#### Bun HMR fix — rewrite Host in WebSocket tunnel

In `handle_websocket`, before forwarding the upgrade request, rewrite the `Host` header to `localhost:{port}`:

```rust
// Replace Host with what the backend is actually bound to
headers.insert(
    http::header::HOST,
    format!("localhost:{port}").parse().unwrap(),
);
```

All other headers (including `Origin`, `Sec-WebSocket-Key`, `Upgrade`) are forwarded unchanged.

### Files changed

- `src/proxy.rs` — hostname resolution with `X-Forwarded-Host` fallback; Host rewrite in `handle_websocket`

---

## Testing Strategy

- **Section 1:** Unit tests in `process.rs` — verify `extra_env` is passed through; verify `NODE_EXTRA_CA_CERTS` is set when ca.pem exists; verify custom `port_env` name used. Config tests for new fields.
- **Section 2:** Unit tests for error response content-type branching. CLI unit tests verify banner suppressed under `--quiet`.
- **Section 3:** `StateStore` unit tests cover all existing `RouteStore` tests plus: concurrent insert/remove race (spawn N tasks, verify no data loss); hosts-sync called on insert and remove.
- **Section 4:** Unit tests in `proxy.rs` — `X-Forwarded-Host` lookup test; WebSocket Host header rewrite test.
