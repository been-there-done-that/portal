# Group A — Runtime Improvements Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Fix 7 real-world usability issues: env var injection (custom names + Node CA trust), quiet mode, content-type-aware proxy errors, parallel write safety via `StateStore`, ngrok compatibility, and Bun HMR WebSocket fix.

**Architecture:** Targeted changes across 6 files. Biggest structural change is `RouteStore` → `StateStore` which moves hosts-sync into write methods under a tokio Mutex. All other changes are additive or surgical one-location edits.

**Tech Stack:** Rust, tokio (full features), dashmap 6, hyper 1, clap 4 derive

---

## File Map

| File | Change |
|---|---|
| `src/process.rs` | Remove `hostname` param, add `extra_env: &[(String, String)]` param |
| `src/config.rs` | Add `port_env: Option<String>` to `ProjectConfig` / `PartialProjectConfig` |
| `src/cli/mod.rs` | Add `--quiet` to `Run`/`Start`; build `extra_env` in `do_run`; inject `NODE_EXTRA_CA_CERTS` |
| `src/cli/banner.rs` | Add `SetupPrinter::quiet()` constructor; no-op all methods when quiet |
| `src/routes.rs` | Replace `RouteStore` with `StateStore`; write methods become `async`; hosts-sync moved in |
| `src/daemon/ipc.rs` | Update type to `StateStore`; await write calls; delete `sync_hosts` helper |
| `src/daemon/mod.rs` | Construct `StateStore`; await inspector `insert` call |
| `src/proxy.rs` | Content-type-aware errors; `X-Forwarded-Host` fallback; WebSocket upgrade forwarding |

---

## Task 1: `spawn_child` refactor — remove `hostname`, add `extra_env`

**Files:**
- Modify: `src/process.rs`
- Modify: `src/config.rs`

### Background
`spawn_child` currently hardcodes `PORT` and `PORTAL_URL` env vars and takes a `hostname` param only used for `PORTAL_URL`. Removing it and letting callers build the env list enables custom `port_env` names (#59) and `NODE_EXTRA_CA_CERTS` injection (#218) without any new parameters.

- [ ] **Step 1: Write failing tests**

Add to `src/process.rs` `#[cfg(test)]` block:

```rust
#[tokio::test]
async fn extra_env_vars_are_passed_to_child() {
    #[cfg(unix)]
    {
        use rand::Rng;
        let random_id = rand::thread_rng().gen::<u32>();
        let test_file = format!("/tmp/portal_extra_env_{random_id}.txt");
        let args = vec![
            "sh".to_string(),
            "-c".to_string(),
            format!("echo $MY_CUSTOM_VAR > {test_file}"),
        ];
        let extra_env = vec![
            ("MY_CUSTOM_VAR".to_string(), "hello123".to_string()),
        ];
        let mut child = spawn_child(
            Path::new("/tmp"), &args, 4321,
            crate::detect::PortInjection::EnvOnly,
            &extra_env,
        ).await.expect("spawn failed");
        let _ = child.wait().await;
        let content = std::fs::read_to_string(&test_file).unwrap();
        assert_eq!(content.trim(), "hello123");
        let _ = std::fs::remove_file(&test_file);
    }
}

#[tokio::test]
async fn port_env_not_set_when_not_in_extra_env() {
    #[cfg(unix)]
    {
        use rand::Rng;
        let random_id = rand::thread_rng().gen::<u32>();
        let test_file = format!("/tmp/portal_no_port_{random_id}.txt");
        let args = vec![
            "sh".to_string(),
            "-c".to_string(),
            format!("echo \"$PORT\" > {test_file}"),
        ];
        // No PORT in extra_env
        let mut child = spawn_child(
            Path::new("/tmp"), &args, 4321,
            crate::detect::PortInjection::EnvOnly,
            &[],
        ).await.expect("spawn failed");
        let _ = child.wait().await;
        let content = std::fs::read_to_string(&test_file).unwrap();
        // PORT should be empty (not set by spawn_child itself)
        assert_eq!(content.trim(), "", "PORT should not be set automatically; got: {content}");
        let _ = std::fs::remove_file(&test_file);
    }
}
```

- [ ] **Step 2: Run to verify they fail**

```bash
cargo test extra_env_vars_are_passed extra_env_not_set 2>&1 | grep -E "FAILED|error"
```

Expected: compilation errors (new signature not yet written).

- [ ] **Step 3: Update `spawn_child` signature and body in `src/process.rs`**

Replace the entire `spawn_child` function:

```rust
/// Spawn a child dev server process.
/// Callers provide `extra_env` — all env vars to set (PORT, PORTAL_URL, NODE_EXTRA_CA_CERTS, etc.).
/// Handles PortInjection variants for framework-specific port passing.
pub async fn spawn_child(
    cwd: &Path,
    args: &[String],
    port: u16,
    injection: crate::detect::PortInjection,
    extra_env: &[(String, String)],
) -> Result<tokio::process::Child> {
    if args.is_empty() {
        return Err(crate::error::Error::Ipc("No arguments provided to spawn_child".to_string()));
    }

    let port_str = port.to_string();

    // Substitute {port} in every arg
    let args: Vec<String> = args.iter()
        .map(|a| a.replace("{port}", &port_str))
        .collect();

    let program = &args[0];
    let rest: Vec<&str> = args[1..].iter().map(String::as_str).collect();

    let mut cmd = tokio::process::Command::new(program);
    #[cfg(unix)]
    cmd.process_group(0);
    cmd.current_dir(cwd).kill_on_drop(false);

    for (k, v) in extra_env {
        cmd.env(k, v);
    }

    match injection {
        crate::detect::PortInjection::EnvOnly => {
            cmd.args(&rest);
        }
        crate::detect::PortInjection::CliArgs(ref extra) => {
            cmd.args(&rest).args(extra);
        }
        crate::detect::PortInjection::AppendAddress(ref addr) => {
            cmd.args(&rest).arg(addr);
        }
    }

    Ok(cmd.spawn()?)
}
```

- [ ] **Step 4: Fix all existing tests in `src/process.rs` that call `spawn_child`**

Every existing `spawn_child` call in the test module passes `hostname` as 4th arg and no `extra_env`. Update each one:

- Remove the `"test.localhost"` / `"myapp.localhost"` argument
- Add `&[]` as the last argument (empty extra_env)
- Add `("PORT".to_string(), "4321".to_string())` to extra_env where the test checks `$PORT`

Example — `spawns_and_kills_child` (no PORT check, just needs running):
```rust
let mut child = spawn_child(Path::new("/tmp"), &args, 4321,
    crate::detect::PortInjection::EnvOnly, &[])
    .await
    .expect("Failed to spawn child");
```

Example — `child_receives_port_env` (checks `$PORT`):
```rust
let extra_env = vec![("PORT".to_string(), "4321".to_string())];
let mut child = spawn_child(Path::new("/tmp"), &args, 4321,
    crate::detect::PortInjection::EnvOnly, &extra_env)
    .await
    .expect("Failed to spawn child");
```

Example — `child_receives_portal_url_env` (checks `$PORTAL_URL`):
```rust
let extra_env = vec![("PORTAL_URL".to_string(), "https://myapp.localhost".to_string())];
let mut child = spawn_child(
    Path::new("/tmp"), &args, 4321,
    crate::detect::PortInjection::EnvOnly, &extra_env,
).await.expect("Failed to spawn child");
```

Apply same pattern to `spawn_child_env_only_sets_port_env`, `spawn_child_cli_args_appended`, `spawn_child_uses_separate_process_group`, `stop_child_kills_entire_process_group`, `spawn_child_append_address_appended`.

- [ ] **Step 5: Add `port_env` to `src/config.rs`**

In `ProjectConfig`:
```rust
pub struct ProjectConfig {
    pub name: Option<String>,
    pub start_command: Option<String>,
    pub port_arg: Option<String>,
    pub host_arg: Option<String>,
    pub port_position: Option<String>,
    pub port_env: Option<String>,   // custom env var name for the port (default: PORT)
}
```

In `PartialProjectConfig`:
```rust
struct PartialProjectConfig {
    name: Option<String>,
    start_command: Option<String>,
    port_arg: Option<String>,
    host_arg: Option<String>,
    port_position: Option<String>,
    port_env: Option<String>,
}
```

In `apply_partial`, after the existing `port_position` block:
```rust
if partial.project.port_env.is_some() {
    config.project.port_env = partial.project.port_env;
}
```

- [ ] **Step 6: Add config test for `port_env`**

Add to `src/config.rs` test block:
```rust
#[test]
fn port_env_can_be_overridden_via_toml() {
    let temp = TempDir::new().unwrap();
    let project_path = temp.path().join("portal.toml");
    std::fs::write(&project_path, r#"
[project]
port_env = "APP_PORT"
"#).unwrap();
    let config = Config::load_with_paths(None, Some(project_path), &[]).unwrap();
    assert_eq!(config.project.port_env, Some("APP_PORT".to_string()));
}
```

- [ ] **Step 7: Run tests**

```bash
cargo test 2>&1 | grep -E "^test result|FAILED"
```

Expected: all tests pass. Fix any call sites in `src/cli/mod.rs` that still pass the old `spawn_child` signature — those are updated in Task 2.

- [ ] **Step 8: Commit**

```bash
git add src/process.rs src/config.rs
git commit -m "refactor(process): remove hostname param, add extra_env to spawn_child; add port_env config field"
```

---

## Task 2: Build `extra_env` in `do_run` — port_env + NODE_EXTRA_CA_CERTS

**Files:**
- Modify: `src/cli/mod.rs`

### Background
`do_run` is the single call site for `spawn_child`. It now builds the complete env list using `config.project.port_env` and injects `NODE_EXTRA_CA_CERTS` when the portal CA cert exists and HTTPS is on.

- [ ] **Step 1: Write failing test**

Add to `src/config.rs` (or a new integration test — add to `src/cli/mod.rs` test block if one exists):

Since this logic is in `do_run` which is hard to unit test, add a config-level unit test that verifies the env name resolution logic in isolation:

```rust
// In src/config.rs tests
#[test]
fn port_env_defaults_to_port_when_unset() {
    let config = Config::load_with_paths(None, None, &[]).unwrap();
    // When None, caller should default to "PORT"
    assert_eq!(config.project.port_env.as_deref().unwrap_or("PORT"), "PORT");
}
```

- [ ] **Step 2: Update `do_run` in `src/cli/mod.rs`**

Find the call site for `spawn_child` (around line 387). Replace the call and add env building above it:

```rust
    // Build env vars for the child process
    let port_env_name = config.project.port_env.as_deref().unwrap_or("PORT");
    let mut extra_env: Vec<(String, String)> = vec![
        (port_env_name.to_string(), port.to_string()),
        ("PORTAL_URL".to_string(), format!("https://{hostname}")),
    ];
    // Inject NODE_EXTRA_CA_CERTS so Node.js child processes trust our local CA
    if config.proxy.https {
        let ca_path = crate::config::dirs_for_state().join("ca.pem");
        if ca_path.exists() {
            extra_env.push((
                "NODE_EXTRA_CA_CERTS".to_string(),
                ca_path.to_string_lossy().into_owned(),
            ));
        }
    }

    let my_pid = std::process::id();
    let mut child = crate::process::spawn_child(
        &cwd, &args, port, injection, &extra_env,
    ).await?;
```

- [ ] **Step 3: Run tests**

```bash
cargo test 2>&1 | grep -E "^test result|FAILED"
```

Expected: all tests pass. The compiler will catch any remaining old `spawn_child` signatures.

- [ ] **Step 4: Commit**

```bash
git add src/cli/mod.rs
git commit -m "feat(cli): build extra_env in do_run — custom port_env name + NODE_EXTRA_CA_CERTS injection (#59, #218)"
```

---

## Task 3: `--quiet` flag

**Files:**
- Modify: `src/cli/mod.rs`
- Modify: `src/cli/banner.rs`

### Background
`--quiet` / `-q` suppresses the startup banner and the `SetupPrinter` first-run steps. Errors still go to stderr. Add `quiet: bool` field to `SetupPrinter` so all downstream functions need zero changes.

- [ ] **Step 1: Write failing test**

Add to `src/cli/banner.rs` test block (add the block if it doesn't exist):

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn quiet_setup_printer_does_not_panic() {
        // A quiet SetupPrinter should be a no-op — just verify it doesn't panic
        let mut setup = SetupPrinter::quiet();
        setup.plain_step("this should be silent");
        let _pb = setup.begin_step("daemon", "starting…");
        setup.done(); // should not print anything
    }
}
```

- [ ] **Step 2: Run to verify it fails**

```bash
cargo test quiet_setup_printer 2>&1 | grep -E "FAILED|error"
```

Expected: compilation error — `SetupPrinter::quiet()` doesn't exist yet.

- [ ] **Step 3: Add `quiet` field to `SetupPrinter` in `src/cli/banner.rs`**

Replace the struct definition and all impls:

```rust
pub struct SetupPrinter {
    mp: MultiProgress,
    started: bool,
    quiet: bool,
}

impl SetupPrinter {
    pub fn new() -> Self {
        Self {
            mp: MultiProgress::new(),
            started: false,
            quiet: false,
        }
    }

    /// A no-op printer — all methods silently succeed.
    pub fn quiet() -> Self {
        Self {
            mp: MultiProgress::new(),
            started: false,
            quiet: true,
        }
    }

    fn ensure_header(&mut self) {
        if self.quiet { return; }
        if !self.started {
            self.started = true;
            let version = env!("CARGO_PKG_VERSION");
            let badge = style(" portal ").bold().white().on_blue();
            let label = style(format!("v{version}  ·  first run")).dim();
            let _ = self.mp.println(format!("  {badge}  {label}"));
            let _ = self.mp.println("");
        }
    }

    pub fn begin_step(&mut self, name: &str, msg: &str) -> ProgressBar {
        if self.quiet {
            return ProgressBar::hidden();
        }
        self.ensure_header();
        let pb = self.mp.add(ProgressBar::new_spinner());
        let spinner_style = ProgressStyle::with_template("  {spinner:.cyan} {msg}")
            .expect("invalid spinner template")
            .tick_strings(&[
                "⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏",
            ]);
        pb.set_style(spinner_style);
        pb.set_message(format!("{:<8} {}", name, msg));
        pb.enable_steady_tick(std::time::Duration::from_millis(80));
        pb
    }

    pub fn plain_step(&mut self, msg: &str) {
        if self.quiet { return; }
        self.ensure_header();
        eprintln!("  {}", console::style(msg).dim());
    }

    pub fn done(self) {
        if self.quiet { return; }
        if self.started {
            eprintln!("  {}", style("╰─ ready").dim());
            eprintln!();
        }
    }
}

impl Default for SetupPrinter {
    fn default() -> Self { Self::new() }
}
```

- [ ] **Step 4: Add `quiet` to `CliCommand::Run` and `CliCommand::Start` in `src/cli/mod.rs`**

Replace the two variant definitions:

```rust
/// Auto-detect and start the best dev script from package.json
Start {
    #[arg(long, short = 'q', help = "Suppress startup banner and running output")]
    quiet: bool,
},
/// Run a dev server and assign it a .localhost URL
Run {
    #[arg(long)]
    hostname: Option<String>,
    #[arg(long)]
    port: Option<u16>,
    #[arg(long, short = 'q', help = "Suppress startup banner and running output")]
    quiet: bool,
    #[arg(trailing_var_arg = true, required = true)]
    args: Vec<String>,
},
```

- [ ] **Step 5: Update match arms for `Start` and `Run` to destructure `quiet`, pass to `do_run`**

Find the `CliCommand::Start` arm (around line 85) and update — only the variant pattern and `do_run` call change:
```rust
CliCommand::Start { quiet } => {
    let cwd = std::env::current_dir()?;
    let config = crate::config::Config::load(&cwd)?;
    let registry = crate::detect::DriverRegistry::new(&config);

    let driver = match registry.detect(&cwd) {
        Some(d) => d,
        None => {
            eprintln!("No supported project detected. Run `portal init` to set up this project.");
            std::process::exit(1);
        }
    };

    let raw_cmd = match driver.start_command(&cwd) {
        Some(cmd) => cmd,
        None => {
            eprintln!("Detected {} but couldn't determine a start command. Run `portal init`.", driver.name());
            std::process::exit(1);
        }
    };

    let hostname_override = config.project.name.clone()
        .or_else(|| driver.project_name(&cwd));

    let args: Vec<String> = raw_cmd
        .split_whitespace()
        .map(String::from)
        .collect();

    do_run(cwd, config, args, hostname_override, None, true, quiet).await?;
}
```

Find the `CliCommand::Run` arm and update:
```rust
CliCommand::Run { hostname, port, quiet, args } => {
    let cwd = std::env::current_dir()?;
    let config = crate::config::Config::load(&cwd)?;
    do_run(cwd, config, args, hostname, port, false, quiet).await?;
}
```

- [ ] **Step 6: Update `do_run` signature and guard banner + setup output**

Change signature:
```rust
async fn do_run(
    cwd: std::path::PathBuf,
    config: crate::config::Config,
    args: Vec<String>,
    hostname_override: Option<String>,
    port_override: Option<u16>,
    use_full_registry: bool,
    quiet: bool,
) -> Result<()> {
```

Change `SetupPrinter::new()` at the top:
```rust
let mut setup = if quiet {
    banner::SetupPrinter::quiet()
} else {
    banner::SetupPrinter::new()
};
```

Guard banner print (find `banner::print_banner` call, around line 407):
```rust
if !quiet {
    banner::print_banner(&hostname, port, child_pid, reuse_port.is_some());
}
```

- [ ] **Step 7: Run tests**

```bash
cargo test 2>&1 | grep -E "^test result|FAILED"
```

Expected: all tests pass.

- [ ] **Step 8: Commit**

```bash
git add src/cli/mod.rs src/cli/banner.rs
git commit -m "feat(cli): add --quiet flag to portal run and portal start (#163)"
```

---

## Task 4: Content-type-aware proxy error responses

**Files:**
- Modify: `src/proxy.rs`

### Background
When a proxied service is down, portal returns full HTML error pages. API clients (SSR fetch, curl) receive this HTML and log it to the terminal as hundreds of lines. If the `Accept` header doesn't include `text/html`, return a short plain-text message instead.

- [ ] **Step 1: Write failing test**

Add to `src/proxy.rs` test block:

```rust
#[test]
fn wants_html_returns_true_for_browser_accept() {
    let mut headers = http::HeaderMap::new();
    headers.insert(
        http::header::ACCEPT,
        "text/html,application/xhtml+xml,application/xml;q=0.9,*/*;q=0.8".parse().unwrap(),
    );
    assert!(wants_html(&headers));
}

#[test]
fn wants_html_returns_false_for_json_accept() {
    let mut headers = http::HeaderMap::new();
    headers.insert(http::header::ACCEPT, "application/json".parse().unwrap());
    assert!(!wants_html(&headers));
}

#[test]
fn wants_html_returns_false_when_no_accept_header() {
    let headers = http::HeaderMap::new();
    assert!(!wants_html(&headers));
}
```

- [ ] **Step 2: Run to verify they fail**

```bash
cargo test wants_html 2>&1 | grep -E "FAILED|error"
```

Expected: compile error — `wants_html` not yet defined.

- [ ] **Step 3: Add helpers to `src/proxy.rs`**

Add these two functions before `handle_https_request`:

```rust
/// Returns true if the request prefers HTML responses (i.e. a browser navigation).
fn wants_html(headers: &http::HeaderMap) -> bool {
    headers
        .get(http::header::ACCEPT)
        .and_then(|v| v.to_str().ok())
        .map(|v| v.contains("text/html"))
        .unwrap_or(false)
}

/// Short plain-text error for API callers (no Accept: text/html).
fn plain_error(status: http::StatusCode, msg: &str) -> Response<BoxBodyType> {
    Response::builder()
        .status(status)
        .header("content-type", "text/plain")
        .body(full_body(format!("{} {msg}", status.as_u16())))
        .unwrap()
}
```

- [ ] **Step 4: Update `handle_https_request` to branch on `wants_html`**

At the top of `handle_https_request`, after extracting `hops`, add:
```rust
let accept_html = wants_html(req.headers());
```

Replace the hops-exceeded block:
```rust
if hops >= MAX_HOPS {
    return Ok(if accept_html {
        Response::builder()
            .status(StatusCode::LOOP_DETECTED)
            .header("content-type", "text/html")
            .body(full_body(crate::pages::page_508(&hostname)))
            .unwrap()
    } else {
        plain_error(StatusCode::LOOP_DETECTED, &format!("loop detected proxying {hostname}"))
    });
}
```

Replace the route-not-found block:
```rust
let route = match routes.get(&hostname) {
    Some(r) => r,
    None => {
        return Ok(if accept_html {
            Response::builder()
                .status(StatusCode::NOT_FOUND)
                .header("content-type", "text/html")
                .body(full_body(crate::pages::page_404(&hostname)))
                .unwrap()
        } else {
            plain_error(StatusCode::NOT_FOUND, &format!("no route registered for {hostname}"))
        });
    }
};
```

Replace the body-collect error block (after `into_parts`):
```rust
let req_body_bytes = match body.collect().await {
    Ok(c) => c.to_bytes(),
    Err(_) => {
        return Ok(if accept_html {
            Response::builder()
                .status(StatusCode::BAD_GATEWAY)
                .header("content-type", "text/html")
                .body(full_body(crate::pages::page_502(&hostname)))
                .unwrap()
        } else {
            plain_error(StatusCode::BAD_GATEWAY, &format!("{hostname} → port {port} unreachable"))
        });
    }
};
```

Replace the upstream-error block at the end of `handle_https_request`:
```rust
Err(_) => Ok(if accept_html {
    Response::builder()
        .status(StatusCode::BAD_GATEWAY)
        .header("content-type", "text/html")
        .body(full_body(crate::pages::page_502(&hostname)))
        .unwrap()
} else {
    plain_error(StatusCode::BAD_GATEWAY, &format!("{hostname} → port {port} unreachable"))
}),
```

Note: `accept_html` is captured before `req.into_parts()`. After `into_parts()`, `parts` carries the headers — `accept_html` (a `bool`) is already evaluated so no borrow issues.

- [ ] **Step 5: Run tests**

```bash
cargo test 2>&1 | grep -E "^test result|FAILED"
```

Expected: all tests pass.

- [ ] **Step 6: Commit**

```bash
git add src/proxy.rs
git commit -m "feat(proxy): plain-text error responses for non-browser requests (#182)"
```

---

## Task 5: `StateStore` — unified write lock + hosts-sync

**Files:**
- Modify: `src/routes.rs`
- Modify: `src/daemon/ipc.rs`
- Modify: `src/daemon/mod.rs`
- Modify: `src/proxy.rs`

### Background
`RouteStore::persist()` races when multiple tokio tasks write simultaneously — last-rename-wins can overwrite with a stale snapshot. With hosts-sync also needing to stay in sync, both operations must happen under the same lock. `StateStore` wraps DashMap + write lock + hosts-sync into one type. Reads stay lock-free.

- [ ] **Step 1: Write failing tests**

Add to `src/routes.rs` test block (after the existing tests):

```rust
#[tokio::test]
async fn concurrent_inserts_no_data_loss() {
    use std::sync::Arc;
    let temp = tempfile::TempDir::new().unwrap();
    let store = Arc::new(StateStore::new(temp.path().join("routes.json")).unwrap());

    let mut handles = vec![];
    for i in 0u32..20 {
        let s = Arc::clone(&store);
        handles.push(tokio::spawn(async move {
            s.insert(crate::routes::Route {
                hostname: format!("app{i}.localhost"),
                port: 4000 + i as u16,
                pid: std::process::id(),
                owner_pid: std::process::id(),
                cwd: "/tmp".to_string(),
                created_at: chrono::Utc::now(),
            }).await.unwrap();
        }));
    }
    for h in handles { h.await.unwrap(); }

    // All 20 routes must be present — no overwrites from racing persist
    assert_eq!(store.list().len(), 20);

    // Reload from disk and verify persistence
    let store2 = StateStore::new(temp.path().join("routes.json")).unwrap();
    assert_eq!(store2.list().len(), 20);
}

#[tokio::test]
async fn state_store_remove_works() {
    let temp = tempfile::TempDir::new().unwrap();
    let store = StateStore::new(temp.path().join("routes.json")).unwrap();
    store.insert(crate::routes::Route {
        hostname: "test.localhost".to_string(),
        port: 4000,
        pid: std::process::id(),
        owner_pid: std::process::id(),
        cwd: "/tmp".to_string(),
        created_at: chrono::Utc::now(),
    }).await.unwrap();
    assert!(store.get("test.localhost").is_some());
    store.remove("test.localhost").await.unwrap();
    assert!(store.get("test.localhost").is_none());
}

#[tokio::test]
async fn state_store_remove_stale_removes_dead_pids() {
    let temp = tempfile::TempDir::new().unwrap();
    let store = StateStore::new(temp.path().join("routes.json")).unwrap();

    store.insert(crate::routes::Route {
        hostname: "alive.localhost".to_string(),
        port: 4000,
        pid: std::process::id(),
        owner_pid: std::process::id(),
        cwd: "/tmp".to_string(),
        created_at: chrono::Utc::now(),
    }).await.unwrap();

    store.insert(crate::routes::Route {
        hostname: "dead.localhost".to_string(),
        port: 4001,
        pid: u32::MAX,
        owner_pid: u32::MAX,
        cwd: "/tmp".to_string(),
        created_at: chrono::Utc::now(),
    }).await.unwrap();

    store.remove_stale().await.unwrap();

    assert!(store.get("alive.localhost").is_some());
    assert!(store.get("dead.localhost").is_none());
}
```

- [ ] **Step 2: Run to verify they fail**

```bash
cargo test concurrent_inserts state_store_remove 2>&1 | grep -E "FAILED|error"
```

Expected: compile errors — `StateStore` doesn't exist yet.

- [ ] **Step 3: Replace `RouteStore` with `StateStore` in `src/routes.rs`**

Replace the entire file content (keep the existing `Route` struct, `pid_alive_check`, and existing tests — just replace `RouteStore` struct and impl):

```rust
use crate::error::Result;
use chrono::{DateTime, Utc};
use dashmap::DashMap;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use std::sync::Arc;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Route {
    pub hostname: String,
    pub port: u16,
    pub pid: u32,
    #[serde(default)]
    pub owner_pid: u32,
    #[serde(default)]
    pub cwd: String,
    #[serde(default = "chrono::Utc::now")]
    pub created_at: DateTime<Utc>,
}

/// Thread-safe store backed by DashMap.
/// Reads (get, list) are lock-free.
/// Writes (insert, remove, remove_stale) are serialised under a tokio Mutex
/// and atomically update routes.json + /etc/hosts in one locked transaction.
#[derive(Clone)]
pub struct StateStore {
    map: Arc<DashMap<String, Route>>,
    write_lock: Arc<tokio::sync::Mutex<()>>,
    path: PathBuf,
}

impl StateStore {
    /// Create a new StateStore. Loads existing routes from disk if file exists.
    pub fn new(path: PathBuf) -> Result<Self> {
        let map = Arc::new(DashMap::new());
        if path.exists() {
            let contents = std::fs::read_to_string(&path)?;
            if !contents.is_empty() {
                let routes: Vec<Route> = serde_json::from_str(&contents)?;
                for route in routes {
                    map.insert(route.hostname.clone(), route);
                }
            }
        }
        Ok(Self {
            map,
            write_lock: Arc::new(tokio::sync::Mutex::new(())),
            path,
        })
    }

    // ── Read API (lock-free) ──────────────────────────────────────────────

    pub fn get(&self, hostname: &str) -> Option<Route> {
        self.map.get(hostname).map(|e| e.clone())
    }

    pub fn list(&self) -> Vec<Route> {
        self.map.iter().map(|e| e.value().clone()).collect()
    }

    // ── Write API (serialised) ────────────────────────────────────────────

    pub async fn insert(&self, route: Route) -> Result<()> {
        let _guard = self.write_lock.lock().await;
        self.map.insert(route.hostname.clone(), route);
        self.persist_locked()?;
        self.sync_hosts_locked();
        Ok(())
    }

    pub async fn remove(&self, hostname: &str) -> Result<()> {
        let _guard = self.write_lock.lock().await;
        self.map.remove(hostname);
        self.persist_locked()?;
        self.sync_hosts_locked();
        Ok(())
    }

    pub async fn remove_stale(&self) -> Result<()> {
        let _guard = self.write_lock.lock().await;
        let to_remove: Vec<String> = self.map.iter()
            .filter(|e| !pid_alive_check(e.value().pid))
            .map(|e| e.key().clone())
            .collect();
        if to_remove.is_empty() {
            return Ok(());
        }
        for h in &to_remove {
            self.map.remove(h);
        }
        self.persist_locked()?;
        self.sync_hosts_locked();
        Ok(())
    }

    // ── Private helpers (called while write_lock is held) ─────────────────

    fn persist_locked(&self) -> Result<()> {
        let routes: Vec<Route> = self.map.iter().map(|e| e.value().clone()).collect();
        let json = serde_json::to_string_pretty(&routes)?;
        let tmp_path = format!("{}.tmp", self.path.display());
        std::fs::write(&tmp_path, &json)?;
        std::fs::rename(&tmp_path, &self.path)?;

        #[cfg(unix)]
        if let Some((uid, gid)) = crate::config::sudo_uid_gid() {
            unsafe {
                let p = std::ffi::CString::new(self.path.to_string_lossy().as_bytes()).unwrap();
                nix::libc::chown(p.as_ptr(), uid, gid);
            }
        }
        Ok(())
    }

    fn sync_hosts_locked(&self) {
        if !crate::hosts::should_sync() {
            return;
        }
        let hostnames: Vec<String> = self.map.iter()
            .filter(|e| e.key() != "_.localhost")
            .map(|e| e.key().clone())
            .collect();
        let refs: Vec<&str> = hostnames.iter().map(|s| s.as_str()).collect();
        if let Err(e) = crate::hosts::sync_hosts_file(&refs) {
            tracing::warn!("hosts sync failed: {e}");
        }
    }
}

/// Check if a process with the given PID is alive.
/// Copy this verbatim from the existing src/routes.rs — do not change it.
pub fn pid_alive_check(pid: u32) -> bool {
    #[cfg(unix)]
    {
        use nix::sys::signal::kill;
        use nix::unistd::Pid;
        let raw = match i32::try_from(pid) {
            Ok(v) if v > 0 => v,
            _ => return false,
        };
        match kill(Pid::from_raw(raw), None) {
            Ok(_) => true,
            Err(nix::errno::Errno::EPERM) => true,
            Err(_) => false,
        }
    }

    #[cfg(windows)]
    {
        use windows_sys::Win32::Foundation::CloseHandle;
        use windows_sys::Win32::System::Threading::OpenProcess;
        use windows_sys::Win32::System::Threading::PROCESS_QUERY_INFORMATION;
        unsafe {
            let handle = OpenProcess(PROCESS_QUERY_INFORMATION, 0, pid);
            if handle.is_null() { return false; }
            let _ = CloseHandle(handle);
            true
        }
    }
}
```

Keep the existing `pid_alive_check` implementation and all existing tests. Add the three new tests from Step 1. Remove the old `RouteStore` struct entirely.

- [ ] **Step 4: Update `src/daemon/ipc.rs`**

Change import at top:
```rust
use crate::routes::StateStore;
```

Replace all `RouteStore` type references with `StateStore`. The struct field:
```rust
pub struct IpcServer {
    sock_path: PathBuf,
    pid_path: PathBuf,
    routes: StateStore,
    start_time: std::time::Instant,
    http_port: u16,
    https_port: u16,
}
```

Update `IpcServer::new` signature:
```rust
pub fn new(
    sock_path: PathBuf,
    pid_path: PathBuf,
    routes: StateStore,
    http_port: u16,
    https_port: u16,
) -> Self
```

Update `handle_connection` and `dispatch` signatures (replace `RouteStore` → `StateStore`).

**Delete** the `sync_hosts` helper function entirely (lines 127–137 in current file).

In `dispatch`, update mutating calls to use `.await`:

`Command::Ls` arm:
```rust
Command::Ls => {
    let _ = routes.remove_stale().await;
    let list: Vec<_> = routes
        .list()
        .into_iter()
        .filter(|r| r.hostname != "_.localhost")
        .collect();
    Response::ok(serde_json::to_value(&list).unwrap_or(serde_json::Value::Array(vec![])))
}
```

`Command::Stop` arm (remove `sync_hosts` call):
```rust
Command::Stop { hostname } => {
    if hostname.is_empty() {
        return Response::err("hostname required for stop");
    }
    match routes.get(&hostname) {
        None => Response::err(format!("no route for {hostname}")),
        Some(route) => {
            #[cfg(unix)]
            {
                use nix::sys::signal::{killpg, Signal};
                use nix::unistd::Pid;
                killpg(Pid::from_raw(route.pid as i32), Signal::SIGTERM).ok();
            }
            let _ = routes.remove(&hostname).await;
            Response::ok_empty()
        }
    }
}
```

`Command::Rm` arm (remove `sync_hosts` call):
```rust
Command::Rm { hostname } => {
    let _ = routes.remove(&hostname).await;
    Response::ok_empty()
}
```

`Command::RegisterRoute` arm (remove `sync_hosts` call):
```rust
Command::RegisterRoute { hostname, port, pid, cwd } => {
    let route = crate::routes::Route {
        hostname: hostname.clone(),
        port,
        pid,
        owner_pid: pid,
        cwd,
        created_at: chrono::Utc::now(),
    };
    match routes.insert(route).await {
        Ok(_) => Response::ok_empty(),
        Err(e) => Response::err(e.to_string()),
    }
}
```

Update the test in the test block that creates a `RouteStore`:
```rust
// In tests:
let store = crate::routes::StateStore::new(dir.path().join("routes.json")).unwrap();
store.insert(crate::routes::Route { ... }).await.unwrap();
// etc.
```

The `user_hostnames` function signature stays unchanged — it takes `&StateStore`. Update its parameter type:
```rust
fn user_hostnames(routes: &StateStore) -> Vec<String> {
```

- [ ] **Step 5: Update `src/daemon/mod.rs`**

Change import:
```rust
use crate::routes::StateStore;
```

Replace `RouteStore::new`:
```rust
let routes = match StateStore::new(state_dir.join("routes.json")) {
    Ok(r) => r,
    Err(e) => {
        eprintln!("portal: failed to load route store: {e}");
        return Err(e);
    }
};
```

The inspector insert (around line 153) is in an async context — add `.await`:
```rust
let _ = routes.insert(crate::routes::Route {
    hostname: "_.localhost".to_string(),
    port: insp.port,
    pid: std::process::id(),
    owner_pid: std::process::id(),
    cwd: String::new(),
    created_at: chrono::Utc::now(),
}).await;
```

Update the `serve_proxy` function's routes parameter type from `RouteStore` to `StateStore`.

- [ ] **Step 6: Update `src/proxy.rs`**

Change the `handle_https_request` signature:
```rust
pub async fn handle_https_request(
    req: Request<Incoming>,
    routes: crate::routes::StateStore,
    inspector: Option<crate::inspector::InspectorSender>,
) -> Result<Response<BoxBodyType>, std::convert::Infallible> {
```

No logic changes — just the type name. All `.get()` calls on StateStore are still sync (lock-free reads).

- [ ] **Step 7: Run tests**

```bash
cargo test 2>&1 | grep -E "^test result|FAILED"
```

Expected: all tests pass including the 3 new concurrent StateStore tests.

- [ ] **Step 8: Commit**

```bash
git add src/routes.rs src/daemon/ipc.rs src/daemon/mod.rs src/proxy.rs
git commit -m "feat(routes): StateStore — serialised writes with tokio Mutex, hosts-sync absorbed (#174)"
```

---

## Task 6: ngrok `X-Forwarded-Host` fallback

**Files:**
- Modify: `src/proxy.rs`

### Background
ngrok rewrites the `Host` header to its tunnel URL (`abc.ngrok.io`). Our proxy looks up routes by `Host`, returns 404. ngrok passes the original hostname in `X-Forwarded-Host`. After failing a Host lookup, try `X-Forwarded-Host` before returning 404.

- [ ] **Step 1: Write failing test**

Add to `src/proxy.rs` test block:

```rust
#[test]
fn extract_host_strips_port() {
    let val = http::HeaderValue::from_static("myapp.localhost:443");
    assert_eq!(extract_host(Some(&val)), "myapp.localhost");
}

#[test]
fn extract_host_returns_empty_on_none() {
    assert_eq!(extract_host(None), "");
}
```

- [ ] **Step 2: Run to verify they fail**

```bash
cargo test extract_host 2>&1 | grep -E "FAILED|error"
```

Expected: compile error — `extract_host` not defined.

- [ ] **Step 3: Add `extract_host` helper and update hostname resolution in `src/proxy.rs`**

Add the helper function (before `handle_https_request`):

```rust
/// Extract hostname from a Host or X-Forwarded-Host header value, stripping any port.
pub fn extract_host(h: Option<&http::HeaderValue>) -> String {
    h.and_then(|v| v.to_str().ok())
        .unwrap_or("")
        .split(':')
        .next()
        .unwrap_or("")
        .to_string()
}
```

Replace the hostname extraction block inside `handle_https_request` (lines 105–113):

```rust
let hostname = {
    let from_host = extract_host(req.headers().get(http::header::HOST));
    if routes.get(&from_host).is_some() {
        from_host
    } else {
        // Fallback: reverse proxies (ngrok, Cloudflare Tunnel) pass the original
        // hostname in X-Forwarded-Host when they rewrite the Host header.
        let forwarded = extract_host(req.headers().get("x-forwarded-host"));
        if !forwarded.is_empty() { forwarded } else { from_host }
    }
};
```

- [ ] **Step 4: Run tests**

```bash
cargo test 2>&1 | grep -E "^test result|FAILED"
```

Expected: all tests pass.

- [ ] **Step 5: Commit**

```bash
git add src/proxy.rs
git commit -m "feat(proxy): X-Forwarded-Host fallback for ngrok and reverse proxy compatibility (#43)"
```

---

## Task 7: WebSocket upgrade forwarding — Bun HMR fix

**Files:**
- Modify: `src/proxy.rs`

### Background
The current `handle_websocket` returns 101 to the client immediately, then does a raw TCP copy — but never sends the HTTP upgrade request to the upstream. Bun's Next.js server validates the `Host` header and rejects connections where it doesn't match `localhost:{port}`. The fix is to forward the complete HTTP upgrade request (with `Host` rewritten) to the upstream, read the 101 response, then bridge the two connections.

- [ ] **Step 1: Write failing test**

Add to `src/proxy.rs` test block:

```rust
#[tokio::test]
async fn websocket_proxy_forwards_upgrade_to_upstream() {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    // Minimal upstream that expects an HTTP upgrade request and responds with 101
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();

    tokio::spawn(async move {
        let (mut stream, _) = listener.accept().await.unwrap();
        let mut buf = vec![0u8; 1024];
        let n = stream.read(&mut buf).await.unwrap();
        let req = String::from_utf8_lossy(&buf[..n]);
        // Verify Host was rewritten to localhost:{port}
        assert!(
            req.contains(&format!("host: localhost:{port}")),
            "Host header not rewritten: {req}"
        );
        // Respond with 101
        stream.write_all(
            b"HTTP/1.1 101 Switching Protocols\r\nUpgrade: websocket\r\nConnection: Upgrade\r\n\r\n"
        ).await.unwrap();
        // Keep alive for test duration
        tokio::time::sleep(std::time::Duration::from_millis(200)).await;
    });

    // Give upstream a moment to start
    tokio::time::sleep(std::time::Duration::from_millis(10)).await;

    // Build a WebSocket upgrade request with Host: myapp.localhost (NOT localhost:{port})
    let req = http::Request::builder()
        .method("GET")
        .uri("/_next/webpack-hmr")
        .header(http::header::HOST, "myapp.localhost")
        .header(http::header::UPGRADE, "websocket")
        .header(http::header::CONNECTION, "Upgrade")
        .header("Sec-WebSocket-Key", "dGhlIHNhbXBsZSBub25jZQ==")
        .header("Sec-WebSocket-Version", "13")
        .body(hyper::body::Incoming::default())
        .unwrap();

    // handle_websocket should return 101 (not 502)
    let resp = handle_websocket(req, port).await.unwrap();
    assert_eq!(resp.status(), http::StatusCode::SWITCHING_PROTOCOLS);
}
```

Note: `hyper::body::Incoming::default()` may not be constructable in tests — if it doesn't compile, skip the body by adjusting the test to only test the upstream-received request, or use `http_body_util::Empty`.

- [ ] **Step 2: Run to verify it fails**

```bash
cargo test websocket_proxy_forwards 2>&1 | grep -E "FAILED|error"
```

Expected: test fails (upstream receives no HTTP request with current implementation).

- [ ] **Step 3: Rewrite `handle_websocket` in `src/proxy.rs`**

Replace the entire `handle_websocket` function:

```rust
/// Handle a WebSocket upgrade by forwarding the full HTTP upgrade request to upstream,
/// verifying the 101 response, then bridging connections bidirectionally.
/// Host header is rewritten to localhost:{route_port} (required by Bun/Next.js HMR).
async fn handle_websocket(
    req: Request<Incoming>,
    route_port: u16,
) -> Result<Response<BoxBodyType>, std::convert::Infallible> {
    use tokio::io::AsyncWriteExt;

    // 1. Connect to upstream
    let upstream_addr = format!("127.0.0.1:{route_port}");
    let mut upstream = match TcpStream::connect(&upstream_addr).await {
        Ok(s) => s,
        Err(_) => {
            return Ok(Response::builder()
                .status(StatusCode::BAD_GATEWAY)
                .header("content-type", "text/plain")
                .body(full_body("502 Bad Gateway: upstream connection failed"))
                .unwrap());
        }
    };

    // 2. Build and forward HTTP upgrade request with Host rewritten
    let path = req
        .uri()
        .path_and_query()
        .map(|p| p.as_str())
        .unwrap_or("/")
        .to_string();
    let method = req.method().as_str().to_string();

    let mut upgrade_req = format!("{method} {path} HTTP/1.1\r\n");
    upgrade_req.push_str(&format!("host: localhost:{route_port}\r\n"));
    for (k, v) in req.headers() {
        if k == http::header::HOST {
            continue; // already wrote rewritten Host above
        }
        if let Ok(v_str) = v.to_str() {
            upgrade_req.push_str(&format!("{}: {v_str}\r\n", k.as_str()));
        }
    }
    upgrade_req.push_str("\r\n");

    if upstream.write_all(upgrade_req.as_bytes()).await.is_err() {
        return Ok(Response::builder()
            .status(StatusCode::BAD_GATEWAY)
            .header("content-type", "text/plain")
            .body(full_body("502 Bad Gateway: upstream write failed"))
            .unwrap());
    }

    // 3. Read upstream's 101 response (dev servers respond quickly on localhost)
    let mut buf = vec![0u8; 1024];
    let n = match upstream.read(&mut buf).await {
        Ok(n) if n > 0 => n,
        _ => {
            return Ok(Response::builder()
                .status(StatusCode::BAD_GATEWAY)
                .header("content-type", "text/plain")
                .body(full_body("502 Bad Gateway: upstream did not respond to WebSocket upgrade"))
                .unwrap());
        }
    };

    let response_head = String::from_utf8_lossy(&buf[..n]);
    if !response_head.contains("101") {
        return Ok(Response::builder()
            .status(StatusCode::BAD_GATEWAY)
            .header("content-type", "text/plain")
            .body(full_body("502 Bad Gateway: upstream rejected WebSocket upgrade"))
            .unwrap());
    }

    // 4. Return 101 to client and bridge the two connections
    let resp = Response::builder()
        .status(StatusCode::SWITCHING_PROTOCOLS)
        .header(http::header::UPGRADE, "websocket")
        .header(http::header::CONNECTION, "Upgrade")
        .body(full_body(""))
        .unwrap();

    tokio::spawn(async move {
        match hyper::upgrade::on(req).await {
            Ok(upgraded) => {
                let mut client_io = hyper_util::rt::TokioIo::new(upgraded);
                let _ = tokio::io::copy_bidirectional(&mut client_io, &mut upstream).await;
            }
            Err(e) => {
                tracing::warn!("WebSocket upgrade failed: {e}");
            }
        }
    });

    Ok(resp)
}
```

Note: `read` on `buf` was already imported as `use tokio::io::AsyncReadExt`. This is already in scope in proxy.rs.

- [ ] **Step 4: Run tests**

```bash
cargo test 2>&1 | grep -E "^test result|FAILED"
```

Expected: all tests pass. The new WebSocket test may be hard to fully run in CI (needs real hyper upgrade) — if it requires a real HTTP/1.1 handshake, mark `#[ignore]` and note manual test steps.

- [ ] **Step 5: Commit**

```bash
git add src/proxy.rs
git commit -m "fix(proxy): forward HTTP upgrade request to upstream with Host rewrite; fixes Bun HMR (#64)"
```

---

## Self-Review Checklist

- [x] **#218**: `NODE_EXTRA_CA_CERTS` injected in Task 2 when ca.pem exists and https=true ✓
- [x] **#59**: `port_env` field in config, used in Task 2 for env var name ✓
- [x] **#163**: `--quiet` flag in Task 3, guards banner + SetupPrinter ✓
- [x] **#182**: Content-type-aware errors in Task 4, branches on `Accept: text/html` ✓
- [x] **#174**: `StateStore` in Task 5, write_lock covers DashMap + routes.json + hosts-sync ✓
- [x] **#43**: `X-Forwarded-Host` fallback in Task 6 ✓
- [x] **#64**: WebSocket upgrade forwarding with Host rewrite in Task 7 ✓
- [x] Type consistency: `StateStore` used in all 4 consumer files from Task 5 onward ✓
- [x] `spawn_child` new signature (no `hostname`, has `extra_env`) used consistently from Task 1 ✓
- [x] `do_run` quiet param added in Task 3, all callers updated ✓
