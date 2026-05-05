use serde::Deserialize;
use std::path::{Path, PathBuf};

pub struct WorkspacePackage {
    pub dir: PathBuf,
    pub name: String,
    pub command: Vec<String>,
    pub injection: crate::detect::PortInjection,
}

/// Walk up from `cwd` to find the first directory containing
/// `pnpm-workspace.yaml` or a `package.json` with a `"workspaces"` field.
pub fn find_workspace_root(cwd: &Path) -> Option<PathBuf> {
    let mut dir = cwd.to_path_buf();
    loop {
        if dir.join("pnpm-workspace.yaml").exists() {
            return Some(dir);
        }
        if let Ok(content) = std::fs::read_to_string(dir.join("package.json")) {
            if let Ok(val) = serde_json::from_str::<serde_json::Value>(&content) {
                if val.get("workspaces").is_some() {
                    return Some(dir);
                }
            }
        }
        if !dir.pop() {
            return None;
        }
    }
}

/// Discover all runnable workspace packages under `root`.
/// Packages without a detected dev command are silently skipped.
pub fn discover_workspace_packages(
    root: &Path,
    config: &crate::config::Config,
) -> Vec<WorkspacePackage> {
    let globs = workspace_globs(root);
    if globs.is_empty() {
        return vec![];
    }

    let registry = crate::detect::DriverRegistry::new(config);
    let mut packages = Vec::new();

    for glob in &globs {
        for pkg_dir in expand_glob(root, glob) {
            let Some(driver) = registry.detect(&pkg_dir) else {
                continue;
            };
            let Some(cmd_str) = driver.start_command(&pkg_dir) else {
                continue;
            };
            let Ok(args) = crate::cli::parse_command_line(&cmd_str) else {
                continue;
            };
            let name = driver
                .project_name(&pkg_dir)
                .unwrap_or_else(|| {
                    pkg_dir
                        .file_name()
                        .unwrap_or_default()
                        .to_string_lossy()
                        .into_owned()
                });
            let injection = driver.port_injection(&pkg_dir, 0);
            packages.push(WorkspacePackage {
                dir: pkg_dir,
                name,
                command: args,
                injection,
            });
        }
    }

    packages
}

pub fn has_turbo_config(dir: &Path) -> bool {
    dir.join("turbo.json").exists()
}

#[derive(Deserialize)]
struct PnpmWorkspace {
    packages: Vec<String>,
}

fn workspace_globs(root: &Path) -> Vec<String> {
    if let Ok(content) = std::fs::read_to_string(root.join("pnpm-workspace.yaml")) {
        if let Ok(ws) = serde_yaml::from_str::<PnpmWorkspace>(&content) {
            return ws.packages;
        }
    }
    if let Ok(content) = std::fs::read_to_string(root.join("package.json")) {
        if let Ok(val) = serde_json::from_str::<serde_json::Value>(&content) {
            if let Some(ws) = val.get("workspaces").and_then(|v| v.as_array()) {
                return ws
                    .iter()
                    .filter_map(|v| v.as_str().map(String::from))
                    .collect();
            }
        }
    }
    vec![]
}

/// Expand a workspace glob pattern against `root`.
/// Supports `dir/*` (list immediate subdirs) and exact paths.
fn expand_glob(root: &Path, pattern: &str) -> Vec<PathBuf> {
    let clean = pattern.trim_end_matches('/');
    if let Some(prefix) = clean.strip_suffix("/*") {
        let parent = root.join(prefix);
        if parent.starts_with(root) {
            if let Ok(entries) = std::fs::read_dir(&parent) {
                return entries
                    .filter_map(|e| e.ok())
                    .map(|e| e.path())
                    .filter(|p| p.is_dir() && p.starts_with(root))
                    .collect();
            }
        }
        return vec![];
    }
    let p = root.join(clean);
    if p.is_dir() && p.starts_with(root) {
        return vec![p];
    }
    if clean.contains('*') {
        tracing::warn!("workspace: glob pattern {:?} is not supported (only dir/* is); skipping", pattern);
    }
    vec![]
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn finds_pnpm_workspace_root() {
        let root = TempDir::new().unwrap();
        std::fs::write(
            root.path().join("pnpm-workspace.yaml"),
            "packages:\n  - \"packages/*\"\n",
        ).unwrap();

        let found = find_workspace_root(root.path());
        assert_eq!(found, Some(root.path().to_path_buf()));
    }

    #[test]
    fn finds_npm_workspace_root() {
        let root = TempDir::new().unwrap();
        std::fs::write(
            root.path().join("package.json"),
            r#"{"name":"root","workspaces":["packages/*"]}"#,
        ).unwrap();

        let found = find_workspace_root(root.path());
        assert_eq!(found, Some(root.path().to_path_buf()));
    }

    #[test]
    fn returns_none_for_non_workspace() {
        let root = TempDir::new().unwrap();
        assert!(find_workspace_root(root.path()).is_none());
    }

    #[test]
    fn discovers_packages_from_pnpm_workspace() {
        let root = TempDir::new().unwrap();
        std::fs::write(
            root.path().join("pnpm-workspace.yaml"),
            "packages:\n  - \"apps/*\"\n",
        ).unwrap();

        let apps = root.path().join("apps");
        std::fs::create_dir_all(apps.join("web")).unwrap();
        std::fs::write(
            apps.join("web").join("package.json"),
            r#"{"name":"web","scripts":{"dev":"vite"}}"#,
        ).unwrap();
        std::fs::write(apps.join("web").join("pnpm-lock.yaml"), "").unwrap();

        let config = crate::config::Config::default();
        let pkgs = discover_workspace_packages(root.path(), &config);
        assert_eq!(pkgs.len(), 1);
        assert_eq!(pkgs[0].name, "web");
    }

    #[test]
    fn finds_workspace_root_from_subdirectory() {
        let root = TempDir::new().unwrap();
        std::fs::write(
            root.path().join("pnpm-workspace.yaml"),
            "packages:\n  - \"packages/*\"\n",
        ).unwrap();
        let subdir = root.path().join("packages").join("myapp");
        std::fs::create_dir_all(&subdir).unwrap();

        let found = find_workspace_root(&subdir);
        assert_eq!(found, Some(root.path().to_path_buf()));
    }

    #[test]
    fn has_turbo_config_detects_turbo_json() {
        let dir = TempDir::new().unwrap();
        assert!(!has_turbo_config(dir.path()));
        std::fs::write(dir.path().join("turbo.json"), "{}").unwrap();
        assert!(has_turbo_config(dir.path()));
    }
}
