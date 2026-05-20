//! The per-borg actor: one task that owns all mutable room state.
//!
//! Chosen over a shared/sharded map because the hot path is fan-out *within*
//! one room: a single owner mutates the broadcast channel with zero locks, and
//! the actor is the one `select!` site merging client commands and adaptor
//! events. The broadcast payload is serialized exactly once per line and shared
//! as `Bytes` (a refcount bump per subscriber — no per-client work).

use std::collections::{HashMap, VecDeque};
use std::sync::Arc;

use bytes::Bytes;
use tokio::sync::{broadcast, mpsc, oneshot};

use crate::adaptor::{AdaptorControl, AdaptorEvent, AdaptorHandle, InputAdaptor};
use crate::clock::ServerClock;
use crate::config::Config;
use crate::protocol::{BroadcastLine, LineSpec, SendLineResponse};

/// Capacity of the per-borg command and broadcast channels.
const CHANNEL_CAP: usize = 64;

/// A command sent to a borg actor over its mpsc channel.
pub enum BorgCommand {
    /// An SSE connection wants to receive this borg's broadcast.
    Subscribe {
        client_id: String,
        reply: oneshot::Sender<SubscribeResult>,
    },
    /// The borg master submitted text to broadcast.
    SubmitText {
        text: String,
        duration_us: Option<u64>,
        reply: oneshot::Sender<SendLineResponse>,
    },
    /// A client reported its measured network latency.
    RttReport { client_id: String, one_way_us: u64 },
    /// A client's SSE connection dropped.
    ClientGone { client_id: String },
}

/// What a subscriber receives when it joins the broadcast.
pub struct SubscribeResult {
    pub rx: broadcast::Receiver<Bytes>,
    pub server_time_us: u64,
    pub lead_time_us: u64,
}

struct ClientInfo {
    one_way_us: u64,
}

/// The per-borg actor. Constructed and immediately spawned by [`Borg::spawn`].
pub struct Borg {
    join_code: String,
    clock: Arc<ServerClock>,
    cfg: Config,
    seq: u64,
    bcast: broadcast::Sender<Bytes>,
    clients: HashMap<String, ClientInfo>,
    adaptor_control: mpsc::Sender<AdaptorControl>,
    /// Reply channels for in-flight `SubmitText`s, matched 1:1 to adaptor lines.
    pending: VecDeque<oneshot::Sender<SendLineResponse>>,
}

impl Borg {
    /// Spawn a borg actor and return the channel used to command it.
    /// The borg's input source is whatever [`InputAdaptor`] the caller supplies.
    pub fn spawn(
        join_code: String,
        clock: Arc<ServerClock>,
        cfg: Config,
        adaptor: Box<dyn InputAdaptor>,
    ) -> mpsc::Sender<BorgCommand> {
        let (cmd_tx, cmd_rx) = mpsc::channel(CHANNEL_CAP);
        let (bcast_tx, _) = broadcast::channel(CHANNEL_CAP);

        tracing::info!(borg = %join_code, adaptor = adaptor.name(), "spawning borg");
        let AdaptorHandle { events, control } = adaptor.start(clock.clone());

        let actor = Borg {
            join_code,
            clock,
            cfg,
            seq: 0,
            bcast: bcast_tx,
            clients: HashMap::new(),
            adaptor_control: control,
            pending: VecDeque::new(),
        };
        tokio::spawn(actor.run(cmd_rx, events));
        cmd_tx
    }

    async fn run(
        mut self,
        mut cmd_rx: mpsc::Receiver<BorgCommand>,
        mut adaptor_events: mpsc::Receiver<AdaptorEvent>,
    ) {
        tracing::info!(borg = %self.join_code, "borg actor started");
        loop {
            tokio::select! {
                cmd = cmd_rx.recv() => match cmd {
                    Some(cmd) => self.handle_cmd(cmd),
                    None => break, // all handles dropped
                },
                ev = adaptor_events.recv() => match ev {
                    Some(AdaptorEvent::Line(spec)) => self.handle_line(spec),
                    Some(AdaptorEvent::Closed) | None => break,
                },
            }
        }
        // Tell the adaptor to wind down its own task.
        let _ = self.adaptor_control.try_send(AdaptorControl::Stop);
        tracing::info!(borg = %self.join_code, "borg actor stopped");
    }

    fn handle_cmd(&mut self, cmd: BorgCommand) {
        match cmd {
            BorgCommand::Subscribe { client_id, reply } => {
                self.clients
                    .entry(client_id)
                    .or_insert(ClientInfo { one_way_us: self.cfg.default_one_way_us });
                let _ = reply.send(SubscribeResult {
                    rx: self.bcast.subscribe(),
                    server_time_us: self.clock.now_micros(),
                    lead_time_us: self.lead_time_us(),
                });
            }
            BorgCommand::SubmitText { text, duration_us, reply } => {
                self.pending.push_back(reply);
                if self
                    .adaptor_control
                    .try_send(AdaptorControl::SubmitText { text, duration_us })
                    .is_err()
                {
                    // Adaptor unavailable: drop the reply we just queued so the
                    // HTTP handler observes the failure.
                    let _ = self.pending.pop_back();
                }
            }
            BorgCommand::RttReport { client_id, one_way_us } => {
                self.clients
                    .entry(client_id)
                    .and_modify(|c| c.one_way_us = one_way_us)
                    .or_insert(ClientInfo { one_way_us });
            }
            BorgCommand::ClientGone { client_id } => {
                self.clients.remove(&client_id);
            }
        }
    }

    fn handle_line(&mut self, spec: LineSpec) {
        let resp = self.broadcast_line(spec);
        if let Some(reply) = self.pending.pop_front() {
            let _ = reply.send(resp);
        }
    }

    /// Adaptive, capped lead time: the slowest client's one-way latency plus a
    /// jitter margin, clamped so one bad connection cannot delay the room.
    fn lead_time_us(&self) -> u64 {
        let slowest = self
            .clients
            .values()
            .map(|c| c.one_way_us)
            .max()
            .unwrap_or(self.cfg.default_one_way_us);
        (slowest + self.cfg.jitter_margin_us)
            .clamp(self.cfg.lead_min_us, self.cfg.lead_cap_us)
    }

    /// Anchor a line in absolute time, serialize it once, and fan it out.
    fn broadcast_line(&mut self, spec: LineSpec) -> SendLineResponse {
        self.seq += 1;
        let lead = self.lead_time_us();
        let display_at = self.clock.now_micros() + lead;
        let word_count = spec.words.len();
        let total = spec.total_duration_us;

        let line = BroadcastLine {
            seq: self.seq,
            display_at_us: display_at,
            total_duration_us: total,
            words: spec.words,
        };
        let json = serde_json::to_string(&line).expect("BroadcastLine is serializable");
        let frame = Bytes::from(format!("event: line\ndata: {json}\n\n"));

        let receivers = self.bcast.send(frame).unwrap_or(0);
        tracing::info!(
            borg = %self.join_code,
            seq = self.seq,
            lead_us = lead,
            clients = self.clients.len(),
            receivers,
            "broadcast line"
        );

        SendLineResponse {
            display_at_us: display_at,
            word_count,
            total_duration_us: total,
        }
    }
}
