//! The SSE receive path — the broadcast channel for each client.

use std::convert::Infallible;
use std::time::Duration;

use axum::body::Body;
use axum::extract::{Path, Query, State};
use axum::http::{StatusCode, header};
use axum::response::{IntoResponse, Response};
use bytes::Bytes;
use serde::Deserialize;
use tokio::sync::{broadcast, mpsc, oneshot, watch};

use crate::borg::{BorgCommand, SubscribeResult};
use crate::codes;
use crate::protocol::Hello;
use crate::routes::AppState;

/// Interval between SSE keepalive comments.
const KEEPALIVE: Duration = Duration::from_secs(15);

#[derive(Deserialize)]
pub struct StreamQuery {
    pub client_id: String,
}

/// `GET /api/borg/{join}/stream?client_id=` — subscribe to a borg's broadcast.
pub async fn stream(
    State(state): State<AppState>,
    Path(join_raw): Path<String>,
    Query(q): Query<StreamQuery>,
) -> Response {
    let join = codes::normalize_join_code(&join_raw);
    let handle = match state.registry.get(&join) {
        Some(h) => h,
        None => return (StatusCode::NOT_FOUND, "no borg with that code").into_response(),
    };

    let (tx, rx) = oneshot::channel();
    if handle
        .cmd
        .send(BorgCommand::Subscribe { client_id: q.client_id.clone(), reply: tx })
        .await
        .is_err()
    {
        return (StatusCode::GONE, "borg is closed").into_response();
    }
    let SubscribeResult { rx: bcast_rx, server_time_us, lead_time_us } = match rx.await {
        Ok(s) => s,
        Err(_) => return (StatusCode::GONE, "borg is closed").into_response(),
    };

    let hello = Hello { client_id: q.client_id.clone(), server_time_us, lead_time_us };
    let hello_frame = Bytes::from(format!(
        "event: hello\ndata: {}\n\n",
        serde_json::to_string(&hello).expect("Hello is serializable")
    ));

    let body = build_body(
        hello_frame,
        bcast_rx,
        state.shutdown.clone(),
        handle.cmd.clone(),
        q.client_id,
    );

    Response::builder()
        .header(header::CONTENT_TYPE, "text/event-stream")
        .header(header::CACHE_CONTROL, "no-cache")
        .header("X-Accel-Buffering", "no")
        .body(body)
        .expect("valid SSE response")
}

/// Sends `ClientGone` to the borg actor when the SSE stream is dropped —
/// whether the client disconnected or the stream ended on shutdown.
struct ClientGuard {
    cmd: mpsc::Sender<BorgCommand>,
    client_id: String,
}

impl Drop for ClientGuard {
    fn drop(&mut self) {
        let _ = self.cmd.try_send(BorgCommand::ClientGone {
            client_id: std::mem::take(&mut self.client_id),
        });
    }
}

/// Build the SSE response body: a stream of complete, pre-formatted frames.
/// Each broadcast frame was serialized once by the borg actor and arrives here
/// as `Bytes` — this path does zero per-client serialization or allocation.
fn build_body(
    hello: Bytes,
    mut bcast_rx: broadcast::Receiver<Bytes>,
    mut shutdown: watch::Receiver<bool>,
    cmd: mpsc::Sender<BorgCommand>,
    client_id: String,
) -> Body {
    let stream = async_stream::stream! {
        let _guard = ClientGuard { cmd, client_id };
        yield Ok::<Bytes, Infallible>(hello);

        let mut keepalive = tokio::time::interval(KEEPALIVE);
        keepalive.tick().await; // discard the immediate first tick

        loop {
            tokio::select! {
                _ = shutdown.changed() => {
                    yield Ok(Bytes::from_static(b"event: borg_closed\ndata: {}\n\n"));
                    break;
                }
                _ = keepalive.tick() => {
                    yield Ok(Bytes::from_static(b": keepalive\n\n"));
                }
                msg = bcast_rx.recv() => match msg {
                    Ok(frame) => yield Ok(frame),
                    // Lagged: this client fell behind; skip stale frames.
                    Err(broadcast::error::RecvError::Lagged(_)) => continue,
                    Err(broadcast::error::RecvError::Closed) => break,
                },
            }
        }
    };
    Body::from_stream(stream)
}
