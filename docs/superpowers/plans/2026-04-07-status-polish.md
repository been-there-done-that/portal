# Status Polish & Spinner Fix — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Fix the `⠏` spinner artifact on the trust step and enrich `portal status` to show daemon info (with human-readable uptime and ports) plus active routes in one view.

**Architecture:** Four changes across four files: fix `ensure_cert_trusted` to use `plain_step` instead of a live spinner; add `http_port`/`https_port` to `IpcServer` and its `Status` response; add `format_uptime` helper and update `print_status` to accept and render routes; update `CliCommand::Status` to make two IPC calls.

**Tech Stack:** Rust, `console 0.15`, `indicatif 0.17`, existing `SetupPrinter`/`print_ls`/`print_status`.

---

## File Map

| File | Change |
|------|--------|
| `src/cli/mod.rs` | Fix `ensure_cert_trusted` (lines 441–472); update `CliCommand::Status` arm (lines 120–125) |
| `src/daemon/ipc.rs` | Add `http_port`/`https_port` fields to `IpcServer`; update `new()` and `Status` dispatch |
| `src/daemon/mod.rs` | Pass `config.proxy.http_port`/`https_port` to `IpcServer::new` |
| `src/cli/output.rs` | Add `format_uptime`; update `print_status` signature and body |

---

### Task 1: Fix spinner artifact in `ensure_cert_trusted`

**Files:**
- Modify: `src/cli/mod.rs:441-472`

The current code creates a live spinner (`begin_step`) then runs sudo — the TTY/spinner conflict leaves `⠏` in the output. Replace with `plain_step` calls throughout.

- [ ] **Step 1: Read `ensure_cert_trusted` (lines 441–472) to orient yourself**

The function is:
```rust
async fn ensure_cert_trusted(setup: &mut banner::SetupPrinter) -> Result<()> {
    if crate::certs::is_ca_trusted() {
        return Ok(());
    }

    let trust_pb = setup.begin_step("trust", "installing CA certificate…  (sudo required)");

    let exe = std::env::current_exe()?;
    let status = tokio::process::Command::new("sudo")
        .arg(&exe)
        .arg("cert")
        .arg("install")
        .status()
        .await?;

    if !status.success() {
        trust_pb.abandon_with_message(format!(
            "{} trust   failed  (run `sudo portal cert install` manually)",
            console::style("✗").red()
        ));
        return Err(crate::error::Error::Cert(
            "Failed to install CA certificate. Run `sudo portal cert install` manually."
                .to_string(),
        ));
    }

    trust_pb.finish_with_message(format!(
        "{} trust   installed  (sudo)",
        console::style("✓").green()
    ));
    Ok(())
}
```

- [ ] **Step 2: Replace the function body**

Replace the entire `ensure_cert_trusted` function with:

```rust
async fn ensure_cert_trusted(setup: &mut banner::SetupPrinter) -> Result<()> {
    if crate::certs::is_ca_trusted() {
        return Ok(());
    }

    // Use plain_step (no spinner) — sudo needs raw TTY access, same as daemon start.
    setup.plain_step("trust    installing CA certificate…  (sudo required)");

    let exe = std::env::current_exe()?;
    let status = tokio::process::Command::new("sudo")
        .arg(&exe)
        .arg("cert")
        .arg("install")
        .stdin(std::process::Stdio::inherit())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::inherit())
        .status()
        .await?;

    if !status.success() {
        setup.plain_step(&format!(
            "{} trust   failed  (run `sudo portal cert install` manually)",
            console::style("✗").red()
        ));
        return Err(crate::error::Error::Cert(
            "Failed to install CA certificate. Run `sudo portal cert install` manually."
                .to_string(),
        ));
    }

    setup.plain_step(&format!(
        "{} trust   installed  (sudo)",
        console::style("✓").green()
    ));
    Ok(())
}
```

- [ ] **Step 3: Build**

```bash
cargo build 2>&1
```

Expected: compiles cleanly.

- [ ] **Step 4: Commit**

```bash
git add src/cli/mod.rs
git commit -m "fix(cli): replace trust spinner with plain_step to fix artifact"
```

---

### Task 2: Add `http_port`/`https_port` to `IpcServer` and `Status` response

**Files:**
- Modify: `src/daemon/ipc.rs:7-24` (struct + new) and `:117-126` (Status dispatch)
- Modify: `src/daemon/mod.rs` (IpcServer::new call)

- [ ] **Step 1: Add fields to `IpcServer` struct and `new()`**

In `src/daemon/ipc.rs`, the current struct is:
```rust
pub struct IpcServer {
    sock_path: PathBuf,
    pid_path: PathBuf,
    routes: RouteStore,
    start_time: std::time::Instant,
}

impl IpcServer {
    pub fn new(sock_path: PathBuf, pid_path: PathBuf, routes: RouteStore) -> Self {
        std::fs::remove_file(&sock_path).ok();
        IpcServer {
            sock_path,
            pid_path,
            routes,
            start_time: std::time::Instant::now(),
        }
    }
```

Replace with:
```rust
pub struct IpcServer {
    sock_path: PathBuf,
    pid_path: PathBuf,
    routes: RouteStore,
    start_time: std::time::Instant,
    http_port: u16,
    https_port: u16,
}

impl IpcServer {
    pub fn new(
        sock_path: PathBuf,
        pid_path: PathBuf,
        routes: RouteStore,
        http_port: u16,
        https_port: u16,
    ) -> Self {
        std::fs::remove_file(&sock_path).ok();
        IpcServer {
            sock_path,
            pid_path,
            routes,
            start_time: std::time::Instant::now(),
            http_port,
            https_port,
        }
    }
```

- [ ] **Step 2: Thread ports into `handle_connection` and `dispatch`**

In `src/daemon/ipc.rs`, `serve()` currently passes `routes`, `start_time`, `sock_path`, `pid_path` to `handle_connection`. Add `http_port` and `https_port`.

Replace the `serve` method's inner clone/loop section. Find:
```rust
        let routes = self.routes.clone();
        let start_time = self.start_time;
        let sock_path = self.sock_path.clone();
        let pid_path = self.pid_path.clone();

        loop {
            match listener.accept().await {
                Ok((stream, _)) => {
                    let routes = routes.clone();
                    let sock = sock_path.clone();
                    let pid = pid_path.clone();
                    tokio::spawn(async move {
                        handle_connection(stream, routes, start_time, sock, pid).await;
                    });
```

Replace with:
```rust
        let routes = self.routes.clone();
        let start_time = self.start_time;
        let sock_path = self.sock_path.clone();
        let pid_path = self.pid_path.clone();
        let http_port = self.http_port;
        let https_port = self.https_port;

        loop {
            match listener.accept().await {
                Ok((stream, _)) => {
                    let routes = routes.clone();
                    let sock = sock_path.clone();
                    let pid = pid_path.clone();
                    tokio::spawn(async move {
                        handle_connection(stream, routes, start_time, sock, pid, http_port, https_port).await;
                    });
```

Update `handle_connection` signature:
```rust
async fn handle_connection(
    mut stream: tokio::net::UnixStream,
    routes: RouteStore,
    start_time: std::time::Instant,
    sock_path: PathBuf,
    pid_path: PathBuf,
    http_port: u16,
    https_port: u16,
) {
    let cmd: Command = match read_frame(&mut stream).await {
        Ok(c) => c,
        Err(_) => return,
    };

    let response = dispatch(cmd, routes, start_time, sock_path, pid_path, http_port, https_port).await;

    write_frame(&mut stream, &response).await.ok();
}
```

Update `dispatch` signature and `Status` arm:
```rust
async fn dispatch(
    cmd: Command,
    routes: RouteStore,
    start_time: std::time::Instant,
    sock_path: PathBuf,
    pid_path: PathBuf,
    http_port: u16,
    https_port: u16,
) -> Response {
    match cmd {
        // ... Ls arm unchanged ...

        Command::Status => {
            let uptime_secs = start_time.elapsed().as_secs();
            let routes_count = routes.list().len();
            Response::ok(serde_json::json!({
                "version": env!("CARGO_PKG_VERSION"),
                "pid": std::process::id(),
                "uptime_secs": uptime_secs,
                "http_port": http_port,
                "https_port": https_port,
                "routes_count": routes_count,
            }))
        }
        // ... rest unchanged ...
```

- [ ] **Step 3: Update `IpcServer::new` call in `src/daemon/mod.rs`**

In `src/daemon/mod.rs`, find:
```rust
    let ipc = ipc::IpcServer::new(sock_path, pid_path, routes.clone());
```

Replace with:
```rust
    let ipc = ipc::IpcServer::new(sock_path, pid_path, routes.clone(), config.proxy.http_port, config.proxy.https_port);
```

- [ ] **Step 4: Build**

```bash
cargo build 2>&1
```

Expected: compiles cleanly. Fix any type errors — the compiler will point at every call site that needs updating.

- [ ] **Step 5: Commit**

```bash
git add src/daemon/ipc.rs src/daemon/mod.rs
git commit -m "feat(daemon): expose http_port/https_port in Status response"
```

---

### Task 3: Update `print_status` with formatted uptime, ports, and embedded routes

**Files:**
- Modify: `src/cli/output.rs`

- [ ] **Step 1: Add `format_uptime` and rewrite `print_status`**

Replace the entire `print_status` function and add the helper. The new `print_status` takes a second `routes` response:

```rust
fn format_uptime(secs: u64) -> String {
    let h = secs / 3600;
    let m = (secs % 3600) / 60;
    let s = secs % 60;
    if h > 0 {
        format!("{h}h {m}m {s}s")
    } else if m > 0 {
        format!("{m}m {s}s")
    } else {
        format!("{s}s")
    }
}

/// Print daemon status + active routes.
pub fn print_status(status: &Response, routes: &Response) {
    if !status.ok {
        let msg = status.error.as_deref().unwrap_or("unknown error");
        eprintln!("error: {msg}");
        std::process::exit(1);
    }

    if let Some(data) = &status.data {
        let version = data["version"].as_str().unwrap_or("?");
        let pid = data["pid"].as_u64().unwrap_or(0);
        let uptime = format_uptime(data["uptime_secs"].as_u64().unwrap_or(0));
        let http_port = data["http_port"].as_u64().unwrap_or(80);
        let https_port = data["https_port"].as_u64().unwrap_or(443);
        let routes_count = data["routes_count"].as_u64().unwrap_or(0);

        println!(
            "  {}  {}",
            style(" portal ").bold().white().on_blue(),
            style(format!("v{version}")).dim()
        );
        println!();

        let label_w = 10;
        println!(
            "  {}  {}",
            pad_right(&style("pid").dim().to_string(), label_w),
            style(pid.to_string()).dim()
        );
        println!(
            "  {}  {}",
            pad_right(&style("uptime").dim().to_string(), label_w),
            style(&uptime).dim()
        );
        println!(
            "  {}  {}  →  {}",
            pad_right(&style("ports").dim().to_string(), label_w),
            style(format!(":{http_port}")).dim(),
            style(format!(":{https_port}")).dim()
        );
        println!(
            "  {}  {}",
            pad_right(&style("routes").dim().to_string(), label_w),
            style(routes_count.to_string()).green()
        );

        if routes_count > 0 {
            println!();
            // Reuse the routes table rendering inline
            print_routes_table(routes);
        }
    } else {
        println!("{}", style("daemon running, no status data available").dim());
    }
}

/// Shared routes table renderer used by both print_ls and print_status.
fn print_routes_table(resp: &Response) {
    let routes = match &resp.data {
        Some(serde_json::Value::Array(arr)) if !arr.is_empty() => arr.clone(),
        _ => return,
    };

    let hostname_col = style("HOSTNAME").dim().to_string();
    let port_col = style("PORT").dim().to_string();
    let url_col = style("URL").dim().to_string();
    println!(
        "  {}  {}  {}",
        pad_right(&hostname_col, 30),
        pad_left(&port_col, 6),
        url_col
    );
    println!("  {}", style("─".repeat(58)).dim());
    for route in &routes {
        let hostname = route["hostname"].as_str().unwrap_or("-");
        let port = route["port"].as_u64().unwrap_or(0);
        let url = format!("https://{hostname}");
        let hostname_styled = style(hostname).dim().to_string();
        let port_styled = style(format!("{port}")).red().to_string();
        let url_styled = style(url).bold().white().to_string();
        println!(
            "  {}  {}  {}",
            pad_right(&hostname_styled, 30),
            pad_left(&port_styled, 6),
            url_styled
        );
    }
}
```

Also update `print_ls` to call `print_routes_table` instead of duplicating the table logic:

```rust
pub fn print_ls(resp: &Response) {
    if !resp.ok {
        let msg = resp.error.as_deref().unwrap_or("unknown error");
        eprintln!("error: {msg}");
        std::process::exit(1);
    }

    match &resp.data {
        Some(serde_json::Value::Array(arr)) if !arr.is_empty() => {
            print_routes_table(resp);
        }
        _ => {
            println!("{}", style("No active routes.").dim());
        }
    }
}
```

- [ ] **Step 2: Build — expect a compile error in `src/cli/mod.rs`**

```bash
cargo build 2>&1
```

Expected: error on `output::print_status(&resp)` — signature changed. Fix in next step.

---

### Task 4: Update `CliCommand::Status` to make two IPC calls

**Files:**
- Modify: `src/cli/mod.rs:120-125`

- [ ] **Step 1: Update the Status arm**

Find:
```rust
        CliCommand::Status => {
            let mut stream = ipc_connect().await?;
            write_frame(&mut stream, &Command::Status).await?;
            let resp = read_frame(&mut stream).await?;
            output::print_status(&resp);
        }
```

Replace with:
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

- [ ] **Step 2: Build**

```bash
cargo build 2>&1
```

Expected: compiles cleanly.

- [ ] **Step 3: Run tests**

```bash
cargo test 2>&1
```

Expected: all tests pass.

- [ ] **Step 4: Commit**

```bash
git add src/cli/mod.rs src/cli/output.rs
git commit -m "feat(cli): enrich portal status with uptime, ports, and embedded routes table"
```

---

### Task 5: Manual verification

- [ ] **Step 1: Verify spinner artifact is gone**

With the daemon running, run a fresh `portal run` or `portal start` in a project without a trusted cert (or run `portal cert reset` first to force re-trust):

```bash
portal cert reset && portal start
```

Expected output — NO `⠏` artifact:
```
   portal   v1.0.0  ·  first run

  cert     generating CA certificate…
  daemon   starting (sudo may ask for your password)…
  ✓ daemon  started  on :80/:443
  trust    installing CA certificate…  (sudo required)
  ✓ trust   installed  (sudo)
  ╰─ ready
```

- [ ] **Step 2: Verify `portal status`**

```bash
portal status
```

Expected:
```
   portal   v1.0.0

  pid         12345
  uptime      2m 5s
  ports       :80  →  :443
  routes      1

  HOSTNAME                         PORT   URL
  ──────────────────────────────────────────────────────────
  livsyt.localhost                  4229  https://livsyt.localhost
```

- [ ] **Step 3: Verify `format_uptime` edge cases (quick mental check)**

| Input | Expected |
|-------|----------|
| 45 | `45s` |
| 125 | `2m 5s` |
| 7323 | `2h 2m 3s` |
| 3600 | `1h 0m 0s` |
