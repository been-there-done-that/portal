# Docker Compose Driver Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add a `DockerComposeDriver` that auto-detects Docker Compose projects, starts all services via `docker compose up`, and routes a `.localhost` URL to a user-selected service port read directly from the compose file.

**Architecture:** New file `src/detect/docker_compose.rs` for the driver. `service_port_candidates` added as a default-empty method on the `LanguageDriver` trait in `src/detect/mod.rs`. `DockerComposeDriver` overrides it to parse the compose YAML and return `(service_name, host_port)` pairs. `do_run` in `src/cli/mod.rs` is restructured to detect the driver early, call `service_port_candidates`, and show a `dialoguer::Select` picker when multiple services have port mappings. YAML parsing via `serde_yaml` (new dependency).

**Tech Stack:** Rust, serde_yaml 0.9, dialoguer 0.11 (already a dependency)

---

## File Map

| File | Change |
|---|---|
| `Cargo.toml` | Add `serde_yaml = "0.9"` |
| `src/detect/docker_compose.rs` | New — `DockerComposeDriver`, `parse_host_port`, `service_port_candidates`, unit tests |
| `src/detect/mod.rs` | Add `service_port_candidates` to trait; `pub mod docker_compose;`; register driver |
| `src/cli/mod.rs` | Restructure `do_run`: detect driver early, call `service_port_candidates`, picker, use declared port |

---

## Task 1: `DockerComposeDriver` in `src/detect/docker_compose.rs`

**Files:**
- Create: `src/detect/docker_compose.rs`
- Modify: `Cargo.toml`

### Background

`DockerComposeDriver` detects compose projects by filename, starts all services via `docker compose up`, and provides `service_port_candidates()` — a list of `(service_name, host_port)` pairs parsed from the compose YAML. Port injection is empty (`CliArgs(vec![])`) because portal does not inject a port into Docker Compose; the port mapping is declared in the file itself.

`serde_yaml` is not yet a dependency. Add it before writing any code.

- [ ] **Step 1: Add `serde_yaml` to `Cargo.toml`**

In `Cargo.toml`, under `[dependencies]`, add:
```toml
serde_yaml = "0.9"
```

- [ ] **Step 2: Verify it compiles**

```bash
cargo check 2>&1 | grep -E "^error"
```

Expected: no errors.

- [ ] **Step 3: Write failing tests — create `src/detect/docker_compose.rs` with stub impl**

```rust
use std::fs;
use std::path::Path;
use crate::detect::{LanguageDriver, PortInjection};

pub struct DockerComposeDriver;

fn parse_host_port(_val: &serde_yaml::Value) -> Option<u16> { None }

impl LanguageDriver for DockerComposeDriver {
    fn detect(&self, _cwd: &Path) -> bool { false }
    fn priority(&self) -> u8 { 55 }
    fn name(&self) -> &'static str { "Docker Compose" }
    fn project_name(&self, _cwd: &Path) -> Option<String> { None }
    fn start_command(&self, _cwd: &Path) -> Option<String> { None }
    fn port_injection(&self, _cwd: &Path, _port: u16) -> PortInjection {
        PortInjection::CliArgs(vec![])
    }
    fn service_port_candidates(&self, _cwd: &Path) -> Vec<(String, u16)> { vec![] }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    // ── detection ────────────────────────────────────────────────────────────

    #[test]
    fn detects_docker_compose_yml() {
        let tmp = TempDir::new().unwrap();
        fs::write(tmp.path().join("docker-compose.yml"), "services: {}").unwrap();
        assert!(DockerComposeDriver.detect(tmp.path()));
    }

    #[test]
    fn detects_docker_compose_yaml() {
        let tmp = TempDir::new().unwrap();
        fs::write(tmp.path().join("docker-compose.yaml"), "services: {}").unwrap();
        assert!(DockerComposeDriver.detect(tmp.path()));
    }

    #[test]
    fn detects_compose_yml() {
        let tmp = TempDir::new().unwrap();
        fs::write(tmp.path().join("compose.yml"), "services: {}").unwrap();
        assert!(DockerComposeDriver.detect(tmp.path()));
    }

    #[test]
    fn detects_compose_yaml() {
        let tmp = TempDir::new().unwrap();
        fs::write(tmp.path().join("compose.yaml"), "services: {}").unwrap();
        assert!(DockerComposeDriver.detect(tmp.path()));
    }

    #[test]
    fn does_not_detect_empty_directory() {
        let tmp = TempDir::new().unwrap();
        assert!(!DockerComposeDriver.detect(tmp.path()));
    }

    // ── start_command ─────────────────────────────────────────────────────────

    #[test]
    fn start_command_is_docker_compose_up() {
        let tmp = TempDir::new().unwrap();
        assert_eq!(
            DockerComposeDriver.start_command(tmp.path()),
            Some("docker compose up".to_string()),
        );
    }

    // ── port_injection ────────────────────────────────────────────────────────

    #[test]
    fn port_injection_is_empty_cli_args() {
        let tmp = TempDir::new().unwrap();
        match DockerComposeDriver.port_injection(tmp.path(), 3000) {
            PortInjection::CliArgs(args) => assert!(args.is_empty(), "expected empty args, got {args:?}"),
            other => panic!("expected CliArgs, got {other:?}"),
        }
    }

    // ── service_port_candidates ───────────────────────────────────────────────

    #[test]
    fn service_port_candidates_single_service() {
        let tmp = TempDir::new().unwrap();
        fs::write(tmp.path().join("docker-compose.yml"), r#"
services:
  web:
    ports:
      - "3000:80"
"#).unwrap();
        let candidates = DockerComposeDriver.service_port_candidates(tmp.path());
        assert_eq!(candidates, vec![("web".to_string(), 3000u16)]);
    }

    #[test]
    fn service_port_candidates_multiple_services() {
        let tmp = TempDir::new().unwrap();
        fs::write(tmp.path().join("docker-compose.yml"), r#"
services:
  web:
    ports:
      - "3000:80"
  api:
    ports:
      - "8080:8080"
  db:
    image: postgres
"#).unwrap();
        let candidates = DockerComposeDriver.service_port_candidates(tmp.path());
        assert_eq!(candidates.len(), 2);
        assert!(candidates.iter().any(|(n, p)| n == "web" && *p == 3000));
        assert!(candidates.iter().any(|(n, p)| n == "api" && *p == 8080));
    }

    #[test]
    fn service_port_candidates_skips_services_without_ports() {
        let tmp = TempDir::new().unwrap();
        fs::write(tmp.path().join("docker-compose.yml"), r#"
services:
  db:
    image: postgres
  redis:
    image: redis
"#).unwrap();
        assert!(DockerComposeDriver.service_port_candidates(tmp.path()).is_empty());
    }

    #[test]
    fn service_port_candidates_parses_host_colon_container() {
        let tmp = TempDir::new().unwrap();
        fs::write(tmp.path().join("compose.yml"), r#"
services:
  web:
    ports:
      - "3000:80"
"#).unwrap();
        assert_eq!(DockerComposeDriver.service_port_candidates(tmp.path())[0].1, 3000);
    }

    #[test]
    fn service_port_candidates_parses_bare_port() {
        let tmp = TempDir::new().unwrap();
        fs::write(tmp.path().join("compose.yml"), r#"
services:
  web:
    ports:
      - "3000"
"#).unwrap();
        assert_eq!(DockerComposeDriver.service_port_candidates(tmp.path())[0].1, 3000);
    }

    #[test]
    fn service_port_candidates_parses_ip_host_container() {
        let tmp = TempDir::new().unwrap();
        fs::write(tmp.path().join("compose.yml"), r#"
services:
  web:
    ports:
      - "127.0.0.1:3000:80"
"#).unwrap();
        assert_eq!(DockerComposeDriver.service_port_candidates(tmp.path())[0].1, 3000);
    }

    // ── project_name ──────────────────────────────────────────────────────────

    #[test]
    fn project_name_reads_compose_name_field() {
        let tmp = TempDir::new().unwrap();
        fs::write(tmp.path().join("docker-compose.yml"), r#"
name: my-app
services: {}
"#).unwrap();
        assert_eq!(DockerComposeDriver.project_name(tmp.path()), Some("my-app".to_string()));
    }

    #[test]
    fn project_name_falls_back_to_directory_name() {
        let tmp = TempDir::new().unwrap();
        fs::write(tmp.path().join("docker-compose.yml"), "services: {}").unwrap();
        assert!(DockerComposeDriver.project_name(tmp.path()).is_some());
    }

    // ── priority ──────────────────────────────────────────────────────────────

    #[test]
    fn priority_beats_node_driver() {
        assert!(DockerComposeDriver.priority() > crate::detect::node::NodeDriver.priority());
    }
}
```

- [ ] **Step 4: Run to verify tests fail**

```bash
cargo test docker_compose 2>&1 | grep -E "FAILED|error\[" | head -10
```

Expected: multiple failures — stub `detect` returns `false`, `start_command` returns `None`, candidates always empty.

- [ ] **Step 5: Implement `DockerComposeDriver` fully**

Replace the entire `src/detect/docker_compose.rs` with:

```rust
use std::fs;
use std::path::Path;
use crate::detect::{LanguageDriver, PortInjection};

pub struct DockerComposeDriver;

/// Compose file names to check, in priority order.
const COMPOSE_FILES: &[&str] = &[
    "docker-compose.yml",
    "docker-compose.yaml",
    "compose.yml",
    "compose.yaml",
];

/// Parse the host (published) port from a single entry in a compose `ports:` list.
///
/// Handles three short-syntax formats:
///   - `"3000:80"`          → 3000  (host:container)
///   - `"127.0.0.1:3000:80"` → 3000 (ip:host:container)
///   - `"3000"`             → 3000  (bare port)
///   - `3000`               (YAML integer) → 3000
///
/// Also handles long syntax: `{published: 3000, target: 80}`.
fn parse_host_port(val: &serde_yaml::Value) -> Option<u16> {
    if let Some(s) = val.as_str() {
        let parts: Vec<&str> = s.split(':').collect();
        return match parts.len() {
            1 => parts[0].trim().parse().ok(),
            2 => parts[0].trim().parse().ok(),   // "host:container"
            3 => parts[1].trim().parse().ok(),   // "ip:host:container"
            _ => None,
        };
    }
    if let Some(n) = val.as_u64() {
        return u16::try_from(n).ok();
    }
    // Long syntax: {published: 3000, target: 80}
    if let Some(map) = val.as_mapping() {
        for (k, v) in map {
            if k.as_str() == Some("published") {
                if let Some(n) = v.as_u64() {
                    return u16::try_from(n).ok();
                }
                if let Some(s) = v.as_str() {
                    return s.trim().parse().ok();
                }
            }
        }
    }
    None
}

/// Read and parse the first compose file found in `cwd`.
fn read_compose_yaml(cwd: &Path) -> Option<serde_yaml::Value> {
    for name in COMPOSE_FILES {
        if let Ok(contents) = fs::read_to_string(cwd.join(name)) {
            if let Ok(yaml) = serde_yaml::from_str::<serde_yaml::Value>(&contents) {
                return Some(yaml);
            }
        }
    }
    None
}

impl LanguageDriver for DockerComposeDriver {
    fn detect(&self, cwd: &Path) -> bool {
        COMPOSE_FILES.iter().any(|name| cwd.join(name).is_file())
    }

    fn priority(&self) -> u8 { 55 }

    fn name(&self) -> &'static str { "Docker Compose" }

    fn project_name(&self, cwd: &Path) -> Option<String> {
        // Prefer the top-level `name:` field in the compose file
        if let Some(yaml) = read_compose_yaml(cwd) {
            if let Some(name) = yaml.get("name").and_then(|v| v.as_str()) {
                if !name.is_empty() {
                    return Some(crate::detect::sanitize_hostname(name));
                }
            }
        }
        // Fallback: directory name
        cwd.file_name()
            .and_then(|n| n.to_str())
            .map(crate::detect::sanitize_hostname)
    }

    fn start_command(&self, _cwd: &Path) -> Option<String> {
        Some("docker compose up".to_string())
    }

    /// No port injection — Docker Compose manages its own port bindings.
    fn port_injection(&self, _cwd: &Path, _port: u16) -> PortInjection {
        PortInjection::CliArgs(vec![])
    }

    /// Return every service that has a `ports:` mapping as `(service_name, host_port)`.
    /// The first port entry of each service is used.
    fn service_port_candidates(&self, cwd: &Path) -> Vec<(String, u16)> {
        let yaml = match read_compose_yaml(cwd) {
            Some(y) => y,
            None => return vec![],
        };
        let services = match yaml.get("services").and_then(|v| v.as_mapping()) {
            Some(s) => s,
            None => return vec![],
        };
        let mut candidates = Vec::new();
        for (key, service) in services {
            let name = match key.as_str() {
                Some(s) => s.to_string(),
                None => continue,
            };
            if let Some(ports) = service.get("ports").and_then(|v| v.as_sequence()) {
                if let Some(first) = ports.first() {
                    if let Some(port) = parse_host_port(first) {
                        candidates.push((name, port));
                    }
                }
            }
        }
        candidates
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    // ── detection ────────────────────────────────────────────────────────────

    #[test]
    fn detects_docker_compose_yml() {
        let tmp = TempDir::new().unwrap();
        fs::write(tmp.path().join("docker-compose.yml"), "services: {}").unwrap();
        assert!(DockerComposeDriver.detect(tmp.path()));
    }

    #[test]
    fn detects_docker_compose_yaml() {
        let tmp = TempDir::new().unwrap();
        fs::write(tmp.path().join("docker-compose.yaml"), "services: {}").unwrap();
        assert!(DockerComposeDriver.detect(tmp.path()));
    }

    #[test]
    fn detects_compose_yml() {
        let tmp = TempDir::new().unwrap();
        fs::write(tmp.path().join("compose.yml"), "services: {}").unwrap();
        assert!(DockerComposeDriver.detect(tmp.path()));
    }

    #[test]
    fn detects_compose_yaml() {
        let tmp = TempDir::new().unwrap();
        fs::write(tmp.path().join("compose.yaml"), "services: {}").unwrap();
        assert!(DockerComposeDriver.detect(tmp.path()));
    }

    #[test]
    fn does_not_detect_empty_directory() {
        let tmp = TempDir::new().unwrap();
        assert!(!DockerComposeDriver.detect(tmp.path()));
    }

    // ── start_command ─────────────────────────────────────────────────────────

    #[test]
    fn start_command_is_docker_compose_up() {
        let tmp = TempDir::new().unwrap();
        assert_eq!(
            DockerComposeDriver.start_command(tmp.path()),
            Some("docker compose up".to_string()),
        );
    }

    // ── port_injection ────────────────────────────────────────────────────────

    #[test]
    fn port_injection_is_empty_cli_args() {
        let tmp = TempDir::new().unwrap();
        match DockerComposeDriver.port_injection(tmp.path(), 3000) {
            PortInjection::CliArgs(args) => assert!(args.is_empty(), "expected empty args, got {args:?}"),
            other => panic!("expected CliArgs, got {other:?}"),
        }
    }

    // ── service_port_candidates ───────────────────────────────────────────────

    #[test]
    fn service_port_candidates_single_service() {
        let tmp = TempDir::new().unwrap();
        fs::write(tmp.path().join("docker-compose.yml"), r#"
services:
  web:
    ports:
      - "3000:80"
"#).unwrap();
        let candidates = DockerComposeDriver.service_port_candidates(tmp.path());
        assert_eq!(candidates, vec![("web".to_string(), 3000u16)]);
    }

    #[test]
    fn service_port_candidates_multiple_services() {
        let tmp = TempDir::new().unwrap();
        fs::write(tmp.path().join("docker-compose.yml"), r#"
services:
  web:
    ports:
      - "3000:80"
  api:
    ports:
      - "8080:8080"
  db:
    image: postgres
"#).unwrap();
        let candidates = DockerComposeDriver.service_port_candidates(tmp.path());
        assert_eq!(candidates.len(), 2);
        assert!(candidates.iter().any(|(n, p)| n == "web" && *p == 3000));
        assert!(candidates.iter().any(|(n, p)| n == "api" && *p == 8080));
    }

    #[test]
    fn service_port_candidates_skips_services_without_ports() {
        let tmp = TempDir::new().unwrap();
        fs::write(tmp.path().join("docker-compose.yml"), r#"
services:
  db:
    image: postgres
  redis:
    image: redis
"#).unwrap();
        assert!(DockerComposeDriver.service_port_candidates(tmp.path()).is_empty());
    }

    #[test]
    fn service_port_candidates_parses_host_colon_container() {
        let tmp = TempDir::new().unwrap();
        fs::write(tmp.path().join("compose.yml"), r#"
services:
  web:
    ports:
      - "3000:80"
"#).unwrap();
        assert_eq!(DockerComposeDriver.service_port_candidates(tmp.path())[0].1, 3000);
    }

    #[test]
    fn service_port_candidates_parses_bare_port() {
        let tmp = TempDir::new().unwrap();
        fs::write(tmp.path().join("compose.yml"), r#"
services:
  web:
    ports:
      - "3000"
"#).unwrap();
        assert_eq!(DockerComposeDriver.service_port_candidates(tmp.path())[0].1, 3000);
    }

    #[test]
    fn service_port_candidates_parses_ip_host_container() {
        let tmp = TempDir::new().unwrap();
        fs::write(tmp.path().join("compose.yml"), r#"
services:
  web:
    ports:
      - "127.0.0.1:3000:80"
"#).unwrap();
        assert_eq!(DockerComposeDriver.service_port_candidates(tmp.path())[0].1, 3000);
    }

    // ── project_name ──────────────────────────────────────────────────────────

    #[test]
    fn project_name_reads_compose_name_field() {
        let tmp = TempDir::new().unwrap();
        fs::write(tmp.path().join("docker-compose.yml"), r#"
name: my-app
services: {}
"#).unwrap();
        assert_eq!(DockerComposeDriver.project_name(tmp.path()), Some("my-app".to_string()));
    }

    #[test]
    fn project_name_falls_back_to_directory_name() {
        let tmp = TempDir::new().unwrap();
        fs::write(tmp.path().join("docker-compose.yml"), "services: {}").unwrap();
        assert!(DockerComposeDriver.project_name(tmp.path()).is_some());
    }

    // ── priority ──────────────────────────────────────────────────────────────

    #[test]
    fn priority_beats_node_driver() {
        assert!(DockerComposeDriver.priority() > crate::detect::node::NodeDriver.priority());
    }
}
```

- [ ] **Step 6: Run tests**

```bash
cargo test docker_compose 2>&1 | grep -E "^test result|FAILED"
```

Expected: all docker_compose tests pass.

- [ ] **Step 7: Commit**

```bash
git add Cargo.toml Cargo.lock src/detect/docker_compose.rs
git commit -m "feat(detect): add DockerComposeDriver with YAML port parsing"
```

---

## Task 2: Register `DockerComposeDriver` in `DriverRegistry` + add `service_port_candidates` to trait

**Files:**
- Modify: `src/detect/mod.rs`

### Background

Two changes needed in `src/detect/mod.rs`:
1. Add `service_port_candidates` as a default method on the `LanguageDriver` trait (default returns `vec![]` so existing drivers are unaffected).
2. Register `DockerComposeDriver` and declare the module.

The trait method must be added before Task 3 uses it from `do_run`.

- [ ] **Step 1: Write failing integration tests**

Add to the `#[cfg(test)]` block at the bottom of `src/detect/mod.rs`:

```rust
#[test]
fn registry_docker_compose_beats_node_for_compose_project() {
    let tmp = TempDir::new().unwrap();
    fs::write(tmp.path().join("package.json"), r#"{"name":"app"}"#).unwrap();
    fs::write(tmp.path().join("docker-compose.yml"), "services: {}").unwrap();
    let cfg = crate::config::Config::default();
    let reg = DriverRegistry::new(&cfg);
    assert_eq!(reg.detect(tmp.path()).unwrap().name(), "Docker Compose");
}

#[test]
fn registry_docker_compose_service_port_candidates_via_trait() {
    let tmp = TempDir::new().unwrap();
    fs::write(tmp.path().join("docker-compose.yml"), r#"
services:
  web:
    ports:
      - "3000:80"
"#).unwrap();
    let cfg = crate::config::Config::default();
    let reg = DriverRegistry::new(&cfg);
    let driver = reg.detect(tmp.path()).unwrap();
    let candidates = driver.service_port_candidates(tmp.path());
    assert_eq!(candidates, vec![("web".to_string(), 3000u16)]);
}
```

- [ ] **Step 2: Run to verify they fail**

```bash
cargo test registry_docker_compose 2>&1 | grep -E "FAILED|error\["
```

Expected: compile error — `docker_compose` module not declared, `service_port_candidates` not on trait.

- [ ] **Step 3: Add trait method, module, and register driver**

In `src/detect/mod.rs`:

**a) Add `service_port_candidates` to the `LanguageDriver` trait** (after `port_injection`):

```rust
pub trait LanguageDriver: Send + Sync {
    fn detect(&self, cwd: &Path) -> bool;
    fn priority(&self) -> u8 { 50 }
    fn name(&self) -> &'static str;
    fn project_name(&self, cwd: &Path) -> Option<String>;
    fn start_command(&self, cwd: &Path) -> Option<String>;
    fn port_injection(&self, cwd: &Path, port: u16) -> PortInjection;
    /// Return (service_name, host_port) pairs for services that declare port mappings.
    /// Default is empty — most drivers do not use this.
    fn service_port_candidates(&self, _cwd: &Path) -> Vec<(String, u16)> { vec![] }
}
```

**b) Add `pub mod docker_compose;`** to the sub-modules block (around line 176, after `pub mod storybook;`):

```rust
pub mod docker_compose;
```

**c) Add `Box::new(docker_compose::DockerComposeDriver)` to `DriverRegistry::new()`** (before `storybook::StorybookDriver`):

```rust
pub fn new(config: &crate::config::Config) -> Self {
    let mut reg = Self {
        drivers: vec![
            Box::new(PortalTomlDriver { config: config.project.clone() }),
            Box::new(python::DjangoDriver),
            Box::new(python::UvicornDriver),
            Box::new(python::FlaskDriver),
            Box::new(ruby::RailsDriver),
            Box::new(ruby::RackDriver),
            Box::new(php::PhpDriver),
            Box::new(go::GoDriver),
            Box::new(rust::RustDriver),
            Box::new(docker_compose::DockerComposeDriver),
            Box::new(storybook::StorybookDriver),
            Box::new(node::NodeDriver),
        ],
    };
    reg.sort();
    reg
}
```

- [ ] **Step 4: Run all tests**

```bash
cargo test 2>&1 | grep -E "^test result|FAILED"
```

Expected: all tests pass.

- [ ] **Step 5: Commit**

```bash
git add src/detect/mod.rs
git commit -m "feat(detect): register DockerComposeDriver; add service_port_candidates to trait"
```

---

## Task 3: CLI picker in `do_run`

**Files:**
- Modify: `src/cli/mod.rs`

### Background

`do_run` currently detects the driver inside a block scoped to building `injection` (around line 401). The change restructures this so:
1. The registry and driver are detected **before** port determination (currently line 375).
2. `service_port_candidates` is called on the driver to get `(name, port)` pairs.
3. If 0 candidates → normal pool allocation (no change).
4. If 1 candidate → use that port directly, skip pool.
5. If 2+ candidates → show a `dialoguer::Select` picker; use chosen port.
6. The old `injection` block (which created its own inner registry) is replaced with a single call using the already-detected driver.

The relevant section of `do_run` spans approximately lines 355–411 in the current file.

- [ ] **Step 1: Locate the section to replace**

Read `src/cli/mod.rs` lines 355–415 to confirm the exact current code before editing.

The section to replace begins with the comment `// Determine backend port:` and ends just after the `injection` block. It currently looks like:

```rust
    // Determine backend port:
    //   1. User pinned --port  → use it (stop old if exists)
    //   2. Existing route      → stop old, reuse its port
    //   3. No existing route   → find a free port
    let port = if let Some(explicit_port) = port_override {
        if let Some(_old_port) = reuse_port {
            let mut s = ipc_connect().await?;
            write_frame(&mut s, &Command::Stop { hostname: hostname.clone() }).await?;
            let _: crate::proto::Response = read_frame(&mut s).await?;
            crate::ports::wait_for_port_free(
                explicit_port,
                std::time::Duration::from_secs(2),
            )
            .await;
        }
        explicit_port
    } else if let Some(old_port) = reuse_port {
        let mut s = ipc_connect().await?;
        write_frame(&mut s, &Command::Stop { hostname: hostname.clone() }).await?;
        let _: crate::proto::Response = read_frame(&mut s).await?;
        crate::ports::wait_for_port_free(old_port, std::time::Duration::from_secs(2))
            .await;
        old_port
    } else {
        crate::ports::find_free_port(
            config.proxy.port_range.0,
            config.proxy.port_range.1,
        )?
    };

    let injection = {
        let registry = crate::detect::DriverRegistry::new(&config);
        let driver = if use_full_registry {
            registry.detect(&cwd)
        } else {
            registry.detect_language(&cwd)
        };
        driver
            .map(|d| d.port_injection(&cwd, port))
            .unwrap_or(crate::detect::PortInjection::EnvOnly)
    };
```

- [ ] **Step 2: Replace that section with the new implementation**

Replace the section identified in Step 1 with:

```rust
    // Detect driver early — needed for service_port_candidates and injection.
    let registry = crate::detect::DriverRegistry::new(&config);
    let driver: Option<&dyn crate::detect::LanguageDriver> = if use_full_registry {
        registry.detect(&cwd)
    } else {
        registry.detect_language(&cwd)
    };

    // Check for service-declared port candidates (e.g., Docker Compose).
    // If candidates are present portal uses the declared port and skips pool allocation.
    let declared_port: Option<u16> = {
        let candidates: Vec<(String, u16)> = driver
            .map(|d| d.service_port_candidates(&cwd))
            .unwrap_or_default();
        match candidates.len() {
            0 => None,
            1 => Some(candidates[0].1),
            _ => {
                use std::io::IsTerminal;
                let labels: Vec<String> = candidates
                    .iter()
                    .map(|(name, port)| format!("{name} → {port}"))
                    .collect();
                let idx = if std::io::stdin().is_terminal() {
                    dialoguer::Select::new()
                        .with_prompt("Multiple services found. Which should portal proxy to?")
                        .items(&labels)
                        .default(0)
                        .interact()
                        .unwrap_or(0)
                } else {
                    eprintln!("Multiple services found, selecting first: {}", labels[0]);
                    0
                };
                Some(candidates[idx].1)
            }
        }
    };

    // Determine backend port:
    //   1. User pinned --port       → use it (stop old if exists)
    //   2. Driver declared port     → use it directly (skip pool)
    //   3. Existing route           → stop old, reuse its port
    //   4. No existing route        → find a free port
    let port = if let Some(explicit_port) = port_override {
        if let Some(_old_port) = reuse_port {
            let mut s = ipc_connect().await?;
            write_frame(&mut s, &Command::Stop { hostname: hostname.clone() }).await?;
            let _: crate::proto::Response = read_frame(&mut s).await?;
            crate::ports::wait_for_port_free(
                explicit_port,
                std::time::Duration::from_secs(2),
            )
            .await;
        }
        explicit_port
    } else if let Some(dp) = declared_port {
        dp
    } else if let Some(old_port) = reuse_port {
        let mut s = ipc_connect().await?;
        write_frame(&mut s, &Command::Stop { hostname: hostname.clone() }).await?;
        let _: crate::proto::Response = read_frame(&mut s).await?;
        crate::ports::wait_for_port_free(old_port, std::time::Duration::from_secs(2))
            .await;
        old_port
    } else {
        crate::ports::find_free_port(
            config.proxy.port_range.0,
            config.proxy.port_range.1,
        )?
    };

    let injection = driver
        .map(|d| d.port_injection(&cwd, port))
        .unwrap_or(crate::detect::PortInjection::EnvOnly);
```

- [ ] **Step 3: Build and run all tests**

```bash
cargo test 2>&1 | grep -E "^test result|FAILED"
```

Expected: all tests pass. (The CLI picker logic is an interactive flow not covered by unit tests; it is verified by the compile check and manual testing against a real Docker Compose project.)

- [ ] **Step 4: Commit**

```bash
git add src/cli/mod.rs
git commit -m "feat(cli): detect driver early; use service_port_candidates for Docker Compose port routing"
```

---

## Self-Review

**Spec coverage:**

- ✅ Detects all four compose file names — Task 1 `detects_docker_compose_yml/yaml`, `detects_compose_yml/yaml`
- ✅ `detect()` false for empty dir — Task 1 `does_not_detect_empty_directory`
- ✅ Priority 55 (above NodeDriver 40, StorybookDriver 45) — Task 1 `priority_beats_node_driver`; registry sorting in Task 2
- ✅ `start_command` always `"docker compose up"` — Task 1 `start_command_is_docker_compose_up`
- ✅ `port_injection` returns empty `CliArgs` — Task 1 `port_injection_is_empty_cli_args`
- ✅ `service_port_candidates`: single service, multi-service, skips no-ports services — Task 1 tests
- ✅ Port string formats: `host:container`, bare, `ip:host:container` — Task 1 tests
- ✅ `project_name` reads compose `name:` field — Task 1 `project_name_reads_compose_name_field`
- ✅ `project_name` falls back to directory name — Task 1 `project_name_falls_back_to_directory_name`
- ✅ `service_port_candidates` on trait default = `vec![]` — Task 2 trait definition
- ✅ Registry: DockerCompose beats Node for compose+package.json project — Task 2 integration test
- ✅ `service_port_candidates` accessible via trait from registry — Task 2 integration test
- ✅ CLI picker: 0/1/N candidates handled — Task 3 implementation
- ✅ Pool allocation skipped when declared port present — Task 3 `declared_port` branch

**No placeholders found.**

**Type consistency:** `service_port_candidates` returns `Vec<(String, u16)>` consistently across trait definition (Task 2), driver impl (Task 1), and CLI usage (Task 3). `DockerComposeDriver` used by name consistently throughout.
