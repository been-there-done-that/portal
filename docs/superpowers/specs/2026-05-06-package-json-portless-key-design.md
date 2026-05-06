# Group B: `package.json "portless"` Config Key — portal v0.4.0

**Date:** 2026-05-06
**Branch:** `feature/package-json-portless-key`
**Base branch:** `main`

---

## Goal

Allow developers to configure portal from their existing `package.json` by adding a `"portless"` top-level key, eliminating the need for a separate `portal.toml` file in Node.js projects.

---

## Architecture

All changes are in `src/config.rs`. The config loading pipeline already has a `PartialConfig` → `apply_partial` → `apply_env_overrides` layering. We add a new `load_from_package_json` step that produces a `PartialConfig` and feeds into the same pipeline.

**Merge priority (highest → lowest):**
`portal.toml` > `package.json["portless"]` > env vars > compiled defaults

---

## Config Loading Changes (`src/config.rs`)

### New function: `load_partial_from_package_json`

```rust
fn load_partial_from_package_json(dir: &Path) -> Option<PartialConfig>
```

- Reads `package.json` from `dir`
- Parses JSON
- Extracts the `"portless"` key as a `serde_json::Value`
- Deserializes it into `PartialConfig` via `serde_json::from_value`
- Returns `None` if file missing, key absent, or parse fails (silent — not an error)

### Updated `Config::load_with_paths`

The existing upward search finds `portal.toml`. Extend this:

1. Walk upward from `cwd` looking for `portal.toml` — if found, load as `PartialConfig` (existing behaviour).
2. If `portal.toml` **not found**, walk upward looking for `package.json` containing a `"portless"` key — if found, call `load_partial_from_package_json`.
3. If neither found, use empty `PartialConfig` (existing behaviour).

The two searches use the same upward walk limit. `portal.toml` always wins — if both exist in the same directory tree, `portal.toml` is used and `package.json["portless"]` is ignored.

---

## Supported Keys

The `"portless"` JSON object maps to `PartialConfig` fields. Supported keys (all optional):

```json
{
  "portless": {
    "tld": "localhost",
    "hostname": "myapp",
    "port": 4000,
    "https": true,
    "http_port": 80,
    "https_port": 443,
    "wildcard": false,
    "lan": false,
    "script": "start",
    "h2c": false
  }
}
```

The `"script"` key in `"portless"` provides the same override as `--script` CLI flag. CLI flag takes precedence over `package.json["portless"]["script"]`.

### `PartialConfig` additions

`PartialConfig` needs a `script: Option<String>` field so it can carry the script override through the pipeline. This is also used by Group A's `--script` flag — both paths write to the same `Config.project.script` field.

Add to `ProjectConfig` in `src/config.rs`:
```rust
#[serde(default)]
pub script: Option<String>,
```

And to `PartialProjectConfig`:
```rust
pub script: Option<String>,
```

---

## Error Handling

- Malformed `"portless"` JSON value (e.g. wrong types): skip silently with a `tracing::debug!` log. Never fail config loading because of a bad `package.json`.
- Unknown keys in `"portless"`: ignored by serde (default behaviour).
- `portal.toml` present in parent directory, `package.json["portless"]` present in child: `portal.toml` wins (current directory walk stops at first match).

---

## Testing

In `src/config.rs`:

- `portless_key_in_package_json_sets_tld` — `package.json` with `"portless": {"tld": "test"}` → `config.proxy.tld == "test"`
- `portless_key_sets_hostname` — `"portless": {"hostname": "myapi"}` → `config.project.name == Some("myapi")` (the `hostname` JSON key maps to `ProjectConfig.name` via `#[serde(alias = "hostname")]`)
- `portal_toml_wins_over_package_json` — both present → `portal.toml` values used
- `portless_key_missing_uses_defaults` — `package.json` with no `"portless"` key → all defaults
- `malformed_portless_key_ignored` — `"portless": "not-an-object"` → silent, uses defaults
- `portless_key_script_override` — `"portless": {"script": "start"}` → `config.project.script == Some("start")`

---

## Affected Files

| File | Change |
|------|--------|
| `src/config.rs` | Add `load_partial_from_package_json`, extend `load_with_paths`, add `script` to `ProjectConfig` / `PartialProjectConfig` |
