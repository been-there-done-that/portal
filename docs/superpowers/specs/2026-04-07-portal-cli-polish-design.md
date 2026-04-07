# Portal CLI Polish — Design Spec

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Make `portal` feel like a first-class CLI tool — animated setup feedback, a polished startup banner, and smart script shortcuts so you never have to type `npm run` again.

**Architecture:** New `src/cli/banner.rs` owns all terminal output. `indicatif` + `console` replace raw `eprintln!`. `portal start` and smart `portal run <script>` shorthand added. All changes are additive — no protocol, proxy, or daemon changes.

**Tech Stack:** Rust, `indicatif 0.17` (animated spinners/MultiProgress), `console 0.15` (colors/styles, TTY detection).

---

## Part 0: Dependencies

Add to `Cargo.toml`:

```toml
indicatif = "0.17"
console = "0.15"
```

Remove `owo-colors` if it was added previously (it was not — good, nothing to remove).

---

## Part 1: Script Shorthand

### 1a. `portal start` — zero-arg subcommand

New `CliCommand::Start` variant. No args required.

**Behavior:**
1. Load `cwd` + `config`
2. Run `ensure_daemon_running` + `ensure_cert_trusted` (same as `Run`)
3. Detect package manager from cwd lock files (see below)
4. Read `package.json` scripts object
5. Pick best script in priority order: `dev` → `start` → `serve` → `develop` → first script alphabetically
6. Build args: `[<pm>, "run", <script>]`
7. Continue with hostname resolution, port assignment, spawn, banner — identical to `Run`

**Error if no `package.json`:** `error: no package.json found in {cwd} (or any parent). Use 'portal run <command>' to run an arbitrary command.`

### 1b. `portal run <script>` — smart script detection

Existing `Run` variant gains auto-detection. If `args[0]` is NOT a known runner, treat it as a package.json script name:

```
portal run dev          → pnpm run dev      (pnpm-lock.yaml detected)
portal run build        → npm run build     (no lock file)
portal run npm run dev  → npm run dev       (unchanged — npm is a known runner)
portal run node server  → node server       (unchanged — node is a known runner)
```

**Known runners list** (checked with `is_known_runner()`):
`npm`, `pnpm`, `yarn`, `bun`, `node`, `deno`, `npx`, `bunx`, `pnpx`, `python`, `python3`, `ruby`, `go`, `cargo`, `java`, `sh`, `bash`, `zsh`, `fish`

If `args[0]` is not in that list AND `package.json` exists with that script → prepend `<pm> run`.
If `args[0]` is not in that list AND no `package.json` / script not found → fall through to current behavior (treat as literal command, let the OS error).

### 1c. Package manager detection

New `pub fn detect_package_manager(cwd: &Path) -> &'static str` in `src/detect.rs`:

| Lock file | Returns |
|-----------|---------|
| `pnpm-lock.yaml` | `"pnpm"` |
| `bun.lockb` or `bun.lock` | `"bun"` |
| `yarn.lock` | `"yarn"` |
| (none of the above) | `"npm"` |

Checked in that order; first match wins.

---

## Part 2: Startup Banner (style C)

New `pub fn print_banner(hostname: &str, port: u16, pid: u32, replaced: bool)` in `src/cli/banner.rs`.

**Normal run:**
```
  portal  v1.0.0  ·  ● running

  https://myapp.localhost
  └─ localhost:4123  ·  cert ✓  ·  pid 91842
```

**Replace (previous instance stopped):**
```
  portal  v1.0.0  ·  ● replaced

  https://myapp.localhost
  └─ localhost:4123  ·  cert ✓  ·  pid 91842
```

**Color mapping (via `console::style()`):**
| Element | Style |
|---------|-------|
| `portal` | bright white bold, blue bg (`on_blue`) |
| `v1.0.0` | dim |
| `● running` | green |
| `● replaced` | yellow |
| URL (`https://...`) | bold white |
| `└─ localhost:PORT` | dim |
| port number | red |
| `cert ✓` | green |
| `pid NNNN` | dim |

**Integration:** Replace `eprintln!("  https://{hostname}  ->  port {port}")` in `cli/mod.rs` with `banner::print_banner(&hostname, port, child_pid, replaced)`. The `replaced` flag is `true` when `reuse_port.is_some()`.

**Also update `print_ls` and `print_status` in `src/cli/output.rs`** to use `console::style()` for the same color palette — dim headers, colored URLs, etc.

---

## Part 3: Animated Setup Tree (style C with spinners)

### 3a. `SetupPrinter` struct in `src/cli/banner.rs`

```rust
pub struct SetupPrinter {
    mp: indicatif::MultiProgress,
    started: bool,
}

impl SetupPrinter {
    pub fn new() -> Self;

    /// Print the "portal  v1.0.0  · first run" header (once, on first step).
    fn ensure_header(&mut self);

    /// Add an animated spinner for a step. Returns a handle to finish it.
    pub fn begin_step(&mut self, name: &str, msg: &str) -> indicatif::ProgressBar;

    /// Print the "╰─ ready" footer and clear the MultiProgress.
    /// No-op if no steps were started.
    pub fn done(self);
}
```

**Spinner style:** `{spinner:.cyan} {msg}` with frames `⠋⠙⠹⠸⠼⠴⠦⠧⠇⠏` at 80ms tick.

**Step lifecycle:**
```rust
let pb = setup.begin_step("trust", "installing…  (sudo required)");
// ... run sudo ...
pb.finish_with_message(format!("{} trust   installed  (sudo)", console::style("✓").green()));
```

### 3b. Updated `ensure_daemon_running` signature

```rust
async fn ensure_daemon_running(
    config: &crate::config::Config,
    setup: &mut banner::SetupPrinter,
) -> Result<()>
```

- If daemon already running: no step printed.
- If daemon needs starting: `setup.begin_step("daemon", "starting…")` → spawn → poll socket → finish step with `"✓ daemon  started on :PORT"`.

### 3c. Updated `ensure_cert_trusted` signature

```rust
async fn ensure_cert_trusted(setup: &mut banner::SetupPrinter) -> Result<()>
```

- If already trusted: no step printed.
- If needs install: `setup.begin_step("trust", "installing…  (sudo required)")` → run sudo → finish or error.
- CA generation (when CA files don't exist yet): `setup.begin_step("cert", "generating…")` before `ensure_ca()` call inside daemon startup. But `ensure_ca` runs inside the daemon, not the CLI — so for the cert step we check if `~/.portal/ca.pem` exists before calling daemon start:
  - If `ca.pem` missing: `setup.begin_step("cert", "generating…")` → note only; cert generation happens inside the daemon on first start. Finish step after daemon is up.
  - If `ca.pem` exists: no cert step.

### 3d. Call site in `Run` / `Start` handler

```rust
let mut setup = banner::SetupPrinter::new();
ensure_daemon_running(&config, &mut setup).await?;
ensure_cert_trusted(&mut setup).await?;
setup.done();
// ... then spawn child and print banner ...
banner::print_banner(&hostname, port, child_pid, replaced);
```

### 3e. TTY / CI behavior

`indicatif` automatically disables animations when stdout is not a TTY (piped, CI). `console` also respects `NO_COLOR`. No special handling needed — both crates manage this.

---

## Part 4: File Change Map

| File | Change |
|------|--------|
| `Cargo.toml` | Add `indicatif = "0.17"`, `console = "0.15"` |
| `src/cli/banner.rs` | **Create** — `print_banner()`, `SetupPrinter` |
| `src/cli/mod.rs` | Add `Start` variant; smart detection in `Run`; wire `SetupPrinter`; update `ensure_*` signatures |
| `src/cli/output.rs` | Update `print_ls` + `print_status` with `console::style()` colors |
| `src/detect.rs` | Add `detect_package_manager()`, `KNOWN_RUNNERS`, `is_known_runner()` |
| `src/lib.rs` | Export `detect` module publicly if not already (needed by integration tests) |

---

## Part 5: Out of Scope

- `portal ls` live-updating table (would need ratatui — separate feature)
- `portal status` interactive dashboard
- Color themes / config
- Windows-specific spinner fallback (indicatif handles this automatically)
