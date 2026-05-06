# Git Worktree Branch-Prefix Routing Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** When `portal run` is invoked inside a git linked worktree, automatically prefix the resolved hostname with the sanitized current branch name, so each feature branch gets its own stable `.localhost` URL.

**Architecture:** Add `src/git.rs` with two pure functions (`is_linked_worktree`, `current_branch`) that inspect the filesystem only (no git subprocess). Wire a new `--no-branch-prefix` flag into `CliCommand::Run` and apply prefix logic in `do_run` after `resolve_hostname`.

**Tech Stack:** Rust stable, `tempfile = "3"` (already in `[dev-dependencies]`), no new runtime dependencies.

---

### Task 1: Create `src/git.rs` with `is_linked_worktree`, `current_branch`, `sanitize_branch`

**Files:**
- Create: `src/git.rs`
- Modify: `src/lib.rs` (add `pub mod git;`)

- [ ] **Step 1: Add `pub mod git;` to `src/lib.rs`**

Open `src/lib.rs`. It currently ends at line 20. Add the new module declaration in alphabetical order between `error` and `hosts`:

```rust
// src/lib.rs — add this line between `error` and `hosts`:
pub mod git;
```

The file should look like:

```rust
pub mod certs;
pub mod cli;
pub mod config;
pub mod daemon;
pub mod detect;
pub mod error;
pub mod git;
pub mod hosts;
pub mod inspector;
pub mod lan;
pub mod pages;
pub mod ports;
pub mod process;
pub mod proto;
pub mod proxy;
pub mod route_manager;
pub mod routes;
pub mod switcher;
pub mod tailscale;
pub mod tcp;
pub mod workspace;
```

- [ ] **Step 2: Create `src/git.rs` with the 9 failing tests only (no implementation yet)**

Create `src/git.rs` with just stub function signatures that will make the tests compile but fail:

```rust
/// Returns true if cwd is inside a git linked worktree (not the main worktree).
/// A linked worktree has `.git` as a FILE, not a directory.
pub fn is_linked_worktree(_cwd: &std::path::Path) -> bool {
    false
}

/// Returns the current branch name for a linked worktree, or None for detached HEAD.
pub fn current_branch(_cwd: &std::path::Path) -> Option<String> {
    None
}

pub fn sanitize_branch(_name: &str) -> String {
    String::new()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    // ── is_linked_worktree ──────────────────────────────────────────────────

    #[test]
    fn main_worktree_is_not_linked() {
        // .git is a DIRECTORY → main worktree → should return false
        let tmp = TempDir::new().unwrap();
        let root = tmp.path();
        fs::create_dir(root.join(".git")).unwrap();
        assert!(!is_linked_worktree(root));
    }

    #[test]
    fn linked_worktree_detected() {
        // .git is a FILE → linked worktree → should return true
        let tmp = TempDir::new().unwrap();
        let root = tmp.path();
        fs::write(
            root.join(".git"),
            "gitdir: /some/main/.git/worktrees/feature\n",
        )
        .unwrap();
        assert!(is_linked_worktree(root));
    }

    #[test]
    fn no_git_returns_false() {
        // No .git at all → not a git repo → should return false
        let tmp = TempDir::new().unwrap();
        assert!(!is_linked_worktree(tmp.path()));
    }

    // ── current_branch ─────────────────────────────────────────────────────

    #[test]
    fn current_branch_from_linked_worktree() {
        // Full fixture:
        //   <root>/.git  (file) → "gitdir: <fake_git_dir>"
        //   <fake_git_dir>/HEAD → "ref: refs/heads/fix/auth-bug"
        let tmp = TempDir::new().unwrap();
        let root = tmp.path();

        // Fake git dir that the .git file points to
        let fake_git_dir = tmp.path().join("fake_git");
        fs::create_dir_all(&fake_git_dir).unwrap();
        fs::write(
            fake_git_dir.join("HEAD"),
            "ref: refs/heads/fix/auth-bug\n",
        )
        .unwrap();

        // .git file in root pointing at fake_git_dir
        fs::write(
            root.join(".git"),
            format!("gitdir: {}\n", fake_git_dir.display()),
        )
        .unwrap();

        let branch = current_branch(root);
        assert_eq!(branch.as_deref(), Some("fix-auth-bug"));
    }

    #[test]
    fn detached_head_returns_none() {
        // HEAD contains a SHA → detached HEAD → should return None
        let tmp = TempDir::new().unwrap();
        let root = tmp.path();

        let fake_git_dir = tmp.path().join("fake_git");
        fs::create_dir_all(&fake_git_dir).unwrap();
        fs::write(
            fake_git_dir.join("HEAD"),
            "a1b2c3d4e5f6a1b2c3d4e5f6a1b2c3d4e5f6a1b2\n",
        )
        .unwrap();

        fs::write(
            root.join(".git"),
            format!("gitdir: {}\n", fake_git_dir.display()),
        )
        .unwrap();

        assert!(current_branch(root).is_none());
    }

    // ── sanitize_branch ─────────────────────────────────────────────────────

    #[test]
    fn sanitize_branch_slash() {
        assert_eq!(sanitize_branch("fix/foo"), "fix-foo");
    }

    #[test]
    fn sanitize_branch_uppercase() {
        assert_eq!(sanitize_branch("Feature/FOO"), "feature-foo");
    }

    #[test]
    fn sanitize_branch_consecutive_dashes() {
        assert_eq!(sanitize_branch("fix--bar"), "fix-bar");
    }

    #[test]
    fn sanitize_branch_truncation() {
        // 50-character input → truncated to 40
        let long_name = "a".repeat(50);
        let result = sanitize_branch(&long_name);
        assert_eq!(result.len(), 40);
    }
}
```

- [ ] **Step 3: Run tests to verify they fail**

```bash
cd /Users/__deesh_reddy__/projects/personal_git/rust_builds/portless && cargo test --lib git 2>&1 | tail -30
```

Expected: 9 test failures. The sanitize/branch/worktree functions are all stubs returning `false`/`None`/empty string.

- [ ] **Step 4: Implement `sanitize_branch`**

Replace the stub in `src/git.rs`:

```rust
pub fn sanitize_branch(name: &str) -> String {
    let lower = name.to_lowercase();
    // Replace non-alphanumeric (except hyphen) with hyphen
    let replaced: String = lower
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() || c == '-' { c } else { '-' })
        .collect();
    // Collapse consecutive hyphens
    let mut result = String::new();
    let mut last_hyphen = false;
    for c in replaced.chars() {
        if c == '-' {
            if !last_hyphen {
                result.push(c);
            }
            last_hyphen = true;
        } else {
            result.push(c);
            last_hyphen = false;
        }
    }
    // Strip leading/trailing hyphens and truncate to 40 chars
    let trimmed = result.trim_matches('-');
    trimmed.chars().take(40).collect()
}
```

- [ ] **Step 5: Run sanitize tests to verify they pass**

```bash
cd /Users/__deesh_reddy__/projects/personal_git/rust_builds/portless && cargo test --lib git::tests::sanitize 2>&1 | tail -15
```

Expected:
```
test git::tests::sanitize_branch_consecutive_dashes ... ok
test git::tests::sanitize_branch_slash ... ok
test git::tests::sanitize_branch_truncation ... ok
test git::tests::sanitize_branch_uppercase ... ok
```

- [ ] **Step 6: Implement `is_linked_worktree`**

Replace the stub:

```rust
pub fn is_linked_worktree(cwd: &std::path::Path) -> bool {
    let mut dir = cwd;
    loop {
        let git_path = dir.join(".git");
        if git_path.is_file() {
            return true;
        }
        if git_path.is_dir() {
            return false;
        }
        match dir.parent() {
            Some(p) => dir = p,
            None => return false,
        }
    }
}
```

- [ ] **Step 7: Run worktree detection tests**

```bash
cd /Users/__deesh_reddy__/projects/personal_git/rust_builds/portless && cargo test --lib "git::tests::main_worktree\|git::tests::linked_worktree\|git::tests::no_git" 2>&1 | tail -15
```

Expected:
```
test git::tests::linked_worktree_detected ... ok
test git::tests::main_worktree_is_not_linked ... ok
test git::tests::no_git_returns_false ... ok
```

- [ ] **Step 8: Implement `current_branch`**

Replace the stub:

```rust
pub fn current_branch(cwd: &std::path::Path) -> Option<String> {
    // Walk up to find the .git FILE (linked worktree indicator)
    let mut dir = cwd;
    let git_file = loop {
        let git_path = dir.join(".git");
        if git_path.is_file() {
            break git_path;
        }
        match dir.parent() {
            Some(p) => dir = p,
            None => return None,
        }
    };
    // Read "gitdir: /path/to/.git/worktrees/<name>"
    let contents = std::fs::read_to_string(&git_file).ok()?;
    let git_dir = contents
        .strip_prefix("gitdir: ")?
        .trim()
        .to_string();
    let git_dir = std::path::Path::new(&git_dir);
    // Read HEAD from that resolved git dir
    let head = std::fs::read_to_string(git_dir.join("HEAD")).ok()?;
    // Parse "ref: refs/heads/<branch>" — detached HEAD is a raw SHA, no prefix match
    let branch = head
        .trim()
        .strip_prefix("ref: refs/heads/")?
        .to_string();
    Some(sanitize_branch(&branch))
}
```

- [ ] **Step 9: Run all 9 git tests**

```bash
cd /Users/__deesh_reddy__/projects/personal_git/rust_builds/portless && cargo test --lib git 2>&1 | tail -20
```

Expected:
```
test git::tests::current_branch_from_linked_worktree ... ok
test git::tests::detached_head_returns_none ... ok
test git::tests::linked_worktree_detected ... ok
test git::tests::main_worktree_is_not_linked ... ok
test git::tests::no_git_returns_false ... ok
test git::tests::sanitize_branch_consecutive_dashes ... ok
test git::tests::sanitize_branch_slash ... ok
test git::tests::sanitize_branch_truncation ... ok
test git::tests::sanitize_branch_uppercase ... ok

test result: ok. 9 passed; 0 failed
```

- [ ] **Step 10: Run full test suite to check for regressions**

```bash
cd /Users/__deesh_reddy__/projects/personal_git/rust_builds/portless && cargo test 2>&1 | tail -10
```

Expected: all tests pass, 0 failed.

- [ ] **Step 11: Commit**

```bash
cd /Users/__deesh_reddy__/projects/personal_git/rust_builds/portless
git add src/git.rs src/lib.rs
git commit -m "feat(git): add is_linked_worktree, current_branch, sanitize_branch"
```

---

### Task 2: Wire `--no-branch-prefix` flag and prefix logic into `do_run`

**Files:**
- Modify: `src/cli/mod.rs`

- [ ] **Step 1: Write the failing clap test**

Open `src/cli/mod.rs`. Find the `#[cfg(test)]` block at the bottom (or add one if absent). Add this test:

```rust
#[cfg(test)]
mod tests {
    #[test]
    fn run_command_has_no_branch_prefix_arg() {
        use clap::Parser;
        use crate::cli::Cli;
        // Verify the flag parses correctly and defaults to false
        let cli = Cli::try_parse_from(["portal", "run", "npm", "start"]).unwrap();
        if let crate::cli::CliCommand::Run { no_branch_prefix, .. } = cli.command {
            assert!(!no_branch_prefix, "no_branch_prefix should default to false");
        } else {
            panic!("expected Run command");
        }
        // Verify opt-in flag sets it to true
        let cli = Cli::try_parse_from(["portal", "run", "--no-branch-prefix", "npm", "start"]).unwrap();
        if let crate::cli::CliCommand::Run { no_branch_prefix, .. } = cli.command {
            assert!(no_branch_prefix, "no_branch_prefix should be true when flag is passed");
        } else {
            panic!("expected Run command");
        }
    }
}
```

- [ ] **Step 2: Run the test to verify it fails**

```bash
cd /Users/__deesh_reddy__/projects/personal_git/rust_builds/portless && cargo test --lib cli::tests::run_command_has_no_branch_prefix_arg 2>&1 | tail -15
```

Expected: compile error — `no_branch_prefix` field does not exist on `CliCommand::Run`.

- [ ] **Step 3: Add `no_branch_prefix` field to `CliCommand::Run`**

In `src/cli/mod.rs`, inside the `Run { ... }` variant (currently ends at line ~70 with `args`), add the new field before `args`:

```rust
    /// Disable automatic branch-name prefix in git worktrees
    #[arg(long)]
    no_branch_prefix: bool,
    #[arg(trailing_var_arg = true, required = true)]
    args: Vec<String>,
```

The full `Run` variant field list becomes (order matters for readability, put `no_branch_prefix` after `label`):

```rust
    Run {
        #[arg(long)]
        hostname: Option<String>,
        #[arg(long)]
        port: Option<u16>,
        #[arg(long, short = 'q', help = "Suppress startup banner and running output")]
        quiet: bool,
        /// Treat as a TCP service (skip HTTPS proxy; for databases, caches, etc.)
        #[arg(long)]
        tcp: bool,
        /// Kill any existing process registered under this hostname and replace it
        #[arg(long)]
        force: bool,
        /// Expose this app to the local network via mDNS .local hostname
        #[arg(long)]
        lan: bool,
        /// Override the auto-detected LAN IP (e.g. for VPN setups)
        #[arg(long, value_name = "ADDR")]
        ip: Option<String>,
        /// Use HTTP/2 cleartext (h2c) for upstream connections (for gRPC backends)
        #[arg(long)]
        h2c: bool,
        /// Share this app on your Tailscale tailnet
        #[arg(long)]
        tailscale: bool,
        /// Share this app publicly via Tailscale Funnel (implies --tailscale)
        #[arg(long)]
        funnel: bool,
        /// Register as a specific slot number (default: auto-assign next available)
        #[arg(long)]
        slot: Option<u32>,
        /// Label shown in the app-switcher UI (default: slot-N)
        #[arg(long)]
        label: Option<String>,
        /// Disable automatic branch-name prefix in git worktrees
        #[arg(long)]
        no_branch_prefix: bool,
        #[arg(trailing_var_arg = true, required = true)]
        args: Vec<String>,
    },
```

- [ ] **Step 4: Add `no_branch_prefix` to `do_run` signature**

Find `async fn do_run(` (line ~791). Add `no_branch_prefix: bool,` as the last parameter before the closing `)`:

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
    no_branch_prefix: bool,
) -> Result<()> {
```

- [ ] **Step 5: Update the `CliCommand::Run` destructuring and call site**

In the `CliCommand::Run { ... } =>` arm (around line 524), add `no_branch_prefix` to the destructure pattern and pass it to `do_run`:

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
            no_branch_prefix,
            args,
        } => {
```

And in the `do_run(...)` call for this arm, append `no_branch_prefix` as the final argument:

```rust
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
                no_branch_prefix,
            )
            .await?;
```

- [ ] **Step 6: Update `CliCommand::Start` call site to pass `false`**

In the `CliCommand::Start { quiet } =>` arm (around line 243), the existing `do_run(...)` call must also get the new parameter. Append `false` as the final argument:

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
                false,   // no_branch_prefix — Start never applies branch prefix
            )
            .await?;
```

- [ ] **Step 7: Apply branch-prefix logic in `do_run` after `resolve_hostname`**

Find this section in `do_run` (around line 822–824):

```rust
    let hostname =
        crate::detect::resolve_hostname(&cwd, hostname_override.as_deref(), &config.proxy.tld);
    let public_url = build_public_url(&config, &hostname);
```

Insert the prefix logic between `resolve_hostname` and `build_public_url`:

```rust
    let hostname =
        crate::detect::resolve_hostname(&cwd, hostname_override.as_deref(), &config.proxy.tld);

    // Auto-prefix with branch name when inside a git linked worktree
    let hostname = if !no_branch_prefix && hostname_override.is_none() {
        if let Some(branch) = crate::git::current_branch(&cwd)
            .filter(|_| crate::git::is_linked_worktree(&cwd))
        {
            format!("{branch}.{hostname}")
        } else {
            hostname
        }
    } else {
        hostname
    };

    let public_url = build_public_url(&config, &hostname);
```

- [ ] **Step 8: Run clap test to confirm it now passes**

```bash
cd /Users/__deesh_reddy__/projects/personal_git/rust_builds/portless && cargo test --lib cli::tests::run_command_has_no_branch_prefix_arg 2>&1 | tail -10
```

Expected:
```
test cli::tests::run_command_has_no_branch_prefix_arg ... ok

test result: ok. 1 passed; 0 failed
```

- [ ] **Step 9: Run full test suite**

```bash
cd /Users/__deesh_reddy__/projects/personal_git/rust_builds/portless && cargo test 2>&1 | tail -10
```

Expected: all tests pass, 0 failed.

- [ ] **Step 10: Commit**

```bash
cd /Users/__deesh_reddy__/projects/personal_git/rust_builds/portless
git add src/cli/mod.rs
git commit -m "feat(cli): add --no-branch-prefix flag and branch-prefix logic to do_run"
```

---

### Task 3: Branch, full build + test, push

**Files:** none new — just git operations.

- [ ] **Step 1: Create the feature branch**

```bash
cd /Users/__deesh_reddy__/projects/personal_git/rust_builds/portless
git checkout -b feature/git-worktree-branch-prefix
```

Expected: `Switched to a new branch 'feature/git-worktree-branch-prefix'`

- [ ] **Step 2: Full clean build**

```bash
cd /Users/__deesh_reddy__/projects/personal_git/rust_builds/portless && cargo build 2>&1 | tail -5
```

Expected: `Finished dev [unoptimized + debuginfo] target(s)` with 0 errors.

- [ ] **Step 3: Full test run**

```bash
cd /Users/__deesh_reddy__/projects/personal_git/rust_builds/portless && cargo test 2>&1 | tail -10
```

Expected: all tests pass (229+ tests), 0 failed.

- [ ] **Step 4: Push the branch**

```bash
cd /Users/__deesh_reddy__/projects/personal_git/rust_builds/portless
git push -u origin feature/git-worktree-branch-prefix
```

Expected: branch pushed, tracking remote set.
