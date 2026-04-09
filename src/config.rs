use crate::error::Result;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

/// Proxy configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProxyConfig {
    pub tld: String,
    pub port_range: (u16, u16),
    pub https: bool,
    pub http_port: u16,
    pub https_port: u16,
}

impl Default for ProxyConfig {
    fn default() -> Self {
        Self {
            tld: "localhost".to_string(),
            port_range: (4000, 4999),
            https: true,
            http_port: 80,
            https_port: 443,
        }
    }
}

/// Daemon configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DaemonConfig {
    pub log_level: String,
    pub auto_start: bool,
}

impl Default for DaemonConfig {
    fn default() -> Self {
        Self {
            log_level: "info".to_string(),
            auto_start: true,
        }
    }
}

/// Project configuration
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ProjectConfig {
    pub name: Option<String>,
    pub start_command: Option<String>,
    pub port_arg: Option<String>,
    pub host_arg: Option<String>,
    /// "append" → appends "0.0.0.0:{port}" as a positional arg
    pub port_position: Option<String>,
    /// Name of the env var to use for passing the port (e.g. "APP_PORT")
    pub port_env: Option<String>,
}

/// Complete configuration
#[derive(Debug, Clone, Default)]
pub struct Config {
    pub proxy: ProxyConfig,
    pub daemon: DaemonConfig,
    pub project: ProjectConfig,
}

/// Partial config for deserialization from TOML files
#[derive(Debug, Serialize, Deserialize, Default)]
struct PartialConfig {
    #[serde(default)]
    proxy: PartialProxyConfig,
    #[serde(default)]
    daemon: PartialDaemonConfig,
    #[serde(default)]
    project: PartialProjectConfig,
}

#[derive(Debug, Serialize, Deserialize, Default)]
struct PartialProxyConfig {
    tld: Option<String>,
    port_range: Option<(u16, u16)>,
    https: Option<bool>,
    http_port: Option<u16>,
    https_port: Option<u16>,
}

#[derive(Debug, Serialize, Deserialize, Default)]
struct PartialDaemonConfig {
    log_level: Option<String>,
    auto_start: Option<bool>,
}

#[derive(Debug, Serialize, Deserialize, Default)]
struct PartialProjectConfig {
    name: Option<String>,
    start_command: Option<String>,
    port_arg: Option<String>,
    host_arg: Option<String>,
    port_position: Option<String>,
    port_env: Option<String>,
}

impl Config {
    /// Load configuration from default paths (used at runtime)
    pub fn load(cwd: &Path) -> Result<Self> {
        let global_path = dirs::home_dir().map(|h| h.join(".portal/config.toml"));
        let project_path = find_project_toml(cwd);

        // Collect owned strings first, then borrow for the call
        let env_vars: Vec<(String, String)> = std::env::vars()
            .filter(|(k, _)| k.starts_with("PORTAL_"))
            .collect();
        let env_refs: Vec<(&str, &str)> = env_vars
            .iter()
            .map(|(k, v)| (k.as_str(), v.as_str()))
            .collect();
        Self::load_with_paths(global_path, project_path, &env_refs)
    }

    /// Load configuration with explicit paths (used in tests)
    pub fn load_with_paths(
        global_path: Option<PathBuf>,
        project_path: Option<PathBuf>,
        env_overrides: &[(&str, &str)],
    ) -> Result<Self> {
        let mut config = Config::default();

        // Layer 1: Load global config
        if let Some(path) = global_path {
            if path.exists() {
                let contents = std::fs::read_to_string(&path)?;
                let partial: PartialConfig = toml::from_str(&contents)?;
                apply_partial(&mut config, partial);
            }
        }

        // Layer 2: Load project config (overrides global)
        if let Some(path) = project_path {
            if path.exists() {
                let contents = std::fs::read_to_string(&path)?;
                let partial: PartialConfig = toml::from_str(&contents)?;
                apply_partial(&mut config, partial);
            }
        }

        // Layer 3: Apply env var overrides
        apply_env_overrides(&mut config, env_overrides)?;

        Ok(config)
    }
}

/// Apply a partial config to a full config (overrides defaults)
fn apply_partial(config: &mut Config, partial: PartialConfig) {
    if let Some(tld) = partial.proxy.tld {
        config.proxy.tld = tld;
    }
    if let Some(port_range) = partial.proxy.port_range {
        config.proxy.port_range = port_range;
    }
    if let Some(https) = partial.proxy.https {
        config.proxy.https = https;
    }
    if let Some(http_port) = partial.proxy.http_port {
        config.proxy.http_port = http_port;
    }
    if let Some(https_port) = partial.proxy.https_port {
        config.proxy.https_port = https_port;
    }

    if let Some(log_level) = partial.daemon.log_level {
        config.daemon.log_level = log_level;
    }
    if let Some(auto_start) = partial.daemon.auto_start {
        config.daemon.auto_start = auto_start;
    }

    if partial.project.name.is_some() {
        config.project.name = partial.project.name;
    }
    if partial.project.start_command.is_some() {
        config.project.start_command = partial.project.start_command;
    }
    if partial.project.port_arg.is_some() {
        config.project.port_arg = partial.project.port_arg;
    }
    if partial.project.host_arg.is_some() {
        config.project.host_arg = partial.project.host_arg;
    }
    if partial.project.port_position.is_some() {
        config.project.port_position = partial.project.port_position;
    }
    if partial.project.port_env.is_some() {
        config.project.port_env = partial.project.port_env;
    }
}

/// Apply environment variable overrides
fn apply_env_overrides(config: &mut Config, env_overrides: &[(&str, &str)]) -> Result<()> {
    for (key, value) in env_overrides {
        match *key {
            "PORTAL_TLD" => config.proxy.tld = value.to_string(),
            "PORTAL_HTTPS" => {
                config.proxy.https = matches!(
                    value.to_ascii_lowercase().as_str(),
                    "1" | "true" | "yes" | "on"
                );
            }
            "PORTAL_HTTP_PORT" => {
                config.proxy.http_port = value.parse()?;
            }
            "PORTAL_HTTPS_PORT" => {
                config.proxy.https_port = value.parse()?;
            }
            "PORTAL_PORT_ENV" => {
                config.project.port_env = Some(value.to_string());
            }
            _ => {
                // Ignore unknown env vars
            }
        }
    }
    Ok(())
}

/// Search upward from cwd for portal.toml
pub fn find_project_toml(cwd: &Path) -> Option<PathBuf> {
    let mut current = cwd;
    loop {
        let candidate = current.join("portal.toml");
        if candidate.exists() {
            return Some(candidate);
        }
        current = current.parent()?;
    }
}

/// Returns ~/.portal/ — or the invoking user's home when running under sudo.
///
/// When `sudo portless daemon` is used, sudo sets `SUDO_USER` to the original
/// username. We resolve that user's home so the daemon socket and state live
/// in the same place regardless of whether the process is root.
pub fn dirs_for_state() -> PathBuf {
    // If running under sudo, prefer the invoking user's home directory
    if let Ok(sudo_user) = std::env::var("SUDO_USER") {
        if !sudo_user.is_empty() && sudo_user != "root" {
            // Use `getpwnam` on Unix to look up the user's home
            #[cfg(unix)]
            {
                use std::ffi::CString;
                if let Ok(c_name) = CString::new(sudo_user) {
                    let pw = unsafe { nix::libc::getpwnam(c_name.as_ptr()) };
                    if !pw.is_null() {
                        let home_ptr = unsafe { (*pw).pw_dir };
                        if !home_ptr.is_null() {
                            let home = unsafe { std::ffi::CStr::from_ptr(home_ptr) };
                            if let Ok(s) = home.to_str() {
                                return PathBuf::from(s).join(".portal");
                            }
                        }
                    }
                }
            }
        }
    }
    dirs::home_dir()
        .map(|h| h.join(".portal"))
        .unwrap_or_else(|| PathBuf::from(".portal"))
}

/// Returns the UID/GID of the invoking user (before sudo elevation), if any.
/// Used to chown state files so the real user can access them.
#[cfg(unix)]
pub fn sudo_uid_gid() -> Option<(u32, u32)> {
    let uid: u32 = std::env::var("SUDO_UID").ok()?.parse().ok()?;
    let gid: u32 = std::env::var("SUDO_GID").ok()?.parse().ok()?;
    Some((uid, gid))
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn defaults_when_no_files() {
        let config = Config::load_with_paths(None, None, &[]).unwrap();

        assert_eq!(config.proxy.tld, "localhost");
        assert_eq!(config.proxy.port_range, (4000, 4999));
        assert_eq!(config.proxy.https, true);
        assert_eq!(config.proxy.http_port, 80);
        assert_eq!(config.proxy.https_port, 443);
        assert_eq!(config.daemon.auto_start, true);
        assert_eq!(config.daemon.log_level, "info");
        assert_eq!(config.project.name, None);
    }

    #[test]
    fn global_toml_overrides_defaults() {
        let temp = TempDir::new().unwrap();
        let global_path = temp.path().join("config.toml");

        // Write global config
        std::fs::write(
            &global_path,
            r#"
[proxy]
tld = "test"
https_port = 8443

[daemon]
log_level = "debug"
"#,
        )
        .unwrap();

        let config = Config::load_with_paths(Some(global_path), None, &[]).unwrap();

        assert_eq!(config.proxy.tld, "test");
        assert_eq!(config.proxy.https_port, 8443);
        // http_port should still be default 80
        assert_eq!(config.proxy.http_port, 80);
        // https should still be default true
        assert_eq!(config.proxy.https, true);
        assert_eq!(config.daemon.log_level, "debug");
    }

    #[test]
    fn project_toml_overrides_global() {
        let temp = TempDir::new().unwrap();
        let global_path = temp.path().join("global.toml");
        let project_path = temp.path().join("project.toml");

        // Write global config
        std::fs::write(
            &global_path,
            r#"
[proxy]
tld = "test"
"#,
        )
        .unwrap();

        // Write project config (overrides global)
        std::fs::write(
            &project_path,
            r#"
[proxy]
tld = "local"

[project]
name = "my-project"
"#,
        )
        .unwrap();

        let config = Config::load_with_paths(Some(global_path), Some(project_path), &[]).unwrap();

        assert_eq!(config.proxy.tld, "local");
        assert_eq!(config.project.name, Some("my-project".to_string()));
    }

    #[test]
    fn env_vars_override_toml() {
        let temp = TempDir::new().unwrap();
        let global_path = temp.path().join("config.toml");

        // Write global config
        std::fs::write(
            &global_path,
            r#"
[proxy]
tld = "test"
"#,
        )
        .unwrap();

        let env_overrides = [("PORTAL_TLD", "myenv")];
        let config = Config::load_with_paths(Some(global_path), None, &env_overrides).unwrap();

        assert_eq!(config.proxy.tld, "myenv");
    }

    #[test]
    fn env_vars_parse_correctly() {
        let env_overrides = [
            ("PORTAL_TLD", "custom.local"),
            ("PORTAL_HTTPS", "0"),
            ("PORTAL_HTTP_PORT", "8080"),
            ("PORTAL_HTTPS_PORT", "8443"),
        ];
        let config = Config::load_with_paths(None, None, &env_overrides).unwrap();

        assert_eq!(config.proxy.tld, "custom.local");
        assert_eq!(config.proxy.https, false);
        assert_eq!(config.proxy.http_port, 8080);
        assert_eq!(config.proxy.https_port, 8443);
    }

    #[test]
    fn find_project_toml_in_current_dir() {
        let temp = TempDir::new().unwrap();
        let toml_path = temp.path().join("portal.toml");
        std::fs::write(&toml_path, "").unwrap();

        let found = find_project_toml(temp.path());
        assert_eq!(found, Some(toml_path));
    }

    #[test]
    fn find_project_toml_upward() {
        let temp = TempDir::new().unwrap();
        let toml_path = temp.path().join("portal.toml");
        std::fs::write(&toml_path, "").unwrap();

        // Create a subdirectory
        let subdir = temp.path().join("src").join("lib");
        std::fs::create_dir_all(&subdir).unwrap();

        // Search from subdirectory should find it
        let found = find_project_toml(&subdir);
        assert_eq!(found, Some(toml_path));
    }

    #[test]
    fn find_project_toml_returns_none_when_not_found() {
        let temp = TempDir::new().unwrap();
        let found = find_project_toml(temp.path());
        assert_eq!(found, None);
    }

    #[test]
    fn invalid_port_env_var_returns_error() {
        let env_overrides = [("PORTAL_HTTP_PORT", "not_a_number")];
        let result = Config::load_with_paths(None, None, &env_overrides);
        assert!(result.is_err(), "expected error for invalid port value");
    }

    #[test]
    fn portal_port_env_var_sets_port_env() {
        let env_overrides = [("PORTAL_PORT_ENV", "APP_PORT")];
        let config = Config::load_with_paths(None, None, &env_overrides).unwrap();
        assert_eq!(config.project.port_env, Some("APP_PORT".to_string()));
    }

    #[test]
    fn port_env_can_be_overridden_via_toml() {
        let temp = TempDir::new().unwrap();
        let project_path = temp.path().join("portal.toml");
        std::fs::write(
            &project_path,
            r#"
[project]
port_env = "APP_PORT"
"#,
        )
        .unwrap();
        let config = Config::load_with_paths(None, Some(project_path), &[]).unwrap();
        assert_eq!(config.project.port_env, Some("APP_PORT".to_string()));
    }

    #[test]
    fn port_env_defaults_to_port_when_unset() {
        let config = Config::load_with_paths(None, None, &[]).unwrap();
        // When None, caller should default to "PORT"
        assert_eq!(config.project.port_env.as_deref().unwrap_or("PORT"), "PORT");
    }
}
