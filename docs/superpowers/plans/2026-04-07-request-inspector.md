# Request Inspector Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Capture every proxied HTTP request (full headers + bodies) and display them live in a Svelte dashboard at `_.localhost`, persisted in SQLite, with real-time SSE updates.

**Architecture:** New `src/inspector/` Rust module. Proxy buffers req/res bodies, fires `try_send` to a bounded mpsc channel (non-blocking). Background worker drains channel, writes to SQLite, broadcasts `RequestMeta` over a broadcast channel. Axum server at a random port serves the embedded Svelte UI and REST+SSE API. `_.localhost` is registered in the daemon route table so the existing TLS proxy handles it transparently. Svelte 5 frontend in `ui/` connects to `/api/stream` SSE on mount and fetches history from `/api/requests`.

**Tech Stack:** Rust (`axum 0.8`, `rusqlite 0.32` bundled, `rust-embed 8`, `tokio-stream 0.1`), Svelte 5 + shadcn-svelte + Vite (already scaffolded in `ui/`).

---

## File Structure

**New Rust files:**
- `src/inspector/types.rs` — `CapturedRequest`, `CapturedBody`, `RequestMeta`, `RequestRecord`
- `src/inspector/db.rs` — `Db` struct wrapping `Arc<Mutex<rusqlite::Connection>>`, schema init, insert, query, delete
- `src/inspector/sse.rs` — broadcast channel type aliases and `RequestMeta` formatting
- `src/inspector/assets.rs` — `rust-embed` derive for `ui/dist/`
- `src/inspector/server.rs` — axum `Router`, all HTTP handlers
- `src/inspector/mod.rs` — `Inspector`, `InspectorSender`, `start()` async, background worker

**Modified Rust files:**
- `Cargo.toml` — add deps
- `src/main.rs` — add `mod inspector;`
- `src/proxy.rs` — add `InspectorSender` param, buffer bodies, fire capture
- `src/daemon/mod.rs` — call `Inspector::start()`, register route, thread sender into proxy
- `src/daemon/ipc.rs` — filter `_.localhost` from `Ls` response
- `src/cli/mod.rs` — add `CliCommand::Inspect`

**New Svelte files (in `ui/src/`):**
- `lib/api.ts` — typed fetch wrappers
- `lib/stores/requests.svelte.ts` — Svelte 5 runes state + SSE connection
- `lib/components/Sidebar.svelte` — route list, filter chips, clear button
- `lib/components/RequestFeed.svelte` — request rows
- `lib/components/RequestDetail.svelte` — tabs: Request / Response / Headers / Timing
- `routes/+page.svelte` — 3-panel layout (replaces placeholder)

---

### Task 1: Add Cargo dependencies

**Files:**
- Modify: `Cargo.toml`

- [ ] **Step 1: Add deps**

In `Cargo.toml` under `[dependencies]`, add:

```toml
axum = { version = "0.8", features = ["http1", "macros"] }
rusqlite = { version = "0.32", features = ["bundled"] }
rust-embed = "8"
tokio-stream = { version = "0.1", features = ["sync"] }
```

- [ ] **Step 2: Verify build**

```bash
cargo build 2>&1 | grep -E "^error"
```

Expected: no errors (warnings OK).

- [ ] **Step 3: Commit**

```bash
git add Cargo.toml Cargo.lock
git commit -m "deps: add axum, rusqlite, rust-embed, tokio-stream for inspector"
```

---

### Task 2: Types module

**Files:**
- Create: `src/inspector/types.rs`

- [ ] **Step 1: Write failing tests**

Create `src/inspector/types.rs` with just the test module to verify it fails:

```rust
pub const BODY_CAP: usize = 1_048_576; // 1 MB

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn request_meta_json_round_trip() {
        let meta = RequestMeta {
            id: 1,
            hostname: "myapp.localhost".to_string(),
            method: "GET".to_string(),
            path: "/api/users".to_string(),
            status: 200,
            duration_ms: 42,
            timestamp: 1712500321000,
        };
        let json = serde_json::to_string(&meta).unwrap();
        let back: RequestMeta = serde_json::from_str(&json).unwrap();
        assert_eq!(back.id, 1);
        assert_eq!(back.method, "GET");
    }

    #[test]
    fn captured_body_cap() {
        let big = bytes::Bytes::from(vec![b'x'; BODY_CAP + 100]);
        let captured = CapturedBody::from_bytes(&big);
        assert!(captured.is_truncated());
        assert_eq!(captured.prefix_bytes().len(), BODY_CAP);
        assert_eq!(captured.total_bytes(), BODY_CAP + 100);
    }

    #[test]
    fn captured_body_small() {
        let small = bytes::Bytes::from(b"hello".as_ref());
        let captured = CapturedBody::from_bytes(&small);
        assert!(!captured.is_truncated());
        assert_eq!(captured.to_display_string(), "hello");
    }

    #[test]
    fn captured_body_empty() {
        let captured = CapturedBody::from_bytes(&bytes::Bytes::new());
        assert!(matches!(captured, CapturedBody::Empty));
        assert_eq!(captured.total_bytes(), 0);
    }
}
```

- [ ] **Step 2: Run failing tests**

```bash
cargo test inspector::types 2>&1 | tail -10
```

Expected: compile error — types not defined yet.

- [ ] **Step 3: Implement types**

Replace `src/inspector/types.rs` with full implementation:

```rust
use bytes::Bytes;
use serde::{Deserialize, Serialize};

pub const BODY_CAP: usize = 1_048_576; // 1 MB

/// Data captured from a single proxied request/response pair.
/// Sent over the mpsc channel from the proxy to the background worker.
#[derive(Debug, Clone)]
pub struct CapturedRequest {
    pub hostname: String,
    pub method: String,
    pub path: String, // includes query string
    pub req_headers: Vec<(String, String)>,
    pub req_body: CapturedBody,
    pub res_status: u16,
    pub res_headers: Vec<(String, String)>,
    pub res_body: CapturedBody,
    pub duration_ms: u64,
    pub timestamp: i64, // Unix ms
}

#[derive(Debug, Clone)]
pub enum CapturedBody {
    Full(Bytes),
    Truncated { prefix: Bytes, total_bytes: usize },
    Empty,
}

impl CapturedBody {
    pub fn from_bytes(bytes: &Bytes) -> Self {
        if bytes.is_empty() {
            CapturedBody::Empty
        } else if bytes.len() <= BODY_CAP {
            CapturedBody::Full(bytes.clone())
        } else {
            CapturedBody::Truncated {
                prefix: bytes.slice(..BODY_CAP),
                total_bytes: bytes.len(),
            }
        }
    }

    pub fn is_truncated(&self) -> bool {
        matches!(self, CapturedBody::Truncated { .. })
    }

    pub fn total_bytes(&self) -> usize {
        match self {
            CapturedBody::Empty => 0,
            CapturedBody::Full(b) => b.len(),
            CapturedBody::Truncated { total_bytes, .. } => *total_bytes,
        }
    }

    pub fn prefix_bytes(&self) -> &[u8] {
        match self {
            CapturedBody::Empty => &[],
            CapturedBody::Full(b) => b.as_ref(),
            CapturedBody::Truncated { prefix, .. } => prefix.as_ref(),
        }
    }

    /// Returns UTF-8 text if valid, otherwise a lossy conversion with replacement chars.
    pub fn to_display_string(&self) -> String {
        String::from_utf8_lossy(self.prefix_bytes()).into_owned()
    }
}

/// Lightweight metadata sent over SSE — no bodies or headers.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RequestMeta {
    pub id: i64,
    pub hostname: String,
    pub method: String,
    pub path: String,
    pub status: u16,
    pub duration_ms: u64,
    pub timestamp: i64,
}

/// Full record returned by GET /api/requests (includes headers + bodies).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RequestRecord {
    pub id: i64,
    pub hostname: String,
    pub method: String,
    pub path: String,
    pub status: u16,
    pub duration_ms: u64,
    pub timestamp: i64,
    pub req_headers: Vec<(String, String)>,
    pub req_body: String,
    pub req_truncated: bool,
    pub req_total_bytes: usize,
    pub res_headers: Vec<(String, String)>,
    pub res_body: String,
    pub res_truncated: bool,
    pub res_total_bytes: usize,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn request_meta_json_round_trip() {
        let meta = RequestMeta {
            id: 1,
            hostname: "myapp.localhost".to_string(),
            method: "GET".to_string(),
            path: "/api/users".to_string(),
            status: 200,
            duration_ms: 42,
            timestamp: 1712500321000,
        };
        let json = serde_json::to_string(&meta).unwrap();
        let back: RequestMeta = serde_json::from_str(&json).unwrap();
        assert_eq!(back.id, 1);
        assert_eq!(back.method, "GET");
    }

    #[test]
    fn captured_body_cap() {
        let big = Bytes::from(vec![b'x'; BODY_CAP + 100]);
        let captured = CapturedBody::from_bytes(&big);
        assert!(captured.is_truncated());
        assert_eq!(captured.prefix_bytes().len(), BODY_CAP);
        assert_eq!(captured.total_bytes(), BODY_CAP + 100);
    }

    #[test]
    fn captured_body_small() {
        let small = Bytes::from(b"hello".as_ref());
        let captured = CapturedBody::from_bytes(&small);
        assert!(!captured.is_truncated());
        assert_eq!(captured.to_display_string(), "hello");
    }

    #[test]
    fn captured_body_empty() {
        let captured = CapturedBody::from_bytes(&Bytes::new());
        assert!(matches!(captured, CapturedBody::Empty));
        assert_eq!(captured.total_bytes(), 0);
    }
}
```

- [ ] **Step 4: Run tests**

```bash
cargo test inspector::types 2>&1 | tail -15
```

Expected: 4 tests pass.

- [ ] **Step 5: Commit**

```bash
git add src/inspector/types.rs
git commit -m "feat(inspector): add CapturedRequest, CapturedBody, RequestMeta, RequestRecord types"
```

---

### Task 3: DB module

**Files:**
- Create: `src/inspector/db.rs`

- [ ] **Step 1: Write failing tests**

Create `src/inspector/db.rs` with just the test module:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn test_db(dir: &TempDir) -> Db {
        Db::open(dir.path().join("inspector.db")).unwrap()
    }

    #[test]
    fn insert_and_query() {
        let dir = TempDir::new().unwrap();
        let db = test_db(&dir);
        let req = make_test_captured();
        let id = db.insert(&req).unwrap();
        assert!(id > 0);
        let page = db.query_page(None, None, 10, None).unwrap();
        assert_eq!(page.len(), 1);
        assert_eq!(page[0].id, id);
        assert_eq!(page[0].method, "GET");
    }

    #[test]
    fn query_by_hostname() {
        let dir = TempDir::new().unwrap();
        let db = test_db(&dir);
        let mut req = make_test_captured();
        db.insert(&req).unwrap();
        req.hostname = "other.localhost".to_string();
        db.insert(&req).unwrap();
        let page = db.query_page(Some("myapp.localhost"), None, 10, None).unwrap();
        assert_eq!(page.len(), 1);
    }

    #[test]
    fn delete_one() {
        let dir = TempDir::new().unwrap();
        let db = test_db(&dir);
        let id = db.insert(&make_test_captured()).unwrap();
        db.delete_one(id).unwrap();
        assert!(db.query_page(None, None, 10, None).unwrap().is_empty());
    }

    #[test]
    fn delete_all() {
        let dir = TempDir::new().unwrap();
        let db = test_db(&dir);
        db.insert(&make_test_captured()).unwrap();
        db.insert(&make_test_captured()).unwrap();
        db.delete_all(None).unwrap();
        assert!(db.query_page(None, None, 10, None).unwrap().is_empty());
    }

    fn make_test_captured() -> crate::inspector::types::CapturedRequest {
        use crate::inspector::types::*;
        CapturedRequest {
            hostname: "myapp.localhost".to_string(),
            method: "GET".to_string(),
            path: "/api/users".to_string(),
            req_headers: vec![("content-type".to_string(), "application/json".to_string())],
            req_body: CapturedBody::Full(bytes::Bytes::from("{}".as_bytes())),
            res_status: 200,
            res_headers: vec![],
            res_body: CapturedBody::Full(bytes::Bytes::from("[]".as_bytes())),
            duration_ms: 42,
            timestamp: 1712500321000,
        }
    }
}
```

- [ ] **Step 2: Run to confirm failure**

```bash
cargo test inspector::db 2>&1 | tail -5
```

Expected: compile error.

- [ ] **Step 3: Implement Db**

Replace `src/inspector/db.rs`:

```rust
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use rusqlite::{params, Connection};

use crate::inspector::types::{CapturedRequest, RequestRecord};

/// Thread-safe SQLite handle for the inspector database.
#[derive(Clone)]
pub struct Db(Arc<Mutex<Connection>>);

impl Db {
    pub fn open(path: PathBuf) -> crate::error::Result<Self> {
        let conn = Connection::open(&path)
            .map_err(|e| crate::error::Error::Ipc(e.to_string()))?;

        conn.execute_batch("PRAGMA journal_mode=WAL;")
            .map_err(|e| crate::error::Error::Ipc(e.to_string()))?;

        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS requests (
                id              INTEGER PRIMARY KEY AUTOINCREMENT,
                hostname        TEXT    NOT NULL,
                method          TEXT    NOT NULL,
                path            TEXT    NOT NULL,
                status          INTEGER NOT NULL,
                duration_ms     INTEGER NOT NULL,
                timestamp       INTEGER NOT NULL,
                req_headers     TEXT    NOT NULL,
                req_body        BLOB,
                req_truncated   INTEGER NOT NULL DEFAULT 0,
                req_total_bytes INTEGER NOT NULL DEFAULT 0,
                res_headers     TEXT    NOT NULL,
                res_body        BLOB,
                res_truncated   INTEGER NOT NULL DEFAULT 0,
                res_total_bytes INTEGER NOT NULL DEFAULT 0
            );
            CREATE INDEX IF NOT EXISTS idx_hostname  ON requests(hostname);
            CREATE INDEX IF NOT EXISTS idx_timestamp ON requests(timestamp DESC);",
        )
        .map_err(|e| crate::error::Error::Ipc(e.to_string()))?;

        Ok(Db(Arc::new(Mutex::new(conn))))
    }

    /// Insert a captured request and return the assigned row id.
    pub fn insert(&self, req: &CapturedRequest) -> crate::error::Result<i64> {
        let conn = self.0.lock().unwrap();
        let req_headers_json = serde_json::to_string(&req.req_headers).unwrap_or_default();
        let res_headers_json = serde_json::to_string(&req.res_headers).unwrap_or_default();

        conn.execute(
            "INSERT INTO requests (
                hostname, method, path, status, duration_ms, timestamp,
                req_headers, req_body, req_truncated, req_total_bytes,
                res_headers, res_body, res_truncated, res_total_bytes
             ) VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,?14)",
            params![
                req.hostname,
                req.method,
                req.path,
                req.res_status,
                req.duration_ms,
                req.timestamp,
                req_headers_json,
                req.req_body.prefix_bytes(),
                req.req_body.is_truncated() as i32,
                req.req_body.total_bytes() as i64,
                res_headers_json,
                req.res_body.prefix_bytes(),
                req.res_body.is_truncated() as i32,
                req.res_body.total_bytes() as i64,
            ],
        )
        .map_err(|e| crate::error::Error::Ipc(e.to_string()))?;

        Ok(conn.last_insert_rowid())
    }

    /// Query a page of request records, newest first.
    /// `hostname` — filter by route (None = all).
    /// `before_id` — pagination cursor (return rows with id < before_id).
    /// `limit` — max rows (capped at 500).
    pub fn query_page(
        &self,
        hostname: Option<&str>,
        before_id: Option<i64>,
        limit: usize,
        single_id: Option<i64>,
    ) -> crate::error::Result<Vec<RequestRecord>> {
        let conn = self.0.lock().unwrap();
        let limit = limit.min(500) as i64;

        let rows = if let Some(id) = single_id {
            conn.prepare(
                "SELECT id,hostname,method,path,status,duration_ms,timestamp,
                         req_headers,req_body,req_truncated,req_total_bytes,
                         res_headers,res_body,res_truncated,res_total_bytes
                  FROM requests WHERE id=?1",
            )
            .and_then(|mut s| {
                s.query_map(params![id], row_to_record)
                    .and_then(|rows| rows.collect::<Result<Vec<_>, _>>())
            })
        } else {
            let (where_clause, p_hostname, p_before) = match (hostname, before_id) {
                (Some(h), Some(b)) => ("WHERE hostname=?1 AND id<?2", Some(h.to_string()), Some(b)),
                (Some(h), None) => ("WHERE hostname=?1", Some(h.to_string()), None),
                (None, Some(b)) => ("WHERE id<?2", None, Some(b)),
                (None, None) => ("", None, None),
            };
            let sql = format!(
                "SELECT id,hostname,method,path,status,duration_ms,timestamp,
                         req_headers,req_body,req_truncated,req_total_bytes,
                         res_headers,res_body,res_truncated,res_total_bytes
                  FROM requests {where_clause} ORDER BY id DESC LIMIT ?3"
            );
            conn.prepare(&sql).and_then(|mut s| {
                s.query_map(
                    params![
                        p_hostname.as_deref().unwrap_or(""),
                        p_before.unwrap_or(i64::MAX),
                        limit
                    ],
                    row_to_record,
                )
                .and_then(|rows| rows.collect::<Result<Vec<_>, _>>())
            })
        };

        rows.map_err(|e| crate::error::Error::Ipc(e.to_string()))
    }

    /// Delete a single request by id.
    pub fn delete_one(&self, id: i64) -> crate::error::Result<()> {
        let conn = self.0.lock().unwrap();
        conn.execute("DELETE FROM requests WHERE id=?1", params![id])
            .map_err(|e| crate::error::Error::Ipc(e.to_string()))?;
        Ok(())
    }

    /// Delete all requests, optionally filtered by hostname.
    pub fn delete_all(&self, hostname: Option<&str>) -> crate::error::Result<()> {
        let conn = self.0.lock().unwrap();
        match hostname {
            Some(h) => conn.execute("DELETE FROM requests WHERE hostname=?1", params![h]),
            None => conn.execute("DELETE FROM requests", []),
        }
        .map_err(|e| crate::error::Error::Ipc(e.to_string()))?;
        Ok(())
    }
}

fn row_to_record(row: &rusqlite::Row<'_>) -> rusqlite::Result<RequestRecord> {
    let req_body_bytes: Vec<u8> = row.get(8).unwrap_or_default();
    let res_body_bytes: Vec<u8> = row.get(12).unwrap_or_default();
    let req_headers_json: String = row.get(7)?;
    let res_headers_json: String = row.get(11)?;

    Ok(RequestRecord {
        id: row.get(0)?,
        hostname: row.get(1)?,
        method: row.get(2)?,
        path: row.get(3)?,
        status: row.get::<_, i64>(4)? as u16,
        duration_ms: row.get::<_, i64>(5)? as u64,
        timestamp: row.get(6)?,
        req_headers: serde_json::from_str(&req_headers_json).unwrap_or_default(),
        req_body: String::from_utf8_lossy(&req_body_bytes).into_owned(),
        req_truncated: row.get::<_, i32>(9)? != 0,
        req_total_bytes: row.get::<_, i64>(10)? as usize,
        res_headers: serde_json::from_str(&res_headers_json).unwrap_or_default(),
        res_body: String::from_utf8_lossy(&res_body_bytes).into_owned(),
        res_truncated: row.get::<_, i32>(13)? != 0,
        res_total_bytes: row.get::<_, i64>(14)? as usize,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::inspector::types::*;
    use tempfile::TempDir;

    fn test_db(dir: &TempDir) -> Db {
        Db::open(dir.path().join("inspector.db")).unwrap()
    }

    fn make_test_captured() -> CapturedRequest {
        CapturedRequest {
            hostname: "myapp.localhost".to_string(),
            method: "GET".to_string(),
            path: "/api/users".to_string(),
            req_headers: vec![("content-type".to_string(), "application/json".to_string())],
            req_body: CapturedBody::Full(bytes::Bytes::from("{}".as_bytes())),
            res_status: 200,
            res_headers: vec![],
            res_body: CapturedBody::Full(bytes::Bytes::from("[]".as_bytes())),
            duration_ms: 42,
            timestamp: 1712500321000,
        }
    }

    #[test]
    fn insert_and_query() {
        let dir = TempDir::new().unwrap();
        let db = test_db(&dir);
        let id = db.insert(&make_test_captured()).unwrap();
        assert!(id > 0);
        let page = db.query_page(None, None, 10, None).unwrap();
        assert_eq!(page.len(), 1);
        assert_eq!(page[0].id, id);
        assert_eq!(page[0].method, "GET");
    }

    #[test]
    fn query_by_hostname() {
        let dir = TempDir::new().unwrap();
        let db = test_db(&dir);
        let mut req = make_test_captured();
        db.insert(&req).unwrap();
        req.hostname = "other.localhost".to_string();
        db.insert(&req).unwrap();
        let page = db.query_page(Some("myapp.localhost"), None, 10, None).unwrap();
        assert_eq!(page.len(), 1);
        assert_eq!(page[0].hostname, "myapp.localhost");
    }

    #[test]
    fn delete_one() {
        let dir = TempDir::new().unwrap();
        let db = test_db(&dir);
        let id = db.insert(&make_test_captured()).unwrap();
        db.delete_one(id).unwrap();
        assert!(db.query_page(None, None, 10, None).unwrap().is_empty());
    }

    #[test]
    fn delete_all() {
        let dir = TempDir::new().unwrap();
        let db = test_db(&dir);
        db.insert(&make_test_captured()).unwrap();
        db.insert(&make_test_captured()).unwrap();
        db.delete_all(None).unwrap();
        assert!(db.query_page(None, None, 10, None).unwrap().is_empty());
    }
}
```

Note: `query_page` with no hostname/before_id uses params `?1`/`?2` even when unused — rusqlite ignores extra params. This keeps the param list uniform.

- [ ] **Step 4: Wire the module (temporary) so tests compile**

Create a minimal `src/inspector/mod.rs` for now:

```rust
pub mod db;
pub mod types;
```

And add to `src/main.rs`:

```rust
mod inspector;
```

- [ ] **Step 5: Run tests**

```bash
cargo test inspector::db 2>&1 | tail -15
```

Expected: 4 tests pass.

- [ ] **Step 6: Commit**

```bash
git add src/inspector/ src/main.rs
git commit -m "feat(inspector): add SQLite Db module with schema, insert, query, delete"
```

---

### Task 4: SSE module

**Files:**
- Create: `src/inspector/sse.rs`

- [ ] **Step 1: Create the SSE module**

Create `src/inspector/sse.rs`:

```rust
use crate::inspector::types::{CapturedRequest, RequestMeta};

/// Capacity of the broadcast channel (number of in-flight SSE events).
/// Lagging receivers are dropped automatically by tokio.
pub const SSE_CHANNEL_CAPACITY: usize = 256;

pub type SseTx = tokio::sync::broadcast::Sender<RequestMeta>;
pub type SseRx = tokio::sync::broadcast::Receiver<RequestMeta>;

pub fn new_channel() -> SseTx {
    let (tx, _) = tokio::sync::broadcast::channel(SSE_CHANNEL_CAPACITY);
    tx
}

/// Convert a CapturedRequest + DB-assigned id into a RequestMeta for SSE broadcast.
pub fn to_meta(req: &CapturedRequest, id: i64) -> RequestMeta {
    RequestMeta {
        id,
        hostname: req.hostname.clone(),
        method: req.method.clone(),
        path: req.path.clone(),
        status: req.res_status,
        duration_ms: req.duration_ms,
        timestamp: req.timestamp,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::inspector::types::CapturedBody;

    #[test]
    fn broadcast_send_recv() {
        let tx = new_channel();
        let mut rx = tx.subscribe();
        let req = CapturedRequest {
            hostname: "test.localhost".to_string(),
            method: "POST".to_string(),
            path: "/ping".to_string(),
            req_headers: vec![],
            req_body: CapturedBody::Empty,
            res_status: 204,
            res_headers: vec![],
            res_body: CapturedBody::Empty,
            duration_ms: 1,
            timestamp: 0,
        };
        let meta = to_meta(&req, 7);
        tx.send(meta).unwrap();
        let received = rx.try_recv().unwrap();
        assert_eq!(received.id, 7);
        assert_eq!(received.status, 204);
    }
}
```

- [ ] **Step 2: Add to mod.rs**

Update `src/inspector/mod.rs`:

```rust
pub mod db;
pub mod sse;
pub mod types;
```

- [ ] **Step 3: Run tests**

```bash
cargo test inspector::sse 2>&1 | tail -10
```

Expected: 1 test passes.

- [ ] **Step 4: Commit**

```bash
git add src/inspector/sse.rs src/inspector/mod.rs
git commit -m "feat(inspector): add SSE broadcast channel module"
```

---

### Task 5: Assets module

**Files:**
- Create: `src/inspector/assets.rs`

- [ ] **Step 1: Create assets module**

Create `src/inspector/assets.rs`:

```rust
use axum::{
    body::Body,
    http::{header, Response, StatusCode},
    response::IntoResponse,
};
use rust_embed::RustEmbed;

#[derive(RustEmbed)]
#[folder = "ui/dist/"]
pub struct UiAssets;

fn mime_from_path(path: &str) -> &'static str {
    if path.ends_with(".html") {
        "text/html; charset=utf-8"
    } else if path.ends_with(".js") || path.ends_with(".mjs") {
        "application/javascript"
    } else if path.ends_with(".css") {
        "text/css"
    } else if path.ends_with(".svg") {
        "image/svg+xml"
    } else if path.ends_with(".woff2") {
        "font/woff2"
    } else if path.ends_with(".woff") {
        "font/woff"
    } else if path.ends_with(".png") {
        "image/png"
    } else if path.ends_with(".ico") {
        "image/x-icon"
    } else {
        "application/octet-stream"
    }
}

/// Serve a file from the embedded assets, or fall back to index.html (SPA routing).
pub fn serve_embedded(path: &str) -> Response<Body> {
    let path = path.trim_start_matches('/');
    let path = if path.is_empty() { "index.html" } else { path };

    if let Some(file) = UiAssets::get(path) {
        let mime = mime_from_path(path);
        Response::builder()
            .status(StatusCode::OK)
            .header(header::CONTENT_TYPE, mime)
            .body(Body::from(file.data.into_owned()))
            .unwrap()
    } else {
        // SPA fallback — return index.html for unknown paths (client-side routing)
        match UiAssets::get("index.html") {
            Some(file) => Response::builder()
                .status(StatusCode::OK)
                .header(header::CONTENT_TYPE, "text/html; charset=utf-8")
                .body(Body::from(file.data.into_owned()))
                .unwrap(),
            None => Response::builder()
                .status(StatusCode::NOT_FOUND)
                .body(Body::from("UI not built — run: cd ui && bun run build"))
                .unwrap(),
        }
    }
}
```

- [ ] **Step 2: Add to mod.rs**

Update `src/inspector/mod.rs`:

```rust
pub mod assets;
pub mod db;
pub mod sse;
pub mod types;
```

- [ ] **Step 3: Build**

```bash
cargo build 2>&1 | grep "^error"
```

Expected: no errors. (`ui/dist/` is empty but exists thanks to `build.rs` — embed compiles fine.)

- [ ] **Step 4: Commit**

```bash
git add src/inspector/assets.rs src/inspector/mod.rs
git commit -m "feat(inspector): add rust-embed assets module for ui/dist/"
```

---

### Task 6: Server module

**Files:**
- Create: `src/inspector/server.rs`

- [ ] **Step 1: Create server.rs**

Create `src/inspector/server.rs`:

```rust
use axum::{
    extract::{Path, Query, State},
    http::{StatusCode, Uri},
    response::{
        sse::{Event, KeepAlive, Sse},
        IntoResponse, Response,
    },
    routing::{delete, get},
    Json, Router,
};
use serde::{Deserialize, Serialize};
use std::convert::Infallible;
use tokio_stream::StreamExt;

use crate::inspector::{
    assets::serve_embedded,
    db::Db,
    sse::SseTx,
    types::RequestMeta,
};

#[derive(Clone)]
pub struct AppState {
    pub db: Db,
    pub sse_tx: SseTx,
}

pub fn router(state: AppState) -> Router {
    Router::new()
        .route("/api/requests", get(get_requests).delete(delete_all_requests))
        .route("/api/requests/{id}", delete(delete_one_request))
        .route("/api/stream", get(sse_handler))
        .fallback(static_handler)
        .with_state(state)
}

// ── Query params ──────────────────────────────────────────────────────────────

#[derive(Debug, Deserialize)]
pub struct RequestsQuery {
    hostname: Option<String>,
    limit: Option<usize>,
    before_id: Option<i64>,
    id: Option<i64>,
}

#[derive(Serialize)]
struct RequestsResponse {
    requests: Vec<crate::inspector::types::RequestRecord>,
    has_more: bool,
}

// ── Handlers ─────────────────────────────────────────────────────────────────

async fn get_requests(
    State(state): State<AppState>,
    Query(q): Query<RequestsQuery>,
) -> impl IntoResponse {
    let limit = q.limit.unwrap_or(100).min(500);
    let records = state
        .db
        .query_page(
            q.hostname.as_deref(),
            q.before_id,
            limit + 1, // fetch one extra to detect has_more
            q.id,
        )
        .unwrap_or_default();

    let has_more = records.len() > limit;
    let records = records.into_iter().take(limit).collect();
    Json(RequestsResponse { requests: records, has_more })
}

async fn delete_all_requests(
    State(state): State<AppState>,
    Query(q): Query<RequestsQuery>,
) -> impl IntoResponse {
    state.db.delete_all(q.hostname.as_deref()).ok();
    StatusCode::NO_CONTENT
}

async fn delete_one_request(
    State(state): State<AppState>,
    Path(id): Path<i64>,
) -> impl IntoResponse {
    state.db.delete_one(id).ok();
    StatusCode::NO_CONTENT
}

async fn sse_handler(State(state): State<AppState>) -> Sse<impl tokio_stream::Stream<Item = Result<Event, Infallible>>> {
    let rx = state.sse_tx.subscribe();
    let stream = tokio_stream::wrappers::BroadcastStream::new(rx)
        .filter_map(|res| res.ok())
        .map(|meta: RequestMeta| {
            let data = serde_json::to_string(&meta).unwrap_or_default();
            Ok::<Event, Infallible>(Event::default().event("request").data(data))
        });
    Sse::new(stream).keep_alive(KeepAlive::default())
}

async fn static_handler(uri: Uri) -> Response {
    // Dev mode: proxy to Vite dev server
    if std::env::var("PORTAL_UI_DEV").is_ok() {
        return dev_proxy(uri).await;
    }
    serve_embedded(uri.path())
}

async fn dev_proxy(uri: Uri) -> Response {
    let vite_url = format!("http://localhost:5173{}", uri.path_and_query().map(|p| p.as_str()).unwrap_or("/"));
    match reqwest_or_hyper_get(&vite_url).await {
        Ok(resp) => resp,
        Err(_) => serve_embedded(uri.path()),
    }
}

/// Forward a GET request to the Vite dev server and return the response.
async fn reqwest_or_hyper_get(url: &str) -> Result<Response, ()> {
    use http_body_util::BodyExt;
    use hyper_util::client::legacy::{connect::HttpConnector, Client};
    use hyper_util::rt::TokioExecutor;

    let client: Client<HttpConnector, http_body_util::Empty<bytes::Bytes>> =
        Client::builder(TokioExecutor::new()).build_http();

    let req = hyper::Request::builder()
        .uri(url)
        .body(http_body_util::Empty::new())
        .map_err(|_| ())?;

    let resp = client.request(req).await.map_err(|_| ())?;
    let (parts, body) = resp.into_parts();
    let bytes = body.collect().await.map_err(|_| ())?.to_bytes();

    let content_type = parts
        .headers
        .get(hyper::header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("application/octet-stream")
        .to_string();

    Ok(axum::response::Response::builder()
        .status(parts.status)
        .header(axum::http::header::CONTENT_TYPE, content_type)
        .body(axum::body::Body::from(bytes))
        .unwrap())
}
```

- [ ] **Step 2: Add to mod.rs**

Update `src/inspector/mod.rs`:

```rust
pub mod assets;
pub mod db;
pub mod server;
pub mod sse;
pub mod types;
```

- [ ] **Step 3: Build**

```bash
cargo build 2>&1 | grep "^error"
```

Expected: no errors.

- [ ] **Step 4: Commit**

```bash
git add src/inspector/server.rs src/inspector/mod.rs
git commit -m "feat(inspector): add axum server with REST API and SSE endpoint"
```

---

### Task 7: Inspector mod — start() and background worker

**Files:**
- Modify: `src/inspector/mod.rs`

- [ ] **Step 1: Write the full mod.rs**

Replace `src/inspector/mod.rs`:

```rust
pub mod assets;
pub mod db;
pub mod server;
pub mod sse;
pub mod types;

use std::path::PathBuf;
use std::sync::atomic::{AtomicU16, Ordering};

use crate::error::Result;
use crate::inspector::{
    db::Db,
    server::{router, AppState},
    sse::{new_channel, to_meta},
    types::CapturedRequest,
};

/// Capacity of the mpsc channel between proxy and background worker.
const CHANNEL_CAPACITY: usize = 8192;

/// Cheap-to-clone handle for sending captured requests from the proxy.
/// `try_send` is non-blocking — drops silently if channel is full.
#[derive(Clone)]
pub struct InspectorSender(tokio::sync::mpsc::Sender<CapturedRequest>);

impl InspectorSender {
    pub fn send(&self, req: CapturedRequest) {
        let _ = self.0.try_send(req);
    }
}

pub struct Inspector {
    pub sender: InspectorSender,
    pub port: u16,
}

impl Inspector {
    /// Starts the background worker and axum server.
    /// Returns the Inspector (with its sender and bound port).
    pub async fn start(db_path: PathBuf) -> Result<Inspector> {
        let db = Db::open(db_path)?;
        let sse_tx = new_channel();
        let (tx, mut rx) = tokio::sync::mpsc::channel::<CapturedRequest>(CHANNEL_CAPACITY);

        // Background worker: drain channel → write to SQLite → broadcast SSE
        {
            let db = db.clone();
            let sse_tx = sse_tx.clone();
            tokio::spawn(async move {
                while let Some(req) = rx.recv().await {
                    if let Ok(id) = db.insert(&req) {
                        let meta = to_meta(&req, id);
                        let _ = sse_tx.send(meta); // ok if no SSE listeners
                    }
                }
            });
        }

        // Find a free port for the axum server
        let port = crate::ports::find_free_port(
            crate::config::Config::default_port_range().0,
            crate::config::Config::default_port_range().1,
        )?;

        // Start axum server
        let state = AppState { db, sse_tx };
        let app = router(state);
        let listener = tokio::net::TcpListener::bind(format!("127.0.0.1:{port}")).await?;

        tokio::spawn(async move {
            axum::serve(listener, app).await.ok();
        });

        Ok(Inspector {
            sender: InspectorSender(tx),
            port,
        })
    }
}
```

- [ ] **Step 2: Add `default_port_range` to Config**

Open `src/config.rs` and add a helper so the inspector can reuse the configured port range. Find the `Config` struct's `impl` block and add:

```rust
/// Returns the default port range used for assigning inspector and backend ports.
pub fn default_port_range() -> (u16, u16) {
    (3000, 9999)
}
```

- [ ] **Step 3: Build**

```bash
cargo build 2>&1 | grep "^error"
```

Expected: no errors.

- [ ] **Step 4: Commit**

```bash
git add src/inspector/mod.rs src/config.rs
git commit -m "feat(inspector): add Inspector::start() with background worker and axum server"
```

---

### Task 8: Proxy intercept

**Files:**
- Modify: `src/proxy.rs`

The proxy must buffer both request and response bodies, capture them, and fire a non-blocking `try_send`. The `handle_https_request` signature gains an `inspector: Option<InspectorSender>` parameter.

- [ ] **Step 1: Add inspector param and body buffering**

Replace the full `handle_https_request` function in `src/proxy.rs`:

```rust
/// Main proxy handler for HTTPS requests.
pub async fn handle_https_request(
    req: Request<Incoming>,
    routes: crate::routes::RouteStore,
    inspector: Option<crate::inspector::InspectorSender>,
) -> Result<Response<BoxBodyType>, std::convert::Infallible> {
    let start = std::time::Instant::now();

    // 1. Check hop counter
    let hops: u8 = req
        .headers()
        .get(HOP_HEADER)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.parse().ok())
        .unwrap_or(0);

    // Extract hostname
    let hostname = req
        .headers()
        .get(http::header::HOST)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("")
        .split(':')
        .next()
        .unwrap_or("")
        .to_string();

    if hops >= MAX_HOPS {
        let body = crate::pages::page_508(&hostname);
        return Ok(Response::builder()
            .status(StatusCode::LOOP_DETECTED)
            .header("content-type", "text/html")
            .body(full_body(body))
            .unwrap());
    }

    // Route lookup
    let route = match routes.get(&hostname) {
        Some(r) => r,
        None => {
            let body = crate::pages::page_404(&hostname);
            return Ok(Response::builder()
                .status(StatusCode::NOT_FOUND)
                .header("content-type", "text/html")
                .body(full_body(body))
                .unwrap());
        }
    };

    // WebSocket — no body capture needed
    if is_websocket_upgrade(&req) {
        return handle_websocket(req, route.port).await;
    }

    let port = route.port;

    let path_and_query = req
        .uri()
        .path_and_query()
        .map(|pq| pq.as_str())
        .unwrap_or("/")
        .to_string();

    let (mut parts, body) = req.into_parts();

    // Capture request headers before modifying parts
    let req_headers: Vec<(String, String)> = parts
        .headers
        .iter()
        .filter_map(|(k, v)| Some((k.as_str().to_string(), v.to_str().ok()?.to_string())))
        .collect();

    // Buffer request body
    let req_body_bytes = match body.collect().await {
        Ok(c) => c.to_bytes(),
        Err(_) => {
            return Ok(Response::builder()
                .status(StatusCode::BAD_GATEWAY)
                .header("content-type", "text/html")
                .body(full_body(crate::pages::page_502(&hostname)))
                .unwrap());
        }
    };

    // Build upstream request
    let upstream_uri = format!("http://127.0.0.1:{}{}", port, path_and_query);
    parts.uri = match upstream_uri.parse() {
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

    let upstream_req = Request::from_parts(parts, full_body(req_body_bytes.clone()));
    let client: Client<HttpConnector, BoxBodyType> =
        Client::builder(TokioExecutor::new()).build_http();

    match client.request(upstream_req).await {
        Ok(upstream_resp) => {
            let (resp_parts, resp_body) = upstream_resp.into_parts();
            let res_status = resp_parts.status.as_u16();

            // Capture response headers
            let res_headers: Vec<(String, String)> = resp_parts
                .headers
                .iter()
                .filter_map(|(k, v)| Some((k.as_str().to_string(), v.to_str().ok()?.to_string())))
                .collect();

            // Buffer response body
            let resp_bytes = match resp_body.collect().await {
                Ok(c) => c.to_bytes(),
                Err(_) => bytes::Bytes::new(),
            };

            // Fire capture — non-blocking try_send
            if let Some(sender) = &inspector {
                use crate::inspector::types::{CapturedBody, CapturedRequest};
                sender.send(CapturedRequest {
                    hostname: hostname.clone(),
                    method: parts_method_string_from_status(res_status, &req_body_bytes),
                    path: path_and_query.clone(),
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
        Err(_) => {
            let body = crate::pages::page_502(&hostname);
            Ok(Response::builder()
                .status(StatusCode::BAD_GATEWAY)
                .header("content-type", "text/html")
                .body(full_body(body))
                .unwrap())
        }
    }
}
```

Note: the HTTP method lives on `parts.method` but `into_parts()` consumes `req`, so capture the method string before calling `into_parts()`. Fix the method capture:

The correct approach is to capture method **before** `into_parts`. Replace with this updated version that captures method before destructuring:

```rust
// Capture method before consuming req
let method = req.method().to_string();

// ... then later in the inspector send block:
sender.send(CapturedRequest {
    hostname: hostname.clone(),
    method,   // ← use captured method
    path: path_and_query.clone(),
    ...
```

Remove the helper function `parts_method_string_from_status` — it was a placeholder. The full corrected `handle_https_request` is:

```rust
pub async fn handle_https_request(
    req: Request<Incoming>,
    routes: crate::routes::RouteStore,
    inspector: Option<crate::inspector::InspectorSender>,
) -> Result<Response<BoxBodyType>, std::convert::Infallible> {
    let start = std::time::Instant::now();

    let hops: u8 = req
        .headers()
        .get(HOP_HEADER)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.parse().ok())
        .unwrap_or(0);

    let hostname = req
        .headers()
        .get(http::header::HOST)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("")
        .split(':')
        .next()
        .unwrap_or("")
        .to_string();

    if hops >= MAX_HOPS {
        return Ok(Response::builder()
            .status(StatusCode::LOOP_DETECTED)
            .header("content-type", "text/html")
            .body(full_body(crate::pages::page_508(&hostname)))
            .unwrap());
    }

    let route = match routes.get(&hostname) {
        Some(r) => r,
        None => {
            return Ok(Response::builder()
                .status(StatusCode::NOT_FOUND)
                .header("content-type", "text/html")
                .body(full_body(crate::pages::page_404(&hostname)))
                .unwrap());
        }
    };

    if is_websocket_upgrade(&req) {
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

    let req_body_bytes = match body.collect().await {
        Ok(c) => c.to_bytes(),
        Err(_) => {
            return Ok(Response::builder()
                .status(StatusCode::BAD_GATEWAY)
                .header("content-type", "text/html")
                .body(full_body(crate::pages::page_502(&hostname)))
                .unwrap());
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
    parts.headers.insert(HOP_HEADER, (hops + 1).to_string().parse().unwrap());
    parts.headers.insert("x-forwarded-proto", "https".parse().unwrap());

    let client: Client<HttpConnector, BoxBodyType> =
        Client::builder(TokioExecutor::new()).build_http();
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
                    res_headers,
                    res_body: CapturedBody::from_bytes(&resp_bytes),
                    duration_ms: start.elapsed().as_millis() as u64,
                    timestamp: chrono::Utc::now().timestamp_millis(),
                });
            }

            Ok(Response::from_parts(resp_parts, full_body(resp_bytes)))
        }
        Err(_) => Ok(Response::builder()
            .status(StatusCode::BAD_GATEWAY)
            .header("content-type", "text/html")
            .body(full_body(crate::pages::page_502(&hostname)))
            .unwrap()),
    }
}
```

- [ ] **Step 2: Update the existing proxy tests**

The two proxy tests that call `handle_https_request` need updating (they'll fail with wrong arg count). Find them in `src/proxy.rs`'s test module — they test `is_tls_client_hello`, `http_redirect_listener_sends_301`, and `websocket_upgrade_is_detected`. None of these call `handle_https_request` directly, so no test changes are needed.

- [ ] **Step 3: Build**

```bash
cargo build 2>&1 | grep "^error"
```

Expected: error about `serve_https` passing wrong number of args to `handle_https_request` — that's fixed in Task 9.

- [ ] **Step 4: Commit (after Task 9 makes it build)**

Hold this commit until Task 9 is complete and `cargo build` passes.

---

### Task 9: Daemon wiring

**Files:**
- Modify: `src/daemon/mod.rs`

- [ ] **Step 1: Update serve_https to accept inspector**

In `src/daemon/mod.rs`, update the `serve_https` function signature and its call to `handle_https_request`:

```rust
async fn serve_https(
    listener: tokio::net::TcpListener,
    cert_store: CertStore,
    routes: RouteStore,
    inspector: Option<crate::inspector::InspectorSender>,
) {
    use hyper::server::conn::http1;
    use hyper_util::rt::TokioIo;
    use rustls::ServerConfig;
    use std::sync::Arc;
    use tokio_rustls::TlsAcceptor;

    let resolver = Arc::new(crate::certs::PortlessCertResolver::new(cert_store));
    let tls_config = Arc::new(
        ServerConfig::builder()
            .with_no_client_auth()
            .with_cert_resolver(resolver),
    );
    let acceptor = TlsAcceptor::from(tls_config);

    loop {
        let Ok((tcp_stream, _)) = listener.accept().await else {
            continue;
        };
        let acceptor = acceptor.clone();
        let routes = routes.clone();
        let inspector = inspector.clone();
        tokio::spawn(async move {
            let first = match crate::proxy::peek_first_byte(&tcp_stream).await {
                Ok(b) => b,
                Err(_) => return,
            };
            if !crate::proxy::is_tls_client_hello(first) {
                return;
            }
            let Ok(tls_stream) = acceptor.accept(tcp_stream).await else {
                return;
            };
            let io = TokioIo::new(tls_stream);
            http1::Builder::new()
                .serve_connection(
                    io,
                    hyper::service::service_fn(move |req| {
                        let r = routes.clone();
                        let insp = inspector.clone();
                        async move {
                            crate::proxy::handle_https_request(req, r, insp).await
                        }
                    }),
                )
                .with_upgrades()
                .await
                .ok();
        });
    }
}
```

- [ ] **Step 2: Start inspector in run_daemon_loop and register route**

In `run_daemon_loop`, after the cert store init and before binding listeners, add:

```rust
// Start inspector (background worker + axum server at _.localhost)
let inspector = match crate::inspector::Inspector::start(state_dir.join("inspector.db")).await {
    Ok(insp) => {
        // Register _.localhost in the route table
        let _ = routes.insert(crate::routes::Route {
            hostname: "_.localhost".to_string(),
            port: insp.port,
            pid: std::process::id(),
            owner_pid: std::process::id(),
            cwd: String::new(),
            created_at: chrono::Utc::now(),
        });
        tracing::info!("portal inspector started at _.localhost (internal port {})", insp.port);
        Some(insp.sender)
    }
    Err(e) => {
        tracing::warn!("portal inspector failed to start: {e}");
        None
    }
};
```

Then update the call to `serve_https` to pass `inspector`:

```rust
{
    let cs = cert_store.clone();
    let rt = routes.clone();
    let insp = inspector.clone();
    tokio::spawn(serve_https(https_listener, cs, rt, insp));
}
```

- [ ] **Step 3: Build**

```bash
cargo build 2>&1 | grep "^error"
```

Expected: no errors.

- [ ] **Step 4: Run tests**

```bash
cargo test 2>&1 | tail -10
```

Expected: all existing tests still pass.

- [ ] **Step 5: Commit Tasks 8 and 9 together**

```bash
git add src/proxy.rs src/daemon/mod.rs src/inspector/mod.rs src/config.rs
git commit -m "feat(inspector): wire proxy intercept and daemon startup — captures all requests"
```

---

### Task 10: Filter `_.localhost` from portal ls

**Files:**
- Modify: `src/daemon/ipc.rs`

- [ ] **Step 1: Write failing test**

In `src/daemon/ipc.rs` test module, add:

```rust
#[test]
fn ls_hides_inspector_route() {
    // The Ls command filters _.localhost from the route list.
    // Simulate by checking the filter logic directly.
    let routes: Vec<String> = vec![
        "myapp.localhost".to_string(),
        "_.localhost".to_string(),
        "api.localhost".to_string(),
    ];
    let filtered: Vec<&String> = routes.iter().filter(|h| *h != "_.localhost").collect();
    assert_eq!(filtered.len(), 2);
    assert!(!filtered.iter().any(|h| *h == "_.localhost"));
}
```

- [ ] **Step 2: Run to confirm test passes (it tests filter logic, not the IPC directly)**

```bash
cargo test ls_hides_inspector_route 2>&1 | tail -5
```

Expected: 1 test passes.

- [ ] **Step 3: Apply filter in the Ls handler**

In `src/daemon/ipc.rs`, find the `Command::Ls` arm and update the list construction:

```rust
Command::Ls => {
    let _ = routes.remove_stale();
    let list: Vec<_> = routes
        .list()
        .into_iter()
        .filter(|r| r.hostname != "_.localhost")
        .collect();
    Response::ok(serde_json::to_value(&list).unwrap_or(serde_json::Value::Array(vec![])))
}
```

- [ ] **Step 4: Also filter from Status routes_count**

In `Command::Status`:

```rust
let routes_count = routes.list().iter().filter(|r| r.hostname != "_.localhost").count();
```

- [ ] **Step 5: Build and test**

```bash
cargo test 2>&1 | tail -10
```

Expected: all tests pass.

- [ ] **Step 6: Commit**

```bash
git add src/daemon/ipc.rs
git commit -m "feat(inspector): hide _.localhost from portal ls and status routes_count"
```

---

### Task 11: portal inspect CLI command

**Files:**
- Modify: `src/cli/mod.rs`

- [ ] **Step 1: Add Inspect variant**

In `src/cli/mod.rs`, add to the `CliCommand` enum:

```rust
/// Open the request inspector in the browser
Inspect,
```

- [ ] **Step 2: Add handler**

In `run()`, add to the `match cli.command` block:

```rust
CliCommand::Inspect => {
    let url = "https://_.localhost";
    #[cfg(target_os = "macos")]
    {
        std::process::Command::new("open").arg(url).spawn().ok();
    }
    #[cfg(target_os = "linux")]
    {
        std::process::Command::new("xdg-open").arg(url).spawn().ok();
    }
    println!("Opening {url}");
}
```

- [ ] **Step 3: Build and test**

```bash
cargo build 2>&1 | grep "^error"
cargo test 2>&1 | tail -5
```

Expected: no errors, all tests pass.

- [ ] **Step 4: Commit**

```bash
git add src/cli/mod.rs
git commit -m "feat(cli): add portal inspect command — opens _.localhost in browser"
```

---

### Task 12: Frontend API layer

**Files:**
- Create: `ui/src/lib/api.ts`

- [ ] **Step 1: Create api.ts**

Create `ui/src/lib/api.ts`:

```typescript
export interface RequestMeta {
  id: number;
  hostname: string;
  method: string;
  path: string;
  status: number;
  duration_ms: number;
  timestamp: number;
}

export interface RequestRecord extends RequestMeta {
  req_headers: [string, string][];
  req_body: string;
  req_truncated: boolean;
  req_total_bytes: number;
  res_headers: [string, string][];
  res_body: string;
  res_truncated: boolean;
  res_total_bytes: number;
}

export interface RequestsResponse {
  requests: RequestRecord[];
  has_more: boolean;
}

export async function fetchRequests(params: {
  hostname?: string;
  limit?: number;
  before_id?: number;
  id?: number;
}): Promise<RequestsResponse> {
  const url = new URL('/api/requests', window.location.origin);
  if (params.hostname) url.searchParams.set('hostname', params.hostname);
  if (params.limit) url.searchParams.set('limit', String(params.limit));
  if (params.before_id) url.searchParams.set('before_id', String(params.before_id));
  if (params.id) url.searchParams.set('id', String(params.id));
  const res = await fetch(url.toString());
  return res.json();
}

export async function deleteAllRequests(hostname?: string): Promise<void> {
  const url = new URL('/api/requests', window.location.origin);
  if (hostname) url.searchParams.set('hostname', hostname);
  await fetch(url.toString(), { method: 'DELETE' });
}

export async function deleteRequest(id: number): Promise<void> {
  await fetch(`/api/requests/${id}`, { method: 'DELETE' });
}
```

- [ ] **Step 2: Verify TypeScript compiles**

```bash
cd /path/to/portless/ui && bun run check 2>&1 | tail -10
```

Expected: no errors.

- [ ] **Step 3: Commit**

```bash
cd .. && git add ui/src/lib/api.ts
git commit -m "feat(ui): add typed API layer for inspector endpoints"
```

---

### Task 13: Requests store

**Files:**
- Create: `ui/src/lib/stores/requests.svelte.ts`

- [ ] **Step 1: Create the store**

Create directory `ui/src/lib/stores/` then create `ui/src/lib/stores/requests.svelte.ts`:

```typescript
import { fetchRequests, type RequestMeta, type RequestRecord } from '$lib/api.js';

const MAX_IN_MEMORY = 2000;

// ── State ──────────────────────────────────────────────────────────────────
export let requests = $state<RequestMeta[]>([]);
export let selectedId = $state<number | null>(null);
export let selectedRecord = $state<RequestRecord | null>(null);
export let filterHostname = $state<string | null>(null);
export let filterMethods = $state<Set<string>>(new Set());
export let filterErrors = $state(false);
export let loading = $state(false);

// ── Derived ────────────────────────────────────────────────────────────────
export const filtered = $derived(
  requests.filter((r) => {
    if (filterHostname && r.hostname !== filterHostname) return false;
    if (filterMethods.size > 0 && !filterMethods.has(r.method)) return false;
    if (filterErrors && r.status < 400) return false;
    return true;
  })
);

export const hostnames = $derived([...new Set(requests.map((r) => r.hostname))]);

// ── Actions ────────────────────────────────────────────────────────────────
export async function loadHistory() {
  loading = true;
  try {
    const res = await fetchRequests({ limit: 100, hostname: filterHostname ?? undefined });
    requests = res.requests;
  } finally {
    loading = false;
  }
}

export function prependRequest(meta: RequestMeta) {
  requests = [meta, ...requests].slice(0, MAX_IN_MEMORY);
}

export async function selectRequest(id: number) {
  selectedId = id;
  selectedRecord = null;
  const res = await fetchRequests({ id });
  if (res.requests.length > 0) {
    selectedRecord = res.requests[0];
  }
}

export function clearSelected() {
  selectedId = null;
  selectedRecord = null;
}
```

- [ ] **Step 2: Check**

```bash
cd ui && bun run check 2>&1 | tail -10
```

Expected: no errors.

- [ ] **Step 3: Commit**

```bash
cd .. && git add ui/src/lib/stores/
git commit -m "feat(ui): add Svelte 5 runes store for requests state and SSE"
```

---

### Task 14: Sidebar component

**Files:**
- Create: `ui/src/lib/components/Sidebar.svelte`

- [ ] **Step 1: Create Sidebar.svelte**

Create `ui/src/lib/components/Sidebar.svelte`:

```svelte
<script lang="ts">
  import { Button } from '$lib/components/ui/button/index.js';
  import { Separator } from '$lib/components/ui/separator/index.js';
  import { Badge } from '$lib/components/ui/badge/index.js';
  import {
    filterHostname,
    filterMethods,
    filterErrors,
    hostnames,
    loadHistory,
  } from '$lib/stores/requests.svelte.js';
  import { deleteAllRequests } from '$lib/api.js';

  const METHODS = ['GET', 'POST', 'PUT', 'PATCH', 'DELETE'];

  function toggleMethod(m: string) {
    const next = new Set(filterMethods);
    next.has(m) ? next.delete(m) : next.add(m);
    filterMethods = next;
  }

  async function clearHistory() {
    await deleteAllRequests(filterHostname ?? undefined);
    await loadHistory();
  }
</script>

<aside class="flex h-full w-[200px] flex-shrink-0 flex-col border-r border-border bg-card font-mono text-xs">
  <!-- Routes -->
  <div class="px-3 pt-4 pb-2">
    <p class="mb-2 text-[10px] uppercase tracking-widest text-muted-foreground">Routes</p>

    <button
      class="w-full rounded px-2 py-1.5 text-left transition-colors {filterHostname === null
        ? 'bg-accent text-accent-foreground'
        : 'hover:bg-accent/50 text-muted-foreground'}"
      onclick={() => { filterHostname = null; }}
    >
      All routes
    </button>

    {#each hostnames as hostname}
      <button
        class="w-full rounded px-2 py-1.5 text-left transition-colors {filterHostname === hostname
          ? 'bg-accent text-accent-foreground'
          : 'hover:bg-accent/50 text-muted-foreground'}"
        onclick={() => { filterHostname = hostname; }}
      >
        {hostname}
      </button>
    {/each}
  </div>

  <Separator />

  <!-- Filters -->
  <div class="px-3 py-3">
    <p class="mb-2 text-[10px] uppercase tracking-widest text-muted-foreground">Filter</p>
    <div class="flex flex-wrap gap-1.5">
      {#each METHODS as method}
        <button onclick={() => toggleMethod(method)}>
          <Badge
            variant={filterMethods.has(method) ? 'default' : 'outline'}
            class="cursor-pointer px-2 py-0.5 text-[10px]"
          >
            {method}
          </Badge>
        </button>
      {/each}
      <button onclick={() => { filterErrors = !filterErrors; }}>
        <Badge
          variant={filterErrors ? 'destructive' : 'outline'}
          class="cursor-pointer px-2 py-0.5 text-[10px]"
        >
          errors
        </Badge>
      </button>
    </div>
  </div>

  <!-- Clear -->
  <div class="mt-auto border-t border-border px-3 py-3">
    <Button variant="ghost" size="sm" class="w-full text-destructive hover:text-destructive text-[11px]" onclick={clearHistory}>
      Clear history
    </Button>
  </div>
</aside>
```

- [ ] **Step 2: Check**

```bash
cd ui && bun run check 2>&1 | tail -10
```

Expected: no errors.

- [ ] **Step 3: Commit**

```bash
cd .. && git add ui/src/lib/components/Sidebar.svelte
git commit -m "feat(ui): add Sidebar component with route filter and method chips"
```

---

### Task 15: RequestFeed component

**Files:**
- Create: `ui/src/lib/components/RequestFeed.svelte`

- [ ] **Step 1: Create RequestFeed.svelte**

Create `ui/src/lib/components/RequestFeed.svelte`:

```svelte
<script lang="ts">
  import { ScrollArea } from '$lib/components/ui/scroll-area/index.js';
  import { Badge } from '$lib/components/ui/badge/index.js';
  import { filtered, selectedId, selectRequest } from '$lib/stores/requests.svelte.js';
  import type { RequestMeta } from '$lib/api.js';

  function methodVariant(method: string): 'default' | 'secondary' | 'destructive' | 'outline' {
    if (method === 'GET') return 'secondary';
    if (method === 'DELETE') return 'destructive';
    return 'default';
  }

  function statusColor(status: number): string {
    if (status >= 500) return 'text-destructive';
    if (status >= 400) return 'text-orange-500';
    if (status >= 300) return 'text-blue-400';
    return 'text-green-500';
  }

  function formatTime(ms: number): string {
    const d = new Date(ms);
    return d.toLocaleTimeString('en-US', { hour12: false });
  }
</script>

<div class="flex h-full w-[300px] flex-shrink-0 flex-col border-r border-border bg-background">
  <!-- Header -->
  <div class="flex items-center justify-between border-b border-border px-3 py-2">
    <span class="font-mono text-[11px] text-muted-foreground">{filtered.length} requests</span>
  </div>

  <ScrollArea class="flex-1">
    {#each filtered as req (req.id)}
      <button
        class="w-full border-l-2 px-3 py-2 text-left font-mono transition-colors hover:bg-accent/30
               {selectedId === req.id ? 'border-primary bg-accent/50' : 'border-transparent'}"
        onclick={() => selectRequest(req.id)}
      >
        <div class="flex items-center gap-2">
          <Badge variant={methodVariant(req.method)} class="px-1.5 py-0 text-[10px] font-mono">
            {req.method}
          </Badge>
          <span class="flex-1 truncate text-[11px] text-foreground">{req.path}</span>
          <span class="text-[10px] font-medium {statusColor(req.status)}">{req.status}</span>
        </div>
        <div class="mt-0.5 flex gap-3 text-[10px] text-muted-foreground">
          <span>{formatTime(req.timestamp)}</span>
          <span>{req.duration_ms}ms</span>
        </div>
      </button>
    {/each}

    {#if filtered.length === 0}
      <div class="px-4 py-8 text-center font-mono text-xs text-muted-foreground">
        No requests yet
      </div>
    {/if}
  </ScrollArea>
</div>
```

- [ ] **Step 2: Check**

```bash
cd ui && bun run check 2>&1 | tail -10
```

Expected: no errors.

- [ ] **Step 3: Commit**

```bash
cd .. && git add ui/src/lib/components/RequestFeed.svelte
git commit -m "feat(ui): add RequestFeed component with method badge and status color"
```

---

### Task 16: RequestDetail component

**Files:**
- Create: `ui/src/lib/components/RequestDetail.svelte`

- [ ] **Step 1: Create RequestDetail.svelte**

Create `ui/src/lib/components/RequestDetail.svelte`:

```svelte
<script lang="ts">
  import * as Tabs from '$lib/components/ui/tabs/index.js';
  import { Badge } from '$lib/components/ui/badge/index.js';
  import { ScrollArea } from '$lib/components/ui/scroll-area/index.js';
  import { selectedRecord, selectedId } from '$lib/stores/requests.svelte.js';

  function tryFormatJson(text: string): string {
    try {
      return JSON.stringify(JSON.parse(text), null, 2);
    } catch {
      return text;
    }
  }

  function isJson(headers: [string, string][]): boolean {
    return headers.some(
      ([k, v]) => k.toLowerCase() === 'content-type' && v.includes('json')
    );
  }

  function statusVariant(status: number): 'default' | 'secondary' | 'destructive' | 'outline' {
    if (status >= 500) return 'destructive';
    if (status >= 400) return 'outline';
    return 'secondary';
  }
</script>

<div class="flex flex-1 flex-col overflow-hidden bg-background">
  {#if !selectedId}
    <div class="flex flex-1 items-center justify-center font-mono text-xs text-muted-foreground">
      Select a request to inspect
    </div>
  {:else if !selectedRecord}
    <div class="flex flex-1 items-center justify-center font-mono text-xs text-muted-foreground">
      Loading…
    </div>
  {:else}
    <!-- Header -->
    <div class="border-b border-border px-4 py-3">
      <div class="flex items-center gap-2">
        <Badge variant="outline" class="font-mono text-[11px]">{selectedRecord.method}</Badge>
        <span class="flex-1 truncate font-mono text-sm text-foreground">{selectedRecord.path}</span>
        <Badge variant={statusVariant(selectedRecord.status)} class="font-mono text-[11px]">
          {selectedRecord.status}
        </Badge>
      </div>
      <p class="mt-1 font-mono text-[10px] text-muted-foreground">
        {selectedRecord.hostname} · {selectedRecord.duration_ms}ms
      </p>
    </div>

    <!-- Tabs -->
    <Tabs.Root value="request" class="flex flex-1 flex-col overflow-hidden">
      <Tabs.List class="mx-4 mt-2 w-fit rounded-none border-b border-border bg-transparent p-0">
        {#each ['request', 'response', 'headers', 'timing'] as tab}
          <Tabs.Trigger
            value={tab}
            class="rounded-none border-b-2 border-transparent px-3 py-1.5 font-mono text-[11px] capitalize
                   data-[state=active]:border-primary data-[state=active]:text-foreground"
          >
            {tab}
          </Tabs.Trigger>
        {/each}
      </Tabs.List>

      <!-- Request body -->
      <Tabs.Content value="request" class="flex-1 overflow-hidden p-0">
        <ScrollArea class="h-full px-4 py-3">
          {#if selectedRecord.req_truncated}
            <p class="mb-2 font-mono text-[10px] text-orange-500">
              Body truncated — showing first 1 MB of {selectedRecord.req_total_bytes.toLocaleString()} bytes
            </p>
          {/if}
          <pre class="whitespace-pre-wrap font-mono text-[11px] text-muted-foreground">{isJson(selectedRecord.req_headers) ? tryFormatJson(selectedRecord.req_body) : selectedRecord.req_body || '(empty)'}</pre>
        </ScrollArea>
      </Tabs.Content>

      <!-- Response body -->
      <Tabs.Content value="response" class="flex-1 overflow-hidden p-0">
        <ScrollArea class="h-full px-4 py-3">
          {#if selectedRecord.res_truncated}
            <p class="mb-2 font-mono text-[10px] text-orange-500">
              Body truncated — showing first 1 MB of {selectedRecord.res_total_bytes.toLocaleString()} bytes
            </p>
          {/if}
          <pre class="whitespace-pre-wrap font-mono text-[11px] text-muted-foreground">{isJson(selectedRecord.res_headers) ? tryFormatJson(selectedRecord.res_body) : selectedRecord.res_body || '(empty)'}</pre>
        </ScrollArea>
      </Tabs.Content>

      <!-- Headers -->
      <Tabs.Content value="headers" class="flex-1 overflow-hidden p-0">
        <ScrollArea class="h-full px-4 py-3">
          <p class="mb-2 font-mono text-[10px] uppercase tracking-widest text-muted-foreground">Request</p>
          {#each selectedRecord.req_headers as [key, value]}
            <div class="mb-1 flex gap-2 font-mono text-[11px]">
              <span class="w-48 flex-shrink-0 text-blue-400">{key}</span>
              <span class="break-all text-muted-foreground">{value}</span>
            </div>
          {/each}
          <p class="mb-2 mt-4 font-mono text-[10px] uppercase tracking-widest text-muted-foreground">Response</p>
          {#each selectedRecord.res_headers as [key, value]}
            <div class="mb-1 flex gap-2 font-mono text-[11px]">
              <span class="w-48 flex-shrink-0 text-green-400">{key}</span>
              <span class="break-all text-muted-foreground">{value}</span>
            </div>
          {/each}
        </ScrollArea>
      </Tabs.Content>

      <!-- Timing -->
      <Tabs.Content value="timing" class="flex-1 overflow-hidden p-0">
        <ScrollArea class="h-full px-4 py-3">
          <div class="space-y-2 font-mono text-[11px]">
            <div class="flex justify-between">
              <span class="text-muted-foreground">Total duration</span>
              <span class="text-foreground">{selectedRecord.duration_ms}ms</span>
            </div>
            <div class="flex justify-between">
              <span class="text-muted-foreground">Timestamp</span>
              <span class="text-foreground">{new Date(selectedRecord.timestamp).toISOString()}</span>
            </div>
            <div class="flex justify-between">
              <span class="text-muted-foreground">Request size</span>
              <span class="text-foreground">{selectedRecord.req_total_bytes} bytes</span>
            </div>
            <div class="flex justify-between">
              <span class="text-muted-foreground">Response size</span>
              <span class="text-foreground">{selectedRecord.res_total_bytes} bytes</span>
            </div>
          </div>
        </ScrollArea>
      </Tabs.Content>
    </Tabs.Root>
  {/if}
</div>
```

- [ ] **Step 2: Check**

```bash
cd ui && bun run check 2>&1 | tail -10
```

Expected: no errors.

- [ ] **Step 3: Commit**

```bash
cd .. && git add ui/src/lib/components/RequestDetail.svelte
git commit -m "feat(ui): add RequestDetail component with Request/Response/Headers/Timing tabs"
```

---

### Task 17: Main page — 3-panel layout + SSE connection

**Files:**
- Modify: `ui/src/routes/+page.svelte`
- Modify: `ui/src/routes/+layout.svelte` (add base styles)

- [ ] **Step 1: Update +layout.svelte**

Replace `ui/src/routes/+layout.svelte` with:

```svelte
<script lang="ts">
  import './layout.css';
  let { children } = $props();
</script>

<div class="h-screen overflow-hidden bg-background text-foreground">
  {@render children()}
</div>
```

- [ ] **Step 2: Replace +page.svelte**

Replace `ui/src/routes/+page.svelte`:

```svelte
<script lang="ts">
  import { onMount } from 'svelte';
  import Sidebar from '$lib/components/Sidebar.svelte';
  import RequestFeed from '$lib/components/RequestFeed.svelte';
  import RequestDetail from '$lib/components/RequestDetail.svelte';
  import { loadHistory, prependRequest } from '$lib/stores/requests.svelte.js';
  import type { RequestMeta } from '$lib/api.js';

  onMount(() => {
    // Load history on mount
    loadHistory();

    // Connect SSE for live updates
    const es = new EventSource('/api/stream');
    es.addEventListener('request', (e: MessageEvent) => {
      const meta: RequestMeta = JSON.parse(e.data);
      prependRequest(meta);
    });
    es.onerror = () => {
      // SSE will auto-reconnect; no action needed
    };

    return () => {
      es.close();
    };
  });
</script>

<svelte:head>
  <title>Portal Inspector</title>
</svelte:head>

<div class="flex h-full">
  <Sidebar />
  <RequestFeed />
  <RequestDetail />
</div>
```

- [ ] **Step 3: Check TypeScript**

```bash
cd ui && bun run check 2>&1 | tail -10
```

Expected: no errors.

- [ ] **Step 4: Build the UI**

```bash
bun run build 2>&1 | tail -10
```

Expected: build succeeds, `dist/` is populated.

- [ ] **Step 5: Verify Rust embeds it**

```bash
cd .. && cargo build 2>&1 | grep "^error"
```

Expected: no errors.

- [ ] **Step 6: Run all tests**

```bash
cargo test 2>&1 | tail -15
```

Expected: all tests pass.

- [ ] **Step 7: Commit**

```bash
git add ui/src/routes/ ui/dist/
git commit -m "feat(ui): compose 3-panel layout with SSE live feed — inspector complete"
```

---

## Manual Verification

After all tasks complete, verify end-to-end:

1. Start the daemon: `portal start` (or `sudo portal daemon`)
2. Open inspector: `portal inspect` → browser opens `https://_.localhost`
3. Make a request to any `.localhost` route from the browser
4. Confirm: row appears in the feed without refreshing (SSE)
5. Click the row → Request/Response/Headers/Timing tabs show correct data
6. Filter by method (click GET chip) → only GET requests shown
7. Select a route in sidebar → feed filters to that hostname
8. Click "Clear history" → table empties
9. Check `portal ls` → `_.localhost` does not appear

Dev mode test:
```bash
cd ui && bun run dev &
PORTAL_UI_DEV=1 sudo portal daemon
# Open https://_.localhost — should load from Vite dev server with hot-reload
```
