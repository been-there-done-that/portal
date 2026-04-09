# SNI-based TCP Routing Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** After TLS termination, detect non-HTTP traffic and bridge raw bytes to the backend — enabling `psql`, `redis-cli`, etc. to connect via `{name}.localhost` on portal's HTTPS port.

**Architecture:** Add `is_http_method_prefix()` to `src/proxy.rs`. In `serve_https` (in `src/daemon/mod.rs`), after TLS accept, read first 4 bytes. If HTTP → feed to hyper with a `PrefixedIo` wrapper that replays the peeked bytes. If not HTTP → extract SNI hostname, look up route, bridge with `copy_bidirectional`.

**Tech Stack:** Rust, tokio, tokio-rustls 0.26, rustls 0.23, hyper 1

---

## File Map

| File | Change |
|---|---|
| `src/proxy.rs` | Add `is_http_method_prefix()` + `PrefixedIo` struct + tests |
| `src/daemon/mod.rs` | Rewrite `serve_https` spawn block: peek → branch HTTP vs TCP bridge |

---

## Task 1: Add `is_http_method_prefix` and `PrefixedIo` to `src/proxy.rs`

**Files:**
- Modify: `src/proxy.rs`

### Background

`is_http_method_prefix` checks if a byte slice starts with an HTTP method. HTTP/1.x requests always begin with `METHOD SP` — the first 3-4 bytes are ASCII letters. Non-HTTP protocols (Postgres, Redis, MySQL, MongoDB, gRPC/h2) start with binary bytes or protocol-specific markers that never look like HTTP methods.

`PrefixedIo` is a wrapper that replays peeked bytes before reading from the inner stream. This lets us peek at the TLS stream, decide HTTP vs TCP, then pass the full stream (including peeked bytes) to hyper for the HTTP path.

- [ ] **Step 1: Write failing tests**

Add to the bottom of `src/proxy.rs`, in the `#[cfg(test)]` block (create one if it doesn't exist):

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn http_method_prefix_detects_get() {
        assert!(is_http_method_prefix(b"GET "));
        assert!(is_http_method_prefix(b"GET /index.html HTTP/1.1"));
    }

    #[test]
    fn http_method_prefix_detects_post() {
        assert!(is_http_method_prefix(b"POST"));
        assert!(is_http_method_prefix(b"POST /api HTTP/1.1"));
    }

    #[test]
    fn http_method_prefix_detects_other_methods() {
        assert!(is_http_method_prefix(b"PUT "));
        assert!(is_http_method_prefix(b"HEAD"));
        assert!(is_http_method_prefix(b"DELE"));
        assert!(is_http_method_prefix(b"PATC"));
        assert!(is_http_method_prefix(b"OPTI"));
        assert!(is_http_method_prefix(b"CONN"));
    }

    #[test]
    fn http_method_prefix_rejects_postgres() {
        // Postgres startup message begins with 4-byte length prefix (big-endian int)
        assert!(!is_http_method_prefix(&[0x00, 0x00, 0x00, 0x08]));
    }

    #[test]
    fn http_method_prefix_rejects_redis() {
        // Redis RESP protocol: *3\r\n
        assert!(!is_http_method_prefix(b"*3\r\n"));
    }

    #[test]
    fn http_method_prefix_rejects_mysql() {
        // MySQL handshake starts with packet length (3 bytes) + sequence byte
        assert!(!is_http_method_prefix(&[0x4a, 0x00, 0x00, 0x00]));
    }

    #[test]
    fn http_method_prefix_rejects_empty() {
        assert!(!is_http_method_prefix(b""));
        assert!(!is_http_method_prefix(b"GE"));
    }

    #[test]
    fn http_method_prefix_rejects_short_buffer() {
        assert!(!is_http_method_prefix(b"G"));
        assert!(!is_http_method_prefix(b"PO"));
    }
}
```

- [ ] **Step 2: Run tests to verify they fail**

```bash
cargo test is_http_method_prefix 2>&1 | grep -E "FAILED|error\[" | head -5
```

Expected: compile error — `is_http_method_prefix` doesn't exist yet.

- [ ] **Step 3: Implement `is_http_method_prefix`**

Add this function in `src/proxy.rs` after the `is_tls_client_hello` function:

```rust
/// Check if a byte buffer starts with an HTTP/1.x method.
/// Used after TLS termination to distinguish HTTP from raw TCP (Postgres, Redis, etc.).
/// Requires at least 3 bytes; returns false for short buffers.
pub fn is_http_method_prefix(buf: &[u8]) -> bool {
    if buf.len() < 3 {
        return false;
    }
    // HTTP methods: GET, PUT, HEAD, POST, DELETE, PATCH, OPTIONS, CONNECT
    // Check first 3-4 bytes against known prefixes
    matches!(
        &buf[..3],
        b"GET" | b"PUT" | b"POS" | b"HEA" | b"DEL" | b"PAT" | b"OPT" | b"CON"
    )
}
```

- [ ] **Step 4: Run tests**

```bash
cargo test is_http_method_prefix 2>&1 | grep -E "^test result|FAILED"
```

Expected: all tests pass.

- [ ] **Step 5: Implement `PrefixedIo`**

Add this struct in `src/proxy.rs` after the `is_http_method_prefix` function:

```rust
use std::pin::Pin;
use std::task::{Context, Poll};

/// A wrapper that replays a prefix buffer before reading from the inner stream.
/// Used to peek at TLS-decrypted bytes for protocol detection, then pass the
/// full stream (prefix + rest) to hyper for HTTP handling.
pub struct PrefixedIo<S> {
    prefix: Vec<u8>,
    pos: usize,
    inner: S,
}

impl<S> PrefixedIo<S> {
    pub fn new(prefix: Vec<u8>, inner: S) -> Self {
        Self { prefix, pos: 0, inner }
    }
}

impl<S: tokio::io::AsyncRead + Unpin> tokio::io::AsyncRead for PrefixedIo<S> {
    fn poll_read(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut tokio::io::ReadBuf<'_>,
    ) -> Poll<std::io::Result<()>> {
        let this = self.get_mut();
        // First, drain the prefix buffer
        if this.pos < this.prefix.len() {
            let remaining = &this.prefix[this.pos..];
            let to_copy = remaining.len().min(buf.remaining());
            buf.put_slice(&remaining[..to_copy]);
            this.pos += to_copy;
            return Poll::Ready(Ok(()));
        }
        // Then delegate to the inner stream
        Pin::new(&mut this.inner).poll_read(cx, buf)
    }
}

impl<S: tokio::io::AsyncWrite + Unpin> tokio::io::AsyncWrite for PrefixedIo<S> {
    fn poll_write(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<std::io::Result<usize>> {
        Pin::new(&mut self.get_mut().inner).poll_write(cx, buf)
    }

    fn poll_flush(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<std::io::Result<()>> {
        Pin::new(&mut self.get_mut().inner).poll_flush(cx)
    }

    fn poll_shutdown(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<std::io::Result<()>> {
        Pin::new(&mut self.get_mut().inner).poll_shutdown(cx)
    }
}
```

Note: `PrefixedIo` implements both `AsyncRead` and `AsyncWrite` because hyper's `TokioIo` requires both. The write side just delegates directly to the inner stream.

- [ ] **Step 6: Add `PrefixedIo` test**

Add to the test module:

```rust
    #[tokio::test]
    async fn prefixed_io_replays_prefix_then_inner() {
        use tokio::io::AsyncReadExt;
        let inner = std::io::Cursor::new(b"world".to_vec());
        let mut reader = PrefixedIo::new(b"hello".to_vec(), inner);
        let mut buf = vec![0u8; 10];
        let n = reader.read(&mut buf).await.unwrap();
        assert_eq!(&buf[..n], b"hello");
        let n2 = reader.read(&mut buf).await.unwrap();
        assert_eq!(&buf[..n2], b"world");
    }
```

- [ ] **Step 7: Run all tests**

```bash
cargo test 2>&1 | grep -E "^test result|FAILED"
```

Expected: all tests pass.

- [ ] **Step 8: Commit**

```bash
git add src/proxy.rs
git commit -m "feat(proxy): add is_http_method_prefix and PrefixedIo for protocol detection"
```

---

## Task 2: Protocol branch in `serve_https`

**Files:**
- Modify: `src/daemon/mod.rs`

### Background

The current `serve_https` spawn block does:
1. Peek first byte → reject non-TLS
2. TLS accept
3. Wrap in `TokioIo` → feed to hyper HTTP/1

The new flow:
1. Peek first byte → reject non-TLS
2. TLS accept
3. **Read first 4 bytes** of decrypted stream
4. **If HTTP** → wrap in `PrefixedIo` (replay peeked bytes) → `TokioIo` → hyper
5. **If not HTTP** → extract SNI hostname → look up route → `copy_bidirectional` to backend

- [ ] **Step 1: Read the current `serve_https` function**

Read `src/daemon/mod.rs` lines 296-350 to see the exact current code.

- [ ] **Step 2: Replace the spawn block**

Replace the entire content inside `tokio::spawn(async move { ... })` (lines 323-348) with:

```rust
        tokio::spawn(async move {
            let first = match crate::proxy::peek_first_byte(&tcp_stream).await {
                Ok(b) => b,
                Err(_) => return,
            };
            if !crate::proxy::is_tls_client_hello(first) {
                return;
            }

            let Ok(mut tls_stream) = acceptor.accept(tcp_stream).await else {
                return;
            };

            // Read first bytes to detect HTTP vs raw TCP
            let mut peek_buf = [0u8; 4];
            let n = match tokio::io::AsyncReadExt::read(&mut tls_stream, &mut peek_buf).await {
                Ok(0) => return, // connection closed
                Ok(n) => n,
                Err(_) => return,
            };
            let peeked = peek_buf[..n].to_vec();

            if crate::proxy::is_http_method_prefix(&peeked) {
                // HTTP path: replay peeked bytes + rest of stream → hyper
                let prefixed = crate::proxy::PrefixedIo::new(peeked, tls_stream);
                let io = hyper_util::rt::TokioIo::new(prefixed);
                hyper::server::conn::http1::Builder::new()
                    .serve_connection(
                        io,
                        hyper::service::service_fn(move |req| {
                            let r = routes.clone();
                            let insp = inspector.clone();
                            async move { crate::proxy::handle_https_request(req, r, insp).await }
                        }),
                    )
                    .with_upgrades()
                    .await
                    .ok();
            } else {
                // TCP bridge: extract SNI hostname → look up route → bridge
                let sni = tls_stream
                    .get_ref()
                    .1
                    .server_name()
                    .map(|s| s.to_string());

                let hostname = match sni {
                    Some(h) => h,
                    None => {
                        tracing::debug!("non-HTTP connection without SNI hostname, dropping");
                        return;
                    }
                };

                let route = match routes.get(&hostname) {
                    Some(r) => r,
                    None => {
                        tracing::debug!("no route for TCP connection to {hostname}");
                        return;
                    }
                };

                let mut backend = match tokio::net::TcpStream::connect(("127.0.0.1", route.port)).await {
                    Ok(s) => s,
                    Err(e) => {
                        tracing::debug!("TCP bridge: failed to connect to backend port {}: {e}", route.port);
                        return;
                    }
                };

                // Send the peeked bytes to the backend first
                if let Err(_) = tokio::io::AsyncWriteExt::write_all(&mut backend, &peeked).await {
                    return;
                }

                // Bridge the rest
                let _ = tokio::io::copy_bidirectional(&mut tls_stream, &mut backend).await;
            }
        });
```

- [ ] **Step 3: Remove unused imports if needed**

The old code had `use hyper_util::rt::TokioIo;` inside the function — it's now inlined as `hyper_util::rt::TokioIo::new(...)`. Same for `http1::Builder`. Check if the existing `use` statements at the top of `serve_https` are still needed; the imports `use hyper::server::conn::http1;` and `use hyper_util::rt::TokioIo;` can stay or be removed since they're now fully qualified in the code.

- [ ] **Step 4: Build**

```bash
cargo build 2>&1 | grep "^error" | head -20
```

Expected: no errors. If there are type mismatches (e.g. `TokioIo` not implementing the right trait for `PrefixedIo`), debug by checking that `PrefixedIo` implements both `AsyncRead` and `AsyncWrite`.

- [ ] **Step 5: Run all tests**

```bash
cargo test 2>&1 | grep -E "^test result|FAILED"
```

Expected: all tests pass.

- [ ] **Step 6: Commit**

```bash
git add src/daemon/mod.rs
git commit -m "feat(daemon): SNI-based TCP routing — detect non-HTTP after TLS and bridge to backend"
```

---

## Self-Review

**Spec coverage:**

- ✅ `is_http_method_prefix` detects HTTP methods — Task 1 with 8 test cases
- ✅ Rejects Postgres, Redis, MySQL byte patterns — Task 1 tests
- ✅ `PrefixedIo` replays peeked bytes for HTTP path — Task 1 implementation + test
- ✅ SNI hostname extraction from rustls — Task 2 `tls_stream.get_ref().1.server_name()`
- ✅ Route lookup by SNI hostname — Task 2 `routes.get(&hostname)`
- ✅ TCP bridge via `copy_bidirectional` — Task 2 non-HTTP branch
- ✅ Peeked bytes forwarded to backend — Task 2 `write_all(&mut backend, &peeked)`
- ✅ No route found → drop connection — Task 2 with `tracing::debug!`
- ✅ No CLI/IPC/route model changes — only `proxy.rs` and `daemon/mod.rs` touched

**No placeholders found.**

**Type consistency:** `PrefixedIo<S>` used consistently in Task 1 (definition) and Task 2 (usage with `TlsStream`). `is_http_method_prefix(&[u8])` signature matches between Task 1 and Task 2.
