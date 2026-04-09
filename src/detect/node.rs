use crate::detect::{LanguageDriver, PortInjection};
use std::fs;
use std::path::Path;

pub const KNOWN_RUNNERS: &[&str] = &[
    "npm", "pnpm", "yarn", "bun", "node", "deno", "npx", "bunx", "pnpx", "python", "python3",
    "ruby", "go", "cargo", "java", "sh", "bash", "zsh", "fish",
];

pub fn is_known_runner(cmd: &str) -> bool {
    KNOWN_RUNNERS.contains(&cmd)
}

pub(crate) fn detect_package_manager(cwd: &Path) -> &'static str {
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

fn pick_dev_script(json: &serde_json::Value) -> Option<String> {
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

pub fn resolve_run_args(cwd: &Path, args: Vec<String>) -> Vec<String> {
    let first = match args.first() {
        Some(f) => f.clone(),
        None => return args,
    };
    if is_known_runner(&first) {
        return args;
    }
    let pkg_path = cwd.join("package.json");
    let script_exists = pkg_path.exists() && {
        fs::read_to_string(&pkg_path)
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
        let pm = detect_package_manager(cwd);
        let mut new_args = vec![pm.to_string(), "run".to_string()];
        new_args.extend(args);
        new_args
    } else {
        args
    }
}

// JS framework detection for port injection
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Framework {
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
        let p = port.to_string();
        match self {
            Framework::Vite => vec!["--port".into(), p, "--host".into()],
            Framework::Astro => vec!["--port".into(), p, "--host".into(), "0.0.0.0".into()],
            Framework::Angular => vec!["--port".into(), p, "--host".into(), "0.0.0.0".into()],
            Framework::SvelteKit => vec!["--port".into(), p, "--host".into()],
            Framework::ReactRouter | Framework::Expo | Framework::Nuxt | Framework::Remix => {
                vec!["--port".into(), p]
            }
            Framework::Unknown => vec![],
        }
    }
}

fn detect_framework(cwd: &Path) -> Framework {
    if cwd.join("angular.json").exists() {
        return Framework::Angular;
    }
    if cwd.join("svelte.config.js").exists() || cwd.join("svelte.config.ts").exists() {
        return Framework::SvelteKit;
    }
    if let Ok(s) = fs::read_to_string(cwd.join("package.json")) {
        if let Ok(json) = serde_json::from_str::<serde_json::Value>(&s) {
            if let Some(scripts) = json.get("scripts").and_then(|v| v.as_object()) {
                let scripts_str = serde_json::to_string(scripts).unwrap_or_default();
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
    if let Ok(s) = fs::read_to_string(cwd.join("app.json")) {
        if s.contains("expo") {
            return Framework::Expo;
        }
    }
    Framework::Unknown
}

// ─── NodeDriver ───────────────────────────────────────────────────────────────

pub struct NodeDriver;

impl LanguageDriver for NodeDriver {
    fn detect(&self, cwd: &Path) -> bool {
        cwd.join("package.json").exists()
    }
    fn priority(&self) -> u8 {
        40
    }
    fn name(&self) -> &'static str {
        "Node.js"
    }
    fn project_name(&self, cwd: &Path) -> Option<String> {
        crate::detect::read_json_field(cwd, "package.json", "name")
    }
    fn start_command(&self, cwd: &Path) -> Option<String> {
        let contents = fs::read_to_string(cwd.join("package.json")).ok()?;
        let json: serde_json::Value = serde_json::from_str(&contents).ok()?;
        let script = pick_dev_script(&json)?;
        let pm = detect_package_manager(cwd);
        Some(format!("{pm} run {script}"))
    }
    fn port_injection(&self, cwd: &Path, port: u16) -> PortInjection {
        let extra = detect_framework(cwd).extra_args(port);
        if extra.is_empty() {
            PortInjection::EnvOnly
        } else {
            PortInjection::CliArgs(extra)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    #[test]
    fn node_driver_detects_package_json() {
        let tmp = TempDir::new().unwrap();
        fs::write(
            tmp.path().join("package.json"),
            r#"{"name":"app","scripts":{"dev":"vite"}}"#,
        )
        .unwrap();
        assert!(NodeDriver.detect(tmp.path()));
    }

    #[test]
    fn node_driver_does_not_detect_without_package_json() {
        let tmp = TempDir::new().unwrap();
        assert!(!NodeDriver.detect(tmp.path()));
    }

    #[test]
    fn node_driver_project_name_from_package_json() {
        let tmp = TempDir::new().unwrap();
        fs::write(tmp.path().join("package.json"), r#"{"name":"my-app"}"#).unwrap();
        assert_eq!(
            NodeDriver.project_name(tmp.path()),
            Some("my-app".to_string())
        );
    }

    #[test]
    fn node_driver_start_command_picks_dev_script() {
        let tmp = TempDir::new().unwrap();
        fs::write(
            tmp.path().join("package.json"),
            r#"{"scripts":{"dev":"vite","build":"tsc"}}"#,
        )
        .unwrap();
        let cmd = NodeDriver.start_command(tmp.path()).unwrap();
        assert!(cmd.contains("dev"), "expected dev in '{cmd}'");
    }

    #[test]
    fn node_driver_vite_injection() {
        let tmp = TempDir::new().unwrap();
        fs::write(
            tmp.path().join("package.json"),
            r#"{"scripts":{"dev":"vite"}}"#,
        )
        .unwrap();
        let inj = NodeDriver.port_injection(tmp.path(), 4123);
        match inj {
            crate::detect::PortInjection::CliArgs(args) => {
                assert!(args.contains(&"--port".to_string()));
                assert!(args.contains(&"4123".to_string()));
            }
            _ => panic!("expected CliArgs for Vite"),
        }
    }

    #[test]
    fn node_driver_unknown_framework_env_only() {
        let tmp = TempDir::new().unwrap();
        fs::write(
            tmp.path().join("package.json"),
            r#"{"scripts":{"dev":"node server.js"}}"#,
        )
        .unwrap();
        assert!(matches!(
            NodeDriver.port_injection(tmp.path(), 4123),
            crate::detect::PortInjection::EnvOnly
        ));
    }

    #[test]
    fn resolve_run_args_expands_script_name() {
        let tmp = TempDir::new().unwrap();
        fs::write(tmp.path().join("pnpm-lock.yaml"), "").unwrap();
        fs::write(
            tmp.path().join("package.json"),
            r#"{"scripts":{"dev":"vite"}}"#,
        )
        .unwrap();
        let result = resolve_run_args(tmp.path(), vec!["dev".to_string()]);
        assert_eq!(result, vec!["pnpm", "run", "dev"]);
    }

    #[test]
    fn resolve_run_args_passthrough_known_runner() {
        let tmp = TempDir::new().unwrap();
        let args = vec!["npm".to_string(), "run".to_string(), "dev".to_string()];
        assert_eq!(resolve_run_args(tmp.path(), args.clone()), args);
    }
}
