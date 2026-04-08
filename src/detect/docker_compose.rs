use std::fs;
use std::path::Path;
use crate::detect::{LanguageDriver, PortInjection};

pub struct DockerComposeDriver;

/// Compose file names to check, in priority order.
const COMPOSE_FILES: &[&str] = &[
    "docker-compose.yml",
    "docker-compose.yaml",
    "compose.yml",
    "compose.yaml",
];

/// Parse the host (published) port from a single entry in a compose `ports:` list.
///
/// Handles three short-syntax formats:
///   - `"3000:80"`              → 3000  (host:container)
///   - `"127.0.0.1:3000:80"`   → 3000  (ip:host:container)
///   - `"3000"`                 → 3000  (bare port)
///   - `3000`  (YAML integer)   → 3000
///
/// Also handles long syntax: `{published: 3000, target: 80}`.
fn parse_host_port(val: &serde_yaml::Value) -> Option<u16> {
    if let Some(s) = val.as_str() {
        let parts: Vec<&str> = s.split(':').collect();
        return match parts.len() {
            1 => parts[0].trim().parse().ok(),
            2 => parts[0].trim().parse().ok(),   // "host:container"
            3 => parts[1].trim().parse().ok(),   // "ip:host:container"
            _ => None,
        };
    }
    if let Some(n) = val.as_u64() {
        return u16::try_from(n).ok();
    }
    // Long syntax: {published: 3000, target: 80}
    if let Some(map) = val.as_mapping() {
        for (k, v) in map {
            if k.as_str() == Some("published") {
                if let Some(n) = v.as_u64() {
                    return u16::try_from(n).ok();
                }
                if let Some(s) = v.as_str() {
                    return s.trim().parse().ok();
                }
            }
        }
    }
    None
}

/// Read and parse the first compose file found in `cwd`.
fn read_compose_yaml(cwd: &Path) -> Option<serde_yaml::Value> {
    for name in COMPOSE_FILES {
        if let Ok(contents) = fs::read_to_string(cwd.join(name)) {
            if let Ok(yaml) = serde_yaml::from_str::<serde_yaml::Value>(&contents) {
                return Some(yaml);
            }
        }
    }
    None
}

impl LanguageDriver for DockerComposeDriver {
    fn detect(&self, cwd: &Path) -> bool {
        COMPOSE_FILES.iter().any(|name| cwd.join(name).is_file())
    }

    fn priority(&self) -> u8 { 55 }

    fn name(&self) -> &'static str { "Docker Compose" }

    fn project_name(&self, cwd: &Path) -> Option<String> {
        // Prefer the top-level `name:` field in the compose file
        if let Some(yaml) = read_compose_yaml(cwd) {
            if let Some(name) = yaml.get("name").and_then(|v| v.as_str()) {
                if !name.is_empty() {
                    return Some(crate::detect::sanitize_hostname(name));
                }
            }
        }
        // Fallback: directory name
        cwd.file_name()
            .and_then(|n| n.to_str())
            .map(crate::detect::sanitize_hostname)
    }

    fn start_command(&self, _cwd: &Path) -> Option<String> {
        Some("docker compose up".to_string())
    }

    /// No port injection — Docker Compose manages its own port bindings.
    fn port_injection(&self, _cwd: &Path, _port: u16) -> PortInjection {
        PortInjection::EnvOnly
    }

    fn service_port_candidates(&self, cwd: &Path) -> Vec<(String, u16)> {
        let yaml = match read_compose_yaml(cwd) {
            Some(y) => y,
            None => return vec![],
        };
        let services = match yaml.get("services").and_then(|v| v.as_mapping()) {
            Some(s) => s,
            None => return vec![],
        };
        let mut candidates = Vec::new();
        for (key, service) in services {
            let name = match key.as_str() {
                Some(s) => s.to_string(),
                None => continue,
            };
            if let Some(ports) = service.get("ports").and_then(|v| v.as_sequence()) {
                if let Some(first) = ports.first() {
                    if let Some(port) = parse_host_port(first) {
                        candidates.push((name, port));
                    }
                }
            }
        }
        candidates
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    // ── detection ────────────────────────────────────────────────────────────

    #[test]
    fn detects_docker_compose_yml() {
        let tmp = TempDir::new().unwrap();
        fs::write(tmp.path().join("docker-compose.yml"), "services: {}").unwrap();
        assert!(DockerComposeDriver.detect(tmp.path()));
    }

    #[test]
    fn detects_docker_compose_yaml() {
        let tmp = TempDir::new().unwrap();
        fs::write(tmp.path().join("docker-compose.yaml"), "services: {}").unwrap();
        assert!(DockerComposeDriver.detect(tmp.path()));
    }

    #[test]
    fn detects_compose_yml() {
        let tmp = TempDir::new().unwrap();
        fs::write(tmp.path().join("compose.yml"), "services: {}").unwrap();
        assert!(DockerComposeDriver.detect(tmp.path()));
    }

    #[test]
    fn detects_compose_yaml() {
        let tmp = TempDir::new().unwrap();
        fs::write(tmp.path().join("compose.yaml"), "services: {}").unwrap();
        assert!(DockerComposeDriver.detect(tmp.path()));
    }

    #[test]
    fn does_not_detect_empty_directory() {
        let tmp = TempDir::new().unwrap();
        assert!(!DockerComposeDriver.detect(tmp.path()));
    }

    // ── start_command ─────────────────────────────────────────────────────────

    #[test]
    fn start_command_is_docker_compose_up() {
        let tmp = TempDir::new().unwrap();
        assert_eq!(
            DockerComposeDriver.start_command(tmp.path()),
            Some("docker compose up".to_string()),
        );
    }

    // ── port_injection ────────────────────────────────────────────────────────

    #[test]
    fn port_injection_is_env_only() {
        let tmp = TempDir::new().unwrap();
        assert!(matches!(
            DockerComposeDriver.port_injection(tmp.path(), 3000),
            PortInjection::EnvOnly,
        ));
    }

    // ── service_port_candidates ───────────────────────────────────────────────

    #[test]
    fn service_port_candidates_single_service() {
        let tmp = TempDir::new().unwrap();
        fs::write(tmp.path().join("docker-compose.yml"), r#"
services:
  web:
    ports:
      - "3000:80"
"#).unwrap();
        let candidates = DockerComposeDriver.service_port_candidates(tmp.path());
        assert_eq!(candidates, vec![("web".to_string(), 3000u16)]);
    }

    #[test]
    fn service_port_candidates_multiple_services() {
        let tmp = TempDir::new().unwrap();
        fs::write(tmp.path().join("docker-compose.yml"), r#"
services:
  web:
    ports:
      - "3000:80"
  api:
    ports:
      - "8080:8080"
  db:
    image: postgres
"#).unwrap();
        let candidates = DockerComposeDriver.service_port_candidates(tmp.path());
        assert_eq!(candidates.len(), 2);
        assert!(candidates.iter().any(|(n, p)| n == "web" && *p == 3000));
        assert!(candidates.iter().any(|(n, p)| n == "api" && *p == 8080));
    }

    #[test]
    fn service_port_candidates_skips_services_without_ports() {
        let tmp = TempDir::new().unwrap();
        fs::write(tmp.path().join("docker-compose.yml"), r#"
services:
  db:
    image: postgres
  redis:
    image: redis
"#).unwrap();
        assert!(DockerComposeDriver.service_port_candidates(tmp.path()).is_empty());
    }

    #[test]
    fn service_port_candidates_parses_host_colon_container() {
        let tmp = TempDir::new().unwrap();
        fs::write(tmp.path().join("compose.yml"), r#"
services:
  web:
    ports:
      - "3000:80"
"#).unwrap();
        assert_eq!(DockerComposeDriver.service_port_candidates(tmp.path())[0].1, 3000);
    }

    #[test]
    fn service_port_candidates_parses_bare_port() {
        let tmp = TempDir::new().unwrap();
        fs::write(tmp.path().join("compose.yml"), r#"
services:
  web:
    ports:
      - "3000"
"#).unwrap();
        assert_eq!(DockerComposeDriver.service_port_candidates(tmp.path())[0].1, 3000);
    }

    #[test]
    fn service_port_candidates_parses_ip_host_container() {
        let tmp = TempDir::new().unwrap();
        fs::write(tmp.path().join("compose.yml"), r#"
services:
  web:
    ports:
      - "127.0.0.1:3000:80"
"#).unwrap();
        assert_eq!(DockerComposeDriver.service_port_candidates(tmp.path())[0].1, 3000);
    }

    #[test]
    fn service_port_candidates_parses_integer_port() {
        let tmp = TempDir::new().unwrap();
        fs::write(tmp.path().join("compose.yml"), r#"
services:
  web:
    ports:
      - 3000
"#).unwrap();
        assert_eq!(DockerComposeDriver.service_port_candidates(tmp.path())[0].1, 3000);
    }

    #[test]
    fn service_port_candidates_parses_long_syntax() {
        let tmp = TempDir::new().unwrap();
        fs::write(tmp.path().join("compose.yml"), r#"
services:
  web:
    ports:
      - published: 3000
        target: 80
"#).unwrap();
        assert_eq!(DockerComposeDriver.service_port_candidates(tmp.path())[0].1, 3000);
    }

    // ── project_name ──────────────────────────────────────────────────────────

    #[test]
    fn project_name_reads_compose_name_field() {
        let tmp = TempDir::new().unwrap();
        fs::write(tmp.path().join("docker-compose.yml"), r#"
name: my-app
services: {}
"#).unwrap();
        assert_eq!(DockerComposeDriver.project_name(tmp.path()), Some("my-app".to_string()));
    }

    #[test]
    fn project_name_falls_back_to_directory_name() {
        let tmp = TempDir::new().unwrap();
        fs::write(tmp.path().join("docker-compose.yml"), "services: {}").unwrap();
        let name = DockerComposeDriver.project_name(tmp.path()).unwrap();
        assert!(!name.is_empty(), "expected non-empty fallback name, got empty string");
    }

    // ── priority ──────────────────────────────────────────────────────────────

    #[test]
    fn priority_beats_node_driver() {
        assert!(DockerComposeDriver.priority() > crate::detect::node::NodeDriver.priority());
    }
}
