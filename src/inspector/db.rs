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
