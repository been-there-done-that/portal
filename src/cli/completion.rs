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
