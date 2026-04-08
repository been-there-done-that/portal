# TCP Proxy Design (Databases/Caches)

## Overview

`portal run` currently assumes the service speaks HTTP/HTTPS and sets up a HTTPS reverse proxy. TCP services (Redis, PostgreSQL, MySQL, memcached) don't speak HTTP — they need direct TCP access, not a proxy. A `--tcp` flag tells portal to skip the HTTPS proxy machinery and manage the process as a plain TCP service instead.

## What Changes

### `src/cli/mod.rs`

Add `#[arg(long)]` `tcp: bool` to `CliCommand::Run`. Pass it through to `do_run()` as a new `tcp: bool` parameter.

In `do_run()` when `tcp` is `true`:
- Skip `ensure_cert_trusted()` — no TLS certificate needed
- Skip `PORTAL_URL` env var — no HTTPS URL to expose
- Skip `NODE_EXTRA_CA_CERTS` env var — no Node TLS config needed
- Call `banner::print_tcp_banner(&hostname, port, pid, replaced)` instead of `banner::print_banner()`
- Everything else stays the same: ensure daemon running, hostname resolution, port allocation, process spawn, route registration, `portal stop` support

### `src/cli/banner.rs`

Add `print_tcp_banner(hostname: &str, port: u16, pid: u32, replaced: bool)`:

```text
  portal  v1.0.0  ·  ● running

  myapp.localhost  [TCP]
  └─ localhost:4001  ·  pid 91842
```

## Files

| File | Change |
|---|---|
| `src/cli/mod.rs` | Add `--tcp` flag; pass to `do_run()`; skip cert/HTTPS env when tcp=true; different banner call |
| `src/cli/banner.rs` | Add `print_tcp_banner(hostname, port, pid, replaced)` |

## Usage Examples

```bash
# Redis (specify port explicitly since Redis ignores PORT env var)
portal run --tcp --port 6379 redis-server

# Custom TCP server that respects PORT env var
portal run --tcp my-tcp-server

# PostgreSQL
portal run --tcp --port 5432 postgres -D /usr/local/var/postgresql@14
```

## Testing

- `banner.rs`: `print_tcp_banner_does_not_panic` — quiet-mode test
- `banner.rs`: `print_tcp_banner_replaced_does_not_panic` — with replaced=true
