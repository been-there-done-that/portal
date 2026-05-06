# Implementation Plan: `package.json "portless"` Config Key

**Date:** 2026-05-06
**Branch:** `feature/package-json-portless-key`
**Base branch:** `main`
**Spec:** `docs/superpowers/specs/2026-05-06-package-json-portless-key-design.md`

---

## Overview

Two tasks, both touching only `src/config.rs`:

1. **Task 1** — Add `script` field, add `PartialPortlessJson` struct, implement `load_partial_from_package_json` and `find_and_load_package_json_config` with 6 TDD tests written first.
2. **Task 2** — Wire the new function into `load_with_paths`, create branch, commit, push.

Merge priority (highest → lowest):
`portal.toml` > `package.json["portless"]` > env vars > compiled defaults

---

## Task 1 — Add `script` field and implement `load_partial_from_package_json`

**File:** `src/config.rs`

### Step 1.1 — Add `script` to `ProjectConfig`

In `ProjectConfig` (after `proxy: Option<bool>`), add:

```rust
/// npm/yarn/pnpm script to run (equivalent of `--script` CLI flag)
#[serde(default)]
pub script: Option<String>,
```

Full updated struct:

```rust
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ProjectConfig {
    pub name: Option<String>,
    pub start_command: Option<String>,
    pub port_arg: Option<String>,
    pub host_arg: Option<String>,
    /// "append" → appends "0.0.0.0:{port}" as a positional arg
    pub port_position: Option<String>,
    /// Name of the env var to use for passing the port (e.g. "APP_PORT")
    pub port_env: Option<String>,
    /// Whether to proxy this service (None = auto-detect, Some(false) = build-only mode, Some(true) = force proxy)
    pub proxy: Option<bool>,
    /// npm/yarn/pnpm script to run (equivalent of `--script` CLI flag)
    #[serde(default)]
    pub script: Option<String>,
}
```

### Step 1.2 — Add `script` to `PartialProjectConfig`

```rust
#[derive(Debug, Serialize, Deserialize, Default)]
struct PartialProjectConfig {
    name: Option<String>,
    start_command: Option<String>,
    port_arg: Option<String>,
    host_arg: Option<String>,
    port_position: Option<String>,
    port_env: Option<String>,
    proxy: Option<bool>,
    script: Option<String>,
}
```

### Step 1.3 — Wire `script` through `apply_partial`

At the end of the project block in `apply_partial`, add:

```rust
if partial.project.script.is_some() {
    config.project.script = partial.project.script;
}
```

### Step 1.4 — Define `PartialPortlessJson`

Add this struct **above** `load_partial_from_package_json`. It matches the flat JSON shape under `"portless"`:

```rust
/// Flat JSON shape for `package.json["portless"]`
#[derive(Debug, Deserialize, Default)]
#[serde(default, rename_all = "snake_case")]
struct PartialPortlessJson {
    tld: Option<String>,
    /// Maps to `ProjectConfig.name`
    hostname: Option<String>,
    https: Option<bool>,
    http_port: Option<u16>,
    https_port: Option<u16>,
    wildcard: Option<bool>,
    lan: Option<bool>,
    script: Option<String>,
    h2c: Option<bool>,
}
```

### Step 1.5 — Write 6 TDD tests (BEFORE implementation)

Add these tests inside the `#[cfg(test)] mod tests` block in `src/config.rs`:

```rust
// ── package.json["portless"] tests ────────────────────────────────────────

#[test]
fn portless_key_in_package_json_sets_tld() {
    let temp = TempDir::new().unwrap();
    let pkg = temp.path().join("package.json");
    std::fs::write(
        &pkg,
        r#"{"name":"myapp","portless":{"tld":"test"}}"#,
    )
    .unwrap();

    let config = Config::load_with_paths(None, None, &[])
        .unwrap();
    // Without wiring, this test will fail — that's expected in TDD step 1.
    // After Task 2 wiring with search_dir = temp.path(), re-run with the
    // dedicated helper used in the test below.
    let partial = load_partial_from_package_json(temp.path()).unwrap();
    let mut cfg = Config::default();
    apply_partial(&mut cfg, partial);
    assert_eq!(cfg.proxy.tld, "test");
}

#[test]
fn portless_key_sets_hostname() {
    let temp = TempDir::new().unwrap();
    std::fs::write(
        temp.path().join("package.json"),
        r#"{"portless":{"hostname":"myapi"}}"#,
    )
    .unwrap();

    let partial = load_partial_from_package_json(temp.path()).unwrap();
    let mut cfg = Config::default();
    apply_partial(&mut cfg, partial);
    assert_eq!(cfg.project.name, Some("myapi".to_string()));
}

#[test]
fn portal_toml_wins_over_package_json() {
    let temp = TempDir::new().unwrap();
    // package.json says tld = "from-json"
    std::fs::write(
        temp.path().join("package.json"),
        r#"{"portless":{"tld":"from-json"}}"#,
    )
    .unwrap();
    // portal.toml says tld = "from-toml"
    let toml_path = temp.path().join("portal.toml");
    std::fs::write(&toml_path, "[proxy]\ntld = \"from-toml\"\n").unwrap();

    // Load with portal.toml present — json should be skipped entirely
    let config =
        Config::load_with_paths(None, Some(toml_path), &[]).unwrap();
    assert_eq!(config.proxy.tld, "from-toml");
}

#[test]
fn portless_key_missing_uses_defaults() {
    let temp = TempDir::new().unwrap();
    std::fs::write(
        temp.path().join("package.json"),
        r#"{"name":"myapp","version":"1.0.0"}"#,
    )
    .unwrap();

    // No "portless" key → returns None
    let result = load_partial_from_package_json(temp.path());
    assert!(result.is_none());
}

#[test]
fn malformed_portless_key_ignored() {
    let temp = TempDir::new().unwrap();
    std::fs::write(
        temp.path().join("package.json"),
        r#"{"portless":"not-an-object"}"#,
    )
    .unwrap();

    // Wrong type → returns None silently
    let result = load_partial_from_package_json(temp.path());
    assert!(result.is_none());
}

#[test]
fn portless_key_script_override() {
    let temp = TempDir::new().unwrap();
    std::fs::write(
        temp.path().join("package.json"),
        r#"{"portless":{"script":"start"}}"#,
    )
    .unwrap();

    let partial = load_partial_from_package_json(temp.path()).unwrap();
    let mut cfg = Config::default();
    apply_partial(&mut cfg, partial);
    assert_eq!(cfg.project.script, Some("start".to_string()));
}
```

Run to confirm they compile but fail (red):

```
cargo test portless_key -p portless 2>&1 | head -40
```

Expected: compile errors or test failures because `load_partial_from_package_json` doesn't exist yet.

### Step 1.6 — Implement `load_partial_from_package_json` and `find_and_load_package_json_config`

Add these two functions in `src/config.rs`, just above the `#[cfg(test)]` block:

```rust
/// Read `package.json` in `dir`, extract `["portless"]`, deserialize to `PartialConfig`.
/// Returns `None` if the file is absent, the key is missing, or parsing fails.
fn load_partial_from_package_json(dir: &Path) -> Option<PartialConfig> {
    let contents = std::fs::read_to_string(dir.join("package.json")).ok()?;
    let json: serde_json::Value = serde_json::from_str(&contents).ok()?;
    let portless_val = json.get("portless")?.clone();

    let portless: PartialPortlessJson =
        serde_json::from_value(portless_val)
            .map_err(|e| {
                tracing::debug!(
                    "package.json[\"portless\"] deserialization failed (ignored): {e}"
                );
                e
            })
            .ok()?;

    Some(PartialConfig {
        proxy: PartialProxyConfig {
            tld: portless.tld,
            https: portless.https,
            http_port: portless.http_port,
            https_port: portless.https_port,
            wildcard: portless.wildcard,
            lan: portless.lan,
            h2c: portless.h2c,
            ..Default::default()
        },
        daemon: PartialDaemonConfig::default(),
        project: PartialProjectConfig {
            name: portless.hostname,
            script: portless.script,
            ..Default::default()
        },
    })
}

/// Walk upward from `cwd` looking for a `package.json` that contains a
/// `"portless"` key.  Returns `Some(PartialConfig)` for the first match.
fn find_and_load_package_json_config(cwd: &Path) -> Option<PartialConfig> {
    let mut current = cwd;
    loop {
        if let Some(partial) = load_partial_from_package_json(current) {
            return Some(partial);
        }
        current = current.parent()?;
    }
}
```

### Step 1.7 — Run tests (green)

```
cargo test portless_key -p portless 2>&1
```

Expected output (all 6 pass):

```
running 6 tests
test config::tests::malformed_portless_key_ignored ... ok
test config::tests::portless_key_in_package_json_sets_tld ... ok
test config::tests::portless_key_missing_uses_defaults ... ok
test config::tests::portless_key_script_override ... ok
test config::tests::portless_key_sets_hostname ... ok
test config::tests::portal_toml_wins_over_package_json ... ok

test result: ok. 6 passed; 0 failed; 0 ignored
```

### Step 1.8 — Full test suite (no regressions)

```
cargo test -p portless 2>&1 | tail -5
```

Expected:

```
test result: ok. N passed; 0 failed; 0 ignored; 0 measured
```

---

## Task 2 — Wire into `load_with_paths`, create branch, commit, push

**File:** `src/config.rs`

### Step 2.1 — Update `load_with_paths`

Replace the "Layer 2" block in `Config::load_with_paths` to track whether a toml was found, and fall back to `package.json["portless"]` when it wasn't.

Current Layer 2 code (lines ~174-180):

```rust
// Layer 2: Load project config (overrides global)
if let Some(path) = project_path {
    if path.exists() {
        let contents = std::fs::read_to_string(&path)?;
        let partial: PartialConfig = toml::from_str(&contents)?;
        apply_partial(&mut config, partial);
    }
}
```

Replace with:

```rust
// Layer 2: Load project config (overrides global)
let mut toml_found = false;
if let Some(ref path) = project_path {
    if path.exists() {
        let contents = std::fs::read_to_string(path)?;
        let partial: PartialConfig = toml::from_str(&contents)?;
        apply_partial(&mut config, partial);
        toml_found = true;
    }
}

// Layer 2b: If no portal.toml found, try package.json["portless"]
if !toml_found {
    // Use the directory of the project_path if given; otherwise skip
    // (in the runtime `load` path, `find_project_toml` already walked upward,
    //  so we use the same cwd via a separate call to find_and_load_package_json_config
    //  only when called from `Config::load` — see below).
    //
    // For the testable case: callers can pass a project_path that points to a
    // package.json's parent directory by using the dedicated test helpers above.
    // The runtime wiring is done in `Config::load`.
}
```

### Step 2.2 — Update `Config::load` to call `find_and_load_package_json_config`

The runtime entry point `Config::load(cwd)` already calls `find_project_toml(cwd)`. Extend it:

Current `Config::load`:

```rust
pub fn load(cwd: &Path) -> Result<Self> {
    let global_path = dirs::home_dir().map(|h| h.join(".portal/config.toml"));
    let project_path = find_project_toml(cwd);

    let env_vars: Vec<(String, String)> = std::env::vars()
        .filter(|(k, _)| k.starts_with("PORTAL_"))
        .collect();
    let env_refs: Vec<(&str, &str)> = env_vars
        .iter()
        .map(|(k, v)| (k.as_str(), v.as_str()))
        .collect();
    Self::load_with_paths(global_path, project_path, &env_refs)
}
```

Replace with:

```rust
pub fn load(cwd: &Path) -> Result<Self> {
    let global_path = dirs::home_dir().map(|h| h.join(".portal/config.toml"));
    let project_path = find_project_toml(cwd);

    let env_vars: Vec<(String, String)> = std::env::vars()
        .filter(|(k, _)| k.starts_with("PORTAL_"))
        .collect();
    let env_refs: Vec<(&str, &str)> = env_vars
        .iter()
        .map(|(k, v)| (k.as_str(), v.as_str()))
        .collect();

    let mut config = Self::load_with_paths(global_path, project_path.clone(), &env_refs)?;

    // If no portal.toml was found in the upward walk, try package.json["portless"]
    if project_path.is_none() {
        if let Some(partial) = find_and_load_package_json_config(cwd) {
            apply_partial(&mut config, partial);
            // Re-apply env overrides so they still win over package.json
            apply_env_overrides(&mut config, &env_refs)?;
        }
    }

    Ok(config)
}
```

### Step 2.3 — Add an integration-style test for the full `load_with_paths` path

The `portal_toml_wins_over_package_json` test already exercises the toml-wins branch. Add one more test that exercises the search-from-subdirectory path via `find_and_load_package_json_config` directly:

```rust
#[test]
fn find_and_load_package_json_config_walks_upward() {
    let temp = TempDir::new().unwrap();
    std::fs::write(
        temp.path().join("package.json"),
        r#"{"portless":{"tld":"walked"}}"#,
    )
    .unwrap();
    let subdir = temp.path().join("src").join("components");
    std::fs::create_dir_all(&subdir).unwrap();

    let partial = find_and_load_package_json_config(&subdir).unwrap();
    let mut cfg = Config::default();
    apply_partial(&mut cfg, partial);
    assert_eq!(cfg.proxy.tld, "walked");
}
```

### Step 2.4 — Run full test suite

```
cargo test -p portless 2>&1 | tail -10
```

Expected: all tests pass, no regressions.

### Step 2.5 — Create branch and commit

```bash
cd /Users/__deesh_reddy__/projects/personal_git/rust_builds/portless
git checkout -b feature/package-json-portless-key
git add src/config.rs
git commit -m "feat(config): support package.json[\"portless\"] key for config loading

- Add \`script\` field to ProjectConfig and PartialProjectConfig
- Add PartialPortlessJson struct for flat JSON shape deserialization
- Implement load_partial_from_package_json (silent on missing/malformed)
- Implement find_and_load_package_json_config (walks upward)
- Wire into Config::load; portal.toml always wins over package.json
- 7 new tests (6 TDD + 1 integration walk test)"
```

### Step 2.6 — Push branch

```bash
git push -u origin feature/package-json-portless-key
```

Expected output:

```
Branch 'feature/package-json-portless-key' set up to track remote branch 'feature/package-json-portless-key' from 'origin'.
```

---

## Checklist

- [ ] Task 1.1 — `script` field added to `ProjectConfig`
- [ ] Task 1.2 — `script` field added to `PartialProjectConfig`
- [ ] Task 1.3 — `apply_partial` handles `script`
- [ ] Task 1.4 — `PartialPortlessJson` struct defined
- [ ] Task 1.5 — 6 TDD tests written and failing (red)
- [ ] Task 1.6 — `load_partial_from_package_json` + `find_and_load_package_json_config` implemented
- [ ] Task 1.7 — 6 TDD tests pass (green)
- [ ] Task 1.8 — Full suite passes
- [ ] Task 2.1 — `load_with_paths` updated
- [ ] Task 2.2 — `Config::load` updated
- [ ] Task 2.3 — Walk-upward integration test added and passing
- [ ] Task 2.4 — Full suite passes
- [ ] Task 2.5 — Committed on `feature/package-json-portless-key`
- [ ] Task 2.6 — Pushed to remote
