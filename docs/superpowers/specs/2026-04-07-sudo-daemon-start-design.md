# Sudo Daemon Start — Design Spec

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Fix `portal start` / `portal run` on privileged ports (80/443) so that sudo authentication works correctly, Touch ID works on configured systems, and the user gets clear feedback throughout.

**Architecture:** All changes are confined to `ensure_daemon_running` in `src/cli/mod.rs`. Replace the broken two-step `sudo -v` / `sudo -n` pattern with a single blocking `tokio::process::Command::status().await` call that gives sudo full TTY access — exactly how portless handles it. Fix the sequence so daemon-running check and sudo-need determination happen before any UI is shown.

**Tech Stack:** Rust, Tokio, `console 0.15` (for TTY detection), existing `SetupPrinter` in `src/cli/banner.rs`.

---

## Root Causes

### Bug 1 — Two-step sudo breaks authentication flow

Current code:
```rust
let auth = tokio::process::Command::new("sudo").arg("-v").status().await?;
std::process::Command::new("sudo").arg("-n").arg(&exe).arg("daemon")
    .env("PORTAL_IS_DAEMON", "1")
    .spawn()
```

`sudo -v` authenticates and caches a credential. `sudo -n` immediately uses that cached credential to spawn the daemon non-interactively. The bug: `.spawn()` is non-blocking — it returns the moment the child process starts. There is no guarantee the daemon is running when the socket poll begins. If anything goes wrong silently (port conflict, cert failure), the poll just times out with no feedback.

### Bug 2 — PORTAL_IS_DAEMON stripped by sudo

`.env("PORTAL_IS_DAEMON", "1")` sets the variable in the `sudo` process's environment, but sudo strips unknown env vars before running the target. So `portal daemon` runs without `PORTAL_IS_DAEMON=1`, triggering the double-spawn path instead of running directly.

This is harmless (double-spawn still works) but unexpected and fragile.

### Bug 3 — Wrong sequence

The current code shows cert/daemon UI steps *before* checking whether the daemon is already running or whether sudo is needed. A user running `portal start` for the second time (daemon already up) should see nothing.

### Bug 4 — No feedback during socket poll

After authentication, the user sees nothing for up to 60 seconds. No indication that anything is happening.

### Touch ID (not a bug — it gets fixed for free)

portless has zero PAM / Touch ID code. Touch ID works on portless because `spawnSync("sudo", ..., { stdio: "inherit" })` gives sudo full unmodified TTY access. If the user's macOS has Touch ID configured in `/etc/pam.d/sudo_local`, it shows up automatically. Our current two-step approach breaks this — the single `status().await` fix restores it.

---

## Design

### Sequence (fixed)

```
1. Check if daemon socket exists and is responsive
   → if yes: return immediately (no UI, no work)

2. Compute needs_sudo = http_port < 1024 || https_port < 1024

3. Check for TTY if sudo needed
   → if needs_sudo && !is_tty: print error + hint, exit 1

4. Compute ca_missing = !dirs_for_state().join("ca.pem").exists()

5. Show UI (now we know what we need)
   → sudo path:    plain_step messages only (no spinners)
   → no-sudo path: animated spinners (unchanged)

6. Start daemon
   → sudo path:    single blocking status().await
   → no-sudo path: non-blocking spawn (unchanged)

7. Poll for socket with periodic feedback
```

### Sudo path — single blocking call

```rust
let status = tokio::process::Command::new("sudo")
    .arg(&exe)
    .arg("daemon")
    .stdin(Stdio::inherit())   // sudo gets full TTY → password or Touch ID
    .stdout(Stdio::null())
    .stderr(Stdio::inherit())
    .status()
    .await?;

if !status.success() {
    // print error
    return Err(Error::DaemonNotRunning);
}
```

`portal daemon` (without `PORTAL_IS_DAEMON`) spawns a background grandchild and exits in <100ms. So `status().await` returns quickly after authentication completes. The grandchild continues running as root, binds ports 80/443, and creates the socket.

This is identical in structure to portless:
```javascript
spawnSync("sudo", ["env", ...envArgs, ...startArgs], { stdio: "inherit" })
```

### TTY detection

```rust
use std::io::IsTerminal;
let is_tty = std::io::stdin().is_terminal();
```

`std::io::IsTerminal` is stable since Rust 1.70. If `needs_sudo && !is_tty`, print:
```
error: daemon is not running and no TTY is available for sudo.
  Option 1: run portal in a terminal (will prompt for password):
    portal start
  Option 2: use unprivileged ports in portal.toml:
    [proxy]
    http_port = 8080
    https_port = 8443
```

### Socket polling with feedback

After `status()` returns (sudo path), poll for the socket. `portal daemon` has already exited and the grandchild daemon is starting up. 10 seconds is more than enough:

```
poll 67 × 150ms = ~10 seconds
every 13 iterations (~2 seconds): print "         waiting…"
```

This gives the user visible feedback rather than a frozen terminal.

### No-sudo path (unchanged in behavior)

Animated spinners via `begin_step()` remain. The only change is that the sequence check moves to the top so a running daemon returns immediately without printing any UI.

---

## File Change Map

| File | Change |
|------|--------|
| `src/cli/mod.rs` | Rewrite `ensure_daemon_running` only — fix sequence, replace two-step sudo with single `status().await`, add TTY check, add poll feedback |

No other files change. `banner.rs`, `output.rs`, `detect.rs`, `daemon/mod.rs` are untouched.

---

## Out of Scope

- PAM / Touch ID configuration (works automatically with `Stdio::inherit()` if user's system is configured)
- Windows support for privileged ports
- Non-daemon (foreground) mode
- CI/Docker daemon auto-start (covered by TTY error message pointing to `portal.toml`)
