# Portal Alias Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add `portal alias <name> <port>` to register static HTTP routes for already-running services, matching the reference portless `alias` command.

**Architecture:** New `Alias` variant in `CliCommand`. Aliases are routes with `pid: 0` (sentinel for "no managed process"). Stale cleanup skips pid=0. Stop on an alias skips SIGTERM. Display shows `(alias)` label.

**Tech Stack:** Rust, clap 4, existing IPC/route infrastructure

---

## File Map

| File | Change |
|---|---|
| `src/cli/mod.rs` | Add `Alias` variant to `CliCommand` + handler |
| `src/routes.rs` | Guard `pid_alive_check` for pid=0 |
| `src/daemon/ipc.rs` | Guard `killpg` in Stop for pid=0 |
| `src/cli/output.rs` | Show `(alias)` for pid=0 routes in ls table |

---

## Task 1: Guard pid=0 in stale cleanup and Stop handler

**Files:**
- Modify: `src/routes.rs`
- Modify: `src/daemon/ipc.rs`

### Background

Aliases use `pid: 0` as a sentinel. Two places need to handle this:
1. `pid_alive_check(0)` — must return `true` so aliases survive `remove_stale()`. Currently on Unix, `kill(0, 0)` signals the calling process group and returns `Ok`, accidentally correct. Make it explicit.
2. `Command::Stop` — must skip `killpg` when pid is 0 (no process to kill).

- [ ] **Step 1: Write failing test in `src/routes.rs`**

Add to the `#[cfg(test)] mod tests` block in `src/routes.rs`:

```rust
#[test]
fn pid_alive_check_returns_true_for_zero_alias_sentinel() {
    assert!(pid_alive_check(0), "pid 0 (alias sentinel) should always be considered alive");
}

#[tokio::test]
async fn alias_route_survives_remove_stale() {
    let temp = TempDir::new().unwrap();
    let store = StateStore::new(temp.path().join("routes.json")).unwrap();

    // Alias route: pid=0
    store.insert(Route {
        hostname: "my-postgres.localhost".to_string(),
        port: 5432,
        public_port: None,
        protocol: RouteProtocol::Http,
        pid: 0,
        owner_pid: 0,
        cwd: String::new(),
        created_at: Utc::now(),
    }).await.unwrap();

    // Dead route: should be removed
    store.insert(Route {
        hostname: "dead.localhost".to_string(),
        port: 4000,
        public_port: None,
        protocol: RouteProtocol::Http,
        pid: u32::MAX,
        owner_pid: u32::MAX,
        cwd: "/tmp".to_string(),
        created_at: Utc::now(),
    }).await.unwrap();

    let removed = store.remove_stale().await.unwrap();

    assert_eq!(removed.len(), 1);
    assert_eq!(removed[0].hostname, "dead.localhost");
    assert!(store.get("my-postgres.localhost").is_some(), "alias should survive stale cleanup");
}
```

- [ ] **Step 2: Run tests to verify behavior**

```bash
cargo test pid_alive_check_returns_true_for_zero 2>&1 | grep -E "^test result|FAILED|ok"
cargo test alias_route_survives 2>&1 | grep -E "^test result|FAILED|ok"
```

The `pid_alive_check(0)` test may already pass on Unix (accidental behavior of `kill(0,0)`). The `alias_route_survives` test should pass because `kill(0,0)` returns Ok. But we still need the explicit guard for correctness and portability.

- [ ] **Step 3: Add explicit pid=0 guard in `pid_alive_check`**

In `src/routes.rs`, at the very top of `pid_alive_check`, before any platform-specific code:

```rust
pub fn pid_alive_check(pid: u32) -> bool {
    // pid=0 is the alias sentinel — aliases are never stale
    if pid == 0 {
        return true;
    }
    #[cfg(unix)]
    {
        // ... existing code ...
    }
```

- [ ] **Step 4: Guard killpg in Stop handler**

In `src/daemon/ipc.rs`, in the `Command::Stop` handler, wrap the `killpg` call:

Find:
```rust
                Some(route) => {
                    #[cfg(unix)]
                    {
                        use nix::sys::signal::{killpg, Signal};
                        use nix::unistd::Pid;
                        killpg(Pid::from_raw(route.pid as i32), Signal::SIGTERM).ok();
                    }
```

Replace with:
```rust
                Some(route) => {
                    #[cfg(unix)]
                    if route.pid != 0 {
                        use nix::sys::signal::{killpg, Signal};
                        use nix::unistd::Pid;
                        killpg(Pid::from_raw(route.pid as i32), Signal::SIGTERM).ok();
                    }
```

- [ ] **Step 5: Run all tests**

```bash
cargo test 2>&1 | grep -E "^test result|FAILED"
```

Expected: all tests pass.

- [ ] **Step 6: Commit**

```bash
git add src/routes.rs src/daemon/ipc.rs
git commit -m "fix: guard pid=0 alias sentinel in stale cleanup and Stop handler"
```

---

## Task 2: Add `Alias` CLI command

**Files:**
- Modify: `src/cli/mod.rs`

### Background

Add an `Alias` variant to `CliCommand` with clap args. The handler resolves the hostname, checks for existing routes (unless `--force`), and sends `RegisterRoute` IPC with `pid: 0`. The `--remove` flag sends `Rm` IPC instead.

The handler needs the daemon running for IPC, so it calls `ensure_daemon_running` first.

- [ ] **Step 1: Add `Alias` variant to `CliCommand`**

In `src/cli/mod.rs`, add to the `CliCommand` enum (after `Rm`):

```rust
    /// Register a static route for an already-running service
    Alias {
        /// App name (becomes <name>.localhost)
        name: String,
        /// Port the service is listening on
        #[arg(required_unless_present = "remove")]
        port: Option<u16>,
        /// Overwrite an existing route
        #[arg(long)]
        force: bool,
        /// Remove the alias instead of creating it
        #[arg(long)]
        remove: bool,
    },
```

- [ ] **Step 2: Add the handler in the `match cli.command` block**

Add this arm in the `pub async fn run(cli: Cli) -> Result<()>` match block (before `CliCommand::Init`):

```rust
        CliCommand::Alias { name, port, force, remove } => {
            let cwd = std::env::current_dir()?;
            let config = crate::config::Config::load(&cwd)?;
            let mut setup = banner::SetupPrinter::quiet();
            ensure_daemon_running(&config, &mut setup, DaemonRequirement::Any).await?;

            let hostname = format!("{}.{}", crate::detect::sanitize_hostname(&name), config.proxy.tld);

            if remove {
                let mut stream = ipc_connect().await?;
                write_frame(&mut stream, &Command::Rm { hostname: hostname.clone() }).await?;
                let resp: crate::proto::Response = read_frame(&mut stream).await?;
                if !resp.ok {
                    let msg = resp.error.as_deref().unwrap_or("unknown error");
                    eprintln!("{} {msg}", console::style("error:").red());
                    std::process::exit(1);
                }
                println!("{} Removed alias: {}", console::style("✓").green(), hostname);
                return Ok(());
            }

            let port = port.expect("port is required when not removing");

            // Check for existing route (unless --force)
            if !force {
                let mut stream = ipc_connect().await?;
                write_frame(&mut stream, &Command::Ls).await?;
                let resp: crate::proto::Response = read_frame(&mut stream).await?;
                if let Some(serde_json::Value::Array(routes)) = resp.data {
                    if routes.iter().any(|r| r["hostname"].as_str() == Some(&hostname)) {
                        eprintln!(
                            "{} Route already exists for {}. Use --force to overwrite.",
                            console::style("error:").red(),
                            hostname
                        );
                        std::process::exit(1);
                    }
                }
            }

            let mut stream = ipc_connect().await?;
            write_frame(
                &mut stream,
                &Command::RegisterRoute {
                    hostname: hostname.clone(),
                    port,
                    public_port: None,
                    protocol: crate::routes::RouteProtocol::Http,
                    pid: 0,
                    cwd: String::new(),
                },
            ).await?;
            let resp: crate::proto::Response = read_frame(&mut stream).await?;
            if !resp.ok {
                let msg = resp.error.as_deref().unwrap_or("unknown error");
                eprintln!("{} {msg}", console::style("error:").red());
                std::process::exit(1);
            }

            let url = build_public_url(&config, &hostname);
            println!(
                "{} {} → localhost:{}",
                console::style("✓").green(),
                console::style(&url).bold(),
                port
            );
        }
```

- [ ] **Step 3: Build and verify**

```bash
cargo build 2>&1 | grep "^error" | head -10
```

Expected: no errors.

- [ ] **Step 4: Run all tests**

```bash
cargo test 2>&1 | grep -E "^test result|FAILED"
```

Expected: all tests pass.

- [ ] **Step 5: Commit**

```bash
git add src/cli/mod.rs
git commit -m "feat(cli): add portal alias command for static route registration"
```

---

## Task 3: Show `(alias)` label in `portal ls`

**Files:**
- Modify: `src/cli/output.rs`

### Background

In `print_routes_table`, routes with `pid == 0` should show `(alias)` instead of just the pid column. Currently the table shows `HOSTNAME`, `PROTO`, `BACKEND`, `TARGET`. We need to add a `STATUS` column that shows the pid or `(alias)`.

Actually, looking at the current table, there's no pid column — it shows `HOSTNAME`, `PROTO`, `BACKEND` (port), `TARGET` (URL). The simplest change: append `(alias)` to the target column for pid=0 routes.

- [ ] **Step 1: Modify `print_routes_table` in `src/cli/output.rs`**

In the route rendering loop inside `print_routes_table`, after getting the `target` and `target_styled`, add an alias indicator:

Find the line:
```rust
        let target_styled = style(target).bold().white().to_string();
```

Replace with:
```rust
        let pid = route["pid"].as_u64().unwrap_or(1);
        let target_styled = if pid == 0 {
            format!("{}  {}", style(target).bold().white(), style("(alias)").dim())
        } else {
            style(target).bold().white().to_string()
        };
```

- [ ] **Step 2: Build and verify**

```bash
cargo build 2>&1 | grep "^error"
```

Expected: no errors.

- [ ] **Step 3: Run all tests**

```bash
cargo test 2>&1 | grep -E "^test result|FAILED"
```

Expected: all tests pass.

- [ ] **Step 4: Commit**

```bash
git add src/cli/output.rs
git commit -m "feat(output): show (alias) label for static routes in portal ls"
```

---

## Self-Review

**Spec coverage:**

- ✅ `portal alias <name> <port>` — Task 2 CLI handler
- ✅ `portal alias <name> <port> --force` — Task 2 checks existing route, skips with `--force`
- ✅ `portal alias --remove <name>` — Task 2 sends `Rm` IPC
- ✅ Route registered with `pid: 0` — Task 2 `RegisterRoute` with `pid: 0`
- ✅ Stale cleanup skips pid=0 — Task 1 `pid_alive_check` guard + test
- ✅ Alias survives `remove_stale` — Task 1 `alias_route_survives_remove_stale` test
- ✅ Stop skips SIGTERM for pid=0 — Task 1 `killpg` guard
- ✅ `portal ls` shows `(alias)` — Task 3 output modification

**No placeholders found.**

**Type consistency:** `pid: 0` used consistently across registration (Task 2), stale check (Task 1), stop guard (Task 1), and display (Task 3).
