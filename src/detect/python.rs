use std::path::Path;
use crate::detect::{LanguageDriver, PortInjection, read_toml_field, file_contains};

// ─── DjangoDriver ─────────────────────────────────────────────────────────────

pub struct DjangoDriver;

impl LanguageDriver for DjangoDriver {
    fn detect(&self, cwd: &Path) -> bool {
        cwd.join("manage.py").exists()
    }
    fn priority(&self) -> u8 { 90 }
    fn name(&self) -> &'static str { "Django (Python)" }
    fn project_name(&self, cwd: &Path) -> Option<String> {
        read_toml_field(cwd, "pyproject.toml", &["project", "name"])
    }
    fn start_command(&self, _cwd: &Path) -> Option<String> {
        Some("python manage.py runserver".to_string())
    }
    fn port_injection(&self, _cwd: &Path, port: u16) -> PortInjection {
        PortInjection::AppendAddress(format!("0.0.0.0:{port}"))
    }
}

// ─── UvicornDriver ────────────────────────────────────────────────────────────

pub struct UvicornDriver;

impl LanguageDriver for UvicornDriver {
    fn detect(&self, cwd: &Path) -> bool {
        file_contains(cwd, "pyproject.toml", "uvicorn")
            || file_contains(cwd, "pyproject.toml", "fastapi")
            || file_contains(cwd, "requirements.txt", "uvicorn")
            || file_contains(cwd, "requirements.txt", "fastapi")
    }
    fn priority(&self) -> u8 { 80 }
    fn name(&self) -> &'static str { "uvicorn/FastAPI (Python)" }
    fn project_name(&self, cwd: &Path) -> Option<String> {
        read_toml_field(cwd, "pyproject.toml", &["project", "name"])
    }
    fn start_command(&self, _cwd: &Path) -> Option<String> {
        Some("uvicorn main:app".to_string())
    }
    fn port_injection(&self, _cwd: &Path, port: u16) -> PortInjection {
        PortInjection::CliArgs(vec![
            "--host".to_string(), "0.0.0.0".to_string(),
            "--port".to_string(), port.to_string(),
        ])
    }
}

// ─── FlaskDriver ──────────────────────────────────────────────────────────────

pub struct FlaskDriver;

impl LanguageDriver for FlaskDriver {
    fn detect(&self, cwd: &Path) -> bool {
        file_contains(cwd, "pyproject.toml", "flask")
            || file_contains(cwd, "requirements.txt", "flask")
            || cwd.join("app.py").exists()
            || cwd.join("wsgi.py").exists()
    }
    fn priority(&self) -> u8 { 80 }
    fn name(&self) -> &'static str { "Flask (Python)" }
    fn project_name(&self, cwd: &Path) -> Option<String> {
        read_toml_field(cwd, "pyproject.toml", &["project", "name"])
    }
    fn start_command(&self, _cwd: &Path) -> Option<String> {
        Some("flask run".to_string())
    }
    fn port_injection(&self, _cwd: &Path, port: u16) -> PortInjection {
        PortInjection::CliArgs(vec![
            "--host".to_string(), "0.0.0.0".to_string(),
            "--port".to_string(), port.to_string(),
        ])
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;
    use std::fs;

    #[test]
    fn django_detects_manage_py() {
        let tmp = TempDir::new().unwrap();
        fs::write(tmp.path().join("manage.py"), "").unwrap();
        assert!(DjangoDriver.detect(tmp.path()));
    }

    #[test]
    fn django_does_not_detect_without_manage_py() {
        let tmp = TempDir::new().unwrap();
        assert!(!DjangoDriver.detect(tmp.path()));
    }

    #[test]
    fn django_append_address_injection() {
        let tmp = TempDir::new().unwrap();
        let inj = DjangoDriver.port_injection(tmp.path(), 4123);
        match inj {
            crate::detect::PortInjection::AppendAddress(addr) => {
                assert_eq!(addr, "0.0.0.0:4123");
            }
            _ => panic!("expected AppendAddress"),
        }
    }

    #[test]
    fn uvicorn_detects_from_pyproject() {
        let tmp = TempDir::new().unwrap();
        fs::write(tmp.path().join("pyproject.toml"), "uvicorn = \"*\"").unwrap();
        assert!(UvicornDriver.detect(tmp.path()));
    }

    #[test]
    fn uvicorn_detects_from_requirements_txt() {
        let tmp = TempDir::new().unwrap();
        fs::write(tmp.path().join("requirements.txt"), "fastapi\nuvicorn[standard]\n").unwrap();
        assert!(UvicornDriver.detect(tmp.path()));
    }

    #[test]
    fn uvicorn_cli_args_injection() {
        let tmp = TempDir::new().unwrap();
        let inj = UvicornDriver.port_injection(tmp.path(), 4123);
        match inj {
            crate::detect::PortInjection::CliArgs(args) => {
                assert!(args.contains(&"--port".to_string()));
                assert!(args.contains(&"4123".to_string()));
                assert!(args.contains(&"--host".to_string()));
                assert!(args.contains(&"0.0.0.0".to_string()));
            }
            _ => panic!("expected CliArgs"),
        }
    }

    #[test]
    fn flask_detects_from_requirements_txt() {
        let tmp = TempDir::new().unwrap();
        fs::write(tmp.path().join("requirements.txt"), "flask\ngunicorn\n").unwrap();
        assert!(FlaskDriver.detect(tmp.path()));
    }

    #[test]
    fn flask_detects_from_app_py() {
        let tmp = TempDir::new().unwrap();
        fs::write(tmp.path().join("app.py"), "from flask import Flask").unwrap();
        assert!(FlaskDriver.detect(tmp.path()));
    }

    #[test]
    fn flask_does_not_shadow_uvicorn() {
        let tmp = TempDir::new().unwrap();
        fs::write(tmp.path().join("requirements.txt"), "flask\nuvicorn\n").unwrap();
        assert!(FlaskDriver.detect(tmp.path()));
        assert!(UvicornDriver.detect(tmp.path()));
    }
}
