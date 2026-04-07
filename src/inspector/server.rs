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
            limit + 1,
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
    if std::env::var("PORTAL_UI_DEV").is_ok() {
        return dev_proxy(uri).await;
    }
    serve_embedded(uri.path())
}

async fn dev_proxy(uri: Uri) -> Response {
    let vite_url = format!(
        "http://localhost:5173{}",
        uri.path_and_query().map(|p| p.as_str()).unwrap_or("/")
    );
    match hyper_get(&vite_url).await {
        Ok(resp) => resp,
        Err(_) => serve_embedded(uri.path()),
    }
}

async fn hyper_get(url: &str) -> Result<Response, ()> {
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
