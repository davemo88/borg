//! The single server time domain.

use std::time::Instant;

/// A monotonic clock measuring microseconds since process start.
///
/// Every `*_us` value on the wire lives in this domain. It never goes
/// backwards and is immune to wall-clock/NTP adjustments.
#[derive(Debug)]
pub struct ServerClock {
    start: Instant,
}

impl ServerClock {
    pub fn new() -> Self {
        ServerClock { start: Instant::now() }
    }

    /// Microseconds elapsed since this clock was created.
    pub fn now_micros(&self) -> u64 {
        self.start.elapsed().as_micros() as u64
    }
}

impl Default for ServerClock {
    fn default() -> Self {
        Self::new()
    }
}
