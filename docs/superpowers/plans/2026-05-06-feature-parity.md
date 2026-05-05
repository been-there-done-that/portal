# Feature Parity Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Close the feature gap between the Rust `portal` CLI (v0.1.0) and the reference JS `portless` package (v0.12.0).

**Architecture:** Groups 1+2 (Tasks 1–9) add new IPC commands, CLI flags, and config fields to the existing single-binary architecture. Group 3 (Tasks 10–13) adds two new source files (`workspace.rs`, `lan.rs`) and wires them into the `Start` command and daemon. Tasks 1–9 are fully independent of Tasks 10–13 and can be shipped first.

**Tech Stack:** Rust, Tokio, Hyper 1.x, clap 4 (derive), serde_json/toml/serde_yaml, nix (Unix signals), dialoguer (prompts), serde_yaml (workspace YAML).

---

## File Map

| File | Action | What changes |
|------|--------|-------------|
| `src/proto.rs` | Modify | Add `GetUrl`, `Prune` variants; add `force: bool` to `Run` |
| `src/cli/mod.rs` | Modify | Add `Get`, `Prune`, `Clean` commands; `--force` on `Run`; `--lan` on `Run`+`Daemon`; wire workspace + LAN; rename `PORTAL_URL`; expose `parse_command_line` |
| `src/daemon/ipc.rs` | Modify | Handle `GetUrl`, `Prune` dispatch |
| `src/config.rs` | Modify | Add `wildcard`, `lan`, `lan_ip` to `ProxyConfig`; add `proxy: Option<bool>` to `ProjectConfig` |
| `src/proxy.rs` | Modify | Wildcard fallback in `handle_https_request`; add `wildcard: bool` param |
| `src/process.rs` | Modify | Add `is_build_only()`; rename `PORTAL_URL` → `PORTLESS_URL` |
| `src/daemon/mod.rs` | Modify | Pass `wildcard` to `serve_https`; pass LAN config to route registration |
| `src/certs.rs` | Modify | Add `untrust_system_ca()` used by `portal clean` |
| `src/workspace.rs` | Create | Workspace discovery, glob expansion, turbo detection |
| `src/lan.rs` | Create | LAN IP detection, mDNS publish/unpublish |
| `src/lib.rs` | Modify | Export `workspace` and `lan` modules |

---

## Task 1: Rename `PORTAL_URL` → `PORTLESS_URL`

**Files:**
- Modify: `src/process.rs`
- Modify: `src/cli/mod.rs`

- [ ] **Step 1: Write the failing test in `src/process.rs`**

Add inside the existing `#[cfg(test)]` block:

```rust
#[test]
fn portless_url_env_var_name_is_correct() {
    // Ensures the exported env var name matches the JS reference implementation
    assert_eq!(crate::process::PORTLESS_URL_ENV, "PORTLESS_URL");
}
```

- [ ] **Step 2: Run test to verify it fails**

```bash
cargo test portless_url_env_var_name_is_correct
```

Expected: FAIL — `PORTLESS_URL_ENV` not defined yet.

- [ ] **Step 3: Add the constant and rename in `src/process.rs`**

At the top of `src/process.rs` (after the `use` lines), add:

```rust
pub const PORTLESS_URL_ENV: &str = "PORTLESS_URL";
```

- [ ] **Step 4: Find and replace all `PORTAL_URL` string literals in `src/cli/mod.rs`**

In `src/cli/mod.rs` around line 669, change:
```rust
extra_env.push(("PORTAL_URL".to_string(), public_url.clone()));
```
to:
```rust
extra_env.push((crate::process::PORTLESS_URL_ENV.to_string(), public_url.clone()));
```

- [ ] **Step 5: Update existing `PORTAL_URL` test strings in `src/process.rs`**

The existing test at line ~274 references `"PORTAL_URL"`:
```rust
// Before:
"PORTAL_URL".to_string(),
// After:
crate::process::PORTLESS_URL_ENV.to_string(),
```

Apply the same change to the second occurrence (~line 313).

- [ ] **Step 6: Run all tests**

```bash
cargo test
```

Expected: All pass with 0 failures.

- [ ] **Step 7: Commit**

```bash
git add src/process.rs src/cli/mod.rs
git commit -m "fix: rename PORTAL_URL env var to PORTLESS_URL"
```

---

## Task 2: Add `portal get <name>` command

**Files:**
- Modify: `src/proto.rs`
- Modify: `src/cli/mod.rs`
- Modify: `src/daemon/ipc.rs`

- [ ] **Step 1: Write failing test for IPC serialization in `src/proto.rs`**

Add to the `#[cfg(test)]` block in `src/proto.rs`:

```rust
#[test]
fn round_trips_get_url_command() {
    let cmd = Command::GetUrl {
        hostname: "myapp.localhost".to_string(),
    };
    let json = serde_json::to_string(&cmd).expect("serialize");
    let back: Command = serde_json::from_str(&json).expect("deserialize");
    match back {
        Command::GetUrl { hostname } => assert_eq!(hostname, "myapp.localhost"),
        other => panic!("unexpected: {other:?}"),
    }
}
```

- [ ] **Step 2: Run test to verify it fails**

```bash
cargo test round_trips_get_url_command
```

Expected: FAIL — `GetUrl` variant not defined.

- [ ] **Step 3: Add `GetUrl` and `Prune` to `Command` in `src/proto.rs`**

In the `Command` enum, add after `HostsClean`:

```rust
/// Get the public URL for a named service
GetUrl { hostname: String },
/// Find and kill orphaned dev server processes left by crashed CLI sessions
Prune,
```

- [ ] **Step 4: Run serialization test**

```bash
cargo test round_trips_get_url_command
```

Expected: PASS.

- [ ] **Step 5: Handle `GetUrl` in `src/daemon/ipc.rs`**

In the `dispatch` function, add a new match arm before the final `Command::Run { .. }` arm:

```rust
Command::GetUrl { hostname } => {
    match manager.get(&hostname) {
        None => Response::err(format!("no route for \"{hostname}\"")),
        Some(route) => {
            let url = public_url(https_enabled, &route.hostname, http_port, https_port);
            Response::ok(serde_json::json!({ "url": url }))
        }
    }
}

Command::Prune => {
    Response::ok_empty() // placeholder — implemented in Task 3
}
```

- [ ] **Step 6: Add `Get` to `CliCommand` in `src/cli/mod.rs`**

In the `CliCommand` enum, add after `Rm`:

```rust
/// Print the public URL for a named service
Get {
    /// App name (becomes <name>.<tld>) or full hostname
    name: String,
},
```

- [ ] **Step 7: Add handler in `run()` function in `src/cli/mod.rs`**

After the `CliCommand::Rm` arm, add:

```rust
CliCommand::Get { name } => {
    let cwd = std::env::current_dir()?;
    let config = crate::config::Config::load(&cwd)?;
    let hostname = if name.contains('.') {
        name.clone()
    } else {
        format!("{}.{}", name, config.proxy.tld)
    };
    let mut stream = ipc_connect().await?;
    write_frame(&mut stream, &Command::GetUrl { hostname }).await?;
    let resp: crate::proto::Response = read_frame(&mut stream).await?;
    if resp.ok {
        if let Some(url) = resp.data.as_ref().and_then(|d| d.get("url")).and_then(|u| u.as_str()) {
            println!("{url}");
        }
    } else {
        eprintln!("error: {}", resp.error.unwrap_or_default());
        std::process::exit(1);
    }
}
```

- [ ] **Step 8: Run tests**

```bash
cargo test
```

Expected: All pass.

- [ ] **Step 9: Commit**

```bash
git add src/proto.rs src/cli/mod.rs src/daemon/ipc.rs
git commit -m "feat: add portal get command to print service URL"
```

---

## Task 3: Add `portal prune` command

**Files:**
- Modify: `src/daemon/ipc.rs`
- Modify: `src/cli/mod.rs`

- [ ] **Step 1: Write failing unit test for prune logic in `src/daemon/ipc.rs`**

Add to the `#[cfg(test)]` block in `src/daemon/ipc.rs`:

```rust
#[tokio::test]
async fn prune_removes_dead_routes_keeps_aliases() {
    let dir = tempfile::tempdir().unwrap();
    let store = StateStore::new(dir.path().join("routes.json")).unwrap();
    let tcp_routes = crate::tcp::TcpRouteManager::default();
    let manager = RouteManager::new(store.clone(), tcp_routes);

    // Dead route: pid that cannot possibly be running (u32::MAX)
    store.insert(crate::routes::Route {
        hostname: "dead.localhost".to_string(),
        port: 4001,
        public_port: None,
        protocol: crate::routes::RouteProtocol::Http,
        pid: u32::MAX,
        owner_pid: u32::MAX,
        cwd: "/tmp".to_string(),
        created_at: chrono::Utc::now(),
    }).await.unwrap();

    // Alias route: pid == 0, must survive prune
    store.insert(crate::routes::Route {
        hostname: "alias.localhost".to_string(),
        port: 4002,
        public_port: None,
        protocol: crate::routes::RouteProtocol::Http,
        pid: 0,
        owner_pid: 0,
        cwd: "/tmp".to_string(),
        created_at: chrono::Utc::now(),
    }).await.unwrap();

    let response = dispatch(
        Command::Prune,
        manager.clone(),
        std::time::Instant::now(),
        DaemonMode::TcpOnly,
        dir.path().join("portal.sock"),
        dir.path().join("daemon.pid"),
        false,
        80,
        443,
    ).await;

    assert!(response.ok);
    let pruned = response.data.unwrap();
    let arr = pruned.as_array().unwrap();
    assert_eq!(arr.len(), 1);
    assert_eq!(arr[0].as_str().unwrap(), "dead.localhost");
    assert!(manager.get("alias.localhost").is_some(), "alias should survive");
    assert!(manager.get("dead.localhost").is_none(), "dead route should be pruned");
}
```

- [ ] **Step 2: Run test to verify it fails**

```bash
cargo test prune_removes_dead_routes_keeps_aliases
```

Expected: FAIL — `Prune` arm returns `ok_empty()` placeholder.

- [ ] **Step 3: Implement `Prune` dispatch in `src/daemon/ipc.rs`**

Replace the placeholder `Command::Prune` arm with:

```rust
Command::Prune => {
    let routes = manager.list();
    let mut pruned: Vec<String> = Vec::new();

    for route in routes {
        if route.hostname == "_.localhost" {
            continue;
        }
        // Alias routes (pid == 0) are intentionally persistent
        if route.pid == 0 {
            continue;
        }
        let is_dead = {
            #[cfg(unix)]
            {
                use nix::sys::signal::{kill, Signal};
                use nix::unistd::Pid;
                // kill(pid, None) returns Ok if process exists, Err if not
                kill(Pid::from_raw(route.pid as i32), None).is_err()
            }
            #[cfg(not(unix))]
            { false }
        };
        if is_dead {
            #[cfg(unix)]
            {
                use nix::sys::signal::{kill, Signal};
                use nix::unistd::Pid;
                // Best-effort SIGTERM — it's already dead but clean up group
                let _ = kill(Pid::from_raw(-(route.pid as i32)), Signal::SIGTERM);
            }
            if let Err(e) = manager.remove(&route.hostname).await {
                tracing::warn!("prune: failed to remove {}: {e}", route.hostname);
            }
            pruned.push(route.hostname);
        }
    }

    let values: Vec<serde_json::Value> = pruned
        .into_iter()
        .map(serde_json::Value::String)
        .collect();
    Response::ok(serde_json::Value::Array(values))
}
```

- [ ] **Step 4: Run the test**

```bash
cargo test prune_removes_dead_routes_keeps_aliases
```

Expected: PASS.

- [ ] **Step 5: Add `Prune` to `CliCommand` in `src/cli/mod.rs`**

In `CliCommand`, add after `Get`:

```rust
/// Find and kill orphaned dev servers left by crashed CLI sessions
Prune,
```

- [ ] **Step 6: Add handler in `run()` in `src/cli/mod.rs`**

After the `CliCommand::Get` arm:

```rust
CliCommand::Prune => {
    let mut stream = ipc_connect().await?;
    write_frame(&mut stream, &Command::Prune).await?;
    let resp: crate::proto::Response = read_frame(&mut stream).await?;
    if resp.ok {
        let pruned = resp.data
            .as_ref()
            .and_then(|d| d.as_array())
            .map(|a| a.iter().filter_map(|v| v.as_str()).collect::<Vec<_>>())
            .unwrap_or_default();
        if pruned.is_empty() {
            println!("nothing to prune");
        } else {
            for h in &pruned {
                println!("pruned {h}");
            }
        }
    } else {
        eprintln!("error: {}", resp.error.unwrap_or_default());
        std::process::exit(1);
    }
}
```

- [ ] **Step 7: Run all tests**

```bash
cargo test
```

Expected: All pass.

- [ ] **Step 8: Commit**

```bash
git add src/proto.rs src/cli/mod.rs src/daemon/ipc.rs
git commit -m "feat: add portal prune command to kill orphaned dev servers"
```

---

## Task 4: Add `portal clean` command

**Files:**
- Modify: `src/certs.rs`
- Modify: `src/cli/mod.rs`

- [ ] **Step 1: Add `untrust_system_ca()` to `src/certs.rs`**

After the `install_system_trust_impl` functions, add:

```rust
/// Remove the portal CA from the system trust store.
/// Best-effort: returns Ok even if the cert was never trusted.
pub fn untrust_system_ca() -> Result<()> {
    let ca_path = crate::config::dirs_for_state().join("certs").join("ca.crt");
    untrust_system_ca_impl(&ca_path)
}

#[cfg(target_os = "macos")]
fn untrust_system_ca_impl(ca_path: &std::path::Path) -> Result<()> {
    if !ca_path.exists() {
        return Ok(());
    }
    let status = std::process::Command::new("security")
        .args([
            "remove-trusted-cert",
            "-d",
            ca_path.to_str().ok_or_else(|| Error::Cert("invalid CA path".into()))?,
        ])
        .status()?;
    if !status.success() {
        // Not fatal — cert might already be gone
        tracing::warn!("security remove-trusted-cert exited {:?}", status.code());
    }
    Ok(())
}

#[cfg(target_os = "linux")]
fn untrust_system_ca_impl(_ca_path: &std::path::Path) -> Result<()> {
    let dest = std::path::Path::new("/usr/local/share/ca-certificates/portal-ca.crt");
    if dest.exists() {
        std::fs::remove_file(dest)?;
        let _ = std::process::Command::new("update-ca-certificates").status();
    }
    Ok(())
}

#[cfg(not(any(target_os = "macos", target_os = "linux")))]
fn untrust_system_ca_impl(_ca_path: &std::path::Path) -> Result<()> {
    Ok(()) // no-op on unsupported platforms
}
```

- [ ] **Step 2: Write a test for untrust on the current platform in `src/certs.rs`**

```rust
#[test]
fn untrust_system_ca_does_not_panic_when_cert_missing() {
    // Should be a no-op / best-effort — never panics
    let result = untrust_system_ca();
    assert!(result.is_ok());
}
```

- [ ] **Step 3: Run the test**

```bash
cargo test untrust_system_ca_does_not_panic
```

Expected: PASS.

- [ ] **Step 4: Add `Clean` to `CliCommand` in `src/cli/mod.rs`**

In the `CliCommand` enum, add after `Prune`:

```rust
/// Stop daemon, remove CA trust, and delete all portal state
Clean {
    /// Skip confirmation prompt (required in CI / non-interactive mode)
    #[arg(long, short = 'y')]
    yes: bool,
},
```

- [ ] **Step 5: Add handler in `run()` in `src/cli/mod.rs`**

After the `CliCommand::Prune` arm:

```rust
CliCommand::Clean { yes } => {
    use std::io::IsTerminal;
    let is_ci = std::env::var("CI").map(|v| !v.is_empty()).unwrap_or(false);
    let is_tty = std::io::stdin().is_terminal();

    if !yes {
        if is_ci || !is_tty {
            eprintln!("error: --yes required in non-interactive mode");
            std::process::exit(1);
        }
        use dialoguer::Confirm;
        let confirmed = Confirm::new()
            .with_prompt("This will stop the daemon and remove all portal state. Continue?")
            .default(false)
            .interact()
            .unwrap_or(false);
        if !confirmed {
            return Ok(());
        }
    }

    // 1. Try graceful shutdown (ignores error if daemon not running)
    if let Ok(mut stream) = ipc_connect().await {
        let _ = write_frame(&mut stream, &Command::Shutdown).await;
        let _: Result<crate::proto::Response, _> = read_frame(&mut stream).await;
        // Give daemon time to clean up hosts + remove socket
        tokio::time::sleep(std::time::Duration::from_millis(300)).await;
    }

    // 2. Untrust CA (best-effort)
    if let Err(e) = crate::certs::untrust_system_ca() {
        eprintln!("warning: could not untrust CA: {e}");
    }

    // 3. Remove state directory
    let state_dir = crate::config::dirs_for_state();
    if state_dir.exists() {
        if let Err(e) = std::fs::remove_dir_all(&state_dir) {
            eprintln!("warning: could not remove state dir: {e}");
        }
    }

    println!("portal state cleared");
}
```

- [ ] **Step 6: Run all tests**

```bash
cargo test
```

Expected: All pass.

- [ ] **Step 7: Commit**

```bash
git add src/certs.rs src/cli/mod.rs
git commit -m "feat: add portal clean command for full state teardown"
```

---

## Task 5: Add `--force` flag to `portal run`

**Files:**
- Modify: `src/cli/mod.rs`

The CLI-side run handler already calls `Command::Stop` before re-registering. The change is: add an error when a live route exists and `--force` is NOT set.

- [ ] **Step 1: Write the test for the force check in `src/cli/mod.rs`**

Add to the `#[cfg(test)]` block in `src/cli/mod.rs`:

```rust
#[test]
fn run_command_has_force_arg() {
    use clap::CommandFactory;
    let cmd = Cli::command();
    let run_sub = cmd.find_subcommand("run").expect("run subcommand");
    assert!(
        run_sub.get_arguments().any(|a| a.get_id() == "force"),
        "run subcommand must have --force flag"
    );
}
```

- [ ] **Step 2: Run test to verify it fails**

```bash
cargo test run_command_has_force_arg
```

Expected: FAIL.

- [ ] **Step 3: Add `force` to `CliCommand::Run` in `src/cli/mod.rs`**

Change the `Run` variant:

```rust
Run {
    #[arg(long)]
    hostname: Option<String>,
    #[arg(long)]
    port: Option<u16>,
    #[arg(long, short = 'q', help = "Suppress startup banner and running output")]
    quiet: bool,
    /// Treat as a TCP service (skip HTTPS proxy; for databases, caches, etc.)
    #[arg(long)]
    tcp: bool,
    /// Kill any existing process registered under this hostname and replace it
    #[arg(long)]
    force: bool,
    #[arg(trailing_var_arg = true, required = true)]
    args: Vec<String>,
},
```

- [ ] **Step 4: Thread `force` into `do_run` in `src/cli/mod.rs`**

Change `do_run` signature to accept `force: bool`:

```rust
async fn do_run(
    cwd: std::path::PathBuf,
    config: crate::config::Config,
    args: Vec<String>,
    hostname_override: Option<String>,
    port_override: Option<u16>,
    use_full_registry: bool,
    quiet: bool,
    tcp: bool,
    force: bool,   // new
) -> Result<()> {
```

Update the call site in `CliCommand::Run`:

```rust
CliCommand::Run { hostname, port, quiet, tcp, force, args } => {
    let cwd = std::env::current_dir()?;
    let config = crate::config::Config::load(&cwd)?;
    do_run(cwd, config, args, hostname, port, true, quiet, tcp, force).await?;
}
```

Update the call site in `CliCommand::Start`:

```rust
do_run(cwd, config, args, hostname_override, None, true, quiet, false, false).await?;
```

- [ ] **Step 5: Add conflict guard inside `do_run` in `src/cli/mod.rs`**

Immediately after `existing_route` is fetched (around where it's used in the port selection block), add this guard before the `if let Some(explicit_port) = ...` block:

```rust
// Guard: error if route is live and --force not set
if let Some(ref route) = existing_route {
    if !force && route.pid != 0 {
        let alive = {
            #[cfg(unix)]
            {
                use nix::sys::signal::{kill, Signal};
                use nix::unistd::Pid;
                kill(Pid::from_raw(route.pid as i32), None).is_ok()
            }
            #[cfg(not(unix))]
            { false }
        };
        if alive {
            eprintln!(
                "error: {} is already running (PID {}). Use --force to replace it.",
                hostname, route.pid
            );
            std::process::exit(1);
        }
    }
}
```

- [ ] **Step 6: Run all tests**

```bash
cargo test
```

Expected: All pass.

- [ ] **Step 7: Commit**

```bash
git add src/cli/mod.rs
git commit -m "feat: add --force flag to portal run to replace live routes"
```

---

## Task 6: Non-server script detection

**Files:**
- Modify: `src/process.rs`
- Modify: `src/config.rs`
- Modify: `src/cli/mod.rs`

- [ ] **Step 1: Write failing tests in `src/process.rs`**

Add to `#[cfg(test)]` block in `src/process.rs`:

```rust
#[test]
fn tsc_is_build_only() {
    assert!(is_build_only(&["tsc".to_string()]));
}

#[test]
fn tsup_is_build_only() {
    assert!(is_build_only(&["tsup".to_string(), "src/index.ts".to_string()]));
}

#[test]
fn vite_build_is_build_only() {
    assert!(is_build_only(&["vite".to_string(), "build".to_string()]));
}

#[test]
fn vite_dev_is_not_build_only() {
    assert!(!is_build_only(&["vite".to_string()]));
    assert!(!is_build_only(&["vite".to_string(), "dev".to_string()]));
}

#[test]
fn next_build_is_build_only() {
    assert!(is_build_only(&["next".to_string(), "build".to_string()]));
}

#[test]
fn next_dev_is_not_build_only() {
    assert!(!is_build_only(&["next".to_string(), "dev".to_string()]));
}

#[test]
fn node_server_is_not_build_only() {
    assert!(!is_build_only(&["node".to_string(), "server.js".to_string()]));
}

#[test]
fn empty_args_is_not_build_only() {
    assert!(!is_build_only(&[]));
}
```

- [ ] **Step 2: Run tests to verify they fail**

```bash
cargo test tsc_is_build_only
```

Expected: FAIL — `is_build_only` not defined.

- [ ] **Step 3: Implement `is_build_only` in `src/process.rs`**

Add before the `#[cfg(test)]` block:

```rust
const BUILD_ONLY_TOOLS: &[&str] = &[
    "tsc", "tsup", "esbuild", "rollup", "webpack", "parcel",
];

const BUILD_ONLY_SUBCMDS: &[(&str, &str)] = &[
    ("vite", "build"),
    ("next", "build"),
    ("bun", "build"),
    ("turbo", "build"),
    ("nuxt", "build"),
    ("astro", "build"),
    ("svelte-kit", "build"),
    ("rspack", "build"),
    ("rsbuild", "build"),
];

/// Returns true when `args` represents a build-only tool that should not
/// be proxied (it produces artifacts, not a long-running server).
pub fn is_build_only(args: &[String]) -> bool {
    let Some(first) = args.first() else { return false };

    let basename = std::path::Path::new(first.as_str())
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or(first.as_str());
    // Strip Windows .cmd / .exe suffixes
    let basename = basename
        .strip_suffix(".cmd")
        .or_else(|| basename.strip_suffix(".exe"))
        .unwrap_or(basename);

    if BUILD_ONLY_TOOLS.contains(&basename) {
        return true;
    }

    if let Some(second) = args.get(1) {
        for (tool, subcmd) in BUILD_ONLY_SUBCMDS {
            if basename == *tool && second == *subcmd {
                return true;
            }
        }
    }

    false
}
```

- [ ] **Step 4: Run the tests**

```bash
cargo test tsc_is_build_only tsup_is_build_only vite_build_is_build_only vite_dev_is_not_build_only next_build_is_build_only next_dev_is_not_build_only node_server_is_not_build_only empty_args_is_not_build_only
```

Expected: All 8 PASS.

- [ ] **Step 5: Add `proxy: Option<bool>` to `ProjectConfig` in `src/config.rs`**

In `ProjectConfig`:
```rust
pub struct ProjectConfig {
    pub name: Option<String>,
    pub start_command: Option<String>,
    pub port_arg: Option<String>,
    pub host_arg: Option<String>,
    pub port_position: Option<String>,
    pub port_env: Option<String>,
    pub proxy: Option<bool>,    // new: false = build-only, None = auto-detect
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
    proxy: Option<bool>,    // new
}
```

In the `Config::load` merge section, add:
```rust
if let Some(proxy) = partial.project.proxy {
    config.project.proxy = Some(proxy);
}
```

- [ ] **Step 6: Wire build-only detection into `do_run` in `src/cli/mod.rs`**

In `do_run`, after `let args = ...` are resolved and before `let injection = ...`, add:

```rust
// Skip proxy if forced by config or if command is a known build-only tool
let is_build_only = config.project.proxy == Some(false)
    || crate::process::is_build_only(&args);

if is_build_only {
    // Run directly without registering a proxy route
    let child = crate::process::spawn_child(
        &cwd,
        &args,
        0,
        crate::detect::PortInjection::EnvOnly,
        &[],
    )
    .await?;
    let mut child = child;
    let _ = child.wait().await;
    return Ok(());
}
```

- [ ] **Step 7: Run all tests**

```bash
cargo test
```

Expected: All pass.

- [ ] **Step 8: Commit**

```bash
git add src/process.rs src/config.rs src/cli/mod.rs
git commit -m "feat: skip proxy for build-only tools (tsc, tsup, vite build, etc.)"
```

---

## Task 7: Wildcard subdomain routing

**Files:**
- Modify: `src/config.rs`
- Modify: `src/proxy.rs`
- Modify: `src/daemon/mod.rs`

- [ ] **Step 1: Write failing unit test in `src/proxy.rs`**

Add to the `#[cfg(test)]` block in `src/proxy.rs`:

```rust
#[test]
fn wildcard_parent_strips_first_label() {
    assert_eq!(wildcard_parent("tenant.myapp.localhost"), Some("myapp.localhost".to_string()));
}

#[test]
fn wildcard_parent_single_label_returns_none() {
    // "myapp.localhost" has no subdomain to strip to a parent route
    assert_eq!(wildcard_parent("myapp.localhost"), None);
}

#[test]
fn wildcard_parent_empty_returns_none() {
    assert_eq!(wildcard_parent(""), None);
}
```

- [ ] **Step 2: Run tests to verify they fail**

```bash
cargo test wildcard_parent_strips_first_label
```

Expected: FAIL.

- [ ] **Step 3: Add `wildcard_parent` helper + update `handle_https_request` in `src/proxy.rs`**

Add the helper near the top of `proxy.rs` (after the constants):

```rust
/// Strip the first DNS label from a hostname.
/// "tenant.myapp.localhost" → Some("myapp.localhost")
/// "myapp.localhost" → None (only two labels, no parent route possible)
fn wildcard_parent(hostname: &str) -> Option<String> {
    let rest = hostname.splitn(2, '.').nth(1)?;
    // Require at least one more dot in the remainder (it must itself be a FQDN)
    if rest.contains('.') {
        Some(rest.to_string())
    } else {
        None
    }
}
```

Change the signature of `handle_https_request` to accept a `wildcard` flag:

```rust
pub async fn handle_https_request(
    req: Request<Incoming>,
    routes: crate::routes::StateStore,
    inspector: Option<crate::inspector::InspectorSender>,
    wildcard: bool,
) -> Result<Response<BoxBodyType>, std::convert::Infallible> {
```

Update the route lookup block (around line 341) from:

```rust
let route = match routes.get(&hostname) {
    Some(r) => r,
    None => {
        return Ok(if accept_html {
```

to:

```rust
let route = match routes.get(&hostname).or_else(|| {
    if wildcard {
        wildcard_parent(&hostname).and_then(|parent| routes.get(&parent))
    } else {
        None
    }
}) {
    Some(r) => r,
    None => {
        return Ok(if accept_html {
```

- [ ] **Step 4: Fix compiler error — update call sites of `handle_https_request`**

In `src/daemon/mod.rs` at line ~406:

```rust
async move { crate::proxy::handle_https_request(req, r, insp, wc).await }
```

where `wc` is the wildcard flag. To pass it, update `serve_https` to accept `wildcard: bool`:

```rust
async fn serve_https(
    listener: tokio::net::TcpListener,
    cert_store: CertStore,
    routes: StateStore,
    inspector: Option<crate::inspector::InspectorSender>,
    wildcard: bool,
) {
```

And the closure:
```rust
let wc = wildcard;
async move { crate::proxy::handle_https_request(req, r, insp, wc).await }
```

At the call site for `serve_https` in `run_daemon_loop` (~line 272):

```rust
tokio::spawn(serve_https(https_listener, cs, rt, inspector.clone(), config.proxy.wildcard));
```

(The `config` variable is already in scope there — verify by looking at the function.)

- [ ] **Step 5: Add `wildcard` to `ProxyConfig` in `src/config.rs`**

```rust
pub struct ProxyConfig {
    pub tld: String,
    pub port_range: (u16, u16),
    pub https: bool,
    pub http_port: u16,
    pub https_port: u16,
    pub wildcard: bool,   // new
}

impl Default for ProxyConfig {
    fn default() -> Self {
        Self {
            tld: "localhost".to_string(),
            port_range: (4000, 4999),
            https: true,
            http_port: 80,
            https_port: 443,
            wildcard: false,   // new
        }
    }
}
```

In `PartialProxyConfig`:
```rust
struct PartialProxyConfig {
    tld: Option<String>,
    port_range: Option<(u16, u16)>,
    https: Option<bool>,
    http_port: Option<u16>,
    https_port: Option<u16>,
    wildcard: Option<bool>,   // new
}
```

In the merge block, add:
```rust
if let Some(wildcard) = partial.proxy.wildcard {
    config.proxy.wildcard = wildcard;
}
```

In the env-var override block, add:
```rust
"PORTLESS_WILDCARD" => config.proxy.wildcard = matches!(value, "1" | "true" | "yes" | "on"),
```

- [ ] **Step 6: Run the tests**

```bash
cargo test wildcard_parent
```

Expected: All 3 PASS.

```bash
cargo test
```

Expected: All pass (no regressions from signature change).

- [ ] **Step 7: Commit**

```bash
git add src/config.rs src/proxy.rs src/daemon/mod.rs
git commit -m "feat: add wildcard subdomain routing (PORTLESS_WILDCARD)"
```

---

## Task 8: Wire Groups 1+2 end-to-end build check

- [ ] **Step 1: Full build + test run**

```bash
cargo build && cargo test
```

Expected: 0 errors, 0 failures.

- [ ] **Step 2: Commit any leftover changes**

```bash
git status
git add -p   # review and stage any remaining changes
git commit -m "chore: groups 1+2 cleanup"
```

---

## Task 9: Workspace discovery (`src/workspace.rs`)

**Files:**
- Create: `src/workspace.rs`
- Modify: `src/lib.rs`
- Modify: `src/cli/mod.rs` (expose `parse_command_line`)

- [ ] **Step 1: Expose `parse_command_line` as `pub(crate)` in `src/cli/mod.rs`**

Change line 768:
```rust
fn parse_command_line(input: &str) -> Result<Vec<String>> {
```
to:
```rust
pub(crate) fn parse_command_line(input: &str) -> Result<Vec<String>> {
```

- [ ] **Step 2: Write failing tests for workspace discovery**

Create `src/workspace.rs` with the test module only first:

```rust
pub struct WorkspacePackage {
    pub dir: std::path::PathBuf,
    pub name: String,
    pub command: Vec<String>,
    pub injection: crate::detect::PortInjection,
}

pub fn find_workspace_root(_cwd: &std::path::Path) -> Option<std::path::PathBuf> {
    None // placeholder
}

pub fn discover_workspace_packages(
    _root: &std::path::Path,
    _config: &crate::config::Config,
) -> Vec<WorkspacePackage> {
    vec![] // placeholder
}

pub fn has_turbo_config(dir: &std::path::Path) -> bool {
    dir.join("turbo.json").exists()
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn finds_pnpm_workspace_root() {
        let root = TempDir::new().unwrap();
        std::fs::write(
            root.path().join("pnpm-workspace.yaml"),
            "packages:\n  - \"packages/*\"\n",
        ).unwrap();

        let found = find_workspace_root(root.path());
        assert_eq!(found, Some(root.path().to_path_buf()));
    }

    #[test]
    fn finds_npm_workspace_root() {
        let root = TempDir::new().unwrap();
        std::fs::write(
            root.path().join("package.json"),
            r#"{"name":"root","workspaces":["packages/*"]}"#,
        ).unwrap();

        let found = find_workspace_root(root.path());
        assert_eq!(found, Some(root.path().to_path_buf()));
    }

    #[test]
    fn returns_none_for_non_workspace() {
        let root = TempDir::new().unwrap();
        assert!(find_workspace_root(root.path()).is_none());
    }

    #[test]
    fn discovers_packages_from_pnpm_workspace() {
        let root = TempDir::new().unwrap();
        std::fs::write(
            root.path().join("pnpm-workspace.yaml"),
            "packages:\n  - \"apps/*\"\n",
        ).unwrap();

        let apps = root.path().join("apps");
        std::fs::create_dir_all(apps.join("web")).unwrap();
        std::fs::write(
            apps.join("web").join("package.json"),
            r#"{"name":"web","scripts":{"dev":"vite"}}"#,
        ).unwrap();
        std::fs::write(apps.join("web").join("pnpm-lock.yaml"), "").unwrap();

        let config = crate::config::Config::default();
        let pkgs = discover_workspace_packages(root.path(), &config);
        assert_eq!(pkgs.len(), 1);
        assert_eq!(pkgs[0].name, "web");
    }

    #[test]
    fn has_turbo_config_detects_turbo_json() {
        let dir = TempDir::new().unwrap();
        assert!(!has_turbo_config(dir.path()));
        std::fs::write(dir.path().join("turbo.json"), "{}").unwrap();
        assert!(has_turbo_config(dir.path()));
    }
}
```

- [ ] **Step 3: Run tests to verify they fail**

First add `pub mod workspace;` to `src/lib.rs`, then:

```bash
cargo test workspace
```

Expected: `finds_pnpm_workspace_root` and others FAIL with "placeholder returns None".

- [ ] **Step 4: Implement `find_workspace_root` and `discover_workspace_packages` in `src/workspace.rs`**

Replace the placeholder functions:

```rust
use serde::Deserialize;
use std::path::{Path, PathBuf};

pub struct WorkspacePackage {
    pub dir: PathBuf,
    pub name: String,
    pub command: Vec<String>,
    pub injection: crate::detect::PortInjection,
}

/// Walk up from `cwd` to find the first directory containing
/// `pnpm-workspace.yaml` or a `package.json` with a `"workspaces"` field.
pub fn find_workspace_root(cwd: &Path) -> Option<PathBuf> {
    let mut dir = cwd.to_path_buf();
    loop {
        if dir.join("pnpm-workspace.yaml").exists() {
            return Some(dir);
        }
        if let Ok(content) = std::fs::read_to_string(dir.join("package.json")) {
            if let Ok(val) = serde_json::from_str::<serde_json::Value>(&content) {
                if val.get("workspaces").is_some() {
                    return Some(dir);
                }
            }
        }
        match dir.parent() {
            Some(p) => dir = p.to_path_buf(),
            None => return None,
        }
    }
}

/// Discover all runnable workspace packages under `root`.
/// Packages without a detected dev command are silently skipped.
pub fn discover_workspace_packages(
    root: &Path,
    config: &crate::config::Config,
) -> Vec<WorkspacePackage> {
    let globs = workspace_globs(root);
    if globs.is_empty() {
        return vec![];
    }

    let registry = crate::detect::DriverRegistry::new(config);
    let mut packages = Vec::new();

    for glob in &globs {
        for pkg_dir in expand_glob(root, glob) {
            let Some(driver) = registry.detect(&pkg_dir) else {
                continue;
            };
            let Some(cmd_str) = driver.start_command(&pkg_dir) else {
                continue;
            };
            let Ok(args) = crate::cli::parse_command_line(&cmd_str) else {
                continue;
            };
            let name = driver
                .project_name(&pkg_dir)
                .unwrap_or_else(|| {
                    pkg_dir
                        .file_name()
                        .unwrap_or_default()
                        .to_string_lossy()
                        .into_owned()
                });
            let injection = driver.port_injection(&pkg_dir, 0);
            packages.push(WorkspacePackage {
                dir: pkg_dir,
                name,
                command: args,
                injection,
            });
        }
    }

    packages
}

pub fn has_turbo_config(dir: &Path) -> bool {
    dir.join("turbo.json").exists()
}

#[derive(Deserialize)]
struct PnpmWorkspace {
    packages: Vec<String>,
}

fn workspace_globs(root: &Path) -> Vec<String> {
    // pnpm-workspace.yaml takes priority
    if let Ok(content) = std::fs::read_to_string(root.join("pnpm-workspace.yaml")) {
        if let Ok(ws) = serde_yaml::from_str::<PnpmWorkspace>(&content) {
            return ws.packages;
        }
    }
    // package.json "workspaces"
    if let Ok(content) = std::fs::read_to_string(root.join("package.json")) {
        if let Ok(val) = serde_json::from_str::<serde_json::Value>(&content) {
            if let Some(ws) = val.get("workspaces").and_then(|v| v.as_array()) {
                return ws
                    .iter()
                    .filter_map(|v| v.as_str().map(String::from))
                    .collect();
            }
        }
    }
    vec![]
}

/// Expand a workspace glob pattern against `root`.
/// Supports `dir/*` (list immediate subdirs) and exact paths.
fn expand_glob(root: &Path, pattern: &str) -> Vec<PathBuf> {
    let clean = pattern.trim_end_matches('/');
    if let Some(prefix) = clean.strip_suffix("/*") {
        let parent = root.join(prefix);
        if let Ok(entries) = std::fs::read_dir(&parent) {
            return entries
                .filter_map(|e| e.ok())
                .map(|e| e.path())
                .filter(|p| p.is_dir())
                .collect();
        }
        return vec![];
    }
    // Exact path
    let p = root.join(clean);
    if p.is_dir() { vec![p] } else { vec![] }
}
```

- [ ] **Step 5: Run the workspace tests**

```bash
cargo test workspace
```

Expected: All PASS.

- [ ] **Step 6: Commit**

```bash
git add src/workspace.rs src/lib.rs src/cli/mod.rs
git commit -m "feat: add workspace discovery for monorepo support"
```

---

## Task 10: Monorepo orchestration in `portal start`

**Files:**
- Modify: `src/cli/mod.rs`

- [ ] **Step 1: Write a test that `Start` handler can detect monorepos**

Add to `#[cfg(test)]` in `src/cli/mod.rs`:

```rust
#[test]
fn workspace_packages_found_in_pnpm_monorepo() {
    use tempfile::TempDir;
    let root = TempDir::new().unwrap();
    std::fs::write(
        root.path().join("pnpm-workspace.yaml"),
        "packages:\n  - \"apps/*\"\n",
    ).unwrap();
    let apps = root.path().join("apps").join("web");
    std::fs::create_dir_all(&apps).unwrap();
    std::fs::write(
        apps.join("package.json"),
        r#"{"name":"web","scripts":{"dev":"vite"}}"#,
    ).unwrap();
    std::fs::write(apps.join("pnpm-lock.yaml"), "").unwrap();

    let config = crate::config::Config::default();
    let pkgs = crate::workspace::discover_workspace_packages(root.path(), &config);
    assert_eq!(pkgs.len(), 1);
}
```

- [ ] **Step 2: Run to verify it passes (depends on Task 9)**

```bash
cargo test workspace_packages_found_in_pnpm_monorepo
```

Expected: PASS (workspace module already implemented).

- [ ] **Step 3: Add `run_monorepo` function in `src/cli/mod.rs`**

Add after `do_run`:

```rust
/// Spawn all workspace packages concurrently under a single `portal start`.
/// Each package gets its own route registered via IPC.
async fn run_monorepo(
    packages: Vec<crate::workspace::WorkspacePackage>,
    root: &std::path::Path,
    config: crate::config::Config,
    quiet: bool,
) -> Result<()> {
    let mut setup = if quiet { banner::SetupPrinter::quiet() } else { banner::SetupPrinter::new() };
    ensure_daemon_running(&config, &mut setup, DaemonRequirement::Full).await?;
    ensure_cert_trusted(&mut setup).await?;
    setup.done();

    let has_turbo = crate::workspace::has_turbo_config(root);

    let mut handles = Vec::new();

    for pkg in packages {
        let pkg_config = crate::config::Config::load(&pkg.dir).unwrap_or(config.clone());
        let hostname = crate::detect::resolve_hostname(
            &pkg.dir,
            None,
            &config.proxy.tld,
        );
        let public_url = build_public_url(&config, &hostname);

        let args = if has_turbo {
            // Use turbo to run each package (respects task graph)
            let pkg_name = pkg.name.clone();
            vec!["turbo".to_string(), "run".to_string(), "dev".to_string(),
                 format!("--filter={pkg_name}")]
        } else {
            pkg.command.clone()
        };

        let port = crate::ports::find_free_port(
            config.proxy.port_range.0,
            config.proxy.port_range.1,
        )?;

        let ca_path = portal_ca_cert_path();
        let mut extra_env = vec![
            ("PORT".to_string(), port.to_string()),
            (crate::process::PORTLESS_URL_ENV.to_string(), public_url.clone()),
        ];
        if config.proxy.https && ca_path.exists() {
            extra_env.push(("NODE_EXTRA_CA_CERTS".to_string(), ca_path.to_string_lossy().into_owned()));
        }

        let label = pkg.name.clone();
        let hostname_clone = hostname.clone();
        let cwd = pkg.dir.clone();
        let injection = pkg.injection.clone();
        let config_clone = config.clone();

        let handle = tokio::spawn(async move {
            let mut child = match crate::process::spawn_child(
                &cwd, &args, port, injection, &extra_env,
            ).await {
                Ok(c) => c,
                Err(e) => {
                    eprintln!("[{label}] failed to start: {e}");
                    return;
                }
            };

            let child_pid = child.id().unwrap_or(std::process::id());
            if let Ok(mut stream) = ipc_connect().await {
                let _ = write_frame(&mut stream, &Command::RegisterRoute {
                    hostname: hostname_clone.clone(),
                    port,
                    public_port: None,
                    protocol: crate::routes::RouteProtocol::Http,
                    pid: child_pid,
                    cwd: cwd.to_string_lossy().to_string(),
                }).await;
                let _: Result<crate::proto::Response, _> = read_frame(&mut stream).await;
            }

            if !config_clone.proxy.https {
                println!("[{label}] → http://{hostname_clone}");
            } else {
                println!("[{label}] → https://{hostname_clone}");
            }

            let _ = child.wait().await;
        });

        handles.push(handle);
    }

    // Wait for all children — Ctrl-C is handled by the process group
    for h in handles {
        let _ = h.await;
    }
    Ok(())
}
```

- [ ] **Step 4: Update `CliCommand::Start` handler to try monorepo first**

Replace the existing `CliCommand::Start` arm:

```rust
CliCommand::Start { quiet } => {
    let cwd = std::env::current_dir()?;
    let config = crate::config::Config::load(&cwd)?;

    // Try monorepo: look for workspace root, discover packages
    if let Some(root) = crate::workspace::find_workspace_root(&cwd) {
        let packages = crate::workspace::discover_workspace_packages(&root, &config);
        if packages.len() > 1 {
            return run_monorepo(packages, &root, config, quiet).await;
        }
    }

    // Single-app fallback (existing behaviour)
    let registry = crate::detect::DriverRegistry::new(&config);
    let driver = match registry.detect(&cwd) {
        Some(d) => d,
        None => {
            eprintln!(
                "No supported project detected. Run `portal init` to set up this project."
            );
            std::process::exit(1);
        }
    };
    let raw_cmd = match driver.start_command(&cwd) {
        Some(cmd) => cmd,
        None => {
            eprintln!(
                "Detected {} but couldn't determine a start command. Run `portal init`.",
                driver.name()
            );
            std::process::exit(1);
        }
    };
    let hostname_override = config
        .project
        .name
        .clone()
        .or_else(|| driver.project_name(&cwd));
    let args = parse_start_command(driver.name(), &raw_cmd)?;
    do_run(cwd, config, args, hostname_override, None, true, quiet, false, false).await?;
}
```

- [ ] **Step 5: Run all tests**

```bash
cargo test
```

Expected: All pass.

- [ ] **Step 6: Commit**

```bash
git add src/cli/mod.rs
git commit -m "feat: add monorepo orchestration to portal start"
```

---

## Task 11: LAN IP detection (`src/lan.rs`)

**Files:**
- Create: `src/lan.rs`
- Modify: `src/lib.rs`

- [ ] **Step 1: Write failing tests**

Create `src/lan.rs`:

```rust
use std::net::IpAddr;

/// Detect the active LAN IP using a UDP trick: connect to an external address
/// (no packet sent) and read which local interface was chosen.
pub fn detect_lan_ip() -> Option<IpAddr> {
    None // placeholder
}

/// Spawn a background mDNS publisher for `hostname.local` → `ip`.
/// Returns the child process handle so the caller can kill it on shutdown.
pub fn publish_mdns(
    _hostname: &str,
    _ip: IpAddr,
    _port: u16,
) -> Option<std::process::Child> {
    None // placeholder
}

/// Kill a previously started mDNS publisher.
pub fn unpublish_mdns(child: &mut std::process::Child) {
    let _ = child.kill();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detect_lan_ip_returns_non_loopback_or_none() {
        match detect_lan_ip() {
            Some(ip) => assert!(!ip.is_loopback(), "LAN IP must not be loopback: {ip}"),
            None => {} // acceptable on CI / no active interface
        }
    }

    #[test]
    fn publish_mdns_does_not_panic_without_tools() {
        // Should return None gracefully if dns-sd/avahi-publish-address is absent
        // (this is a no-op assertion — just ensuring it doesn't panic/unwrap)
        let _ = publish_mdns("test", "192.168.1.1".parse().unwrap(), 80);
    }
}
```

- [ ] **Step 2: Add module to `src/lib.rs`**

```rust
pub mod lan;
```

- [ ] **Step 3: Run tests to verify they fail**

```bash
cargo test lan::tests
```

Expected: `detect_lan_ip_returns_non_loopback_or_none` FAIL (returns None always).

Actually, on CI without a network, `None` is acceptable, so let's just run to see it compiles cleanly:

```bash
cargo test lan
```

- [ ] **Step 4: Implement `detect_lan_ip` in `src/lan.rs`**

```rust
pub fn detect_lan_ip() -> Option<IpAddr> {
    // UDP trick: connect() on a UDP socket doesn't send any packet, but it
    // causes the OS to pick a source address and we can read it back.
    let socket = std::net::UdpSocket::bind("0.0.0.0:0").ok()?;
    socket.connect("8.8.8.8:80").ok()?;
    let addr = socket.local_addr().ok()?;
    let ip = addr.ip();
    if ip.is_loopback() { None } else { Some(ip) }
}
```

- [ ] **Step 5: Implement `publish_mdns` in `src/lan.rs`**

```rust
pub fn publish_mdns(
    hostname: &str,
    ip: IpAddr,
    port: u16,
) -> Option<std::process::Child> {
    #[cfg(target_os = "macos")]
    {
        // dns-sd -P registers a service with an address record.
        // Format: dns-sd -P <name> <type> <domain> <port> <host> [addr]
        std::process::Command::new("dns-sd")
            .args([
                "-P",
                hostname,
                "_http._tcp",
                "local",
                &port.to_string(),
                &format!("{hostname}.local"),
                &ip.to_string(),
            ])
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()
            .ok()
    }
    #[cfg(target_os = "linux")]
    {
        std::process::Command::new("avahi-publish-address")
            .args(["-R", &format!("{hostname}.local"), &ip.to_string()])
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()
            .ok()
    }
    #[cfg(not(any(target_os = "macos", target_os = "linux")))]
    { None }
}
```

- [ ] **Step 6: Run all tests**

```bash
cargo test
```

Expected: All pass (detect_lan_ip test accepts None on CI).

- [ ] **Step 7: Commit**

```bash
git add src/lan.rs src/lib.rs
git commit -m "feat: add LAN IP detection and mDNS publish helpers"
```

---

## Task 12: Wire LAN mode into daemon and CLI

**Files:**
- Modify: `src/config.rs`
- Modify: `src/cli/mod.rs`
- Modify: `src/daemon/mod.rs`
- Modify: `src/certs.rs`

- [ ] **Step 1: Add `lan` and `lan_ip` to `ProxyConfig` in `src/config.rs`**

```rust
pub struct ProxyConfig {
    pub tld: String,
    pub port_range: (u16, u16),
    pub https: bool,
    pub http_port: u16,
    pub https_port: u16,
    pub wildcard: bool,
    pub lan: bool,             // new
    pub lan_ip: Option<String>, // new: override auto-detected LAN IP
}

impl Default for ProxyConfig {
    fn default() -> Self {
        Self {
            tld: "localhost".to_string(),
            port_range: (4000, 4999),
            https: true,
            http_port: 80,
            https_port: 443,
            wildcard: false,
            lan: false,
            lan_ip: None,
        }
    }
}
```

In `PartialProxyConfig`:
```rust
struct PartialProxyConfig {
    tld: Option<String>,
    port_range: Option<(u16, u16)>,
    https: Option<bool>,
    http_port: Option<u16>,
    https_port: Option<u16>,
    wildcard: Option<bool>,
    lan: Option<bool>,
    lan_ip: Option<String>,
}
```

In the merge + env-var blocks:
```rust
if let Some(lan) = partial.proxy.lan {
    config.proxy.lan = lan;
}
if let Some(lan_ip) = partial.proxy.lan_ip {
    config.proxy.lan_ip = Some(lan_ip);
}
// env vars:
"PORTLESS_LAN" => config.proxy.lan = matches!(value, "1" | "true" | "yes" | "on"),
"PORTLESS_LAN_IP" => config.proxy.lan_ip = Some(value.to_string()),
```

- [ ] **Step 2: Add `--lan` and `--ip` flags to `CliCommand::Run` in `src/cli/mod.rs`**

```rust
Run {
    #[arg(long)]
    hostname: Option<String>,
    #[arg(long)]
    port: Option<u16>,
    #[arg(long, short = 'q')]
    quiet: bool,
    #[arg(long)]
    tcp: bool,
    #[arg(long)]
    force: bool,
    /// Expose this app to the local network via mDNS .local hostname
    #[arg(long)]
    lan: bool,
    /// Override the auto-detected LAN IP (e.g. for VPN setups)
    #[arg(long, value_name = "ADDR")]
    ip: Option<String>,
    #[arg(trailing_var_arg = true, required = true)]
    args: Vec<String>,
},
```

Update the `Run` handler to merge `lan`/`ip` into config before calling `do_run`:

```rust
CliCommand::Run { hostname, port, quiet, tcp, force, lan, ip, args } => {
    let cwd = std::env::current_dir()?;
    let mut config = crate::config::Config::load(&cwd)?;
    if lan { config.proxy.lan = true; }
    if let Some(addr) = ip { config.proxy.lan_ip = Some(addr); }
    do_run(cwd, config, args, hostname, port, true, quiet, tcp, force).await?;
}
```

- [ ] **Step 3: Add LAN IP as extra SAN in cert generation in `src/certs.rs`**

`CertStore::cert_for_host` generates the cert. Find where SANs are built (it uses `rcgen`). Add the LAN IP as an additional IP SAN when `PORTLESS_LAN_IP` is set.

Add a helper:
```rust
pub fn lan_ip_san() -> Option<std::net::IpAddr> {
    std::env::var("PORTLESS_LAN_IP")
        .ok()
        .and_then(|s| s.parse().ok())
}
```

In `cert_for_host`, after the line `let mut host_params = CertificateParams::new(vec![hostname.to_string()])`:

```rust
// Add LAN IP as a SAN so HTTPS works from other devices when --lan is active
if let Some(ip) = lan_ip_san() {
    host_params.subject_alt_names.push(rcgen::SanType::IpAddress(ip));
}
```

- [ ] **Step 4: Wire LAN publishing into daemon route registration in `src/daemon/mod.rs`**

The daemon receives `RegisterRoute` IPC calls when a new app starts. In `run_daemon_loop`, after the `IpcServer` is created but before `.serve()`, store the LAN config in a shared state so the IPC server can access it.

The simplest approach: read LAN config from env vars in the IPC handler for `RegisterRoute`. In `src/daemon/ipc.rs`, the `RegisterRoute` arm already calls `manager.insert(route).await`. After a successful insert, add:

```rust
// Publish mDNS if LAN mode is active
let lan_enabled = std::env::var("PORTLESS_LAN")
    .map(|v| matches!(v.as_str(), "1" | "true" | "yes" | "on"))
    .unwrap_or(false);
if lan_enabled && protocol == RouteProtocol::Http {
    let ip_str = std::env::var("PORTLESS_LAN_IP").ok();
    let ip = ip_str
        .as_deref()
        .and_then(|s| s.parse().ok())
        .or_else(crate::lan::detect_lan_ip);
    if let Some(ip) = ip {
        // Spawn mDNS publisher (fire-and-forget — pid not tracked here)
        let _ = crate::lan::publish_mdns(&hostname, ip, http_port);
    }
}
```

- [ ] **Step 5: Print LAN URL in `do_run` banner in `src/cli/mod.rs`**

After the existing `banner::print_banner(...)` call for the HTTP case, add:

```rust
if config.proxy.lan {
    let ip = config.proxy.lan_ip.as_deref()
        .and_then(|s| s.parse::<std::net::IpAddr>().ok())
        .or_else(crate::lan::detect_lan_ip);
    if let Some(ip) = ip {
        println!("  LAN: https://{ip}  (from other devices on your network)");
    }
}
```

- [ ] **Step 6: Run all tests**

```bash
cargo test
```

Expected: All pass.

- [ ] **Step 7: Commit**

```bash
git add src/config.rs src/cli/mod.rs src/daemon/mod.rs src/daemon/ipc.rs src/certs.rs
git commit -m "feat: add LAN mode (--lan flag, mDNS publish, LAN IP in cert SANs)"
```

---

## Task 13: Final build verification and cleanup

- [ ] **Step 1: Full build in release mode**

```bash
cargo build --release
```

Expected: 0 errors, 0 warnings (or only pre-existing warnings).

- [ ] **Step 2: Full test suite**

```bash
cargo test
```

Expected: All pass.

- [ ] **Step 3: Check for any `PORTAL_URL` string literals that weren't renamed**

```bash
grep -rn '"PORTAL_URL"' src/
```

Expected: 0 results.

- [ ] **Step 4: Commit final cleanup**

```bash
git add -p
git commit -m "chore: feature parity cleanup and release build verification"
```

- [ ] **Step 5: Push branch and open PR**

```bash
git push -u origin feature/parity-fixes
gh pr create --title "feat: feature parity v0.2.0 (get, prune, clean, force, wildcard, monorepo, LAN)" \
  --body "$(cat <<'EOF'
## Summary
- `portal get <name>` — print service URL for shell composition
- `portal prune` — kill orphaned dev servers from dead CLI sessions
- `portal clean` — full teardown (daemon + CA + state)
- `--force` on `portal run` — replace a live route instead of erroring
- Non-server script detection — tsc/tsup/esbuild/etc skip the proxy
- `PORTAL_URL` → `PORTLESS_URL` env var rename
- Wildcard subdomain routing (`PORTLESS_WILDCARD=1` / `portal.toml`)
- Monorepo orchestration — `portal start` in a workspace root starts all packages
- Turborepo integration — delegates to `turbo run dev --filter=<pkg>` when `turbo.json` present
- LAN mode (`--lan`) — mDNS publish, LAN IP in cert SANs, prints LAN URL

## Test plan
- [ ] `cargo test` passes
- [ ] `portal get myapp` prints URL
- [ ] `portal prune` reports orphaned routes
- [ ] `portal clean --yes` removes ~/.portal
- [ ] `portal run --force npm run dev` replaces existing route
- [ ] `portal run tsc` exits without proxy
- [ ] `PORTLESS_URL` received by child process
- [ ] `PORTLESS_WILDCARD=1 portal run ...` then `curl tenant.myapp.localhost` proxies correctly
- [ ] `portal start` in a pnpm monorepo starts all packages

🤖 Generated with [Claude Code](https://claude.com/claude-code)
EOF
)"
```
