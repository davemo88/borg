//! The LLM adaptor: the borg becomes the spoken voice of a Claude conversation.
//!
//! Each text submitted to this adaptor is treated as the *counterparty's* turn
//! in an ongoing conversation. The adaptor appends it to the running history,
//! calls the Claude Messages API, appends the reply, and turns that reply into
//! a paced sequence of karaoke lines — so the borg "speaks" the model's words.
//!
//! Two techniques mask the model's latency:
//!
//! * **Streaming** — the Messages API is called with `stream: true`, so the
//!   reply is parsed sentence-by-sentence as tokens arrive; the borg can start
//!   speaking the first sentence long before the model has finished.
//! * **Canned filler** — while the first real sentence is still in flight, the
//!   borg speaks short generic filler lines, paced identically to real lines.
//!   The transition into the model's reply lands exactly on a line boundary, so
//!   the seam between canned and dynamic speech is invisible.
//!
//! Claude has no official Rust SDK, so the Messages API is called over raw
//! HTTPS. Prompt caching is applied to the system prompt and the growing
//! conversation prefix.

use std::sync::Arc;
use std::time::Duration;

use rand::RngExt;
use serde::{Deserialize, Serialize};
use tokio::sync::{mpsc, oneshot};
use tokio::time::{Instant, sleep_until};
use tokio_stream::StreamExt;

use super::{AdaptorControl, AdaptorEvent, AdaptorHandle, InputAdaptor};
use crate::clock::ServerClock;
use crate::config::LlmConfig;
use crate::timing::estimate_line;

/// Longest a single karaoke line may get before it is split further.
const MAX_WORDS_PER_LINE: usize = 12;
/// How long one Claude API call may take before it is abandoned.
const REQUEST_TIMEOUT: Duration = Duration::from_secs(60);
/// `cache_control` marker placed on cacheable prompt blocks.
const CACHE_EPHEMERAL: CacheControl = CacheControl { kind: "ephemeral" };

/// An adaptor that conducts a Claude conversation and speaks the replies.
pub struct LlmAdaptor {
    wpm: u32,
    cfg: LlmConfig,
}

impl LlmAdaptor {
    pub fn new(wpm: u32, cfg: LlmConfig) -> Self {
        LlmAdaptor { wpm, cfg }
    }
}

impl InputAdaptor for LlmAdaptor {
    fn name(&self) -> &'static str {
        "llm-claude"
    }

    fn start(self: Box<Self>, _clock: Arc<ServerClock>) -> AdaptorHandle {
        let (ev_tx, ev_rx) = mpsc::channel(32);
        let (ctl_tx, mut ctl_rx) = mpsc::channel(32);
        let LlmAdaptor { wpm, cfg } = *self;

        tokio::spawn(async move {
            let client = match reqwest::Client::builder().timeout(REQUEST_TIMEOUT).build() {
                Ok(c) => c,
                Err(e) => {
                    tracing::error!(error = %e, "failed to build HTTP client");
                    let _ = ev_tx.send(AdaptorEvent::Closed).await;
                    return;
                }
            };
            tracing::info!(model = %cfg.model, "llm adaptor ready");
            let mut history: Vec<Turn> = Vec::new();

            while let Some(ctl) = ctl_rx.recv().await {
                match ctl {
                    AdaptorControl::SubmitText { text, .. } => {
                        history.push(Turn { role: "user", text });
                        if speak_turn(&ev_tx, &client, &cfg, wpm, &mut history).await.is_err()
                        {
                            return; // the borg actor is gone
                        }
                    }
                    AdaptorControl::Stop => {
                        let _ = ev_tx.send(AdaptorEvent::Closed).await;
                        return;
                    }
                }
            }
        });

        AdaptorHandle { events: ev_rx, control: ctl_tx }
    }
}

/// One turn of conversation history.
#[derive(Clone)]
struct Turn {
    role: &'static str,
    text: String,
}

/// Handle one conversation turn: stream the reply, masking the latency with
/// canned filler, then speak the real reply and record it in `history`.
///
/// Returns `Err(())` only if the borg actor's event channel has closed.
async fn speak_turn(
    ev_tx: &mpsc::Sender<AdaptorEvent>,
    client: &reqwest::Client,
    cfg: &LlmConfig,
    wpm: u32,
    history: &mut Vec<Turn>,
) -> Result<(), ()> {
    // Stream the model reply on a side task: completed karaoke lines arrive on
    // `line_rx`, and the final reply text (or error) on `result_rx`.
    let (line_tx, mut line_rx) = mpsc::channel::<String>(64);
    let (result_tx, result_rx) = oneshot::channel::<Result<String, String>>();
    tokio::spawn(stream_claude(
        client.clone(),
        cfg.clone(),
        history.clone(),
        line_tx,
        result_tx,
    ));

    // Pacing anchor: the wall-clock instant the next line may be emitted.
    let mut next_at: Option<Instant> = None;

    // --- Masking phase: speak filler until the first real line is ready. ---
    let mut filler = shuffled_filler(&cfg.filler);
    let first_real: Option<String> = loop {
        // Wait out the current line, then check whether real content arrived.
        if let Some(t) = next_at.take() {
            sleep_until(t).await;
        }
        match line_rx.try_recv() {
            Ok(line) => break Some(line),
            Err(mpsc::error::TryRecvError::Disconnected) => break None,
            Err(mpsc::error::TryRecvError::Empty) => {}
        }
        let Some(fill) = filler.pop() else {
            // Out of filler — just wait for the model.
            break line_rx.recv().await;
        };
        let spec = estimate_line(&fill, wpm, None);
        let hold = Duration::from_micros(spec.total_duration_us + cfg.line_gap_us);
        ev_tx.send(AdaptorEvent::Line(spec)).await.map_err(|_| ())?;
        next_at = Some(Instant::now() + hold);
    };

    // --- Real phase: speak the model's reply, paced line by line. ---
    let mut spoke_real = false;
    let mut next = first_real;
    while let Some(line) = next {
        if let Some(t) = next_at.take() {
            sleep_until(t).await;
        }
        let spec = estimate_line(&line, wpm, None);
        let hold = Duration::from_micros(spec.total_duration_us + cfg.line_gap_us);
        ev_tx.send(AdaptorEvent::Line(spec)).await.map_err(|_| ())?;
        next_at = Some(Instant::now() + hold);
        spoke_real = true;
        next = line_rx.recv().await;
    }

    // --- Finalize: record history, or speak a fallback on failure. ---
    match result_rx.await {
        Ok(Ok(full)) => history.push(Turn { role: "assistant", text: full }),
        Ok(Err(e)) => {
            tracing::error!(error = %e, "Claude streaming failed");
            history.pop(); // drop the unanswered user turn
            if !spoke_real {
                if let Some(t) = next_at.take() {
                    sleep_until(t).await;
                }
                let spec =
                    estimate_line("the borg cannot reach its mind right now", wpm, None);
                ev_tx.send(AdaptorEvent::Line(spec)).await.map_err(|_| ())?;
            }
        }
        Err(_) => {
            tracing::error!("Claude streaming task ended without a result");
            history.pop();
        }
    }
    Ok(())
}

/// A shuffled copy of the filler pool, so the borg does not always stall the
/// same way.
fn shuffled_filler(filler: &[String]) -> Vec<String> {
    let mut pool = filler.to_vec();
    let mut rng = rand::rng();
    for i in (1..pool.len()).rev() {
        pool.swap(i, rng.random_range(0..=i));
    }
    pool
}

// ---- Claude Messages API wire types ----

#[derive(Serialize)]
struct ClaudeRequest<'a> {
    model: &'a str,
    max_tokens: u32,
    stream: bool,
    system: [SystemBlock<'a>; 1],
    messages: &'a [OutMessage<'a>],
    /// Top-level breakpoint: auto-caches the last (growing) message prefix.
    #[serde(skip_serializing_if = "Option::is_none")]
    cache_control: Option<CacheControl>,
}

#[derive(Serialize)]
struct SystemBlock<'a> {
    #[serde(rename = "type")]
    kind: &'static str,
    text: &'a str,
    #[serde(skip_serializing_if = "Option::is_none")]
    cache_control: Option<CacheControl>,
}

#[derive(Serialize, Clone, Copy)]
struct CacheControl {
    #[serde(rename = "type")]
    kind: &'static str,
}

#[derive(Serialize)]
struct OutMessage<'a> {
    role: &'static str,
    content: &'a str,
}

#[derive(Deserialize)]
struct StreamEvent {
    #[serde(rename = "type", default)]
    kind: String,
    #[serde(default)]
    delta: Option<StreamDelta>,
    #[serde(default)]
    message: Option<StreamMessage>,
}

#[derive(Deserialize)]
struct StreamDelta {
    #[serde(default)]
    text: Option<String>,
}

#[derive(Deserialize)]
struct StreamMessage {
    #[serde(default)]
    usage: Option<Usage>,
}

#[derive(Deserialize, Default)]
struct Usage {
    #[serde(default)]
    input_tokens: u64,
    #[serde(default)]
    cache_creation_input_tokens: u64,
    #[serde(default)]
    cache_read_input_tokens: u64,
}

/// Drive one streaming Claude call: send karaoke lines to `line_tx` as they
/// become available, then report the full reply (or an error) on `result_tx`.
async fn stream_claude(
    client: reqwest::Client,
    cfg: LlmConfig,
    history: Vec<Turn>,
    line_tx: mpsc::Sender<String>,
    result_tx: oneshot::Sender<Result<String, String>>,
) {
    let result = stream_claude_inner(&client, &cfg, &history, &line_tx).await;
    drop(line_tx); // close the line channel before the result is observed
    let _ = result_tx.send(result);
}

async fn stream_claude_inner(
    client: &reqwest::Client,
    cfg: &LlmConfig,
    history: &[Turn],
    line_tx: &mpsc::Sender<String>,
) -> Result<String, String> {
    let api_key = cfg.api_key.as_deref().ok_or("ANTHROPIC_API_KEY not set")?;

    let messages: Vec<OutMessage> = history
        .iter()
        .map(|t| OutMessage { role: t.role, content: &t.text })
        .collect();
    let request = ClaudeRequest {
        model: &cfg.model,
        max_tokens: cfg.max_tokens,
        stream: true,
        system: [SystemBlock {
            kind: "text",
            text: &cfg.system_prompt,
            cache_control: Some(CACHE_EPHEMERAL),
        }],
        messages: &messages,
        cache_control: Some(CACHE_EPHEMERAL),
    };

    let url = format!("{}/v1/messages", cfg.base_url.trim_end_matches('/'));
    let resp = client
        .post(url)
        .header("x-api-key", api_key)
        .header("anthropic-version", "2023-06-01")
        .json(&request)
        .send()
        .await
        .map_err(|e| format!("request error: {e}"))?;

    let status = resp.status();
    if !status.is_success() {
        let body = resp.bytes().await.unwrap_or_default();
        return Err(format!("HTTP {status}: {}", String::from_utf8_lossy(&body)));
    }

    // Parse the Server-Sent Events stream frame by frame.
    let mut stream = std::pin::pin!(resp.bytes_stream());
    let mut raw: Vec<u8> = Vec::new();
    let mut pending = String::new(); // reply text not yet split into a line
    let mut full = String::new(); // the entire reply, for history

    while let Some(chunk) = stream.next().await {
        let chunk = chunk.map_err(|e| format!("stream error: {e}"))?;
        raw.extend_from_slice(&chunk);

        while let Some(end) = find_frame_end(&raw) {
            let frame: Vec<u8> = raw.drain(..end + 2).collect();
            let Some(ev) = parse_event(&frame[..end]) else { continue };
            match ev.kind.as_str() {
                "message_start" => {
                    if let Some(u) = ev.message.and_then(|m| m.usage) {
                        tracing::info!(
                            input_tokens = u.input_tokens,
                            cache_read = u.cache_read_input_tokens,
                            cache_write = u.cache_creation_input_tokens,
                            "claude usage"
                        );
                    }
                }
                "content_block_delta" => {
                    if let Some(t) = ev.delta.and_then(|d| d.text) {
                        pending.push_str(&t);
                        full.push_str(&t);
                        for sentence in drain_complete_sentences(&mut pending) {
                            for line in chunk_sentence(&sentence) {
                                if line_tx.send(line).await.is_err() {
                                    return Ok(full); // consumer gone — stop
                                }
                            }
                        }
                    }
                }
                _ => {}
            }
        }
    }

    // Flush whatever sentence fragment is left once the stream ends.
    for line in chunk_sentence(pending.trim()) {
        let _ = line_tx.send(line).await;
    }
    if full.trim().is_empty() {
        return Err("empty response from Claude".to_string());
    }
    Ok(full)
}

/// Index of the `\n\n` that terminates the first complete SSE frame, if any.
fn find_frame_end(buf: &[u8]) -> Option<usize> {
    buf.windows(2).position(|w| w == b"\n\n")
}

/// Parse one SSE frame's `data:` line into a [`StreamEvent`].
fn parse_event(frame: &[u8]) -> Option<StreamEvent> {
    for line in frame.split(|&b| b == b'\n') {
        if let Some(data) = line.strip_prefix(b"data:") {
            if let Ok(ev) = serde_json::from_slice::<StreamEvent>(data.trim_ascii_start()) {
                return Some(ev);
            }
        }
    }
    None
}

/// Pull every complete sentence out of `buf`, leaving the trailing fragment.
/// A sentence ends at `.`, `!`, or `?` followed by whitespace.
fn drain_complete_sentences(buf: &mut String) -> Vec<String> {
    let mut sentences = Vec::new();
    loop {
        let bytes = buf.as_bytes();
        let boundary = bytes.iter().enumerate().find_map(|(i, &b)| {
            let ends = matches!(b, b'.' | b'!' | b'?')
                && bytes.get(i + 1).is_some_and(u8::is_ascii_whitespace);
            ends.then_some(i + 1)
        });
        let Some(end) = boundary else { break };
        let sentence = buf[..end].trim().to_string();
        buf.drain(..end);
        let ws = buf.len() - buf.trim_start().len();
        buf.drain(..ws);
        if !sentence.is_empty() {
            sentences.push(sentence);
        }
    }
    sentences
}

/// Split one sentence into karaoke lines of at most [`MAX_WORDS_PER_LINE`].
fn chunk_sentence(sentence: &str) -> Vec<String> {
    let words: Vec<&str> = sentence.split_whitespace().collect();
    words
        .chunks(MAX_WORDS_PER_LINE)
        .map(|chunk| chunk.join(" "))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn drains_complete_sentences_leaving_the_fragment() {
        let mut buf = String::from("Hello there. How are");
        assert_eq!(drain_complete_sentences(&mut buf), ["Hello there."]);
        assert_eq!(buf, "How are");
        buf.push_str(" you? Good");
        assert_eq!(drain_complete_sentences(&mut buf), ["How are you?"]);
        assert_eq!(buf, "Good");
    }

    #[test]
    fn decimal_point_is_not_a_sentence_boundary() {
        let mut buf = String::from("Pi is 3.14 here. ");
        assert_eq!(drain_complete_sentences(&mut buf), ["Pi is 3.14 here."]);
        assert!(buf.is_empty());
    }

    #[test]
    fn handles_all_three_terminators() {
        let mut buf = String::from("Wait! Really? Yes. ");
        assert_eq!(drain_complete_sentences(&mut buf), ["Wait!", "Really?", "Yes."]);
    }

    #[test]
    fn long_sentence_is_chunked_by_word_count() {
        let long = "one two three four five six seven eight nine ten eleven twelve thirteen";
        let lines = chunk_sentence(long);
        assert_eq!(lines.len(), 2);
        assert_eq!(lines[0].split_whitespace().count(), MAX_WORDS_PER_LINE);
        assert_eq!(lines[1], "thirteen");
    }

    #[test]
    fn chunking_empty_text_yields_nothing() {
        assert!(chunk_sentence("   ").is_empty());
    }

    #[test]
    fn finds_sse_frame_boundary() {
        assert_eq!(find_frame_end(b"event: x\ndata: {}\n\nrest"), Some(17));
        assert_eq!(find_frame_end(b"incomplete frame"), None);
    }

    #[test]
    fn parses_a_text_delta_frame() {
        let frame = b"event: content_block_delta\n\
                      data: {\"type\":\"content_block_delta\",\"delta\":{\"type\":\"text_delta\",\"text\":\"hi\"}}";
        let ev = parse_event(frame).expect("event parses");
        assert_eq!(ev.kind, "content_block_delta");
        assert_eq!(ev.delta.and_then(|d| d.text).as_deref(), Some("hi"));
    }
}
