use crate::error::{Error, Result};
use serde::{Deserialize, Serialize};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

/// Command enum sent from CLI to Daemon (IPC).
/// Uses serde tag "cmd" to identify the command type.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "cmd", rename_all = "snake_case")]
pub enum Command {
    /// Start a proxy for a hostname
    Run {
        hostname: String,
        args: Vec<String>,
        cwd: String,
    },
    /// Stop a proxy for a hostname
    Stop { hostname: String },
    /// List all active proxies
    Ls,
    /// Get daemon status
    Status,
    /// Remove a proxy configuration
    Rm { hostname: String },
    /// Shutdown the daemon
    Shutdown,
    /// Install the TLS certificate
    CertInstall,
    /// Reset the TLS certificate
    CertReset,
    /// Register a route in the daemon's in-memory store (called by CLI after spawning child)
    RegisterRoute {
        hostname: String,
        port: u16,
        public_port: Option<u16>,
        protocol: crate::routes::RouteProtocol,
        pid: u32,
        cwd: String,
    },
    /// Force-sync /etc/hosts with the current route table
    HostsSync,
    /// Remove the portless-managed block from /etc/hosts
    HostsClean,
    /// Get the public URL for a named service
    GetUrl { hostname: String },
    /// Find and kill orphaned dev server processes left by crashed CLI sessions
    Prune,
}

/// Response sent from Daemon to CLI (IPC).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Response {
    pub ok: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub data: Option<serde_json::Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

impl Response {
    /// Create a successful response with data
    pub fn ok(data: serde_json::Value) -> Self {
        Response {
            ok: true,
            data: Some(data),
            error: None,
        }
    }

    /// Create a successful response with no data
    pub fn ok_empty() -> Self {
        Response {
            ok: true,
            data: None,
            error: None,
        }
    }

    /// Create an error response
    pub fn err(msg: impl Into<String>) -> Self {
        Response {
            ok: false,
            data: None,
            error: Some(msg.into()),
        }
    }
}

/// Write a length-prefixed JSON frame to an async writer.
/// Frame format: 4-byte big-endian u32 length, followed by UTF-8 JSON payload.
pub async fn write_frame<W, T>(writer: &mut W, value: &T) -> Result<()>
where
    W: AsyncWriteExt + Unpin,
    T: Serialize,
{
    let json = serde_json::to_string(value)?;
    let len =
        u32::try_from(json.len()).map_err(|_| Error::Ipc("frame payload too large".to_string()))?;
    let mut buf = [0u8; 4];
    buf.copy_from_slice(&len.to_be_bytes());

    writer.write_all(&buf).await?;
    writer.write_all(json.as_bytes()).await?;
    Ok(())
}

/// Read a length-prefixed JSON frame from an async reader.
/// Frame format: 4-byte big-endian u32 length, followed by UTF-8 JSON payload.
pub async fn read_frame<R, T>(reader: &mut R) -> Result<T>
where
    R: AsyncReadExt + Unpin,
    T: for<'de> Deserialize<'de>,
{
    let mut len_buf = [0u8; 4];
    reader.read_exact(&mut len_buf).await?;
    const MAX_FRAME_LEN: usize = 1 * 1024 * 1024; // 1 MiB
    let len = u32::from_be_bytes(len_buf) as usize;
    if len > MAX_FRAME_LEN {
        return Err(Error::Ipc(format!(
            "frame too large: {len} bytes (max {MAX_FRAME_LEN})"
        )));
    }

    let mut payload = vec![0u8; len];
    reader.read_exact(&mut payload).await?;

    let json_str = String::from_utf8(payload)
        .map_err(|_| Error::Ipc("Invalid UTF-8 in frame payload".to_string()))?;
    let value = serde_json::from_str(&json_str)?;
    Ok(value)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trips_run_command() {
        let cmd = Command::Run {
            hostname: "myapp.localhost".to_string(),
            args: vec!["npm".to_string(), "start".to_string()],
            cwd: "/home/user/myapp".to_string(),
        };

        // Serialize to JSON
        let json = serde_json::to_string(&cmd).expect("Failed to serialize");

        // Deserialize back
        let deserialized: Command = serde_json::from_str(&json).expect("Failed to deserialize");

        // Verify
        if let Command::Run {
            hostname,
            args: _,
            cwd: _,
        } = deserialized
        {
            assert_eq!(hostname, "myapp.localhost");
        } else {
            panic!("Expected Run command");
        }
    }

    #[test]
    fn round_trips_ok_response() {
        let data = serde_json::json!({"status": "running"});
        let response = Response::ok(data.clone());

        // Serialize to JSON
        let json = serde_json::to_string(&response).expect("Failed to serialize");

        // Deserialize back
        let deserialized: Response = serde_json::from_str(&json).expect("Failed to deserialize");

        // Verify
        assert!(deserialized.ok);
        assert_eq!(deserialized.data, Some(data));
        assert!(deserialized.error.is_none());
    }

    #[test]
    fn round_trips_err_response() {
        let response = Response::err("Something went wrong");

        // Serialize to JSON
        let json = serde_json::to_string(&response).expect("Failed to serialize");

        // Deserialize back
        let deserialized: Response = serde_json::from_str(&json).expect("Failed to deserialize");

        // Verify
        assert!(!deserialized.ok);
        assert!(deserialized.data.is_none());
        assert_eq!(deserialized.error, Some("Something went wrong".to_string()));
    }

    #[tokio::test]
    async fn frame_encode_decode() {
        // Create a command
        let cmd = Command::Run {
            hostname: "testapp.localhost".to_string(),
            args: vec!["python".to_string(), "app.py".to_string()],
            cwd: "/tmp/testapp".to_string(),
        };

        // Create a duplex (in-memory pipe)
        let (mut client, mut server) = tokio::io::duplex(4096);

        // Write frame from client side
        write_frame(&mut client, &cmd)
            .await
            .expect("Failed to write frame");

        // Read frame from server side
        let received: Command = read_frame(&mut server).await.expect("Failed to read frame");

        // Verify
        if let Command::Run {
            hostname,
            args: _,
            cwd: _,
        } = received
        {
            assert_eq!(hostname, "testapp.localhost");
        } else {
            panic!("Expected Run command");
        }
    }

    #[test]
    fn round_trips_hosts_sync_command() {
        let cmd = Command::HostsSync;
        let json = serde_json::to_string(&cmd).expect("serialize");
        let back: Command = serde_json::from_str(&json).expect("deserialize");
        assert!(matches!(back, Command::HostsSync));
    }

    #[test]
    fn round_trips_hosts_clean_command() {
        let cmd = Command::HostsClean;
        let json = serde_json::to_string(&cmd).expect("serialize");
        let back: Command = serde_json::from_str(&json).expect("deserialize");
        assert!(matches!(back, Command::HostsClean));
    }

    #[test]
    fn round_trips_register_route_with_protocol() {
        let cmd = Command::RegisterRoute {
            hostname: "redis.localhost".to_string(),
            port: 6379,
            public_port: Some(46379),
            protocol: crate::routes::RouteProtocol::Tcp,
            pid: 123,
            cwd: "/tmp".to_string(),
        };

        let json = serde_json::to_string(&cmd).expect("serialize");
        let back: Command = serde_json::from_str(&json).expect("deserialize");

        match back {
            Command::RegisterRoute {
                hostname,
                port,
                public_port,
                protocol,
                ..
            } => {
                assert_eq!(hostname, "redis.localhost");
                assert_eq!(port, 6379);
                assert_eq!(public_port, Some(46379));
                assert_eq!(protocol, crate::routes::RouteProtocol::Tcp);
            }
            other => panic!("unexpected command: {other:?}"),
        }
    }

    #[test]
    fn round_trips_get_url_command() {
        let cmd = Command::GetUrl {
            hostname: "myapp.localhost".to_string(),
        };
        let json = serde_json::to_string(&cmd).expect("serialize");
        let back: Command = serde_json::from_str(&json).expect("deserialize");
        match back {
            Command::GetUrl { hostname } => assert_eq!(hostname, "myapp.localhost"),
            other => panic!("unexpected: {other:?}"),
        }
    }
}
