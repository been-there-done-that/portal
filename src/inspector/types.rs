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
    pub content_type: Option<String>,
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
    pub content_type: Option<String>,
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
    pub content_type: Option<String>,
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
            content_type: None,
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
