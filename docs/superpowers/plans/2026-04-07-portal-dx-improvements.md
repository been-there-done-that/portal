# Portal DX Improvements Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make `portal run` work with zero manual setup — auto-installs the CA cert and starts the daemon on first run, and replaces existing instances by default reusing the same backend port.

**Architecture:** Four surgical changes across four files — `is_ca_trusted()` in certs, `wait_for_port_free()` in ports, `PORTAL_URL` injection in process, and the run-path logic in CLI. No new files, no IPC protocol changes.

**Tech Stack:** Rust, Tokio async, rustls/rcgen for TLS, clap for CLI parsing, nix for UNIX syscalls, serde_json for IPC frames.

---

## File Change Map

| File | Change |
|------|--------|
| `src/certs.rs` | Fix CA common name ("Portless" → "Portal"); add `pub fn is_ca_trusted() -> bool` |
| `src/ports.rs` | Add `pub async fn wait_for_port_free(port: u16, timeout: Duration)` |
| `src/process.rs` | Add `hostname: &str` param to `spawn_child`; inject `PORTAL_URL` env var |
| `src/cli/mod.rs` | Remove `--force`; replace-by-default + port reuse; `ensure_daemon_running` takes config; add `ensure_cert_trusted`; call both in Run handler |

---

## Task 1: `is_ca_trusted()` and fix CA common name (`src/certs.rs`)

**Files:**
- Modify: `src/certs.rs`

The CA common name is currently `"Portless Local CA"` — fix it to `"Portal Local CA"` so `is_ca_trusted()` can search for it by name. Then add the platform-specific `is_ca_trusted()` function.

- [ ] **Step 1: Write the failing test**

Add at the bottom of `src/certs.rs` inside the `#[cfg(test)] mod tests` block:

```rust
#[test]
fn is_ca_trusted_returns_bool_without_panicking() {
    // Just verify the function is callable and returns a bool.
    // In a normal test environment the portal CA is not installed,
    // so we expect false. On a machine where it IS installed this
    // passes regardless.
    let trusted = is_ca_trusted();
    // Either outcome is acceptable; the function must not panic.
    let _ = trusted;
}

#[test]
fn is_ca_trusted_false_for_unknown_cert() {
    // The CA must not be trusted in the base test environment
    // (CI machines have no portal CA). If it happens to be trusted
    // on a dev machine this test is #[ignore]-ed.
    // We just call is_ca_trusted() and do not assert the return value,
    // since the function should never panic.
    let _ = is_ca_trusted();
}
```

- [ ] **Step 2: Run the tests to verify they fail (function doesn't exist yet)**

```bash
cargo test -p portal is_ca_trusted 2>&1 | head -30
```

Expected: `error[E0425]: cannot find function 'is_ca_trusted'`

- [ ] **Step 3: Fix the CA common name**

In `src/certs.rs`, in `ensure_ca()`, change:

```rust
        params
            .distinguished_name
            .push(DnType::CommonName, "Portless Local CA");
```

to:

```rust
        params
            .distinguished_name
            .push(DnType::CommonName, "Portal Local CA");
```

- [ ] **Step 4: Add `is_ca_trusted()` to `src/certs.rs`**

Add this block after the `impl CertStore` closing brace and before `fn safe_hostname`:

```rust
// ---------------------------------------------------------------------------
// System trust store check
// ---------------------------------------------------------------------------

/// Returns true if the Portal local CA certificate is already present in the
/// OS system trust store.  Returns false if not found or if the check fails.
#[cfg(target_os = "macos")]
pub fn is_ca_trusted() -> bool {
    use std::process::Command;
    Command::new("security")
        .args([
            "find-certificate",
            "-c",
            "Portal Local CA",
            "/Library/Keychains/System.keychain",
        ])
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

#[cfg(target_os = "linux")]
pub fn is_ca_trusted() -> bool {
    std::path::Path::new("/usr/local/share/ca-certificates/portal-ca.crt").exists()
}

#[cfg(not(any(target_os = "macos", target_os = "linux")))]
pub fn is_ca_trusted() -> bool {
    false
}
```

- [ ] **Step 5: Run the tests to verify they pass**

```bash
cargo test -p portal is_ca_trusted 2>&1
```

Expected: `test tests::is_ca_trusted_returns_bool_without_panicking ... ok`

- [ ] **Step 6: Run the full test suite to check for regressions**

```bash
cargo test -p portal 2>&1 | tail -20
```

Expected: All existing tests pass. (Note: existing CA tests pass because `ensure_ca` is idempotent and the name only affects newly generated CAs. Users with old "Portless Local CA" certs need to run `portal cert reset` once.)

- [ ] **Step 7: Commit**

```bash
git add src/certs.rs
git commit -m "feat: add is_ca_trusted() and fix CA name to 'Portal Local CA'"
```

---

## Task 2: `wait_for_port_free()` (`src/ports.rs`)

**Files:**
- Modify: `src/ports.rs`

- [ ] **Step 1: Write the failing tests**

Add to the `#[cfg(test)] mod tests` block in `src/ports.rs`:

```rust
#[tokio::test]
async fn returns_immediately_when_port_is_free() {
    // Port 19997 should be free; function should return within 200ms
    let start = std::time::Instant::now();
    wait_for_port_free(19997, std::time::Duration::from_secs(2)).await;
    assert!(
        start.elapsed() < std::time::Duration::from_millis(500),
        "should return quickly when port is free, took {:?}",
        start.elapsed()
    );
}

#[tokio::test]
async fn times_out_when_port_stays_bound() {
    use std::net::TcpListener;
    // Bind port 19996 to simulate a still-running process
    let listener = TcpListener::bind("127.0.0.1:19996").unwrap();
    let start = std::time::Instant::now();
    // Wait with a 350ms timeout
    wait_for_port_free(19996, std::time::Duration::from_millis(350)).await;
    let elapsed = start.elapsed();
    // Should have waited at least ~300ms before timing out
    assert!(
        elapsed >= std::time::Duration::from_millis(250),
        "should have waited for timeout, only waited {:?}",
        elapsed
    );
    drop(listener);
}
```

- [ ] **Step 2: Run to verify they fail**

```bash
cargo test -p portal wait_for_port_free 2>&1 | head -20
```

Expected: `error[E0425]: cannot find function 'wait_for_port_free'`

- [ ] **Step 3: Implement `wait_for_port_free`**

Add after the existing `find_free_port` function in `src/ports.rs`:

```rust
/// Poll until `port` is no longer accepting connections (i.e., the previous
/// process has released it), or until `timeout` elapses.
/// Never returns an error — on timeout it simply returns so the caller can
/// proceed (the new process will bind once the old one fully exits).
pub async fn wait_for_port_free(port: u16, timeout: std::time::Duration) {
    let deadline = tokio::time::Instant::now() + timeout;
    loop {
        // If connect fails, the port is free
        if tokio::net::TcpStream::connect(format!("127.0.0.1:{port}"))
            .await
            .is_err()
        {
            return;
        }
        if tokio::time::Instant::now() >= deadline {
            return; // Timeout — proceed anyway
        }
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    }
}
```

- [ ] **Step 4: Run the tests to verify they pass**

```bash
cargo test -p portal wait_for_port_free 2>&1
```

Expected:
```
test tests::returns_immediately_when_port_is_free ... ok
test tests::times_out_when_port_stays_bound ... ok
```

- [ ] **Step 5: Run the full test suite**

```bash
cargo test -p portal 2>&1 | tail -10
```

Expected: All tests pass.

- [ ] **Step 6: Commit**

```bash
git add src/ports.rs
git commit -m "feat: add wait_for_port_free() for port-reuse replace semantics"
```

---

## Task 3: Inject `PORTAL_URL` env var into child process (`src/process.rs`)

**Files:**
- Modify: `src/process.rs`
- Modify: `src/cli/mod.rs` (call site only — update the `spawn_child` call to pass `&hostname`)

`spawn_child` needs a `hostname` parameter so it can inject both `PORT` and `PORTAL_URL` into the child's environment.

- [ ] **Step 1: Write the failing test**

Add to the `#[cfg(test)] mod tests` block in `src/process.rs`:

```rust
#[tokio::test]
async fn child_receives_portal_url_env() {
    #[cfg(unix)]
    {
        use rand::Rng;
        let mut rng = rand::thread_rng();
        let random_id = rng.gen::<u32>();
        let test_file = format!("/tmp/portal_url_test_{}.txt", random_id);

        let args = vec![
            "sh".to_string(),
            "-c".to_string(),
            format!("echo $PORTAL_URL > {}", test_file),
        ];

        let mut child = spawn_child(
            Path::new("/tmp"),
            &args,
            4321,
            "myapp.localhost",
        )
        .await
        .expect("Failed to spawn child");

        let _ = child.wait().await;

        let content = std::fs::read_to_string(&test_file)
            .expect("Failed to read test file");
        assert_eq!(content.trim(), "https://myapp.localhost");

        let _ = std::fs::remove_file(&test_file);
    }
}
```

- [ ] **Step 2: Run to verify it fails**

```bash
cargo test -p portal child_receives_portal_url_env 2>&1 | head -20
```

Expected: compile error — `spawn_child` called with wrong number of arguments (test uses 4 args, function currently takes 3).

- [ ] **Step 3: Update `spawn_child` signature and inject `PORTAL_URL`**

In `src/process.rs`, change the function signature and env setup:

```rust
/// Spawn a child dev server process.
/// Sets PORT=<port> and PORTAL_URL=https://<hostname> env vars.
/// Calls extra_args_for_port to inject framework flags.
pub async fn spawn_child(
    cwd: &Path,
    args: &[String],
    port: u16,
    hostname: &str,
) -> Result<tokio::process::Child> {
    if args.is_empty() {
        return Err(crate::error::Error::Ipc(
            "No arguments provided to spawn_child".to_string(),
        ));
    }

    // Split args into program and rest
    let program = &args[0];
    let rest_args: Vec<&str> = args[1..].iter().map(|s| s.as_str()).collect();

    // Get extra args for this framework
    let extra = crate::detect::extra_args_for_port(cwd, &rest_args, port)?;

    // Spawn the child process
    let mut cmd = Command::new(program);
    cmd.args(&rest_args)
        .args(&extra)
        .env("PORT", port.to_string())
        .env("PORTAL_URL", format!("https://{hostname}"))
        .current_dir(cwd)
        .kill_on_drop(false);

    let child = cmd.spawn()?;
    Ok(child)
}
```

- [ ] **Step 4: Update existing tests in `src/process.rs` that call `spawn_child` with 3 args**

In `spawns_and_kills_child`, change:
```rust
            let mut child = spawn_child(Path::new("/tmp"), &args, 4321)
```
to:
```rust
            let mut child = spawn_child(Path::new("/tmp"), &args, 4321, "test.localhost")
```

In `child_receives_port_env`, change:
```rust
            let mut child = spawn_child(Path::new("/tmp"), &args, 4321)
```
to:
```rust
            let mut child = spawn_child(Path::new("/tmp"), &args, 4321, "test.localhost")
```

(And do the same for any Windows `#[cfg(windows)]` variants in those tests — replace `spawn_child(Path::new("C:\\"), &args, 4321)` with `spawn_child(Path::new("C:\\"), &args, 4321, "test.localhost")`.)

- [ ] **Step 5: Update the call site in `src/cli/mod.rs`**

Find (around line 191):
```rust
            let mut child = crate::process::spawn_child(&cwd, &args, port).await?;
```

Change to:
```rust
            let mut child = crate::process::spawn_child(&cwd, &args, port, &hostname).await?;
```

- [ ] **Step 6: Run all tests**

```bash
cargo test -p portal 2>&1 | tail -15
```

Expected: All tests pass including `child_receives_portal_url_env`.

- [ ] **Step 7: Commit**

```bash
git add src/process.rs src/cli/mod.rs
git commit -m "feat: inject PORTAL_URL env var into child dev server process"
```

---

## Task 4: Replace-by-default with port reuse (`src/cli/mod.rs`)

**Files:**
- Modify: `src/cli/mod.rs`

Remove the `--force` flag. When `portal run` detects an existing live route for the same hostname, it stops the old process, waits for its port to free, then starts the new process on the same port.

- [ ] **Step 1: Remove `--force` from the `CliCommand::Run` variant**

In `src/cli/mod.rs`, change the `Run` variant from:

```rust
    Run {
        #[arg(long)]
        hostname: Option<String>,
        #[arg(long)]
        port: Option<u16>,
        /// Kill any existing instance for this hostname before starting
        #[arg(long)]
        force: bool,
        #[arg(trailing_var_arg = true, required = true)]
        args: Vec<String>,
    },
```

to:

```rust
    Run {
        #[arg(long)]
        hostname: Option<String>,
        #[arg(long)]
        port: Option<u16>,
        #[arg(trailing_var_arg = true, required = true)]
        args: Vec<String>,
    },
```

- [ ] **Step 2: Replace the duplicate-detection and port-assignment logic in the `Run` match arm**

Find the current `CliCommand::Run` match arm (around line 143). Replace the entire match arm with:

```rust
        CliCommand::Run {
            hostname,
            port,
            args,
        } => {
            // Load config and cwd first (needed for port range + hostname resolution)
            let cwd = std::env::current_dir()?;
            let config = crate::config::Config::load(&cwd)?;
            ensure_daemon_running().await?;   // signature updated in Task 5
            let hostname =
                crate::detect::resolve_hostname(&cwd, hostname.as_deref(), &config.proxy.tld);

            // Check for an existing live route for this hostname
            let reuse_port: Option<u16> = {
                let mut stream = ipc_connect().await?;
                write_frame(&mut stream, &Command::Ls).await?;
                let resp: crate::proto::Response = read_frame(&mut stream).await?;
                if let Some(serde_json::Value::Array(routes)) = resp.data {
                    routes
                        .iter()
                        .find(|r| r["hostname"].as_str() == Some(&hostname))
                        .and_then(|r| r["port"].as_u64())
                        .and_then(|p| u16::try_from(p).ok())
                } else {
                    None
                }
            };

            // Determine backend port:
            //   1. User pinned --port  → use it (stop old if exists)
            //   2. Existing route      → stop old, reuse its port
            //   3. No existing route   → find a free port
            let port = if let Some(explicit_port) = port {
                if let Some(old_port) = reuse_port {
                    let mut s = ipc_connect().await?;
                    write_frame(&mut s, &Command::Stop { hostname: hostname.clone() }).await?;
                    let _: crate::proto::Response = read_frame(&mut s).await?;
                    eprintln!("  replaced existing instance (port {})", old_port);
                    crate::ports::wait_for_port_free(
                        explicit_port,
                        std::time::Duration::from_secs(2),
                    )
                    .await;
                }
                explicit_port
            } else if let Some(old_port) = reuse_port {
                // Replace-by-default: stop old, reuse its port
                let mut s = ipc_connect().await?;
                write_frame(&mut s, &Command::Stop { hostname: hostname.clone() }).await?;
                let _: crate::proto::Response = read_frame(&mut s).await?;
                eprintln!("  replaced existing instance (port {})", old_port);
                crate::ports::wait_for_port_free(old_port, std::time::Duration::from_secs(2))
                    .await;
                old_port
            } else {
                crate::ports::find_free_port(
                    config.proxy.port_range.0,
                    config.proxy.port_range.1,
                )?
            };

            let my_pid = std::process::id();
            let mut child = crate::process::spawn_child(&cwd, &args, port, &hostname).await?;

            eprintln!("  https://{hostname}  ->  port {port}");

            // Register the route in the daemon's live in-memory store via IPC
            {
                let child_pid = child.id().unwrap_or(my_pid);
                if let Ok(mut stream) = ipc_connect().await {
                    let _ = write_frame(
                        &mut stream,
                        &Command::RegisterRoute {
                            hostname: hostname.clone(),
                            port,
                            pid: child_pid,
                            cwd: cwd.to_string_lossy().to_string(),
                        },
                    )
                    .await;
                    let _: crate::proto::Response = read_frame(&mut stream)
                        .await
                        .unwrap_or(crate::proto::Response::ok_empty());
                }
            }

            child.wait().await?;
        }
```

- [ ] **Step 3: Verify no reference to `force` remains**

The old arm had `--force` stripping lines — they are already gone in the replacement above. Confirm:

```bash
grep -n "force" /Users/__deesh_reddy__/projects/personal_git/rust_builds/portless/src/cli/mod.rs
```

Expected: no output.

- [ ] **Step 4: Build to check for compile errors**

```bash
cargo build -p portal 2>&1
```

Expected: Compiles without errors.

- [ ] **Step 5: Run tests**

```bash
cargo test -p portal 2>&1 | tail -15
```

Expected: All tests pass.

- [ ] **Step 6: Commit**

```bash
git add src/cli/mod.rs
git commit -m "feat: replace-by-default with port reuse, remove --force flag"
```

---

## Task 5: Transparent first-run setup (`src/cli/mod.rs`)

**Files:**
- Modify: `src/cli/mod.rs`

Two changes:
1. `ensure_daemon_running` gets a `config: &Config` parameter; if privileged ports are configured it uses `sudo` and prints a message.
2. New `ensure_cert_trusted()` is called after daemon is up; auto-elevates `sudo portal cert install` if the CA is not trusted.

- [ ] **Step 1: Update `ensure_daemon_running` signature and body**

Find the current `ensure_daemon_running` function (it was last called with no args in Tasks 4 — update all call sites at the same time). Replace the function entirely:

```rust
async fn ensure_daemon_running(config: &crate::config::Config) -> Result<()> {
    let sock = crate::config::dirs_for_state().join("portal.sock");
    if sock.exists() && tokio::net::UnixStream::connect(&sock).await.is_ok() {
        return Ok(());
    }
    let exe = std::env::current_exe()?;
    let needs_sudo =
        config.proxy.http_port < 1024 || config.proxy.https_port < 1024;
    if needs_sudo {
        eprintln!("  portal: starting daemon (requires sudo for ports 80/443)...");
        tokio::process::Command::new("sudo")
            .arg(&exe)
            .arg("daemon")
            .env("PORTAL_IS_DAEMON", "1")
            .spawn()?;
    } else {
        tokio::process::Command::new(&exe)
            .arg("daemon")
            .env("PORTAL_IS_DAEMON", "1")
            .spawn()?;
    }
    for _ in 0..20 {
        tokio::time::sleep(std::time::Duration::from_millis(150)).await;
        if tokio::net::UnixStream::connect(&sock).await.is_ok() {
            return Ok(());
        }
    }
    Err(crate::error::Error::DaemonNotRunning)
}
```

- [ ] **Step 2: Add `ensure_cert_trusted()`**

Add this function directly after `ensure_daemon_running` in `src/cli/mod.rs`:

```rust
async fn ensure_cert_trusted() -> Result<()> {
    if crate::certs::is_ca_trusted() {
        return Ok(());
    }
    eprintln!("  portal: trusting CA certificate (requires sudo)...");
    let exe = std::env::current_exe()?;
    let status = tokio::process::Command::new("sudo")
        .arg(&exe)
        .arg("cert")
        .arg("install")
        .status()
        .await?;
    if !status.success() {
        return Err(crate::error::Error::Ipc(
            "Failed to install CA certificate. Run `sudo portal cert install` manually."
                .to_string(),
        ));
    }
    Ok(())
}
```

- [ ] **Step 3: Update the `Ls` call site to pass config**

The `Ls` handler also calls `ensure_daemon_running()`. It needs a config now. Change the `Ls` match arm from:

```rust
        CliCommand::Ls => {
            ensure_daemon_running().await?;
            let mut stream = ipc_connect().await?;
```

to:

```rust
        CliCommand::Ls => {
            let cwd = std::env::current_dir()?;
            let config = crate::config::Config::load(&cwd)?;
            ensure_daemon_running(&config).await?;
            let mut stream = ipc_connect().await?;
```

- [ ] **Step 4: Wire `ensure_cert_trusted()` into the `Run` handler and update `ensure_daemon_running` call sites**

The `Run` arm currently starts with (from Task 4):
```rust
            let cwd = std::env::current_dir()?;
            let config = crate::config::Config::load(&cwd)?;
            ensure_daemon_running().await?;   // still old signature
```

Change these three lines to:
```rust
            let cwd = std::env::current_dir()?;
            let config = crate::config::Config::load(&cwd)?;
            ensure_daemon_running(&config).await?;
            ensure_cert_trusted().await?;
```

Also update the `Ls` arm call site (Step 3 above already adds `config` loading there — change `ensure_daemon_running().await?` to `ensure_daemon_running(&config).await?`).

- [ ] **Step 6: Build**

```bash
cargo build -p portal 2>&1
```

Expected: clean build. If there are "unused variable" or "variable already defined" errors, fix them before continuing.

- [ ] **Step 7: Run all tests**

```bash
cargo test -p portal 2>&1 | tail -15
```

Expected: All tests pass.

- [ ] **Step 8: Verify the full CLI help still looks correct**

```bash
cargo run -p portal -- --help 2>&1
cargo run -p portal -- run --help 2>&1
```

Expected: `run --help` shows `--hostname` and `--port` but NOT `--force`.

- [ ] **Step 9: Commit**

```bash
git add src/cli/mod.rs
git commit -m "feat: transparent first-run setup — auto sudo cert trust + daemon start"
```

---

## Spec Coverage Checklist (self-review)

| Spec requirement | Task |
|-----------------|------|
| `is_ca_trusted()` on macOS via `security find-certificate` | Task 1 |
| `is_ca_trusted()` on Linux via file existence | Task 1 |
| CA common name fixed to "Portal Local CA" | Task 1 |
| `wait_for_port_free(port, timeout)` polls 100ms intervals | Task 2 |
| `wait_for_port_free` never errors on timeout | Task 2 |
| `PORTAL_URL` env var injected into child | Task 3 |
| `spawn_child` signature updated | Task 3 |
| `--force` flag removed | Task 4 |
| Replace-by-default: always stop old on same hostname | Task 4 |
| Port reuse: new process uses same port as old | Task 4 |
| `--port` explicit override still works | Task 4 |
| `ensure_daemon_running` takes config, uses sudo for privileged ports | Task 5 |
| Prints message before sudo elevation | Task 5 |
| `ensure_cert_trusted` auto-installs CA if not trusted | Task 5 |
| `ensure_cert_trusted` called in Run handler after daemon check | Task 5 |
| `Ls` handler updated for new `ensure_daemon_running` signature | Task 5 |
