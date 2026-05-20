//! Every type that crosses the network. The single source of wire truth.
//!
//! All `*_us` fields are microseconds in the server clock domain
//! (see [`crate::clock::ServerClock`]), except `TimeQuery::t0` / `TimeResponse::t0`
//! which are echoed verbatim from the client's own clock.

use serde::{Deserialize, Serialize};

/// Per-word sweep timing, relative to the line's own start (offset 0).
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct WordTiming {
    pub text: String,
    pub start_us: u64,
    pub end_us: u64,
}

/// What an input adaptor emits: a line of words with purely *relative* timing.
/// The server adds the absolute anchor — adaptors never see `display_at`.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct LineSpec {
    pub words: Vec<WordTiming>,
    pub total_duration_us: u64,
}

/// A line ready to broadcast — the server has anchored it in absolute time.
/// Sent as the SSE `line` event.
#[derive(Clone, Debug, Serialize)]
pub struct BroadcastLine {
    pub seq: u64,
    /// Absolute server time at which word 0's sweep begins.
    pub display_at_us: u64,
    pub total_duration_us: u64,
    pub words: Vec<WordTiming>,
}

/// Sent as the SSE `hello` event when a client (re)connects.
#[derive(Clone, Debug, Serialize)]
pub struct Hello {
    pub client_id: String,
    pub server_time_us: u64,
    pub lead_time_us: u64,
}

/// Query for `GET /api/time` — the client's send timestamp, echoed back.
#[derive(Debug, Deserialize)]
pub struct TimeQuery {
    pub t0: i64,
}

/// Response for `GET /api/time` — the four-timestamp clock-sync triple.
#[derive(Debug, Serialize)]
pub struct TimeResponse {
    pub t0: i64,
    pub server_recv_us: u64,
    pub server_send_us: u64,
}

/// Response for `POST /api/borg` (create).
#[derive(Debug, Serialize)]
pub struct CreateResponse {
    pub borg_id: String,
    pub join_code: String,
    pub master_code: String,
    pub client_id: String,
    pub stream_url: String,
    pub server_time_us: u64,
}

/// Response for `POST /api/borg/{join}/join`.
#[derive(Debug, Serialize)]
pub struct JoinResponse {
    pub join_code: String,
    pub client_id: String,
    pub stream_url: String,
    pub server_time_us: u64,
}

/// Body for `POST /api/borg/{join}/line` (master-authenticated send).
#[derive(Debug, Deserialize)]
pub struct SendLineRequest {
    pub master_code: String,
    pub text: String,
    /// Optional override of the total sweep duration.
    #[serde(default)]
    pub duration_us: Option<u64>,
}

/// Response for a successful send.
#[derive(Debug, Clone, Serialize)]
pub struct SendLineResponse {
    pub display_at_us: u64,
    pub word_count: usize,
    pub total_duration_us: u64,
}

/// Body for `POST /api/borg/{join}/rtt` — a client's measured latency.
#[derive(Debug, Deserialize)]
pub struct RttReport {
    pub client_id: String,
    pub one_way_us: u64,
}
