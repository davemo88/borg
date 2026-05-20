//! Input adaptors — pluggable sources of lines for a borg.
//!
//! An adaptor is a *source of [`LineSpec`]s over time*. Once started it pushes
//! events into a channel the borg actor owns, so the actor can treat adaptor
//! events uniformly alongside client commands in its `select!` loop. This shape
//! serves manual (caller-fed), push (live transcription), and timed-playback
//! adaptors equally — only the manual adaptor is implemented in v1.

use std::sync::Arc;

use tokio::sync::mpsc;

use crate::clock::ServerClock;
use crate::protocol::LineSpec;

pub mod manual;
pub use manual::ManualTextAdaptor;

/// An event produced by a running adaptor.
#[derive(Debug)]
pub enum AdaptorEvent {
    /// Emit this line now; the borg actor anchors and broadcasts it.
    Line(LineSpec),
    /// The adaptor has finished or its source disconnected.
    Closed,
}

/// A control message sent into a running adaptor.
#[derive(Debug)]
pub enum AdaptorControl {
    /// Feed text into a manual adaptor. Timed/push adaptors may ignore this.
    SubmitText { text: String, duration_us: Option<u64> },
    /// Ask the adaptor to stop.
    Stop,
}

/// The borg actor's handle on a running adaptor.
pub struct AdaptorHandle {
    /// Lines (and the eventual close) produced by the adaptor.
    pub events: mpsc::Receiver<AdaptorEvent>,
    /// Control channel into the adaptor.
    pub control: mpsc::Sender<AdaptorControl>,
}

/// A pluggable source of lines for a borg.
pub trait InputAdaptor: Send + 'static {
    /// A short identifier, used in logs.
    fn name(&self) -> &'static str;

    /// Start the adaptor. Implementations spawn whatever task they need and
    /// return a handle the borg actor uses to receive lines and send control.
    fn start(self: Box<Self>, clock: Arc<ServerClock>) -> AdaptorHandle;
}
