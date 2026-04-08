# pf Redirects Fix Design

## Overview

On macOS, users sometimes configure `pf` (packet filter) `rdr` rules to forward a privileged port (e.g. 80) to portal's actual listen port (e.g. 8080). This means connections to port 80 arrive at 8080 transparently. The risk: an external health checker making an HTTP request to port 80 may receive a valid-looking 301 redirect *through the pf redirect* even when portal is not the process actually bound to port 80.

The Rust daemon already uses Unix sockets (`portal.sock`) for its own "is daemon running?" check, which is immune to pf redirects. This fix adds `X-Portal-Port: <listen-port>` to `serve_http_redirect` responses so that external tools and future health checks can distinguish direct portal responses from pf-redirected ones.

## What Changes

### `src/proxy.rs`

**Add constant:**
```rust
pub const PORTAL_PORT_HEADER: &str = "x-portal-port";
```

**Update `serve_http_redirect` signature:**
```rust
pub async fn serve_http_redirect(
    listener: tokio::net::TcpListener,
    http_port: u16,
    https_port: u16,
)
```

The `http_port` is the port `listener` is actually bound to (e.g. 80 or 8080). The 301 response gains a new header:

```
X-Portal-Port: 80
```

Full response becomes:
```
HTTP/1.1 301 Moved Permanently\r\n
Location: https://...\r\n
X-Portal-Port: 80\r\n
Content-Length: 0\r\n
Connection: close\r\n
\r\n
```

### `src/daemon/mod.rs`

Update the `serve_http_redirect` call to pass `config.proxy.http_port`:

```rust
tokio::spawn(serve_http_redirect(http_listener, config.proxy.http_port, http_https_port));
```

## Files

| File | Change |
|---|---|
| `src/proxy.rs` | Add `PORTAL_PORT_HEADER` constant; add `http_port: u16` param to `serve_http_redirect`; insert header in 301 response |
| `src/daemon/mod.rs` | Pass `config.proxy.http_port` to `serve_http_redirect` |

## Testing

Unit tests in `src/proxy.rs`:
- Existing `http_redirect_listener_sends_301` — update call signature (pass `http_port`)
- New `http_redirect_includes_portal_port_header` — verify `x-portal-port: 80` present in response
- New `http_redirect_portal_port_matches_listen_port` — verify value equals the actual listen port, not the https port
