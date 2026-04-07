# Portal DX Improvements — Design Spec

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Make `portal run` work with zero manual setup — first run auto-installs the CA cert and starts the daemon, and re-running the same hostname replaces the existing instance by default (reusing the same backend port to avoid ENV churn).

**Architecture:** Two surgical changes to the run path: (1) before spawning the child, check cert trust + daemon health and fix both inline if needed; (2) remove the `--force` flag concept and replace with port-reuse replace-by-default semantics.

---

## Part 0: Command Reference

Full reference for every `portal` CLI command.

### `portal run [OPTIONS] <CMD...>`

Run a dev server and expose it at a `.localhost` URL.

| Flag | Default | Description |
|------|---------|-------------|
| `--hostname <NAME>` | inferred from `package.json` / cwd | Override the hostname prefix (e.g. `--hostname api` → `api.localhost`) |
| `--port <PORT>` | random from `port_range` | Pin the backend port; skips free-port scan |

**Hostname inference order:**
1. `--hostname` flag
2. `portal.toml` → `[project] name`
3. `package.json` → `"name"` field
4. cwd directory name

**In a git worktree** the branch name is prepended: `feature-login-myapp.localhost`.

**What it does:**
1. Ensures daemon is running (auto-starts if not).
2. Ensures CA cert is trusted (installs if not — first run only).
3. If the hostname already has a live route, stops the old process and reuses its port.
4. Detects framework (Vite, Astro, Angular, etc.) and injects `--port` / `--host` args automatically.
5. Spawns the child process.
6. Prints `  https://<hostname>  ->  port <port>`.
7. Registers the route in the daemon via `RegisterRoute` IPC.
8. Waits for the child; on exit the daemon's `remove_stale()` cleans up the route.

**Environment injected into child:**
- `PORT=<port>` — the backend port assigned
- `PORTAL_URL=https://<hostname>` — the public URL (useful for OAuth redirect URIs, etc.)

---

### `portal daemon`

Start the reverse-proxy daemon in the foreground. Normally invoked automatically by `portal run`; you only need this if you want to run on privileged ports (80/443) with `sudo portal daemon`.

The daemon listens on:
- `http_port` (default 80) for HTTP → HTTPS redirect
- `https_port` (default 443) for TLS-terminated reverse proxy
- `~/.portal/portal.sock` for IPC from the CLI

When run under `sudo`, the daemon resolves the socket path from `SUDO_USER`'s home (not root's), so non-root CLI invocations still connect to the same socket.

---

### `portal ls`

List all active routes. Auto-removes stale routes (where the backend process has exited).

**Output columns:** `HOSTNAME`, `PORT`, `PID`, `CWD`, `AGE`

---

### `portal stop [HOSTNAME]`

Stop the proxy for `HOSTNAME` and kill its associated process (SIGTERM). If no hostname given, stops all routes.

---

### `portal rm <HOSTNAME>`

Remove the route entry without killing the process. Useful when you killed the dev server yourself and want to clean up the route table.

---

### `portal status`

Show daemon health: uptime, number of active routes, listening ports, TLS status, CA cert path.

---

### `portal shutdown`

Gracefully shut down the daemon. All routes are removed.

---

### `portal cert install`

Install the local CA certificate into the system trust store. On macOS uses `security add-trusted-cert`. Requires `sudo` (or is auto-elevated).

---

### `portal cert reset`

Regenerate the CA keypair and reinstall. Existing per-hostname certs are invalidated. Run this if the CA cert is expired or compromised.

---

### `portal config`

Print the effective configuration (merged global + project TOML + env vars) as JSON. Useful for debugging why a setting is wrong.

---

## Part 1: Configuration Reference

### Config file locations (layered, last wins)

| Priority | Path | Scope |
|----------|------|-------|
| 1 (lowest) | built-in defaults | always |
| 2 | `~/.portal/config.toml` | all projects on this machine |
| 3 | `portal.toml` in project (walks up) | this project only |
| 4 (highest) | `PORTAL_*` env vars | current shell session |

### `portal.toml` / `~/.portal/config.toml` schema

```toml
[proxy]
tld = "localhost"            # TLD for all routes (default: "localhost")
port_range = [4000, 4999]   # Range for auto-assigned backend ports
https = true                 # Enable TLS termination (default: true)
http_port = 80               # Port for HTTP→HTTPS redirect listener
https_port = 443             # Port for HTTPS proxy listener

[daemon]
log_level = "info"           # "trace" | "debug" | "info" | "warn" | "error"
auto_start = true            # Auto-start daemon when portal run is called

[project]
name = "myapp"               # Override inferred hostname prefix
```

### Environment variables

| Variable | Equivalent TOML | Example |
|----------|----------------|---------|
| `PORTAL_TLD` | `proxy.tld` | `PORTAL_TLD=local` |
| `PORTAL_HTTPS` | `proxy.https` | `PORTAL_HTTPS=0` |
| `PORTAL_HTTP_PORT` | `proxy.http_port` | `PORTAL_HTTP_PORT=8080` |
| `PORTAL_HTTPS_PORT` | `proxy.https_port` | `PORTAL_HTTPS_PORT=8443` |
| `PORTAL_IS_DAEMON` | — | Set by daemon auto-start; prevents recursive spawn |

---

## Part 2: Known Issues from Portless (prior art)

During research we audited the TypeScript `portless` project's changelog. Several issues are relevant to portal's roadmap (not implemented in this spec, but tracked here):

| Issue | Description | Priority |
|-------|-------------|----------|
| **WebSocket memory leak** | Long-lived WS connections accumulate if the ping-pong heartbeat is not forwarded through the tunnel. Portal's current proxy uses hyper; need to verify upgrade path and add a connection map with idle cleanup. | High |
| **CA cert not in TLS chain** | Some browsers (especially Firefox) reject the cert if the CA is not included in the TLS handshake chain alongside the leaf cert. Portal uses rustls; need to verify `CertifiedKey` includes the CA. | High |
| **Browser-blocked ports** | Chrome and Firefox block connections to certain ports (6000, 6665–6669, etc.) even over HTTPS. Portal's `port_range` defaults to 4000–4999, which is safe. Should add a blocklist check in `find_free_port`. | Medium |
| **`PORTAL_URL` env var** | Child processes need to know their public URL for OAuth redirect URIs and health-check endpoints. Currently not injected. Part of this spec (Feature 1 implementation injects it). | High |
| **Loop detection** | If a child process also calls `portal run`, we get infinite nesting. Should detect via `PORTAL_IS_DAEMON` or a `PORTAL_DEPTH` env var. | Medium |
| **Per-hostname certs already done** | Portless TS used a single wildcard cert; portal already generates per-hostname certs. ✓ | Done |
| **Strict subdomain routing** | Requests to `*.hostname.localhost` currently 404 (e.g. `api.myapp.localhost` when route is `myapp.localhost`). Consider wildcard route matching. | Low |

---

## Part 3: Feature 1 — Transparent First-Run Setup

### Problem

Today, after installing `portal`, the user must:
1. `sudo portal cert install` — install CA into system trust
2. `sudo portal daemon` — start the daemon on ports 80/443

If either step is skipped, `portal run` gives cryptic errors. On first run the user sees a `NET::ERR_CERT_AUTHORITY_INVALID` browser error or a `DaemonNotRunning` error.

### Design

`portal run` gains a pre-flight check sequence that runs before spawning the child:

```
portal run npm run dev
  │
  ├─ 1. Is daemon alive?
  │     No → sudo elevation message → sudo portal daemon (detached)
  │         → wait up to 3s for socket
  │
  ├─ 2. Is CA trusted in system store?
  │     No → sudo elevation message → sudo portal cert install (inline)
  │
  └─ 3. (continues to port assignment + spawn)
```

**Transparency:** Each elevation step prints a clear message before running so users know exactly what is happening:

```
  portal: starting daemon (requires sudo for ports 80/443)...
  portal: trusting CA certificate (requires sudo)...
```

No silent elevation. No prompts or confirmation required — users can always see what happened via `portal status`.

### Implementation details

#### `is_ca_trusted() -> bool` (new, `src/certs.rs`)

On macOS: run `security find-certificate -c "portal" -p` and check for non-empty output.
On Linux: check if `/usr/local/share/ca-certificates/portal-ca.crt` exists.
Returns `false` if the command fails or the file is missing.

#### `ensure_daemon_running()` (modify, `src/cli/mod.rs`)

Current behavior: tries socket, then auto-spawns non-privileged daemon.

New behavior:
1. Try connecting to socket. If success → return Ok.
2. Detect if privileged ports are configured (`https_port < 1024 || http_port < 1024`).
   - If privileged: print `"  portal: starting daemon (requires sudo for ports 80/443)..."`, spawn `sudo portal daemon` detached, wait up to 3s polling socket.
   - If non-privileged: spawn `portal daemon` detached (existing behavior).
3. If socket still not connectable after timeout → `Err(DaemonNotRunning)`.

#### `ensure_cert_trusted()` (new, `src/cli/mod.rs`)

1. Call `is_ca_trusted()`. If trusted → return Ok.
2. Print `"  portal: trusting CA certificate (requires sudo)..."`.
3. Run `sudo portal cert install` (blocking, inherits stdout/stderr so user sees output).
4. Check exit code; if non-zero → return `Err(...)` with message.

#### Call site in `CliCommand::Run` handler

```
ensure_daemon_running().await?;
ensure_cert_trusted().await?;   // NEW — after daemon is up
```

#### `PORT` and `PORTAL_URL` env vars (modify, `src/process.rs`)

`spawn_child()` already sets `PORT`. Add `PORTAL_URL`:
```rust
cmd.env("PORT", port.to_string())
   .env("PORTAL_URL", format!("https://{hostname}"));
```

### Edge cases

- **Already trusted** — `is_ca_trusted()` returns true immediately; no sudo invocation.
- **User cancels sudo prompt** — `sudo portal cert install` exits non-zero; `portal run` prints error and exits. The user can retry or run `sudo portal cert install` manually.
- **Non-privileged ports** — daemon auto-starts without sudo (existing behavior); cert install still requires sudo on macOS because `security add-trusted-cert` always does.
- **CI environments** — `PORTAL_IS_DAEMON=1` check prevents recursive spawn; cert trust check should be skipped when `CI=1` is set (or handled gracefully if `security` is absent).

---

## Part 4: Feature 2 — Replace-by-Default with Port Reuse

### Problem

Today, running `portal run` for a hostname that is already live fails with:
```
error: myapp.localhost is already running on port 4123 (use --force to replace it)
```

This is the wrong default for a dev tool. You always want the new invocation to replace the old one. Additionally, `--force` picks a new random port, which means the `PORT` env var seen by the new process differs, causing frameworks that hard-code their port to rebind differently.

### Design

Remove the `--force` flag. Replace-by-default with port reuse:

1. Query daemon for existing route on this hostname.
2. If found:
   a. Record its `port` value.
   b. Send `Stop { hostname }` to daemon (kills old process, removes route).
   c. Wait for the backend port to be free (poll up to 2s).
   d. Use the **same port** for the new child.
3. If not found: proceed with free-port scan as normal.

This means `portal run` is always idempotent — running it twice just replaces the old instance cleanly.

### Why port reuse matters

- Framework dev servers read `PORT` from the environment at startup. If the port changes, the server starts on a different port, which can confuse HMR clients that have already connected.
- Certs are per-hostname, not per-port, so reusing the port has no cert implications.
- No code in the proxy needs to change — it always reads the route's `port` from the in-memory store.

### Implementation details

#### Remove `--force` from CLI (`src/cli/mod.rs`)

Delete the `#[arg(long)] force: bool` field from `CliCommand::Run`.
Delete the trailing-args `--force` stripping logic.
Delete the `--force` block in the match arm.

#### Replace-by-default logic in `CliCommand::Run`

```rust
// Determine port: reuse existing if hostname already registered
let port = match existing_route_port {
    Some(existing_port) => {
        // Stop old process
        let mut s = ipc_connect().await?;
        write_frame(&mut s, &Command::Stop { hostname: hostname.clone() }).await?;
        let _: Response = read_frame(&mut s).await?;
        eprintln!("  replaced existing instance (port {})", existing_port);

        // Wait for port to be free
        wait_for_port_free(existing_port, Duration::from_secs(2)).await;
        existing_port
    }
    None => {
        port.map(Ok).unwrap_or_else(|| {
            find_free_port(config.proxy.port_range.0, config.proxy.port_range.1)
        })?
    }
};
```

#### `wait_for_port_free(port, timeout)` (new, `src/ports.rs`)

Polls `TcpStream::connect("127.0.0.1:<port>")` every 100ms until it fails to connect (port free) or timeout elapses. Does not error on timeout — just proceeds with the port (the old process may still be shutting down, and the new one will bind once the old releases it).

#### `--port` flag interaction

If `--port` is given explicitly by the user, it overrides both free-port scan and reuse logic:

```rust
let port = if let Some(explicit_port) = port {
    // User pinned a specific port — stop old instance if present, use this port
    if let Some(_) = existing_route_port {
        // stop old...
    }
    explicit_port
} else {
    // replace-by-default or free-port scan (logic above)
};
```

### UX output

Before:
```
  stopped existing instance on port 4123  (only with --force)
  https://myapp.localhost  ->  port 4567  (new random port)
```

After (always):
```
  replaced existing instance on port 4123
  https://myapp.localhost  ->  port 4123  (same port reused)
```

First run (no existing instance):
```
  https://myapp.localhost  ->  port 4123
```

---

## Part 5: File-Level Change Map

| File | Change |
|------|--------|
| `src/cli/mod.rs` | `ensure_cert_trusted()` function; modify `ensure_daemon_running()` for sudo elevation; replace-by-default logic; remove `--force`; inject `PORTAL_URL` |
| `src/certs.rs` | `pub fn is_ca_trusted() -> bool` |
| `src/ports.rs` | `pub async fn wait_for_port_free(port: u16, timeout: Duration)` |
| `src/process.rs` | Add `PORTAL_URL` env var injection in `spawn_child()` |

No new files. No changes to IPC protocol, proxy, or daemon.

---

## Part 6: Out of Scope

- WebSocket memory leak fix (separate issue)
- CA chain fix in TLS handshake (separate issue)
- Browser-blocked port detection (separate issue)
- Wildcard subdomain routing (separate issue)
- Linux `systemd` unit for privileged daemon (separate issue)
