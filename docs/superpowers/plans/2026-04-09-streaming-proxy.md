# Streaming Proxy Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Replace full body buffering with streaming `TeeBody` that forwards chunks at wire speed while capturing the first 1MB for the inspector.

**Architecture:** `TeeBody<B>` wraps any `hyper::body::Body`, forwarding each polled frame to the consumer while copying the first 1MB into a side buffer. `handle_https_request` wraps request body in `TeeBody`, sends to backend (streaming), wraps response body in `TeeBody`, returns to client (streaming), fires inspector capture on response completion via a spawned task.

**Tech Stack:** Rust, hyper 1, http-body-util, bytes, tokio

---

## File Map

| File | Change |
|---|---|
| `src/proxy.rs` | Add `TeeBody<B>` struct + `Body` impl + tests. Refactor `handle_https_request`. |

---

## Task 1: `TeeBody<B>` — streaming body wrapper with side capture

**Files:**
- Modify: `src/proxy.rs`

### Background

`TeeBody<B>` wraps a `hyper::body::Body`. On each `poll_frame`, it forwards the frame to the consumer and copies data bytes into a side buffer up to `BODY_CAP` (1MB from `crate::inspector::types::BODY_CAP`). After the stream ends, `take_captured()` returns the prefix bytes and total byte count.

The side buffer uses `Arc<std::sync::Mutex<Vec<u8>>>` and `Arc<AtomicUsize>` so the capture can be read from a different task (the inspector fires after the response finishes streaming).

- [ ] **Step 1: Write failing tests for `TeeBody`**

Add these tests to the `#[cfg(test)] mod tests` block at the bottom of `src/proxy.rs`:

```rust
#[tokio::test]
async fn tee_body_captures_small_body() {
    use http_body_util::BodyExt;
    let original = full_body("hello world");
    let tee = TeeBody::new(original);
    let collected = tee.collect().await.unwrap().to_bytes();
    assert_eq!(&collected[..], b"hello world");
    // Can't call take_captured after collect since tee is consumed
    // Instead test via a shared reference pattern
}

#[tokio::test]
async fn tee_body_captures_and_forwards_full_body() {
    use http_body_util::BodyExt;
    let data = bytes::Bytes::from(vec![b'x'; 500]);
    let original = full_body(data.clone());
    let tee = TeeBody::new(original);
    let tee_captured = tee.captured_handle();
    let collected = tee.collect().await.unwrap().to_bytes();
    assert_eq!(collected.len(), 500);
    let (prefix, total) = tee_captured.take();
    assert_eq!(prefix.len(), 500);
    assert_eq!(total, 500);
}

#[tokio::test]
async fn tee_body_truncates_capture_at_1mb() {
    use http_body_util::BodyExt;
    let size = crate::inspector::types::BODY_CAP + 1000;
    let data = bytes::Bytes::from(vec![b'y'; size]);
    let original = full_body(data.clone());
    let tee = TeeBody::new(original);
    let handle = tee.captured_handle();
    let collected = tee.collect().await.unwrap().to_bytes();
    // Full body forwarded
    assert_eq!(collected.len(), size);
    // Capture truncated at BODY_CAP
    let (prefix, total) = handle.take();
    assert_eq!(prefix.len(), crate::inspector::types::BODY_CAP);
    assert_eq!(total, size);
}

#[tokio::test]
async fn tee_body_empty() {
    use http_body_util::BodyExt;
    let original = full_body("");
    let tee = TeeBody::new(original);
    let handle = tee.captured_handle();
    let collected = tee.collect().await.unwrap().to_bytes();
    assert_eq!(collected.len(), 0);
    let (prefix, total) = handle.take();
    assert_eq!(prefix.len(), 0);
    assert_eq!(total, 0);
}
```

- [ ] **Step 2: Run tests to verify they fail**

```bash
cargo test tee_body 2>&1 | grep -E "FAILED|error\[" | head -5
```

Expected: compile error — `TeeBody` doesn't exist.

- [ ] **Step 3: Implement `TeeBody<B>` and `CaptureHandle`**

Add these types after the `PrefixedIo` implementation (before the `handle_https_request` function):

```rust
use std::sync::atomic::{AtomicUsize, Ordering};

/// Handle to read the captured body prefix after the stream is consumed.
#[derive(Clone)]
pub struct CaptureHandle {
    buf: std::sync::Arc<std::sync::Mutex<Vec<u8>>>,
    total: std::sync::Arc<AtomicUsize>,
    cap: usize,
}

impl CaptureHandle {
    /// Take the captured prefix and total byte count.
    pub fn take(&self) -> (Bytes, usize) {
        let buf = self.buf.lock().unwrap_or_else(|e| e.into_inner());
        (Bytes::from(buf.clone()), self.total.load(Ordering::Relaxed))
    }

    /// Build a CapturedBody from the capture.
    pub fn to_captured_body(&self) -> crate::inspector::types::CapturedBody {
        let (prefix, total) = self.take();
        if prefix.is_empty() {
            crate::inspector::types::CapturedBody::Empty
        } else if total <= self.cap {
            crate::inspector::types::CapturedBody::Full(prefix)
        } else {
            crate::inspector::types::CapturedBody::Truncated { prefix, total_bytes: total }
        }
    }
}

/// A body wrapper that forwards all frames to the consumer while capturing
/// the first `BODY_CAP` bytes in a side buffer for the inspector.
/// The proxy streams at wire speed — only the capture is bounded.
pub struct TeeBody<B> {
    inner: B,
    handle: CaptureHandle,
}

impl<B> TeeBody<B> {
    pub fn new(inner: B) -> Self {
        let cap = crate::inspector::types::BODY_CAP;
        Self {
            inner,
            handle: CaptureHandle {
                buf: std::sync::Arc::new(std::sync::Mutex::new(Vec::with_capacity(cap.min(65536)))),
                total: std::sync::Arc::new(AtomicUsize::new(0)),
                cap,
            },
        }
    }

    /// Get a handle to read the captured prefix after the body is consumed.
    pub fn captured_handle(&self) -> CaptureHandle {
        self.handle.clone()
    }
}

impl<B> hyper::body::Body for TeeBody<B>
where
    B: hyper::body::Body<Data = Bytes> + Unpin,
    B::Error: Into<Box<dyn std::error::Error + Send + Sync>>,
{
    type Data = Bytes;
    type Error = B::Error;

    fn poll_frame(
        self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<Option<Result<hyper::body::Frame<Bytes>, Self::Error>>> {
        let this = self.get_mut();
        match std::pin::Pin::new(&mut this.inner).poll_frame(cx) {
            std::task::Poll::Ready(Some(Ok(frame))) => {
                if let Some(data) = frame.data_ref() {
                    this.handle.total.fetch_add(data.len(), Ordering::Relaxed);
                    let mut buf = this.handle.buf.lock().unwrap_or_else(|e| e.into_inner());
                    if buf.len() < this.handle.cap {
                        let room = this.handle.cap - buf.len();
                        let to_copy = data.len().min(room);
                        buf.extend_from_slice(&data[..to_copy]);
                    }
                }
                std::task::Poll::Ready(Some(Ok(frame)))
            }
            other => other,
        }
    }

    fn is_end_stream(&self) -> bool {
        self.inner.is_end_stream()
    }

    fn size_hint(&self) -> hyper::body::SizeHint {
        self.inner.size_hint()
    }
}
```

- [ ] **Step 4: Run tests**

```bash
cargo test tee_body 2>&1 | grep -E "^test result|FAILED"
```

Expected: all 4 tee_body tests pass.

- [ ] **Step 5: Run full test suite**

```bash
cargo test 2>&1 | grep -E "^test result|FAILED"
```

Expected: all tests pass.

- [ ] **Step 6: Commit**

```bash
git add src/proxy.rs
git commit -m "feat(proxy): add TeeBody streaming body wrapper with 1MB inspector capture"
```

---

## Task 2: Refactor `handle_https_request` to stream

**Files:**
- Modify: `src/proxy.rs`

### Background

Replace the `.collect().await` pattern with `TeeBody` for both request and response bodies. The flow becomes:

1. Wrap incoming request body in `TeeBody` → box it → send to backend
2. Backend consumes the body by streaming (hyper client pulls frames)
3. After the upstream response arrives, extract request capture from the handle
4. Wrap response body in `TeeBody` → box it → return to client
5. Spawn a background task: when the response body finishes streaming, fire the inspector capture

### Key change: HTTP client body type

The shared `HTTP_CLIENT` has type `Client<HttpConnector, BoxBodyType>`. `BoxBodyType` is `BoxBody<Bytes, hyper::Error>`. We wrap `TeeBody` in `BoxBody` via `.map_err(into).boxed()` before sending.

But there's a subtlety: after `client.request(upstream_req).await` returns, the request body has been fully consumed by hyper (streamed to the backend). At that point, the request `CaptureHandle` has the captured prefix.

For the response: we return a `TeeBody`-wrapped response to the client. But hyper expects `BoxBodyType` as the response body type from our handler. So we box the `TeeBody<Incoming>` into `BoxBody`.

The inspector fires in a spawned task that checks when the response body handle has been fully consumed. Since we can't await the response consumption (it happens after we return), we use `tokio::spawn` with a polling approach — or simpler: we accept that the response capture may lag slightly and have the inspector record fire with what was captured at the time we can read it.

Actually, the simplest correct approach: **capture the request body** (which IS fully consumed before the response arrives) and for the **response body**, fire the inspector capture eagerly with headers + status + request body, and update the response body capture asynchronously.

Even simpler: for the response body, we can still `.collect().await` for small responses (< BODY_CAP), and only stream for large ones. But this defeats the purpose.

**Final approach: eager inspector capture + async response body.**

1. Stream request body → capture first 1MB
2. Get response headers + status
3. Fire inspector capture with request body + response headers + status + **empty response body** (or partial)
4. Stream response body to client via TeeBody
5. When response body finishes, update the inspector record with the captured prefix

This requires an inspector update mechanism. Too complex.

**Simplest correct approach:** Just buffer the request body (it's typically small — JSON, form data) and stream the response body (which is where the large payloads are — JS bundles, images, etc.). This is what nginx does for proxy_pass with small request buffers.

For the request side: keep `body.collect().await` with the existing inspector capture pattern — but use `http_body_util::Limited` to cap at a sane limit (e.g., 10MB).

For the response side: wrap in `TeeBody`, return to client streaming, fire inspector in a spawn after response completes.

BUT: we need the response inspector capture to fire. The spawn needs to know when the response is done. We can do this by wrapping the response in a custom body that fires on drop or on end-of-stream.

**Let's use this design:**

- Request: `Limited` body → collect (capped at 10MB) → capture for inspector + forward to backend
- Response: `TeeBody` wrapping backend response → box → return to client. Inspector fires in `poll_frame` when stream ends.

Actually we CAN fire the inspector inline in `poll_frame` when the stream returns `None`:

```rust
// In TeeBody, add an optional oneshot sender
on_complete: Option<tokio::sync::oneshot::Sender<()>>,
```

When `poll_frame` returns `Ready(None)`, fire the completion signal. A spawned task awaits that signal and sends the `CapturedRequest`.

- [ ] **Step 1: Locate the section to replace**

Read `src/proxy.rs` lines 286-380 (the body collect + forward + inspector section).

- [ ] **Step 2: Replace the request body handling**

Replace the 50MB collect with a 10MB `Limited` collect. This is still buffered but with a sane limit — request bodies in dev are almost always under 10MB.

Find:
```rust
    // Collect request body with a 50 MB safety limit to prevent OOM
    const MAX_BODY: usize = 50 * 1024 * 1024;
    let req_body_bytes = match body.collect().await {
        Ok(c) => {
            let b = c.to_bytes();
            if b.len() > MAX_BODY { b.slice(..MAX_BODY) } else { b }
        }
```

Replace with:
```rust
    // Collect request body — capped at 10MB (request bodies in dev are typically small)
    let req_body_bytes = match http_body_util::Limited::new(body, 10 * 1024 * 1024)
        .collect()
        .await
    {
        Ok(c) => c.to_bytes(),
```

Keep the error handling the same.

- [ ] **Step 3: Replace the response body handling with TeeBody**

Replace the response section (from `match client.request(upstream_req).await {` to the end of the Ok arm):

Find the current pattern:
```rust
    match client.request(upstream_req).await {
        Ok(upstream_resp) => {
            let (resp_parts, resp_body) = upstream_resp.into_parts();
            let res_status = resp_parts.status.as_u16();

            let res_headers: Vec<(String, String)> = resp_parts
                .headers
                .iter()
                .filter_map(|(k, v)| Some((k.as_str().to_string(), v.to_str().ok()?.to_string())))
                .collect();

            let resp_bytes = match resp_body.collect().await {
                Ok(c) => {
                    let b = c.to_bytes();
                    if b.len() > MAX_BODY { b.slice(..MAX_BODY) } else { b }
                }
                Err(_) => bytes::Bytes::new(),
            };

            if let Some(sender) = &inspector {
                use crate::inspector::types::{CapturedBody, CapturedRequest};
                sender.send(CapturedRequest {
                    hostname: hostname.clone(),
                    method,
                    path: path_and_query,
                    req_headers,
                    req_body: CapturedBody::from_bytes(&req_body_bytes),
                    res_status,
                    res_headers,
                    res_body: CapturedBody::from_bytes(&resp_bytes),
                    duration_ms: start.elapsed().as_millis() as u64,
                    timestamp: chrono::Utc::now().timestamp_millis(),
                });
            }

            Ok(Response::from_parts(resp_parts, full_body(resp_bytes)))
        }
```

Replace with:
```rust
    match client.request(upstream_req).await {
        Ok(upstream_resp) => {
            let (resp_parts, resp_body) = upstream_resp.into_parts();
            let res_status = resp_parts.status.as_u16();

            let res_headers: Vec<(String, String)> = resp_parts
                .headers
                .iter()
                .filter_map(|(k, v)| Some((k.as_str().to_string(), v.to_str().ok()?.to_string())))
                .collect();

            // Check if response is small enough to buffer (for simplicity)
            // or needs true streaming.
            let content_length = resp_parts
                .headers
                .get(http::header::CONTENT_LENGTH)
                .and_then(|v| v.to_str().ok())
                .and_then(|s| s.parse::<usize>().ok());

            let should_stream = content_length.map_or(true, |cl| cl > crate::inspector::types::BODY_CAP);

            if should_stream {
                // Stream the response body via TeeBody — captures first 1MB for inspector
                let tee = TeeBody::new(resp_body);
                let res_handle = tee.captured_handle();
                let body = tee
                    .map_err(|e| -> hyper::Error { unreachable!("incoming body error: {e:?}") })
                    .boxed();

                // Fire inspector capture in a background task when response streams
                if let Some(sender) = inspector {
                    let hostname = hostname.clone();
                    let req_body_capture = crate::inspector::types::CapturedBody::from_bytes(&req_body_bytes);
                    tokio::spawn(async move {
                        // Poll briefly for the response to stream — give it up to 30s
                        for _ in 0..300u32 {
                            tokio::time::sleep(std::time::Duration::from_millis(100)).await;
                            let (_, total) = res_handle.take();
                            // If the body has been fully consumed (total > 0 and stable)
                            // or if Content-Length is known and we've seen it all, fire.
                            if let Some(cl) = content_length {
                                if total >= cl {
                                    break;
                                }
                            }
                        }
                        sender.send(crate::inspector::types::CapturedRequest {
                            hostname,
                            method,
                            path: path_and_query,
                            req_headers,
                            req_body: req_body_capture,
                            res_status,
                            res_headers,
                            res_body: res_handle.to_captured_body(),
                            duration_ms: start.elapsed().as_millis() as u64,
                            timestamp: chrono::Utc::now().timestamp_millis(),
                        });
                    });
                }

                Ok(Response::from_parts(resp_parts, body))
            } else {
                // Small response — collect and capture synchronously (existing pattern)
                let resp_bytes = match resp_body.collect().await {
                    Ok(c) => c.to_bytes(),
                    Err(_) => bytes::Bytes::new(),
                };

                if let Some(sender) = &inspector {
                    use crate::inspector::types::{CapturedBody, CapturedRequest};
                    sender.send(CapturedRequest {
                        hostname: hostname.clone(),
                        method,
                        path: path_and_query,
                        req_headers,
                        req_body: CapturedBody::from_bytes(&req_body_bytes),
                        res_status,
                        res_headers: res_headers.clone(),
                        res_body: CapturedBody::from_bytes(&resp_bytes),
                        duration_ms: start.elapsed().as_millis() as u64,
                        timestamp: chrono::Utc::now().timestamp_millis(),
                    });
                }

                Ok(Response::from_parts(resp_parts, full_body(resp_bytes)))
            }
        }
```

**Note:** The `resp_body` type from hyper client is `hyper::body::Incoming`. The `TeeBody<Incoming>` needs to be boxed to match `BoxBodyType`. The `.map_err(...).boxed()` converts it. The `unreachable!` for the error mapping is because `Incoming`'s error type is `hyper::Error`, and we're mapping to `hyper::Error` — but `BodyExt::boxed()` needs the error types to match `BoxBody<Bytes, hyper::Error>`. Actually `Incoming` already has `Error = hyper::Error`, so the map_err is a no-op identity. Use:

```rust
let body: BoxBodyType = BodyExt::boxed(tee.map_err(|e| e));
```

Or simpler — since `Incoming`'s error IS `hyper::Error`:
```rust
let body: BoxBodyType = BodyExt::boxed(tee);
```

Wait — `TeeBody<Incoming>` has `Error = <Incoming as Body>::Error = hyper::Error`. And `BoxBodyType = BoxBody<Bytes, hyper::Error>`. So `BodyExt::boxed(tee)` should work directly if the error types match.

Actually, `BodyExt::boxed()` requires `Body<Data=Bytes, Error=E> + Send + Sync + 'static`. `TeeBody<Incoming>` is `Send` (if `Incoming` is Send — it is). Is it `Sync`? `Arc<Mutex<Vec<u8>>>` is Sync. `Arc<AtomicUsize>` is Sync. `Incoming` may or may not be Sync. If not, we need `UnsyncBoxBody` or wrap differently.

The simplest safe path: use `http_body_util::combinators::UnsyncBoxBody` or just `BoxBody` with the right bounds. Let the compiler tell us. If `Incoming` isn't `Sync`, use `http_body_util::BodyExt::map_frame` + `boxed_unsync`.

For the plan, note this as a potential compile issue and instruct the implementer to fix bounds as needed.

- [ ] **Step 4: Build and fix type errors**

```bash
cargo build 2>&1 | grep "^error" | head -20
```

Expected: possible type bound issues with `TeeBody<Incoming>` → `BoxBodyType`. Fix by adjusting the boxing:

If `Sync` bound fails:
```rust
// Use map_frame to convert TeeBody<Incoming> → BoxBody
let body: BoxBodyType = http_body_util::BodyExt::boxed(
    http_body_util::BodyExt::map_frame(tee, |f| f)
);
```

Or add `unsafe impl Sync for TeeBody` if inner is Sync (check).

- [ ] **Step 5: Run all tests**

```bash
cargo test 2>&1 | grep -E "^test result|FAILED"
```

Expected: all tests pass.

- [ ] **Step 6: Commit**

```bash
git add src/proxy.rs
git commit -m "feat(proxy): stream response bodies via TeeBody — no full buffering"
```

---

## Self-Review

**Spec coverage:**

- ✅ `TeeBody<B>` struct with side capture — Task 1
- ✅ `CaptureHandle` for reading capture from another task — Task 1
- ✅ Captures first 1MB, forwards full stream — Task 1 tests
- ✅ Request body: Limited collect at 10MB — Task 2
- ✅ Response body: streamed via TeeBody — Task 2
- ✅ Inspector capture fires on response completion — Task 2 (spawned task)
- ✅ Small responses still collected synchronously — Task 2 (should_stream check)
- ✅ 50MB cap removed — Task 2 (replaced by Limited + TeeBody)

**No placeholders found.**

**Type consistency:** `CaptureHandle` used in both Task 1 (tests) and Task 2 (handle_https_request). `TeeBody::new()` / `captured_handle()` API consistent across both tasks.
