//! borg — a super-low-latency synchronized karaoke / teleprompter server.

mod adaptor;
mod borg;
mod clock;
mod codes;
mod config;
mod protocol;
mod registry;
mod routes;
mod static_files;
mod timing;

use std::sync::Arc;
use std::time::Duration;

use tokio::sync::watch;
use tracing_subscriber::EnvFilter;

#[tokio::main]
async fn main() {
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| EnvFilter::new("borg=info,tower_http=info")),
        )
        .init();

    let cfg = config::Config::from_env();
    let clock = Arc::new(clock::ServerClock::new());
    let registry = Arc::new(registry::Registry::new());
    let (shutdown_tx, shutdown_rx) = watch::channel(false);

    let state = routes::AppState {
        clock: clock.clone(),
        registry: registry.clone(),
        cfg: cfg.clone(),
        shutdown: shutdown_rx,
    };
    let app = routes::router(state);

    let listener = tokio::net::TcpListener::bind(cfg.bind)
        .await
        .expect("bind listener");
    tracing::info!(addr = %cfg.bind, "borg listening");

    let shutdown = async move {
        let _ = tokio::signal::ctrl_c().await;
        tracing::info!("shutdown signal received — closing borgs");
        let _ = shutdown_tx.send(true);
        // Give SSE streams a moment to emit `borg_closed` and end cleanly.
        tokio::time::sleep(Duration::from_millis(300)).await;
    };

    axum::serve(listener, app)
        .with_graceful_shutdown(shutdown)
        .await
        .expect("serve");
}
