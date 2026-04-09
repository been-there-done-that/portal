# SNI-based TCP Routing Design

**Goal:** After TLS termination on portal's HTTPS port, detect non-HTTP traffic and bridge raw bytes to the backend — enabling `psql`, `redis-cli`, and any TLS-capable client to connect via `{name}.localhost:443`.

**Architecture:** In `serve_https`, after `acceptor.accept()`, peek the first bytes of the decrypted stream. If they match an HTTP method, feed to hyper as today. Otherwise, extract the SNI hostname from the TLS connection, look up the route, and bridge with `copy_bidirectional`.

---

## Flow

```
Client (psql sslmode=require)
  │
  ▼ TLS ClientHello with SNI: my-postgres.localhost
  │
Portal (port 443)
  │ TLS terminate (rustls, same cert generation as HTTP)
  │ Peek first 4 bytes of decrypted stream
  │
  ├─ Bytes match HTTP method? → hyper HTTP/1 handler (existing path)
  │
  └─ Bytes don't match HTTP? → look up route by SNI hostname
                                 → TCP connect to localhost:{route.port}
                                 → copy_bidirectional (bridge raw bytes)
```

## HTTP Method Detection

Peek the first 4 bytes after TLS termination. HTTP requests always start with a method token followed by a space. Check if the bytes match any of:

- `GET ` / `PUT ` / `HEAD` / `POST` / `DELE` / `PATC` / `OPTI` / `CONN`

If yes → HTTP. If no → raw TCP bridge.

This is safe because no common wire protocol (Postgres, Redis, MySQL, MongoDB, gRPC) starts with these byte sequences.

## SNI Hostname Extraction

After `acceptor.accept()` returns a `tokio_rustls::server::TlsStream<TcpStream>`, the SNI hostname is available via:

```rust
let sni = tls_stream.get_ref().1.server_name()
    .map(|s| s.to_string());
```

`rustls::ServerConnection::server_name()` returns the SNI the client sent in the ClientHello.

## TCP Bridge

For non-HTTP connections:

1. Extract SNI hostname from TLS connection
2. Look up route in `StateStore` by hostname
3. If not found, drop the connection (no error response possible — not HTTP)
4. `TcpStream::connect(("127.0.0.1", route.port))` to the backend
5. `tokio::io::copy_bidirectional(&mut tls_stream, &mut backend)` to bridge bytes
6. When either side closes, the other side closes automatically

## Buffered First Bytes

After peeking, the bytes are consumed from the stream. The HTTP path needs them back. Two options:

1. **Use `tokio::io::AsyncReadExt::read` into a buffer, then chain it back** — wrap the stream in a `Chain<Cursor<Vec<u8>>, TlsStream>` before passing to hyper
2. **Use a peekable wrapper** — custom `AsyncRead` that buffers peeked bytes

Option 1 is simpler. Read 4 bytes, check, then for the HTTP path create a chained reader:

```rust
let mut peek_buf = [0u8; 4];
let n = tls_stream.read(&mut peek_buf).await?;
let peeked = &peek_buf[..n];

if is_http_method_prefix(peeked) {
    // Chain peeked bytes back + rest of stream → feed to hyper
    let chain = tokio::io::join(std::io::Cursor::new(peeked.to_vec()), tls_stream);
    // ... hyper handler with chained reader
} else {
    // TCP bridge
}
```

Actually, simpler: use `BufReader` with the peeked bytes pre-filled, or use the existing `peek` if available on TLS streams. The exact buffering approach is an implementation detail — the key requirement is that the HTTP path receives the full request including the peeked bytes.

## Files Changed

| File | Change |
|---|---|
| `src/proxy.rs` | Add `is_http_method_prefix(buf: &[u8]) -> bool` |
| `src/daemon/mod.rs` | In `serve_https` spawn block: read first bytes, branch HTTP vs TCP bridge |

## Testing

- `is_http_method_prefix(b"GET ")` → true
- `is_http_method_prefix(b"POST")` → true
- `is_http_method_prefix(b"\x00\x00")` → false (Postgres startup)
- `is_http_method_prefix(b"*3\r\n")` → false (Redis RESP)
- Integration: Docker Postgres through portal alias, `psql sslmode=require` connects successfully

## What This Enables

```bash
# Register alias
portal alias my-postgres 5432

# Connect via psql through portal's TLS
psql "host=my-postgres.localhost port=443 sslmode=require user=postgres"

# Same alias also works for HTTP tools
curl https://my-postgres.localhost  # (502 — Postgres doesn't speak HTTP, but portal tries)

# Redis
portal alias redis 6379
redis-cli --tls -h redis.localhost -p 443
```
