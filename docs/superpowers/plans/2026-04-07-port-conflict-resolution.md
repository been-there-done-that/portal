# Port Conflict Resolution — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** When `portal start` can't bind ports 80/443 because another process is using them, show an interactive multi-select menu so the user can kill the occupiers and retry — instead of timing out silently.

**Architecture:** All new logic lives in `src/cli/mod.rs` as private helper functions wired into `ensure_daemon_running` before the daemon spawn. New dep `dialoguer = "0.11"` provides the interactive multi-select prompt. Port checking uses `std::net::TcpListener::bind`. Process discovery uses `lsof`. Killing uses `nix::signal::kill` with a `sudo kill` fallback for root processes.

**Tech Stack:** Rust, `dialoguer = "0.11"`, `lsof` (system tool, available on macOS and most Linux distros), `nix 0.29` (already in deps for signal sending).

---

## File Map

| File | Change |
|------|--------|
| `Cargo.toml` | Add `dialoguer = "0.11"` |
| `src/cli/mod.rs` | Add `check_ports_free`, `PortOccupier`, `discover_port_occupiers`, `show_conflict_menu`, `kill_occupiers`; wire into `ensure_daemon_running` |

---

### Task 1: Add `dialoguer` dependency

**Files:**
- Modify: `Cargo.toml`

- [ ] **Step 1: Add the dependency**

In `Cargo.toml`, after the `console = "0.15"` line, add:

```toml
dialoguer = "0.11"
```

- [ ] **Step 2: Build to fetch**

```bash
cargo build 2>&1 | tail -5
```

Expected: fetches `dialoguer` and its deps, compiles cleanly.

- [ ] **Step 3: Commit**

```bash
git add Cargo.toml Cargo.lock
git commit -m "chore: add dialoguer for interactive prompts"
```

---

### Task 2: Implement `check_ports_free` and `PortOccupier`

**Files:**
- Modify: `src/cli/mod.rs` (add before `ipc_connect`)

- [ ] **Step 1: Add the struct and function after the existing `ipc_connect` fn**

In `src/cli/mod.rs`, after the closing `}` of `ensure_cert_trusted` (currently the last function), add:

```rust
/// A process that is currently listening on one of the configured ports.
#[derive(Debug, Clone)]
struct PortOccupier {
    pid: u32,
    name: String,
    ports: Vec<u16>,
}

/// Returns the subset of `ports` that are already bound by another process.
/// Uses a non-blocking TcpListener::bind attempt — if binding fails with
/// AddrInUse, the port is occupied.
fn check_ports_free(ports: &[u16]) -> Vec<u16> {
    ports
        .iter()
        .copied()
        .filter(|&p| std::net::TcpListener::bind(("0.0.0.0", p)).is_err())
        .collect()
}
```

- [ ] **Step 2: Build**

```bash
cargo build 2>&1
```

Expected: compiles cleanly (no usages yet, but no dead-code warning since we'll add them next).

---

### Task 3: Implement `discover_port_occupiers`

**Files:**
- Modify: `src/cli/mod.rs` (add after `check_ports_free`)

- [ ] **Step 1: Add `discover_port_occupiers`**

```rust
/// Run `lsof` to find which processes are listening on the given ports.
/// Deduplicates by PID — one `PortOccupier` per process even if it holds
/// multiple ports. Returns empty Vec if lsof is unavailable or returns nothing.
fn discover_port_occupiers(ports: &[u16]) -> Vec<PortOccupier> {
    if ports.is_empty() {
        return vec![];
    }

    // Build: lsof -nP -sTCP:LISTEN -F pcn -iTCP:80 -iTCP:443
    let mut args = vec![
        "-nP".to_string(),
        "-sTCP:LISTEN".to_string(),
        "-F".to_string(),
        "pcn".to_string(),
    ];
    for &p in ports {
        args.push(format!("-iTCP:{p}"));
    }

    let output = match std::process::Command::new("lsof")
        .args(&args)
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null())
        .output()
    {
        Ok(o) => o,
        Err(_) => return vec![],
    };

    let text = String::from_utf8_lossy(&output.stdout);
    parse_lsof_output(&text, ports)
}

/// Parse `lsof -F pcn` output into `PortOccupier` entries.
///
/// Field format (one per line):
///   p<pid>      — starts a new process block
///   c<command>  — process name
///   n*:<port>   — listening address (e.g. `*:443` or `127.0.0.1:80`)
fn parse_lsof_output(text: &str, wanted_ports: &[u16]) -> Vec<PortOccupier> {
    let mut occupiers: Vec<PortOccupier> = Vec::new();
    let mut current_pid: Option<u32> = None;
    let mut current_name = String::new();
    let mut current_ports: Vec<u16> = Vec::new();

    let flush = |pid: Option<u32>, name: &str, ports: &[u16], out: &mut Vec<PortOccupier>| {
        if let Some(pid) = pid {
            if !ports.is_empty() {
                // Merge into existing entry for this PID if present
                if let Some(existing) = out.iter_mut().find(|o| o.pid == pid) {
                    for &p in ports {
                        if !existing.ports.contains(&p) {
                            existing.ports.push(p);
                        }
                    }
                } else {
                    out.push(PortOccupier {
                        pid,
                        name: name.to_string(),
                        ports: ports.to_vec(),
                    });
                }
            }
        }
    };

    for line in text.lines() {
        if let Some(rest) = line.strip_prefix('p') {
            flush(current_pid, &current_name, &current_ports, &mut occupiers);
            current_pid = rest.parse().ok();
            current_name.clear();
            current_ports.clear();
        } else if let Some(rest) = line.strip_prefix('c') {
            current_name = rest.to_string();
        } else if let Some(rest) = line.strip_prefix('n') {
            // Extract port from address like "*:443" or "127.0.0.1:80"
            if let Some(port_str) = rest.rsplit(':').next() {
                if let Ok(port) = port_str.parse::<u16>() {
                    if wanted_ports.contains(&port) && !current_ports.contains(&port) {
                        current_ports.push(port);
                    }
                }
            }
        }
    }
    flush(current_pid, &current_name, &current_ports, &mut occupiers);
    occupiers
}
```

- [ ] **Step 2: Add a unit test for `parse_lsof_output`**

In `src/cli/mod.rs`, add at the bottom of the file:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_lsof_output_single_process() {
        let input = "p51055\ncportless\nn*:80\nn*:443\n";
        let result = parse_lsof_output(input, &[80, 443]);
        assert_eq!(result.len(), 1);
        assert_eq!(result[0].pid, 51055);
        assert_eq!(result[0].name, "portless");
        assert!(result[0].ports.contains(&80));
        assert!(result[0].ports.contains(&443));
    }

    #[test]
    fn test_parse_lsof_output_two_processes() {
        let input = "p100\ncnginx\nn*:80\np200\ncnode\nn*:443\n";
        let result = parse_lsof_output(input, &[80, 443]);
        assert_eq!(result.len(), 2);
        assert_eq!(result[0].pid, 100);
        assert_eq!(result[0].name, "nginx");
        assert_eq!(result[0].ports, vec![80]);
        assert_eq!(result[1].pid, 200);
        assert_eq!(result[1].name, "node");
        assert_eq!(result[1].ports, vec![443]);
    }

    #[test]
    fn test_parse_lsof_output_ignores_unwanted_ports() {
        let input = "p100\ncnginx\nn*:8080\nn*:80\n";
        let result = parse_lsof_output(input, &[80, 443]);
        assert_eq!(result.len(), 1);
        assert_eq!(result[0].ports, vec![80]);
    }

    #[test]
    fn test_parse_lsof_output_empty() {
        let result = parse_lsof_output("", &[80, 443]);
        assert!(result.is_empty());
    }
}
```

- [ ] **Step 3: Run tests**

```bash
cargo test test_parse_lsof 2>&1
```

Expected: 4 tests pass.

- [ ] **Step 4: Commit**

```bash
git add src/cli/mod.rs
git commit -m "feat(cli): add port-conflict detection helpers (check_ports_free, discover_port_occupiers)"
```

---

### Task 4: Implement `show_conflict_menu` and `kill_occupiers`

**Files:**
- Modify: `src/cli/mod.rs` (add after `parse_lsof_output`)

- [ ] **Step 1: Add `show_conflict_menu`**

```rust
enum ConflictAction {
    KillAndRetry(Vec<u32>),
    Cancel,
}

/// Show an interactive multi-select prompt listing processes that hold the
/// conflicting ports. Returns `KillAndRetry(pids)` or `Cancel`.
fn show_conflict_menu(
    occupiers: &[PortOccupier],
    conflicting_ports: &[u16],
) -> crate::error::Result<ConflictAction> {
    use console::style;
    use dialoguer::{MultiSelect, Select, theme::ColorfulTheme};

    let ports_str = conflicting_ports
        .iter()
        .map(|p| format!(":{p}"))
        .collect::<Vec<_>>()
        .join(" and ");

    eprintln!();
    eprintln!(
        "  {} {}",
        style("port conflict:").yellow().bold(),
        style(format!("{ports_str} already in use")).dim()
    );
    eprintln!();

    // Build item labels with aligned columns
    let items: Vec<String> = occupiers
        .iter()
        .map(|o| {
            let ports = o
                .ports
                .iter()
                .map(|p| format!(":{p}"))
                .collect::<Vec<_>>()
                .join(" ");
            format!("{:<14} pid {:<8} {}", o.name, o.pid, ports)
        })
        .collect();

    // Pre-select all by default
    let defaults: Vec<bool> = vec![true; items.len()];

    let selections = MultiSelect::with_theme(&ColorfulTheme::default())
        .with_prompt("Select processes to kill")
        .items(&items)
        .defaults(&defaults)
        .interact_opt()
        .unwrap_or(None);

    let selected_indices = match selections {
        None | Some(ref v) if v.is_empty() => {
            return Ok(ConflictAction::Cancel);
        }
        Some(v) => v,
    };

    let action_items = &["Kill selected & retry", "Cancel"];
    let action = Select::with_theme(&ColorfulTheme::default())
        .with_prompt("Action")
        .items(action_items)
        .default(0)
        .interact_opt()
        .unwrap_or(None);

    match action {
        Some(0) => {
            let pids = selected_indices
                .iter()
                .map(|&i| occupiers[i].pid)
                .collect();
            Ok(ConflictAction::KillAndRetry(pids))
        }
        _ => Ok(ConflictAction::Cancel),
    }
}
```

- [ ] **Step 2: Add `kill_occupiers`**

```rust
/// Kill each PID. Tries `nix::kill(SIGTERM)` first; falls back to
/// `sudo kill <pid>` for permission errors (root-owned processes).
/// After all kills, waits up to 2 s for ports to free.
async fn kill_occupiers(pids: &[u32], ports: &[u16]) {
    for &pid in pids {
        #[cfg(unix)]
        {
            use nix::sys::signal::{kill, Signal};
            use nix::unistd::Pid;
            match kill(Pid::from_raw(pid as i32), Signal::SIGTERM) {
                Ok(_) => {
                    eprintln!(
                        "  {} killed pid {pid}",
                        console::style("✓").green()
                    );
                }
                Err(nix::errno::Errno::EPERM) => {
                    // Root-owned process — escalate with sudo
                    let status = tokio::process::Command::new("sudo")
                        .args(["kill", &pid.to_string()])
                        .stdin(std::process::Stdio::inherit())
                        .stdout(std::process::Stdio::null())
                        .stderr(std::process::Stdio::inherit())
                        .status()
                        .await;
                    match status {
                        Ok(s) if s.success() => {
                            eprintln!(
                                "  {} killed pid {pid} (sudo)",
                                console::style("✓").green()
                            );
                        }
                        _ => {
                            eprintln!(
                                "  {} failed to kill pid {pid}",
                                console::style("✗").red()
                            );
                        }
                    }
                }
                Err(e) => {
                    eprintln!(
                        "  {} failed to kill pid {pid}: {e}",
                        console::style("✗").red()
                    );
                }
            }
        }
        #[cfg(not(unix))]
        {
            let _ = pid;
        }
    }

    // Wait up to 2 s for the ports to free
    for _ in 0..14 {
        tokio::time::sleep(std::time::Duration::from_millis(150)).await;
        if check_ports_free(ports).is_empty() {
            return;
        }
    }
    // Ports may still be in use — proceed anyway, daemon spawn will surface the real error
}
```

- [ ] **Step 3: Build**

```bash
cargo build 2>&1
```

Expected: compiles cleanly. Fix any import errors — you may need to add `use` for `dialoguer`.

- [ ] **Step 4: Commit**

```bash
git add src/cli/mod.rs
git commit -m "feat(cli): add show_conflict_menu and kill_occupiers"
```

---

### Task 5: Wire conflict resolution into `ensure_daemon_running`

**Files:**
- Modify: `src/cli/mod.rs` — `ensure_daemon_running` function, needs_sudo path

- [ ] **Step 1: Insert conflict check before the UI steps**

In `ensure_daemon_running`, in the `if needs_sudo {` block, after the TTY guard (the `if !std::io::stdin().is_terminal()` block) and before the `plain_step` UI lines, insert:

```rust
        // Check for port conflicts before attempting daemon start.
        // If ports are occupied, let the user kill the offenders interactively.
        let conflicting = check_ports_free(&[config.proxy.http_port, config.proxy.https_port]);
        if !conflicting.is_empty() {
            let occupiers = discover_port_occupiers(&conflicting);
            if occupiers.is_empty() {
                let ports_str = conflicting
                    .iter()
                    .map(|p| p.to_string())
                    .collect::<Vec<_>>()
                    .join(", ");
                eprintln!(
                    "error: port(s) {} are already in use (cannot identify processes).",
                    ports_str
                );
                eprintln!("  Try: sudo lsof -iTCP:{} -sTCP:LISTEN", conflicting[0]);
                return Err(crate::error::Error::DaemonNotRunning);
            }
            match show_conflict_menu(&occupiers, &conflicting)? {
                ConflictAction::KillAndRetry(pids) => {
                    kill_occupiers(&pids, &conflicting).await;
                }
                ConflictAction::Cancel => {
                    return Err(crate::error::Error::DaemonNotRunning);
                }
            }
        }
```

The complete `needs_sudo` block should now look like:

```rust
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

        // Check for port conflicts before attempting daemon start.
        let conflicting = check_ports_free(&[config.proxy.http_port, config.proxy.https_port]);
        if !conflicting.is_empty() {
            let occupiers = discover_port_occupiers(&conflicting);
            if occupiers.is_empty() {
                let ports_str = conflicting
                    .iter()
                    .map(|p| p.to_string())
                    .collect::<Vec<_>>()
                    .join(", ");
                eprintln!(
                    "error: port(s) {} are already in use (cannot identify processes).",
                    ports_str
                );
                eprintln!("  Try: sudo lsof -iTCP:{} -sTCP:LISTEN", conflicting[0]);
                return Err(crate::error::Error::DaemonNotRunning);
            }
            match show_conflict_menu(&occupiers, &conflicting)? {
                ConflictAction::KillAndRetry(pids) => {
                    kill_occupiers(&pids, &conflicting).await;
                }
                ConflictAction::Cancel => {
                    return Err(crate::error::Error::DaemonNotRunning);
                }
            }
        }

        // Plain-text path — no indicatif spinners that could corrupt the TTY.
        if ca_missing {
            setup.plain_step("cert     generating CA certificate…");
        }
        setup.plain_step("daemon   starting (sudo may ask for your password)…");

        // ... rest of sudo path unchanged ...
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

Expected: all tests pass (including the 4 lsof parse tests from Task 3).

- [ ] **Step 4: Commit**

```bash
git add src/cli/mod.rs
git commit -m "feat(cli): show interactive kill menu when ports 80/443 are already in use"
```

---

### Task 6: Manual verification

- [ ] **Step 1: Simulate a port conflict**

Start something on port 80 (or use the old portless daemon if still installed):

```bash
# Start a process on port 443 for testing
python3 -c "import socket; s=socket.socket(); s.bind(('0.0.0.0', 8443)); s.listen(1); input()" &
```

Or if the default portal ports are 80/443, simply have any other daemon running on those ports.

Then run:

```bash
portal start
```

Expected interactive output:
```
  port conflict: :80 and :443 already in use

  Select processes to kill:
  > [✓] portless      pid 51055   :80 :443

  Kill selected & retry
  Cancel
```

- [ ] **Step 2: Select "Kill selected & retry"**

Press Enter to confirm. Expected:
```
  ✓ killed pid 51055 (sudo)
  cert     generating CA certificate…     (if first run)
  daemon   starting (sudo may ask for your password)…
  ✓ daemon  started  on :80/:443
```

- [ ] **Step 3: Select "Cancel"**

Run `portal start` again with a conflict, navigate to "Cancel". Expected: process exits cleanly with no error spam.

- [ ] **Step 4: Verify no conflict path is unaffected**

Stop the conflicting process, kill portal daemon, run `portal start` fresh. Expected: no conflict menu appears, normal start flow proceeds.
