# Group C: Git Worktree Branch-Prefix Routing — portal v0.4.0

**Date:** 2026-05-06
**Branch:** `feature/git-worktree-branch-prefix`
**Base branch:** `main`

---

## Goal

When `portal run` is invoked inside a **git linked worktree**, automatically prefix the hostname with the sanitized branch name. This gives each feature branch its own stable URL without manual `--hostname` overrides.

Example: branch `fix-auth-bug` + project `myapp` → `fix-auth-bug.myapp.localhost`

---

## Architecture

**New file `src/git.rs`** — two pure functions with no external dependencies (reads `.git` file/dir only):

- `is_linked_worktree(cwd: &Path) -> bool`
- `current_branch(cwd: &Path) -> Option<String>`

**Modified `src/cli/mod.rs`** — hostname resolution in `do_run` gains branch-prefix logic.

**New flag `--no-branch-prefix`** on `CliCommand::Run` — opt-out.

---

## `src/git.rs`

### `is_linked_worktree(cwd: &Path) -> bool`

A git linked worktree has `.git` as a **file** (not a directory) in the project root. The main worktree has `.git` as a directory.

```
Search upward from cwd for .git:
  - If .git is a FILE → linked worktree → return true
  - If .git is a DIRECTORY → main worktree → return false
  - If not found → not a git repo → return false
```

Stop searching at filesystem root.

### `current_branch(cwd: &Path) -> Option<String>`

1. Find `.git` file (linked worktree path, already confirmed by `is_linked_worktree`)
2. Read the file — it contains `gitdir: /path/to/main/.git/worktrees/<name>`
3. Read `HEAD` from that resolved git dir path
4. Parse: `ref: refs/heads/<branch-name>` → return `<branch-name>`
5. If `HEAD` is a detached commit hash → return `None` (no prefix in detached HEAD state)

### Branch name sanitization

Convert branch name to a valid subdomain component:
- Lowercase
- Replace any character outside `[a-z0-9]` with `-`
- Collapse consecutive `-` into one
- Strip leading and trailing `-`
- Truncate to 40 characters (subdomain component limit)

Examples:
- `fix/auth-bug` → `fix-auth-bug`
- `feature/JIRA-123-add-login` → `feature-jira-123-add-login`
- `main` → `main` (but this won't be prefixed since main worktree is excluded)

---

## Hostname Resolution in `do_run`

Current flow (simplified):
```
hostname = hostname_override OR driver.project_name() OR dir_name
```

New flow:
```
hostname = hostname_override OR driver.project_name() OR dir_name

if hostname_override is None        // user didn't explicitly set --hostname
   AND !no_branch_prefix            // user didn't opt out
   AND is_linked_worktree(cwd)      // we're in a linked worktree
   AND current_branch(cwd) is Some(branch)
{
    hostname = format!("{branch}.{hostname}")
}
```

The prefix only applies when:
- No explicit `--hostname` was passed (respects user intent)
- Not opted out via `--no-branch-prefix`
- Actually in a linked worktree (main worktrees never get prefixed)

---

## CLI Changes (`src/cli/mod.rs`)

Add to `CliCommand::Run`:

```rust
/// Disable automatic branch-name prefix in git worktrees
#[arg(long)]
no_branch_prefix: bool,
```

Pass `no_branch_prefix` to `do_run`. Add to `do_run` signature. `CliCommand::Start` passes `false`.

---

## `src/lib.rs`

Add `pub mod git;`.

---

## Error Handling

- Not a git repo → `is_linked_worktree` returns `false` → no prefix, silent.
- Detached HEAD → `current_branch` returns `None` → no prefix, silent.
- `.git` file malformed or unreadable → return `false` / `None`, silent.
- Branch name sanitizes to empty string (e.g. branch named `---`) → no prefix, silent.
- Worktree resolution path doesn't exist → return `None`, silent.

All failures are non-fatal. The prefix is a convenience, never a requirement.

---

## Testing (`src/git.rs`)

- `main_worktree_is_not_linked` — `.git` is a directory → `is_linked_worktree` returns `false`
- `linked_worktree_detected` — `.git` is a file with `gitdir:` content → returns `true`
- `no_git_returns_false` — no `.git` → returns `false`
- `current_branch_from_linked_worktree` — full fixture with `.git` file + HEAD file → returns branch name
- `detached_head_returns_none` — HEAD contains a SHA → returns `None`
- `sanitize_branch_slash` — `fix/foo` → `fix-foo`
- `sanitize_branch_uppercase` — `Feature/FOO` → `feature-foo`
- `sanitize_branch_consecutive_dashes` — `fix--bar` → `fix-bar`
- `sanitize_branch_truncation` — 50-char name → truncated to 40

**`src/cli/mod.rs`:**
- `run_command_has_no_branch_prefix_arg` — clap test

---

## Affected Files

| File | Change |
|------|--------|
| `src/git.rs` | New file — `is_linked_worktree`, `current_branch`, sanitize |
| `src/lib.rs` | Add `pub mod git;` |
| `src/cli/mod.rs` | Add `--no-branch-prefix`, thread through `do_run`, apply prefix logic |
