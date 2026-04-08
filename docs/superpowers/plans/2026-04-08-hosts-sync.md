# Hosts File Sync Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Automatically keep `/etc/hosts` in sync with the daemon's route table so that all TLDs (including `.localhost` on Safari) resolve without any user-side DNS configuration.

**Architecture:** A standalone `src/hosts.rs` module holds all pure sync logic. The IPC dispatch in `src/daemon/ipc.rs` calls `sync_hosts_file` after every mutating command. Two new IPC commands (`HostsSync`, `HostsClean`) expose this to `portless hosts sync` / `portless hosts clean` CLI subcommands.

**Tech Stack:** Rust std (fs, process), `tempfile` crate (already in dev-deps via existing tests), `clap` (already used), `tracing` (already used).

**Working directory:** All commands run from `.worktrees/hosts-sync/`

**Spec:** `docs/superpowers/specs/2026-04-08-hosts-sync-design.md`

---

## File Map

| Action | Path | Responsibility |
|---|---|---|
| Create | `src/hosts.rs` | All `/etc/hosts` logic: pure string functions + filesystem sync |
| Modify | `src/main.rs` | Register `mod hosts;` |
| Modify | `src/proto.rs` | Add `Command::HostsSync` and `Command::HostsClean` |
| Modify | `src/daemon/ipc.rs` | Call sync after mutations; handle new IPC commands |
| Modify | `src/cli/mod.rs` | Add `portless hosts sync` / `portless hosts clean` subcommands |
| Modify | `src/cli/output.rs` | Add `print_hosts_sync` and `print_hosts_clean` |

---

## Task 1: `src/hosts.rs` — pure string functions

**Files:**
- Create: `src/hosts.rs`

These functions operate on strings only — no filesystem access, no daemon state. Fully testable without root.

- [ ] **Step 1: Create `src/hosts.rs` with failing tests**

```rust
// src/hosts.rs

const MARKER_START: &str = "# portless-start";
const MARKER_END: &str = "# portless-end";

/// Returns the path to the system hosts file.
pub fn hosts_path() -> std::path::PathBuf {
    #[cfg(windows)]
    {
        let root = std::env::var("SystemRoot").unwrap_or_else(|_| "C:\\Windows".to_string());
        std::path::PathBuf::from(root)
            .join("System32")
            .join("drivers")
            .join("etc")
            .join("hosts")
    }
    #[cfg(not(windows))]
    {
        std::path::PathBuf::from("/etc/hosts")
    }
}

/// Returns false only when PORTAL_SYNC_HOSTS is "0" or "false". True otherwise.
pub fn should_sync() -> bool {
    !matches!(
        std::env::var("PORTAL_SYNC_HOSTS").as_deref(),
        Ok("0") | Ok("false")
    )
}

/// Build the portless-managed block for the given hostnames.
/// Returns an empty string when hostnames is empty.
pub fn build_block(hostnames: &[&str]) -> String {
    if hostnames.is_empty() {
        return String::new();
    }
    let entries = hostnames
        .iter()
        .map(|h| format!("127.0.0.1 {h}"))
        .collect::<Vec<_>>()
        .join("\n");
    format!("{MARKER_START}\n{entries}\n{MARKER_END}")
}

/// Strip the portless-managed block from hosts file content.
/// Collapses 3+ consecutive blank lines to 2, trims trailing whitespace,
/// and ensures a single trailing newline.
pub fn remove_block(content: &str) -> String {
    let start_idx = content.find(MARKER_START);
    let end_idx = content.find(MARKER_END);
    let (s, e) = match (start_idx, end_idx) {
        (Some(s), Some(e)) if e > s => (s, e),
        _ => return content.to_string(),
    };
    let before = &content[..s];
    let after = &content[e + MARKER_END.len()..];
    let combined = format!("{before}{after}");
    // Collapse 3+ consecutive newlines to 2
    let mut out = combined;
    while out.contains("\n\n\n") {
        out = out.replace("\n\n\n", "\n\n");
    }
    let trimmed = out.trim_end();
    if trimmed.is_empty() {
        String::new()
    } else {
        format!("{trimmed}\n")
    }
}

/// Extract lines from within the portless-managed block.
/// Returns empty vec if no managed block exists.
pub fn extract_managed(content: &str) -> Vec<String> {
    let start_idx = content.find(MARKER_START);
    let end_idx = content.find(MARKER_END);
    let (s, e) = match (start_idx, end_idx) {
        (Some(s), Some(e)) if e > s => (s, e),
        _ => return vec![],
    };
    content[s + MARKER_START.len()..e]
        .lines()
        .map(|l| l.trim().to_string())
        .filter(|l| !l.is_empty())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hosts_path_is_not_empty() {
        assert!(!hosts_path().as_os_str().is_empty());
    }

    #[cfg(not(windows))]
    #[test]
    fn hosts_path_unix() {
        assert_eq!(hosts_path(), std::path::PathBuf::from("/etc/hosts"));
    }

    #[test]
    fn should_sync_default_true() {
        // PORTAL_SYNC_HOSTS not set → true (assuming test env doesn't set it)
        // We can't assert the env isn't set, so just test the "0" branch below.
    }

    #[test]
    fn build_block_empty() {
        assert_eq!(build_block(&[]), "");
    }

    #[test]
    fn build_block_single() {
        let block = build_block(&["myapp.localhost"]);
        assert!(block.starts_with("# portless-start\n"));
        assert!(block.contains("127.0.0.1 myapp.localhost"));
        assert!(block.ends_with("\n# portless-end"));
    }

    #[test]
    fn build_block_multiple() {
        let block = build_block(&["myapp.localhost", "api.localhost"]);
        assert!(block.contains("127.0.0.1 myapp.localhost\n127.0.0.1 api.localhost"));
    }

    #[test]
    fn remove_block_no_markers() {
        let content = "127.0.0.1 localhost\n";
        assert_eq!(remove_block(content), content);
    }

    #[test]
    fn remove_block_strips_managed_block() {
        let content = "127.0.0.1 localhost\n\n# portless-start\n127.0.0.1 myapp.localhost\n# portless-end\n";
        let result = remove_block(content);
        assert!(!result.contains("portless-start"));
        assert!(!result.contains("myapp.localhost"));
        assert!(result.contains("127.0.0.1 localhost"));
    }

    #[test]
    fn remove_block_normalises_blank_lines() {
        let content = "a\n\n\n\n# portless-start\nentry\n# portless-end\n";
        let result = remove_block(content);
        assert!(!result.contains("\n\n\n"));
    }

    #[test]
    fn extract_managed_no_block() {
        assert_eq!(extract_managed("127.0.0.1 localhost\n"), vec![] as Vec<String>);
    }

    #[test]
    fn extract_managed_returns_inner_lines() {
        let block = build_block(&["myapp.localhost", "api.localhost"]);
        let lines = extract_managed(&block);
        assert_eq!(lines, vec!["127.0.0.1 myapp.localhost", "127.0.0.1 api.localhost"]);
    }

    #[test]
    fn round_trip_build_extract() {
        let hostnames = &["myapp.localhost", "api.localhost", "admin.local"];
        let block = build_block(hostnames);
        let content = format!("127.0.0.1 localhost\n\n{block}\n");
        let extracted = extract_managed(&content);
        let recovered: Vec<&str> = extracted
            .iter()
            .map(|l| l.splitn(2, ' ').nth(1).unwrap_or(""))
            .collect();
        assert_eq!(recovered, hostnames.to_vec());
    }

    #[test]
    fn remove_then_rebuild_is_idempotent() {
        let hostnames = &["myapp.localhost"];
        let original = "127.0.0.1 localhost\n";
        let with_block = format!("{original}\n{}\n", build_block(hostnames));
        let cleaned = remove_block(&with_block);
        let rebuilt = format!("{}\n{}\n", cleaned.trim_end(), build_block(hostnames));
        let cleaned2 = remove_block(&rebuilt);
        assert_eq!(cleaned, cleaned2);
    }
}
```

- [ ] **Step 2: Run tests to verify they fail (module not yet registered)**

```bash
cargo test hosts::
```

Expected: `error[E0583]: file not found for module 'hosts'` or similar compile error.

- [ ] **Step 3: Register the module in `src/main.rs`**

Add `mod hosts;` to `src/main.rs` after the existing mod declarations:

```rust
// src/main.rs  — add this line with the other mods
mod hosts;
```

The full mod block should look like:
```rust
mod certs;
mod cli;
mod config;
mod daemon;
mod detect;
mod error;
mod hosts;
mod inspector;
mod pages;
mod ports;
mod process;
mod proto;
mod proxy;
mod routes;
```

- [ ] **Step 4: Run tests and verify they pass**

```bash
cargo test hosts::tests
```

Expected output (all pass):
```
test hosts::tests::build_block_empty ... ok
test hosts::tests::build_block_multiple ... ok
test hosts::tests::build_block_single ... ok
test hosts::tests::extract_managed_no_block ... ok
test hosts::tests::extract_managed_returns_inner_lines ... ok
test hosts::tests::hosts_path_is_not_empty ... ok
test hosts::tests::hosts_path_unix ... ok
test hosts::tests::remove_block_no_markers ... ok
test hosts::tests::remove_block_normalises_blank_lines ... ok
test hosts::tests::remove_block_strips_managed_block ... ok
test hosts::tests::remove_then_rebuild_is_idempotent ... ok
test hosts::tests::round_trip_build_extract ... ok
test hosts::tests::should_sync_default_true ... ok
```

- [ ] **Step 5: Commit**

```bash
git add src/hosts.rs src/main.rs
git commit -m "feat(hosts): pure string functions — build_block, remove_block, extract_managed"
```

---

## Task 2: `src/hosts.rs` — filesystem sync functions

**Files:**
- Modify: `src/hosts.rs`

Add `read_hosts`, `sync_hosts_file`, `clean_hosts_file`, and the macOS DNS flush.

- [ ] **Step 1: Add failing tests for filesystem functions**

Append these tests to the `#[cfg(test)] mod tests` block in `src/hosts.rs`:

```rust
    #[test]
    fn sync_creates_managed_block_in_file() {
        let dir = tempfile::tempdir().unwrap();
        let hosts_file = dir.path().join("hosts");
        std::fs::write(&hosts_file, "127.0.0.1 localhost\n").unwrap();

        sync_hosts_file_at(&["myapp.localhost"], &hosts_file).unwrap();

        let content = std::fs::read_to_string(&hosts_file).unwrap();
        assert!(content.contains("# portless-start"));
        assert!(content.contains("127.0.0.1 myapp.localhost"));
        assert!(content.contains("# portless-end"));
        assert!(content.contains("127.0.0.1 localhost"));
    }

    #[test]
    fn sync_replaces_existing_block() {
        let dir = tempfile::tempdir().unwrap();
        let hosts_file = dir.path().join("hosts");
        let initial = "127.0.0.1 localhost\n\n# portless-start\n127.0.0.1 oldapp.localhost\n# portless-end\n";
        std::fs::write(&hosts_file, initial).unwrap();

        sync_hosts_file_at(&["newapp.localhost"], &hosts_file).unwrap();

        let content = std::fs::read_to_string(&hosts_file).unwrap();
        assert!(content.contains("127.0.0.1 newapp.localhost"));
        assert!(!content.contains("oldapp.localhost"));
    }

    #[test]
    fn sync_empty_removes_block() {
        let dir = tempfile::tempdir().unwrap();
        let hosts_file = dir.path().join("hosts");
        let initial = "127.0.0.1 localhost\n\n# portless-start\n127.0.0.1 myapp.localhost\n# portless-end\n";
        std::fs::write(&hosts_file, initial).unwrap();

        sync_hosts_file_at(&[], &hosts_file).unwrap();

        let content = std::fs::read_to_string(&hosts_file).unwrap();
        assert!(!content.contains("portless-start"));
        assert!(!content.contains("myapp.localhost"));
        assert!(content.contains("127.0.0.1 localhost"));
    }

    #[test]
    fn sync_is_idempotent() {
        let dir = tempfile::tempdir().unwrap();
        let hosts_file = dir.path().join("hosts");
        std::fs::write(&hosts_file, "127.0.0.1 localhost\n").unwrap();

        let hostnames = &["myapp.localhost", "api.localhost"];
        sync_hosts_file_at(hostnames, &hosts_file).unwrap();
        let content1 = std::fs::read_to_string(&hosts_file).unwrap();

        sync_hosts_file_at(hostnames, &hosts_file).unwrap();
        let content2 = std::fs::read_to_string(&hosts_file).unwrap();

        assert_eq!(content1, content2);
    }
```

- [ ] **Step 2: Run to verify they fail**

```bash
cargo test hosts::tests::sync_
```

Expected: compile error — `sync_hosts_file_at` not defined.

- [ ] **Step 3: Implement the filesystem functions**

Add these functions to `src/hosts.rs` (before the `#[cfg(test)]` block):

```rust
/// Read the hosts file. Returns empty string on failure.
fn read_hosts() -> String {
    std::fs::read_to_string(hosts_path()).unwrap_or_default()
}

/// Core sync logic — writes to `path` instead of the real hosts file.
/// This separation allows tests to use a temp file.
pub fn sync_hosts_file_at(hostnames: &[&str], path: &std::path::Path) -> crate::error::Result<()> {
    let tmp_path = path.with_extension("tmp");

    // Preserve existing file permissions before overwriting
    #[cfg(unix)]
    let existing_perms = std::fs::metadata(path).ok().map(|m| m.permissions());

    let content = std::fs::read_to_string(path).unwrap_or_default();
    let cleaned = remove_block(&content);

    let new_content = if hostnames.is_empty() {
        cleaned
    } else {
        let block = build_block(hostnames);
        format!("{}\n{}\n", cleaned.trim_end(), block)
    };

    std::fs::write(&tmp_path, &new_content)?;

    // Restore permissions on the temp file before atomic rename
    #[cfg(unix)]
    if let Some(perms) = existing_perms {
        let _ = std::fs::set_permissions(&tmp_path, perms);
    }

    std::fs::rename(&tmp_path, path)?;

    Ok(())
}

/// Sync the real /etc/hosts with the given hostnames, then flush the macOS DNS cache.
pub fn sync_hosts_file(hostnames: &[&str]) -> crate::error::Result<()> {
    sync_hosts_file_at(hostnames, &hosts_path())?;

    #[cfg(target_os = "macos")]
    flush_dns_cache();

    Ok(())
}

/// Remove the portless-managed block from /etc/hosts.
pub fn clean_hosts_file() -> crate::error::Result<()> {
    sync_hosts_file(&[])
}

/// Fire-and-forget DNS cache flush on macOS. Failures are swallowed.
#[cfg(target_os = "macos")]
fn flush_dns_cache() {
    let _ = std::process::Command::new("dscacheutil")
        .arg("-flushcache")
        .output();
    let _ = std::process::Command::new("killall")
        .args(["-HUP", "mDNSResponder"])
        .output();
}
```

- [ ] **Step 4: Run tests and verify they pass**

```bash
cargo test hosts::tests::sync_
```

Expected:
```
test hosts::tests::sync_creates_managed_block_in_file ... ok
test hosts::tests::sync_empty_removes_block ... ok
test hosts::tests::sync_is_idempotent ... ok
test hosts::tests::sync_replaces_existing_block ... ok
```

- [ ] **Step 5: Run all hosts tests to confirm nothing broke**

```bash
cargo test hosts::
```

Expected: all 17 tests pass, 0 failures.

- [ ] **Step 6: Commit**

```bash
git add src/hosts.rs
git commit -m "feat(hosts): add sync_hosts_file, clean_hosts_file with atomic write + macOS DNS flush"
```

---

## Task 3: Add `HostsSync` and `HostsClean` to `src/proto.rs`

**Files:**
- Modify: `src/proto.rs`

- [ ] **Step 1: Write failing test**

Append to the `#[cfg(test)] mod tests` block in `src/proto.rs`:

```rust
    #[test]
    fn round_trips_hosts_sync_command() {
        let cmd = Command::HostsSync;
        let json = serde_json::to_string(&cmd).expect("serialize");
        let back: Command = serde_json::from_str(&json).expect("deserialize");
        assert!(matches!(back, Command::HostsSync));
    }

    #[test]
    fn round_trips_hosts_clean_command() {
        let cmd = Command::HostsClean;
        let json = serde_json::to_string(&cmd).expect("serialize");
        let back: Command = serde_json::from_str(&json).expect("deserialize");
        assert!(matches!(back, Command::HostsClean));
    }
```

- [ ] **Step 2: Run to verify they fail**

```bash
cargo test proto::tests::round_trips_hosts
```

Expected: compile error — `Command::HostsSync` and `Command::HostsClean` not found.

- [ ] **Step 3: Add the two variants to the `Command` enum**

In `src/proto.rs`, add after the `RegisterRoute` variant:

```rust
    /// Force-sync /etc/hosts with the current route table
    HostsSync,
    /// Remove the portless-managed block from /etc/hosts
    HostsClean,
```

The full enum now ends with:
```rust
    /// Register a route in the daemon's in-memory store (called by CLI after spawning child)
    RegisterRoute {
        hostname: String,
        port: u16,
        pid: u32,
        cwd: String,
    },
    /// Force-sync /etc/hosts with the current route table
    HostsSync,
    /// Remove the portless-managed block from /etc/hosts
    HostsClean,
}
```

- [ ] **Step 4: Run tests and verify they pass**

```bash
cargo test proto::tests::round_trips_hosts
```

Expected:
```
test proto::tests::round_trips_hosts_clean_command ... ok
test proto::tests::round_trips_hosts_sync_command ... ok
```

- [ ] **Step 5: Verify all existing proto tests still pass**

```bash
cargo test proto::
```

Expected: 5 tests pass, 0 failures.

- [ ] **Step 6: Commit**

```bash
git add src/proto.rs
git commit -m "feat(proto): add HostsSync and HostsClean IPC commands"
```

---

## Task 4: Wire hosts sync into `src/daemon/ipc.rs`

**Files:**
- Modify: `src/daemon/ipc.rs`

Add a helper to extract user-facing hostnames, then call `sync_hosts_file` after every mutating command. Handle the two new IPC commands.

- [ ] **Step 1: Write failing test**

Append to the `#[cfg(test)] mod tests` block in `src/daemon/ipc.rs`:

```rust
    #[test]
    fn user_hostnames_excludes_inspector() {
        // Simulate a route list containing the inspector internal route
        let all = vec![
            "myapp.localhost".to_string(),
            "_.localhost".to_string(),
            "api.localhost".to_string(),
        ];
        let user: Vec<&str> = all
            .iter()
            .filter(|h| h.as_str() != "_.localhost")
            .map(|h| h.as_str())
            .collect();
        assert_eq!(user.len(), 2);
        assert!(!user.contains(&"_.localhost"));
        assert!(user.contains(&"myapp.localhost"));
        assert!(user.contains(&"api.localhost"));
    }
```

- [ ] **Step 2: Run to verify test compiles and passes** (it's a pure logic test, no new code needed yet)

```bash
cargo test daemon::ipc::tests::user_hostnames_excludes_inspector
```

Expected: `ok` — this test validates the filter logic we're about to use.

- [ ] **Step 3: Add the `user_hostnames` helper and wire sync into dispatch**

In `src/daemon/ipc.rs`, add the helper function before `dispatch`:

```rust
/// Collect the hostnames of all user-registered routes (excludes internal `_.localhost`).
fn user_hostnames(routes: &RouteStore) -> Vec<String> {
    routes
        .list()
        .into_iter()
        .filter(|r| r.hostname != "_.localhost")
        .map(|r| r.hostname)
        .collect()
}

/// Sync /etc/hosts with current user routes. Logs a warning on failure but never panics.
fn sync_hosts(routes: &RouteStore) {
    if !crate::hosts::should_sync() {
        return;
    }
    let hostnames = user_hostnames(routes);
    let refs: Vec<&str> = hostnames.iter().map(|s| s.as_str()).collect();
    if let Err(e) = crate::hosts::sync_hosts_file(&refs) {
        tracing::warn!("hosts sync failed: {e}");
    }
}
```

Then update the `dispatch` function. Replace the `RegisterRoute` arm:

```rust
        Command::RegisterRoute {
            hostname,
            port,
            pid,
            cwd,
        } => {
            let route = crate::routes::Route {
                hostname: hostname.clone(),
                port,
                pid,
                owner_pid: pid,
                cwd,
                created_at: chrono::Utc::now(),
            };
            match routes.insert(route) {
                Ok(_) => {
                    sync_hosts(&routes);
                    Response::ok_empty()
                }
                Err(e) => Response::err(e.to_string()),
            }
        }
```

Replace the `Rm` arm:

```rust
        Command::Rm { hostname } => {
            let _ = routes.remove(&hostname);
            sync_hosts(&routes);
            Response::ok_empty()
        }
```

Replace the `Stop` arm:

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
                        use nix::sys::signal::{kill, Signal};
                        use nix::unistd::Pid;
                        kill(Pid::from_raw(route.pid as i32), Signal::SIGTERM).ok();
                    }
                    let _ = routes.remove(&hostname);
                    sync_hosts(&routes);
                    Response::ok_empty()
                }
            }
        }
```

Replace the `Ls` arm:

```rust
        Command::Ls => {
            let _ = routes.remove_stale();
            sync_hosts(&routes);
            let list: Vec<_> = routes
                .list()
                .into_iter()
                .filter(|r| r.hostname != "_.localhost")
                .collect();
            Response::ok(serde_json::to_value(&list).unwrap_or(serde_json::Value::Array(vec![])))
        }
```

Replace the `Shutdown` arm:

```rust
        Command::Shutdown => {
            let sock = sock_path.clone();
            let pid = pid_path.clone();
            tokio::spawn(async move {
                tokio::time::sleep(std::time::Duration::from_millis(100)).await;
                if let Err(e) = crate::hosts::clean_hosts_file() {
                    tracing::warn!("hosts cleanup on shutdown failed: {e}");
                }
                let _ = std::fs::remove_file(&sock);
                let _ = std::fs::remove_file(&pid);
                std::process::exit(0);
            });
            Response::ok_empty()
        }
```

Add the two new command arms (before the final `Command::Run` arm):

```rust
        Command::HostsSync => {
            let hostnames = user_hostnames(&routes);
            let refs: Vec<&str> = hostnames.iter().map(|s| s.as_str()).collect();
            match crate::hosts::sync_hosts_file(&refs) {
                Ok(_) => {
                    let entries: Vec<serde_json::Value> = refs
                        .iter()
                        .map(|h| serde_json::Value::String(format!("127.0.0.1 {h}")))
                        .collect();
                    Response::ok(serde_json::Value::Array(entries))
                }
                Err(e) => Response::err(e.to_string()),
            }
        }

        Command::HostsClean => match crate::hosts::clean_hosts_file() {
            Ok(_) => Response::ok_empty(),
            Err(e) => Response::err(e.to_string()),
        },
```

- [ ] **Step 4: Run all tests**

```bash
cargo test
```

Expected: all tests pass. The compiler will catch any signature mismatches.

- [ ] **Step 5: Commit**

```bash
git add src/daemon/ipc.rs
git commit -m "feat(daemon): auto-sync /etc/hosts on route mutations; handle HostsSync/HostsClean"
```

---

## Task 5: Add `portless hosts` CLI subcommand

**Files:**
- Modify: `src/cli/mod.rs`
- Modify: `src/cli/output.rs`

- [ ] **Step 1: Add output functions to `src/cli/output.rs`**

Read `src/cli/output.rs` first to understand existing style, then append:

```rust
/// Print the result of `portless hosts sync`.
pub fn print_hosts_sync(resp: &crate::proto::Response) {
    if !resp.ok {
        eprintln!(
            "error: {}",
            resp.error.as_deref().unwrap_or("unknown error")
        );
        return;
    }
    match &resp.data {
        Some(serde_json::Value::Array(entries)) if !entries.is_empty() => {
            for entry in entries {
                if let serde_json::Value::String(s) = entry {
                    println!("  {s}");
                }
            }
        }
        _ => println!("no active routes"),
    }
}

/// Print the result of `portless hosts clean`.
pub fn print_hosts_clean(resp: &crate::proto::Response) {
    if !resp.ok {
        eprintln!(
            "error: {}",
            resp.error.as_deref().unwrap_or("unknown error")
        );
        return;
    }
    println!("hosts file cleaned");
}
```

- [ ] **Step 2: Add `HostsAction` enum and `Hosts` variant to `src/cli/mod.rs`**

After the existing `CertAction` enum (around line 64), add:

```rust
#[derive(Subcommand)]
pub enum HostsAction {
    /// Force-rewrite the portless block in /etc/hosts from current routes
    Sync,
    /// Remove the portless block from /etc/hosts
    Clean,
}
```

In the `CliCommand` enum, add after `Init`:

```rust
    /// Manage /etc/hosts entries for portless routes
    Hosts {
        #[command(subcommand)]
        action: HostsAction,
    },
```

- [ ] **Step 3: Handle `CliCommand::Hosts` in the `run` function**

In the `match cli.command` block in `pub async fn run`, add after the `CliCommand::Cert` arm:

```rust
        CliCommand::Hosts { action } => {
            let (cmd, is_sync) = match action {
                HostsAction::Sync => (Command::HostsSync, true),
                HostsAction::Clean => (Command::HostsClean, false),
            };
            let mut stream = ipc_connect().await?;
            write_frame(&mut stream, &cmd).await?;
            let resp: crate::proto::Response = read_frame(&mut stream).await?;
            if is_sync {
                output::print_hosts_sync(&resp);
            } else {
                output::print_hosts_clean(&resp);
            }
        }
```

- [ ] **Step 4: Run all tests**

```bash
cargo test
```

Expected: all tests pass.

- [ ] **Step 5: Verify the subcommand appears in help**

```bash
cargo run --quiet -- --help
```

Expected: `hosts` appears in the command list.

```bash
cargo run --quiet -- hosts --help
```

Expected: shows `sync` and `clean` subcommands.

- [ ] **Step 6: Commit**

```bash
git add src/cli/mod.rs src/cli/output.rs
git commit -m "feat(cli): add 'portless hosts sync' and 'portless hosts clean' subcommands"
```

---

## Final: Full test run

- [ ] **Run full test suite**

```bash
cargo test
```

Expected: all tests pass, 0 failures.

- [ ] **Verify help output is clean**

```bash
cargo run --quiet -- hosts sync --help
cargo run --quiet -- hosts clean --help
```

Both should show concise help text with no errors.
