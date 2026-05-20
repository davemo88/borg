//! Runtime configuration, read from environment variables with sane defaults.

use std::net::SocketAddr;

/// Tunable parameters for the server. Cheap to clone.
#[derive(Clone, Debug)]
pub struct Config {
    /// Address the HTTP server binds to.
    pub bind: SocketAddr,
    /// Reading rate used by the manual adaptor when no duration is given.
    pub default_wpm: u32,
    /// Lower bound for the synchronized display lead time.
    pub lead_min_us: u64,
    /// Upper bound for the lead time — a client slower than this lags.
    pub lead_cap_us: u64,
    /// Safety margin added on top of the slowest client's one-way latency.
    pub jitter_margin_us: u64,
    /// One-way latency assumed for a client that has not reported yet.
    pub default_one_way_us: u64,
}

impl Config {
    /// Build the config from `BORG_*` environment variables.
    pub fn from_env() -> Self {
        let bind = std::env::var("BORG_BIND")
            .ok()
            .and_then(|s| s.parse().ok())
            .unwrap_or_else(|| SocketAddr::from(([127, 0, 0, 1], 8080)));
        Config {
            bind,
            default_wpm: env_num("BORG_WPM", 200),
            lead_min_us: env_num("BORG_LEAD_MIN_US", 80_000),
            lead_cap_us: env_num("BORG_LEAD_CAP_US", 400_000),
            jitter_margin_us: env_num("BORG_JITTER_US", 25_000),
            default_one_way_us: env_num("BORG_DEFAULT_ONE_WAY_US", 60_000),
        }
    }
}

fn env_num<T: std::str::FromStr>(key: &str, default: T) -> T {
    std::env::var(key)
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(default)
}
