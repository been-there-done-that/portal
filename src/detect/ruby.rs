use std::path::Path;
use crate::detect::{LanguageDriver, PortInjection, file_contains};

pub struct RailsDriver;

impl LanguageDriver for RailsDriver {
    fn detect(&self, cwd: &Path) -> bool {
        file_contains(cwd, "Gemfile", "rails")
    }
    fn priority(&self) -> u8 { 70 }
    fn name(&self) -> &'static str { "Rails (Ruby)" }
    fn project_name(&self, cwd: &Path) -> Option<String> {
        cwd.file_name()?.to_str().map(crate::detect::sanitize_hostname)
    }
    fn start_command(&self, _cwd: &Path) -> Option<String> {
        Some("rails server".to_string())
    }
    fn port_injection(&self, _cwd: &Path, port: u16) -> PortInjection {
        PortInjection::CliArgs(vec![
            "-p".to_string(), port.to_string(),
            "-b".to_string(), "0.0.0.0".to_string(),
        ])
    }
}

pub struct RackDriver;

impl LanguageDriver for RackDriver {
    fn detect(&self, cwd: &Path) -> bool {
        cwd.join("Gemfile").exists() && !file_contains(cwd, "Gemfile", "rails")
    }
    fn priority(&self) -> u8 { 70 }
    fn name(&self) -> &'static str { "Rack (Ruby)" }
    fn project_name(&self, cwd: &Path) -> Option<String> {
        cwd.file_name()?.to_str().map(crate::detect::sanitize_hostname)
    }
    fn start_command(&self, _cwd: &Path) -> Option<String> {
        Some("bundle exec rackup".to_string())
    }
    fn port_injection(&self, _cwd: &Path, port: u16) -> PortInjection {
        PortInjection::CliArgs(vec![
            "-p".to_string(), port.to_string(),
            "-o".to_string(), "0.0.0.0".to_string(),
        ])
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;
    use std::fs;

    #[test]
    fn rails_detects_gemfile_with_rails() {
        let tmp = TempDir::new().unwrap();
        fs::write(tmp.path().join("Gemfile"), "gem 'rails', '~> 7.0'\n").unwrap();
        assert!(RailsDriver.detect(tmp.path()));
    }

    #[test]
    fn rails_does_not_detect_without_rails_gem() {
        let tmp = TempDir::new().unwrap();
        fs::write(tmp.path().join("Gemfile"), "gem 'sinatra'\n").unwrap();
        assert!(!RailsDriver.detect(tmp.path()));
    }

    #[test]
    fn rails_cli_args_injection() {
        let tmp = TempDir::new().unwrap();
        let inj = RailsDriver.port_injection(tmp.path(), 4123);
        match inj {
            crate::detect::PortInjection::CliArgs(args) => {
                assert!(args.contains(&"-p".to_string()));
                assert!(args.contains(&"4123".to_string()));
                assert!(args.contains(&"-b".to_string()));
                assert!(args.contains(&"0.0.0.0".to_string()));
            }
            _ => panic!("expected CliArgs"),
        }
    }

    #[test]
    fn rack_detects_gemfile_without_rails() {
        let tmp = TempDir::new().unwrap();
        fs::write(tmp.path().join("Gemfile"), "gem 'sinatra'\n").unwrap();
        assert!(RackDriver.detect(tmp.path()));
    }

    #[test]
    fn rack_does_not_detect_without_gemfile() {
        let tmp = TempDir::new().unwrap();
        assert!(!RackDriver.detect(tmp.path()));
    }

    #[test]
    fn rack_cli_args_injection() {
        let tmp = TempDir::new().unwrap();
        let inj = RackDriver.port_injection(tmp.path(), 4123);
        match inj {
            crate::detect::PortInjection::CliArgs(args) => {
                assert!(args.contains(&"-p".to_string()));
                assert!(args.contains(&"4123".to_string()));
            }
            _ => panic!("expected CliArgs"),
        }
    }
}
