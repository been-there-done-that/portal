# Status Polish & Spinner Fix — Design Spec

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Fix the `⠏` spinner artifact on the trust step and improve `portal status` to show daemon info + routes in a single well-formatted view.

**Architecture:** Two targeted changes: (1) `ensure_cert_trusted` in `src/cli/mod.rs` switches from `begin_step` to `plain_step` to avoid TTY/spinner conflict during sudo; (2) `portal status` output is enriched — the daemon returns `http_port`/`https_port` in the `Status` response, uptime is formatted human-readably, and the active routes table is embedded directly below the daemon info.

**Tech Stack:** Rust, `console 0.15`, `indicatif 0.17`, existing `SetupPrinter` and `print_ls`/`print_status` in `src/cli/output.rs`.

---

## Part 1: Spinner Artifact Fix

### Problem

`ensure_cert_trusted` in `src/cli/mod.rs` creates a spinner via `setup.begin_step("trust", ...)` then immediately runs `tokio::process::Command::new("sudo")...status().await`. While sudo runs, the spinner is live. When sudo finishes, `finish_with_message` is called — but the last-drawn spinner frame (`⠏`) bleeds into the terminal alongside the final message, producing:

```
  ⠏ ✓ trust   installed  (sudo)                       ╰─ ready
```

### Fix

Replace `begin_step` with `plain_step` in `ensure_cert_trusted`. Since trust also runs sudo, it has the same TTY conflict as the daemon path. Consistent with the fix applied to `ensure_daemon_running`.

**Before:**
```rust
let trust_pb = setup.begin_step("trust", "installing CA certificate…  (sudo required)");
// ... sudo ...
trust_pb.finish_with_message(format!("{} trust   installed  (sudo)", ...));
```

**After:**
```rust
setup.plain_step("trust    installing CA certificate…  (sudo required)");
// ... sudo ...
setup.plain_step(&format!("{} trust   installed  (sudo)", console::style("✓").green()));
```

On failure, `plain_step` the error message then return `Err`.

---

## Part 2: `portal status` Improvements

### 2a. Daemon adds `http_port` and `https_port` to Status response

`src/daemon/ipc.rs` `Command::Status` handler currently returns `version`, `pid`, `uptime_secs`, `routes_count`. Add `http_port` and `https_port` from the config.

The `IpcServer::serve` method needs access to the config ports. Pass them in via `IpcServer::new` or read config inside the handler. The simplest approach: add `http_port: u16` and `https_port: u16` fields to `IpcServer`, set at construction in `daemon/mod.rs`.

**Updated Status response:**
```json
{
  "version": "1.0.0",
  "pid": 12345,
  "uptime_secs": 9123,
  "http_port": 80,
  "https_port": 443,
  "routes_count": 3
}
```

### 2b. Uptime formatted as human-readable

Convert `uptime_secs` to `Xh Ym Zs` in `print_status` in `src/cli/output.rs`:

| Seconds | Display |
|---------|---------|
| 45 | `45s` |
| 125 | `2m 5s` |
| 7323 | `2h 2m 3s` |

```rust
fn format_uptime(secs: u64) -> String {
    let h = secs / 3600;
    let m = (secs % 3600) / 60;
    let s = secs % 60;
    if h > 0 { format!("{h}h {m}m {s}s") }
    else if m > 0 { format!("{m}m {s}s") }
    else { format!("{s}s") }
}
```

### 2c. `portal status` embeds routes table

After the daemon info block, print a blank line then the active routes — reusing the existing `print_ls` logic. This means `portal status` no longer needs a separate `portal ls` call for a full picture.

**Output shape:**
```
  portal   v1.0.0

  pid      12345
  uptime   2h 32m 5s
  ports    :80  →  :443
  routes   3

  HOSTNAME                         PORT   URL
  ────────────────────────────────────────────────────────────
  livsyt.localhost                  4229  https://livsyt.localhost
```

The `IpcServer` needs access to the `RouteStore` for the status response to include route data, **or** the CLI makes two IPC calls (Status + Ls) and combines them in `print_status`. The simpler approach: CLI makes two IPC calls. No protocol change required.

### 2d. `portal status` CLI change

`CliCommand::Status` in `src/cli/mod.rs` currently makes one IPC call. Update it to make two:

```rust
CliCommand::Status => {
    let mut stream = ipc_connect().await?;
    write_frame(&mut stream, &Command::Status).await?;
    let status_resp = read_frame(&mut stream).await?;

    let mut stream2 = ipc_connect().await?;
    write_frame(&mut stream2, &Command::Ls).await?;
    let ls_resp = read_frame(&mut stream2).await?;

    output::print_status(&status_resp, &ls_resp);
}
```

Update `print_status` signature to `pub fn print_status(status: &Response, routes: &Response)`.

---

## File Change Map

| File | Change |
|------|--------|
| `src/cli/mod.rs` | Fix `ensure_cert_trusted`: replace `begin_step`/`finish_with_message` with `plain_step`; update `CliCommand::Status` to make two IPC calls |
| `src/daemon/ipc.rs` | Add `http_port`/`https_port` fields to `IpcServer`, include them in `Status` response |
| `src/daemon/mod.rs` | Pass `http_port`/`https_port` to `IpcServer::new` |
| `src/cli/output.rs` | Add `format_uptime`; update `print_status` to accept routes response and render combined view |

---

## Out of Scope

- Live-updating status (would need ratatui)
- PID/CWD columns in the routes table (save for a dedicated `portal ls` polish pass)
- Color themes
