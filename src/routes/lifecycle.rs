//! Borg lifecycle: create and join.

use axum::Json;
use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use serde::Deserialize;

use crate::adaptor::{InputAdaptor, LlmAdaptor, ManualTextAdaptor};
use crate::borg::Borg;
use crate::codes;
use crate::protocol::{CreateResponse, JoinResponse};
use crate::registry::BorgHandle;
use crate::routes::AppState;

#[derive(Deserialize)]
pub struct CreateQuery {
    pub wpm: Option<u32>,
    /// Input source for the borg: `manual` (default) or `llm`.
    pub adaptor: Option<String>,
}

/// `POST /api/borg` — create a new borg; the caller becomes the borg master.
pub async fn create(State(state): State<AppState>, Query(q): Query<CreateQuery>) -> Response {
    let wpm = q.wpm.unwrap_or(state.cfg.default_wpm).clamp(40, 1200);
    let kind = q.adaptor.as_deref().unwrap_or("manual");

    let adaptor: Box<dyn InputAdaptor> = match kind {
        "manual" => Box::new(ManualTextAdaptor::new(wpm)),
        "llm" => {
            if state.cfg.llm.api_key.is_none() {
                return (
                    StatusCode::BAD_REQUEST,
                    "LLM adaptor unavailable: ANTHROPIC_API_KEY is not set on the server",
                )
                    .into_response();
            }
            Box::new(LlmAdaptor::new(wpm, state.cfg.llm.clone()))
        }
        other => {
            return (StatusCode::BAD_REQUEST, format!("unknown adaptor: {other}"))
                .into_response();
        }
    };

    let master = codes::master_code();
    let clock = state.clock.clone();
    let cfg = state.cfg.clone();

    let join = state.registry.try_register(&master, move |join| {
        let cmd = Borg::spawn(join, clock, cfg, adaptor);
        BorgHandle { cmd }
    });

    tracing::info!(borg = %join, adaptor = kind, wpm, "borg created");

    let resp = CreateResponse {
        borg_id: join.clone(),
        join_code: join.clone(),
        master_code: master,
        client_id: codes::client_id(),
        stream_url: format!("/api/borg/{join}/stream"),
        server_time_us: state.clock.now_micros(),
    };
    (StatusCode::CREATED, Json(resp)).into_response()
}

/// `POST /api/borg/{join}/join` — join an existing borg as a viewer.
pub async fn join(State(state): State<AppState>, Path(join_raw): Path<String>) -> Response {
    let join = codes::normalize_join_code(&join_raw);
    if state.registry.get(&join).is_none() {
        return (StatusCode::NOT_FOUND, "no borg with that code").into_response();
    }
    let resp = JoinResponse {
        join_code: join.clone(),
        client_id: codes::client_id(),
        stream_url: format!("/api/borg/{join}/stream"),
        server_time_us: state.clock.now_micros(),
    };
    Json(resp).into_response()
}
