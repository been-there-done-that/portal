use crate::detect::{LanguageDriver, PortInjection};
use std::{fs, path::Path};

pub struct GoDriver;

impl LanguageDriver for GoDriver {
    fn detect(&self, cwd: &Path) -> bool {
        cwd.join("go.mod").exists()
    }
    fn priority(&self) -> u8 {
        50
    }
    fn name(&self) -> &'static str {
        "Go"
    }
    fn project_name(&self, cwd: &Path) -> Option<String> {
        let contents = fs::read_to_string(cwd.join("go.mod")).ok()?;
        let first = contents.lines().next()?;
        let module = first.strip_prefix("module ")?.trim();
        Some(module.split('/').last()?.to_string())
    }
    fn start_command(&self, _cwd: &Path) -> Option<String> {
        Some("go run .".to_string())
    }
    fn port_injection(&self, _cwd: &Path, _port: u16) -> PortInjection {
        PortInjection::EnvOnly
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    #[test]
    fn go_detects_go_mod() {
        let tmp = TempDir::new().unwrap();
        fs::write(
            tmp.path().join("go.mod"),
            "module github.com/user/myservice\n\ngo 1.21\n",
        )
        .unwrap();
        assert!(GoDriver.detect(tmp.path()));
    }

    #[test]
    fn go_does_not_detect_without_go_mod() {
        let tmp = TempDir::new().unwrap();
        assert!(!GoDriver.detect(tmp.path()));
    }

    #[test]
    fn go_project_name_from_module() {
        let tmp = TempDir::new().unwrap();
        fs::write(
            tmp.path().join("go.mod"),
            "module github.com/user/myservice\n",
        )
        .unwrap();
        assert_eq!(
            GoDriver.project_name(tmp.path()),
            Some("myservice".to_string())
        );
    }

    #[test]
    fn go_start_command() {
        let tmp = TempDir::new().unwrap();
        assert_eq!(
            GoDriver.start_command(tmp.path()),
            Some("go run .".to_string())
        );
    }

    #[test]
    fn go_uses_env_only_injection() {
        let tmp = TempDir::new().unwrap();
        assert!(matches!(
            GoDriver.port_injection(tmp.path(), 4123),
            crate::detect::PortInjection::EnvOnly
        ));
    }
}
