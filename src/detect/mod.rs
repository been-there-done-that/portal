use std::fs;
use std::path::Path;

// ─── Trait ───────────────────────────────────────────────────────────────────

pub trait LanguageDriver: Send + Sync {
    fn detect(&self, cwd: &Path) -> bool;
    fn priority(&self) -> u8 {
        50
    }
    fn name(&self) -> &'static str;
    fn project_name(&self, cwd: &Path) -> Option<String>;
    fn start_command(&self, cwd: &Path) -> Option<String>;
    fn port_injection(&self, cwd: &Path, port: u16) -> PortInjection;
    /// Return (service_name, host_port) pairs for services that declare port mappings.
    /// Default is empty — most drivers do not use this.
    fn service_port_candidates(&self, _cwd: &Path) -> Vec<(String, u16)> {
        vec![]
    }
}

// ─── Port injection strategy ─────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub enum PortInjection {
    EnvOnly,
    CliArgs(Vec<String>),
    AppendAddress(String),
}

// ─── Registry ────────────────────────────────────────────────────────────────

pub struct DriverRegistry {
    pub(crate) drivers: Vec<Box<dyn LanguageDriver>>,
}

impl DriverRegistry {
    /// Build the default registry with all built-in drivers registered.
    pub fn new(config: &crate::config::Config) -> Self {
        let mut reg = Self {
            drivers: vec![
                Box::new(PortalTomlDriver {
                    config: config.project.clone(),
                }),
                Box::new(python::DjangoDriver),
                Box::new(python::UvicornDriver),
                Box::new(python::FlaskDriver),
                Box::new(ruby::RailsDriver),
                Box::new(ruby::RackDriver),
                Box::new(php::PhpDriver),
                Box::new(go::GoDriver),
                Box::new(rust::RustDriver),
                Box::new(docker_compose::DockerComposeDriver),
                Box::new(storybook::StorybookDriver),
                Box::new(node::NodeDriver),
            ],
        };
        reg.sort();
        reg
    }

    pub fn sort(&mut self) {
        self.drivers.sort_by(|a, b| b.priority().cmp(&a.priority()));
    }

    pub fn detect(&self, cwd: &Path) -> Option<&dyn LanguageDriver> {
        self.drivers
            .iter()
            .find(|d| d.detect(cwd))
            .map(|d| d.as_ref())
    }

    pub fn detect_language(&self, cwd: &Path) -> Option<&dyn LanguageDriver> {
        self.drivers
            .iter()
            .filter(|d| d.name() != "portal.toml")
            .find(|d| d.detect(cwd))
            .map(|d| d.as_ref())
    }
}

// ─── Utility functions ───────────────────────────────────────────────────────

pub fn sanitize_hostname(s: &str) -> String {
    let mut result = String::new();
    let mut prev_dash = false;
    for c in s.to_lowercase().chars() {
        if c.is_ascii_alphanumeric() {
            result.push(c);
            prev_dash = false;
        } else if !prev_dash {
            result.push('-');
            prev_dash = true;
        }
    }
    result.trim_matches('-').to_string()
}

pub fn infer_project_name(cwd: &Path, override_name: Option<&str>) -> String {
    if let Some(name) = override_name {
        return sanitize_hostname(name);
    }
    if let Some(n) = read_json_field(cwd, "package.json", "name") {
        if !n.is_empty() {
            return sanitize_hostname(&n);
        }
    }
    if let Some(n) = read_toml_field(cwd, "pyproject.toml", &["project", "name"]) {
        if !n.is_empty() {
            return sanitize_hostname(&n);
        }
    }
    if let Some(n) = read_toml_field(cwd, "Cargo.toml", &["package", "name"]) {
        if !n.is_empty() {
            return sanitize_hostname(&n);
        }
    }
    if let Some(n) = read_go_module_name(cwd) {
        return sanitize_hostname(&n);
    }
    if let Some(n) = read_json_field(cwd, "composer.json", "name") {
        let segment = n.split('/').last().unwrap_or(&n).to_string();
        if !segment.is_empty() {
            return sanitize_hostname(&segment);
        }
    }
    cwd.file_name()
        .and_then(|n| n.to_str())
        .map(sanitize_hostname)
        .unwrap_or_else(|| "app".to_string())
}

pub fn resolve_hostname(cwd: &Path, override_name: Option<&str>, tld: &str) -> String {
    let project_name = infer_project_name(cwd, override_name);
    let git_path = cwd.join(".git");
    if let Ok(metadata) = fs::metadata(&git_path) {
        if metadata.is_file() {
            if let Ok(contents) = fs::read_to_string(&git_path) {
                if let Some(gitdir_line) = contents.lines().find(|l| l.starts_with("gitdir:")) {
                    let gitdir_path = gitdir_line
                        .strip_prefix("gitdir:")
                        .map(|s| s.trim())
                        .unwrap_or("");
                    let gitdir_abs = if Path::new(gitdir_path).is_absolute() {
                        std::path::PathBuf::from(gitdir_path)
                    } else {
                        cwd.join(gitdir_path)
                    };
                    let head_path = gitdir_abs.join("HEAD");
                    if let Ok(head_contents) = fs::read_to_string(&head_path) {
                        let branch = head_contents
                            .trim()
                            .strip_prefix("ref: refs/heads/")
                            .unwrap_or(&head_contents);
                        return format!("{}-{}.{}", sanitize_hostname(branch), project_name, tld);
                    }
                }
            }
        }
    }
    format!("{}.{}", project_name, tld)
}

// ─── Internal manifest helpers ───────────────────────────────────────────────

pub(crate) fn read_json_field(cwd: &Path, filename: &str, key: &str) -> Option<String> {
    let contents = fs::read_to_string(cwd.join(filename)).ok()?;
    let json: serde_json::Value = serde_json::from_str(&contents).ok()?;
    json.get(key)?.as_str().map(String::from)
}

pub(crate) fn read_toml_field(cwd: &Path, filename: &str, keys: &[&str]) -> Option<String> {
    let contents = fs::read_to_string(cwd.join(filename)).ok()?;
    let val: toml::Value = toml::from_str(&contents).ok()?;
    let mut cur = &val;
    for key in keys {
        cur = cur.get(key)?;
    }
    cur.as_str().map(String::from)
}

pub(crate) fn file_contains(cwd: &Path, filename: &str, needle: &str) -> bool {
    fs::read_to_string(cwd.join(filename))
        .map(|s| s.contains(needle))
        .unwrap_or(false)
}

fn read_go_module_name(cwd: &Path) -> Option<String> {
    let contents = fs::read_to_string(cwd.join("go.mod")).ok()?;
    let first = contents.lines().next()?;
    let module = first.strip_prefix("module ")?.trim();
    Some(module.split('/').last()?.to_string())
}

// ─── Re-exports used by cli/mod.rs ───────────────────────────────────────────

pub use node::{is_known_runner, resolve_run_args, KNOWN_RUNNERS};

// ─── Sub-modules ─────────────────────────────────────────────────────────────

pub mod docker_compose;
pub mod go;
pub mod node;
pub mod php;
pub mod python;
pub mod ruby;
pub mod rust;
pub mod storybook;

// ─── PortalTomlDriver ────────────────────────────────────────────────────────

pub struct PortalTomlDriver {
    pub config: crate::config::ProjectConfig,
}

impl LanguageDriver for PortalTomlDriver {
    fn detect(&self, _cwd: &Path) -> bool {
        self.config.start_command.is_some()
            || self.config.port_arg.is_some()
            || self.config.port_position.is_some()
    }
    fn priority(&self) -> u8 {
        255
    }
    fn name(&self) -> &'static str {
        "portal.toml"
    }
    fn project_name(&self, _cwd: &Path) -> Option<String> {
        self.config.name.clone()
    }
    fn start_command(&self, _cwd: &Path) -> Option<String> {
        self.config.start_command.clone()
    }
    fn port_injection(&self, _cwd: &Path, port: u16) -> PortInjection {
        // {port} in start_command → caller substitutes; no extra injection needed
        if self
            .config
            .start_command
            .as_deref()
            .map_or(false, |c| c.contains("{port}"))
        {
            return PortInjection::EnvOnly;
        }
        if let Some(ref arg) = self.config.port_arg {
            let mut args = vec![arg.clone(), port.to_string()];
            if let Some(ref host_arg) = self.config.host_arg {
                args.push(host_arg.clone());
                args.push("0.0.0.0".to_string());
            }
            return PortInjection::CliArgs(args);
        }
        if self.config.port_position.as_deref() == Some("append") {
            return PortInjection::AppendAddress(format!("0.0.0.0:{port}"));
        }
        PortInjection::EnvOnly
    }
}

// ─── Tests ───────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    struct AlwaysDriver;
    impl LanguageDriver for AlwaysDriver {
        fn detect(&self, _: &Path) -> bool {
            true
        }
        fn name(&self) -> &'static str {
            "always"
        }
        fn project_name(&self, _: &Path) -> Option<String> {
            None
        }
        fn start_command(&self, _: &Path) -> Option<String> {
            Some("echo hi".to_string())
        }
        fn port_injection(&self, _: &Path, port: u16) -> PortInjection {
            PortInjection::CliArgs(vec!["--port".to_string(), port.to_string()])
        }
    }

    struct NeverDriver;
    impl LanguageDriver for NeverDriver {
        fn detect(&self, _: &Path) -> bool {
            false
        }
        fn name(&self) -> &'static str {
            "never"
        }
        fn project_name(&self, _: &Path) -> Option<String> {
            None
        }
        fn start_command(&self, _: &Path) -> Option<String> {
            None
        }
        fn port_injection(&self, _: &Path, _: u16) -> PortInjection {
            PortInjection::EnvOnly
        }
    }

    #[test]
    fn registry_returns_none_when_no_match() {
        let reg = DriverRegistry {
            drivers: vec![Box::new(NeverDriver)],
        };
        let tmp = TempDir::new().unwrap();
        assert!(reg.detect(tmp.path()).is_none());
    }

    #[test]
    fn registry_returns_first_matching_driver() {
        let reg = DriverRegistry {
            drivers: vec![Box::new(NeverDriver), Box::new(AlwaysDriver)],
        };
        let tmp = TempDir::new().unwrap();
        assert_eq!(reg.detect(tmp.path()).unwrap().name(), "always");
    }

    #[test]
    fn registry_respects_priority_order() {
        struct PrioDriver(u8, &'static str);
        impl LanguageDriver for PrioDriver {
            fn detect(&self, _: &Path) -> bool {
                true
            }
            fn priority(&self) -> u8 {
                self.0
            }
            fn name(&self) -> &'static str {
                self.1
            }
            fn project_name(&self, _: &Path) -> Option<String> {
                None
            }
            fn start_command(&self, _: &Path) -> Option<String> {
                None
            }
            fn port_injection(&self, _: &Path, _: u16) -> PortInjection {
                PortInjection::EnvOnly
            }
        }
        let mut reg = DriverRegistry {
            drivers: vec![
                Box::new(PrioDriver(40, "low")),
                Box::new(PrioDriver(90, "high")),
            ],
        };
        reg.sort();
        let tmp = TempDir::new().unwrap();
        assert_eq!(reg.detect(tmp.path()).unwrap().name(), "high");
    }

    #[test]
    fn sanitize_hostname_works() {
        assert_eq!(sanitize_hostname("My App"), "my-app");
        assert_eq!(sanitize_hostname("feature/login"), "feature-login");
        assert_eq!(sanitize_hostname("api_v2"), "api-v2");
        assert_eq!(sanitize_hostname("UPPERCASE"), "uppercase");
    }

    #[test]
    fn portal_toml_driver_detects_when_start_command_set() {
        use crate::config::ProjectConfig;
        let cfg = ProjectConfig {
            name: None,
            start_command: Some("uvicorn main:app".to_string()),
            port_arg: None,
            host_arg: None,
            port_position: None,
            port_env: None,
            proxy: None,
            script: None,
        };
        let driver = PortalTomlDriver { config: cfg };
        let tmp = TempDir::new().unwrap();
        assert!(driver.detect(tmp.path()));
    }

    #[test]
    fn portal_toml_driver_does_not_detect_when_only_name_set() {
        use crate::config::ProjectConfig;
        let cfg = ProjectConfig {
            name: Some("myapp".to_string()),
            start_command: None,
            port_arg: None,
            host_arg: None,
            port_position: None,
            port_env: None,
            proxy: None,
            script: None,
        };
        let driver = PortalTomlDriver { config: cfg };
        let tmp = TempDir::new().unwrap();
        assert!(!driver.detect(tmp.path()));
    }

    #[test]
    fn portal_toml_driver_port_arg_injection() {
        use crate::config::ProjectConfig;
        let cfg = ProjectConfig {
            name: None,
            start_command: Some("uvicorn main:app".to_string()),
            port_arg: Some("--port".to_string()),
            host_arg: Some("--host".to_string()),
            port_position: None,
            port_env: None,
            proxy: None,
            script: None,
        };
        let driver = PortalTomlDriver { config: cfg };
        let tmp = TempDir::new().unwrap();
        let inj = driver.port_injection(tmp.path(), 4123);
        match inj {
            PortInjection::CliArgs(args) => {
                assert!(args.contains(&"--port".to_string()));
                assert!(args.contains(&"4123".to_string()));
                assert!(args.contains(&"--host".to_string()));
                assert!(args.contains(&"0.0.0.0".to_string()));
            }
            _ => panic!("expected CliArgs"),
        }
    }

    #[test]
    fn registry_new_detects_django() {
        let tmp = TempDir::new().unwrap();
        std::fs::write(tmp.path().join("manage.py"), "").unwrap();
        let cfg = crate::config::Config::default();
        let reg = DriverRegistry::new(&cfg);
        assert_eq!(reg.detect(tmp.path()).unwrap().name(), "Django (Python)");
    }

    #[test]
    fn registry_portal_toml_beats_django() {
        let tmp = TempDir::new().unwrap();
        std::fs::write(tmp.path().join("manage.py"), "").unwrap();
        let mut cfg = crate::config::Config::default();
        cfg.project.start_command = Some("my-custom-server".to_string());
        let reg = DriverRegistry::new(&cfg);
        assert_eq!(reg.detect(tmp.path()).unwrap().name(), "portal.toml");
    }

    #[test]
    fn registry_detect_language_skips_portal_toml() {
        let tmp = TempDir::new().unwrap();
        std::fs::write(tmp.path().join("manage.py"), "").unwrap();
        let mut cfg = crate::config::Config::default();
        cfg.project.start_command = Some("my-custom-server".to_string());
        let reg = DriverRegistry::new(&cfg);
        assert_eq!(
            reg.detect_language(tmp.path()).unwrap().name(),
            "Django (Python)"
        );
    }

    #[test]
    fn registry_returns_none_for_unknown_project() {
        let tmp = TempDir::new().unwrap();
        let cfg = crate::config::Config::default();
        let reg = DriverRegistry::new(&cfg);
        assert!(reg.detect(tmp.path()).is_none());
    }

    #[test]
    fn registry_storybook_beats_node_for_storybook_project() {
        let tmp = TempDir::new().unwrap();
        fs::write(
            tmp.path().join("package.json"),
            r#"{"name":"ui","scripts":{"dev":"vite","storybook":"storybook dev"}}"#,
        )
        .unwrap();
        fs::create_dir(tmp.path().join(".storybook")).unwrap();
        let cfg = crate::config::Config::default();
        let reg = DriverRegistry::new(&cfg);
        assert_eq!(reg.detect(tmp.path()).unwrap().name(), "Storybook");
    }

    #[test]
    fn registry_storybook_project_name_has_storybook_suffix() {
        let tmp = TempDir::new().unwrap();
        fs::write(tmp.path().join("package.json"), r#"{"name":"my-ui"}"#).unwrap();
        fs::create_dir(tmp.path().join(".storybook")).unwrap();
        let cfg = crate::config::Config::default();
        let reg = DriverRegistry::new(&cfg);
        let driver = reg.detect(tmp.path()).unwrap();
        assert_eq!(
            driver.project_name(tmp.path()),
            Some("my-ui-storybook".to_string())
        );
    }

    #[test]
    fn portal_toml_driver_port_in_command_returns_env_only() {
        use crate::config::ProjectConfig;
        let cfg = ProjectConfig {
            name: None,
            start_command: Some("php -S 0.0.0.0:{port}".to_string()),
            port_arg: None,
            host_arg: None,
            port_position: None,
            port_env: None,
            proxy: None,
            script: None,
        };
        let driver = PortalTomlDriver { config: cfg };
        let tmp = TempDir::new().unwrap();
        assert!(matches!(
            driver.port_injection(tmp.path(), 4123),
            PortInjection::EnvOnly
        ));
    }

    #[test]
    fn registry_docker_compose_beats_node_for_compose_project() {
        let tmp = TempDir::new().unwrap();
        fs::write(tmp.path().join("package.json"), r#"{"name":"app"}"#).unwrap();
        fs::write(tmp.path().join("docker-compose.yml"), "services: {}").unwrap();
        let cfg = crate::config::Config::default();
        let reg = DriverRegistry::new(&cfg);
        assert_eq!(reg.detect(tmp.path()).unwrap().name(), "Docker Compose");
    }

    #[test]
    fn registry_docker_compose_service_port_candidates_via_trait() {
        let tmp = TempDir::new().unwrap();
        fs::write(
            tmp.path().join("docker-compose.yml"),
            r#"
services:
  web:
    ports:
      - "3000:80"
"#,
        )
        .unwrap();
        let cfg = crate::config::Config::default();
        let reg = DriverRegistry::new(&cfg);
        let driver = reg.detect(tmp.path()).unwrap();
        let candidates = driver.service_port_candidates(tmp.path());
        assert_eq!(candidates, vec![("web".to_string(), 3000u16)]);
    }
}
