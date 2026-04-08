use thiserror::Error;

#[derive(Debug, Error)]
pub enum Error {
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),

    #[error("JSON error: {0}")]
    Json(#[from] serde_json::Error),

    #[error("TOML error: {0}")]
    Toml(#[from] toml::de::Error),

    #[error("Parse error: {0}")]
    Parse(#[from] std::num::ParseIntError),

    #[error("TLS error: {0}")]
    Tls(#[from] rustls::Error),

    #[error("Certificate error: {0}")]
    Cert(String),

    #[error("Daemon not running — start with `portal daemon`")]
    DaemonNotRunning,

    #[error("Port range exhausted (no free port in {0}–{1})")]
    NoFreePort(u16, u16),

    #[error("Hostname not found: {0}")]
    HostNotFound(String),

    #[error("IPC error: {0}")]
    Ipc(String),

    #[error("invalid port: {0}")]
    InvalidPort(String),
}

pub type Result<T> = std::result::Result<T, Error>;
