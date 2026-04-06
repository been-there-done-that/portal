//! E2E framework tests — run with: cargo test --test frameworks_test -- --ignored

use portless::ports::find_free_port;
use portless::process::spawn_child;
use std::time::Duration;
use tokio::time::sleep;

async fn wait_for_port(port: u16, timeout: Duration) -> bool {
    let deadline = tokio::time::Instant::now() + timeout;
    while tokio::time::Instant::now() < deadline {
        if tokio::net::TcpStream::connect(format!("127.0.0.1:{port}"))
            .await
            .is_ok()
        {
            return true;
        }
        sleep(Duration::from_millis(100)).await;
    }
    false
}

#[tokio::test]
#[ignore = "requires Node.js"]
async fn express_fixture_responds_via_port() {
    let fixture = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("fixtures/express");
    let port = find_free_port(5000, 5999).unwrap();
    let args = vec!["node".to_string(), "server.js".to_string()];
    let mut child = spawn_child(&fixture, &args, port).await.unwrap();

    let ready = wait_for_port(port, Duration::from_secs(10)).await;
    assert!(ready, "express fixture did not start on port {port}");

    let resp = reqwest::get(format!("http://127.0.0.1:{port}/"))
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    assert!(resp
        .text()
        .await
        .unwrap()
        .contains("hello from express fixture"));

    child.kill().await.ok();
}
