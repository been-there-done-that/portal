# Script Flag + Framework Detection Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add a `--script <name>` CLI flag to override npm script auto-detection, and add Rsbuild and VitePress framework detection with correct port injection.

**Architecture:** All changes live in two existing files — `src/detect/node.rs` (rename `pick_dev_script` → `pick_script` with optional override, add `Rsbuild`/`VitePress` enum variants and detection) and `src/cli/mod.rs` (add `--script` arg to `CliCommand::Run`, thread it through `do_run`). No new files needed.

**Tech Stack:** Rust, clap (CLI parsing), serde_json (package.json parsing), tempfile (test fixtures)

---

## File Map

| File | Change |
|------|--------|
| `src/detect/node.rs` | Rename `pick_dev_script` → `pick_script(json, override_script: Option<&str>)` (pub); add `Rsbuild`, `VitePress` to `Framework` enum; extend `detect_framework` with Rsbuild/VitePress checks before the Vite check |
| `src/cli/mod.rs` | Add `script: Option<String>` field to `CliCommand::Run`; unpack it in the match arm; add `script: Option<String>` param to `do_run`; apply override logic before calling `do_run` |

---

## Task 1: Extend `pick_dev_script` → `pick_script` and add Rsbuild/VitePress detection

**Files:**
- Modify: `src/detect/node.rs`

- [ ] **Step 1.1: Write failing tests for `pick_script` override behaviour**

Add the following inside `mod tests` at the bottom of `src/detect/node.rs`, after the existing tests:

```rust
#[test]
fn pick_script_override_uses_named_script() {
    let json: serde_json::Value =
        serde_json::from_str(r#"{"scripts":{"dev":"vite","start":"node server.js"}}"#).unwrap();
    assert_eq!(
        pick_script(&json, Some("start")),
        Some("start".to_string())
    );
}

#[test]
fn pick_script_override_falls_back_when_missing() {
    let json: serde_json::Value =
        serde_json::from_str(r#"{"scripts":{"dev":"vite"}}"#).unwrap();
    assert_eq!(pick_script(&json, Some("nonexistent")), None);
}

#[test]
fn pick_script_no_override_keeps_priority_order() {
    let json: serde_json::Value =
        serde_json::from_str(r#"{"scripts":{"build":"tsc","dev":"vite","start":"node s.js"}}"#)
            .unwrap();
    // "dev" wins over "start"
    assert_eq!(pick_script(&json, None), Some("dev".to_string()));
}
```

- [ ] **Step 1.2: Run tests to confirm they fail**

```bash
cargo test pick_script 2>&1 | head -30
```

Expected output contains: `error[E0425]: cannot find function \`pick_script\``

- [ ] **Step 1.3: Write failing tests for Rsbuild and VitePress detection**

Add inside `mod tests`, after the tests from step 1.1:

```rust
#[test]
fn rsbuild_detection_from_config_file() {
    let tmp = TempDir::new().unwrap();
    fs::write(
        tmp.path().join("package.json"),
        r#"{"scripts":{"dev":"node server.js"}}"#,
    )
    .unwrap();
    fs::write(tmp.path().join("rsbuild.config.ts"), "").unwrap();
    assert_eq!(detect_framework(tmp.path()), Framework::Rsbuild);
}

#[test]
fn rsbuild_detection_from_scripts() {
    let tmp = TempDir::new().unwrap();
    fs::write(
        tmp.path().join("package.json"),
        r#"{"scripts":{"dev":"rsbuild dev"}}"#,
    )
    .unwrap();
    assert_eq!(detect_framework(tmp.path()), Framework::Rsbuild);
}

#[test]
fn vitepress_detection_from_scripts() {
    let tmp = TempDir::new().unwrap();
    fs::write(
        tmp.path().join("package.json"),
        r#"{"scripts":{"dev":"vitepress dev"}}"#,
    )
    .unwrap();
    assert_eq!(detect_framework(tmp.path()), Framework::VitePress);
}

#[test]
fn rsbuild_port_injection() {
    let tmp = TempDir::new().unwrap();
    fs::write(tmp.path().join("rsbuild.config.ts"), "").unwrap();
    fs::write(
        tmp.path().join("package.json"),
        r#"{"scripts":{"dev":"rsbuild dev"}}"#,
    )
    .unwrap();
    match NodeDriver.port_injection(tmp.path(), 4000) {
        crate::detect::PortInjection::CliArgs(args) => {
            assert_eq!(args, vec!["--port".to_string(), "4000".to_string()]);
        }
        other => panic!("expected CliArgs([--port, 4000]), got {other:?}"),
    }
}

#[test]
fn vitepress_port_injection() {
    let tmp = TempDir::new().unwrap();
    fs::write(
        tmp.path().join("package.json"),
        r#"{"scripts":{"dev":"vitepress dev"}}"#,
    )
    .unwrap();
    match NodeDriver.port_injection(tmp.path(), 4000) {
        crate::detect::PortInjection::CliArgs(args) => {
            assert_eq!(args, vec!["--port".to_string(), "4000".to_string()]);
        }
        other => panic!("expected CliArgs([--port, 4000]), got {other:?}"),
    }
}
```

- [ ] **Step 1.4: Run tests to confirm new detection tests also fail**

```bash
cargo test -p portless 'rsbuild|vitepress' 2>&1 | head -30
```

Expected output contains: `error[E0425]: cannot find value \`Framework::Rsbuild\``

- [ ] **Step 1.5: Implement `pick_script` (rename + extend existing `pick_dev_script`)**

In `src/detect/node.rs`, replace the private `pick_dev_script` function (lines 27–38) with:

```rust
pub fn pick_script(json: &serde_json::Value, override_script: Option<&str>) -> Option<String> {
    let scripts = json.get("scripts")?.as_object()?;
    if scripts.is_empty() {
        return None;
    }
    if let Some(name) = override_script {
        return if scripts.contains_key(name) {
            Some(name.to_string())
        } else {
            None
        };
    }
    for &preferred in &["dev", "start", "serve", "develop"] {
        if scripts.contains_key(preferred) {
            return Some(preferred.to_string());
        }
    }
    scripts.keys().min().cloned()
}
```

Also update the call site inside `NodeDriver::start_command` (the line that currently calls `pick_dev_script`):

```rust
let script = pick_script(&json, None)?;
```

- [ ] **Step 1.6: Add `Rsbuild` and `VitePress` to the `Framework` enum**

In `src/detect/node.rs`, replace the `Framework` enum (lines 72–82) with:

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Framework {
    Vite,
    Rsbuild,
    VitePress,
    Astro,
    Angular,
    ReactRouter,
    Expo,
    Nuxt,
    Remix,
    SvelteKit,
    Unknown,
}
```

- [ ] **Step 1.7: Add port injection arms for the two new variants**

In `Framework::extra_args`, add the two new match arms before `Framework::Unknown`:

```rust
Framework::Rsbuild => vec!["--port".into(), p],
Framework::VitePress => vec!["--port".into(), p],
```

The complete `extra_args` method after the edit:

```rust
fn extra_args(&self, port: u16) -> Vec<String> {
    let p = port.to_string();
    match self {
        Framework::Vite => vec!["--port".into(), p, "--host".into()],
        Framework::Astro => vec!["--port".into(), p, "--host".into(), "0.0.0.0".into()],
        Framework::Angular => vec!["--port".into(), p, "--host".into(), "0.0.0.0".into()],
        Framework::SvelteKit => vec!["--port".into(), p, "--host".into()],
        Framework::ReactRouter | Framework::Expo | Framework::Nuxt | Framework::Remix => {
            vec!["--port".into(), p]
        }
        Framework::Rsbuild => vec!["--port".into(), p],
        Framework::VitePress => vec!["--port".into(), p],
        Framework::Unknown => vec![],
    }
}
```

- [ ] **Step 1.8: Add Rsbuild and VitePress detection in `detect_framework`**

Replace the body of `detect_framework` with the version below. The Rsbuild and VitePress checks must appear **before** the `"vite"` string check inside the scripts block:

```rust
fn detect_framework(cwd: &Path) -> Framework {
    if cwd.join("angular.json").exists() {
        return Framework::Angular;
    }
    if cwd.join("svelte.config.js").exists() || cwd.join("svelte.config.ts").exists() {
        return Framework::SvelteKit;
    }
    // Rsbuild config file takes priority over script-string detection
    if cwd.join("rsbuild.config.ts").exists() || cwd.join("rsbuild.config.js").exists() {
        return Framework::Rsbuild;
    }
    if let Ok(s) = fs::read_to_string(cwd.join("package.json")) {
        if let Ok(json) = serde_json::from_str::<serde_json::Value>(&s) {
            if let Some(scripts) = json.get("scripts").and_then(|v| v.as_object()) {
                let scripts_str = serde_json::to_string(scripts).unwrap_or_default();
                if scripts_str.contains("rsbuild") {
                    return Framework::Rsbuild;
                }
                if scripts_str.contains("vitepress") {
                    return Framework::VitePress;
                }
                if scripts_str.contains("vite") {
                    return Framework::Vite;
                }
                if scripts_str.contains("astro") {
                    return Framework::Astro;
                }
                if scripts_str.contains("react-router") {
                    return Framework::ReactRouter;
                }
                if scripts_str.contains("nuxt") {
                    return Framework::Nuxt;
                }
                if scripts_str.contains("remix") {
                    return Framework::Remix;
                }
            }
        }
    }
    if let Ok(s) = fs::read_to_string(cwd.join("app.json")) {
        if s.contains("expo") {
            return Framework::Expo;
        }
    }
    Framework::Unknown
}
```

- [ ] **Step 1.9: Run all detect/node tests and confirm they pass**

```bash
cargo test -p portless detect::node 2>&1 | tail -20
```

Expected output ends with something like:

```
test result: ok. 14 passed; 0 failed; 0 ignored
```

(Exact count may vary; zero failures is the requirement.)

- [ ] **Step 1.10: Commit**

```bash
git add src/detect/node.rs
git commit -m "feat(detect): rename pick_dev_script -> pick_script with override, add Rsbuild/VitePress"
```

---

## Task 2: Wire `--script` flag into CLI

**Files:**
- Modify: `src/cli/mod.rs`

- [ ] **Step 2.1: Write a failing clap test for the `--script` flag**

Add the following to `src/cli/mod.rs`. Find the existing `#[cfg(test)]` block (search for `mod tests` inside `src/cli/mod.rs`) — if one exists, append to it; otherwise add this block at the bottom of the file:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn run_command_has_script_arg() {
        // Verify clap can parse --script
        let cli = Cli::try_parse_from(["portal", "run", "--script", "start", "npm", "run", "dev"])
            .expect("parse failed");
        match cli.command {
            CliCommand::Run { script, .. } => {
                assert_eq!(script, Some("start".to_string()));
            }
            _ => panic!("expected Run variant"),
        }
    }

    #[test]
    fn run_command_script_absent_by_default() {
        let cli = Cli::try_parse_from(["portal", "run", "npm", "run", "dev"])
            .expect("parse failed");
        match cli.command {
            CliCommand::Run { script, .. } => {
                assert_eq!(script, None);
            }
            _ => panic!("expected Run variant"),
        }
    }
}
```

- [ ] **Step 2.2: Run the test to confirm it fails**

```bash
cargo test -p portless 'cli::tests::run_command_has_script_arg' 2>&1 | head -20
```

Expected: compile error — `CliCommand::Run` has no field `script`.

- [ ] **Step 2.3: Add `script: Option<String>` field to `CliCommand::Run`**

In `src/cli/mod.rs`, inside the `Run` variant of `CliCommand`, add after the existing `label` field and before `args`:

```rust
/// Override the npm/pnpm/yarn/bun script to run (default: auto-detected from package.json)
#[arg(long)]
script: Option<String>,
```

The `args` field (with `trailing_var_arg`) must remain the last field in the variant.

- [ ] **Step 2.4: Add `script: Option<String>` parameter to `do_run`**

In `src/cli/mod.rs`, update the `do_run` signature to add `script` as the last parameter before the closing paren:

```rust
async fn do_run(
    cwd: std::path::PathBuf,
    config: crate::config::Config,
    args: Vec<String>,
    hostname_override: Option<String>,
    port_override: Option<u16>,
    use_full_registry: bool,
    quiet: bool,
    tcp: bool,
    force: bool,
    tailscale: bool,
    funnel: bool,
    slot: Option<u32>,
    label: Option<String>,
    script: Option<String>,
) -> Result<()> {
```

- [ ] **Step 2.5: Apply `--script` override logic in the `CliCommand::Run` match arm**

In the `CliCommand::Run` match arm, unpack the new `script` field and apply override logic **before** calling `do_run`. Replace the current block:

```rust
CliCommand::Run {
    hostname,
    port,
    quiet,
    tcp,
    force,
    lan,
    ip,
    h2c,
    tailscale,
    funnel,
    slot,
    label,
    args,
} => {
    let cwd = std::env::current_dir()?;
    let mut config = crate::config::Config::load(&cwd)?;
    if lan { config.proxy.lan = true; }
    if let Some(addr) = ip { config.proxy.lan_ip = Some(addr); }
    if h2c { config.proxy.h2c = true; }
    let use_tailscale = tailscale || funnel;
    let resolved_args = crate::detect::resolve_run_args(&cwd, args);
    do_run(
        cwd,
        config,
        resolved_args,
        hostname,
        port,
        false,
        quiet,
        tcp,
        force,
        use_tailscale,
        funnel,
        slot,
        label,
    )
    .await?;
}
```

with:

```rust
CliCommand::Run {
    hostname,
    port,
    quiet,
    tcp,
    force,
    lan,
    ip,
    h2c,
    tailscale,
    funnel,
    slot,
    label,
    script,
    args,
} => {
    let cwd = std::env::current_dir()?;
    let mut config = crate::config::Config::load(&cwd)?;
    if lan { config.proxy.lan = true; }
    if let Some(addr) = ip { config.proxy.lan_ip = Some(addr); }
    if h2c { config.proxy.h2c = true; }
    let use_tailscale = tailscale || funnel;

    // Apply --script override: only when user gave no explicit args
    let resolved_args = if args.is_empty() {
        if let Some(ref name) = script {
            let pkg_path = cwd.join("package.json");
            let found = pkg_path.exists() && {
                std::fs::read_to_string(&pkg_path)
                    .ok()
                    .and_then(|s| serde_json::from_str::<serde_json::Value>(&s).ok())
                    .and_then(|j| {
                        crate::detect::node::pick_script(&j, Some(name.as_str()))
                    })
                    .is_some()
            };
            if found {
                let pm = crate::detect::node::detect_package_manager(&cwd);
                vec![pm.to_string(), "run".to_string(), name.clone()]
            } else {
                eprintln!(
                    "warning: script \"{name}\" not found in package.json, falling back to auto-detect"
                );
                crate::detect::resolve_run_args(&cwd, args)
            }
        } else {
            crate::detect::resolve_run_args(&cwd, args)
        }
    } else {
        // User gave explicit args — ignore --script silently
        crate::detect::resolve_run_args(&cwd, args)
    };

    do_run(
        cwd,
        config,
        resolved_args,
        hostname,
        port,
        false,
        quiet,
        tcp,
        force,
        use_tailscale,
        funnel,
        slot,
        label,
        script,
    )
    .await?;
}
```

- [ ] **Step 2.6: Update the `CliCommand::Start` call to `do_run` to pass `None` for `script`**

Find the `do_run(...)` call inside the `CliCommand::Start` arm (around line 243). It currently passes 13 arguments. Add `None` as the 14th argument:

```rust
do_run(
    cwd,
    config,
    args,
    hostname_override,
    None,
    true,
    quiet,
    false,
    false,
    false,
    false,
    None,
    None,
    None,  // script
)
.await?;
```

- [ ] **Step 2.7: Make `pick_script` and `detect_package_manager` accessible from `src/cli/mod.rs`**

In `src/detect/node.rs`, `detect_package_manager` is currently `pub(crate)` — that is sufficient. `pick_script` was made `pub` in Task 1. Verify the module path `crate::detect::node` is reachable from `src/cli/mod.rs` by checking `src/detect/mod.rs`:

```bash
grep -n 'pub mod node\|pub use\|mod node' /Users/__deesh_reddy__/projects/personal_git/rust_builds/portless/src/detect/mod.rs
```

If `node` is not publicly re-exported, add a `pub use` or confirm `pub(crate)` access is sufficient for the `crate::detect::node::pick_script` path. The function was marked `pub` in step 1.5, so `crate::detect::node::pick_script` should compile as long as `mod node` is visible as `pub mod node` or `pub(crate) mod node` in `src/detect/mod.rs`.

- [ ] **Step 2.8: Run the CLI tests**

```bash
cargo test -p portless 'cli::tests' 2>&1 | tail -20
```

Expected:

```
test cli::tests::run_command_has_script_arg ... ok
test cli::tests::run_command_script_absent_by_default ... ok
test result: ok. 2 passed; 0 failed
```

- [ ] **Step 2.9: Run the full test suite**

```bash
cargo test -p portless 2>&1 | tail -10
```

Expected: all tests pass, zero failures.

- [ ] **Step 2.10: Commit**

```bash
git add src/cli/mod.rs
git commit -m "feat(cli): add --script flag to override npm script selection in portal run"
```

---

## Task 3: Branch, full test run, commit, push

- [ ] **Step 3.1: Create feature branch from main**

```bash
git checkout main
git checkout -b feature/script-flag-framework-detection
```

- [ ] **Step 3.2: Cherry-pick the two commits from Task 1 and Task 2**

Identify the two commit hashes just made (they will be the most recent two on whatever branch you were working on):

```bash
git log --oneline -5
```

Then cherry-pick them in order (oldest first):

```bash
git cherry-pick <hash-of-task-1-commit>
git cherry-pick <hash-of-task-2-commit>
```

(If Task 1 and Task 2 work was already done directly on `feature/script-flag-framework-detection`, skip this step — the commits are already on the branch.)

- [ ] **Step 3.3: Run the full test suite on the feature branch**

```bash
cargo test -p portless 2>&1 | tail -15
```

Expected: zero failures. The output should include lines similar to:

```
test detect::node::pick_script_override_uses_named_script ... ok
test detect::node::pick_script_override_falls_back_when_missing ... ok
test detect::node::rsbuild_detection_from_config_file ... ok
test detect::node::rsbuild_detection_from_scripts ... ok
test detect::node::vitepress_detection_from_scripts ... ok
test detect::node::rsbuild_port_injection ... ok
test detect::node::vitepress_port_injection ... ok
test cli::tests::run_command_has_script_arg ... ok
test result: ok. X passed; 0 failed
```

- [ ] **Step 3.4: Push the feature branch**

```bash
git push -u origin feature/script-flag-framework-detection
```

Expected: branch pushed successfully, upstream tracking set.
