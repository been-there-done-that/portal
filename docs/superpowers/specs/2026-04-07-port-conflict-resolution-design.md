# Port Conflict Resolution — Design Spec

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task.

**Goal:** When `portal start` fails because ports 80/443 are already in use, detect the conflicting processes and show an interactive multi-select menu so the user can kill them and retry — instead of timing out silently.

**Architecture:** Before spawning the daemon in `ensure_daemon_running` (needs_sudo path), probe the configured ports. If occupied, discover the owning processes via `lsof`, render a `dialoguer::MultiSelect` prompt, kill selected processes with `sudo kill`, then continue to daemon start as normal. All logic stays in `src/cli/mod.rs`. New dep: `dialoguer = "0.11"`.

**Tech Stack:** Rust, `dialoguer 0.11` (interactive prompts), `lsof` (macOS/Linux system tool), existing `console 0.15` for colors.

---

## Flow

```
portal start
  └─ ensure_daemon_running (needs_sudo path)
       └─ check_ports_free(http_port, https_port)
            ├─ all free → continue to sudo spawn (existing behavior)
            └─ conflict detected
                 ├─ discover_port_occupiers(ports) → Vec<PortOccupier>
                 ├─ if empty (can't read processes) → print error, return Err
                 └─ show_conflict_menu(occupiers) → ConflictAction
                      ├─ KillAndRetry(selected_pids) → kill each, re-check, continue
                      └─ Cancel → return Err(DaemonNotRunning)
```

---

## Port Checking

`fn check_ports_free(ports: &[u16]) -> Vec<u16>` — returns the subset of ports that are already bound.

Try to bind a `TcpListener` on `127.0.0.1:PORT`. If it fails with `AddrInUse`, the port is occupied. This is a pure Rust check, no external tools needed, and works on both macOS and Linux.

```rust
fn check_ports_free(ports: &[u16]) -> Vec<u16> {
    ports.iter().copied()
        .filter(|&p| std::net::TcpListener::bind(("0.0.0.0", p)).is_err())
        .collect()
}
```

---

## Process Discovery

`fn discover_port_occupiers(ports: &[u16]) -> Vec<PortOccupier>` — uses `lsof` to find which processes hold the given ports.

```rust
#[derive(Debug, Clone)]
pub struct PortOccupier {
    pub pid: u32,
    pub name: String,       // process name, e.g. "portless"
    pub ports: Vec<u16>,    // all conflicting ports this process holds
}
```

Run: `lsof -nP -iTCP:PORT -sTCP:LISTEN -F pncf` for each conflicting port, parse output. Deduplicate by PID so one entry per process even if it holds both 80 and 443.

On failure (lsof not found, permission error, no output): return empty Vec — the caller shows a fallback error message.

**`lsof` field format (`-F pncf`):**
```
p51055        ← pid
cportless     ← command/name
n*:443        ← port
```

Parse with simple line-by-line scanning: lines starting with `p` = pid, `c` = command, `n` = address/port.

---

## Interactive Menu

`fn show_conflict_menu(occupiers: &[PortOccupier]) -> ConflictAction`

Uses `dialoguer::MultiSelect` pre-selecting all occupiers, followed by a `dialoguer::Select` for the action.

```
  port 80 and 443 are already in use

  Select processes to kill:
  > [✓] portless   pid 51055   :80 :443
    [✓] nginx      pid 3201    :80

  > Kill selected & retry
    Cancel
```

Implementation:
```rust
enum ConflictAction {
    KillAndRetry(Vec<u32>),  // selected PIDs
    Cancel,
}
```

1. Build item labels: `format!("{:<12} pid {:<8} {}", name, pid, ports_str)` using `console::measure_text_width` for alignment
2. `dialoguer::MultiSelect::new()` with all items pre-selected
3. If user selects nothing, ask again or treat as Cancel
4. After selection confirmed: `dialoguer::Select` with "Kill selected & retry" / "Cancel"

---

## Killing Processes

`async fn kill_occupiers(pids: &[u32]) -> Result<()>`

For each PID:
1. Try `kill(Pid::from_raw(pid), SIGTERM)` via `nix` (works if process is owned by current user)
2. If `EPERM` (root process): run `sudo kill PID` via `tokio::process::Command::new("sudo").args(["kill", &pid.to_string()]).status().await`
3. After all kills: wait up to 2s for ports to be freed (`check_ports_free` in a poll loop)

If ports still occupied after 2s: print warning, continue anyway (daemon spawn will fail with a clear error).

---

## Integration in `ensure_daemon_running`

Insert between the TTY guard and the `plain_step` UI in the `needs_sudo` block:

```rust
// Check for port conflicts before attempting daemon start
let conflicting = check_ports_free(&[config.proxy.http_port, config.proxy.https_port]);
if !conflicting.is_empty() {
    let occupiers = discover_port_occupiers(&conflicting);
    if occupiers.is_empty() {
        eprintln!("error: ports {:?} are already in use (cannot identify processes — try: sudo lsof -iTCP:{} -sTCP:LISTEN)", conflicting, conflicting[0]);
        return Err(crate::error::Error::DaemonNotRunning);
    }
    match show_conflict_menu(&occupiers)? {
        ConflictAction::KillAndRetry(pids) => {
            kill_occupiers(&pids).await?;
        }
        ConflictAction::Cancel => {
            return Err(crate::error::Error::DaemonNotRunning);
        }
    }
}
```

---

## File Change Map

| File | Change |
|------|--------|
| `Cargo.toml` | Add `dialoguer = "0.11"` |
| `src/cli/mod.rs` | Add `check_ports_free`, `discover_port_occupiers`, `show_conflict_menu`, `kill_occupiers`; wire into `ensure_daemon_running` |

No daemon changes. No protocol changes.

---

## Platform Notes

- **macOS**: `lsof` ships with the OS. Works without sudo for user processes; root processes show name but may omit PID in some macOS versions — handle gracefully by using the PID from the `p` field directly.
- **Linux**: `lsof` available on most distros. Alternative: `ss -tlnp` — but `lsof` is more portable and already the macOS standard.
- **Non-sudo path** (ports ≥ 1024): Port conflicts are less likely (user-space processes, easily killed without sudo). The same code runs — `nix::kill` succeeds for user-owned processes; `sudo kill` fallback only needed for root-owned processes.

---

## Out of Scope

- Killing processes that don't appear in `lsof` output
- Windows support (ports < 1024 don't require sudo on Windows; port conflict detection there uses `netstat`)
- Automatic retry after failed kill (one attempt per run)
