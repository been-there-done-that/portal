use bytes::Bytes;
use http::{Request, Response, StatusCode};
use std::sync::atomic::{AtomicUsize, Ordering};
use http_body_util::{combinators::BoxBody, BodyExt, Full};
use hyper::body::Incoming;
use hyper_util::client::legacy::{connect::HttpConnector, Client};
use hyper_util::rt::TokioExecutor;
use tokio::io::AsyncReadExt;
use tokio::io::AsyncWriteExt;
use tokio::net::TcpStream;

pub const HOP_HEADER: &str = "x-portal-hops";
pub const MAX_HOPS: u8 = 5;
pub const PORTAL_PORT_HEADER: &str = "x-portal-port";

pub type BoxBodyType = BoxBody<Bytes, hyper::Error>;

pub fn is_tls_client_hello(first_byte: u8) -> bool {
    first_byte == 0x16
}

/// Check if a byte buffer starts with an HTTP/1.x method.
/// Used after TLS termination to distinguish HTTP from raw TCP (Postgres, Redis, etc.).
/// Requires at least 3 bytes; returns false for short buffers.
pub fn is_http_method_prefix(buf: &[u8]) -> bool {
    if buf.len() < 3 {
        return false;
    }
    matches!(
        &buf[..3],
        b"GET" | b"PUT" | b"POS" | b"HEA" | b"DEL" | b"PAT" | b"OPT" | b"CON"
    )
}

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
        self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
        buf: &mut tokio::io::ReadBuf<'_>,
    ) -> std::task::Poll<std::io::Result<()>> {
        let this = self.get_mut();
        if this.pos < this.prefix.len() {
            let remaining = &this.prefix[this.pos..];
            let to_copy = remaining.len().min(buf.remaining());
            buf.put_slice(&remaining[..to_copy]);
            this.pos += to_copy;
            return std::task::Poll::Ready(Ok(()));
        }
        std::pin::Pin::new(&mut this.inner).poll_read(cx, buf)
    }
}

impl<S: tokio::io::AsyncWrite + Unpin> tokio::io::AsyncWrite for PrefixedIo<S> {
    fn poll_write(
        self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
        buf: &[u8],
    ) -> std::task::Poll<std::io::Result<usize>> {
        std::pin::Pin::new(&mut self.get_mut().inner).poll_write(cx, buf)
    }

    fn poll_flush(
        self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<std::io::Result<()>> {
        std::pin::Pin::new(&mut self.get_mut().inner).poll_flush(cx)
    }

    fn poll_shutdown(
        self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<std::io::Result<()>> {
        std::pin::Pin::new(&mut self.get_mut().inner).poll_shutdown(cx)
    }
}

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
/// the first BODY_CAP bytes in a side buffer for the inspector.
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

pub fn full_body(text: impl Into<Bytes>) -> BoxBodyType {
    Full::new(text.into())
        .map_err(|never| match never {})
        .boxed()
}

/// Peek at the first byte of a TcpStream without consuming it.
pub async fn peek_first_byte(stream: &TcpStream) -> std::io::Result<u8> {
    let mut buf = [0u8; 1];
    let n = stream.peek(&mut buf).await?;
    if n == 0 {
        return Err(std::io::Error::new(
            std::io::ErrorKind::UnexpectedEof,
            "connection closed before first byte",
        ));
    }
    Ok(buf[0])
}

/// Listen on port 80 and redirect all HTTP traffic to HTTPS.
pub async fn serve_http_redirect(
    listener: tokio::net::TcpListener,
    http_port: u16,
    https_port: u16,
) {
    loop {
        let (mut stream, _addr) = match listener.accept().await {
            Ok(pair) => pair,
            Err(_) => continue,
        };

        let https_port = https_port;
        tokio::spawn(async move {
            let mut buf = [0u8; 4096];
            let n = match stream.read(&mut buf).await {
                Ok(n) => n,
                Err(_) => return,
            };

            let request_text = String::from_utf8_lossy(&buf[..n]);

            // Parse Host header from raw HTTP request
            let host = request_text
                .lines()
                .find(|line| line.to_lowercase().starts_with("host:"))
                .and_then(|line| line.splitn(2, ':').nth(1))
                .map(|h| {
                    // Strip port if present: "myapp.localhost:8443" → "myapp.localhost"
                    let trimmed = h.trim();
                    trimmed.split(':').next().unwrap_or(trimmed).to_string()
                })
                .unwrap_or_else(|| "localhost".to_string());

            let location = if https_port == 443 {
                format!("https://{}/", host)
            } else {
                format!("https://{}:{}/", host, https_port)
            };

            let response = format!(
                "HTTP/1.1 301 Moved Permanently\r\nLocation: {}\r\n{}: {}\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
                location, PORTAL_PORT_HEADER, http_port
            );

            let _ = stream.write_all(response.as_bytes()).await;
        });
    }
}

/// Check if a request is a WebSocket upgrade.
pub fn is_websocket_upgrade<B>(req: &Request<B>) -> bool {
    req.headers()
        .get(http::header::UPGRADE)
        .and_then(|v| v.to_str().ok())
        .map(|v| v.to_lowercase().contains("websocket"))
        .unwrap_or(false)
}

/// Returns true if the request prefers HTML responses (i.e. a browser navigation).
fn wants_html(headers: &http::HeaderMap) -> bool {
    headers
        .get(http::header::ACCEPT)
        .and_then(|v| v.to_str().ok())
        .map(|v| v.contains("text/html"))
        .unwrap_or(false)
}

/// Short plain-text error for API callers (no Accept: text/html).
fn plain_error(status: http::StatusCode, msg: &str) -> Response<BoxBodyType> {
    Response::builder()
        .status(status)
        .header("content-type", "text/plain")
        .body(full_body(format!("{} {msg}", status.as_u16())))
        .unwrap()
}

/// Extract hostname from a Host or X-Forwarded-Host header value, stripping any port.
pub fn extract_host(h: Option<&http::HeaderValue>) -> String {
    h.and_then(|v| v.to_str().ok())
        .unwrap_or("")
        .split(':')
        .next()
        .unwrap_or("")
        .to_string()
}

/// Main proxy handler for HTTPS requests.
pub async fn handle_https_request(
    req: Request<Incoming>,
    routes: crate::routes::StateStore,
    inspector: Option<crate::inspector::InspectorSender>,
) -> Result<Response<BoxBodyType>, std::convert::Infallible> {
    let start = std::time::Instant::now();

    let accept_html = wants_html(req.headers());

    let hops: u8 = req
        .headers()
        .get(HOP_HEADER)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.parse().ok())
        .unwrap_or(0);

    let hostname = {
        let from_host = extract_host(req.headers().get(http::header::HOST));
        if routes.get(&from_host).is_some() {
            from_host
        } else {
            // Fallback: reverse proxies (ngrok, Cloudflare Tunnel) pass the original
            // hostname in X-Forwarded-Host when they rewrite the Host header.
            let forwarded = extract_host(req.headers().get("x-forwarded-host"));
            if !forwarded.is_empty() {
                forwarded
            } else {
                from_host
            }
        }
    };

    if hops >= MAX_HOPS {
        return Ok(if accept_html {
            Response::builder()
                .status(StatusCode::LOOP_DETECTED)
                .header("content-type", "text/html")
                .body(full_body(crate::pages::page_508(&hostname)))
                .unwrap()
        } else {
            plain_error(
                StatusCode::LOOP_DETECTED,
                &format!("loop detected proxying {hostname}"),
            )
        });
    }

    let route = match routes.get(&hostname) {
        Some(r) => r,
        None => {
            return Ok(if accept_html {
                Response::builder()
                    .status(StatusCode::NOT_FOUND)
                    .header("content-type", "text/html")
                    .body(full_body(crate::pages::page_404(&hostname)))
                    .unwrap()
            } else {
                plain_error(
                    StatusCode::NOT_FOUND,
                    &format!("no route registered for {hostname}"),
                )
            });
        }
    };

    if is_websocket_upgrade(&req) {
        // WebSocket upgrade errors are always plain-text — they bypass the accept_html branch
        // because WebSocket clients never send Accept: text/html
        return handle_websocket(req, route.port).await;
    }

    let port = route.port;
    let method = req.method().to_string();
    let path_and_query = req
        .uri()
        .path_and_query()
        .map(|pq| pq.as_str())
        .unwrap_or("/")
        .to_string();

    let (mut parts, body) = req.into_parts();

    let req_headers: Vec<(String, String)> = parts
        .headers
        .iter()
        .filter_map(|(k, v)| Some((k.as_str().to_string(), v.to_str().ok()?.to_string())))
        .collect();

    // Collect request body — capped at 10MB (request bodies in dev are typically small)
    let req_body_bytes = match http_body_util::Limited::new(body, 10 * 1024 * 1024)
        .collect()
        .await
    {
        Ok(c) => c.to_bytes(),
        Err(_) => {
            return Ok(if accept_html {
                Response::builder()
                    .status(StatusCode::BAD_GATEWAY)
                    .header("content-type", "text/html")
                    .body(full_body(crate::pages::page_502(&hostname)))
                    .unwrap()
            } else {
                plain_error(
                    StatusCode::BAD_GATEWAY,
                    &format!("{hostname} → port {port} unreachable"),
                )
            });
        }
    };

    parts.uri = match format!("http://127.0.0.1:{}{}", port, path_and_query).parse() {
        Ok(u) => u,
        Err(_) => {
            return Ok(Response::builder()
                .status(StatusCode::BAD_GATEWAY)
                .header("content-type", "text/html")
                .body(full_body(crate::pages::page_502(&hostname)))
                .unwrap());
        }
    };
    parts
        .headers
        .insert(HOP_HEADER, (hops + 1).to_string().parse().unwrap());
    parts
        .headers
        .insert("x-forwarded-proto", "https".parse().unwrap());

    // Reuse a shared HTTP client for connection pooling and keep-alive
    static HTTP_CLIENT: std::sync::OnceLock<Client<HttpConnector, BoxBodyType>> =
        std::sync::OnceLock::new();
    let client = HTTP_CLIENT.get_or_init(|| Client::builder(TokioExecutor::new()).build_http());
    let upstream_req = Request::from_parts(parts, full_body(req_body_bytes.clone()));

    match client.request(upstream_req).await {
        Ok(upstream_resp) => {
            let (resp_parts, resp_body) = upstream_resp.into_parts();
            let res_status = resp_parts.status.as_u16();

            let res_headers: Vec<(String, String)> = resp_parts
                .headers
                .iter()
                .filter_map(|(k, v)| Some((k.as_str().to_string(), v.to_str().ok()?.to_string())))
                .collect();

            // Decide: stream large responses via TeeBody, collect small ones
            let content_length = resp_parts
                .headers
                .get(http::header::CONTENT_LENGTH)
                .and_then(|v| v.to_str().ok())
                .and_then(|s| s.parse::<usize>().ok());

            let should_stream = content_length.map_or(true, |cl| cl > crate::inspector::types::BODY_CAP);

            if should_stream {
                // Stream response body — capture first 1MB for inspector asynchronously
                let tee = TeeBody::new(resp_body);
                let res_handle = tee.captured_handle();

                // Box the TeeBody into our standard body type
                let boxed_body: BoxBodyType = http_body_util::BodyExt::boxed(tee);

                // Fire inspector capture in background after response streams
                if let Some(sender) = inspector {
                    let hostname = hostname.clone();
                    let req_body_capture = crate::inspector::types::CapturedBody::from_bytes(&req_body_bytes);
                    tokio::spawn(async move {
                        // Wait for the response body to be consumed (up to 30s)
                        let mut last_total = 0usize;
                        let mut stable_count = 0u32;
                        for _ in 0..300u32 {
                            tokio::time::sleep(std::time::Duration::from_millis(100)).await;
                            let (_, total) = res_handle.take();
                            if let Some(cl) = content_length {
                                if total >= cl { break; }
                            }
                            if total > 0 && total == last_total {
                                stable_count += 1;
                                if stable_count >= 5 { break; } // no progress for 500ms
                            } else {
                                stable_count = 0;
                            }
                            last_total = total;
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

                Ok(Response::from_parts(resp_parts, boxed_body))
            } else {
                // Small response — collect synchronously (existing pattern)
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
        Err(_) => Ok(if accept_html {
            Response::builder()
                .status(StatusCode::BAD_GATEWAY)
                .header("content-type", "text/html")
                .body(full_body(crate::pages::page_502(&hostname)))
                .unwrap()
        } else {
            plain_error(
                StatusCode::BAD_GATEWAY,
                &format!("{hostname} → port {port} unreachable"),
            )
        }),
    }
}

/// Handle a WebSocket upgrade by forwarding the full HTTP upgrade request to upstream,
/// verifying the 101 response, then bridging connections bidirectionally.
/// Host header is rewritten to localhost:{route_port} (required by Bun/Next.js HMR).
async fn handle_websocket<B>(
    req: Request<B>,
    route_port: u16,
) -> Result<Response<BoxBodyType>, std::convert::Infallible>
where
    B: hyper::body::Body + Send + 'static,
    B::Data: Send,
    B::Error: Into<Box<dyn std::error::Error + Send + Sync>>,
{
    // 1. Connect to upstream
    let upstream_addr = format!("127.0.0.1:{route_port}");
    let mut upstream = match TcpStream::connect(&upstream_addr).await {
        Ok(s) => s,
        Err(_) => {
            return Ok(Response::builder()
                .status(StatusCode::BAD_GATEWAY)
                .header("content-type", "text/plain")
                .body(full_body("502 Bad Gateway: upstream connection failed"))
                .unwrap());
        }
    };

    // 2. Build and forward HTTP upgrade request with Host rewritten
    let path = req
        .uri()
        .path_and_query()
        .map(|p| p.as_str())
        .unwrap_or("/")
        .to_string();
    let method = req.method().as_str().to_string();

    let mut upgrade_req = format!("{method} {path} HTTP/1.1\r\n");
    upgrade_req.push_str(&format!("host: localhost:{route_port}\r\n"));
    for (k, v) in req.headers() {
        if k == http::header::HOST {
            continue; // already wrote rewritten Host above
        }
        if let Ok(v_str) = v.to_str() {
            upgrade_req.push_str(&format!("{}: {v_str}\r\n", k.as_str()));
        }
    }
    upgrade_req.push_str("\r\n");

    if upstream.write_all(upgrade_req.as_bytes()).await.is_err() {
        return Ok(Response::builder()
            .status(StatusCode::BAD_GATEWAY)
            .header("content-type", "text/plain")
            .body(full_body("502 Bad Gateway: upstream write failed"))
            .unwrap());
    }

    // 3. Read upstream's 101 response (loop until \r\n\r\n received)
    let mut buf = Vec::with_capacity(1024);
    let mut tmp = [0u8; 256];
    loop {
        match upstream.read(&mut tmp).await {
            Ok(0) | Err(_) => {
                return Ok(Response::builder()
                    .status(StatusCode::BAD_GATEWAY)
                    .header("content-type", "text/plain")
                    .body(full_body(
                        "502 Bad Gateway: upstream did not respond to WebSocket upgrade",
                    ))
                    .unwrap());
            }
            Ok(n) => {
                buf.extend_from_slice(&tmp[..n]);
                if buf.windows(4).any(|w| w == b"\r\n\r\n") {
                    break;
                }
                if buf.len() > 8192 {
                    // Response header too large — abort
                    return Ok(Response::builder()
                        .status(StatusCode::BAD_GATEWAY)
                        .header("content-type", "text/plain")
                        .body(full_body("502 Bad Gateway: upstream response too large"))
                        .unwrap());
                }
            }
        }
    }
    // Split header bytes from any post-header data that arrived in the same read
    let header_end = buf.windows(4).position(|w| w == b"\r\n\r\n")
        .map(|pos| pos + 4)
        .unwrap_or(buf.len());
    let post_header_data = buf[header_end..].to_vec();
    let response_head = String::from_utf8_lossy(&buf[..header_end]);
    if !response_head.starts_with("HTTP/1.1 101") {
        return Ok(Response::builder()
            .status(StatusCode::BAD_GATEWAY)
            .header("content-type", "text/plain")
            .body(full_body(
                "502 Bad Gateway: upstream rejected WebSocket upgrade",
            ))
            .unwrap());
    }

    // 4. Return 101 to client and bridge the two connections
    // Parse and forward upstream 101 response headers to the client
    let mut resp_builder = Response::builder().status(StatusCode::SWITCHING_PROTOCOLS);
    // Extract the headers section (after the status line, before \r\n\r\n)
    if let Some(headers_str) = response_head.splitn(2, "\r\n").nth(1) {
        let headers_section = headers_str.splitn(2, "\r\n\r\n").next().unwrap_or("");
        for line in headers_section.lines() {
            if let Some((name, value)) = line.split_once(": ") {
                let name_lower = name.to_lowercase();
                // Forward upgrade-relevant headers
                if matches!(
                    name_lower.as_str(),
                    "upgrade"
                        | "connection"
                        | "sec-websocket-accept"
                        | "sec-websocket-protocol"
                        | "sec-websocket-extensions"
                ) {
                    if let Ok(val) = http::HeaderValue::from_str(value) {
                        resp_builder = resp_builder.header(name_lower, val);
                    }
                }
            }
        }
    }
    let resp = resp_builder.body(full_body("")).unwrap();

    tokio::spawn(async move {
        match hyper::upgrade::on(req).await {
            Ok(upgraded) => {
                let mut client_io = hyper_util::rt::TokioIo::new(upgraded);
                // Forward any bytes that arrived after the \r\n\r\n delimiter
                if !post_header_data.is_empty() {
                    if client_io.write_all(&post_header_data).await.is_err() {
                        return;
                    }
                }
                let _ = tokio::io::copy_bidirectional(&mut client_io, &mut upstream).await;
            }
            Err(e) => {
                tracing::warn!("WebSocket upgrade failed: {e}");
            }
        }
    });

    Ok(resp)
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::io::AsyncWriteExt;

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
        assert!(!is_http_method_prefix(&[0x00, 0x00, 0x00, 0x08]));
    }

    #[test]
    fn http_method_prefix_rejects_redis() {
        assert!(!is_http_method_prefix(b"*3\r\n"));
    }

    #[test]
    fn http_method_prefix_rejects_mysql() {
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

    #[test]
    fn tls_byte_is_0x16() {
        assert!(is_tls_client_hello(0x16));
        assert!(!is_tls_client_hello(b'G'));
        assert!(!is_tls_client_hello(0x00));
    }

    #[tokio::test]
    async fn http_redirect_listener_sends_301() {
        use tokio::io::AsyncReadExt;
        use tokio::net::TcpListener;

        // Bind to a random port
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();

        // Spawn the redirect server
        tokio::spawn(serve_http_redirect(listener, port, 443));

        // Give the server a moment to start
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;

        // Connect and send an HTTP request
        let mut client = tokio::net::TcpStream::connect(format!("127.0.0.1:{}", port))
            .await
            .unwrap();

        let request = "GET / HTTP/1.1\r\nHost: myapp.localhost\r\nConnection: close\r\n\r\n";
        client.write_all(request.as_bytes()).await.unwrap();

        let mut response = String::new();
        client.read_to_string(&mut response).await.unwrap();

        assert!(
            response.contains("301"),
            "Expected 301 in response: {}",
            response
        );
        assert!(
            response.contains("https://"),
            "Expected https:// in response: {}",
            response
        );
    }

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

    #[test]
    fn wants_html_returns_true_for_browser_accept() {
        let mut headers = http::HeaderMap::new();
        headers.insert(
            http::header::ACCEPT,
            "text/html,application/xhtml+xml,application/xml;q=0.9,*/*;q=0.8"
                .parse()
                .unwrap(),
        );
        assert!(wants_html(&headers));
    }

    #[test]
    fn wants_html_returns_false_for_json_accept() {
        let mut headers = http::HeaderMap::new();
        headers.insert(http::header::ACCEPT, "application/json".parse().unwrap());
        assert!(!wants_html(&headers));
    }

    #[test]
    fn wants_html_returns_false_when_no_accept_header() {
        let headers = http::HeaderMap::new();
        assert!(!wants_html(&headers));
    }

    #[test]
    fn websocket_upgrade_is_detected() {
        // Build request with Upgrade: websocket header
        let req_with_upgrade = Request::builder()
            .uri("/ws")
            .header(http::header::UPGRADE, "websocket")
            .header(http::header::CONNECTION, "Upgrade")
            .body(())
            .unwrap();

        assert!(is_websocket_upgrade(&req_with_upgrade));

        // Build request without Upgrade header
        let req_without_upgrade = Request::builder().uri("/").body(()).unwrap();

        assert!(!is_websocket_upgrade(&req_without_upgrade));
    }

    #[test]
    fn extract_host_strips_port() {
        let val = http::HeaderValue::from_static("myapp.localhost:443");
        assert_eq!(extract_host(Some(&val)), "myapp.localhost");
    }

    #[test]
    fn extract_host_returns_empty_on_none() {
        assert_eq!(extract_host(None), "");
    }

    #[tokio::test]
    async fn tee_body_captures_and_forwards_full_body() {
        use http_body_util::BodyExt;
        let data = bytes::Bytes::from(vec![b'x'; 500]);
        let original = full_body(data.clone());
        let tee = TeeBody::new(original);
        let handle = tee.captured_handle();
        let collected = tee.collect().await.unwrap().to_bytes();
        assert_eq!(collected.len(), 500);
        let (prefix, total) = handle.take();
        assert_eq!(prefix.len(), 500);
        assert_eq!(total, 500);
    }

    #[tokio::test]
    async fn tee_body_truncates_capture_at_body_cap() {
        use http_body_util::BodyExt;
        let size = crate::inspector::types::BODY_CAP + 1000;
        let data = bytes::Bytes::from(vec![b'y'; size]);
        let original = full_body(data);
        let tee = TeeBody::new(original);
        let handle = tee.captured_handle();
        let collected = tee.collect().await.unwrap().to_bytes();
        assert_eq!(collected.len(), size);
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

    #[tokio::test]
    async fn tee_body_captured_body_truncated() {
        use http_body_util::BodyExt;
        let size = crate::inspector::types::BODY_CAP + 500;
        let data = bytes::Bytes::from(vec![b'z'; size]);
        let original = full_body(data);
        let tee = TeeBody::new(original);
        let handle = tee.captured_handle();
        let _ = tee.collect().await.unwrap();
        let captured = handle.to_captured_body();
        assert!(captured.is_truncated());
        assert_eq!(captured.total_bytes(), size);
        assert_eq!(captured.prefix_bytes().len(), crate::inspector::types::BODY_CAP);
    }

    #[tokio::test]
    async fn websocket_host_header_is_rewritten() {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};

        // Minimal upstream that expects an HTTP upgrade request and responds with 101
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();

        tokio::spawn(async move {
            if let Ok((mut stream, _)) = listener.accept().await {
                let mut buf = vec![0u8; 1024];
                if let Ok(n) = stream.read(&mut buf).await {
                    let req_str = String::from_utf8_lossy(&buf[..n]);
                    // Verify Host was rewritten to localhost:{port}
                    assert!(
                        req_str.contains(&format!("host: localhost:{port}")),
                        "Host header not rewritten. Got:\n{req_str}"
                    );
                    stream.write_all(
                        b"HTTP/1.1 101 Switching Protocols\r\nUpgrade: websocket\r\nConnection: Upgrade\r\n\r\n"
                    ).await.ok();
                }
                tokio::time::sleep(std::time::Duration::from_millis(200)).await;
            }
        });

        tokio::time::sleep(std::time::Duration::from_millis(10)).await;

        // Build a WebSocket upgrade request — Host is myapp.localhost (will be rewritten)
        let req = http::Request::builder()
            .method("GET")
            .uri("/_next/webpack-hmr")
            .header(http::header::HOST, "myapp.localhost")
            .header(http::header::UPGRADE, "websocket")
            .header(http::header::CONNECTION, "Upgrade")
            .header("Sec-WebSocket-Key", "dGhlIHNhbXBsZSBub25jZQ==")
            .header("Sec-WebSocket-Version", "13")
            .body(http_body_util::Empty::<bytes::Bytes>::new())
            .unwrap();

        let resp = handle_websocket(req, port).await.unwrap();
        assert_eq!(resp.status(), http::StatusCode::SWITCHING_PROTOCOLS);
    }
}
