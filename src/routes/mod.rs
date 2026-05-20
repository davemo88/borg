//! HTTP routing and shared application state.

pub mod lifecycle;
pub mod send;
pub mod stream;
pub mod sync;

use std::sync::Arc;

use axum::Router;
use axum::routing::{get, post};
use tokio::sync::watch;
use tower_http::trace::TraceLayer;

use crate::clock::ServerClock;
use crate::config::Config;
use crate::registry::Registry;
use crate::static_files;

/// State shared by every request handler. Cheap to clone.
#[derive(Clone)]
pub struct AppState {
    pub clock: Arc<ServerClock>,
    pub registry: Arc<Registry>,
    pub cfg: Config,
    /// Flips to `true` on graceful shutdown; SSE streams watch it to close.
    pub shutdown: watch::Receiver<bool>,
}

/// Build the full application router.
pub fn router(state: AppState) -> Router {
    Router::new()
        .route("/", get(static_files::index))
        .route("/api/borg", post(lifecycle::create))
        .route("/api/borg/{join}/join", post(lifecycle::join))
        .route("/api/borg/{join}/stream", get(stream::stream))
        .route("/api/borg/{join}/line", post(send::send_line))
        .route("/api/borg/{join}/rtt", post(sync::rtt))
        .route("/api/time", get(sync::time))
        .layer(TraceLayer::new_for_http())
        .with_state(state)
}
