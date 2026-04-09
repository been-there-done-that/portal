# Multi-Language Runtime Fixes Summary

Date: 2026-04-09

This document summarizes the six review issues found during the `feature/multi-language-support` work and how they were fixed.

## First Review Round: Original 4 Problems

### 1. `--tcp` was exposed in the CLI but not implemented end to end

Problem:
- `portal run --tcp ...` changed CLI behavior and banner output, but the daemon still treated all routes as HTTP routes.
- That meant raw TCP services such as Redis or Postgres could be registered but not actually reached through Portal.

Fix:
- Added protocol-aware routes with `RouteProtocol::{Http, Tcp}` in [src/routes.rs](/Users/__deesh_reddy__/projects/personal_git/rust_builds/portless/src/routes.rs).
- Extended route registration IPC to carry `protocol` and `public_port` in [src/proto.rs](/Users/__deesh_reddy__/projects/personal_git/rust_builds/portless/src/proto.rs).
- Added daemon-managed TCP forwarding in [src/tcp.rs](/Users/__deesh_reddy__/projects/personal_git/rust_builds/portless/src/tcp.rs).
- Updated the daemon to restore, register, remove, and display TCP routes separately from HTTP routes in [src/daemon/ipc.rs](/Users/__deesh_reddy__/projects/personal_git/rust_builds/portless/src/daemon/ipc.rs) and [src/daemon/mod.rs](/Users/__deesh_reddy__/projects/personal_git/rust_builds/portless/src/daemon/mod.rs).
- Updated CLI/banner output so TCP routes show a TCP endpoint instead of an HTTPS URL in [src/cli/banner.rs](/Users/__deesh_reddy__/projects/personal_git/rust_builds/portless/src/cli/banner.rs) and [src/cli/output.rs](/Users/__deesh_reddy__/projects/personal_git/rust_builds/portless/src/cli/output.rs).

Result:
- `portal run --tcp ...` now creates a real TCP listener on a public local port and forwards traffic bidirectionally to the backend service.

### 2. `portal start` broke quoted `portal.toml` commands

Problem:
- `start_command` was being tokenized with whitespace splitting.
- This broke quoted arguments and escaped spaces, for example Python factory apps or file paths containing spaces.

Fix:
- Replaced whitespace splitting with a shell-like parser in [src/cli/mod.rs](/Users/__deesh_reddy__/projects/personal_git/rust_builds/portless/src/cli/mod.rs).
- Added `parse_command_line()` and `parse_start_command()` so `portal.toml` command parsing preserves quotes and escapes.
- Added targeted parser tests for quoted and escaped arguments in [src/cli/mod.rs](/Users/__deesh_reddy__/projects/personal_git/rust_builds/portless/src/cli/mod.rs).

Result:
- `portal start` now executes `start_command` as authored in `portal.toml`, including quoted module paths and escaped spaces.

### 3. `NODE_EXTRA_CA_CERTS` pointed at the wrong CA file

Problem:
- The Node trust environment variable was checking the wrong path under the state directory root.
- As a result, Node child processes did not trust Portal-issued localhost certificates.

Fix:
- Centralized the CA path helper in `portal_ca_cert_path()` in [src/cli/mod.rs](/Users/__deesh_reddy__/projects/personal_git/rust_builds/portless/src/cli/mod.rs).
- Corrected the path to `~/.portal/certs/ca.pem`.
- Kept the variable gated behind HTTPS mode and file existence.
- Added a focused test verifying the helper points into `certs/ca.pem`.

Result:
- Node-based child processes now receive the correct CA file path and can trust Portal TLS certs as intended.

### 4. `PORTAL_URL` ignored non-default external ports

Problem:
- `PORTAL_URL` was always exported as `https://hostname` even when Portal was configured to listen on a non-default public HTTPS port such as `4443`.
- That caused bad callback URLs, origin URLs, and absolute links.

Fix:
- Added shared public URL construction helpers in [src/cli/mod.rs](/Users/__deesh_reddy__/projects/personal_git/rust_builds/portless/src/cli/mod.rs) and matching display logic in [src/daemon/ipc.rs](/Users/__deesh_reddy__/projects/personal_git/rust_builds/portless/src/daemon/ipc.rs).
- The URL builder now includes the configured public port when HTTPS is not on `443`, and similarly handles HTTP-only mode.
- Added tests for non-default HTTPS and HTTP-only URL generation in [src/cli/mod.rs](/Users/__deesh_reddy__/projects/personal_git/rust_builds/portless/src/cli/mod.rs).

Result:
- `PORTAL_URL` now reflects the externally reachable Portal address in both default-port and unprivileged-port configurations.

## Second Review Round: Follow-Up 2 Problems

### 5. Stale TCP routes were removed from state but their listeners kept running

Problem:
- When a TCP-backed child process died, stale route cleanup removed the route record but did not stop the live TCP listener task.
- This left the public TCP port occupied and accepted new connections until daemon restart.

Fix:
- Changed `StateStore::remove_stale()` to return the removed `Route` records in [src/routes.rs](/Users/__deesh_reddy__/projects/personal_git/rust_builds/portless/src/routes.rs).
- Updated `Command::Ls` in [src/daemon/ipc.rs](/Users/__deesh_reddy__/projects/personal_git/rust_builds/portless/src/daemon/ipc.rs) to tear down TCP listener tasks for any stale TCP routes it purges.
- Added `shutdown_all()` to `TcpRouteManager` in [src/tcp.rs](/Users/__deesh_reddy__/projects/personal_git/rust_builds/portless/src/tcp.rs) so daemon shutdown also aborts active TCP listeners cleanly.
- Added focused tests for stale TCP route cleanup and public port release in [src/daemon/ipc.rs](/Users/__deesh_reddy__/projects/personal_git/rust_builds/portless/src/daemon/ipc.rs) and [src/routes.rs](/Users/__deesh_reddy__/projects/personal_git/rust_builds/portless/src/routes.rs).

Result:
- Removing stale TCP routes now also removes their live listener tasks, so ports are released and no dead forwarding remains active.

### 6. `portal run --tcp` still bootstrapped the full HTTP/HTTPS daemon

Problem:
- Even in TCP mode, the CLI still called the normal daemon bootstrap path.
- On default `:80/:443` setups, that could still trigger privileged-port checks or `sudo`, even though the TCP route did not need HTTP or HTTPS listeners.

Fix:
- Introduced explicit daemon modes, `DaemonMode::{Full, TcpOnly}`, in [src/daemon/mod.rs](/Users/__deesh_reddy__/projects/personal_git/rust_builds/portless/src/daemon/mod.rs).
- Added a hidden internal CLI flag for daemon respawn mode selection in [src/cli/mod.rs](/Users/__deesh_reddy__/projects/personal_git/rust_builds/portless/src/cli/mod.rs).
- Refactored `ensure_daemon_running()` in [src/cli/mod.rs](/Users/__deesh_reddy__/projects/personal_git/rust_builds/portless/src/cli/mod.rs) to request either:
  - `Full` for normal HTTP/HTTPS routes
  - `TcpOnly` for `portal run --tcp`
- Extended IPC status responses with the daemon mode in [src/daemon/ipc.rs](/Users/__deesh_reddy__/projects/personal_git/rust_builds/portless/src/daemon/ipc.rs).
- Added logic to:
  - reuse a full daemon for TCP requests
  - reuse a TCP-only daemon for TCP requests
  - upgrade a TCP-only daemon to a full daemon when an HTTP route is requested later
- Updated daemon status output to display the mode in [src/cli/output.rs](/Users/__deesh_reddy__/projects/personal_git/rust_builds/portless/src/cli/output.rs).
- Added tests for daemon mode parsing and compatibility in [src/cli/mod.rs](/Users/__deesh_reddy__/projects/personal_git/rust_builds/portless/src/cli/mod.rs).

Result:
- `portal run --tcp` no longer requires starting the HTTP/HTTPS listeners or triggering privileged-port bootstrap just to run a TCP proxy.

## Verification

Focused tests added or validated:
- `cargo test stale_cleanup_returns_removed_tcp_routes`
- `cargo test ls_removes_stale_tcp_routes_and_releases_public_port`
- `cargo test daemon_mode_reads_tcp_only_status`
- `cargo test tcp_requirement_accepts_full_or_tcp_only_daemon`

Full test run:
- `cargo test --quiet` still has one pre-existing sandbox-sensitive failure in [src/process.rs](/Users/__deesh_reddy__/projects/personal_git/rust_builds/portless/src/process.rs): `process::tests::spawn_child_uses_separate_process_group`
- The new runtime and TCP-specific changes passed their targeted coverage.
