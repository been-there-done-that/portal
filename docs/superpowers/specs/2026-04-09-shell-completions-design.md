# Shell Completions Design

## Overview

Add shell completion support to the `portless` CLI so users can press Tab to complete subcommands and flags. Default behavior installs completions for the detected shell with a single confirmation prompt. A `--print` flag outputs the script to stdout for users who prefer to manage it themselves.

## Command Interface

```
portless completion [shell] [--print] [--path <dir>]
```

- `shell` — optional positional argument: `bash | zsh | fish | powershell | elvish`. Auto-detected from `$SHELL` env var if omitted.
- `--print` — dump completion script to stdout; no prompt, no file written. Pipeable.
- `--path <dir>` — override the default install directory.

## User Experience

### Default flow (install mode)

```
$ portless completion
Detected shell: fish
Install completion to ~/.config/fish/completions/portless.fish? [Y/n]: y
✓ Installed. Reload with: source ~/.config/fish/completions/portless.fish
```

If the user declines (`n`):
```
Run this to install manually:
  portless completion fish > ~/.config/fish/completions/portless.fish
```

If `$SHELL` is unrecognizable:
```
Could not detect shell. Run: portless completion <bash|zsh|fish|powershell|elvish>
```

### Print mode

```
$ portless completion fish --print
# fish completion script output...

$ portless completion zsh --print | sudo tee /usr/local/share/zsh/site-functions/_portless
```

Auto-confirms (skips prompt) when stdin is not a TTY (piped usage).

## Install Paths

| Shell | Default path | Extra config required |
|---|---|---|
| fish | `~/.config/fish/completions/portless.fish` | None — fish auto-loads from this directory |
| bash | `~/.local/share/bash-completion/completions/portless` | None — bash-completion v2+ auto-loads from here |
| zsh (Oh My Zsh) | `~/.oh-my-zsh/completions/_portless` | None — OMZ includes this dir in fpath automatically |
| zsh (plain) | `~/.zfunc/_portless` | Add fpath snippet to `~/.zshrc` (printed post-install) |
| PowerShell | `~/Documents/PowerShell/Completions/portless.ps1` | Source line added to `$PROFILE` instructions (printed post-install) |
| Elvish | `~/.config/elvish/lib/portless.elv` | None — `use portless` in rc.elv if desired |

### Post-install messages

**zsh (plain, no OMZ):** After writing `~/.zfunc/_portless`:
```
Add to ~/.zshrc (if not already present):
  fpath=(~/.zfunc $fpath)
  autoload -Uz compinit && compinit
Then reload: source ~/.zshrc
```

**PowerShell:** After writing the `.ps1` file:
```
Add to your $PROFILE:
  . ~/Documents/PowerShell/Completions/portless.ps1
```

**Oh My Zsh detection:** presence of `$ZSH` env var pointing to an existing directory.

## Architecture

### No daemon required

Completions are generated at runtime directly from the `clap::Command` tree. No IPC, no daemon start, no socket connection.

### New dependency

```toml
clap_complete = "4"
```

Same major version as `clap`. Feature-flag: none needed — all shell variants are included by default.

### Shell detection

```rust
fn detect_shell() -> Option<clap_complete::Shell> {
    let shell_path = std::env::var("SHELL").ok()?;
    let binary = std::path::Path::new(&shell_path).file_name()?.to_str()?;
    match binary {
        "bash" => Some(Shell::Bash),
        "zsh"  => Some(Shell::Zsh),
        "fish" => Some(Shell::Fish),
        _      => None,
    }
    // PowerShell and Elvish are not set via $SHELL — user must specify explicitly
}
```

PowerShell (`pwsh`) and Elvish (`elvish`) must be specified as explicit arguments since they don't set `$SHELL` on Unix systems.

### Generation

```rust
clap_complete::generate(shell, &mut cli_command, "portless", &mut writer);
```

`writer` is either `std::io::stdout()` (print mode) or the opened target file (install mode).

### TTY detection for auto-confirm

```rust
use std::io::IsTerminal;
if !std::io::stdin().is_terminal() {
    // non-interactive: auto-confirm install
}
```

`IsTerminal` is stable since Rust 1.70.

## Files

| File | Change |
|---|---|
| `Cargo.toml` | Add `clap_complete = "4"` |
| `src/cli/completion.rs` | New — all completion logic: detection, path resolution, prompt, generation |
| `src/cli/mod.rs` | Add `Completion` subcommand variant + handler dispatch |

## Error Handling

- Shell not detected and no arg provided → print clear error with explicit shell list, exit 1
- Install path parent dir creation failure → surface `io::Error` with path, exit 1
- File write failure → surface `io::Error`, exit 1
- Unknown shell arg → clap handles via enum validation before reaching our code

## Testing

- Unit tests for `detect_shell()` with mocked `SHELL` values
- Unit tests for `default_install_path(shell)` covering all 5 shells, including OMZ detection
- Unit tests for zsh OMZ vs plain path selection
- Integration test: `portless completion bash --print` produces non-empty output containing `portless`
- Integration test: `portless completion fish --print` produces valid fish syntax (`complete -c portless`)
