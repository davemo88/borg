//! Clock-sync endpoint and client RTT reporting.

use axum::Json;
use axum::extract::{Path, Query, State};
use axum::http::{StatusCode, header};
use axum::response::{IntoResponse, Response};

use crate::borg::BorgCommand;
use crate::codes;
use crate::protocol::{RttReport, TimeQuery, TimeResponse};
use crate::routes::AppState;

/// `GET /api/time?t0=` — the latency-critical clock-sync round trip.
/// `server_recv_us` is taken first and `server_send_us` last.
pub async fn time(State(state): State<AppState>, Query(q): Query<TimeQuery>) -> Response {
    let server_recv_us = state.clock.now_micros();
    let server_send_us = state.clock.now_micros();
    (
        [(header::CACHE_CONTROL, "no-store")],
        Json(TimeResponse { t0: q.t0, server_recv_us, server_send_us }),
    )
        .into_response()
}

/// `POST /api/borg/{join}/rtt` — a client reports its measured latency, which
/// feeds the borg's adaptive lead-time calculation.
pub async fn rtt(
    State(state): State<AppState>,
    Path(join_raw): Path<String>,
    Json(req): Json<RttReport>,
) -> Response {
    let join = codes::normalize_join_code(&join_raw);
    if let Some(handle) = state.registry.get(&join) {
        let _ = handle.cmd.try_send(BorgCommand::RttReport {
            client_id: req.client_id,
            one_way_us: req.one_way_us,
        });
    }
    StatusCode::NO_CONTENT.into_response()
}
