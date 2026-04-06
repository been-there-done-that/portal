use bytes::Bytes;
use http::{Request, Response, StatusCode};
use http_body_util::{combinators::BoxBody, BodyExt, Full};
use hyper::body::Incoming;
use hyper_util::client::legacy::{connect::HttpConnector, Client};
use hyper_util::rt::TokioExecutor;
use tokio::io::AsyncReadExt;
use tokio::io::AsyncWriteExt;
use tokio::net::TcpStream;

pub const HOP_HEADER: &str = "x-portless-hops";
pub const MAX_HOPS: u8 = 5;

pub type BoxBodyType = BoxBody<Bytes, hyper::Error>;

pub fn is_tls_client_hello(first_byte: u8) -> bool {
    first_byte == 0x16
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
pub async fn serve_http_redirect(listener: tokio::net::TcpListener, https_port: u16) {
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
                .map(|h| h.trim().to_string())
                .unwrap_or_else(|| "localhost".to_string());

            let location = if https_port == 443 {
                format!("https://{}/", host)
            } else {
                format!("https://{}:{}/", host, https_port)
            };

            let response = format!(
                "HTTP/1.1 301 Moved Permanently\r\nLocation: {}\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
                location
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

/// Main proxy handler for HTTPS requests.
pub async fn handle_https_request(
    req: Request<Incoming>,
    routes: crate::routes::RouteStore,
) -> Result<Response<BoxBodyType>, std::convert::Infallible> {
    // 1. Check hop counter
    let hops: u8 = req
        .headers()
        .get(HOP_HEADER)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.parse().ok())
        .unwrap_or(0);

    // Extract hostname from Host header
    let hostname = req
        .headers()
        .get(http::header::HOST)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("")
        .to_string();

    // Strip port from hostname if present
    let hostname = hostname.split(':').next().unwrap_or("").to_string();

    if hops >= MAX_HOPS {
        let body = crate::pages::page_508(&hostname);
        let resp = Response::builder()
            .status(StatusCode::LOOP_DETECTED)
            .header("content-type", "text/html")
            .body(full_body(body))
            .unwrap();
        return Ok(resp);
    }

    // 3. Route lookup
    let route = match routes.get(&hostname) {
        Some(r) => r,
        None => {
            let body = crate::pages::page_404(&hostname);
            let resp = Response::builder()
                .status(StatusCode::NOT_FOUND)
                .header("content-type", "text/html")
                .body(full_body(body))
                .unwrap();
            return Ok(resp);
        }
    };

    // 4. WebSocket upgrade check
    if is_websocket_upgrade(&req) {
        return handle_websocket(req, route.port).await;
    }

    // 5. Forward via hyper client
    let port = route.port;

    // Build upstream URI
    let path_and_query = req
        .uri()
        .path_and_query()
        .map(|pq| pq.as_str())
        .unwrap_or("/")
        .to_string();

    let upstream_uri = format!("http://127.0.0.1:{}{}", port, path_and_query);

    let (mut parts, body) = req.into_parts();

    // Update URI
    parts.uri = match upstream_uri.parse() {
        Ok(u) => u,
        Err(_) => {
            let resp_body = crate::pages::page_502(&hostname);
            let resp = Response::builder()
                .status(StatusCode::BAD_GATEWAY)
                .header("content-type", "text/html")
                .body(full_body(resp_body))
                .unwrap();
            return Ok(resp);
        }
    };

    // Increment hop header
    let new_hops = hops + 1;
    parts
        .headers
        .insert(HOP_HEADER, new_hops.to_string().parse().unwrap());

    // Add X-Forwarded-Proto
    parts
        .headers
        .insert("x-forwarded-proto", "https".parse().unwrap());

    let upstream_req = Request::from_parts(parts, body);

    let client: Client<HttpConnector, Incoming> =
        Client::builder(TokioExecutor::new()).build_http();

    match client.request(upstream_req).await {
        Ok(upstream_resp) => {
            let (resp_parts, resp_body) = upstream_resp.into_parts();
            let boxed = resp_body.map_err(|e| e).boxed();
            let resp = Response::from_parts(resp_parts, boxed);
            Ok(resp)
        }
        Err(_) => {
            let body = crate::pages::page_502(&hostname);
            let resp = Response::builder()
                .status(StatusCode::BAD_GATEWAY)
                .header("content-type", "text/html")
                .body(full_body(body))
                .unwrap();
            Ok(resp)
        }
    }
}

/// Handle a WebSocket upgrade request by proxying bidirectionally.
async fn handle_websocket(
    req: Request<Incoming>,
    route_port: u16,
) -> Result<Response<BoxBodyType>, std::convert::Infallible> {
    // 1. Connect to upstream
    let upstream_addr = format!("127.0.0.1:{}", route_port);
    let upstream = match TcpStream::connect(&upstream_addr).await {
        Ok(s) => s,
        Err(_) => {
            let resp = Response::builder()
                .status(StatusCode::BAD_GATEWAY)
                .header("content-type", "text/plain")
                .body(full_body("502 Bad Gateway: upstream connection failed"))
                .unwrap();
            return Ok(resp);
        }
    };

    // 3. Build 101 Switching Protocols response
    let resp = Response::builder()
        .status(StatusCode::SWITCHING_PROTOCOLS)
        .header(http::header::UPGRADE, "websocket")
        .header(http::header::CONNECTION, "Upgrade")
        .body(full_body(""))
        .unwrap();

    // 4. Spawn task to perform bidirectional copy after upgrade
    tokio::spawn(async move {
        match hyper::upgrade::on(req).await {
            Ok(upgraded) => {
                let mut client_io = hyper_util::rt::TokioIo::new(upgraded);
                let mut upstream_io = upstream;
                let _ = tokio::io::copy_bidirectional(&mut client_io, &mut upstream_io).await;
            }
            Err(e) => {
                tracing::warn!("WebSocket upgrade failed: {}", e);
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
        tokio::spawn(serve_http_redirect(listener, 443));

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
}
