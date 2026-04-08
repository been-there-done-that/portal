# Group B-B — Docker Compose Driver Design

**Goal:** Add a `DockerComposeDriver` that auto-detects Docker Compose projects, starts all services via `docker compose up`, and routes traffic through a `.localhost` URL by reading the proxy port directly from the compose file.

**Architecture:** One new file `src/detect/docker_compose.rs`. A `service_port_candidates()` method added to the `LanguageDriver` trait (default: empty vec). Picker logic added to `do_run` in `src/cli/mod.rs`. No new `PortInjection` variant needed.

---

## Detection

`DockerComposeDriver::detect(cwd)` returns `true` when any of the following files exist in `cwd`:

1. `docker-compose.yml`
2. `docker-compose.yaml`
3. `compose.yml`
4. `compose.yaml`

All checks are file-local (no network, no subprocess).

## Priority

**55** — above StorybookDriver (45) and NodeDriver (40). A Docker Compose project is more specifically typed than a generic Node project, and Storybook projects rarely use Compose as their primary entrypoint.

## Start Command

Always `docker compose up`. No service filter, no detach flag — portal lets the process own its lifecycle like other drivers.

`start_command` returns `Some("docker compose up".to_string())`.

## Port Handling

### `service_port_candidates`

A new method is added to the `LanguageDriver` trait:

```rust
fn service_port_candidates(&self, cwd: &Path) -> Vec<(String, u16)> {
    vec![]
}
```

`DockerComposeDriver` overrides this to parse the compose yaml and return every service that has a `ports:` mapping as `(service_name, host_port)`.

Port string formats handled:
- `"3000:80"` → host port 3000
- `"3000"` → host port 3000
- `"127.0.0.1:3000:80"` → host port 3000

The first port entry in each service's `ports` list is used.

### `port_injection`

Returns `CliArgs(vec![])` — nothing injected. Docker Compose manages its own port bindings; portal does not pass a port to the process.

### CLI Picker (in `do_run`)

After driver detection, `do_run` calls `driver.service_port_candidates(cwd)`:

- **0 candidates** — fall back to normal pool allocation + `EnvOnly`. User is responsible for port configuration.
- **1 candidate** — use it automatically. Skip pool allocation. Store `(hostname, host_port)` in the state store.
- **2+ candidates** — show an interactive numbered prompt listing service names and ports. User picks one. That port is used as the proxy target, pool allocation skipped.

Example picker output:
```
Multiple services found with port mappings:
  1. web    → 3000
  2. api    → 8080

Which service should portal proxy to? [1]:
```

## `project_name`

1. Read the top-level `name:` field from the compose yaml → use as-is (already a slug in practice)
2. Fallback: `cwd` directory name

No suffix appended (unlike StorybookDriver).

## Files Changed

| File | Change |
|---|---|
| `src/detect/docker_compose.rs` | New — `DockerComposeDriver` struct, `LanguageDriver` impl, `service_port_candidates`, unit tests |
| `src/detect/mod.rs` | Add `service_port_candidates` to `LanguageDriver` trait; add `pub mod docker_compose;`; push `DockerComposeDriver` into `DriverRegistry::new()` |
| `src/cli/mod.rs` | Picker logic in `do_run`: call `service_port_candidates`, handle 0/1/N cases |

## Testing

**`docker_compose.rs` unit tests:**
- `detect()` returns true for each of the four compose file names independently
- `detect()` returns false when none exist
- `service_port_candidates()` returns correct `(name, port)` for a single-service compose file
- `service_port_candidates()` returns all services with port mappings in a multi-service file
- `service_port_candidates()` skips services with no `ports:` entry
- `service_port_candidates()` parses `"host:container"`, bare `"port"`, and `"ip:host:container"` formats
- `project_name()` reads `name:` from compose yaml
- `project_name()` falls back to directory name
- `port_injection()` returns `CliArgs(vec![])`
- `priority()` returns 55 (above NodeDriver 40 and StorybookDriver 45)

**`mod.rs` integration tests:**
- Registry picks `DockerComposeDriver` over `NodeDriver` for a project with both `package.json` and `docker-compose.yml`
- `service_port_candidates` integration test via the registry

**`cli/mod.rs` tests:**
- `do_run` with 1 candidate uses the declared port (no pool allocation)
- Picker prompt shown when 2+ candidates present
