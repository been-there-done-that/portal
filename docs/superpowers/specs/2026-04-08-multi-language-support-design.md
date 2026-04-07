# Multi-Language Support Implementation Plan

**Goal:** Make portal work with any language/framework — Python, Go, Ruby, Rust, PHP — via a trait-based driver registry, with `portal.toml` as a universal override and a new `portal init` command for guided setup.

**Architecture:** A `LanguageDriver` trait + `DriverRegistry` replaces the current ad-hoc `Framework` enum and `detect.rs`. Each language is a self-contained driver file. `portal start` becomes fully language-agnostic. No backward-compatibility shims — clean break.

**Version:** 0.0.1 (pre-1.0, no semver constraints)

---

## Core Trait and Registry

### `LanguageDriver` trait (`src/detect/mod.rs`)

```rust
pub trait LanguageDriver: Send + Sync {
    /// Returns true if this driver recognises the project at `cwd`.
    fn detect(&self, cwd: &Path) -> bool;

    /// Higher priority = checked first. Default 50.
    fn priority(&self) -> u8 { 50 }

    /// Short identifier shown in portal init output, e.g. "Django (Python)".
    fn name(&self) -> &'static str;

    /// Extract project name from the language manifest (falls back to dir name).
    fn project_name(&self, cwd: &Path) -> Option<String>;

    /// The default dev-server command for this driver, e.g. "python manage.py runserver".
    fn start_command(&self, cwd: &Path) -> Option<String>;

    /// How to inject the assigned port into the child process.
    fn port_injection(&self, cwd: &Path, port: u16) -> PortInjection;
}
```

### `PortInjection` enum

```rust
pub enum PortInjection {
    /// Set PORT env var only. Always done regardless; this means nothing extra.
    EnvOnly,
    /// Append CLI flags, e.g. ["--port", "4123", "--host", "0.0.0.0"].
    CliArgs(Vec<String>),
    /// Append a host:port address positionally, e.g. "0.0.0.0:4123".
    AppendAddress(String),
    /// Full command string with {port} already substituted (used by PortalTomlDriver).
    Template(String),
}
```

### `DriverRegistry`

Holds an ordered `Vec<Box<dyn LanguageDriver>>` sorted descending by priority. `detect(cwd)` returns the first driver whose `detect()` returns true. Built once at startup with all built-in drivers registered.

### `PortalTomlDriver` (priority 255)

Always registered first. Reads `portal.toml` `[project]` section. Returns `Some` for any method that has a value in the config. Port injection precedence within this driver:

1. `{port}` in `start_command` → `Template` (substituted at call time)
2. `port_arg` + optional `host_arg` → `CliArgs`
3. `port_position = "append"` → `AppendAddress("0.0.0.0:{port}")`
4. Nothing specified → `EnvOnly`

---

## Built-in Drivers

### Priority order

| Priority | Driver | File |
|---|---|---|
| 255 | `PortalTomlDriver` | `mod.rs` |
| 90 | `DjangoDriver` | `python.rs` |
| 80 | `UvicornDriver` | `python.rs` |
| 80 | `FlaskDriver` | `python.rs` |
| 70 | `RailsDriver` | `ruby.rs` |
| 70 | `RackDriver` | `ruby.rs` |
| 60 | `PhpDriver` | `php.rs` |
| 50 | `GoDriver` | `go.rs` |
| 50 | `RustDriver` | `rust.rs` |
| 40 | `NodeDriver` | `node.rs` |

### Detection and injection per driver

**`DjangoDriver`**
- Detect: `manage.py` exists in cwd
- `start_command`: `"python manage.py runserver"`
- `port_injection`: `AppendAddress("0.0.0.0:{port}")`
- `project_name`: directory name (Django has no standard manifest name field)

**`UvicornDriver`**
- Detect: `pyproject.toml` or `requirements.txt` contains `uvicorn` or `fastapi`
- `start_command`: `"uvicorn main:app"` (user overrides via `portal.toml` for non-standard entry)
- `port_injection`: `CliArgs(["--host", "0.0.0.0", "--port", "{port}"])`
- `project_name`: `[project].name` from `pyproject.toml`, else dir name

**`FlaskDriver`**
- Detect: `pyproject.toml` or `requirements.txt` contains `flask`; or `app.py` / `wsgi.py` exists
- `start_command`: `"flask run"`
- `port_injection`: `CliArgs(["--host", "0.0.0.0", "--port", "{port}"])`
- `project_name`: `[project].name` from `pyproject.toml`, else dir name

**`RailsDriver`**
- Detect: `Gemfile` exists and contains `rails`
- `start_command`: `"rails server"`
- `port_injection`: `CliArgs(["-p", "{port}", "-b", "0.0.0.0"])`
- `project_name`: directory name

**`RackDriver`**
- Detect: `Gemfile` exists (no `rails`)
- `start_command`: `"bundle exec rackup"`
- `port_injection`: `CliArgs(["-p", "{port}", "-o", "0.0.0.0"])`
- `project_name`: directory name

**`PhpDriver`**
- Detect: `index.php` or `composer.json` exists
- `start_command`: `"php -S 0.0.0.0:{port}"` (port is part of the command itself)
- `port_injection`: `Template("php -S 0.0.0.0:{port}")` — no separate injection needed
- `project_name`: `name` from `composer.json`, else dir name

**`GoDriver`**
- Detect: `go.mod` exists
- `start_command`: `"go run ."`
- `port_injection`: `EnvOnly` (Go servers conventionally read `PORT`)
- `project_name`: module name from first line of `go.mod`

**`RustDriver`**
- Detect: `Cargo.toml` exists
- `start_command`: `"cargo run"`
- `port_injection`: `EnvOnly` (axum, actix-web, warp all respect `PORT`)
- `project_name`: `[package].name` from `Cargo.toml`

**`NodeDriver`**
- Detect: `package.json` exists
- `start_command`: uses existing `pick_dev_script` logic
- `port_injection`: uses existing `Framework` detection (Vite, Astro, Angular, etc.) → `CliArgs`
- `project_name`: `name` field from `package.json`, else dir name

---

## `portal.toml` Schema Extension

New `[project]` section (all fields optional):

```toml
[project]
name = "myapi"              # overrides auto-detected hostname segment
start_command = "uvicorn main:app --reload"   # overrides driver's start_command
port_arg = "--port"         # portal appends "--port 4123"
host_arg = "--host"         # portal appends "--host 0.0.0.0" (optional)
port_position = "append"    # alternative: appends "0.0.0.0:4123" to command
# Full template (takes precedence over port_arg / port_position):
# start_command = "php -S 0.0.0.0:{port} -t public"
```

`port_arg` and `port_position` are mutually exclusive; `{port}` in `start_command` takes precedence over both.

---

## `portal init` Command

### Flow

```
$ portal init
```

**Step 1 — Check for existing `portal.toml`**
If it exists: print `portal.toml already exists. Remove it first to reinitialise.` and exit.

**Step 2 — Run `DriverRegistry::detect(cwd)`** (excluding `PortalTomlDriver`)

**Step 3a — Driver matched (TTY available):**
```
✓ Detected  Django (Python)
  command   python manage.py runserver 0.0.0.0:4123
  name      myapi

Does this look right? [Y/n]
```
If yes → write `portal.toml` with the detected values.
If no → fall through to the manual prompt (Step 3b).

**Step 3a — Driver matched (no TTY):**
Write `portal.toml` with detected values directly, no prompt.

**Step 3b — No driver matched (TTY available):**
```
No project type detected.

What command starts your dev server?  > _

How does it accept a port?
  1. FLAG    --port 4123
  2. FLAG    -p 4123
  3. APPEND  command 0.0.0.0:4123
  4. ENV     PORT=4123 only
  5. CUSTOM  I'll write the full command with {port}
Choice [1]: _
```
Write `portal.toml` from answers.

**Step 3b — No driver matched (no TTY):**
Write `portal.toml` with placeholder comment:
```toml
[project]
# name = "myapp"
# start_command = "your-dev-command"
# port_arg = "--port"
# See: https://github.com/been-there-done-that/portal
```

---

## `portal start` Rewrite

Old behaviour: Node-only, reads `package.json`, errors if no `package.json`.

New behaviour:
1. Load `DriverRegistry`
2. `registry.detect(cwd)` → first matching driver
3. `driver.start_command(cwd)` → `Some(cmd)` or `None`
4. If `None` → print `No start command detected. Run portal init to configure this project.` and exit 1
5. `driver.port_injection(cwd, port)` → `PortInjection`
6. Build final argv + env vars (`PORT` always set), spawn child via existing `spawn_child` path

The old `CliCommand::Start` handler is fully replaced. No `package.json` special-casing remains in `cli/mod.rs`.

---

## Error UX

| Situation | Message |
|---|---|
| No driver matches, no `portal.toml` | `No supported project detected. Run portal init to set up this project.` |
| Driver matched, no start command | `Detected <name> but couldn't determine a start command. Run portal init.` |
| `start_command` binary not in PATH | `Command not found: uvicorn. Is it installed and in your PATH?` |
| Multiple drivers match same priority | Highest-priority wins silently; no user-visible ambiguity |
| `portal init` with existing `portal.toml` | `portal.toml already exists. Remove it first to reinitialise.` |

---

## Files Changed

### Created
- `src/detect/mod.rs` — `LanguageDriver` trait, `PortInjection`, `DriverRegistry`, `PortalTomlDriver`
- `src/detect/node.rs` — `NodeDriver` (absorbs all JS/Node logic from old `detect.rs`)
- `src/detect/python.rs` — `DjangoDriver`, `UvicornDriver`, `FlaskDriver`
- `src/detect/go.rs` — `GoDriver`
- `src/detect/ruby.rs` — `RailsDriver`, `RackDriver`
- `src/detect/rust.rs` — `RustDriver`
- `src/detect/php.rs` — `PhpDriver`

### Modified
- `src/config.rs` — add `ProjectConfig` fields: `start_command`, `port_arg`, `host_arg`, `port_position`
- `src/process.rs` — replace `extra_args_for_port()` with `PortInjection` dispatch
- `src/cli/mod.rs` — add `CliCommand::Init`, rewrite `CliCommand::Start` handler

### Deleted
- `src/detect.rs` — all 632 lines replaced by `src/detect/`

---

## Testing Strategy

- Each driver file has unit tests using `TempDir` — same pattern as current `detect.rs` tests
- `DriverRegistry` gets integration tests: precedence (portal.toml beats auto-detect), no-match path, priority tie-breaking
- `portal init` non-TTY path tested with a temp directory for each detection scenario
- Interactive TTY path tested manually
- Existing `process.rs` tests are preserved; `spawn_child` signature unchanged
