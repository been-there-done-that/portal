# Feature Parity Design — portal v0.2.0

**Date:** 2026-05-06  
**Branch:** `feature/parity-fixes`  
**Reference:** JS portless v0.12.0

---

## Goal

Close the feature gap between the Rust `portal` CLI (v0.1.0) and the reference JS `portless` package (v0.12.0). Work is grouped into three layers of effort.

---

## Group 1 — Easy CLI Wins

### `portal get <name>`

**Purpose:** Print the public URL for a running service — enables shell composition like `BACKEND=$(portal get api)`.

**Changes:**
- `src/cli/mod.rs`: Add `Get { name: String }` to `CliCommand`
- `src/proto.rs`: Add `GetUrl { hostname: String }` to `Command`
- `src/daemon/ipc.rs`: Handle `GetUrl` — look up route by hostname, return `{ url: "https://..." }` or error if not found
- CLI prints only the bare URL to stdout (no decoration)

**Edge cases:**
- Hostname not found → non-zero exit with `error: no route for "foo"`
- Daemon not running → propagate existing "daemon not running" error path

---

### `portal prune`

**Purpose:** Find and kill orphaned dev servers left behind by crashed CLI sessions.

**Changes:**
- `src/cli/mod.rs`: Add `Prune` to `CliCommand`
- `src/proto.rs`: Add `Prune` to `Command`
- `src/daemon/ipc.rs`: Handle `Prune` — iterate all routes, check `kill(pid, 0)`, SIGTERM orphans, remove their routes
  - Alias routes (`pid == 0`) are always exempted
  - Returns `{ pruned: ["foo.localhost", ...] }`
- CLI prints each pruned hostname or "nothing to prune"

---

### `portal clean`

**Purpose:** Full teardown — stop daemon, remove CA trust, clean hosts, delete state directory.

**Changes:**
- `src/cli/mod.rs`: Add `Clean { yes: bool }` to `CliCommand`
  - `--yes` / `-y` skips confirmation prompt
- Implementation (client-side only, no new IPC):
  1. If not `--yes`, prompt "This will stop the daemon and remove all portal state. Continue? [y/N]"
  2. Try IPC `Shutdown` (handles hosts cleanup + socket removal gracefully)
  3. If daemon not running, skip shutdown silently
  4. Untrust CA via platform command (`security remove-trusted-cert` on macOS, `update-ca-certificates --fresh` on Linux) — reuses the existing cert untrust logic, does not regenerate
  5. Remove `~/.portal/` entirely (`fs::remove_dir_all`)
  6. Print "portal state cleared"
- Custom `--cert`/`--key` paths from `portal.toml` are **never** removed
- Non-interactive (`CI=1` or no TTY) + no `--yes` → exit with error message

---

### `--force` on `portal run`

**Purpose:** Override a route held by another process without stopping it manually first.

**Changes:**
- `src/cli/mod.rs`: Add `force: bool` arg to `CliCommand::Run`
- `src/proto.rs`: Add `force: bool` field to `Command::Run` (already exists for `alias`, extend to `run`)
- `src/daemon/ipc.rs`: When `force=true` and route already exists — SIGTERM old process group, wait up to 3s, SIGKILL if needed, then proceed with registration

---

### Non-server script detection

**Purpose:** Build-only tools (tsup, tsc, esbuild, etc.) should run directly without registering a proxy route.

**Changes:**
- `src/process.rs`: Add `pub fn is_build_only(args: &[String]) -> bool`
  - Known build-only basenames: `tsc`, `tsup`, `esbuild`, `rollup`, `webpack`, `parcel`, `vite build`, `next build`, `bun build`, `turbo build`, `nuxt build`, `astro build`, `svelte-kit build`, `rspack build`, `rsbuild build`
  - Detection: match basename of `args[0]`, OR match `args[0..=1]` as `<tool> build`
- `src/cli/mod.rs` (`run`/`start` handlers): call `is_build_only` before route registration; if true, skip `RegisterRoute` + proxy setup, just spawn and stream output
- `src/config.rs`: Add `proxy: Option<bool>` to `PartialProjectConfig` / `ProjectConfig`
  - `proxy = false` in `[project]` of `portal.toml` forces build-only mode regardless of command
- Print command without the `→ https://...` URL line when in build-only mode

---

## Group 2 — Variable Rename + Wildcard Routing

### `PORTAL_URL` → `PORTLESS_URL`

**Purpose:** Align env var name with the JS reference implementation.

**Changes:**
- `src/process.rs`: Rename `"PORTAL_URL"` → `"PORTLESS_URL"` in env injection
- `src/cli/mod.rs`: Any remaining `PORTAL_URL` references
- `tests/`: Update test assertions
- Keep `PORTAL_TLD`, `PORTAL_SYNC_HOSTS`, `PORTAL_IS_DAEMON` as-is (portal-specific internals, not user-facing)

---

### Wildcard subdomain routing

**Purpose:** Allow `tenant.myapp.localhost` to route to `myapp.localhost` — useful for multi-tenant apps.

**Changes:**
- `src/config.rs`: Add `wildcard: bool` to `ProxyConfig`, default `false`
  - Env var `PORTLESS_WILDCARD=1` enables
  - `portal.toml` `[proxy] wildcard = true`
- `src/daemon/ipc.rs` config loading: read `wildcard` from config at daemon start, store on shared state
- `src/proxy.rs` route lookup: when `wildcard=true` and no exact match found, strip first DNS label and retry
  - e.g., `tenant.myapp.localhost` → `myapp.localhost`
  - Exact matches always take priority over wildcard matches
  - Only one level of stripping (not recursive)
- No new IPC commands needed — proxy reads wildcard flag from config on startup

---

## Group 3 — Monorepo, Turborepo, LAN

### Monorepo multi-app orchestration

**Purpose:** Running bare `portal start` in a monorepo root starts all workspace packages concurrently.

**New file:** `src/workspace.rs`

**Workspace discovery:**
```
WorkspacePackage {
    dir: PathBuf,
    name: String,       // inferred from package.json / Cargo.toml / etc.
    command: Vec<String>,
    injection: PortInjection,
}
```

Discovery logic (in priority order):
1. `pnpm-workspace.yaml` → parse `packages:` glob list
2. `package.json` `"workspaces"` field (npm/yarn/bun) → glob list
3. If neither found → single-app mode (current behavior)

For each discovered package dir, run `DriverRegistry::detect_language()` + `start_command()`. Packages with no dev command are skipped silently.

**`portal start` changes (`src/cli/mod.rs`):**
- After single-app detection fails or when workspace root is found, call `discover_workspace_packages(cwd)`
- If multiple packages found: spawn all concurrently via `tokio::spawn`, prefix each log line with `[name] `
- Each app registers its own route via existing IPC `RegisterRoute`
- `Ctrl-C` → signal all process groups

---

### Turborepo integration

**Purpose:** When `turbo.json` is present in the workspace root, delegate to `turbo run <script> --filter=<pkg>` per app instead of running the package manager directly.

**Changes (within `src/workspace.rs` / monorepo path):**
- After workspace packages are discovered, check for `turbo.json` in workspace root
- If present and `turbo = true` (default): replace each app's `command` with `turbo run dev --filter=<pkg-name>`
- Per-app `PORT`, `HOST`, `PORTLESS_URL` are injected as env vars (turbo forwards env to tasks)
- Config opt-out: `turbo = false` in `[project]` of root `portal.toml`

---

### LAN mode

**Purpose:** Expose local apps to other devices on the same network via mDNS `.local` hostnames.

**Changes:**
- `src/cli/mod.rs`: Add `--lan` flag to `CliCommand::Run` and `CliCommand::Daemon`
- `src/config.rs`: Add `lan: bool` and `lan_ip: Option<String>` to `ProxyConfig`
  - Env vars: `PORTLESS_LAN=1`, `PORTLESS_LAN_IP=<ip>`
- New file `src/lan.rs`:
  - `detect_lan_ip() -> Option<IpAddr>` — iterates network interfaces, returns first non-loopback IPv4
  - `publish_mdns(hostname: &str, ip: IpAddr, port: u16)` — platform dispatch:
    - macOS: `dns-sd -R <name> _http._tcp local <port> &`
    - Linux: `avahi-publish-address <hostname>.local <ip> &`
  - `unpublish_mdns(hostname: &str)` — kill dns-sd/avahi-publish processes by PID
- `src/certs.rs`: When `lan=true`, add LAN IP as additional SAN in cert generation
- `src/daemon/mod.rs`: When `lan=true`, store LAN IP in shared state, publish mDNS on each route registration
- `src/cli/mod.rs` output: when `--lan` active, print `  LAN: https://192.168.1.42` alongside `.localhost` URL
- `--ip <ip>` override for VPN / multi-interface setups

---

## Affected Files Summary

| File | Changes |
|------|---------|
| `src/proto.rs` | Add `GetUrl`, `Prune`; add `force` to `Run` |
| `src/cli/mod.rs` | Add `Get`, `Prune`, `Clean`, `--force` on `Run`, `--lan` on `Run`+`Daemon` |
| `src/daemon/ipc.rs` | Handle `GetUrl`, `Prune`, `force` in `Run` |
| `src/config.rs` | Add `wildcard`, `lan`, `lan_ip` to `ProxyConfig`; add `proxy` to `ProjectConfig` |
| `src/proxy.rs` | Wildcard subdomain fallback lookup |
| `src/process.rs` | Add `is_build_only()`; rename `PORTAL_URL` → `PORTLESS_URL` |
| `src/daemon/mod.rs` | LAN IP plumbing on daemon start |
| `src/workspace.rs` | New file — workspace discovery, turbo detection |
| `src/lan.rs` | New file — LAN IP detection, mDNS publish/unpublish |

---

## Out of Scope (this PR)

- Windows support
- Tailscale / Tailscale Funnel
- `portless.json` / `package.json "portless"` key config format (portal uses `.toml`)
- `PORTLESS_TAILSCALE_URL` env var
