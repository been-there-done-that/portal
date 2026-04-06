use crate::error::Result;
use std::fs;
use std::path::Path;

/// Strip known package runner prefixes from argv slice.
pub fn strip_runner_prefix<'a>(args: &'a [&'a str]) -> &'a [&'a str] {
    if args.is_empty() {
        return args;
    }

    match args[0] {
        "npx" => &args[1..],
        "bunx" => &args[1..],
        "deno" if args.len() > 1 && args[1] == "run" => &args[2..],
        "pnpm" if args.len() > 1 => match args[1] {
            "dlx" => &args[2..],
            "exec" => &args[2..],
            _ => args,
        },
        "yarn" if args.len() > 1 && args[1] == "exec" => &args[2..],
        _ => args,
    }
}

/// Sanitize a string into a valid, lowercase hostname segment.
/// Rules: lowercase, replace non-alphanumeric chars with '-', collapse runs of '-', trim leading/trailing '-'.
pub fn sanitize_hostname(s: &str) -> String {
    let lower = s.to_lowercase();
    let mut result = String::new();

    for c in lower.chars() {
        if c.is_ascii_alphanumeric() {
            result.push(c);
        } else {
            result.push('-');
        }
    }

    // Collapse consecutive dashes
    while result.contains("--") {
        result = result.replace("--", "-");
    }

    // Trim leading/trailing dashes
    result.trim_matches('-').to_string()
}

/// Infer project name from cwd.
/// Priority: 1) override_name arg, 2) package.json "name" field, 3) directory name.
pub fn infer_project_name(cwd: &Path, override_name: Option<&str>) -> String {
    if let Some(name) = override_name {
        return sanitize_hostname(name);
    }

    // Try package.json
    let package_json_path = cwd.join("package.json");
    if package_json_path.exists() {
        if let Ok(contents) = fs::read_to_string(&package_json_path) {
            if let Ok(json) = serde_json::from_str::<serde_json::Value>(&contents) {
                if let Some(name) = json.get("name").and_then(|v| v.as_str()) {
                    if !name.is_empty() {
                        return sanitize_hostname(name);
                    }
                }
            }
        }
    }

    // Fall back to directory name
    cwd.file_name()
        .and_then(|name| name.to_str())
        .map(sanitize_hostname)
        .unwrap_or_else(|| "app".to_string())
}

/// Resolve the full hostname for a project (e.g. "myapp.localhost").
/// If cwd is a linked git worktree (.git is a file, not directory), prepends branch name.
pub fn resolve_hostname(cwd: &Path, override_name: Option<&str>, tld: &str) -> String {
    let project_name = infer_project_name(cwd, override_name);

    // Check if .git is a file (linked worktree)
    let git_path = cwd.join(".git");

    if let Ok(metadata) = fs::metadata(&git_path) {
        if metadata.is_file() {
            // Read the .git file to get the gitdir path
            if let Ok(contents) = fs::read_to_string(&git_path) {
                // Parse "gitdir: /path/to/.git/worktrees/<name>"
                if let Some(gitdir_line) = contents.lines().find(|line| line.starts_with("gitdir:"))
                {
                    let gitdir_path = gitdir_line
                        .strip_prefix("gitdir:")
                        .map(|s| s.trim())
                        .unwrap_or("");

                    // Read HEAD to get branch name
                    let head_path = Path::new(gitdir_path).join("HEAD");
                    if let Ok(head_contents) = fs::read_to_string(&head_path) {
                        let branch = head_contents
                            .trim()
                            .strip_prefix("ref: refs/heads/")
                            .unwrap_or(&head_contents);
                        let sanitized_branch = sanitize_hostname(branch);
                        return format!("{}-{}.{}", sanitized_branch, project_name, tld);
                    }
                }
            }
        }
    }

    // Regular case (main worktree or no git)
    format!("{}.{}", project_name, tld)
}

/// Detect framework from project files
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Framework {
    Vite,
    Astro,
    Angular,
    ReactRouter,
    Expo,
    Nuxt,
    Remix,
    SvelteKit,
    Unknown,
}

impl Framework {
    fn extra_args(&self, port: u16) -> Vec<String> {
        match self {
            Framework::Vite => vec!["--port".to_string(), port.to_string(), "--host".to_string()],
            Framework::Astro => vec![
                "--port".to_string(),
                port.to_string(),
                "--host".to_string(),
                "0.0.0.0".to_string(),
            ],
            Framework::Angular => vec![
                "--port".to_string(),
                port.to_string(),
                "--host".to_string(),
                "0.0.0.0".to_string(),
            ],
            Framework::ReactRouter => vec!["--port".to_string(), port.to_string()],
            Framework::Expo => vec!["--port".to_string(), port.to_string()],
            Framework::Nuxt => vec!["--port".to_string(), port.to_string()],
            Framework::Remix => vec!["--port".to_string(), port.to_string()],
            Framework::SvelteKit => {
                vec!["--port".to_string(), port.to_string(), "--host".to_string()]
            }
            Framework::Unknown => vec![],
        }
    }
}

/// Detect framework from cwd
fn detect_framework(cwd: &Path) -> Framework {
    // Check for angular.json
    if cwd.join("angular.json").exists() {
        return Framework::Angular;
    }

    // Check for svelte.config.js or svelte.config.ts
    if cwd.join("svelte.config.js").exists() || cwd.join("svelte.config.ts").exists() {
        return Framework::SvelteKit;
    }

    // Check package.json scripts
    let package_json_path = cwd.join("package.json");
    if package_json_path.exists() {
        if let Ok(contents) = fs::read_to_string(&package_json_path) {
            if let Ok(json) = serde_json::from_str::<serde_json::Value>(&contents) {
                if let Some(scripts) = json.get("scripts").and_then(|v| v.as_object()) {
                    let scripts_str = serde_json::to_string(scripts).unwrap_or_default();

                    // Order matters: check more specific frameworks first
                    if scripts_str.contains("vite") {
                        return Framework::Vite;
                    }
                    if scripts_str.contains("astro") {
                        return Framework::Astro;
                    }
                    if scripts_str.contains("react-router") {
                        return Framework::ReactRouter;
                    }
                    if scripts_str.contains("nuxt") {
                        return Framework::Nuxt;
                    }
                    if scripts_str.contains("remix") {
                        return Framework::Remix;
                    }
                }
            }
        }
    }

    // Check for app.json with expo
    let app_json_path = cwd.join("app.json");
    if app_json_path.exists() {
        if let Ok(contents) = fs::read_to_string(&app_json_path) {
            if contents.contains("expo") {
                return Framework::Expo;
            }
        }
    }

    Framework::Unknown
}

/// Return extra CLI args to inject for the given framework, based on cwd and command args.
/// e.g. ["--port", "4123", "--host"] for Vite.
pub fn extra_args_for_port(cwd: &Path, _args: &[&str], port: u16) -> Result<Vec<String>> {
    let framework = detect_framework(cwd);
    Ok(framework.extra_args(port))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::fs;
    use tempfile::TempDir;

    #[test]
    fn detects_vite() {
        let temp = TempDir::new().unwrap();
        let package_json = temp.path().join("package.json");
        fs::write(
            &package_json,
            json!({
                "name": "my-app",
                "scripts": {
                    "dev": "vite"
                }
            })
            .to_string(),
        )
        .unwrap();

        let args = extra_args_for_port(temp.path(), &[], 4123).unwrap();
        assert!(args.contains(&"--port".to_string()));
        assert!(args.contains(&"4123".to_string()));
        assert!(args.contains(&"--host".to_string()));
    }

    #[test]
    fn detects_astro() {
        let temp = TempDir::new().unwrap();
        let package_json = temp.path().join("package.json");
        fs::write(
            &package_json,
            json!({
                "name": "my-app",
                "scripts": {
                    "dev": "astro dev"
                }
            })
            .to_string(),
        )
        .unwrap();

        let args = extra_args_for_port(temp.path(), &[], 4123).unwrap();
        assert!(args.contains(&"--port".to_string()));
        assert!(args.contains(&"--host".to_string()));
        assert!(args.contains(&"0.0.0.0".to_string()));
    }

    #[test]
    fn no_injection_for_unknown() {
        let temp = TempDir::new().unwrap();
        let package_json = temp.path().join("package.json");
        fs::write(
            &package_json,
            json!({
                "name": "my-app",
                "scripts": {
                    "dev": "node server.js"
                }
            })
            .to_string(),
        )
        .unwrap();

        let args = extra_args_for_port(temp.path(), &[], 4123).unwrap();
        assert!(args.is_empty());
    }

    #[test]
    fn sanitizes_hostname() {
        assert_eq!(sanitize_hostname("My App"), "my-app");
        assert_eq!(sanitize_hostname("feature/login"), "feature-login");
        assert_eq!(sanitize_hostname("api_v2"), "api-v2");
        assert_eq!(sanitize_hostname("  hello--world  "), "hello-world");
        assert_eq!(
            sanitize_hostname("test___multiple___underscores"),
            "test-multiple-underscores"
        );
        assert_eq!(sanitize_hostname("UPPERCASE"), "uppercase");
    }

    #[test]
    fn infers_project_name_from_dir() {
        let temp = TempDir::new().unwrap();
        let name = infer_project_name(temp.path(), None);
        assert!(!name.is_empty());
        assert!(name.chars().all(|c| c.is_ascii_alphanumeric() || c == '-'));
    }

    #[test]
    fn infers_project_name_from_override() {
        let temp = TempDir::new().unwrap();
        let name = infer_project_name(temp.path(), Some("my-override"));
        assert_eq!(name, "my-override");
    }

    #[test]
    fn infers_project_name_from_package_json() {
        let temp = TempDir::new().unwrap();
        let package_json = temp.path().join("package.json");
        fs::write(
            &package_json,
            json!({
                "name": "my-package"
            })
            .to_string(),
        )
        .unwrap();

        let name = infer_project_name(temp.path(), None);
        assert_eq!(name, "my-package");
    }

    #[test]
    fn strips_runner_prefixes() {
        assert_eq!(
            strip_runner_prefix(&["npx", "vite", "--mode", "dev"]),
            &["vite", "--mode", "dev"]
        );
        assert_eq!(
            strip_runner_prefix(&["pnpm", "dlx", "astro", "dev"]),
            &["astro", "dev"]
        );
        assert_eq!(
            strip_runner_prefix(&["pnpm", "exec", "remix", "dev"]),
            &["remix", "dev"]
        );
        assert_eq!(strip_runner_prefix(&["yarn", "exec", "nuxt"]), &["nuxt"]);
        assert_eq!(strip_runner_prefix(&["bunx", "vite"]), &["vite"]);
        assert_eq!(
            strip_runner_prefix(&["deno", "run", "server.ts"]),
            &["server.ts"]
        );
        assert_eq!(
            strip_runner_prefix(&["node", "server.js"]),
            &["node", "server.js"]
        );
    }

    #[test]
    fn resolve_hostname_main_worktree() {
        let temp = TempDir::new().unwrap();
        let package_json = temp.path().join("package.json");
        fs::write(
            &package_json,
            json!({
                "name": "myapp"
            })
            .to_string(),
        )
        .unwrap();

        let hostname = resolve_hostname(temp.path(), None, "localhost");
        assert_eq!(hostname, "myapp.localhost");
    }

    #[test]
    fn resolve_hostname_git_worktree() {
        let temp = TempDir::new().unwrap();

        // Create a mock git worktree structure
        let worktree_dir = temp.path().join(".git_worktrees");
        fs::create_dir_all(&worktree_dir).unwrap();
        let head_file = worktree_dir.join("HEAD");
        fs::write(&head_file, "ref: refs/heads/feature/my-branch").unwrap();

        // Create .git file pointing to the worktree
        let git_file = temp.path().join(".git");
        let gitdir_path = worktree_dir.to_string_lossy().to_string();
        fs::write(&git_file, format!("gitdir: {}", gitdir_path)).unwrap();

        let package_json = temp.path().join("package.json");
        fs::write(
            &package_json,
            json!({
                "name": "myapp"
            })
            .to_string(),
        )
        .unwrap();

        let hostname = resolve_hostname(temp.path(), None, "localhost");
        assert!(hostname.contains("feature-my-branch"));
        assert!(hostname.contains("myapp"));
        assert!(hostname.contains("localhost"));
    }
}
