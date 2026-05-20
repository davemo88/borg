//! The master-authenticated send-text endpoint (the manual input adaptor).

use axum::Json;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use tokio::sync::oneshot;

use crate::borg::BorgCommand;
use crate::codes;
use crate::protocol::SendLineRequest;
use crate::routes::AppState;

/// `POST /api/borg/{join}/line` — broadcast a line to the borg.
pub async fn send_line(
    State(state): State<AppState>,
    Path(join_raw): Path<String>,
    Json(req): Json<SendLineRequest>,
) -> Response {
    let join = codes::normalize_join_code(&join_raw);
    let handle = match state.registry.get(&join) {
        Some(h) => h,
        None => return (StatusCode::NOT_FOUND, "no borg with that code").into_response(),
    };
    if !state.registry.verify_master(&req.master_code, &join) {
        return (StatusCode::UNAUTHORIZED, "wrong master code").into_response();
    }
    let text = req.text.trim().to_string();
    if text.is_empty() {
        return (StatusCode::BAD_REQUEST, "empty text").into_response();
    }

    let (tx, rx) = oneshot::channel();
    if handle
        .cmd
        .send(BorgCommand::SubmitText { text, duration_us: req.duration_us, reply: tx })
        .await
        .is_err()
    {
        return (StatusCode::GONE, "borg is closed").into_response();
    }
    match rx.await {
        Ok(resp) => (StatusCode::ACCEPTED, Json(resp)).into_response(),
        Err(_) => (StatusCode::INTERNAL_SERVER_ERROR, "no line produced").into_response(),
    }
}
