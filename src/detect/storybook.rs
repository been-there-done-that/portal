use crate::detect::node::detect_package_manager;
use crate::detect::{LanguageDriver, PortInjection};
use std::fs;
use std::path::Path;

pub struct StorybookDriver;

impl LanguageDriver for StorybookDriver {
    fn detect(&self, cwd: &Path) -> bool {
        // Signal 1: .storybook/ directory exists
        if cwd.join(".storybook").is_dir() {
            return true;
        }
        let Ok(contents) = fs::read_to_string(cwd.join("package.json")) else {
            return false;
        };
        let Ok(json) = serde_json::from_str::<serde_json::Value>(&contents) else {
            return false;
        };
        // Signal 2: scripts contain "storybook" or "start-storybook"
        if let Some(scripts) = json.get("scripts").and_then(|v| v.as_object()) {
            if scripts.contains_key("storybook") || scripts.contains_key("start-storybook") {
                return true;
            }
        }
        // Signal 3: dependencies or devDependencies contain any @storybook/ package
        for deps_key in &["devDependencies", "dependencies"] {
            if let Some(deps) = json.get(deps_key).and_then(|v| v.as_object()) {
                if deps.keys().any(|k| k.starts_with("@storybook/")) {
                    return true;
                }
            }
        }
        false
    }

    fn priority(&self) -> u8 {
        45
    }

    fn name(&self) -> &'static str {
        "Storybook"
    }

    fn project_name(&self, cwd: &Path) -> Option<String> {
        let base = crate::detect::read_json_field(cwd, "package.json", "name")
            .or_else(|| cwd.file_name().and_then(|n| n.to_str()).map(String::from))?;
        Some(format!(
            "{}-storybook",
            crate::detect::sanitize_hostname(&base)
        ))
    }

    fn start_command(&self, cwd: &Path) -> Option<String> {
        let pm = detect_package_manager(cwd);
        if let Ok(contents) = fs::read_to_string(cwd.join("package.json")) {
            if let Ok(json) = serde_json::from_str::<serde_json::Value>(&contents) {
                if let Some(scripts) = json.get("scripts").and_then(|v| v.as_object()) {
                    if scripts.contains_key("storybook") {
                        return Some(format!("{pm} run storybook"));
                    }
                    if scripts.contains_key("start-storybook") {
                        return Some(format!("{pm} run start-storybook"));
                    }
                }
            }
        }
        None
    }

    fn port_injection(&self, _cwd: &Path, port: u16) -> PortInjection {
        PortInjection::CliArgs(vec!["--port".to_string(), port.to_string()])
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn detects_via_storybook_directory() {
        let tmp = TempDir::new().unwrap();
        fs::create_dir(tmp.path().join(".storybook")).unwrap();
        assert!(StorybookDriver.detect(tmp.path()));
    }

    #[test]
    fn detects_via_storybook_script_in_package_json() {
        let tmp = TempDir::new().unwrap();
        fs::write(
            tmp.path().join("package.json"),
            r#"{"scripts":{"storybook":"storybook dev"}}"#,
        )
        .unwrap();
        assert!(StorybookDriver.detect(tmp.path()));
    }

    #[test]
    fn detects_via_start_storybook_script() {
        let tmp = TempDir::new().unwrap();
        fs::write(
            tmp.path().join("package.json"),
            r#"{"scripts":{"start-storybook":"start-storybook -p 6006"}}"#,
        )
        .unwrap();
        assert!(StorybookDriver.detect(tmp.path()));
    }

    #[test]
    fn detects_via_storybook_dev_dependency() {
        let tmp = TempDir::new().unwrap();
        fs::write(
            tmp.path().join("package.json"),
            r#"{"devDependencies":{"@storybook/react":"^7.0.0"}}"#,
        )
        .unwrap();
        assert!(StorybookDriver.detect(tmp.path()));
    }

    #[test]
    fn detects_via_storybook_regular_dependency() {
        let tmp = TempDir::new().unwrap();
        fs::write(
            tmp.path().join("package.json"),
            r#"{"dependencies":{"@storybook/react":"^6.5.0"}}"#,
        )
        .unwrap();
        assert!(StorybookDriver.detect(tmp.path()));
    }

    #[test]
    fn does_not_detect_plain_node_project() {
        let tmp = TempDir::new().unwrap();
        fs::write(
            tmp.path().join("package.json"),
            r#"{"name":"myapp","scripts":{"dev":"vite"}}"#,
        )
        .unwrap();
        assert!(!StorybookDriver.detect(tmp.path()));
    }

    #[test]
    fn does_not_detect_empty_directory() {
        let tmp = TempDir::new().unwrap();
        assert!(!StorybookDriver.detect(tmp.path()));
    }

    #[test]
    fn project_name_appends_storybook_suffix() {
        let tmp = TempDir::new().unwrap();
        fs::write(tmp.path().join("package.json"), r#"{"name":"my-app"}"#).unwrap();
        assert_eq!(
            StorybookDriver.project_name(tmp.path()),
            Some("my-app-storybook".to_string()),
        );
    }

    #[test]
    fn project_name_falls_back_to_directory_name() {
        let tmp = TempDir::new().unwrap();
        let name = StorybookDriver.project_name(tmp.path()).unwrap();
        assert!(
            name.ends_with("-storybook"),
            "expected -storybook suffix, got: {name}"
        );
    }

    #[test]
    fn start_command_uses_storybook_script_with_npm() {
        let tmp = TempDir::new().unwrap();
        fs::write(
            tmp.path().join("package.json"),
            r#"{"scripts":{"storybook":"storybook dev"}}"#,
        )
        .unwrap();
        assert_eq!(
            StorybookDriver.start_command(tmp.path()),
            Some("npm run storybook".to_string()),
        );
    }

    #[test]
    fn start_command_uses_start_storybook_script() {
        let tmp = TempDir::new().unwrap();
        fs::write(
            tmp.path().join("package.json"),
            r#"{"scripts":{"start-storybook":"start-storybook -p 6006"}}"#,
        )
        .unwrap();
        assert_eq!(
            StorybookDriver.start_command(tmp.path()),
            Some("npm run start-storybook".to_string()),
        );
    }

    #[test]
    fn start_command_prefers_storybook_over_start_storybook() {
        let tmp = TempDir::new().unwrap();
        fs::write(
            tmp.path().join("package.json"),
            r#"{"scripts":{"storybook":"storybook dev","start-storybook":"old"}}"#,
        )
        .unwrap();
        assert_eq!(
            StorybookDriver.start_command(tmp.path()),
            Some("npm run storybook".to_string()),
        );
    }

    #[test]
    fn start_command_returns_none_when_no_script() {
        let tmp = TempDir::new().unwrap();
        assert_eq!(StorybookDriver.start_command(tmp.path()), None);
    }

    #[test]
    fn start_command_respects_pnpm_lockfile() {
        let tmp = TempDir::new().unwrap();
        fs::write(tmp.path().join("pnpm-lock.yaml"), "").unwrap();
        fs::write(
            tmp.path().join("package.json"),
            r#"{"scripts":{"storybook":"storybook dev"}}"#,
        )
        .unwrap();
        assert_eq!(
            StorybookDriver.start_command(tmp.path()),
            Some("pnpm run storybook".to_string()),
        );
    }

    #[test]
    fn port_injection_uses_port_flag() {
        let tmp = TempDir::new().unwrap();
        match StorybookDriver.port_injection(tmp.path(), 6006) {
            PortInjection::CliArgs(args) => {
                assert_eq!(args, vec!["--port", "6006"]);
            }
            other => panic!("expected CliArgs, got {other:?}"),
        }
    }

    #[test]
    fn priority_beats_node_driver() {
        assert!(StorybookDriver.priority() > crate::detect::node::NodeDriver.priority());
    }
}
