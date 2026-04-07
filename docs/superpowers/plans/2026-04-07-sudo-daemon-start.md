# Sudo Daemon Start Fix — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Fix `portal start` / `portal run` on privileged ports (80/443) so sudo authentication works, Touch ID shows on configured systems, and the user gets clear feedback.

**Architecture:** Replace the broken two-step `sudo -v` / `sudo -n` in `ensure_daemon_running` (`src/cli/mod.rs:299-409`) with a single blocking `tokio::process::Command::status().await` call that gives sudo full TTY access. Fix execution sequence so the running-check happens before any UI. Add TTY guard for non-interactive environments. Add poll feedback.

**Tech Stack:** Rust (edition 2021), Tokio, `std::io::IsTerminal` (stable since Rust 1.70), `console 0.15`.

---

## File Map

| File | Change |
|------|--------|
| `src/cli/mod.rs` | Replace `ensure_daemon_running` (lines 299–409) — sequence fix, TTY guard, single sudo call, poll feedback |

No other files change.

---

### Task 1: Replace `ensure_daemon_running`

**Files:**
- Modify: `src/cli/mod.rs:299-409`

This task replaces the entire body of `ensure_daemon_running`. Read the existing function (lines 299–409) to orient yourself, then replace it with the code below.

- [ ] **Step 1: Verify the current function compiles and identify line range**

```bash
cargo build 2>&1 | head -20
```

Expected: build succeeds (or only pre-existing errors — nothing in `ensure_daemon_running`).

Note: The function `ensure_daemon_running` runs from line 299 to line 409 in `src/cli/mod.rs`. Its signature is:
```rust
async fn ensure_daemon_running(
    config: &crate::config::Config,
    setup: &mut banner::SetupPrinter,
) -> Result<()>
```

- [ ] **Step 2: Replace the function body**

Replace everything from the opening `{` on line 299 through the closing `}` on line 409 with the implementation below. The function signature on lines 299–302 stays the same; only the body changes.

```rust
async fn ensure_daemon_running(
    config: &crate::config::Config,
    setup: &mut banner::SetupPrinter,
) -> Result<()> {
    let sock = crate::config::dirs_for_state().join("portal.sock");

    // Fast path: daemon is already running — return immediately, no UI.
    if sock.exists() && tokio::net::UnixStream::connect(&sock).await.is_ok() {
        return Ok(());
    }

    let exe = std::env::current_exe()?;
    let needs_sudo = !cfg!(windows)
        && (config.proxy.http_port < 1024 || config.proxy.https_port < 1024);
    let ca_missing = !crate::config::dirs_for_state().join("ca.pem").exists();

    // Non-TTY guard: sudo needs an interactive terminal for its password/Touch ID prompt.
    if needs_sudo {
        use std::io::IsTerminal;
        if !std::io::stdin().is_terminal() {
            eprintln!("error: daemon is not running and no TTY is available for sudo.");
            eprintln!("  Option 1: run portal in a terminal (will prompt for password):");
            eprintln!("    portal start");
            eprintln!("  Option 2: use unprivileged ports in portal.toml:");
            eprintln!("    [proxy]");
            eprintln!("    http_port = 8080");
            eprintln!("    https_port = 8443");
            return Err(crate::error::Error::DaemonNotRunning);
        }
    }

    if needs_sudo {
        // Plain-text path — no indicatif spinners that could corrupt the TTY that sudo needs.
        if ca_missing {
            setup.plain_step("cert     generating CA certificate…");
        }
        setup.plain_step("daemon   starting (sudo may ask for your password)…");

        // Single blocking call — gives sudo full TTY access so the password prompt
        // and Touch ID (if configured in /etc/pam.d/sudo_local) both work naturally.
        //
        // `portal daemon` without PORTAL_IS_DAEMON spawns a background grandchild
        // daemon and exits in <100 ms, so status() returns quickly after authentication.
        // The grandchild continues running as root and binds ports 80/443.
        let status = tokio::process::Command::new("sudo")
            .arg(&exe)
            .arg("daemon")
            .stdin(std::process::Stdio::inherit())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::inherit())
            .status()
            .await?;

        if !status.success() {
            eprintln!(
                "  {} daemon  sudo failed — check the error above",
                console::style("✗").red()
            );
            return Err(crate::error::Error::DaemonNotRunning);
        }

        // Poll for the IPC socket. The grandchild daemon is starting up; 10 s is plenty.
        // Print a waiting line every ~2 s so the user knows we haven't frozen.
        for i in 0..67u32 {
            tokio::time::sleep(std::time::Duration::from_millis(150)).await;
            if tokio::net::UnixStream::connect(&sock).await.is_ok() {
                setup.plain_step(&format!(
                    "{} daemon  started  on :{}/:{}",
                    console::style("✓").green(),
                    config.proxy.http_port,
                    config.proxy.https_port,
                ));
                return Ok(());
            }
            if i > 0 && i % 13 == 0 {
                setup.plain_step("         waiting for daemon…");
            }
        }

        eprintln!(
            "  {} daemon  timed out — socket not found at {}",
            console::style("✗").red(),
            sock.display()
        );
        return Err(crate::error::Error::DaemonNotRunning);
    }

    // No sudo needed: use animated spinners (unchanged behavior).
    let mut cert_pb: Option<indicatif::ProgressBar> = if ca_missing {
        Some(setup.begin_step("cert", "generating CA certificate…"))
    } else {
        None
    };
    let daemon_pb = setup.begin_step("daemon", "starting…");

    if let Err(err) = std::process::Command::new(&exe)
        .arg("daemon")
        .env("PORTAL_IS_DAEMON", "1")
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
    {
        if let Some(pb) = cert_pb.take() {
            pb.abandon_with_message(format!("{} cert    failed", console::style("✗").red()));
        }
        daemon_pb.abandon_with_message(format!(
            "{} daemon  failed to start",
            console::style("✗").red()
        ));
        return Err(err.into());
    }

    for _ in 0..20 {
        tokio::time::sleep(std::time::Duration::from_millis(150)).await;
        if tokio::net::UnixStream::connect(&sock).await.is_ok() {
            if let Some(pb) = cert_pb.take() {
                pb.finish_with_message(format!(
                    "{} cert    generated",
                    console::style("✓").green()
                ));
            }
            daemon_pb.finish_with_message(format!(
                "{} daemon  started  on :{}/:{}",
                console::style("✓").green(),
                config.proxy.http_port,
                config.proxy.https_port,
            ));
            return Ok(());
        }
    }

    if let Some(pb) = cert_pb.take() {
        pb.abandon_with_message(format!("{} cert    failed", console::style("✗").red()));
    }
    daemon_pb.abandon_with_message(format!(
        "{} daemon  failed to start",
        console::style("✗").red()
    ));
    Err(crate::error::Error::DaemonNotRunning)
}
```

- [ ] **Step 3: Build and verify no new errors**

```bash
cargo build 2>&1
```

Expected: compiles cleanly. If you see `IsTerminal` not found, your Rust toolchain is older than 1.70 — run `rustup update stable` first.

- [ ] **Step 4: Commit**

```bash
git add src/cli/mod.rs
git commit -m "fix(cli): replace two-step sudo with single blocking status().await

- Check daemon running FIRST before any UI (fast-path returns immediately)
- Add TTY guard: non-interactive env prints actionable error + exits
- Single tokio::process::Command::status().await with Stdio::inherit gives
  sudo full TTY access, enabling password prompts and Touch ID naturally
- Poll 10s after sudo returns with feedback every 2s (vs silent 60s wait)
- Non-sudo spinner path unchanged except stdin/stdout/stderr set to null

Co-Authored-By: Claude Sonnet 4.6 <noreply@anthropic.com>"
```

---

### Task 2: Manual verification

**Files:** none (testing only)

This task confirms the fix works end-to-end. No code changes.

- [ ] **Step 1: Stop any running daemon**

```bash
portal shutdown 2>/dev/null || true
sleep 1
```

- [ ] **Step 2: Test the privileged-port path (ports 80/443)**

Ensure `portal.toml` (or `~/.portal/portal.toml`) uses default ports (80/443). Then run:

```bash
portal start
```

Expected sequence:
1. Setup header prints: `portal  v1.0.0  ·  first run`
2. `cert     generating CA certificate…` (if first run)
3. `daemon   starting (sudo may ask for your password)…`
4. sudo prompts for password **or** Touch ID (if `/etc/pam.d/sudo_local` has `auth sufficient pam_tid.so`)
5. After authentication: either succeeds immediately or prints `waiting for daemon…` once or twice
6. `✓ daemon  started  on :80/:443`
7. `╰─ ready`
8. Banner prints with `https://appname.localhost`

- [ ] **Step 3: Test the fast path (daemon already running)**

With the daemon running from step 2, run portal start again in a new project directory:

```bash
mkdir /tmp/test-portal && cd /tmp/test-portal && portal start 2>&1 || true
```

Expected: no setup UI printed at all (daemon already running, fast path returns immediately). You'll likely see an error about no `package.json` — that's expected and correct. The key thing is no "first run" header appears.

- [ ] **Step 4: Test the non-TTY guard**

```bash
echo "" | portal start 2>&1 || true
```

Expected output (on stderr):
```
error: daemon is not running and no TTY is available for sudo.
  Option 1: run portal in a terminal (will prompt for password):
    portal start
  Option 2: use unprivileged ports in portal.toml:
    [proxy]
    http_port = 8080
    https_port = 8443
```

Note: this test only shows the error if the daemon is NOT running AND ports are privileged. Stop the daemon first if needed (`portal shutdown`).

- [ ] **Step 5: Test the unprivileged-port path (no sudo)**

Edit `portal.toml` (or create `~/.portal/portal.toml`) with:
```toml
[proxy]
http_port = 8080
https_port = 8443
```

Stop the daemon (`portal shutdown`) and run:

```bash
portal start
```

Expected: animated spinners appear (no sudo prompt). The cert and daemon steps show checkmarks when done.

- [ ] **Step 6: Commit verification note**

No commit needed — this task is testing only.
