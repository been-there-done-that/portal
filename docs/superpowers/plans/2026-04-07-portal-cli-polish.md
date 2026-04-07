# Portal CLI Polish Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add `portal start`, smart `portal run <script>`, animated first-run setup tree, and a polished startup banner using `indicatif` + `console`.

**Architecture:** New `src/cli/banner.rs` owns all terminal output. `ensure_daemon_running` and `ensure_cert_trusted` gain a `&mut SetupPrinter` parameter for animated spinners. Run logic extracted into private `do_run()` helper shared by `Run` and new `Start` variants. Smart run detection checks `KNOWN_RUNNERS` in `src/detect.rs`.

**Tech Stack:** Rust, `indicatif 0.17` (animated spinners/MultiProgress), `console 0.15` (colors/styles, TTY detection).

---

## File Map

| File | Change |
|------|--------|
| `Cargo.toml` | Add `indicatif = "0.17"`, `console = "0.15"` |
| `src/detect.rs` | Add `KNOWN_RUNNERS`, `is_known_runner()`, `detect_package_manager()`, `pick_dev_script()` |
| `src/cli/banner.rs` | **Create** — `print_banner()`, `SetupPrinter` |
| `src/cli/mod.rs` | Add `pub mod banner;`; update `ensure_daemon_running` + `ensure_cert_trusted` signatures; extract `do_run()` helper; add `Start` variant; smart script detection |
| `src/cli/output.rs` | Add `console::style()` colors to `print_ls` and `print_status` |

---

## Task 1: Add Dependencies

**Files:**
- Modify: `Cargo.toml`

- [ ] **Step 1: Add indicatif and console to Cargo.toml**

In the `[dependencies]` section, after `dirs = "5"`:

```toml
indicatif = "0.17"
console = "0.15"
```

- [ ] **Step 2: Verify the build compiles**

```bash
cd /Users/__deesh_reddy__/projects/personal_git/rust_builds/portless
cargo build 2>&1 | tail -5
```

Expected: `Finished dev` (new crates download and compile)

- [ ] **Step 3: Commit**

```bash
git add Cargo.toml Cargo.lock
git commit -m "deps: add indicatif 0.17 and console 0.15"
```

---

## Task 2: Detection Helpers in src/detect.rs

**Files:**
- Modify: `src/detect.rs`

- [ ] **Step 1: Write failing tests**

Add to the `#[cfg(test)]` block at the bottom of `src/detect.rs`:

```rust
    #[test]
    fn known_runners_basic() {
        assert!(is_known_runner("npm"));
        assert!(is_known_runner("pnpm"));
        assert!(is_known_runner("node"));
        assert!(!is_known_runner("vite"));
        assert!(!is_known_runner("dev"));
    }

    #[test]
    fn detects_pnpm_from_lock() {
        let temp = TempDir::new().unwrap();
        fs::write(temp.path().join("pnpm-lock.yaml"), "").unwrap();
        assert_eq!(detect_package_manager(temp.path()), "pnpm");
    }

    #[test]
    fn detects_bun_from_lockb() {
        let temp = TempDir::new().unwrap();
        fs::write(temp.path().join("bun.lockb"), "").unwrap();
        assert_eq!(detect_package_manager(temp.path()), "bun");
    }

    #[test]
    fn detects_yarn_from_lock() {
        let temp = TempDir::new().unwrap();
        fs::write(temp.path().join("yarn.lock"), "").unwrap();
        assert_eq!(detect_package_manager(temp.path()), "yarn");
    }

    #[test]
    fn defaults_to_npm() {
        let temp = TempDir::new().unwrap();
        assert_eq!(detect_package_manager(temp.path()), "npm");
    }

    #[test]
    fn pnpm_beats_yarn_when_both_present() {
        let temp = TempDir::new().unwrap();
        fs::write(temp.path().join("pnpm-lock.yaml"), "").unwrap();
        fs::write(temp.path().join("yarn.lock"), "").unwrap();
        assert_eq!(detect_package_manager(temp.path()), "pnpm");
    }

    #[test]
    fn picks_dev_script_priority() {
        let json = serde_json::json!({ "scripts": { "build": "tsc", "dev": "vite", "test": "vitest" } });
        assert_eq!(pick_dev_script(&json).as_deref(), Some("dev"));
    }

    #[test]
    fn picks_start_when_no_dev() {
        let json = serde_json::json!({ "scripts": { "build": "tsc", "start": "node server.js" } });
        assert_eq!(pick_dev_script(&json).as_deref(), Some("start"));
    }

    #[test]
    fn picks_first_alphabetically_as_fallback() {
        let json = serde_json::json!({ "scripts": { "build": "tsc", "preview": "vite preview" } });
        // "build" < "preview" alphabetically
        assert_eq!(pick_dev_script(&json).as_deref(), Some("build"));
    }

    #[test]
    fn pick_dev_script_no_scripts() {
        let json = serde_json::json!({ "name": "my-app" });
        assert_eq!(pick_dev_script(&json), None);
    }
```

- [ ] **Step 2: Run tests to verify they fail**

```bash
cargo test -q --lib detect 2>&1 | head -20
```

Expected: errors about `is_known_runner`, `detect_package_manager`, `pick_dev_script` not being defined.

- [ ] **Step 3: Implement the new functions**

Add before the existing `strip_runner_prefix` function in `src/detect.rs`:

```rust
/// Known package runners / executors. If `args[0]` is NOT in this list and
/// `package.json` has a matching script, we prepend `<pm> run` automatically.
pub const KNOWN_RUNNERS: &[&str] = &[
    "npm", "pnpm", "yarn", "bun", "node", "deno", "npx", "bunx", "pnpx",
    "python", "python3", "ruby", "go", "cargo", "java", "sh", "bash", "zsh", "fish",
];

/// Returns true if `cmd` is a known package runner / executor.
pub fn is_known_runner(cmd: &str) -> bool {
    KNOWN_RUNNERS.contains(&cmd)
}

/// Detect which package manager to use based on lock files in `cwd`.
/// Checked in priority order: pnpm → bun → yarn → npm (default).
pub fn detect_package_manager(cwd: &Path) -> &'static str {
    if cwd.join("pnpm-lock.yaml").exists() {
        return "pnpm";
    }
    if cwd.join("bun.lockb").exists() || cwd.join("bun.lock").exists() {
        return "bun";
    }
    if cwd.join("yarn.lock").exists() {
        return "yarn";
    }
    "npm"
}

/// Pick the best dev script from a parsed `package.json` value.
/// Priority: dev → start → serve → develop → first script alphabetically.
/// Returns `None` if the JSON has no `scripts` object or it is empty.
pub fn pick_dev_script(json: &serde_json::Value) -> Option<String> {
    let scripts = json.get("scripts")?.as_object()?;
    if scripts.is_empty() {
        return None;
    }
    for &preferred in &["dev", "start", "serve", "develop"] {
        if scripts.contains_key(preferred) {
            return Some(preferred.to_string());
        }
    }
    scripts.keys().min().cloned()
}
```

- [ ] **Step 4: Run tests to verify they pass**

```bash
cargo test -q --lib detect 2>&1 | tail -10
```

Expected: all detect tests pass, including the new ones.

- [ ] **Step 5: Commit**

```bash
git add src/detect.rs
git commit -m "feat(detect): add KNOWN_RUNNERS, detect_package_manager, pick_dev_script"
```

---

## Task 3: Create src/cli/banner.rs

**Files:**
- Create: `src/cli/banner.rs`

- [ ] **Step 1: Create the file**

Create `src/cli/banner.rs` with this content:

```rust
use console::style;
use indicatif::{MultiProgress, ProgressBar, ProgressStyle};

/// Print the Go Fiber-style startup banner after the child process is spawned.
///
/// ```text
///   portal  v1.0.0  ·  ● running
///
///   https://myapp.localhost
///   └─ localhost:4123  ·  cert ✓  ·  pid 91842
/// ```
pub fn print_banner(hostname: &str, port: u16, pid: u32, replaced: bool) {
    let version = env!("CARGO_PKG_VERSION");
    let badge = style(" portal ").bold().white().on_blue();
    let ver = style(format!("v{version}")).dim();
    let status_dot = if replaced {
        style("● replaced").yellow().to_string()
    } else {
        style("● running").green().to_string()
    };
    eprintln!("  {badge}  {ver}  ·  {status_dot}");
    eprintln!();
    eprintln!("  {}", style(format!("https://{hostname}")).bold().white());
    eprintln!(
        "  {}{}  ·  {}  ·  {}",
        style("└─ localhost:").dim(),
        style(port.to_string()).red(),
        style("cert ✓").green(),
        style(format!("pid {pid}")).dim(),
    );
}

/// Manages animated setup steps printed before the first run of a project.
///
/// Steps are shown as a tree with `indicatif` spinners:
/// ```text
///   portal  v1.0.0  ·  first run
///
///   ├─ cert    generating…
///   ├─ daemon  starting…
///   ╰─ ready
/// ```
pub struct SetupPrinter {
    mp: MultiProgress,
    started: bool,
}

impl SetupPrinter {
    pub fn new() -> Self {
        Self {
            mp: MultiProgress::new(),
            started: false,
        }
    }

    /// Print the header line once, on the first step.
    fn ensure_header(&mut self) {
        if !self.started {
            self.started = true;
            let version = env!("CARGO_PKG_VERSION");
            let badge = style(" portal ").bold().white().on_blue();
            let label = style(format!("v{version}  ·  first run")).dim();
            eprintln!("  {badge}  {label}");
            eprintln!();
        }
    }

    /// Add an animated spinner for a setup step. Returns a handle to finish it.
    ///
    /// ```rust
    /// let pb = setup.begin_step("daemon", "starting…");
    /// // ... run async work ...
    /// pb.finish_with_message(format!("{} daemon  started", console::style("✓").green()));
    /// ```
    pub fn begin_step(&mut self, _name: &str, msg: &str) -> ProgressBar {
        self.ensure_header();
        let pb = self.mp.add(ProgressBar::new_spinner());
        let spinner_style = ProgressStyle::with_template("  {spinner:.cyan} {msg}")
            .unwrap()
            .tick_strings(&[
                "⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏",
            ]);
        pb.set_style(spinner_style);
        pb.set_message(msg.to_string());
        pb.enable_steady_tick(std::time::Duration::from_millis(80));
        pb
    }

    /// Print the `╰─ ready` footer and clear the MultiProgress.
    /// No-op if no steps were started (nothing to display).
    pub fn done(self) {
        if self.started {
            eprintln!("  {}", style("╰─ ready").dim());
            eprintln!();
        }
    }
}

impl Default for SetupPrinter {
    fn default() -> Self {
        Self::new()
    }
}
```

- [ ] **Step 2: Add `pub mod banner;` to src/cli/mod.rs**

At the top of `src/cli/mod.rs`, change:

```rust
pub mod output;
```

to:

```rust
pub mod banner;
pub mod output;
```

- [ ] **Step 3: Verify it compiles**

```bash
cargo build 2>&1 | grep -E "error|warning: unused" | head -20
```

Expected: no errors. May see unused warnings for `print_banner` or `SetupPrinter` — those are fine and will be resolved in Task 4.

- [ ] **Step 4: Commit**

```bash
git add src/cli/banner.rs src/cli/mod.rs
git commit -m "feat(banner): create SetupPrinter and print_banner"
```

---

## Task 4: Update ensure_daemon_running and ensure_cert_trusted

**Files:**
- Modify: `src/cli/mod.rs`

The two private async functions need a `&mut banner::SetupPrinter` parameter so they can emit animated steps. The daemon function also shows a cert-generation step when `~/.portal/ca.pem` doesn't exist yet.

- [ ] **Step 1: Replace ensure_daemon_running (lines 246–274 in src/cli/mod.rs)**

Replace the entire `ensure_daemon_running` function with:

```rust
async fn ensure_daemon_running(
    config: &crate::config::Config,
    setup: &mut banner::SetupPrinter,
) -> Result<()> {
    let sock = crate::config::dirs_for_state().join("portal.sock");
    if sock.exists() && tokio::net::UnixStream::connect(&sock).await.is_ok() {
        return Ok(());
    }

    // If ca.pem doesn't exist yet, the daemon will generate it on first start.
    // Show a cert step so the user knows something is happening.
    let ca_pem_path = crate::config::dirs_for_state().join("ca.pem");
    let cert_pb: Option<indicatif::ProgressBar> = if !ca_pem_path.exists() {
        Some(setup.begin_step("cert", "generating CA certificate…"))
    } else {
        None
    };

    let daemon_pb = setup.begin_step("daemon", "starting…");

    let exe = std::env::current_exe()?;
    let needs_sudo = config.proxy.http_port < 1024 || config.proxy.https_port < 1024;
    if needs_sudo {
        tokio::process::Command::new("sudo")
            .arg(&exe)
            .arg("daemon")
            .env("PORTAL_IS_DAEMON", "1")
            .spawn()?;
    } else {
        tokio::process::Command::new(&exe)
            .arg("daemon")
            .env("PORTAL_IS_DAEMON", "1")
            .spawn()?;
    }

    for _ in 0..20 {
        tokio::time::sleep(std::time::Duration::from_millis(150)).await;
        if tokio::net::UnixStream::connect(&sock).await.is_ok() {
            if let Some(pb) = cert_pb {
                pb.finish_with_message(format!(
                    "{} cert    generated",
                    console::style("✓").green()
                ));
            }
            daemon_pb.finish_with_message(format!(
                "{} daemon  started  on :{}/:{}",
                console::style("✓").green(),
                config.proxy.http_port,
                config.proxy.https_port,
            ));
            return Ok(());
        }
    }

    daemon_pb.abandon_with_message(format!(
        "{} daemon  failed to start",
        console::style("✗").red()
    ));
    Err(crate::error::Error::DaemonNotRunning)
}
```

- [ ] **Step 2: Replace ensure_cert_trusted (lines 276–295 in src/cli/mod.rs)**

Replace the entire `ensure_cert_trusted` function with:

```rust
async fn ensure_cert_trusted(setup: &mut banner::SetupPrinter) -> Result<()> {
    if crate::certs::is_ca_trusted() {
        return Ok(());
    }

    let trust_pb = setup.begin_step("trust", "installing CA certificate…  (sudo required)");

    let exe = std::env::current_exe()?;
    let status = tokio::process::Command::new("sudo")
        .arg(&exe)
        .arg("cert")
        .arg("install")
        .status()
        .await?;

    if !status.success() {
        trust_pb.abandon_with_message(format!(
            "{} trust   failed  (run `sudo portal cert install` manually)",
            console::style("✗").red()
        ));
        return Err(crate::error::Error::Cert(
            "Failed to install CA certificate. Run `sudo portal cert install` manually."
                .to_string(),
        ));
    }

    trust_pb.finish_with_message(format!(
        "{} trust   installed  (sudo)",
        console::style("✓").green()
    ));
    Ok(())
}
```

- [ ] **Step 3: Fix the call site in the Ls arm (line ~68)**

The `Ls` arm calls `ensure_daemon_running(&config)` with the old signature. Update it:

```rust
        CliCommand::Ls => {
            let cwd = std::env::current_dir()?;
            let config = crate::config::Config::load(&cwd)?;
            let mut setup = banner::SetupPrinter::new();
            ensure_daemon_running(&config, &mut setup).await?;
            setup.done();
```

- [ ] **Step 4: Verify compilation**

```bash
cargo build 2>&1 | grep "error" | head -20
```

Expected: no errors. The `Run` arm still calls `ensure_daemon_running(&config)` with the old signature — it will error. Fix it in the next step.

- [ ] **Step 5: Update the Run arm's ensure_* call sites (temporary fix before Task 5 refactor)**

Find the two lines in the `Run` arm (around line 150):

```rust
            ensure_daemon_running(&config).await?;
            ensure_cert_trusted().await?;
```

Replace with:

```rust
            let mut setup = banner::SetupPrinter::new();
            ensure_daemon_running(&config, &mut setup).await?;
            ensure_cert_trusted(&mut setup).await?;
            setup.done();
```

- [ ] **Step 6: Verify compilation and tests**

```bash
cargo build 2>&1 | grep "error" | head -10
cargo test -q 2>&1 | tail -10
```

Expected: compiles clean, all tests pass.

- [ ] **Step 7: Commit**

```bash
git add src/cli/mod.rs
git commit -m "feat(setup): animate daemon/cert steps with SetupPrinter spinners"
```

---

## Task 5: Wire print_banner into Run arm + extract do_run helper

**Files:**
- Modify: `src/cli/mod.rs`

Extract the Run arm body into a private `do_run()` helper. This sets up the `Start` variant to share the same logic in Task 6. Also replaces the plain `eprintln!` banner with `banner::print_banner()`.

- [ ] **Step 1: Extract do_run() helper**

Add this private async function just before `ipc_connect()` in `src/cli/mod.rs`:

```rust
/// Core dev-server run logic shared by both `Run` and `Start`.
async fn do_run(
    cwd: std::path::PathBuf,
    config: crate::config::Config,
    args: Vec<String>,
    hostname_override: Option<String>,
    port_override: Option<u16>,
) -> Result<()> {
    let mut setup = banner::SetupPrinter::new();
    ensure_daemon_running(&config, &mut setup).await?;
    ensure_cert_trusted(&mut setup).await?;
    setup.done();

    let hostname =
        crate::detect::resolve_hostname(&cwd, hostname_override.as_deref(), &config.proxy.tld);

    // Check for an existing live route for this hostname (replace-by-default)
    let reuse_port: Option<u16> = {
        let mut stream = ipc_connect().await?;
        write_frame(&mut stream, &Command::Ls).await?;
        let resp: crate::proto::Response = read_frame(&mut stream).await?;
        if let Some(serde_json::Value::Array(routes)) = resp.data {
            routes
                .iter()
                .find(|r| r["hostname"].as_str() == Some(&hostname))
                .and_then(|r| r["port"].as_u64())
                .and_then(|p| u16::try_from(p).ok())
        } else {
            None
        }
    };

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

    let my_pid = std::process::id();
    let mut child = crate::process::spawn_child(&cwd, &args, port, &hostname).await?;

    // Register the route in the daemon's live in-memory store via IPC
    let child_pid = child.id().unwrap_or(my_pid);
    if let Ok(mut stream) = ipc_connect().await {
        let _ = write_frame(
            &mut stream,
            &Command::RegisterRoute {
                hostname: hostname.clone(),
                port,
                pid: child_pid,
                cwd: cwd.to_string_lossy().to_string(),
            },
        )
        .await;
        let _: crate::proto::Response = read_frame(&mut stream)
            .await
            .unwrap_or(crate::proto::Response::ok_empty());
    }

    banner::print_banner(&hostname, port, child_pid, reuse_port.is_some());

    child.wait().await?;

    Ok(())
}
```

- [ ] **Step 2: Replace the Run arm body to call do_run()**

Replace the entire `CliCommand::Run { hostname, port, args }` match arm with:

```rust
        CliCommand::Run {
            hostname,
            port,
            args,
        } => {
            let cwd = std::env::current_dir()?;
            let config = crate::config::Config::load(&cwd)?;
            do_run(cwd, config, args, hostname, port).await?;
        }
```

- [ ] **Step 3: Verify compilation and tests**

```bash
cargo build 2>&1 | grep "error" | head -10
cargo test -q 2>&1 | tail -10
```

Expected: compiles, all tests pass.

- [ ] **Step 4: Commit**

```bash
git add src/cli/mod.rs
git commit -m "refactor(cli): extract do_run helper, replace eprintln banner with print_banner"
```

---

## Task 6: Add portal start Subcommand

**Files:**
- Modify: `src/cli/mod.rs`

- [ ] **Step 1: Add Start variant to CliCommand**

In the `CliCommand` enum, after the `Daemon` variant:

```rust
    /// Auto-detect and start the best dev script from package.json
    Start,
```

- [ ] **Step 2: Add Start arm to the match in run()**

Add this arm in the `match cli.command` block, after `CliCommand::Daemon`:

```rust
        CliCommand::Start => {
            let cwd = std::env::current_dir()?;
            let config = crate::config::Config::load(&cwd)?;

            let pkg_path = cwd.join("package.json");
            if !pkg_path.exists() {
                eprintln!(
                    "error: no package.json found in {}. Use 'portal run <command>' to run an arbitrary command.",
                    cwd.display()
                );
                std::process::exit(1);
            }

            let contents = std::fs::read_to_string(&pkg_path)?;
            let json: serde_json::Value = serde_json::from_str(&contents)
                .map_err(|e| crate::error::Error::Config(e.to_string()))?;

            let script = match crate::detect::pick_dev_script(&json) {
                Some(s) => s,
                None => {
                    eprintln!("error: no scripts found in package.json. Use 'portal run <command>' to run an arbitrary command.");
                    std::process::exit(1);
                }
            };

            let pm = crate::detect::detect_package_manager(&cwd);
            let args = vec![pm.to_string(), "run".to_string(), script];

            do_run(cwd, config, args, None, None).await?;
        }
```

- [ ] **Step 3: Verify compilation**

```bash
cargo build 2>&1 | grep "error" | head -10
```

Expected: no errors.

- [ ] **Step 4: Verify portal start help text shows the new subcommand**

```bash
cargo run --bin portal -- --help 2>&1
```

Expected: `start` appears in the subcommand list with description "Auto-detect and start the best dev script from package.json".

- [ ] **Step 5: Commit**

```bash
git add src/cli/mod.rs
git commit -m "feat(cli): add 'portal start' subcommand with package manager auto-detection"
```

---

## Task 7: Smart portal run \<script\> Detection

**Files:**
- Modify: `src/cli/mod.rs`

If the first arg to `portal run` is not a known runner AND `package.json` has a matching script, prepend `<pm> run` automatically. Otherwise pass through unchanged.

- [ ] **Step 1: Write a test for the detection logic**

Add a test in `src/detect.rs` (in the `#[cfg(test)]` block) to verify the is_known_runner / detect_package_manager combination used for smart detection:

```rust
    #[test]
    fn smart_run_detection_scenario() {
        // Simulates: portal run dev → pnpm run dev
        let temp = TempDir::new().unwrap();
        fs::write(temp.path().join("pnpm-lock.yaml"), "").unwrap();
        fs::write(
            temp.path().join("package.json"),
            serde_json::json!({ "scripts": { "dev": "vite" } }).to_string(),
        )
        .unwrap();

        // First arg is not a known runner
        assert!(!is_known_runner("dev"));
        // Package manager is pnpm
        assert_eq!(detect_package_manager(temp.path()), "pnpm");
        // Script exists
        let contents = std::fs::read_to_string(temp.path().join("package.json")).unwrap();
        let json: serde_json::Value = serde_json::from_str(&contents).unwrap();
        assert!(json["scripts"]["dev"].is_string());
    }
```

- [ ] **Step 2: Run test**

```bash
cargo test -q --lib detect::tests::smart_run_detection_scenario 2>&1
```

Expected: PASS.

- [ ] **Step 3: Add smart detection in the Run arm of src/cli/mod.rs**

Replace the `CliCommand::Run` arm:

```rust
        CliCommand::Run {
            hostname,
            port,
            args,
        } => {
            let cwd = std::env::current_dir()?;
            let config = crate::config::Config::load(&cwd)?;

            // Smart detection: if args[0] is not a known runner, check if it's
            // a package.json script name and prepend `<pm> run` if so.
            let resolved_args = if let Some(first) = args.first() {
                if !crate::detect::is_known_runner(first) {
                    let pkg_path = cwd.join("package.json");
                    let script_exists = pkg_path.exists() && {
                        std::fs::read_to_string(&pkg_path)
                            .ok()
                            .and_then(|s| serde_json::from_str::<serde_json::Value>(&s).ok())
                            .and_then(|j| {
                                j.get("scripts")
                                    .and_then(|s| s.as_object())
                                    .map(|m| m.contains_key(first.as_str()))
                            })
                            .unwrap_or(false)
                    };
                    if script_exists {
                        let pm = crate::detect::detect_package_manager(&cwd);
                        let mut new_args = vec![pm.to_string(), "run".to_string()];
                        new_args.extend(args);
                        new_args
                    } else {
                        args
                    }
                } else {
                    args
                }
            } else {
                args
            };

            do_run(cwd, config, resolved_args, hostname, port).await?;
        }
```

- [ ] **Step 4: Verify compilation and tests**

```bash
cargo build 2>&1 | grep "error" | head -10
cargo test -q 2>&1 | tail -10
```

Expected: compiles, all tests pass.

- [ ] **Step 5: Commit**

```bash
git add src/cli/mod.rs src/detect.rs
git commit -m "feat(cli): smart 'portal run <script>' detects package.json scripts"
```

---

## Task 8: Output Polish — console::style() Colors

**Files:**
- Modify: `src/cli/output.rs`

- [ ] **Step 1: Replace print_ls and print_status with colored versions**

Replace the entire contents of `src/cli/output.rs` with:

```rust
use crate::proto::Response;
use console::style;

/// Print a generic response. If not ok, print error to stderr and exit(1).
pub fn print_response(resp: &Response) {
    if !resp.ok {
        let msg = resp.error.as_deref().unwrap_or("unknown error");
        eprintln!("error: {msg}");
        std::process::exit(1);
    }
}

/// Print the list of routes as a colored table.
pub fn print_ls(resp: &Response) {
    if !resp.ok {
        let msg = resp.error.as_deref().unwrap_or("unknown error");
        eprintln!("error: {msg}");
        std::process::exit(1);
    }

    let routes = match &resp.data {
        Some(serde_json::Value::Array(arr)) => arr.clone(),
        _ => {
            println!("No routes.");
            return;
        }
    };

    if routes.is_empty() {
        println!("{}", style("No active routes.").dim());
        return;
    }

    println!(
        "{:<30} {:>6}  {}",
        style("HOSTNAME").dim(),
        style("PORT").dim(),
        style("URL").dim()
    );
    println!("{}", style("─".repeat(60)).dim());
    for route in &routes {
        let hostname = route["hostname"].as_str().unwrap_or("-");
        let port = route["port"].as_u64().unwrap_or(0);
        let url = format!("https://{hostname}");
        println!(
            "{:<30} {}  {}",
            style(hostname).dim(),
            style(format!("{port:>6}")).red(),
            style(url).bold().white()
        );
    }
}

/// Print daemon status information with colors.
pub fn print_status(resp: &Response) {
    if !resp.ok {
        let msg = resp.error.as_deref().unwrap_or("unknown error");
        eprintln!("error: {msg}");
        std::process::exit(1);
    }

    if let Some(data) = &resp.data {
        let version = data["version"].as_str().unwrap_or("?");
        let pid = data["pid"].as_u64().unwrap_or(0);
        let uptime = data["uptime_secs"].as_u64().unwrap_or(0);
        let routes = data["routes_count"].as_u64().unwrap_or(0);

        println!(
            "  {}  {}",
            style(" portal ").bold().white().on_blue(),
            style(format!("v{version}")).dim()
        );
        println!("  {}  {}", style("pid:    ").dim(), style(pid.to_string()).dim());
        println!("  {}  {}s", style("uptime: ").dim(), style(uptime.to_string()).dim());
        println!("  {}  {}", style("routes: ").dim(), style(routes.to_string()).green());
    }
}
```

- [ ] **Step 2: Verify compilation and tests**

```bash
cargo build 2>&1 | grep "error" | head -10
cargo test -q 2>&1 | tail -10
```

Expected: compiles, all tests pass.

- [ ] **Step 3: Commit**

```bash
git add src/cli/output.rs
git commit -m "feat(output): add console::style() colors to portal ls and portal status"
```

---

## Verification

After all tasks complete, run the full test suite:

```bash
cargo test 2>&1 | tail -15
```

Expected: all tests pass with no failures.

Check the binary compiles in release mode:

```bash
cargo build --release 2>&1 | tail -3
```

Expected: `Finished release [optimized]`.

Smoke-test the new subcommands from a directory with a package.json:

```bash
portal --help            # start and run subcommands visible
portal run --help        # shows args
```
