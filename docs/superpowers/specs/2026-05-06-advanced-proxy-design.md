# Advanced Proxy Features Design — portal v0.3.0

**Date:** 2026-05-06
**Branch:** `feature/advanced-proxy`
**Base branch:** `feature/parity-fixes`
**Reference:** JS portless v0.12.0 + open PRs #264, #278

---

## Goal

Add three advanced proxy capabilities to bring portal to parity with portless v0.12.0's latest features and active development:

1. **HTTP/2 + RFC 8441** — serve HTTP/2 to browsers, fix WebSocket-over-HTTP/2 (Turbopack/Vite HMR), add `--h2c` for gRPC upstreams
2. **Tailscale integration** — `--tailscale` (tailnet) and `--funnel` (public internet) sharing
3. **Multiplexed hostname routing** — multiple apps on one hostname with a floating in-browser app-switcher UI

---

## Architecture

**New files:**
- `src/tailscale.rs` — Tailscale CLI wrapper
- `src/switcher.rs` — App-switcher HTML injection

**Modified files:**

| File | What changes |
|------|-------------|
| `src/routes.rs` | Add slot/label/tailscale fields to `Route`; change `StateStore` map to `DashMap<String, Vec<Route>>` |
| `src/proxy.rs` | HTTP/2 auto-negotiation, RFC 8441 CONNECT handler, slot-aware route lookup, HTML injection |
| `src/daemon/mod.rs` | Switch `serve_https` to `hyper_util::server::conn::auto::Builder` |
| `src/daemon/ipc.rs` | `RegisterRoute` extended with `slot`, `label`, `tailscale`, `funnel`; new `UpdateRoute` command |
| `src/proto.rs` | `RegisterRoute` + new `UpdateRoute` variant |
| `src/cli/mod.rs` | `--tailscale`, `--funnel`, `--h2c`, `--slot`, `--label` flags; Tailscale URL in banner |
| `src/config.rs` | Add `h2c: bool` to `ProxyConfig` |

**Implementation order (4 commit groups, all on one branch):**
1. Route struct + StateStore changes (foundation for groups 2–4)
2. HTTP/2 + RFC 8441
3. Tailscale integration
4. Multiplexed routing + app-switcher

---

## Group 1 — Route Struct & StateStore

### Route struct additions

All new fields use `#[serde(default)]` for backward compatibility with existing `routes.json`:

```rust
pub struct Route {
    // ... existing fields unchanged ...

    // Multiplexed routing
    #[serde(default)]
    pub slot: u32,                   // 0 = primary; auto-incremented per hostname
    #[serde(default)]
    pub label: Option<String>,       // user label shown in switcher UI

    // Tailscale
    #[serde(default)]
    pub tailscale_url: Option<String>,      // e.g. "https://mynode.ts.net:443"
    #[serde(default)]
    pub tailscale_https_port: Option<u16>,  // port registered with tailscale serve
    #[serde(default)]
    pub tailscale_funnel: bool,             // true = public internet via Funnel
}
```

### StateStore map change

Change from `DashMap<String, Route>` to `DashMap<String, Vec<Route>>` where the `Vec` is ordered by `slot`:

**New/changed methods:**
- `get(hostname) -> Option<Route>` — returns `vec[0]` (primary); callers unchanged
- `get_slot(hostname, slot) -> Option<Route>` — returns specific slot
- `list_slots(hostname) -> Vec<Route>` — all slots for a hostname
- `insert(route)` — if hostname already exists and `route.slot == 0`, auto-assigns `next_slot = max(existing slots) + 1`; inserts at the correct position
- `remove(hostname)` — removes all slots (existing behaviour)
- `remove_slot(hostname, slot)` — removes one slot; if it was the primary (slot 0), promotes slot 1 to slot 0

**Backward compat:** `routes.json` stays as a flat `Vec<Route>` array. Existing routes load with `slot: 0` (default), so they become single-slot primaries automatically. No migration needed.

**Edge cases:**
- `remove_stale()` removes dead routes per-slot, not per-hostname
- `insert` on a non-zero slot that already exists: overwrite (replace)
- `list()` (used by prune, list commands) returns all routes across all hostnames and slots flattened

---

## Group 2 — HTTP/2 + RFC 8441

### Frontend: HTTP/2 auto-negotiation

**Change in `src/daemon/mod.rs`:** Replace `hyper::server::conn::http1::Builder` in `serve_https` with `hyper_util::server::conn::auto::Builder::new(TokioExecutor::new())`. The `server-auto` feature is already present in `Cargo.toml`. This gives:
- HTTP/1.1 connections: handled exactly as before
- HTTP/2 connections: ALPN negotiated by rustls (`h2` in ALPN list), hyper-util handles the H2 framing

No new dependencies required.

### RFC 8441 — WebSocket over HTTP/2

When a browser connects over HTTP/2 and opens a WebSocket, it sends `CONNECT` with `:protocol: websocket` (RFC 8441 Extended CONNECT) instead of `Upgrade: websocket`.

**Detection in `src/proxy.rs`:**

```rust
fn is_h2_websocket_connect(req: &Request<Incoming>) -> bool {
    req.method() == Method::CONNECT
        && req.headers()
            .get("protocol")
            .and_then(|v| v.to_str().ok())
            .map(|v| v.eq_ignore_ascii_case("websocket"))
            .unwrap_or(false)
}
```

**Handling:** Call the existing `handle_websocket_upgrade()` path — the upgrade mechanics are identical. The `auto::Builder` must be called with `.serve_connection_with_upgrades()` so `hyper::upgrade::on(req)` works for both H1 and H2 connections.

**Advertise capability:** HTTP/2 servers must send `SETTINGS_ENABLE_CONNECT_PROTOCOL = 1` to allow Extended CONNECT. `hyper_util`'s auto builder does this automatically when using H2.

### `--h2c` upstream (gRPC / HTTP/2 cleartext backends)

**Config:** `h2c: bool` in `ProxyConfig` (default `false`). Env var `PORTLESS_H2C=1`. CLI `--h2c` flag on `portal run`.

**Proxy change in `src/proxy.rs`:** When `h2c = true`, use `hyper_util::client::legacy::Client` with an H2C connector (`hyper::client::conn::http2`) to the upstream instead of the current HTTP/1.1 TCP proxy path. Hop-by-hop headers are stripped before forwarding (already done for normal requests).

**Scope:** `--h2c` applies per-run (the spawned app speaks H2C). The frontend TLS connection remains normal HTTPS regardless.

---

## Group 3 — Tailscale Integration

### New file `src/tailscale.rs`

Five public functions, all using `std::process::Command`:

| Function | CLI command | Returns |
|----------|------------|---------|
| `is_installed() -> bool` | `tailscale version` | `true` if exit 0 |
| `get_node_name() -> Result<String>` | `tailscale status --json` | `Self.DNSName` trimmed of trailing `.` |
| `used_ports() -> Vec<u16>` | `tailscale serve status --json` | ports from `TCP` map keys |
| `register(local_port: u16, funnel: bool) -> Result<(u16, String)>` | `tailscale serve/funnel --bg --yes --https=<port> http://127.0.0.1:<local_port>` | `(https_port, public_url)` |
| `unregister(https_port: u16, funnel: bool) -> Result<()>` | `tailscale serve/funnel --yes --https=<port> off` | — |

**Port selection:**
- Serve (tailnet): try ports `[443, 8443, 8444, 8445, 8446, 8447, 8448, 8449, 8450]`
- Funnel (public): try ports `[443, 8443, 10000]` only (Tailscale restriction)
- Skip any port in `used_ports()`; return `Err` if all exhausted

**URL construction:**
- Tailnet: `https://<node-name>:<https_port>` (omit port if 443)
- Funnel: `https://<node-name>` (Funnel always serves on 443 externally)

**Error types:**
- `TailscaleNotInstalled` — CLI not found
- `TailscaleFunnelDisabled` — exit code with "Funnel not available" in stderr; print actionable message: `run: tailscale funnel on`
- `TailscalePortConflict` — all preferred ports in use

### CLI changes (`src/cli/mod.rs`)

Add to `CliCommand::Run`:
```rust
#[arg(long)]
tailscale: bool,   // share on tailnet
#[arg(long)]
funnel: bool,      // share publicly via Tailscale Funnel (implies --tailscale)
```

**`do_run` flow when `tailscale` or `funnel` is true:**
1. Preflight: `tailscale::is_installed()` → exit 1 with error if false
2. After route registered with daemon: call `tailscale::register(port, funnel)`
3. Store result via `Command::UpdateRoute { hostname, tailscale_url, tailscale_https_port, tailscale_funnel }` IPC
4. Inject `PORTLESS_TAILSCALE_URL=<url>` into child process env
5. Print `  Tailscale: <url>` line in banner
6. On `Ctrl-C` / child exit: call `tailscale::unregister(https_port, funnel)` in cleanup

### New IPC command `UpdateRoute`

```rust
// src/proto.rs
UpdateRoute {
    hostname: String,
    tailscale_url: Option<String>,
    tailscale_https_port: Option<u16>,
    tailscale_funnel: Option<bool>,
}
```

Handler in `src/daemon/ipc.rs`: find route by hostname (slot 0), patch the tailscale fields, persist to `routes.json`.

### `RegisterRoute` IPC extension

Add to existing `Command::RegisterRoute`:
```rust
#[serde(default)]
slot: Option<u32>,   // None = auto-assign
#[serde(default)]
label: Option<String>,
```

---

## Group 4 — Multiplexed Routing + App Switcher

### Slot-aware route lookup in `src/proxy.rs`

Replace the single-route lookup with:

```rust
let slots = routes.list_slots(&hostname);
let route = match slots.len() {
    0 => { /* 404 path */ }
    1 => slots.into_iter().next().unwrap(),
    _ => {
        let preferred_slot = read_slot_cookie(&req, &hostname);
        let primary = slots[0].clone();
        slots.into_iter()
            .find(|r| r.slot == preferred_slot)
            .unwrap_or(primary)
    }
};
```

**Cookie format:** `portless-slot-<hostname>=<slot_number>` (e.g. `portless-slot-myapp.localhost=1`). Cookie name uses the hostname with dots replaced by `-` for broader compatibility: `portless-slot-myapp-localhost`.

### HTML injection — `src/switcher.rs`

**Trigger condition:** Response `content-type` starts with `text/html` AND `routes.list_slots(&hostname).len() > 1`.

**Implementation:** Body is buffered (up to 4 MB; larger responses skip injection), `</body>` located (case-insensitive, last occurrence), switcher HTML inserted immediately before it. If no `</body>`, append to end.

**Switcher HTML** (self-contained, ~45 lines, inline CSS + JS):

```html
<div id="__portless_switcher__" style="position:fixed;bottom:16px;right:16px;z-index:99999;
  background:#1a1a1a;color:#fff;border-radius:8px;padding:8px 12px;font:13px/1.4 monospace;
  box-shadow:0 4px 12px rgba(0,0,0,.4);display:flex;gap:8px;align-items:center">
  <span style="opacity:.5;margin-right:4px">portal</span>
  <!-- one button per slot, injected at response time -->
  <button data-portless-slot="0" style="background:#333;border:none;color:#fff;padding:3px 8px;border-radius:4px;cursor:pointer">api</button>
  <button data-portless-slot="1" style="background:#555;border:none;color:#fff;padding:3px 8px;border-radius:4px;cursor:pointer">api (slot-1)</button>
</div>
<script>
  (function(){
    var h = location.hostname;
    var key = 'portless-slot-' + h.replace(/\./g,'-');
    document.querySelectorAll('[data-portless-slot]').forEach(function(btn){
      btn.onclick = function(){
        document.cookie = key + '=' + btn.dataset.portlessSlot + ';path=/;max-age=86400';
        location.reload();
      };
    });
  })();
</script>
```

The slot buttons and their labels are generated when building the injection string — they are not static HTML.

**Buffer limit:** Responses over 4 MB skip injection silently (avoids memory issues with large HTML pages). API responses (`application/json`, etc.) are never injected regardless of size.

### `portal run` new flags

```
--slot <N>     register as slot N (default: auto-assign next available)
--label <str>  label shown in switcher UI (default: slot-N)
```

---

## Affected Files Summary

| File | Group | Changes |
|------|-------|---------|
| `src/routes.rs` | 1 | Route fields; StateStore `DashMap<String, Vec<Route>>` |
| `src/proxy.rs` | 2, 4 | HTTP/2 auto; RFC 8441; slot-aware lookup; HTML injection |
| `src/daemon/mod.rs` | 2 | `auto::Builder`; `TokioExecutor` |
| `src/proto.rs` | 1, 3 | `RegisterRoute` + `UpdateRoute` variants |
| `src/daemon/ipc.rs` | 1, 3 | `RegisterRoute` slot/label; `UpdateRoute` handler |
| `src/cli/mod.rs` | 3, 4 | `--tailscale`, `--funnel`, `--h2c`, `--slot`, `--label` |
| `src/config.rs` | 2 | `h2c: bool` in `ProxyConfig` |
| `src/tailscale.rs` | 3 | New file — Tailscale CLI wrapper |
| `src/switcher.rs` | 4 | New file — HTML injection |

---

## Out of Scope (this PR)

- Windows support for Tailscale
- Tailscale auth / login flow (assumes `tailscale` is already authenticated)
- Slot persistence across daemon restarts for Tailscale URLs (routes.json already persists this)
- Switcher UI theming / customisation options
- H2C upstream for TCP routes (only HTTP routes)
- RFC 8441 for routes that don't already support WebSocket upgrades
