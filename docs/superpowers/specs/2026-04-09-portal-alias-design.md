# Portal Alias Design

**Goal:** Add `portal alias <name> <port>` to register a static route pointing to an already-running service. No child process is spawned — the existing HTTPS proxy routes by hostname.

**Architecture:** New `Alias` variant in `CliCommand`. Handler sends `RegisterRoute` IPC with `pid: 0` (sentinel for "no managed process"). Stale cleanup skips pid=0 routes. `portal ls` shows `(alias)` label. `portal stop` on an alias removes the route without killing anything.

---

## CLI

```bash
portal alias <name> <port>             # Register: https://<name>.localhost → localhost:<port>
portal alias <name> <port> --force     # Overwrite an existing route
portal alias --remove <name>           # Remove the alias
```

Examples:
```bash
portal alias my-postgres 5432          # https://my-postgres.localhost → localhost:5432
portal alias api.myapp 8080            # https://api.myapp.localhost → localhost:8080
portal alias redis 6379                # https://redis.localhost → localhost:6379
portal alias --remove my-postgres      # Remove the alias
```

## Route Registration

The `Alias` handler:
1. Resolves hostname: `sanitize_hostname(name)` + `.{tld}` (e.g. `my-postgres.localhost`)
2. Checks if a route already exists for that hostname. If it does and `--force` is not set, print an error and exit.
3. Sends `RegisterRoute` IPC with:
   - `hostname`: resolved hostname
   - `port`: user-provided port
   - `public_port`: `None`
   - `protocol`: `RouteProtocol::Http`
   - `pid`: `0` (sentinel — no managed process)
   - `cwd`: empty string
4. Prints: `✓ https://my-postgres.localhost → localhost:5432`

## Stale Cleanup

`pid_alive_check` in `routes.rs` must skip pid=0 — aliases are never stale. Currently `pid_alive_check(0)` would return `true` on Unix (kill(0, 0) signals the current process group), which is accidentally correct but semantically wrong. Add an explicit early return for pid=0: `if pid == 0 { return true; }`.

## Display

In `portal ls` output, routes with `pid == 0` show `(alias)` instead of `pid <N>`:
```
  https://my-postgres.localhost
  └─ localhost:5432  ·  (alias)
```

## Stop/Remove Behavior

`portal stop <hostname>` on an alias: skip the SIGTERM (pid is 0, no process to kill), just remove the route. The current code in `dispatch` already calls `killpg` with the route's pid — we need to guard that behind `pid != 0`.

`portal rm <hostname>` works unchanged — it just removes the route without killing.

## Files Changed

| File | Change |
|---|---|
| `src/cli/mod.rs` | Add `Alias` variant to `CliCommand` with `name`, `port`, `force`, `remove` fields. Add handler that sends `RegisterRoute` IPC or `Rm` IPC for `--remove`. |
| `src/cli/output.rs` | In `print_ls`, show `(alias)` for routes where `pid == 0` |
| `src/routes.rs` | In `pid_alive_check`, return `true` for pid=0 (alias sentinel) |
| `src/daemon/ipc.rs` | In `Command::Stop` handler, skip `killpg` when `pid == 0` |

## Testing

- `pid_alive_check(0)` returns `true` (alias is never stale)
- Alias route with pid=0 survives `remove_stale()`
- `portal alias` registers a route accessible via `portal ls`
- `portal alias --remove` removes the route
- `portal alias` without `--force` errors when route exists
- `portal stop` on alias removes route without SIGTERM
