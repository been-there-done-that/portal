# Port Input Validation Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Validate explicit `--port` inputs early so `portal run --port 6000` (or any browser-blocked/privileged port) fails immediately with a clear error instead of silently misbehaving at runtime.

**Architecture:** Add `InvalidPort(String)` to the existing `Error` enum in `src/error.rs`, add `validate_app_port(u16) -> Result<()>` to `src/ports.rs` reusing the existing `BLOCKED_PORTS` list, then call it inside `do_run()` in `src/cli/mod.rs` as the first thing when `port_override` is `Some`.

**Tech Stack:** Rust, `thiserror` (already in use for `Error` enum).

---

## File Map

| File | Change |
|---|---|
| `src/error.rs` | Add `InvalidPort(String)` variant |
| `src/ports.rs` | Add `pub fn validate_app_port(port: u16) -> Result<()>` + 6 unit tests |
| `src/cli/mod.rs` | Add `crate::ports::validate_app_port(explicit_port)?;` in `do_run()` |

---

### Task 1: Add `InvalidPort` error variant and `validate_app_port` function

**Files:**
- Modify: `src/error.rs`
- Modify: `src/ports.rs`

- [ ] **Step 1: Write failing tests in `src/ports.rs`**

Add this test module content to the existing `#[cfg(test)]` block in `src/ports.rs` (after the existing `skips_browser_blocked_ports` test):

```rust
    #[test]
    fn validate_rejects_privileged_port() {
        let err = validate_app_port(80).unwrap_err();
        assert!(err.to_string().contains("privileged"));
    }

    #[test]
    fn validate_rejects_privileged_boundary() {
        let err = validate_app_port(1023).unwrap_err();
        assert!(err.to_string().contains("privileged"));
    }

    #[test]
    fn validate_accepts_port_1024() {
        assert!(validate_app_port(1024).is_ok());
    }

    #[test]
    fn validate_rejects_browser_blocked_port_6000() {
        let err = validate_app_port(6000).unwrap_err();
        assert!(err.to_string().contains("blocked"));
    }

    #[test]
    fn validate_rejects_browser_blocked_irc_ports() {
        for port in [6665u16, 6666, 6667, 6668, 6669] {
            let err = validate_app_port(port).unwrap_err();
            assert!(err.to_string().contains("blocked"), "port {port} should be blocked");
        }
    }

    #[test]
    fn validate_accepts_normal_port() {
        assert!(validate_app_port(4000).is_ok());
        assert!(validate_app_port(3000).is_ok());
        assert!(validate_app_port(8080).is_ok());
    }
```

- [ ] **Step 2: Run tests to verify they fail**

```bash
cargo test --bin portal -- ports::tests::validate 2>&1 | tail -10
```

Expected: compile error (`validate_app_port` not found) — confirms tests reference a real missing function.

- [ ] **Step 3: Add `InvalidPort` variant to `src/error.rs`**

In `src/error.rs`, after the `HostNotFound` variant, add:

```rust
    #[error("invalid port: {0}")]
    InvalidPort(String),
```

The full enum now ends with:

```rust
    #[error("Hostname not found: {0}")]
    HostNotFound(String),

    #[error("IPC error: {0}")]
    Ipc(String),

    #[error("invalid port: {0}")]
    InvalidPort(String),
```

- [ ] **Step 4: Add `validate_app_port` to `src/ports.rs`**

After the `is_browser_blocked` function (line 16), add:

```rust
/// Validate an explicitly provided app port.
/// Returns `Err(InvalidPort)` if the port is privileged (< 1024) or browser-blocked.
pub fn validate_app_port(port: u16) -> Result<()> {
    if port < 1024 {
        return Err(crate::error::Error::InvalidPort(format!(
            "port {port} is a privileged port (< 1024)"
        )));
    }
    if is_browser_blocked(port) {
        return Err(crate::error::Error::InvalidPort(format!(
            "port {port} is blocked by browsers — see https://fetch.spec.whatwg.org/#bad-port"
        )));
    }
    Ok(())
}
```

- [ ] **Step 5: Run tests to verify they pass**

```bash
cargo test --bin portal -- ports::tests::validate 2>&1 | tail -10
```

Expected: all 6 new tests pass.

- [ ] **Step 6: Run full test suite to check for regressions**

```bash
cargo test --bin portal 2>&1 | grep "test result"
```

Expected: all tests pass, 0 failures.

- [ ] **Step 7: Commit**

```bash
git add src/error.rs src/ports.rs
git commit -m "feat(ports): add validate_app_port to reject privileged and browser-blocked ports"
```

---

### Task 2: Call validation in `do_run()`

**Files:**
- Modify: `src/cli/mod.rs`

- [ ] **Step 1: Locate the insertion point**

In `src/cli/mod.rs`, find the block that begins:

```rust
    let port = if let Some(explicit_port) = port_override {
        if let Some(_old_port) = reuse_port {
```

This is around line 375. The validation must happen as the very first line inside `if let Some(explicit_port) = port_override {`.

- [ ] **Step 2: Add the validation call**

Change:

```rust
    let port = if let Some(explicit_port) = port_override {
        if let Some(_old_port) = reuse_port {
```

To:

```rust
    let port = if let Some(explicit_port) = port_override {
        crate::ports::validate_app_port(explicit_port)?;
        if let Some(_old_port) = reuse_port {
```

- [ ] **Step 3: Verify it compiles**

```bash
cargo check --bin portal 2>&1 | head -20
```

Expected: no errors.

- [ ] **Step 4: Smoke test the error message**

```bash
cargo run --bin portal -- run --port 6000 echo hello 2>&1
```

Expected output contains: `invalid port: port 6000 is blocked by browsers`

```bash
cargo run --bin portal -- run --port 80 echo hello 2>&1
```

Expected output contains: `invalid port: port 80 is a privileged port (< 1024)`

- [ ] **Step 5: Run full test suite**

```bash
cargo test --bin portal 2>&1 | grep "test result"
```

Expected: all tests pass, 0 failures.

- [ ] **Step 6: Commit**

```bash
git add src/cli/mod.rs
git commit -m "feat(cli): validate explicit --port against blocked and privileged port lists"
```
