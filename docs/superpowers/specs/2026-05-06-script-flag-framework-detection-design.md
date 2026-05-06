# Group A: `--script` Flag + Rsbuild/VitePress Detection — portal v0.4.0

**Date:** 2026-05-06
**Branch:** `feature/script-flag-framework-detection`
**Base branch:** `main`

---

## Goal

Two independent, additive changes to `src/detect/node.rs` and `src/cli/mod.rs`:

1. **`--script <name>`** — CLI flag that overrides auto-detected npm script selection.
2. **Rsbuild + VitePress detection** — two new frameworks in `detect_framework()` with correct port injection.

---

## Architecture

No new files. All changes are within the existing detect and CLI layers. The `--script` flag threads through `CliCommand::Run` → `do_run` → `resolve_run_args` / `start_command`. The framework additions extend the existing `Framework` enum and `detect_framework` function.

---

## Feature 1: `--script <name>` Flag

### CLI (`src/cli/mod.rs`)

Add to `CliCommand::Run`:

```rust
/// Override the npm/pnpm/yarn/bun script to run (default: auto-detected from package.json)
#[arg(long)]
script: Option<String>,
```

Unpack `script` in the `CliCommand::Run` match arm and pass to `do_run`.

Add `script: Option<String>` to `do_run` signature. Update the `CliCommand::Start` call to pass `None`.

### Detect (`src/detect/node.rs`)

Change `pick_dev_script` signature to accept an optional override:

```rust
pub fn pick_script(json: &serde_json::Value, override_script: Option<&str>) -> Option<String>
```

- If `override_script` is `Some(name)`, validate it exists in `scripts` object; return it if found, `None` if not (so the caller can emit a warning).
- If `None`, apply the existing priority logic (`dev` > `start` > `serve` > `develop` > alphabetical first).

Rename the private `pick_dev_script` to `pick_script` (public) so CLI can call it directly. Update `NodeDriver::start_command` to call `pick_script(json, None)`.

### CLI wiring

In `do_run`, when building `resolved_args`:

- If `args` is empty (auto-detect mode) AND `script` is `Some(name)`:
  - Read `package.json` from `cwd`
  - Call `pick_script(json, Some(name))` — if `None`, print `warning: script "{name}" not found in package.json, falling back to auto-detect`
  - If found: set `resolved_args = vec![pm, "run", name]`
- If `args` is non-empty, `--script` is ignored (user already specified explicit args).

### `resolve_run_args` — unchanged

`resolve_run_args` handles the case where the user passes a bare script name like `portal run dev`. The `--script` flag is a separate, explicit override path.

---

## Feature 2: Rsbuild + VitePress Detection

### `detect_framework` additions (`src/detect/node.rs`)

**Rsbuild** — Add before the Vite check:

Detection: `rsbuild.config.ts` or `rsbuild.config.js` exists in `cwd`, OR `"rsbuild"` appears in the scripts string.

Port injection: `--port <N>` (same as Vite, no `--host` needed — Rsbuild binds to all interfaces by default).

**VitePress** — Add before the Vite check:

Detection: `"vitepress"` appears in the scripts string.

Port injection: `--port <N>`.

Both must be checked **before** the generic `"vite"` check because VitePress scripts also contain `"vite"` indirectly; Rsbuild scripts may contain `"vite"` in some setups.

### Framework enum additions

```rust
enum Framework {
    Vite,
    Rsbuild,    // new
    VitePress,  // new
    Astro,
    // ... existing ...
}
```

Port args:
```rust
Framework::Rsbuild => vec!["--port".into(), p],
Framework::VitePress => vec!["--port".into(), p],
```

---

## Error Handling

- `--script <name>` with a name not in `package.json`: print warning to stderr, fall back to auto-detect. Never hard-error — the user may be in a non-Node project.
- `--script` in a non-Node project: silently ignored (no `package.json` to look up).

---

## Testing

**`src/detect/node.rs`:**
- `pick_script_override_uses_named_script` — override `"start"` returns `"start"` when it exists
- `pick_script_override_falls_back_when_missing` — override `"nonexistent"` returns `None`
- `rsbuild_detection_from_config_file` — `rsbuild.config.ts` present → `Framework::Rsbuild`
- `rsbuild_detection_from_scripts` — scripts contain `"rsbuild"` → `Framework::Rsbuild`
- `vitepress_detection_from_scripts` — scripts contain `"vitepress"` → `Framework::VitePress`
- `rsbuild_port_injection` — `PortInjection::CliArgs(["--port", "4000"])`
- `vitepress_port_injection` — `PortInjection::CliArgs(["--port", "4000"])`

**`src/cli/mod.rs`:**
- `run_command_has_script_arg` — clap test verifying `--script` flag exists

---

## Affected Files

| File | Change |
|------|--------|
| `src/detect/node.rs` | Add `Rsbuild`, `VitePress` variants; rename/extend `pick_dev_script` → `pick_script` |
| `src/cli/mod.rs` | Add `--script` flag, thread through `do_run` |
