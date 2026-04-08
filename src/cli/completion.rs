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

fn post_install_message(shell: Shell, path: &Path, is_omz: bool) {
    println!("{} Installed to {}", console::style("✓").green(), path.display());
    match shell {
        Shell::Zsh if !is_omz => {
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
    use clap::CommandFactory;
    use clap_complete::generate;
    use std::io::{IsTerminal, Write};

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

    // Confirm: read from stdin whether or not it's a TTY
    let confirmed = {
        let mut input = String::new();
        std::io::stdin().read_line(&mut input)?;
        matches!(input.trim().to_ascii_lowercase().as_str(), "" | "y" | "yes")
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

    let omz = is_omz();
    post_install_message(shell, &install_path, omz);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    static ENV_LOCK: Mutex<()> = Mutex::new(());

    #[test]
    fn detect_shell_bash() {
        let _guard = ENV_LOCK.lock().unwrap();
        unsafe { std::env::set_var("SHELL", "/bin/bash") };
        assert!(matches!(detect_shell(), Some(Shell::Bash)));
        unsafe { std::env::remove_var("SHELL") };
    }

    #[test]
    fn detect_shell_zsh() {
        let _guard = ENV_LOCK.lock().unwrap();
        unsafe { std::env::set_var("SHELL", "/usr/local/bin/zsh") };
        assert!(matches!(detect_shell(), Some(Shell::Zsh)));
        unsafe { std::env::remove_var("SHELL") };
    }

    #[test]
    fn detect_shell_fish() {
        let _guard = ENV_LOCK.lock().unwrap();
        unsafe { std::env::set_var("SHELL", "/opt/homebrew/bin/fish") };
        assert!(matches!(detect_shell(), Some(Shell::Fish)));
        unsafe { std::env::remove_var("SHELL") };
    }

    #[test]
    fn detect_shell_pwsh() {
        let _guard = ENV_LOCK.lock().unwrap();
        unsafe { std::env::set_var("SHELL", "/usr/local/bin/pwsh") };
        assert!(matches!(detect_shell(), Some(Shell::PowerShell)));
        unsafe { std::env::remove_var("SHELL") };
    }

    #[test]
    fn detect_shell_elvish() {
        let _guard = ENV_LOCK.lock().unwrap();
        unsafe { std::env::set_var("SHELL", "/usr/local/bin/elvish") };
        assert!(matches!(detect_shell(), Some(Shell::Elvish)));
        unsafe { std::env::remove_var("SHELL") };
    }

    #[test]
    fn detect_shell_unknown_returns_none() {
        let _guard = ENV_LOCK.lock().unwrap();
        unsafe { std::env::set_var("SHELL", "/usr/bin/tcsh") };
        assert!(detect_shell().is_none());
        unsafe { std::env::remove_var("SHELL") };
    }

    #[test]
    fn detect_shell_unset_returns_none() {
        let _guard = ENV_LOCK.lock().unwrap();
        unsafe { std::env::remove_var("SHELL") };
        assert!(detect_shell().is_none());
    }

    #[test]
    fn default_path_fish() {
        let _guard = ENV_LOCK.lock().unwrap();
        let path = default_install_path(Shell::Fish);
        assert!(path.to_string_lossy().ends_with(".config/fish/completions/portal.fish"));
    }

    #[test]
    fn default_path_bash() {
        let _guard = ENV_LOCK.lock().unwrap();
        let path = default_install_path(Shell::Bash);
        assert!(path.to_string_lossy().ends_with(".local/share/bash-completion/completions/portal"));
    }

    #[test]
    fn default_path_zsh_no_omz() {
        let _guard = ENV_LOCK.lock().unwrap();
        unsafe { std::env::remove_var("ZSH") };
        let path = default_install_path(Shell::Zsh);
        assert!(path.to_string_lossy().ends_with(".zfunc/_portal"));
    }

    #[test]
    fn default_path_zsh_with_omz() {
        let _guard = ENV_LOCK.lock().unwrap();
        // Create a temp dir to simulate $ZSH pointing to a real directory
        let tmp = tempfile::tempdir().unwrap();
        unsafe { std::env::set_var("ZSH", tmp.path()) };
        let path = default_install_path(Shell::Zsh);
        assert!(path.to_string_lossy().ends_with(".oh-my-zsh/completions/_portal"));
        unsafe { std::env::remove_var("ZSH") };
    }

    #[test]
    fn default_path_powershell() {
        let _guard = ENV_LOCK.lock().unwrap();
        let path = default_install_path(Shell::PowerShell);
        assert!(path.to_string_lossy().ends_with("PowerShell/Completions/portal.ps1"));
    }

    #[test]
    fn default_path_elvish() {
        let _guard = ENV_LOCK.lock().unwrap();
        let path = default_install_path(Shell::Elvish);
        assert!(path.to_string_lossy().ends_with(".config/elvish/lib/portal.elv"));
    }

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
}
