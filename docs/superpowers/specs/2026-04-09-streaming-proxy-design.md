# Streaming Proxy Design

**Goal:** Replace full body buffering in `handle_https_request` with a streaming `TeeBody` that forwards chunks at wire speed while capturing the first 1MB for the inspector.

**Architecture:** New `TeeBody<B>` struct in `src/proxy.rs` that wraps any `hyper::body::Body`. Each polled data frame is forwarded to the consumer and simultaneously copied into a side buffer (up to `BODY_CAP` = 1MB). When the stream completes, a completion callback fires the `CapturedRequest` to the inspector. One file changed.

---

## Problem

`handle_https_request` calls `body.collect().await` on both request and response bodies. This buffers the entire body in memory before forwarding. A 2GB file upload allocates 2GB of RAM. The 50MB cap we added truncates silently, which corrupts the data being proxied.

## Solution: `TeeBody<B>`

### Struct

```rust
pub struct TeeBody<B> {
    inner: B,
    captured: Arc<Mutex<Vec<u8>>>,
    total_bytes: Arc<AtomicUsize>,
    cap: usize,
}
```

- `inner`: the original body stream
- `captured`: side buffer accumulating the first `cap` bytes (shared via Arc for the inspector)
- `total_bytes`: total bytes seen (even past cap, for the truncation metadata)
- `cap`: defaults to `BODY_CAP` (1MB)

### Body trait implementation

```rust
impl<B: hyper::body::Body<Data = Bytes, Error = E>> hyper::body::Body for TeeBody<B> {
    fn poll_frame(...) -> Poll<Option<Result<Frame<Bytes>, E>>> {
        match inner.poll_frame(cx) {
            Poll::Ready(Some(Ok(frame))) => {
                if let Some(data) = frame.data_ref() {
                    total_bytes += data.len();
                    if captured.len() < cap {
                        let room = cap - captured.len();
                        captured.extend_from_slice(&data[..data.len().min(room)]);
                    }
                }
                Poll::Ready(Some(Ok(frame)))  // forward unchanged
            }
            other => other  // forward EOF, errors, trailers
        }
    }
}
```

Each chunk flows through at wire speed. The side buffer only accumulates the first 1MB. After 1MB, chunks still flow but nothing is copied.

### Taking the capture

```rust
impl<B> TeeBody<B> {
    pub fn new(inner: B) -> Self { ... }

    /// Extract the captured prefix after the body is fully consumed.
    /// Returns (prefix_bytes, total_bytes_seen).
    pub fn take_captured(&self) -> (Bytes, usize) {
        let buf = self.captured.lock().unwrap_or_else(|e| e.into_inner());
        (Bytes::from(buf.clone()), self.total_bytes.load(Ordering::Relaxed))
    }
}
```

### New request flow

```
1. Extract request headers
2. Wrap request body: let req_tee = TeeBody::new(body)
3. Build upstream request with req_tee as body
4. Send to backend via shared HTTP client
5. Get response
6. Extract response headers + status
7. Take request capture: let (req_prefix, req_total) = req_tee.take_captured()
8. Wrap response body: let res_tee = Arc::new(TeeBody::new(resp_body))
9. Build a response body that:
   a. Streams res_tee to the client
   b. On completion, fires inspector capture with both req + res data
10. Return response immediately — client gets chunks as they arrive
```

### Inspector capture on response completion

The response body is consumed by the client (hyper drains it). We need to fire the inspector capture after the last chunk. Two approaches:

**Approach: Wrap in a completion-sensing body.** Create `InspectorBody<B>` that wraps the response `TeeBody`. When `poll_frame` returns `None` (stream done), it sends the `CapturedRequest` to the inspector channel before returning `None`.

```rust
struct InspectorBody {
    inner: TeeBody<Incoming>,
    req_tee: TeeBody<...>,  // already consumed, but holds the capture
    // ... all metadata needed for CapturedRequest
    inspector: Option<InspectorSender>,
    fired: bool,
}

impl Body for InspectorBody {
    fn poll_frame(...) {
        match inner.poll_frame(cx) {
            Poll::Ready(None) if !fired => {
                // Stream done — send capture
                fired = true;
                let (req_prefix, req_total) = req_captures...;
                let (res_prefix, res_total) = inner.take_captured();
                inspector.send(CapturedRequest { ... });
                Poll::Ready(None)
            }
            other => other
        }
    }
}
```

This is clean but complex. Simpler alternative: since `handle_https_request` already awaits the full request body (it needs to forward it to the backend), we can keep request-side buffering simple and only stream the response.

### Simplified approach

**Request body:** Still collect, but with a streaming cap. Use `http_body_util::Limited` to limit to 50MB for the collect (prevents OOM), but also capture first 1MB for inspector. This is acceptable because request bodies in dev are typically small (JSON payloads, form data). Multi-GB uploads are rare in local dev.

**Response body:** This is where streaming matters most (large JS bundles, images, video). Wrap in `TeeBody` and return directly. The inspector capture fires when the client finishes draining.

This halves the complexity while solving the real problem (response body OOM).

Actually — even simpler: for the request side, hyper's client accepts a streaming body. We can wrap the incoming body in `TeeBody`, pass it directly to the client, then read the capture after the client request resolves. The client consumes the body by streaming it to the backend. We never buffer it.

### Final design (both sides streaming)

```
Request:
  incoming body → TeeBody(body) → hyper client → backend
                     ↓ (side buffer, ≤1MB)
                  req_captured

Response:
  backend response body → TeeBody(resp_body) → InspectorBody → client
                              ↓ (side buffer, ≤1MB)
                           res_captured → inspector on completion
```

## HTTP Client type change

The shared `HTTP_CLIENT` currently has type `Client<HttpConnector, BoxBodyType>`. `BoxBodyType` is `BoxBody<Bytes, hyper::Error>`. To accept `TeeBody` as the request body, the client needs to be generic over the body type, or we box the `TeeBody`.

Simplest: box the `TeeBody` into `BoxBodyType` before sending. This adds one allocation but preserves the shared client type.

## Files Changed

| File | Change |
|---|---|
| `src/proxy.rs` | Add `TeeBody<B>` + `InspectorBody`. Refactor `handle_https_request` to stream both request and response bodies. Remove 50MB cap. |

## Testing

- `TeeBody` captures first 1MB of a 2MB stream, forwards all 2MB
- `TeeBody` captures full body when under 1MB
- `TeeBody` reports correct `total_bytes` even when truncated
- `InspectorBody` fires capture on stream completion
- Existing proxy tests still pass (they use small bodies)
