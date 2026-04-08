# pf Redirects Fix Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add `X-Portal-Port: <listen-port>` response header to `serve_http_redirect` so external health-checkers can distinguish direct portal responses from pf-redirected ones.

**Architecture:** Add a `PORTAL_PORT_HEADER` constant to `src/proxy.rs`, extend `serve_http_redirect` with an `http_port: u16` parameter, insert the header in the 301 response string, update the one call site in `src/daemon/mod.rs`, and update/add tests.

**Tech Stack:** Rust, Tokio, raw HTTP string formatting (no hyper in `serve_http_redirect`).

---

## File Map

| File | Change |
|---|---|
| `src/proxy.rs` | Add `PORTAL_PORT_HEADER` constant; add `http_port: u16` param; insert header in 301 response; update test call + add 2 new tests |
| `src/daemon/mod.rs` | Pass `config.proxy.http_port` to `serve_http_redirect` |

---

### Task 1: Add `PORTAL_PORT_HEADER`, update `serve_http_redirect`, update callers and tests

**Files:**
- Modify: `src/proxy.rs`
- Modify: `src/daemon/mod.rs`

- [ ] **Step 1: Write failing tests**

Add these two tests after the existing `http_redirect_listener_sends_301` test in the `#[cfg(test)]` block in `src/proxy.rs`:

```rust
    #[tokio::test]
    async fn http_redirect_includes_portal_port_header() {
        use tokio::io::AsyncReadExt;
        use tokio::net::TcpListener;

        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();

        tokio::spawn(serve_http_redirect(listener, port, 443));
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;

        let mut client = tokio::net::TcpStream::connect(format!("127.0.0.1:{}", port))
            .await
            .unwrap();
        let request = "GET / HTTP/1.1\r\nHost: myapp.localhost\r\nConnection: close\r\n\r\n";
        client.write_all(request.as_bytes()).await.unwrap();

        let mut response = String::new();
        client.read_to_string(&mut response).await.unwrap();

        assert!(
            response.to_lowercase().contains("x-portal-port:"),
            "Expected x-portal-port header in response: {}",
            response
        );
    }

    #[tokio::test]
    async fn http_redirect_portal_port_matches_listen_port() {
        use tokio::io::AsyncReadExt;
        use tokio::net::TcpListener;

        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let http_port = listener.local_addr().unwrap().port();

        tokio::spawn(serve_http_redirect(listener, http_port, 8443));
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;

        let mut client = tokio::net::TcpStream::connect(format!("127.0.0.1:{}", http_port))
            .await
            .unwrap();
        let request = "GET / HTTP/1.1\r\nHost: myapp.localhost\r\nConnection: close\r\n\r\n";
        client.write_all(request.as_bytes()).await.unwrap();

        let mut response = String::new();
        client.read_to_string(&mut response).await.unwrap();

        // Header value must be the HTTP listen port, not the HTTPS port
        let expected_header = format!("x-portal-port: {}", http_port);
        assert!(
            response.to_lowercase().contains(&expected_header),
            "Expected '{}' in response: {}",
            expected_header,
            response
        );
        assert!(
            !response.to_lowercase().contains("x-portal-port: 8443"),
            "Header should NOT contain the https port: {}",
            response
        );
    }
```

- [ ] **Step 2: Run tests to verify they fail**

```bash
cargo test --bin portal -- proxy::tests::http_redirect_includes_portal_port_header proxy::tests::http_redirect_portal_port_matches_listen_port 2>&1 | tail -15
```

Expected: compile error because `serve_http_redirect` only takes 2 args — confirms tests reference the not-yet-updated signature.

- [ ] **Step 3: Add `PORTAL_PORT_HEADER` constant to `src/proxy.rs`**

After line 12 (`pub const MAX_HOPS: u8 = 5;`), add:

```rust
pub const PORTAL_PORT_HEADER: &str = "x-portal-port";
```

- [ ] **Step 4: Update `serve_http_redirect` signature and response**

Change:
```rust
pub async fn serve_http_redirect(listener: tokio::net::TcpListener, https_port: u16) {
```

To:
```rust
pub async fn serve_http_redirect(listener: tokio::net::TcpListener, http_port: u16, https_port: u16) {
```

Change the response format string from:
```rust
            let response = format!(
                "HTTP/1.1 301 Moved Permanently\r\nLocation: {}\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
                location
            );
```

To:
```rust
            let response = format!(
                "HTTP/1.1 301 Moved Permanently\r\nLocation: {}\r\nX-Portal-Port: {}\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
                location, http_port
            );
```

- [ ] **Step 5: Update existing test call in `src/proxy.rs`**

In the `http_redirect_listener_sends_301` test, change:

```rust
        tokio::spawn(serve_http_redirect(listener, 443));
```

To:

```rust
        tokio::spawn(serve_http_redirect(listener, port, 443));
```

- [ ] **Step 6: Update call site in `src/daemon/mod.rs`**

Change:
```rust
    tokio::spawn(serve_http_redirect(http_listener, http_https_port));
```

To:
```rust
    tokio::spawn(serve_http_redirect(http_listener, config.proxy.http_port, http_https_port));
```

- [ ] **Step 7: Run new tests to verify they pass**

```bash
cargo test --bin portal -- proxy::tests 2>&1 | tail -15
```

Expected: all 3 proxy tests pass.

- [ ] **Step 8: Run full test suite**

```bash
cargo test --bin portal 2>&1 | grep "test result"
```

Expected: all tests pass, 0 failures.

- [ ] **Step 9: Commit**

```bash
git add src/proxy.rs src/daemon/mod.rs
git commit -m "feat(proxy): add X-Portal-Port header to HTTP redirect responses"
```
