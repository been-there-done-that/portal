# Shell Completions Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add `portal completion [shell] [--print] [--path <dir>]` that auto-detects the current shell and installs tab completions with a single confirmation prompt.

**Architecture:** Uses `clap_complete` to generate shell scripts from the clap `CommandFactory` trait at runtime. All logic lives in a new `src/cli/completion.rs` module. No daemon, no IPC — purely local filesystem writes. Wired into the existing `CliCommand` enum in `src/cli/mod.rs`.

**Tech Stack:** `clap_complete = "4"` (same major as existing `clap = "4"`), `dirs = "5"` (already in deps), `dialoguer` (already in deps), `console` (already in deps), `std::io::IsTerminal` (stable since Rust 1.70, already used in `Init`).

---

## File Map

| File | Change |
|---|---|
| `Cargo.toml` | Add `clap_complete = "4"` to `[dependencies]` |
| `src/cli/completion.rs` | **Create** — all completion logic: detection, path resolution, prompt, generation, install |
| `src/cli/mod.rs` | Add `pub mod completion;`, `Completion` variant to `CliCommand`, handler arm |

---

### Task 1: Add dependency and create `src/cli/completion.rs`

**Files:**
- Modify: `Cargo.toml`
- Create: `src/cli/completion.rs`

- [ ] **Step 1: Add `clap_complete` to `Cargo.toml`**

In the `[dependencies]` section, after the `clap` line, add:

```toml
clap_complete = "4"
```

The full `[dependencies]` block around that area becomes:
```toml
clap = { version = "4", features = ["derive", "env"] }
clap_complete = "4"
```

- [ ] **Step 2: Verify it compiles**

```bash
cd /path/to/worktree && cargo check 2>&1 | head -20
```

Expected: no errors (warnings OK).

- [ ] **Step 3: Write failing tests for `detect_shell` and `default_install_path`**

Create `src/cli/completion.rs` with just the tests (functions not yet implemented):

```rust
use std::path::{Path, PathBuf};
use clap_complete::Shell;

pub fn detect_shell() -> Option<Shell> {
    todo!()
}

fn is_omz() -> bool {
    todo!()
}

pub fn default_install_path(shell: Shell) -> PathBuf {
    todo!()
}

fn post_install_message(shell: Shell, path: &Path) {
    todo!()
}

pub fn run(
    shell: Option<Shell>,
    print: bool,
    path: Option<PathBuf>,
) -> crate::error::Result<()> {
    todo!()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detect_shell_bash() {
        // SAFETY: single-threaded test, no other threads reading SHELL
        unsafe { std::env::set_var("SHELL", "/bin/bash") };
        assert!(matches!(detect_shell(), Some(Shell::Bash)));
        unsafe { std::env::remove_var("SHELL") };
    }

    #[test]
    fn detect_shell_zsh() {
        unsafe { std::env::set_var("SHELL", "/usr/local/bin/zsh") };
        assert!(matches!(detect_shell(), Some(Shell::Zsh)));
        unsafe { std::env::remove_var("SHELL") };
    }

    #[test]
    fn detect_shell_fish() {
        unsafe { std::env::set_var("SHELL", "/opt/homebrew/bin/fish") };
        assert!(matches!(detect_shell(), Some(Shell::Fish)));
        unsafe { std::env::remove_var("SHELL") };
    }

    #[test]
    fn detect_shell_pwsh() {
        unsafe { std::env::set_var("SHELL", "/usr/local/bin/pwsh") };
        assert!(matches!(detect_shell(), Some(Shell::PowerShell)));
        unsafe { std::env::remove_var("SHELL") };
    }

    #[test]
    fn detect_shell_elvish() {
        unsafe { std::env::set_var("SHELL", "/usr/local/bin/elvish") };
        assert!(matches!(detect_shell(), Some(Shell::Elvish)));
        unsafe { std::env::remove_var("SHELL") };
    }

    #[test]
    fn detect_shell_unknown_returns_none() {
        unsafe { std::env::set_var("SHELL", "/usr/bin/tcsh") };
        assert!(detect_shell().is_none());
        unsafe { std::env::remove_var("SHELL") };
    }

    #[test]
    fn detect_shell_unset_returns_none() {
        unsafe { std::env::remove_var("SHELL") };
        assert!(detect_shell().is_none());
    }

    #[test]
    fn default_path_fish() {
        let path = default_install_path(Shell::Fish);
        assert!(path.to_string_lossy().ends_with(".config/fish/completions/portal.fish"));
    }

    #[test]
    fn default_path_bash() {
        let path = default_install_path(Shell::Bash);
        assert!(path.to_string_lossy().ends_with(".local/share/bash-completion/completions/portal"));
    }

    #[test]
    fn default_path_zsh_no_omz() {
        unsafe { std::env::remove_var("ZSH") };
        let path = default_install_path(Shell::Zsh);
        assert!(path.to_string_lossy().ends_with(".zfunc/_portal"));
    }

    #[test]
    fn default_path_zsh_with_omz() {
        // Create a temp dir to simulate $ZSH pointing to a real directory
        let tmp = tempfile::tempdir().unwrap();
        unsafe { std::env::set_var("ZSH", tmp.path()) };
        let path = default_install_path(Shell::Zsh);
        assert!(path.to_string_lossy().ends_with(".oh-my-zsh/completions/_portal"));
        unsafe { std::env::remove_var("ZSH") };
    }

    #[test]
    fn default_path_powershell() {
        let path = default_install_path(Shell::PowerShell);
        assert!(path.to_string_lossy().ends_with("PowerShell/Completions/portal.ps1"));
    }

    #[test]
    fn default_path_elvish() {
        let path = default_install_path(Shell::Elvish);
        assert!(path.to_string_lossy().ends_with(".config/elvish/lib/portal.elv"));
    }
}
```

- [ ] **Step 4: Run tests to verify they fail**

```bash
cargo test -p portal completion:: 2>&1 | tail -20
```

Expected: compile error or `not yet implemented` panics — confirms tests exist and detect real gaps.

- [ ] **Step 5: Implement `detect_shell`, `is_omz`, `default_install_path`, and `post_install_message`**

Replace the `todo!()` stubs in `src/cli/completion.rs` with real implementations (keep the `run` function as `todo!()` for now).
Do NOT add `use std::io::{IsTerminal, Write}` yet — those are only needed when `run()` is implemented in Task 2:

```rust
use std::path::{Path, PathBuf};
use clap_complete::Shell;

pub fn detect_shell() -> Option<Shell> {
    let shell_path = std::env::var("SHELL").ok()?;
    let binary = Path::new(&shell_path).file_name()?.to_str()?;
    match binary {
        "bash" => Some(Shell::Bash),
        "zsh" => Some(Shell::Zsh),
        "fish" => Some(Shell::Fish),
        "pwsh" | "powershell" => Some(Shell::PowerShell),
        "elvish" => Some(Shell::Elvish),
        _ => None,
    }
}

fn is_omz() -> bool {
    std::env::var("ZSH")
        .ok()
        .map(|p| Path::new(&p).is_dir())
        .unwrap_or(false)
}

pub fn default_install_path(shell: Shell) -> PathBuf {
    let home = dirs::home_dir().unwrap_or_else(|| PathBuf::from("~"));
    match shell {
        Shell::Bash => home.join(".local/share/bash-completion/completions/portal"),
        Shell::Zsh => {
            if is_omz() {
                home.join(".oh-my-zsh/completions/_portal")
            } else {
                home.join(".zfunc/_portal")
            }
        }
        Shell::Fish => home.join(".config/fish/completions/portal.fish"),
        Shell::PowerShell => home.join("Documents/PowerShell/Completions/portal.ps1"),
        Shell::Elvish => home.join(".config/elvish/lib/portal.elv"),
        _ => home.join(".local/share/completions/portal"),
    }
}

fn post_install_message(shell: Shell, path: &Path) {
    println!("{} Installed to {}", console::style("✓").green(), path.display());
    match shell {
        Shell::Zsh if !is_omz() => {
            println!("\nAdd to ~/.zshrc (if not already present):");
            println!("  fpath=(~/.zfunc $fpath)");
            println!("  autoload -Uz compinit && compinit");
            println!("Then reload: source ~/.zshrc");
        }
        Shell::PowerShell => {
            println!("\nAdd to your $PROFILE:");
            println!("  . {}", path.display());
        }
        _ => {
            println!("Reload with: source {}", path.display());
        }
    }
}

pub fn run(
    shell: Option<Shell>,
    print: bool,
    path: Option<PathBuf>,
) -> crate::error::Result<()> {
    todo!()
}

#[cfg(test)]
mod tests {
    // ... (unchanged from Step 3)
}
```

- [ ] **Step 6: Run tests to verify they pass**

```bash
cargo test -p portal completion:: 2>&1 | tail -20
```

Expected: all tests pass except any touching `run()` (which is still `todo!()`).

- [ ] **Step 7: Commit**

```bash
git add Cargo.toml Cargo.lock src/cli/completion.rs
git commit -m "feat(completion): add dependency, shell detection, and path resolution"
```

---

### Task 2: Implement `run()` and wire the CLI subcommand

**Files:**
- Modify: `src/cli/completion.rs` — implement `run()`
- Modify: `src/cli/mod.rs` — add subcommand + handler

- [ ] **Step 1: Write a failing test for `run()` in print mode**

Add this test to the `#[cfg(test)]` block in `src/cli/completion.rs`:

```rust
    #[test]
    fn run_print_bash_produces_output() {
        // Capture stdout by generating directly — tests the generate path
        let mut cmd = <super::super::Cli as clap::CommandFactory>::command();
        let mut buf = Vec::new();
        clap_complete::generate(Shell::Bash, &mut cmd, "portal", &mut buf);
        let output = String::from_utf8(buf).unwrap();
        assert!(!output.is_empty());
        assert!(output.contains("portal"));
    }

    #[test]
    fn run_print_fish_produces_complete_command() {
        let mut cmd = <super::super::Cli as clap::CommandFactory>::command();
        let mut buf = Vec::new();
        clap_complete::generate(Shell::Fish, &mut cmd, "portal", &mut buf);
        let output = String::from_utf8(buf).unwrap();
        assert!(output.contains("complete") && output.contains("portal"));
    }
```

Note: these tests call `clap_complete::generate` directly (not through `run()`) to validate the generation logic without needing to control stdout. The `run()` integration is verified by wiring in Step 3.

- [ ] **Step 2: Run tests to verify they pass as-is (generation logic is independent)**

```bash
cargo test -p portal completion::tests::run_print 2>&1
```

Expected: PASS — `clap_complete::generate` works as soon as the dependency is present.

- [ ] **Step 3: Implement `run()` in `src/cli/completion.rs`**

Replace the `todo!()` in `run()` with the full implementation:

```rust
pub fn run(
    shell: Option<Shell>,
    print: bool,
    path: Option<PathBuf>,
) -> crate::error::Result<()> {
    use clap::CommandFactory;
    use clap_complete::generate;

    // Resolve shell: explicit arg > $SHELL detection > error
    let shell = match shell {
        Some(s) => s,
        None => match detect_shell() {
            Some(s) => s,
            None => {
                eprintln!(
                    "Could not detect shell. Run: portal completion <bash|zsh|fish|powershell|elvish>"
                );
                std::process::exit(1);
            }
        },
    };

    // --print: dump to stdout and exit
    if print {
        let mut cmd = super::Cli::command();
        generate(shell, &mut cmd, "portal", &mut std::io::stdout());
        return Ok(());
    }

    // Install mode: resolve target path
    let install_path = match path {
        Some(dir) => {
            let default = default_install_path(shell);
            let filename = default.file_name().expect("default path always has filename");
            dir.join(filename)
        }
        None => default_install_path(shell),
    };

    let shell_name = shell.to_string();
    println!("Detected shell: {shell_name}");
    print!("Install completion to {}? [Y/n] ", install_path.display());
    std::io::stdout().flush()?;

    // Confirm: auto-yes when stdin is not a TTY (piped/scripted usage)
    let confirmed = if std::io::stdin().is_terminal() {
        let mut input = String::new();
        std::io::stdin().read_line(&mut input)?;
        matches!(input.trim().to_ascii_lowercase().as_str(), "" | "y" | "yes")
    } else {
        true
    };

    if !confirmed {
        println!(
            "\nRun this to install manually:\n  portal completion {shell_name} --print > {}",
            install_path.display()
        );
        return Ok(());
    }

    // Create parent directories
    if let Some(parent) = install_path.parent() {
        std::fs::create_dir_all(parent)?;
    }

    // Generate and write
    let mut cmd = super::Cli::command();
    let mut buf = Vec::new();
    generate(shell, &mut cmd, "portal", &mut buf);
    std::fs::write(&install_path, &buf)?;

    post_install_message(shell, &install_path);
    Ok(())
}
```

- [ ] **Step 4: Add `pub mod completion;` to `src/cli/mod.rs`**

After the existing `pub mod banner;` and `pub mod output;` lines at the top of `src/cli/mod.rs`, add:

```rust
pub mod banner;
pub mod completion;
pub mod output;
```

- [ ] **Step 5: Add `Completion` variant to `CliCommand` in `src/cli/mod.rs`**

In the `CliCommand` enum, after the `Init` variant, add:

```rust
    /// Generate shell completions for portal
    Completion {
        /// Shell to generate completions for (auto-detected if omitted)
        shell: Option<clap_complete::Shell>,
        /// Print to stdout instead of installing
        #[arg(long, short = 'p')]
        print: bool,
        /// Override the default install directory
        #[arg(long)]
        path: Option<std::path::PathBuf>,
    },
```

- [ ] **Step 6: Add handler arm in `run()` in `src/cli/mod.rs`**

In the `match cli.command { ... }` block, after the `CliCommand::Init { ... }` arm (before the closing `}`), add:

```rust
        CliCommand::Completion { shell, print, path } => {
            completion::run(shell, print, path)?;
        }
```

- [ ] **Step 7: Verify it compiles**

```bash
cargo check 2>&1 | head -30
```

Expected: no errors.

- [ ] **Step 8: Run all tests**

```bash
cargo test 2>&1 | tail -20
```

Expected: all existing tests pass plus the new completion tests.

- [ ] **Step 9: Smoke test the binary**

```bash
cargo run --bin portal -- completion bash --print 2>&1 | head -5
```

Expected: bash completion script starting with something like `_portal()` or `complete`.

```bash
cargo run --bin portal -- completion fish --print 2>&1 | head -5
```

Expected: fish completion lines starting with `complete -c portal`.

```bash
cargo run --bin portal -- completion --help
```

Expected: help text showing `[SHELL]`, `--print`, `--path` options.

- [ ] **Step 10: Commit**

```bash
git add src/cli/completion.rs src/cli/mod.rs
git commit -m "feat(completion): add portal completion subcommand with auto-detect install"
```

---

### Task 3: Install smoke test and final verification

**Files:**
- No new files — manual verification only

- [ ] **Step 1: Test install to a temp directory (non-interactive)**

```bash
TMPDIR=$(mktemp -d)
echo "" | cargo run --bin portal -- completion fish --path "$TMPDIR"
ls "$TMPDIR"
```

Expected: `portal.fish` file created in `$TMPDIR`.

```bash
cat "$TMPDIR/portal.fish" | head -3
```

Expected: fish completion script content (lines containing `complete -c portal`).

- [ ] **Step 2: Test `--path` override for zsh**

```bash
TMPDIR=$(mktemp -d)
echo "" | cargo run --bin portal -- completion zsh --path "$TMPDIR"
ls "$TMPDIR"
```

Expected: `_portal` file in `$TMPDIR`.

- [ ] **Step 3: Test undetectable shell error**

```bash
env -i HOME="$HOME" cargo run --bin portal -- completion 2>&1
```

Expected: `Could not detect shell. Run: portal completion <bash|zsh|fish|powershell|elvish>`

- [ ] **Step 4: Test decline prints manual instructions**

```bash
echo "n" | cargo run --bin portal -- completion fish 2>&1
```

Expected output includes `portal completion fish --print >` with the fish path.

- [ ] **Step 5: Run full test suite one final time**

```bash
cargo test 2>&1 | tail -5
```

Expected: all tests pass, 0 failures.

- [ ] **Step 6: Final commit**

```bash
git add -p  # stage any leftover changes
git commit -m "test(completion): smoke tests for install and error paths"
```
