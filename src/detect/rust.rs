use std::path::Path;
use crate::detect::{LanguageDriver, PortInjection, read_toml_field};

pub struct RustDriver;

impl LanguageDriver for RustDriver {
    fn detect(&self, cwd: &Path) -> bool {
        cwd.join("Cargo.toml").exists()
    }
    fn priority(&self) -> u8 { 50 }
    fn name(&self) -> &'static str { "Rust" }
    fn project_name(&self, cwd: &Path) -> Option<String> {
        read_toml_field(cwd, "Cargo.toml", &["package", "name"])
    }
    fn start_command(&self, _cwd: &Path) -> Option<String> {
        Some("cargo run".to_string())
    }
    fn port_injection(&self, _cwd: &Path, _port: u16) -> PortInjection {
        PortInjection::EnvOnly
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;
    use std::fs;

    #[test]
    fn rust_detects_cargo_toml() {
        let tmp = TempDir::new().unwrap();
        fs::write(tmp.path().join("Cargo.toml"),
            "[package]\nname = \"myservice\"\nversion = \"0.1.0\"\n").unwrap();
        assert!(RustDriver.detect(tmp.path()));
    }

    #[test]
    fn rust_does_not_detect_without_cargo_toml() {
        let tmp = TempDir::new().unwrap();
        assert!(!RustDriver.detect(tmp.path()));
    }

    #[test]
    fn rust_project_name_from_cargo_toml() {
        let tmp = TempDir::new().unwrap();
        fs::write(tmp.path().join("Cargo.toml"),
            "[package]\nname = \"myservice\"\n").unwrap();
        assert_eq!(RustDriver.project_name(tmp.path()), Some("myservice".to_string()));
    }

    #[test]
    fn rust_uses_env_only_injection() {
        let tmp = TempDir::new().unwrap();
        assert!(matches!(RustDriver.port_injection(tmp.path(), 4123), crate::detect::PortInjection::EnvOnly));
    }
}
