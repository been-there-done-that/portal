use std::path::Path;
use crate::detect::{LanguageDriver, PortInjection, read_json_field};

pub struct PhpDriver;

impl LanguageDriver for PhpDriver {
    fn detect(&self, cwd: &Path) -> bool {
        cwd.join("index.php").exists() || cwd.join("composer.json").exists()
    }
    fn priority(&self) -> u8 { 60 }
    fn name(&self) -> &'static str { "PHP" }
    fn project_name(&self, cwd: &Path) -> Option<String> {
        // composer.json "name" is "vendor/package" — take the last segment
        read_json_field(cwd, "composer.json", "name").map(|n| {
            n.split('/').last().unwrap_or(&n).to_string()
        })
    }
    fn start_command(&self, _cwd: &Path) -> Option<String> {
        // {port} is substituted by the caller before spawning
        Some("php -S 0.0.0.0:{port}".to_string())
    }
    fn port_injection(&self, _cwd: &Path, _port: u16) -> PortInjection {
        // Port is already in start_command via {port} placeholder
        PortInjection::EnvOnly
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;
    use std::fs;

    #[test]
    fn php_detects_index_php() {
        let tmp = TempDir::new().unwrap();
        fs::write(tmp.path().join("index.php"), "<?php echo 'hi';").unwrap();
        assert!(PhpDriver.detect(tmp.path()));
    }

    #[test]
    fn php_detects_composer_json() {
        let tmp = TempDir::new().unwrap();
        fs::write(tmp.path().join("composer.json"), r#"{"name":"vendor/myapp"}"#).unwrap();
        assert!(PhpDriver.detect(tmp.path()));
    }

    #[test]
    fn php_does_not_detect_without_markers() {
        let tmp = TempDir::new().unwrap();
        assert!(!PhpDriver.detect(tmp.path()));
    }

    #[test]
    fn php_project_name_from_composer_json() {
        let tmp = TempDir::new().unwrap();
        fs::write(tmp.path().join("composer.json"), r#"{"name":"vendor/myapp"}"#).unwrap();
        assert_eq!(PhpDriver.project_name(tmp.path()), Some("myapp".to_string()));
    }

    #[test]
    fn php_start_command_contains_port_placeholder() {
        let tmp = TempDir::new().unwrap();
        let cmd = PhpDriver.start_command(tmp.path()).unwrap();
        assert!(cmd.contains("{port}"), "expected {{port}} in '{cmd}'");
    }

    #[test]
    fn php_uses_env_only_injection() {
        let tmp = TempDir::new().unwrap();
        assert!(matches!(PhpDriver.port_injection(tmp.path(), 4123), crate::detect::PortInjection::EnvOnly));
    }
}
