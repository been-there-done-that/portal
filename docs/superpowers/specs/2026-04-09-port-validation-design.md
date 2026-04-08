# Port Input Validation Design

## Overview

`portal run --port <N>` currently accepts any port without validation. If the user passes a browser-blocked port (e.g. 6000) or a privileged port (< 1024), the dev server starts but the proxy silently fails or browsers refuse to connect. This adds early validation with a clear error message.

Auto-assignment via `find_free_port` already skips blocked and privileged ports — this feature closes the gap for explicit port inputs only.

## What Changes

### `src/ports.rs`

Add one new public function:

```rust
pub fn validate_app_port(port: u16) -> Result<()>
```

Checks in order:
1. Port < 1024 → `Error::InvalidPort("port {port} is a privileged port (< 1024)")`
2. `is_browser_blocked(port)` → `Error::InvalidPort("port {port} is blocked by browsers — see https://fetch.spec.whatwg.org/#bad-port")`
3. Otherwise → `Ok(())`

Uses the existing `BLOCKED_PORTS` list and `is_browser_blocked()` — no new data.

### `src/cli/mod.rs`

In `do_run()`, where `explicit_port` is resolved (~line 375), call `validate_app_port(explicit_port)?` before any stop/reuse logic. On error, propagate through the `Result<()>` return — the caller (`run()`) prints the error and exits via the existing error-handling path.

## Error Handling

Errors surface as `crate::error::Error::InvalidPort(String)` — a new variant added to `src/error.rs`. The existing `impl fmt::Display for Error` will print it as `"invalid port: {message}"`. The CLI top-level returns `Result<()>` so the message reaches stderr naturally.

## Files

| File | Change |
|---|---|
| `src/error.rs` | Add `InvalidPort(String)` variant |
| `src/ports.rs` | Add `validate_app_port(port: u16) -> Result<()>` + 4 unit tests |
| `src/cli/mod.rs` | Call `validate_app_port(explicit_port)?` in `do_run()` |

## Testing

Unit tests in `src/ports.rs`:
- `validate_app_port(6000)` → `Err` (browser-blocked)
- `validate_app_port(6665)` through `validate_app_port(6669)` → all `Err`
- `validate_app_port(80)` → `Err` (privileged)
- `validate_app_port(4000)` → `Ok`
- `validate_app_port(1023)` → `Err` (privileged boundary)
- `validate_app_port(1024)` → `Ok` (first valid port)
