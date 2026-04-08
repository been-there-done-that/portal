# Hosts File Sync — Design Spec

**Date:** 2026-04-08
**Branch:** feature/hosts-sync (to be created)
**Status:** Approved

---

## Problem

The Rust daemon accepts a configurable `tld` (default: `localhost`) and builds hostnames like `myapp.localhost` or `myapp.local`. However, it never writes these hostnames to `/etc/hosts`, so:

- Custom TLDs (anything other than `localhost`) do not resolve at all.
- `.localhost` subdomains fail in Safari, which relies on the OS resolver rather than browser-native loopback handling.
- Tools like `curl`, system DNS lookups, and non-Chromium/Firefox browsers all break silently.

The reference JS repo (`portless/packages/portless/src/hosts.ts`) solves this with an auto-managed block in `/etc/hosts`. The Rust daemon is missing this entirely.

---

## Goals

- Automatically keep `/etc/hosts` in sync with the daemon's route table.
- Always sync regardless of TLD (covers Safari + custom TLDs + curl etc.).
- Mirror the JS implementation's structure and marker format for compatibility.
- Add `portless hosts sync` and `portless hosts clean` subcommands routed through the daemon.
- Opt-out via `PORTAL_SYNC_HOSTS=0`.

---

## Architecture

### Approach chosen: Option B — Sync in IPC dispatch

A standalone `src/hosts.rs` module provides pure sync functions. The IPC dispatch layer calls them after each mutating command. `RouteStore` stays a pure data layer with no filesystem side-effects beyond its own JSON persistence.

---

## Module: `src/hosts.rs`

Pure functions, no daemon state, fully unit-testable.

### Constants

```
MARKER_START = "# portless-start"
MARKER_END   = "# portless-end"
```

Same markers as the JS repo — `/etc/hosts` managed blocks stay compatible if both tools are used on the same machine.

### Functions

| Function | Signature | Description |
|---|---|---|
| `hosts_path` | `() -> PathBuf` | `/etc/hosts` on Unix; `C:\Windows\System32\drivers\etc\hosts` on Windows |
| `read_hosts` | `() -> String` | Reads file; returns empty string on failure |
| `extract_managed` | `(content: &str) -> Vec<String>` | Lines between markers, exclusive |
| `remove_block` | `(content: &str) -> String` | Strips managed block, normalises excess blank lines |
| `build_block` | `(hostnames: &[&str]) -> String` | Builds `MARKER_START\n127.0.0.1 <host>\n...\nMARKER_END` |
| `sync_hosts_file` | `(hostnames: &[&str]) -> Result<()>` | Atomic rewrite: remove_block + append new block |
| `clean_hosts_file` | `() -> Result<()>` | remove_block only, no new block |
| `should_sync` | `() -> bool` | Returns false only if `PORTAL_SYNC_HOSTS=0` or `false` |

### OS-level quirks handled

1. **Path** — `cfg!(windows)` / `#[cfg(unix)]` for correct path per platform.
2. **CRLF on Windows** — `trim()` each line when parsing to handle CRLF cleanly.
3. **Atomic write** — write to `<hosts_path>.tmp` then `std::fs::rename`. Avoids partial writes on crash.
4. **Permission preservation** — read existing file mode via `std::fs::metadata` before writing; restore with `std::fs::set_permissions` after rename.
5. **macOS DNS cache flush** — after a successful write, fire-and-forget: `dscacheutil -flushcache && killall -HUP mDNSResponder`. Compiled in only on `#[cfg(target_os = "macos")]`. Failures are `tracing::warn!` only, never propagated.
6. **Linux `systemd-resolved`** — reads `/etc/hosts` via `nsswitch.conf` directly; no flush needed.

---

## IPC dispatch integration (`src/daemon/ipc.rs`)

After each mutating command, call `sync_hosts_file` with the current user route hostnames (all routes except `_.localhost`).

| Command | Trigger |
|---|---|
| `RegisterRoute` | After `routes.insert` succeeds |
| `Rm` | After `routes.remove` |
| `Stop` | After kill + `routes.remove` |
| `Ls` (calls `remove_stale`) | Always sync after `remove_stale` — operation is idempotent so no need to track whether anything changed |
| `Shutdown` | `clean_hosts_file()` before spawning the exit task |

`should_sync()` is checked at the start of each handler. If `PORTAL_SYNC_HOSTS=0`, skip silently.

Failures to write `/etc/hosts` are logged as `tracing::warn!` and **never** propagated as IPC errors — the proxy continues to function. Hosts sync is best-effort.

---

## New IPC commands (`src/proto.rs`)

Two new variants added to the `Command` enum:

```rust
Command::HostsSync
Command::HostsClean
```

Handled in `ipc.rs dispatch`:

| Command | Daemon action | Response |
|---|---|---|
| `HostsSync` | `sync_hosts_file(user_hostnames)` | Returns list of written entries, or empty if no routes |
| `HostsClean` | `clean_hosts_file()` | Returns ok |

Unlike background sync, these **do** propagate errors back to the CLI — the user explicitly asked for the operation.

---

## CLI subcommands (`src/cli/mod.rs`)

```
portless hosts sync    # force-rewrite managed block from current routes
portless hosts clean   # remove managed block entirely
```

Both subcommands:
- Send IPC commands over the Unix socket (same as `Ls`, `Stop`, etc.)
- Require the daemon to be running — fail with the standard `daemon not running` error if not
- No sudo handling in the CLI; the daemon already has root

Output:
- `hosts sync` — prints each written entry (`  127.0.0.1 myapp.localhost`) or `no active routes`
- `hosts clean` — prints `hosts file cleaned`

---

## Opt-out

```sh
PORTAL_SYNC_HOSTS=0 portless run dev    # disable for this invocation
```

or in `~/.portal/config.toml` — no config key needed; the env var is the only opt-out mechanism (keeps config surface minimal, mirrors JS repo).

---

## Testing

### `src/hosts.rs` — unit tests (no filesystem)
- `build_block` produces correct marker-delimited output
- `remove_block` strips block and normalises whitespace
- `extract_managed` returns lines between markers; returns empty if no markers
- `should_sync` returns false for `"0"` and `"false"`, true for everything else
- Idempotency: `sync` → `sync` again → content unchanged
- Round-trip: `build_block(hostnames)` → `extract_managed` → same hostnames

### `src/daemon/ipc.rs` — integration tests
- After `RegisterRoute`, managed block in `/etc/hosts` (temp file) contains the hostname
- After `Rm`, hostname is absent from managed block
- After `Shutdown`, managed block is removed entirely

### `src/hosts.rs` — platform test
- `hosts_path()` returns expected path for current OS

### Not tested
- macOS DNS flush — fire-and-forget, no observable side-effect in tests
