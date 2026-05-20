//! The v1 adaptor: the borg master types lines by hand.

use std::sync::Arc;

use tokio::sync::mpsc;

use super::{AdaptorControl, AdaptorEvent, AdaptorHandle, InputAdaptor};
use crate::clock::ServerClock;
use crate::timing::estimate_line;

/// Turns plain text submitted by the borg master into word-timed lines,
/// estimating per-word sweep timing from a words-per-minute reading rate.
pub struct ManualTextAdaptor {
    wpm: u32,
}

impl ManualTextAdaptor {
    pub fn new(wpm: u32) -> Self {
        ManualTextAdaptor { wpm }
    }
}

impl InputAdaptor for ManualTextAdaptor {
    fn name(&self) -> &'static str {
        "manual-text"
    }

    fn start(self: Box<Self>, _clock: Arc<ServerClock>) -> AdaptorHandle {
        let (ev_tx, ev_rx) = mpsc::channel(32);
        let (ctl_tx, mut ctl_rx) = mpsc::channel(32);
        let wpm = self.wpm;

        tokio::spawn(async move {
            while let Some(ctl) = ctl_rx.recv().await {
                match ctl {
                    AdaptorControl::SubmitText { text, duration_us } => {
                        let spec = estimate_line(&text, wpm, duration_us);
                        if ev_tx.send(AdaptorEvent::Line(spec)).await.is_err() {
                            break;
                        }
                    }
                    AdaptorControl::Stop => {
                        let _ = ev_tx.send(AdaptorEvent::Closed).await;
                        break;
                    }
                }
            }
        });

        AdaptorHandle { events: ev_rx, control: ctl_tx }
    }
}
