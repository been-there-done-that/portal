//! Integration tests for process restart and port reuse behavior.

use std::path::Path;
use std::time::Duration;
use portal::ports::{find_free_port, wait_for_port_free};
use portal::process::{spawn_child, stop_child};
use portal::detect::PortInjection;

/// Simulates portal start → stop → start on the same port.
/// Verifies the second bind succeeds (no EADDRINUSE race).
#[tokio::test]
async fn restart_on_same_port_no_eaddrinuse() {
    #[cfg(unix)]
    {
        let port = find_free_port(19800, 19850).unwrap();

        // Write a small python script to a temp file to avoid nested-quote issues
        let script = format!(
            "import socket,time\ns=socket.socket()\ns.setsockopt(socket.SOL_SOCKET,socket.SO_REUSEADDR,1)\ns.bind(('0.0.0.0',{port}))\ns.listen(1)\ntime.sleep(10)\n"
        );
        let script_file = format!("/tmp/portal_bind_test_{port}.py");
        std::fs::write(&script_file, &script).unwrap();

        // Start a server that binds the port
        let args = vec![
            "python3".to_string(),
            script_file.clone(),
        ];
        let mut child = spawn_child(
            Path::new("/tmp"), &args, port, "test.localhost",
            PortInjection::EnvOnly,
        ).await.unwrap();

        // Poll until the port is actually bound (python3 startup can be slow).
        // Bind 0.0.0.0 (wildcard) — on macOS, binding 127.0.0.1 succeeds even when
        // 0.0.0.0 is already owned by another process, making it unreliable as a check.
        let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
        let port_in_use = loop {
            let port_for_check = port;
            let is_free = tokio::task::spawn_blocking(move || {
                std::net::TcpListener::bind(format!("0.0.0.0:{port_for_check}")).is_ok()
            })
            .await
            .unwrap_or(true);
            if !is_free {
                break true;
            }
            if tokio::time::Instant::now() >= deadline {
                break false;
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        };
        assert!(port_in_use, "port {port} should be in use after python3 server started");

        // Stop it
        let _ = stop_child(&mut child).await;

        // wait_for_port_free should detect it's released
        wait_for_port_free(port, Duration::from_secs(3)).await;

        // Now we should be able to bind the port (no EADDRINUSE).
        // Bind 0.0.0.0 (wildcard) for accurate port availability check on macOS.
        let port_for_final = port;
        let is_free = tokio::task::spawn_blocking(move || {
            std::net::TcpListener::bind(format!("0.0.0.0:{port_for_final}")).is_ok()
        })
        .await
        .unwrap_or(false);
        assert!(is_free, "port should be free after stop_child + wait_for_port_free");

        let _ = std::fs::remove_file(&script_file);
    }
}
