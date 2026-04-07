pub mod assets;
pub mod db;
pub mod server;
pub mod sse;
pub mod types;

use std::path::PathBuf;

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
    pub async fn start(db_path: PathBuf) -> crate::error::Result<Inspector> {
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
        let port = crate::ports::find_free_port(3000, 9999)?;

        // Start axum server
        let state = AppState { db, sse_tx };
        let app = router(state);
        let listener = tokio::net::TcpListener::bind(format!("127.0.0.1:{port}"))
            .await
            .map_err(|e| crate::error::Error::Ipc(e.to_string()))?;

        tokio::spawn(async move {
            axum::serve(listener, app).await.ok();
        });

        Ok(Inspector {
            sender: InspectorSender(tx),
            port,
        })
    }
}
