# Storybook Driver Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add a `StorybookDriver` that auto-detects Storybook projects and assigns them a `{project}-storybook.localhost` URL with `--port` CLI injection.

**Architecture:** One new file `src/detect/storybook.rs` with the driver impl. `detect_package_manager` in `src/detect/node.rs` is made `pub(crate)` so `StorybookDriver` can reuse it without duplication. Two lines added to `src/detect/mod.rs` to register the driver.

**Tech Stack:** Rust, serde_json (already a dependency), tempfile (test-only, already used)

---

## File Map

| File | Change |
|---|---|
| `src/detect/storybook.rs` | Create — `StorybookDriver` struct + `LanguageDriver` impl + tests |
| `src/detect/node.rs` | `detect_package_manager` visibility: `fn` → `pub(crate) fn` |
| `src/detect/mod.rs` | Add `pub mod storybook;`, push `StorybookDriver` into `DriverRegistry::new()` |

---

## Task 1: `StorybookDriver` — full implementation in `src/detect/storybook.rs`

**Files:**
- Create: `src/detect/storybook.rs`
- Modify: `src/detect/node.rs` (visibility change only)

### Background

`StorybookDriver` follows the exact same pattern as `NodeDriver` in `src/detect/node.rs`. Three detection signals, package-manager-aware start command, `--port` CLI injection, and a hostname that appends `-storybook` to the project name.

`detect_package_manager` is currently a private function in `node.rs`. Change it to `pub(crate)` so `storybook.rs` can import it directly — no duplication.

- [ ] **Step 1: Make `detect_package_manager` pub(crate) in `src/detect/node.rs`**

Find this line in `src/detect/node.rs`:
```rust
fn detect_package_manager(cwd: &Path) -> &'static str {
```

Change to:
```rust
pub(crate) fn detect_package_manager(cwd: &Path) -> &'static str {
```

- [ ] **Step 2: Run tests to confirm nothing broke**

```bash
cargo test 2>&1 | grep -E "^test result|FAILED"
```

Expected: all tests pass (visibility change only, no logic change).

- [ ] **Step 3: Write failing tests — create `src/detect/storybook.rs` with tests only**

```rust
use std::fs;
use std::path::Path;
use crate::detect::{LanguageDriver, PortInjection};

pub struct StorybookDriver;

impl LanguageDriver for StorybookDriver {
    fn detect(&self, _cwd: &Path) -> bool { false }
    fn priority(&self) -> u8 { 45 }
    fn name(&self) -> &'static str { "Storybook" }
    fn project_name(&self, _cwd: &Path) -> Option<String> { None }
    fn start_command(&self, _cwd: &Path) -> Option<String> { None }
    fn port_injection(&self, _cwd: &Path, port: u16) -> PortInjection {
        PortInjection::CliArgs(vec!["--port".to_string(), port.to_string()])
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn detects_via_storybook_directory() {
        let tmp = TempDir::new().unwrap();
        fs::create_dir(tmp.path().join(".storybook")).unwrap();
        assert!(StorybookDriver.detect(tmp.path()));
    }

    #[test]
    fn detects_via_storybook_script_in_package_json() {
        let tmp = TempDir::new().unwrap();
        fs::write(
            tmp.path().join("package.json"),
            r#"{"scripts":{"storybook":"storybook dev"}}"#,
        ).unwrap();
        assert!(StorybookDriver.detect(tmp.path()));
    }

    #[test]
    fn detects_via_start_storybook_script() {
        let tmp = TempDir::new().unwrap();
        fs::write(
            tmp.path().join("package.json"),
            r#"{"scripts":{"start-storybook":"start-storybook -p 6006"}}"#,
        ).unwrap();
        assert!(StorybookDriver.detect(tmp.path()));
    }

    #[test]
    fn detects_via_storybook_dev_dependency() {
        let tmp = TempDir::new().unwrap();
        fs::write(
            tmp.path().join("package.json"),
            r#"{"devDependencies":{"@storybook/react":"^7.0.0"}}"#,
        ).unwrap();
        assert!(StorybookDriver.detect(tmp.path()));
    }

    #[test]
    fn does_not_detect_plain_node_project() {
        let tmp = TempDir::new().unwrap();
        fs::write(
            tmp.path().join("package.json"),
            r#"{"name":"myapp","scripts":{"dev":"vite"}}"#,
        ).unwrap();
        assert!(!StorybookDriver.detect(tmp.path()));
    }

    #[test]
    fn does_not_detect_empty_directory() {
        let tmp = TempDir::new().unwrap();
        assert!(!StorybookDriver.detect(tmp.path()));
    }

    #[test]
    fn project_name_appends_storybook_suffix() {
        let tmp = TempDir::new().unwrap();
        fs::write(tmp.path().join("package.json"), r#"{"name":"my-app"}"#).unwrap();
        assert_eq!(
            StorybookDriver.project_name(tmp.path()),
            Some("my-app-storybook".to_string()),
        );
    }

    #[test]
    fn project_name_falls_back_to_directory_name() {
        let tmp = TempDir::new().unwrap();
        // No package.json
        let name = StorybookDriver.project_name(tmp.path()).unwrap();
        assert!(name.ends_with("-storybook"), "expected -storybook suffix, got: {name}");
    }

    #[test]
    fn start_command_uses_storybook_script_with_npm() {
        let tmp = TempDir::new().unwrap();
        fs::write(
            tmp.path().join("package.json"),
            r#"{"scripts":{"storybook":"storybook dev"}}"#,
        ).unwrap();
        assert_eq!(
            StorybookDriver.start_command(tmp.path()),
            Some("npm run storybook".to_string()),
        );
    }

    #[test]
    fn start_command_uses_start_storybook_script() {
        let tmp = TempDir::new().unwrap();
        fs::write(
            tmp.path().join("package.json"),
            r#"{"scripts":{"start-storybook":"start-storybook -p 6006"}}"#,
        ).unwrap();
        assert_eq!(
            StorybookDriver.start_command(tmp.path()),
            Some("npm run start-storybook".to_string()),
        );
    }

    #[test]
    fn start_command_prefers_storybook_over_start_storybook() {
        let tmp = TempDir::new().unwrap();
        fs::write(
            tmp.path().join("package.json"),
            r#"{"scripts":{"storybook":"storybook dev","start-storybook":"old"}}"#,
        ).unwrap();
        assert_eq!(
            StorybookDriver.start_command(tmp.path()),
            Some("npm run storybook".to_string()),
        );
    }

    #[test]
    fn start_command_falls_back_to_storybook_dev() {
        let tmp = TempDir::new().unwrap();
        // No package.json — fallback to global install command
        assert_eq!(
            StorybookDriver.start_command(tmp.path()),
            Some("storybook dev".to_string()),
        );
    }

    #[test]
    fn start_command_respects_pnpm_lockfile() {
        let tmp = TempDir::new().unwrap();
        fs::write(tmp.path().join("pnpm-lock.yaml"), "").unwrap();
        fs::write(
            tmp.path().join("package.json"),
            r#"{"scripts":{"storybook":"storybook dev"}}"#,
        ).unwrap();
        assert_eq!(
            StorybookDriver.start_command(tmp.path()),
            Some("pnpm run storybook".to_string()),
        );
    }

    #[test]
    fn port_injection_uses_port_flag() {
        let tmp = TempDir::new().unwrap();
        match StorybookDriver.port_injection(tmp.path(), 6006) {
            PortInjection::CliArgs(args) => {
                assert_eq!(args, vec!["--port", "6006"]);
            }
            other => panic!("expected CliArgs, got {other:?}"),
        }
    }

    #[test]
    fn priority_beats_node_driver() {
        assert!(StorybookDriver.priority() > crate::detect::node::NodeDriver.priority());
    }
}
```

- [ ] **Step 4: Run to verify tests fail**

```bash
cargo test storybook 2>&1 | grep -E "FAILED|error\[" | head -5
```

Expected: compile errors — stub `detect` always returns `false`, several tests fail.

- [ ] **Step 5: Implement `StorybookDriver` fully in `src/detect/storybook.rs`**

Replace the entire file with:

```rust
use std::fs;
use std::path::Path;
use crate::detect::{LanguageDriver, PortInjection};
use crate::detect::node::detect_package_manager;

pub struct StorybookDriver;

impl LanguageDriver for StorybookDriver {
    fn detect(&self, cwd: &Path) -> bool {
        // Signal 1: .storybook/ directory exists
        if cwd.join(".storybook").is_dir() {
            return true;
        }
        let Ok(contents) = fs::read_to_string(cwd.join("package.json")) else {
            return false;
        };
        let Ok(json) = serde_json::from_str::<serde_json::Value>(&contents) else {
            return false;
        };
        // Signal 2: scripts contain "storybook" or "start-storybook"
        if let Some(scripts) = json.get("scripts").and_then(|v| v.as_object()) {
            if scripts.contains_key("storybook") || scripts.contains_key("start-storybook") {
                return true;
            }
        }
        // Signal 3: devDependencies contain any @storybook/ package
        if let Some(dev_deps) = json.get("devDependencies").and_then(|v| v.as_object()) {
            if dev_deps.keys().any(|k| k.starts_with("@storybook/")) {
                return true;
            }
        }
        false
    }

    fn priority(&self) -> u8 { 45 }

    fn name(&self) -> &'static str { "Storybook" }

    fn project_name(&self, cwd: &Path) -> Option<String> {
        let base = crate::detect::read_json_field(cwd, "package.json", "name")
            .or_else(|| {
                cwd.file_name()
                    .and_then(|n| n.to_str())
                    .map(String::from)
            })?;
        Some(format!("{}-storybook", crate::detect::sanitize_hostname(&base)))
    }

    fn start_command(&self, cwd: &Path) -> Option<String> {
        let pm = detect_package_manager(cwd);
        if let Ok(contents) = fs::read_to_string(cwd.join("package.json")) {
            if let Ok(json) = serde_json::from_str::<serde_json::Value>(&contents) {
                if let Some(scripts) = json.get("scripts").and_then(|v| v.as_object()) {
                    if scripts.contains_key("storybook") {
                        return Some(format!("{pm} run storybook"));
                    }
                    if scripts.contains_key("start-storybook") {
                        return Some(format!("{pm} run start-storybook"));
                    }
                }
            }
        }
        Some("storybook dev".to_string())
    }

    fn port_injection(&self, _cwd: &Path, port: u16) -> PortInjection {
        PortInjection::CliArgs(vec!["--port".to_string(), port.to_string()])
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn detects_via_storybook_directory() {
        let tmp = TempDir::new().unwrap();
        fs::create_dir(tmp.path().join(".storybook")).unwrap();
        assert!(StorybookDriver.detect(tmp.path()));
    }

    #[test]
    fn detects_via_storybook_script_in_package_json() {
        let tmp = TempDir::new().unwrap();
        fs::write(
            tmp.path().join("package.json"),
            r#"{"scripts":{"storybook":"storybook dev"}}"#,
        ).unwrap();
        assert!(StorybookDriver.detect(tmp.path()));
    }

    #[test]
    fn detects_via_start_storybook_script() {
        let tmp = TempDir::new().unwrap();
        fs::write(
            tmp.path().join("package.json"),
            r#"{"scripts":{"start-storybook":"start-storybook -p 6006"}}"#,
        ).unwrap();
        assert!(StorybookDriver.detect(tmp.path()));
    }

    #[test]
    fn detects_via_storybook_dev_dependency() {
        let tmp = TempDir::new().unwrap();
        fs::write(
            tmp.path().join("package.json"),
            r#"{"devDependencies":{"@storybook/react":"^7.0.0"}}"#,
        ).unwrap();
        assert!(StorybookDriver.detect(tmp.path()));
    }

    #[test]
    fn does_not_detect_plain_node_project() {
        let tmp = TempDir::new().unwrap();
        fs::write(
            tmp.path().join("package.json"),
            r#"{"name":"myapp","scripts":{"dev":"vite"}}"#,
        ).unwrap();
        assert!(!StorybookDriver.detect(tmp.path()));
    }

    #[test]
    fn does_not_detect_empty_directory() {
        let tmp = TempDir::new().unwrap();
        assert!(!StorybookDriver.detect(tmp.path()));
    }

    #[test]
    fn project_name_appends_storybook_suffix() {
        let tmp = TempDir::new().unwrap();
        fs::write(tmp.path().join("package.json"), r#"{"name":"my-app"}"#).unwrap();
        assert_eq!(
            StorybookDriver.project_name(tmp.path()),
            Some("my-app-storybook".to_string()),
        );
    }

    #[test]
    fn project_name_falls_back_to_directory_name() {
        let tmp = TempDir::new().unwrap();
        let name = StorybookDriver.project_name(tmp.path()).unwrap();
        assert!(name.ends_with("-storybook"), "expected -storybook suffix, got: {name}");
    }

    #[test]
    fn start_command_uses_storybook_script_with_npm() {
        let tmp = TempDir::new().unwrap();
        fs::write(
            tmp.path().join("package.json"),
            r#"{"scripts":{"storybook":"storybook dev"}}"#,
        ).unwrap();
        assert_eq!(
            StorybookDriver.start_command(tmp.path()),
            Some("npm run storybook".to_string()),
        );
    }

    #[test]
    fn start_command_uses_start_storybook_script() {
        let tmp = TempDir::new().unwrap();
        fs::write(
            tmp.path().join("package.json"),
            r#"{"scripts":{"start-storybook":"start-storybook -p 6006"}}"#,
        ).unwrap();
        assert_eq!(
            StorybookDriver.start_command(tmp.path()),
            Some("npm run start-storybook".to_string()),
        );
    }

    #[test]
    fn start_command_prefers_storybook_over_start_storybook() {
        let tmp = TempDir::new().unwrap();
        fs::write(
            tmp.path().join("package.json"),
            r#"{"scripts":{"storybook":"storybook dev","start-storybook":"old"}}"#,
        ).unwrap();
        assert_eq!(
            StorybookDriver.start_command(tmp.path()),
            Some("npm run storybook".to_string()),
        );
    }

    #[test]
    fn start_command_falls_back_to_storybook_dev() {
        let tmp = TempDir::new().unwrap();
        assert_eq!(
            StorybookDriver.start_command(tmp.path()),
            Some("storybook dev".to_string()),
        );
    }

    #[test]
    fn start_command_respects_pnpm_lockfile() {
        let tmp = TempDir::new().unwrap();
        fs::write(tmp.path().join("pnpm-lock.yaml"), "").unwrap();
        fs::write(
            tmp.path().join("package.json"),
            r#"{"scripts":{"storybook":"storybook dev"}}"#,
        ).unwrap();
        assert_eq!(
            StorybookDriver.start_command(tmp.path()),
            Some("pnpm run storybook".to_string()),
        );
    }

    #[test]
    fn port_injection_uses_port_flag() {
        let tmp = TempDir::new().unwrap();
        match StorybookDriver.port_injection(tmp.path(), 6006) {
            PortInjection::CliArgs(args) => {
                assert_eq!(args, vec!["--port", "6006"]);
            }
            other => panic!("expected CliArgs, got {other:?}"),
        }
    }

    #[test]
    fn priority_beats_node_driver() {
        assert!(StorybookDriver.priority() > crate::detect::node::NodeDriver.priority());
    }
}
```

- [ ] **Step 6: Run tests**

```bash
cargo test storybook 2>&1 | grep -E "^test result|FAILED"
```

Expected: all storybook tests pass.

- [ ] **Step 7: Commit**

```bash
git add src/detect/storybook.rs src/detect/node.rs
git commit -m "feat(detect): add StorybookDriver with .storybook dir and devDep detection"
```

---

## Task 2: Register `StorybookDriver` in `DriverRegistry`

**Files:**
- Modify: `src/detect/mod.rs`

### Background

Two lines needed: declare the `storybook` submodule and push `StorybookDriver` into the registry. The registry sorts by priority automatically so insertion order doesn't matter.

- [ ] **Step 1: Write failing integration test**

Add to the `#[cfg(test)]` block in `src/detect/mod.rs`:

```rust
#[test]
fn registry_storybook_beats_node_for_storybook_project() {
    let tmp = TempDir::new().unwrap();
    // Has both package.json (NodeDriver) and .storybook/ (StorybookDriver)
    fs::write(tmp.path().join("package.json"), r#"{"name":"ui","scripts":{"dev":"vite","storybook":"storybook dev"}}"#).unwrap();
    fs::create_dir(tmp.path().join(".storybook")).unwrap();
    let cfg = crate::config::Config::default();
    let reg = DriverRegistry::new(&cfg);
    assert_eq!(reg.detect(tmp.path()).unwrap().name(), "Storybook");
}

#[test]
fn registry_storybook_project_name_has_storybook_suffix() {
    let tmp = TempDir::new().unwrap();
    fs::write(tmp.path().join("package.json"), r#"{"name":"my-ui"}"#).unwrap();
    fs::create_dir(tmp.path().join(".storybook")).unwrap();
    let cfg = crate::config::Config::default();
    let reg = DriverRegistry::new(&cfg);
    let driver = reg.detect(tmp.path()).unwrap();
    assert_eq!(driver.project_name(tmp.path()), Some("my-ui-storybook".to_string()));
}
```

- [ ] **Step 2: Run to verify they fail**

```bash
cargo test registry_storybook 2>&1 | grep -E "FAILED|error\["
```

Expected: compile error — `storybook` module not declared.

- [ ] **Step 3: Register in `src/detect/mod.rs`**

Add the module declaration near the other submodules (around line 175):
```rust
pub mod storybook;
```

Add `StorybookDriver` to `DriverRegistry::new()` (order doesn't matter — registry sorts by priority):
```rust
Box::new(storybook::StorybookDriver),
```

The full updated `DriverRegistry::new()` block:
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
git commit -m "feat(detect): register StorybookDriver in DriverRegistry (priority 45)"
```

---

## Self-Review

**Spec coverage:**
- ✅ Detection via `.storybook/` dir — Task 1 `detects_via_storybook_directory`
- ✅ Detection via `scripts.storybook` — Task 1 `detects_via_storybook_script_in_package_json`
- ✅ Detection via `scripts.start-storybook` — Task 1 `detects_via_start_storybook_script`
- ✅ Detection via `devDependencies @storybook/` — Task 1 `detects_via_storybook_dev_dependency`
- ✅ Priority 45 — Task 1 `priority_beats_node_driver`
- ✅ Start command: `storybook` script → `{pm} run storybook` — Task 1 tests
- ✅ Start command: `start-storybook` → `{pm} run start-storybook` — Task 1 test
- ✅ Start command: fallback → `storybook dev` — Task 1 `start_command_falls_back_to_storybook_dev`
- ✅ Package manager detection (pnpm/bun/yarn/npm) — Task 1 `start_command_respects_pnpm_lockfile`
- ✅ `project_name` appends `-storybook` — Task 1 tests
- ✅ Port injection `CliArgs(["--port", port])` — Task 1 `port_injection_uses_port_flag`
- ✅ Beats NodeDriver in registry — Task 2 `registry_storybook_beats_node_for_storybook_project`

**No placeholders found.**

**Type consistency:** `StorybookDriver` used consistently across both tasks. `detect_package_manager` is `pub(crate)` in node.rs and imported in storybook.rs.
