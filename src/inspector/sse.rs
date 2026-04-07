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
