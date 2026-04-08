# Multi-Language Support Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Replace the ad-hoc `Framework` enum in `detect.rs` with a trait-based `LanguageDriver` registry supporting Python, Go, Ruby, Rust, PHP, and Node out of the box, with `portal.toml` as a universal override and a new `portal init` command for guided setup.

**Architecture:** `LanguageDriver` trait + `DriverRegistry` in a new `src/detect/` module replaces `src/detect.rs`. Each language is one file. `PortalTomlDriver` (priority 255) always wins. `portal start` becomes fully language-agnostic. `process::spawn_child` receives a `PortInjection` enum instead of calling `extra_args_for_port`. Clean break — no backward-compatibility shims.

**Tech Stack:** Rust, `toml` crate (already present) for manifest parsing, `dialoguer` (already present) for `portal init` interactive prompts, `serde_json` (already present) for `composer.json`, `tempfile` (already present) for tests.

---

## File Map

| Action | Path | Responsibility |
|---|---|---|
| Create | `src/detect/mod.rs` | `LanguageDriver` trait, `PortInjection`, `DriverRegistry`, utility fns |
| Create | `src/detect/node.rs` | `NodeDriver` — all JS/Node logic |
| Create | `src/detect/python.rs` | `DjangoDriver`, `UvicornDriver`, `FlaskDriver` |
| Create | `src/detect/go.rs` | `GoDriver` |
| Create | `src/detect/ruby.rs` | `RailsDriver`, `RackDriver` |
| Create | `src/detect/rust.rs` | `RustDriver` |
| Create | `src/detect/php.rs` | `PhpDriver` |
| Modify | `src/config.rs` | Add `start_command`, `port_arg`, `host_arg`, `port_position` to `ProjectConfig` |
| Modify | `src/process.rs` | `spawn_child` accepts `PortInjection` instead of calling `extra_args_for_port` |
| Modify | `src/cli/mod.rs` | Add `CliCommand::Init`, rewrite `CliCommand::Start`, update `do_run` |
| Delete | `src/detect.rs` | Replaced by `src/detect/` module |

---

## Task 1: Core abstractions — `LanguageDriver` trait, `PortInjection`, skeleton `DriverRegistry`

**Files:**
- Create: `src/detect/mod.rs`

- [ ] **Step 1: Write the failing test**

```rust
// src/detect/mod.rs — add at bottom
#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    struct AlwaysDriver;
    impl LanguageDriver for AlwaysDriver {
        fn detect(&self, _: &Path) -> bool { true }
        fn name(&self) -> &'static str { "always" }
        fn project_name(&self, _: &Path) -> Option<String> { None }
        fn start_command(&self, _: &Path) -> Option<String> { Some("echo hi".to_string()) }
        fn port_injection(&self, _: &Path, port: u16) -> PortInjection {
            PortInjection::CliArgs(vec!["--port".to_string(), port.to_string()])
        }
    }

    struct NeverDriver;
    impl LanguageDriver for NeverDriver {
        fn detect(&self, _: &Path) -> bool { false }
        fn name(&self) -> &'static str { "never" }
        fn project_name(&self, _: &Path) -> Option<String> { None }
        fn start_command(&self, _: &Path) -> Option<String> { None }
        fn port_injection(&self, _: &Path, _: u16) -> PortInjection { PortInjection::EnvOnly }
    }

    #[test]
    fn registry_returns_none_when_no_match() {
        let reg = DriverRegistry { drivers: vec![Box::new(NeverDriver)] };
        let tmp = TempDir::new().unwrap();
        assert!(reg.detect(tmp.path()).is_none());
    }

    #[test]
    fn registry_returns_first_matching_driver() {
        let reg = DriverRegistry {
            drivers: vec![Box::new(NeverDriver), Box::new(AlwaysDriver)],
        };
        let tmp = TempDir::new().unwrap();
        assert_eq!(reg.detect(tmp.path()).unwrap().name(), "always");
    }

    #[test]
    fn registry_respects_priority_order() {
        struct PrioDriver(u8, &'static str);
        impl LanguageDriver for PrioDriver {
            fn detect(&self, _: &Path) -> bool { true }
            fn priority(&self) -> u8 { self.0 }
            fn name(&self) -> &'static str { self.1 }
            fn project_name(&self, _: &Path) -> Option<String> { None }
            fn start_command(&self, _: &Path) -> Option<String> { None }
            fn port_injection(&self, _: &Path, _: u16) -> PortInjection { PortInjection::EnvOnly }
        }
        let mut reg = DriverRegistry {
            drivers: vec![
                Box::new(PrioDriver(40, "low")),
                Box::new(PrioDriver(90, "high")),
            ],
        };
        reg.sort();
        let tmp = TempDir::new().unwrap();
        assert_eq!(reg.detect(tmp.path()).unwrap().name(), "high");
    }

    #[test]
    fn sanitize_hostname_works() {
        assert_eq!(sanitize_hostname("My App"), "my-app");
        assert_eq!(sanitize_hostname("feature/login"), "feature-login");
        assert_eq!(sanitize_hostname("api_v2"), "api-v2");
        assert_eq!(sanitize_hostname("UPPERCASE"), "uppercase");
    }
}
```

- [ ] **Step 2: Run test to verify it fails**

```bash
cargo test detect 2>&1 | tail -20
```

Expected: compile error — `LanguageDriver`, `DriverRegistry`, `PortInjection`, `sanitize_hostname` not found.

- [ ] **Step 3: Create `src/detect/mod.rs`**

```rust
use std::path::{Path, PathBuf};
use std::fs;

// ─── Trait ───────────────────────────────────────────────────────────────────

pub trait LanguageDriver: Send + Sync {
    /// Returns true if this driver recognises the project at `cwd`.
    fn detect(&self, cwd: &Path) -> bool;
    /// Higher value = checked first. Default 50.
    fn priority(&self) -> u8 { 50 }
    /// Short label shown in `portal init` output, e.g. "Django (Python)".
    fn name(&self) -> &'static str;
    /// Project name from the language manifest, e.g. package.json "name".
    fn project_name(&self, cwd: &Path) -> Option<String>;
    /// Default dev-server command, e.g. "python manage.py runserver".
    fn start_command(&self, cwd: &Path) -> Option<String>;
    /// How to inject the assigned port into the child process.
    fn port_injection(&self, cwd: &Path, port: u16) -> PortInjection;
}

// ─── Port injection strategy ─────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub enum PortInjection {
    /// Set PORT env var only (always done regardless; this adds nothing extra).
    EnvOnly,
    /// Append CLI flags, e.g. ["--port", "4123", "--host", "0.0.0.0"].
    CliArgs(Vec<String>),
    /// Append a host:port address as a positional arg, e.g. "0.0.0.0:4123".
    AppendAddress(String),
}

// ─── Registry ────────────────────────────────────────────────────────────────

pub struct DriverRegistry {
    pub(crate) drivers: Vec<Box<dyn LanguageDriver>>,
}

impl DriverRegistry {
    /// Sort drivers highest-priority first. Call after constructing the registry.
    pub fn sort(&mut self) {
        self.drivers.sort_by(|a, b| b.priority().cmp(&a.priority()));
    }

    /// First driver whose `detect()` returns true (after sorting).
    pub fn detect(&self, cwd: &Path) -> Option<&dyn LanguageDriver> {
        self.drivers.iter().find(|d| d.detect(cwd)).map(|d| d.as_ref())
    }

    /// Same as `detect` but skips the `portal.toml` driver — used by `portal init`.
    pub fn detect_language(&self, cwd: &Path) -> Option<&dyn LanguageDriver> {
        self.drivers
            .iter()
            .filter(|d| d.name() != "portal.toml")
            .find(|d| d.detect(cwd))
            .map(|d| d.as_ref())
    }
}

// ─── Utility functions (shared by all drivers) ───────────────────────────────

/// Sanitize a string into a valid lowercase hostname segment.
pub fn sanitize_hostname(s: &str) -> String {
    let lower = s.to_lowercase();
    let mut result = String::new();
    for c in lower.chars() {
        if c.is_ascii_alphanumeric() {
            result.push(c);
        } else {
            result.push('-');
        }
    }
    while result.contains("--") {
        result = result.replace("--", "-");
    }
    result.trim_matches('-').to_string()
}

/// Infer project name from cwd, checking all known manifests.
pub fn infer_project_name(cwd: &Path, override_name: Option<&str>) -> String {
    if let Some(name) = override_name {
        return sanitize_hostname(name);
    }
    // package.json
    if let Some(n) = read_json_field(cwd, "package.json", "name") {
        if !n.is_empty() { return sanitize_hostname(&n); }
    }
    // pyproject.toml [project].name
    if let Some(n) = read_toml_field(cwd, "pyproject.toml", &["project", "name"]) {
        if !n.is_empty() { return sanitize_hostname(&n); }
    }
    // Cargo.toml [package].name
    if let Some(n) = read_toml_field(cwd, "Cargo.toml", &["package", "name"]) {
        if !n.is_empty() { return sanitize_hostname(&n); }
    }
    // go.mod — first line: "module github.com/user/repo"
    if let Some(n) = read_go_module_name(cwd) {
        return sanitize_hostname(&n);
    }
    // composer.json "name" (may be "vendor/package", take the last segment)
    if let Some(n) = read_json_field(cwd, "composer.json", "name") {
        let segment = n.split('/').last().unwrap_or(&n).to_string();
        if !segment.is_empty() { return sanitize_hostname(&segment); }
    }
    // fallback: directory name
    cwd.file_name()
        .and_then(|n| n.to_str())
        .map(sanitize_hostname)
        .unwrap_or_else(|| "app".to_string())
}

/// Resolve full hostname, prepending branch name for git worktrees.
pub fn resolve_hostname(cwd: &Path, override_name: Option<&str>, tld: &str) -> String {
    let project_name = infer_project_name(cwd, override_name);
    let git_path = cwd.join(".git");
    if let Ok(metadata) = fs::metadata(&git_path) {
        if metadata.is_file() {
            if let Ok(contents) = fs::read_to_string(&git_path) {
                if let Some(gitdir_line) = contents.lines().find(|l| l.starts_with("gitdir:")) {
                    let gitdir_path = gitdir_line.strip_prefix("gitdir:").map(|s| s.trim()).unwrap_or("");
                    let head_path = Path::new(gitdir_path).join("HEAD");
                    if let Ok(head_contents) = fs::read_to_string(&head_path) {
                        let branch = head_contents.trim()
                            .strip_prefix("ref: refs/heads/")
                            .unwrap_or(&head_contents);
                        return format!("{}-{}.{}", sanitize_hostname(branch), project_name, tld);
                    }
                }
            }
        }
    }
    format!("{}.{}", project_name, tld)
}

// ─── Internal manifest helpers ───────────────────────────────────────────────

pub(crate) fn read_json_field(cwd: &Path, filename: &str, key: &str) -> Option<String> {
    let contents = fs::read_to_string(cwd.join(filename)).ok()?;
    let json: serde_json::Value = serde_json::from_str(&contents).ok()?;
    json.get(key)?.as_str().map(String::from)
}

pub(crate) fn read_toml_field(cwd: &Path, filename: &str, keys: &[&str]) -> Option<String> {
    let contents = fs::read_to_string(cwd.join(filename)).ok()?;
    let val: toml::Value = toml::from_str(&contents).ok()?;
    let mut cur = &val;
    for key in keys {
        cur = cur.get(key)?;
    }
    cur.as_str().map(String::from)
}

pub(crate) fn file_contains(cwd: &Path, filename: &str, needle: &str) -> bool {
    fs::read_to_string(cwd.join(filename))
        .map(|s| s.contains(needle))
        .unwrap_or(false)
}

fn read_go_module_name(cwd: &Path) -> Option<String> {
    let contents = fs::read_to_string(cwd.join("go.mod")).ok()?;
    let first = contents.lines().next()?;
    let module = first.strip_prefix("module ")?.trim();
    // take last path segment: "github.com/user/repo" → "repo"
    Some(module.split('/').last()?.to_string())
}

// ─── Re-exports used by cli/mod.rs ───────────────────────────────────────────

pub use node::{resolve_run_args, KNOWN_RUNNERS, is_known_runner};

// ─── Sub-modules ─────────────────────────────────────────────────────────────

pub mod node;
pub mod python;
pub mod go;
pub mod ruby;
pub mod rust;
pub mod php;

// ─── Tests ───────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    // ... (as written in Step 1) ...
}
```

- [ ] **Step 4: Create stub files so the module tree compiles**

Create each of the following with just an empty comment:

```
src/detect/node.rs      // stub
src/detect/python.rs    // stub
src/detect/go.rs        // stub
src/detect/ruby.rs      // stub
src/detect/rust.rs      // stub
src/detect/php.rs       // stub
```

Each file contains exactly:
```rust
// stub — implemented in a later task
```

- [ ] **Step 5: Run tests to verify they pass**

```bash
cargo test detect::tests 2>&1 | tail -20
```

Expected: `test result: ok. 4 passed`

- [ ] **Step 6: Commit**

```bash
git add src/detect/
git commit -m "feat(detect): LanguageDriver trait, PortInjection, DriverRegistry skeleton"
```

---

## Task 2: Extend `ProjectConfig` + implement `PortalTomlDriver`

**Files:**
- Modify: `src/config.rs`
- Modify: `src/detect/mod.rs`

- [ ] **Step 1: Write the failing tests**

Add to `src/detect/mod.rs` tests block:

```rust
#[test]
fn portal_toml_driver_detects_when_start_command_set() {
    use crate::config::ProjectConfig;
    let cfg = ProjectConfig {
        name: None,
        start_command: Some("uvicorn main:app".to_string()),
        port_arg: None,
        host_arg: None,
        port_position: None,
    };
    let driver = PortalTomlDriver { config: cfg };
    let tmp = TempDir::new().unwrap();
    assert!(driver.detect(tmp.path()));
}

#[test]
fn portal_toml_driver_does_not_detect_when_only_name_set() {
    use crate::config::ProjectConfig;
    let cfg = ProjectConfig {
        name: Some("myapp".to_string()),
        start_command: None,
        port_arg: None,
        host_arg: None,
        port_position: None,
    };
    let driver = PortalTomlDriver { config: cfg };
    let tmp = TempDir::new().unwrap();
    assert!(!driver.detect(tmp.path()));
}

#[test]
fn portal_toml_driver_port_arg_injection() {
    use crate::config::ProjectConfig;
    let cfg = ProjectConfig {
        name: None,
        start_command: Some("uvicorn main:app".to_string()),
        port_arg: Some("--port".to_string()),
        host_arg: Some("--host".to_string()),
        port_position: None,
    };
    let driver = PortalTomlDriver { config: cfg };
    let tmp = TempDir::new().unwrap();
    let inj = driver.port_injection(tmp.path(), 4123);
    match inj {
        PortInjection::CliArgs(args) => {
            assert!(args.contains(&"--port".to_string()));
            assert!(args.contains(&"4123".to_string()));
            assert!(args.contains(&"--host".to_string()));
            assert!(args.contains(&"0.0.0.0".to_string()));
        }
        _ => panic!("expected CliArgs"),
    }
}

#[test]
fn portal_toml_driver_port_in_command_returns_env_only() {
    use crate::config::ProjectConfig;
    let cfg = ProjectConfig {
        name: None,
        start_command: Some("php -S 0.0.0.0:{port}".to_string()),
        port_arg: None,
        host_arg: None,
        port_position: None,
    };
    let driver = PortalTomlDriver { config: cfg };
    let tmp = TempDir::new().unwrap();
    matches!(driver.port_injection(tmp.path(), 4123), PortInjection::EnvOnly);
}
```

- [ ] **Step 2: Run tests to verify they fail**

```bash
cargo test detect::tests::portal_toml 2>&1 | tail -10
```

Expected: compile error — `ProjectConfig` missing fields, `PortalTomlDriver` not defined.

- [ ] **Step 3: Extend `ProjectConfig` in `src/config.rs`**

Replace the existing `ProjectConfig` and `PartialProjectConfig` structs:

```rust
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ProjectConfig {
    pub name: Option<String>,
    pub start_command: Option<String>,
    pub port_arg: Option<String>,
    pub host_arg: Option<String>,
    /// "append" → appends "0.0.0.0:{port}" as a positional arg
    pub port_position: Option<String>,
}

#[derive(Debug, Serialize, Deserialize, Default)]
struct PartialProjectConfig {
    name: Option<String>,
    start_command: Option<String>,
    port_arg: Option<String>,
    host_arg: Option<String>,
    port_position: Option<String>,
}
```

Update `apply_partial` to merge all new fields:

```rust
fn apply_partial(config: &mut Config, partial: PartialConfig) {
    // ... existing proxy/daemon fields unchanged ...

    if partial.project.name.is_some() {
        config.project.name = partial.project.name;
    }
    if partial.project.start_command.is_some() {
        config.project.start_command = partial.project.start_command;
    }
    if partial.project.port_arg.is_some() {
        config.project.port_arg = partial.project.port_arg;
    }
    if partial.project.host_arg.is_some() {
        config.project.host_arg = partial.project.host_arg;
    }
    if partial.project.port_position.is_some() {
        config.project.port_position = partial.project.port_position;
    }
}
```

- [ ] **Step 4: Add `PortalTomlDriver` to `src/detect/mod.rs`**

Add before the `#[cfg(test)]` block:

```rust
// ─── PortalTomlDriver ────────────────────────────────────────────────────────

pub struct PortalTomlDriver {
    pub config: crate::config::ProjectConfig,
}

impl LanguageDriver for PortalTomlDriver {
    fn detect(&self, _cwd: &Path) -> bool {
        self.config.start_command.is_some()
            || self.config.port_arg.is_some()
            || self.config.port_position.is_some()
    }
    fn priority(&self) -> u8 { 255 }
    fn name(&self) -> &'static str { "portal.toml" }
    fn project_name(&self, _cwd: &Path) -> Option<String> {
        self.config.name.clone()
    }
    fn start_command(&self, _cwd: &Path) -> Option<String> {
        self.config.start_command.clone()
    }
    fn port_injection(&self, _cwd: &Path, port: u16) -> PortInjection {
        // {port} in start_command → caller substitutes, no extra injection needed
        if self.config.start_command.as_deref().map_or(false, |c| c.contains("{port}")) {
            return PortInjection::EnvOnly;
        }
        if let Some(ref arg) = self.config.port_arg {
            let mut args = vec![arg.clone(), port.to_string()];
            if let Some(ref host_arg) = self.config.host_arg {
                args.push(host_arg.clone());
                args.push("0.0.0.0".to_string());
            }
            return PortInjection::CliArgs(args);
        }
        if self.config.port_position.as_deref() == Some("append") {
            return PortInjection::AppendAddress(format!("0.0.0.0:{port}"));
        }
        PortInjection::EnvOnly
    }
}
```

- [ ] **Step 5: Run tests to verify they pass**

```bash
cargo test detect::tests 2>&1 | tail -10
```

Expected: all tests pass.

- [ ] **Step 6: Commit**

```bash
git add src/config.rs src/detect/mod.rs
git commit -m "feat(detect): ProjectConfig fields + PortalTomlDriver"
```

---

## Task 3: `NodeDriver` — migrate all Node/JS logic

**Files:**
- Create: `src/detect/node.rs` (replace stub)

- [ ] **Step 1: Write the failing tests**

```rust
// src/detect/node.rs — tests at bottom
#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;
    use std::fs;

    #[test]
    fn node_driver_detects_package_json() {
        let tmp = TempDir::new().unwrap();
        fs::write(tmp.path().join("package.json"), r#"{"name":"app","scripts":{"dev":"vite"}}"#).unwrap();
        assert!(NodeDriver.detect(tmp.path()));
    }

    #[test]
    fn node_driver_does_not_detect_without_package_json() {
        let tmp = TempDir::new().unwrap();
        assert!(!NodeDriver.detect(tmp.path()));
    }

    #[test]
    fn node_driver_project_name_from_package_json() {
        let tmp = TempDir::new().unwrap();
        fs::write(tmp.path().join("package.json"), r#"{"name":"my-app"}"#).unwrap();
        assert_eq!(NodeDriver.project_name(tmp.path()), Some("my-app".to_string()));
    }

    #[test]
    fn node_driver_start_command_picks_dev_script() {
        let tmp = TempDir::new().unwrap();
        fs::write(tmp.path().join("package.json"), r#"{"scripts":{"dev":"vite","build":"tsc"}}"#).unwrap();
        let cmd = NodeDriver.start_command(tmp.path()).unwrap();
        assert!(cmd.contains("dev"), "expected dev in '{cmd}'");
    }

    #[test]
    fn node_driver_vite_injection() {
        let tmp = TempDir::new().unwrap();
        fs::write(tmp.path().join("package.json"), r#"{"scripts":{"dev":"vite"}}"#).unwrap();
        let inj = NodeDriver.port_injection(tmp.path(), 4123);
        match inj {
            crate::detect::PortInjection::CliArgs(args) => {
                assert!(args.contains(&"--port".to_string()));
                assert!(args.contains(&"4123".to_string()));
            }
            _ => panic!("expected CliArgs for Vite"),
        }
    }

    #[test]
    fn node_driver_unknown_framework_env_only() {
        let tmp = TempDir::new().unwrap();
        fs::write(tmp.path().join("package.json"), r#"{"scripts":{"dev":"node server.js"}}"#).unwrap();
        matches!(NodeDriver.port_injection(tmp.path(), 4123), crate::detect::PortInjection::EnvOnly);
    }

    #[test]
    fn resolve_run_args_expands_script_name() {
        let tmp = TempDir::new().unwrap();
        fs::write(tmp.path().join("pnpm-lock.yaml"), "").unwrap();
        fs::write(tmp.path().join("package.json"), r#"{"scripts":{"dev":"vite"}}"#).unwrap();
        let result = resolve_run_args(tmp.path(), vec!["dev".to_string()]);
        assert_eq!(result, vec!["pnpm", "run", "dev"]);
    }

    #[test]
    fn resolve_run_args_passthrough_known_runner() {
        let tmp = TempDir::new().unwrap();
        let args = vec!["npm".to_string(), "run".to_string(), "dev".to_string()];
        assert_eq!(resolve_run_args(tmp.path(), args.clone()), args);
    }
}
```

- [ ] **Step 2: Run tests to verify they fail**

```bash
cargo test detect::node 2>&1 | tail -10
```

Expected: compile error — `NodeDriver` not defined.

- [ ] **Step 3: Write `src/detect/node.rs`**

```rust
use std::fs;
use std::path::Path;
use crate::detect::{LanguageDriver, PortInjection};

pub const KNOWN_RUNNERS: &[&str] = &[
    "npm", "pnpm", "yarn", "bun", "node", "deno", "npx", "bunx", "pnpx",
    "python", "python3", "ruby", "go", "cargo", "java", "sh", "bash", "zsh", "fish",
];

pub fn is_known_runner(cmd: &str) -> bool {
    KNOWN_RUNNERS.contains(&cmd)
}

fn detect_package_manager(cwd: &Path) -> &'static str {
    if cwd.join("pnpm-lock.yaml").exists() { return "pnpm"; }
    if cwd.join("bun.lockb").exists() || cwd.join("bun.lock").exists() { return "bun"; }
    if cwd.join("yarn.lock").exists() { return "yarn"; }
    "npm"
}

fn pick_dev_script(json: &serde_json::Value) -> Option<String> {
    let scripts = json.get("scripts")?.as_object()?;
    if scripts.is_empty() { return None; }
    for &preferred in &["dev", "start", "serve", "develop"] {
        if scripts.contains_key(preferred) { return Some(preferred.to_string()); }
    }
    scripts.keys().min().cloned()
}

pub fn resolve_run_args(cwd: &Path, args: Vec<String>) -> Vec<String> {
    let first = match args.first() {
        Some(f) => f.clone(),
        None => return args,
    };
    if is_known_runner(&first) { return args; }
    let pkg_path = cwd.join("package.json");
    let script_exists = pkg_path.exists() && {
        fs::read_to_string(&pkg_path).ok()
            .and_then(|s| serde_json::from_str::<serde_json::Value>(&s).ok())
            .and_then(|j| j.get("scripts").and_then(|s| s.as_object()).map(|m| m.contains_key(first.as_str())))
            .unwrap_or(false)
    };
    if script_exists {
        let pm = detect_package_manager(cwd);
        let mut new_args = vec![pm.to_string(), "run".to_string()];
        new_args.extend(args);
        new_args
    } else {
        args
    }
}

// JS framework detection for port injection
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Framework {
    Vite, Astro, Angular, ReactRouter, Expo, Nuxt, Remix, SvelteKit, Unknown,
}

impl Framework {
    fn extra_args(&self, port: u16) -> Vec<String> {
        let p = port.to_string();
        match self {
            Framework::Vite     => vec!["--port".into(), p, "--host".into()],
            Framework::Astro    => vec!["--port".into(), p, "--host".into(), "0.0.0.0".into()],
            Framework::Angular  => vec!["--port".into(), p, "--host".into(), "0.0.0.0".into()],
            Framework::SvelteKit => vec!["--port".into(), p, "--host".into()],
            Framework::ReactRouter | Framework::Expo | Framework::Nuxt | Framework::Remix
                                => vec!["--port".into(), p],
            Framework::Unknown  => vec![],
        }
    }
}

fn detect_framework(cwd: &Path) -> Framework {
    if cwd.join("angular.json").exists() { return Framework::Angular; }
    if cwd.join("svelte.config.js").exists() || cwd.join("svelte.config.ts").exists() {
        return Framework::SvelteKit;
    }
    if let Ok(s) = fs::read_to_string(cwd.join("package.json")) {
        if let Ok(json) = serde_json::from_str::<serde_json::Value>(&s) {
            if let Some(scripts) = json.get("scripts").and_then(|v| v.as_object()) {
                let scripts_str = serde_json::to_string(scripts).unwrap_or_default();
                if scripts_str.contains("vite")         { return Framework::Vite; }
                if scripts_str.contains("astro")        { return Framework::Astro; }
                if scripts_str.contains("react-router") { return Framework::ReactRouter; }
                if scripts_str.contains("nuxt")         { return Framework::Nuxt; }
                if scripts_str.contains("remix")        { return Framework::Remix; }
            }
        }
    }
    if let Ok(s) = fs::read_to_string(cwd.join("app.json")) {
        if s.contains("expo") { return Framework::Expo; }
    }
    Framework::Unknown
}

// ─── NodeDriver ───────────────────────────────────────────────────────────────

pub struct NodeDriver;

impl LanguageDriver for NodeDriver {
    fn detect(&self, cwd: &Path) -> bool {
        cwd.join("package.json").exists()
    }
    fn priority(&self) -> u8 { 40 }
    fn name(&self) -> &'static str { "Node.js" }
    fn project_name(&self, cwd: &Path) -> Option<String> {
        crate::detect::read_json_field(cwd, "package.json", "name")
    }
    fn start_command(&self, cwd: &Path) -> Option<String> {
        let contents = fs::read_to_string(cwd.join("package.json")).ok()?;
        let json: serde_json::Value = serde_json::from_str(&contents).ok()?;
        let script = pick_dev_script(&json)?;
        let pm = detect_package_manager(cwd);
        Some(format!("{pm} run {script}"))
    }
    fn port_injection(&self, cwd: &Path, port: u16) -> PortInjection {
        let extra = detect_framework(cwd).extra_args(port);
        if extra.is_empty() {
            PortInjection::EnvOnly
        } else {
            PortInjection::CliArgs(extra)
        }
    }
}

#[cfg(test)]
mod tests {
    // ... (as written in Step 1) ...
}
```

- [ ] **Step 4: Run tests to verify they pass**

```bash
cargo test detect::node 2>&1 | tail -10
```

Expected: all tests pass.

- [ ] **Step 5: Commit**

```bash
git add src/detect/node.rs
git commit -m "feat(detect): NodeDriver — migrates all Node/JS logic"
```

---

## Task 4: Python drivers

**Files:**
- Create: `src/detect/python.rs` (replace stub)

- [ ] **Step 1: Write the failing tests**

```rust
// src/detect/python.rs — tests at bottom
#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;
    use std::fs;

    #[test]
    fn django_detects_manage_py() {
        let tmp = TempDir::new().unwrap();
        fs::write(tmp.path().join("manage.py"), "").unwrap();
        assert!(DjangoDriver.detect(tmp.path()));
    }

    #[test]
    fn django_does_not_detect_without_manage_py() {
        let tmp = TempDir::new().unwrap();
        assert!(!DjangoDriver.detect(tmp.path()));
    }

    #[test]
    fn django_append_address_injection() {
        let tmp = TempDir::new().unwrap();
        let inj = DjangoDriver.port_injection(tmp.path(), 4123);
        match inj {
            crate::detect::PortInjection::AppendAddress(addr) => {
                assert_eq!(addr, "0.0.0.0:4123");
            }
            _ => panic!("expected AppendAddress"),
        }
    }

    #[test]
    fn uvicorn_detects_from_pyproject() {
        let tmp = TempDir::new().unwrap();
        fs::write(tmp.path().join("pyproject.toml"),
            "[project]\nname = \"myapi\"\n[project.dependencies]\nuvicorn = \"*\"\n").unwrap();
        // file_contains checks the whole file, not structured deps — uvicorn appears in file
        fs::write(tmp.path().join("pyproject.toml"), "uvicorn = \"*\"").unwrap();
        assert!(UvicornDriver.detect(tmp.path()));
    }

    #[test]
    fn uvicorn_detects_from_requirements_txt() {
        let tmp = TempDir::new().unwrap();
        fs::write(tmp.path().join("requirements.txt"), "fastapi\nuvicorn[standard]\n").unwrap();
        assert!(UvicornDriver.detect(tmp.path()));
    }

    #[test]
    fn uvicorn_cli_args_injection() {
        let tmp = TempDir::new().unwrap();
        let inj = UvicornDriver.port_injection(tmp.path(), 4123);
        match inj {
            crate::detect::PortInjection::CliArgs(args) => {
                assert!(args.contains(&"--port".to_string()));
                assert!(args.contains(&"4123".to_string()));
                assert!(args.contains(&"--host".to_string()));
                assert!(args.contains(&"0.0.0.0".to_string()));
            }
            _ => panic!("expected CliArgs"),
        }
    }

    #[test]
    fn flask_detects_from_requirements_txt() {
        let tmp = TempDir::new().unwrap();
        fs::write(tmp.path().join("requirements.txt"), "flask\ngunicorn\n").unwrap();
        assert!(FlaskDriver.detect(tmp.path()));
    }

    #[test]
    fn flask_detects_from_app_py() {
        let tmp = TempDir::new().unwrap();
        fs::write(tmp.path().join("app.py"), "from flask import Flask").unwrap();
        assert!(FlaskDriver.detect(tmp.path()));
    }

    #[test]
    fn flask_does_not_shadow_uvicorn() {
        // If both flask and uvicorn are present, UvicornDriver has higher priority.
        // This test just verifies FlaskDriver still detects — priority is a registry concern.
        let tmp = TempDir::new().unwrap();
        fs::write(tmp.path().join("requirements.txt"), "flask\nuvicorn\n").unwrap();
        assert!(FlaskDriver.detect(tmp.path()));
        assert!(UvicornDriver.detect(tmp.path()));
    }
}
```

- [ ] **Step 2: Run tests to verify they fail**

```bash
cargo test detect::python 2>&1 | tail -10
```

Expected: compile error — `DjangoDriver`, `UvicornDriver`, `FlaskDriver` not defined.

- [ ] **Step 3: Write `src/detect/python.rs`**

```rust
use std::path::Path;
use crate::detect::{LanguageDriver, PortInjection, read_toml_field, file_contains};

// ─── DjangoDriver ─────────────────────────────────────────────────────────────

pub struct DjangoDriver;

impl LanguageDriver for DjangoDriver {
    fn detect(&self, cwd: &Path) -> bool {
        cwd.join("manage.py").exists()
    }
    fn priority(&self) -> u8 { 90 }
    fn name(&self) -> &'static str { "Django (Python)" }
    fn project_name(&self, cwd: &Path) -> Option<String> {
        read_toml_field(cwd, "pyproject.toml", &["project", "name"])
    }
    fn start_command(&self, _cwd: &Path) -> Option<String> {
        Some("python manage.py runserver".to_string())
    }
    fn port_injection(&self, _cwd: &Path, port: u16) -> PortInjection {
        PortInjection::AppendAddress(format!("0.0.0.0:{port}"))
    }
}

// ─── UvicornDriver ────────────────────────────────────────────────────────────

pub struct UvicornDriver;

impl LanguageDriver for UvicornDriver {
    fn detect(&self, cwd: &Path) -> bool {
        file_contains(cwd, "pyproject.toml", "uvicorn")
            || file_contains(cwd, "pyproject.toml", "fastapi")
            || file_contains(cwd, "requirements.txt", "uvicorn")
            || file_contains(cwd, "requirements.txt", "fastapi")
    }
    fn priority(&self) -> u8 { 80 }
    fn name(&self) -> &'static str { "uvicorn/FastAPI (Python)" }
    fn project_name(&self, cwd: &Path) -> Option<String> {
        read_toml_field(cwd, "pyproject.toml", &["project", "name"])
    }
    fn start_command(&self, _cwd: &Path) -> Option<String> {
        Some("uvicorn main:app".to_string())
    }
    fn port_injection(&self, _cwd: &Path, port: u16) -> PortInjection {
        PortInjection::CliArgs(vec![
            "--host".to_string(), "0.0.0.0".to_string(),
            "--port".to_string(), port.to_string(),
        ])
    }
}

// ─── FlaskDriver ──────────────────────────────────────────────────────────────

pub struct FlaskDriver;

impl LanguageDriver for FlaskDriver {
    fn detect(&self, cwd: &Path) -> bool {
        file_contains(cwd, "pyproject.toml", "flask")
            || file_contains(cwd, "requirements.txt", "flask")
            || cwd.join("app.py").exists()
            || cwd.join("wsgi.py").exists()
    }
    fn priority(&self) -> u8 { 80 }
    fn name(&self) -> &'static str { "Flask (Python)" }
    fn project_name(&self, cwd: &Path) -> Option<String> {
        read_toml_field(cwd, "pyproject.toml", &["project", "name"])
    }
    fn start_command(&self, _cwd: &Path) -> Option<String> {
        Some("flask run".to_string())
    }
    fn port_injection(&self, _cwd: &Path, port: u16) -> PortInjection {
        PortInjection::CliArgs(vec![
            "--host".to_string(), "0.0.0.0".to_string(),
            "--port".to_string(), port.to_string(),
        ])
    }
}

#[cfg(test)]
mod tests {
    // ... (as written in Step 1) ...
}
```

- [ ] **Step 4: Run tests to verify they pass**

```bash
cargo test detect::python 2>&1 | tail -10
```

Expected: all tests pass.

- [ ] **Step 5: Commit**

```bash
git add src/detect/python.rs
git commit -m "feat(detect): DjangoDriver, UvicornDriver, FlaskDriver"
```

---

## Task 5: Go driver

**Files:**
- Create: `src/detect/go.rs` (replace stub)

- [ ] **Step 1: Write the failing tests**

```rust
// src/detect/go.rs — tests at bottom
#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;
    use std::fs;

    #[test]
    fn go_detects_go_mod() {
        let tmp = TempDir::new().unwrap();
        fs::write(tmp.path().join("go.mod"), "module github.com/user/myservice\n\ngo 1.21\n").unwrap();
        assert!(GoDriver.detect(tmp.path()));
    }

    #[test]
    fn go_does_not_detect_without_go_mod() {
        let tmp = TempDir::new().unwrap();
        assert!(!GoDriver.detect(tmp.path()));
    }

    #[test]
    fn go_project_name_from_module() {
        let tmp = TempDir::new().unwrap();
        fs::write(tmp.path().join("go.mod"), "module github.com/user/myservice\n").unwrap();
        assert_eq!(GoDriver.project_name(tmp.path()), Some("myservice".to_string()));
    }

    #[test]
    fn go_start_command() {
        let tmp = TempDir::new().unwrap();
        assert_eq!(GoDriver.start_command(tmp.path()), Some("go run .".to_string()));
    }

    #[test]
    fn go_uses_env_only_injection() {
        let tmp = TempDir::new().unwrap();
        matches!(GoDriver.port_injection(tmp.path(), 4123), crate::detect::PortInjection::EnvOnly);
    }
}
```

- [ ] **Step 2: Run tests to verify they fail**

```bash
cargo test detect::go 2>&1 | tail -10
```

Expected: compile error — `GoDriver` not defined.

- [ ] **Step 3: Write `src/detect/go.rs`**

```rust
use std::{fs, path::Path};
use crate::detect::{LanguageDriver, PortInjection};

pub struct GoDriver;

impl LanguageDriver for GoDriver {
    fn detect(&self, cwd: &Path) -> bool {
        cwd.join("go.mod").exists()
    }
    fn priority(&self) -> u8 { 50 }
    fn name(&self) -> &'static str { "Go" }
    fn project_name(&self, cwd: &Path) -> Option<String> {
        let contents = fs::read_to_string(cwd.join("go.mod")).ok()?;
        let first = contents.lines().next()?;
        let module = first.strip_prefix("module ")?.trim();
        Some(module.split('/').last()?.to_string())
    }
    fn start_command(&self, _cwd: &Path) -> Option<String> {
        Some("go run .".to_string())
    }
    fn port_injection(&self, _cwd: &Path, _port: u16) -> PortInjection {
        PortInjection::EnvOnly
    }
}

#[cfg(test)]
mod tests {
    // ... (as written in Step 1) ...
}
```

- [ ] **Step 4: Run tests to verify they pass**

```bash
cargo test detect::go 2>&1 | tail -10
```

Expected: all tests pass.

- [ ] **Step 5: Commit**

```bash
git add src/detect/go.rs
git commit -m "feat(detect): GoDriver"
```

---

## Task 6: Ruby drivers

**Files:**
- Create: `src/detect/ruby.rs` (replace stub)

- [ ] **Step 1: Write the failing tests**

```rust
// src/detect/ruby.rs — tests at bottom
#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;
    use std::fs;

    #[test]
    fn rails_detects_gemfile_with_rails() {
        let tmp = TempDir::new().unwrap();
        fs::write(tmp.path().join("Gemfile"), "gem 'rails', '~> 7.0'\n").unwrap();
        assert!(RailsDriver.detect(tmp.path()));
    }

    #[test]
    fn rails_does_not_detect_without_rails_gem() {
        let tmp = TempDir::new().unwrap();
        fs::write(tmp.path().join("Gemfile"), "gem 'sinatra'\n").unwrap();
        assert!(!RailsDriver.detect(tmp.path()));
    }

    #[test]
    fn rails_cli_args_injection() {
        let tmp = TempDir::new().unwrap();
        let inj = RailsDriver.port_injection(tmp.path(), 4123);
        match inj {
            crate::detect::PortInjection::CliArgs(args) => {
                assert!(args.contains(&"-p".to_string()));
                assert!(args.contains(&"4123".to_string()));
                assert!(args.contains(&"-b".to_string()));
                assert!(args.contains(&"0.0.0.0".to_string()));
            }
            _ => panic!("expected CliArgs"),
        }
    }

    #[test]
    fn rack_detects_gemfile_without_rails() {
        let tmp = TempDir::new().unwrap();
        fs::write(tmp.path().join("Gemfile"), "gem 'sinatra'\n").unwrap();
        assert!(RackDriver.detect(tmp.path()));
    }

    #[test]
    fn rack_does_not_detect_without_gemfile() {
        let tmp = TempDir::new().unwrap();
        assert!(!RackDriver.detect(tmp.path()));
    }

    #[test]
    fn rack_cli_args_injection() {
        let tmp = TempDir::new().unwrap();
        let inj = RackDriver.port_injection(tmp.path(), 4123);
        match inj {
            crate::detect::PortInjection::CliArgs(args) => {
                assert!(args.contains(&"-p".to_string()));
                assert!(args.contains(&"4123".to_string()));
            }
            _ => panic!("expected CliArgs"),
        }
    }
}
```

- [ ] **Step 2: Run tests to verify they fail**

```bash
cargo test detect::ruby 2>&1 | tail -10
```

Expected: compile error.

- [ ] **Step 3: Write `src/detect/ruby.rs`**

```rust
use std::path::Path;
use crate::detect::{LanguageDriver, PortInjection, file_contains};

pub struct RailsDriver;

impl LanguageDriver for RailsDriver {
    fn detect(&self, cwd: &Path) -> bool {
        file_contains(cwd, "Gemfile", "rails")
    }
    fn priority(&self) -> u8 { 70 }
    fn name(&self) -> &'static str { "Rails (Ruby)" }
    fn project_name(&self, cwd: &Path) -> Option<String> {
        cwd.file_name()?.to_str().map(crate::detect::sanitize_hostname)
    }
    fn start_command(&self, _cwd: &Path) -> Option<String> {
        Some("rails server".to_string())
    }
    fn port_injection(&self, _cwd: &Path, port: u16) -> PortInjection {
        PortInjection::CliArgs(vec![
            "-p".to_string(), port.to_string(),
            "-b".to_string(), "0.0.0.0".to_string(),
        ])
    }
}

pub struct RackDriver;

impl LanguageDriver for RackDriver {
    fn detect(&self, cwd: &Path) -> bool {
        cwd.join("Gemfile").exists() && !file_contains(cwd, "Gemfile", "rails")
    }
    fn priority(&self) -> u8 { 70 }
    fn name(&self) -> &'static str { "Rack (Ruby)" }
    fn project_name(&self, cwd: &Path) -> Option<String> {
        cwd.file_name()?.to_str().map(crate::detect::sanitize_hostname)
    }
    fn start_command(&self, _cwd: &Path) -> Option<String> {
        Some("bundle exec rackup".to_string())
    }
    fn port_injection(&self, _cwd: &Path, port: u16) -> PortInjection {
        PortInjection::CliArgs(vec![
            "-p".to_string(), port.to_string(),
            "-o".to_string(), "0.0.0.0".to_string(),
        ])
    }
}

#[cfg(test)]
mod tests {
    // ... (as written in Step 1) ...
}
```

- [ ] **Step 4: Run tests to verify they pass**

```bash
cargo test detect::ruby 2>&1 | tail -10
```

Expected: all tests pass.

- [ ] **Step 5: Commit**

```bash
git add src/detect/ruby.rs
git commit -m "feat(detect): RailsDriver, RackDriver"
```

---

## Task 7: Rust driver

**Files:**
- Create: `src/detect/rust.rs` (replace stub)

- [ ] **Step 1: Write the failing tests**

```rust
// src/detect/rust.rs — tests at bottom
#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;
    use std::fs;

    #[test]
    fn rust_detects_cargo_toml() {
        let tmp = TempDir::new().unwrap();
        fs::write(tmp.path().join("Cargo.toml"),
            "[package]\nname = \"myservice\"\nversion = \"0.1.0\"\n").unwrap();
        assert!(RustDriver.detect(tmp.path()));
    }

    #[test]
    fn rust_does_not_detect_without_cargo_toml() {
        let tmp = TempDir::new().unwrap();
        assert!(!RustDriver.detect(tmp.path()));
    }

    #[test]
    fn rust_project_name_from_cargo_toml() {
        let tmp = TempDir::new().unwrap();
        fs::write(tmp.path().join("Cargo.toml"),
            "[package]\nname = \"myservice\"\n").unwrap();
        assert_eq!(RustDriver.project_name(tmp.path()), Some("myservice".to_string()));
    }

    #[test]
    fn rust_uses_env_only_injection() {
        let tmp = TempDir::new().unwrap();
        matches!(RustDriver.port_injection(tmp.path(), 4123), crate::detect::PortInjection::EnvOnly);
    }
}
```

- [ ] **Step 2: Run tests to verify they fail**

```bash
cargo test detect::rust 2>&1 | tail -10
```

Expected: compile error.

- [ ] **Step 3: Write `src/detect/rust.rs`**

```rust
use std::path::Path;
use crate::detect::{LanguageDriver, PortInjection, read_toml_field};

pub struct RustDriver;

impl LanguageDriver for RustDriver {
    fn detect(&self, cwd: &Path) -> bool {
        cwd.join("Cargo.toml").exists()
    }
    fn priority(&self) -> u8 { 50 }
    fn name(&self) -> &'static str { "Rust" }
    fn project_name(&self, cwd: &Path) -> Option<String> {
        read_toml_field(cwd, "Cargo.toml", &["package", "name"])
    }
    fn start_command(&self, _cwd: &Path) -> Option<String> {
        Some("cargo run".to_string())
    }
    fn port_injection(&self, _cwd: &Path, _port: u16) -> PortInjection {
        PortInjection::EnvOnly
    }
}

#[cfg(test)]
mod tests {
    // ... (as written in Step 1) ...
}
```

- [ ] **Step 4: Run tests to verify they pass**

```bash
cargo test detect::rust 2>&1 | tail -10
```

Expected: all tests pass.

- [ ] **Step 5: Commit**

```bash
git add src/detect/rust.rs
git commit -m "feat(detect): RustDriver"
```

---

## Task 8: PHP driver

**Files:**
- Create: `src/detect/php.rs` (replace stub)

- [ ] **Step 1: Write the failing tests**

```rust
// src/detect/php.rs — tests at bottom
#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;
    use std::fs;

    #[test]
    fn php_detects_index_php() {
        let tmp = TempDir::new().unwrap();
        fs::write(tmp.path().join("index.php"), "<?php echo 'hi';").unwrap();
        assert!(PhpDriver.detect(tmp.path()));
    }

    #[test]
    fn php_detects_composer_json() {
        let tmp = TempDir::new().unwrap();
        fs::write(tmp.path().join("composer.json"), r#"{"name":"vendor/myapp"}"#).unwrap();
        assert!(PhpDriver.detect(tmp.path()));
    }

    #[test]
    fn php_does_not_detect_without_markers() {
        let tmp = TempDir::new().unwrap();
        assert!(!PhpDriver.detect(tmp.path()));
    }

    #[test]
    fn php_project_name_from_composer_json() {
        let tmp = TempDir::new().unwrap();
        fs::write(tmp.path().join("composer.json"), r#"{"name":"vendor/myapp"}"#).unwrap();
        assert_eq!(PhpDriver.project_name(tmp.path()), Some("myapp".to_string()));
    }

    #[test]
    fn php_start_command_contains_port_placeholder() {
        let tmp = TempDir::new().unwrap();
        let cmd = PhpDriver.start_command(tmp.path()).unwrap();
        assert!(cmd.contains("{port}"), "expected {{port}} in '{cmd}'");
    }

    #[test]
    fn php_uses_env_only_injection() {
        // Port is embedded in start_command via {port} — no extra args needed
        let tmp = TempDir::new().unwrap();
        matches!(PhpDriver.port_injection(tmp.path(), 4123), crate::detect::PortInjection::EnvOnly);
    }
}
```

- [ ] **Step 2: Run tests to verify they fail**

```bash
cargo test detect::php 2>&1 | tail -10
```

Expected: compile error.

- [ ] **Step 3: Write `src/detect/php.rs`**

```rust
use std::path::Path;
use crate::detect::{LanguageDriver, PortInjection, read_json_field};

pub struct PhpDriver;

impl LanguageDriver for PhpDriver {
    fn detect(&self, cwd: &Path) -> bool {
        cwd.join("index.php").exists() || cwd.join("composer.json").exists()
    }
    fn priority(&self) -> u8 { 60 }
    fn name(&self) -> &'static str { "PHP" }
    fn project_name(&self, cwd: &Path) -> Option<String> {
        // composer.json "name" is "vendor/package" — take the last segment
        read_json_field(cwd, "composer.json", "name").map(|n| {
            n.split('/').last().unwrap_or(&n).to_string()
        })
    }
    fn start_command(&self, _cwd: &Path) -> Option<String> {
        // {port} is substituted by the caller before spawning
        Some("php -S 0.0.0.0:{port}".to_string())
    }
    fn port_injection(&self, _cwd: &Path, _port: u16) -> PortInjection {
        // Port is already in start_command via {port} placeholder
        PortInjection::EnvOnly
    }
}

#[cfg(test)]
mod tests {
    // ... (as written in Step 1) ...
}
```

- [ ] **Step 4: Run tests to verify they pass**

```bash
cargo test detect::php 2>&1 | tail -10
```

Expected: all tests pass.

- [ ] **Step 5: Commit**

```bash
git add src/detect/php.rs
git commit -m "feat(detect): PhpDriver"
```

---

## Task 9: Wire up `DriverRegistry::new()` with all drivers

**Files:**
- Modify: `src/detect/mod.rs` — add `DriverRegistry::new()`, integration tests

- [ ] **Step 1: Write the failing integration tests**

Add to `src/detect/mod.rs` tests block:

```rust
#[test]
fn registry_new_detects_django() {
    let tmp = TempDir::new().unwrap();
    std::fs::write(tmp.path().join("manage.py"), "").unwrap();
    let cfg = crate::config::Config::default();
    let reg = DriverRegistry::new(&cfg);
    assert_eq!(reg.detect(tmp.path()).unwrap().name(), "Django (Python)");
}

#[test]
fn registry_portal_toml_beats_django() {
    let tmp = TempDir::new().unwrap();
    std::fs::write(tmp.path().join("manage.py"), "").unwrap();
    let mut cfg = crate::config::Config::default();
    cfg.project.start_command = Some("my-custom-server".to_string());
    let reg = DriverRegistry::new(&cfg);
    assert_eq!(reg.detect(tmp.path()).unwrap().name(), "portal.toml");
}

#[test]
fn registry_detect_language_skips_portal_toml() {
    let tmp = TempDir::new().unwrap();
    std::fs::write(tmp.path().join("manage.py"), "").unwrap();
    let mut cfg = crate::config::Config::default();
    cfg.project.start_command = Some("my-custom-server".to_string());
    let reg = DriverRegistry::new(&cfg);
    // detect_language skips portal.toml driver
    assert_eq!(reg.detect_language(tmp.path()).unwrap().name(), "Django (Python)");
}

#[test]
fn registry_returns_none_for_unknown_project() {
    let tmp = TempDir::new().unwrap();
    let cfg = crate::config::Config::default();
    let reg = DriverRegistry::new(&cfg);
    assert!(reg.detect(tmp.path()).is_none());
}
```

- [ ] **Step 2: Run tests to verify they fail**

```bash
cargo test detect::tests::registry_new 2>&1 | tail -10
cargo test detect::tests::registry_portal_toml_beats 2>&1 | tail -10
```

Expected: compile error — `DriverRegistry::new` not defined.

- [ ] **Step 3: Add `DriverRegistry::new()` to `src/detect/mod.rs`**

Add after the `DriverRegistry` struct definition:

```rust
impl DriverRegistry {
    /// Build the default registry with all built-in drivers registered.
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
                Box::new(node::NodeDriver),
            ],
        };
        reg.sort();
        reg
    }

    // ... existing sort/detect/detect_language methods ...
}
```

- [ ] **Step 4: Run all detect tests**

```bash
cargo test detect 2>&1 | tail -20
```

Expected: all tests pass.

- [ ] **Step 5: Commit**

```bash
git add src/detect/mod.rs
git commit -m "feat(detect): wire up DriverRegistry::new() with all drivers"
```

---

## Task 10: Update `process.rs` — `spawn_child` accepts `PortInjection`

**Files:**
- Modify: `src/process.rs`

- [ ] **Step 1: Write the failing tests**

Add to `src/process.rs` tests, replacing the existing `child_receives_port_env` test body:

```rust
#[tokio::test]
async fn spawn_child_env_only_sets_port_env() {
    #[cfg(unix)]
    {
        use rand::Rng;
        let random_id = rand::thread_rng().gen::<u32>();
        let test_file = format!("/tmp/portal_port_test_{random_id}.txt");
        let args = vec!["sh".to_string(), "-c".to_string(),
            format!("echo $PORT > {test_file}")];
        let mut child = spawn_child(
            Path::new("/tmp"), &args, 4321, "test.localhost",
            crate::detect::PortInjection::EnvOnly,
        ).await.unwrap();
        let _ = child.wait().await;
        let content = std::fs::read_to_string(&test_file).unwrap();
        assert_eq!(content.trim(), "4321");
        let _ = std::fs::remove_file(&test_file);
    }
}

#[tokio::test]
async fn spawn_child_cli_args_appended() {
    #[cfg(unix)]
    {
        use rand::Rng;
        let random_id = rand::thread_rng().gen::<u32>();
        let test_file = format!("/tmp/portal_args_test_{random_id}.txt");
        let args = vec!["sh".to_string(), "-c".to_string(),
            format!("echo \"$@\" > {test_file}", )];
        let injection = crate::detect::PortInjection::CliArgs(
            vec!["--port".to_string(), "4321".to_string()]
        );
        let mut child = spawn_child(
            Path::new("/tmp"), &args, 4321, "test.localhost", injection,
        ).await.unwrap();
        let _ = child.wait().await;
        let content = std::fs::read_to_string(&test_file).unwrap();
        assert!(content.contains("--port"), "expected --port in '{content}'");
        let _ = std::fs::remove_file(&test_file);
    }
}

#[tokio::test]
async fn spawn_child_append_address_appended() {
    #[cfg(unix)]
    {
        use rand::Rng;
        let random_id = rand::thread_rng().gen::<u32>();
        let test_file = format!("/tmp/portal_addr_test_{random_id}.txt");
        // Use sh -c to capture all positional args
        let args = vec![
            "sh".to_string(), "-c".to_string(),
            format!("echo \"$1\" > {test_file}"),
            "sh".to_string(), // $0 for sh -c
        ];
        let injection = crate::detect::PortInjection::AppendAddress("0.0.0.0:4321".to_string());
        let mut child = spawn_child(
            Path::new("/tmp"), &args, 4321, "test.localhost", injection,
        ).await.unwrap();
        let _ = child.wait().await;
        let content = std::fs::read_to_string(&test_file).unwrap();
        assert!(content.contains("0.0.0.0:4321"), "expected address in '{content}'");
        let _ = std::fs::remove_file(&test_file);
    }
}
```

- [ ] **Step 2: Run tests to verify they fail**

```bash
cargo test process 2>&1 | tail -10
```

Expected: compile error — `spawn_child` signature mismatch.

- [ ] **Step 3: Rewrite `spawn_child` in `src/process.rs`**

```rust
use crate::detect::PortInjection;
use crate::error::Result;
use std::path::Path;
use tokio::process::Command;

/// Spawn a child dev-server process.
///
/// Sets `PORT=<port>` and `PORTAL_URL=https://<hostname>` regardless of
/// `injection`. The `injection` value controls how the port is embedded into
/// the command itself.
///
/// `{port}` in any arg string is substituted before spawning.
pub async fn spawn_child(
    cwd: &Path,
    args: &[String],
    port: u16,
    hostname: &str,
    injection: PortInjection,
) -> Result<tokio::process::Child> {
    if args.is_empty() {
        return Err(crate::error::Error::Ipc("No arguments provided to spawn_child".to_string()));
    }

    let port_str = port.to_string();

    // Substitute {port} in every arg
    let args: Vec<String> = args.iter()
        .map(|a| a.replace("{port}", &port_str))
        .collect();

    let program = &args[0];
    let rest: Vec<&str> = args[1..].iter().map(String::as_str).collect();

    let mut cmd = Command::new(program);
    cmd.env("PORT", &port_str)
        .env("PORTAL_URL", format!("https://{hostname}"))
        .current_dir(cwd)
        .kill_on_drop(false);

    match injection {
        PortInjection::EnvOnly => {
            cmd.args(&rest);
        }
        PortInjection::CliArgs(extra) => {
            cmd.args(&rest).args(&extra);
        }
        PortInjection::AppendAddress(addr) => {
            cmd.args(&rest).arg(&addr);
        }
    }

    Ok(cmd.spawn()?)
}

/// Gracefully stop a child process: SIGTERM, wait 5s, SIGKILL.
pub async fn stop_child(child: &mut tokio::process::Child) -> Result<()> {
    let pid = match child.id() {
        Some(id) => id,
        None => return Ok(()),
    };
    #[cfg(unix)]
    {
        use nix::sys::signal::{kill, Signal};
        use nix::unistd::Pid;
        let _ = kill(Pid::from_raw(pid as i32), Signal::SIGTERM);
    }
    #[cfg(windows)]
    {
        let _ = child.kill().await;
    }
    let result = tokio::select! {
        _ = child.wait() => Ok(()),
        _ = tokio::time::sleep(std::time::Duration::from_secs(5)) => {
            let _ = child.kill().await;
            Err(crate::error::Error::Ipc("Process did not respond to SIGTERM, force killed".to_string()))
        }
    };
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    // ... existing spawns_and_kills_child test unchanged ...
    // ... new tests from Step 1 ...
}
```

- [ ] **Step 4: Run tests to verify they pass**

```bash
cargo test process 2>&1 | tail -10
```

Expected: all tests pass.

- [ ] **Step 5: Commit**

```bash
git add src/process.rs
git commit -m "refactor(process): spawn_child accepts PortInjection, removes extra_args_for_port call"
```

---

## Task 11: Update `portal run` and `portal start` in `cli/mod.rs`

**Files:**
- Modify: `src/cli/mod.rs`

The existing `do_run` helper is called by both `Run` and `Start`. We update it to accept an optional `PortInjection` override. For `portal run` the injection is computed from the language driver (not PortalTomlDriver). For `portal start` the injection comes from `registry.detect()` (which includes PortalTomlDriver).

- [ ] **Step 1: Update `do_run` signature and body**

Replace the current `do_run` function with:

```rust
async fn do_run(
    cwd: std::path::PathBuf,
    config: crate::config::Config,
    args: Vec<String>,
    hostname_override: Option<String>,
    port_override: Option<u16>,
    injection_override: Option<crate::detect::PortInjection>,
) -> Result<()> {
    let mut setup = banner::SetupPrinter::new();
    ensure_daemon_running(&config, &mut setup).await?;
    ensure_cert_trusted(&mut setup).await?;
    setup.done();

    let hostname = crate::detect::resolve_hostname(
        &cwd,
        hostname_override.as_deref(),
        &config.proxy.tld,
    );

    // Check for an existing live route for this hostname (replace-by-default)
    let reuse_port: Option<u16> = {
        let mut stream = ipc_connect().await?;
        write_frame(&mut stream, &Command::Ls).await?;
        let resp: crate::proto::Response = read_frame(&mut stream).await?;
        if let Some(serde_json::Value::Array(routes)) = resp.data {
            routes.iter()
                .find(|r| r["hostname"].as_str() == Some(&hostname))
                .and_then(|r| r["port"].as_u64())
                .and_then(|p| u16::try_from(p).ok())
        } else {
            None
        }
    };

    let port = if let Some(explicit_port) = port_override {
        if let Some(_old_port) = reuse_port {
            let mut s = ipc_connect().await?;
            write_frame(&mut s, &Command::Stop { hostname: hostname.clone() }).await?;
            let _: crate::proto::Response = read_frame(&mut s).await?;
            crate::ports::wait_for_port_free(explicit_port, std::time::Duration::from_secs(2)).await;
        }
        explicit_port
    } else if let Some(old_port) = reuse_port {
        let mut s = ipc_connect().await?;
        write_frame(&mut s, &Command::Stop { hostname: hostname.clone() }).await?;
        let _: crate::proto::Response = read_frame(&mut s).await?;
        crate::ports::wait_for_port_free(old_port, std::time::Duration::from_secs(2)).await;
        old_port
    } else {
        crate::ports::find_free_port(config.proxy.port_range.0, config.proxy.port_range.1)?
    };

    // Resolve port injection: use override if provided, else detect from language driver
    let injection = injection_override.unwrap_or_else(|| {
        let registry = crate::detect::DriverRegistry::new(&config);
        registry.detect_language(&cwd)
            .map(|d| d.port_injection(&cwd, port))
            .unwrap_or(crate::detect::PortInjection::EnvOnly)
    });

    let my_pid = std::process::id();
    let mut child = crate::process::spawn_child(&cwd, &args, port, &hostname, injection).await?;

    let child_pid = child.id().unwrap_or(my_pid);
    if let Ok(mut stream) = ipc_connect().await {
        let _ = write_frame(&mut stream, &Command::RegisterRoute {
            hostname: hostname.clone(),
            port,
            pid: child_pid,
            cwd: cwd.to_string_lossy().to_string(),
        }).await;
        let _: crate::proto::Response = read_frame(&mut stream)
            .await.unwrap_or(crate::proto::Response::ok_empty());
    }

    banner::print_banner(&hostname, port, child_pid, reuse_port.is_some());
    child.wait().await?;
    Ok(())
}
```

- [ ] **Step 2: Update `CliCommand::Run` caller**

The existing `Run` handler calls `do_run` — add `None` as the new last argument:

```rust
CliCommand::Run { hostname, port, args } => {
    let cwd = std::env::current_dir()?;
    let config = crate::config::Config::load(&cwd)?;
    let resolved_args = crate::detect::resolve_run_args(&cwd, args);
    do_run(cwd, config, resolved_args, hostname, port, None).await?;
}
```

- [ ] **Step 3: Rewrite `CliCommand::Start` handler**

Replace the existing `Start` handler entirely:

```rust
CliCommand::Start => {
    let cwd = std::env::current_dir()?;
    let config = crate::config::Config::load(&cwd)?;
    let registry = crate::detect::DriverRegistry::new(&config);

    let driver = match registry.detect(&cwd) {
        Some(d) => d,
        None => {
            eprintln!("No supported project detected. Run `portal init` to set up this project.");
            std::process::exit(1);
        }
    };

    let raw_cmd = match driver.start_command(&cwd) {
        Some(cmd) => cmd,
        None => {
            eprintln!("Detected {} but couldn't determine a start command. Run `portal init`.", driver.name());
            std::process::exit(1);
        }
    };

    // Hostname: portal.toml name > driver name > dir name
    let hostname_override = config.project.name.clone()
        .or_else(|| driver.project_name(&cwd));

    // Pre-compute port injection from the matched driver (includes PortalTomlDriver)
    // We need the port first — find or allocate it via the same logic as do_run.
    // Pass the injection as an override to do_run.
    // Note: port is not known yet; we pass a placeholder (0) to get the injection *type*,
    // then do_run recomputes it with the real port via injection_override=None.
    // Instead we pass the driver reference info and let do_run detect:
    // Since PortalTomlDriver has priority 255, registry.detect() will return it if active,
    // and do_run's injection_override=None path calls detect_language (skips portal.toml).
    // To include portal.toml in the injection for portal start, we must pass it explicitly.
    // Solution: compute injection inside do_run only for portal run (None path).
    // For portal start, detect the injection here once we have the port from do_run...
    // Simplest: let do_run compute it; pass None for injection_override in Start too,
    // but use registry.detect (not detect_language) there.
    // To differentiate: add a boolean flag `use_portal_toml_injection: bool`.

    // Simpler approach: parse args from raw_cmd and pass the driver name to do_run
    // for injection lookup. Instead, just compute injection in the Start handler
    // with a placeholder port and re-derive in do_run.
    //
    // ACTUAL simplest approach: the injection from portal.toml is about the *command*
    // (e.g. port_arg = "--port"). When start_command comes from portal.toml, its
    // injection also comes from portal.toml. We can detect this by checking if
    // the matched driver is portal.toml, and if so, pass the PortalTomlDriver's
    // injection as an override with a real port computed inside do_run.
    //
    // Clean implementation: add a separate path in do_run for "start mode" that
    // uses registry.detect() instead of detect_language().
    // We do this by passing use_toml_driver: bool.

    let args: Vec<String> = raw_cmd
        .split_whitespace()
        .map(String::from)
        .collect();

    // For portal start, use the full registry (including PortalTomlDriver) for injection.
    // We signal this by passing a sentinel injection_override of None but setting a flag
    // via a wrapper that calls registry.detect() not detect_language().
    // Implementation: add `start_mode: bool` to do_run, or compute injection here.
    // Here we compute it with port=0 to get the type, then pass it through.
    // Port substitution in CliArgs/AppendAddress values uses the real port in spawn_child,
    // BUT the values computed by port_injection(port=0) have "0" hardcoded — wrong.
    //
    // Correct solution: pass the detection result as a closure/enum tag and compute
    // injection inside do_run after port is known.
    //
    // Final design: add `for_start: bool` param to do_run.
    // When true, do_run uses registry.detect(); when false, detect_language().

    do_run(cwd, config, args, hostname_override, None, None).await?;
    // Note: do_run will call detect_language() which skips PortalTomlDriver.
    // For portal start we WANT portal.toml injection. Fix in next step below.
}
```

Wait — we have a design conflict. `do_run` uses `detect_language` (skips `PortalTomlDriver`) for `portal run`. But for `portal start`, we want `detect` (includes `PortalTomlDriver`). Clean fix: add a `use_full_registry: bool` parameter to `do_run`.

- [ ] **Step 4: Add `use_full_registry` flag to `do_run` and callers**

Update the `do_run` signature:

```rust
async fn do_run(
    cwd: std::path::PathBuf,
    config: crate::config::Config,
    args: Vec<String>,
    hostname_override: Option<String>,
    port_override: Option<u16>,
    use_full_registry: bool,   // true for portal start, false for portal run
) -> Result<()>
```

Update the injection resolution inside `do_run`:

```rust
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

Update callers:
- `CliCommand::Run`: `do_run(cwd, config, resolved_args, hostname, port, false).await?`
- `CliCommand::Start`: `do_run(cwd, config, args, hostname_override, None, true).await?`

Remove the `injection_override` parameter (replaced by `use_full_registry`). The full updated `do_run` signature is:

```rust
async fn do_run(
    cwd: std::path::PathBuf,
    config: crate::config::Config,
    args: Vec<String>,
    hostname_override: Option<String>,
    port_override: Option<u16>,
    use_full_registry: bool,
) -> Result<()>
```

- [ ] **Step 5: Build to verify it compiles**

```bash
cargo build 2>&1 | tail -20
```

Expected: `Finished dev profile` with no errors.

- [ ] **Step 6: Run all tests**

```bash
cargo test 2>&1 | tail -20
```

Expected: all tests pass.

- [ ] **Step 7: Commit**

```bash
git add src/cli/mod.rs
git commit -m "refactor(cli): portal start language-agnostic via DriverRegistry, do_run uses PortInjection"
```

---

## Task 12: Add `portal init` command

**Files:**
- Modify: `src/cli/mod.rs`

- [ ] **Step 1: Add `Init` to the `CliCommand` enum**

In the `#[derive(Subcommand)]` block, add:

```rust
/// Generate portal.toml for this project
Init,
```

- [ ] **Step 2: Add the `Init` handler in `cli::run()`**

```rust
CliCommand::Init => {
    let cwd = std::env::current_dir()?;

    // Check for existing portal.toml
    if cwd.join("portal.toml").exists() {
        eprintln!("portal.toml already exists. Remove it first to reinitialise.");
        std::process::exit(1);
    }

    let config = crate::config::Config::load(&cwd)?;
    let registry = crate::detect::DriverRegistry::new(&config);

    // Detect language driver (skip portal.toml driver — file doesn't exist yet)
    let detected = registry.detect_language(&cwd);

    use std::io::IsTerminal;
    let is_tty = std::io::stdin().is_terminal();

    let (start_command, port_arg, host_arg, port_position, name) =
        if let Some(driver) = detected {
            let raw_cmd = driver.start_command(&cwd)
                .unwrap_or_else(|| String::new());
            let proj_name = driver.project_name(&cwd)
                .unwrap_or_else(|| {
                    cwd.file_name()
                        .and_then(|n| n.to_str())
                        .map(crate::detect::sanitize_hostname)
                        .unwrap_or_else(|| "app".to_string())
                });

            if is_tty {
                println!("\n  {} Detected  {}", console::style("✓").green(), driver.name());
                println!("  {} command   {}", console::style(" ").dim(), raw_cmd);
                println!("  {} name      {}\n", console::style(" ").dim(), proj_name);

                let confirmed: bool = dialoguer::Confirm::new()
                    .with_prompt("Does this look right?")
                    .default(true)
                    .interact()
                    .unwrap_or(true);

                if confirmed {
                    let (pa, ha, pp) = injection_toml_fields(&driver.port_injection(&cwd, 0));
                    (raw_cmd, pa, ha, pp, Some(proj_name))
                } else {
                    prompt_manual_config()?
                }
            } else {
                // No TTY — write detected values directly
                let (pa, ha, pp) = injection_toml_fields(&driver.port_injection(&cwd, 0));
                (raw_cmd, pa, ha, pp, Some(proj_name))
            }
        } else if is_tty {
            prompt_manual_config()?
        } else {
            // No driver, no TTY — write placeholder
            write_placeholder_toml(&cwd)?;
            println!("portal.toml created with placeholder. Edit it to configure your project.");
            return Ok(());
        };

    write_portal_toml(&cwd, &name, &start_command, &port_arg, &host_arg, &port_position)?;
    println!("{} portal.toml created", console::style("✓").green());
    println!("  Run: portal run {}",
        start_command.split_whitespace().next().unwrap_or("your-server"));
}
```

- [ ] **Step 3: Add helper functions** (add after `ipc_connect` function)

```rust
/// Convert a PortInjection into portal.toml field values.
/// Returns (port_arg, host_arg, port_position).
fn injection_toml_fields(
    injection: &crate::detect::PortInjection,
) -> (Option<String>, Option<String>, Option<String>) {
    match injection {
        crate::detect::PortInjection::EnvOnly => (None, None, None),
        crate::detect::PortInjection::CliArgs(args) => {
            // Find --port / -p flag
            let port_flag = args.windows(2)
                .find(|w| w[1] == "0")
                .map(|w| w[0].clone());
            // Find --host / -b flag
            let host_flag = args.windows(2)
                .find(|w| w[1] == "0.0.0.0")
                .map(|w| w[0].clone());
            (port_flag, host_flag, None)
        }
        crate::detect::PortInjection::AppendAddress(_) => {
            (None, None, Some("append".to_string()))
        }
    }
}

/// Interactive prompt when auto-detection failed or user rejected detection.
fn prompt_manual_config() -> crate::error::Result<
    (String, Option<String>, Option<String>, Option<String>, Option<String>)
> {
    let cmd: String = dialoguer::Input::new()
        .with_prompt("What command starts your dev server?")
        .interact_text()
        .map_err(|e| crate::error::Error::Ipc(e.to_string()))?;

    let choices = &[
        "FLAG   --port 4123",
        "FLAG   -p 4123",
        "APPEND 0.0.0.0:4123 (positional)",
        "ENV    PORT=4123 only",
        "CUSTOM I'll write the full command with {port}",
    ];
    let choice = dialoguer::Select::new()
        .with_prompt("How does it accept a port?")
        .items(choices)
        .default(0)
        .interact()
        .map_err(|e| crate::error::Error::Ipc(e.to_string()))?;

    let (pa, ha, pp) = match choice {
        0 => (Some("--port".to_string()), None, None),
        1 => (Some("-p".to_string()),     None, None),
        2 => (None, None, Some("append".to_string())),
        3 => (None, None, None),
        _ => (None, None, None), // user will write {port} in the command
    };

    Ok((cmd, pa, ha, pp, None))
}

/// Write portal.toml with detected/entered values.
fn write_portal_toml(
    cwd: &std::path::Path,
    name: &Option<String>,
    start_command: &str,
    port_arg: &Option<String>,
    host_arg: &Option<String>,
    port_position: &Option<String>,
) -> crate::error::Result<()> {
    let mut lines = vec!["[project]".to_string()];
    if let Some(n) = name {
        lines.push(format!("name = {n:?}"));
    }
    if !start_command.is_empty() {
        lines.push(format!("start_command = {start_command:?}"));
    }
    if let Some(pa) = port_arg {
        lines.push(format!("port_arg = {pa:?}"));
    }
    if let Some(ha) = host_arg {
        lines.push(format!("host_arg = {ha:?}"));
    }
    if let Some(pp) = port_position {
        lines.push(format!("port_position = {pp:?}"));
    }
    let content = lines.join("\n") + "\n";
    std::fs::write(cwd.join("portal.toml"), content)?;
    Ok(())
}

/// Write a portal.toml with placeholder comments for non-TTY, no-detection path.
fn write_placeholder_toml(cwd: &std::path::Path) -> crate::error::Result<()> {
    let content = r#"[project]
# name = "myapp"
# start_command = "your-dev-command"
# port_arg = "--port"
# host_arg = "--host"
# See: https://github.com/been-there-done-that/portal
"#;
    std::fs::write(cwd.join("portal.toml"), content)?;
    Ok(())
}
```

- [ ] **Step 4: Build to verify it compiles**

```bash
cargo build 2>&1 | tail -20
```

Expected: `Finished` with no errors.

- [ ] **Step 5: Run all tests**

```bash
cargo test 2>&1 | tail -20
```

Expected: all tests pass.

- [ ] **Step 6: Commit**

```bash
git add src/cli/mod.rs
git commit -m "feat(cli): portal init command — hybrid detect + interactive wizard"
```

---

## Task 13: Delete `src/detect.rs`, update version, final cleanup

**Files:**
- Delete: `src/detect.rs`
- Modify: `Cargo.toml` (version)

- [ ] **Step 1: Delete `src/detect.rs`**

```bash
rm src/detect.rs
```

- [ ] **Step 2: Build to verify nothing references the old file**

```bash
cargo build 2>&1 | tail -20
```

Expected: `Finished` with no errors. If there are missing symbol errors, the old file had functions that weren't migrated — fix them by adding re-exports in `src/detect/mod.rs`.

- [ ] **Step 3: Update version in `Cargo.toml`**

Change:
```toml
version = "1.0.0"
```
To:
```toml
version = "0.1.0"
```

- [ ] **Step 4: Run the full test suite**

```bash
cargo test 2>&1 | tail -30
```

Expected: all tests pass.

- [ ] **Step 5: Install and smoke-test**

```bash
cargo install --path . 2>&1 | tail -5
portal --version
```

Expected: `portal 0.1.0`

```bash
# In a Node.js project:
portal init
# Should say "Detected Node.js" and write portal.toml

# In a Python project with manage.py:
portal init
# Should say "Detected Django (Python)"

# Unknown project:
portal init
# Should prompt for command (TTY) or write placeholder (no TTY)
```

- [ ] **Step 6: Final commit**

```bash
git add -A
git commit -m "chore: delete src/detect.rs — fully replaced by src/detect/ module, bump to v0.1.0"
```
