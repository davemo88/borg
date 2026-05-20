# borg — Design

This document explains *why* borg is built the way it is. For how to run it and
the API reference, see [README.md](README.md).

## 1. Problem

Broadcast a line of text to a group of web clients so they read it **together**,
karaoke-style, with a word-by-word highlight sweep — and do it with the lowest
latency possible, on both the server and the clients.

A group of clients is a **borg**. One client creates the borg and becomes the
**borg master**; the rest join with a short code. The master sends text; it
fans out to everyone. Sending text is just one **input adaptor** — the design
must leave room for others (scripted playback, live transcription) later.

## 2. The core insight: two different latencies

"Low latency" hides two distinct goals that are actually in tension:

1. **Glass-to-glass latency** — master hits send → text appears.
2. **Synchronization** — every client flips/sweeps the line at the *same
   wall-clock instant*.

If each client simply rendered a line the moment its packet arrived, you would
minimize (1) but destroy (2): clients sit on different network paths, so their
packets land tens of milliseconds apart, and the highlight bars visibly drift.

**Synchronization is not a transport problem. It is a protocol problem.** The
fix is not "make packets faster" but "agree on *when* to display":

- Each client estimates the offset between its clock and the server's.
- The server stamps every line with an absolute `display_at` timestamp a little
  in the future.
- Each client schedules its render for that instant, off its own synced clock.

Network jitter is absorbed entirely, as long as the line *arrives* before
`display_at`. The price is a small, deliberate lead time — and minimizing that
lead time is what "low latency" concretely means for this system. So borg
optimizes (2) directly and treats (1) as "make the necessary lead time as small
and as adaptive as possible."

## 3. Transport: Server-Sent Events

The data flow is fundamentally **one server → many clients, broadcast**. SSE
matches that shape exactly, so it is the receive path. Everything else
(create/join, send, clock-sync) is plain HTTP `GET`/`POST`.

Why SSE over the alternatives:

| Option | Verdict |
|---|---|
| **SSE** | Chosen. Held-open HTTP stream; pushing an event is a raw socket write — identical broadcast latency to a WebSocket frame. Free `EventSource` auto-reconnect (matters for phones on venue Wi-Fi). Universal, including Safari/iPhone. No framing/upgrade/close machinery. |
| WebSocket | A bidirectional channel buys little here — clients only *receive*. Clock-sync as plain HTTP requests works fine (and multiplexes over one HTTP/2 connection). Not worth the extra protocol surface. |
| WebTransport (HTTP/3 / QUIC / UDP) | True UDP, but no Safari support — iPhones could not join. |
| WebRTC data channel | UDP and universal, but needs signaling/ICE; massive overkill for server→client broadcast. |

The "UDP for low latency" instinct is misdirected here: UDP's win is avoiding
head-of-line blocking under packet loss, but the scheduled-display protocol
(§4) already absorbs jitter, and the payload is one tiny line — TCP HOL almost
never bites. The synchronization is bought in the protocol, not the transport.

## 4. Synchronization

### 4.1 One clock domain

The server clock (`clock.rs`) is **microseconds since process start**, taken
from a monotonic `Instant`. It never goes backward and is immune to wall-clock
or NTP adjustments. Every `*_us` field on the wire lives in this domain.

### 4.2 Clock-sync (Cristian / NTP four-timestamp)

Per round trip to `GET /api/time`:

- `t0` — client clock when the request is sent
- `server_recv_us` (t1), `server_send_us` (t2) — server clock, in the handler
- `t3` — client clock when the response arrives

```
rtt    = (t3 - t0) - (t2 - t1)
offset = ((t1 - t0) + (t2 - t3)) / 2     // server_clock - client_clock
```

The client fires a burst of 5 samples 150 ms apart at startup, then one every
5 s, and **keeps the sample with the minimum RTT** — the least
queueing-contaminated round trip yields the most accurate offset.
`serverNow() = performance.now()·1000 + offset`.

### 4.3 Scheduled display

The server stamps each line with `display_at_us = now + lead_time`. The client
runs a `requestAnimationFrame` loop; each frame it computes
`elapsed = serverNow() - display_at_us` and drives the word sweep from that.
Because every client computes from the *same* `display_at_us` against its own
synced clock, all sweeps advance in lockstep — to within the clock-sync error
(a few ms on a LAN). A line that arrives already past `display_at` (a client
slower than the cap) renders immediately and fast-forwards to the right
position.

### 4.4 Adaptive, capped lead time

The lead time is recomputed on every line (`borg.rs::lead_time_us`):

```
slowest      = max(one_way_us over all clients)   // default 60 ms before any report
lead_time_us = clamp(slowest + JITTER_MARGIN, LEAD_MIN, LEAD_CAP)
```

Defaults: `LEAD_MIN = 80 ms`, `LEAD_CAP = 400 ms`, `JITTER_MARGIN = 25 ms`.
Clients report their measured one-way latency via `POST /api/borg/{join}/rtt`.

The **cap** is the key policy decision: a single client slower than the cap is
*not* allowed to delay the room — its line simply lands a little late and its
own renderer fast-forwards. This trades perfect sync for that one straggler
against bounded latency for everyone else. The **adaptivity** means a borg of
fast local clients gets a tight ~85 ms lead, while a borg spanning worse
networks widens automatically.

## 5. Server architecture

### 5.1 An actor per borg

Each borg is one tokio task — an **actor** — that owns all of that room's
mutable state (client list, latencies, sequence counter, broadcast channel) and
is fed by an mpsc command channel. A global `Registry` (`Mutex<HashMap>` of
handles) maps join/master codes to actors.

Chosen over a shared/sharded map because the hot path is fan-out *within* one
room, and rooms are independent — the textbook actor case:

- The broadcast path takes **zero locks**: one owner mutates one channel.
- Per-client state (latency → lead time) is recomputed in-task, no
  synchronization.
- The actor is a single `select!` site that merges client commands, adaptor
  events, and shutdown uniformly.
- Borg lifetime = task lifetime; when the task ends, dropping the broadcast
  sender cleanly closes every subscriber.

The `Registry` mutex is touched only on create/join/connect/send — never on the
broadcast path — so a plain `std::sync::Mutex` with tiny critical sections is
correct.

### 5.2 Serialize once, fan out shared bytes

When a line is broadcast (`borg.rs::broadcast_line`), the actor:

1. anchors it — `display_at = now + lead_time`,
2. serializes the whole SSE frame (`event: line\ndata: {json}\n\n`) **once**
   into `bytes::Bytes`,
3. sends that `Bytes` over a `tokio::broadcast` channel.

Each subscriber receives the *same* `Bytes` — a refcount bump, no copy. Each SSE
connection's stream forwards pre-formatted frames straight to the socket: **no
per-client serialization, allocation, or locking**. The SSE response body is a
raw `Stream<Bytes>`, bypassing axum's per-event `Event` wrapper entirely.

### 5.3 Connection lifecycle

A subscriber connecting issues a `Subscribe` command; the actor hands back a
broadcast receiver plus the current lead time (sent in the `hello` event). When
the SSE stream drops — client closed the tab, or shutdown — a `ClientGuard`'s
`Drop` fires a `ClientGone` command so the actor stops counting that client's
latency in the lead-time calculation. Graceful shutdown flips a `watch` channel
that every SSE stream selects on; each emits a final `borg_closed` event and
ends, so the server exits cleanly instead of hanging on long-lived streams.

## 6. Input adaptors

An adaptor is a **source of lines over time**. The `InputAdaptor` trait
(`adaptor/mod.rs`) is channel-based: `start()` spawns the adaptor's own task and
returns an `AdaptorHandle` carrying an event receiver (lines out) and a control
sender (text/stop in). The borg actor `select!`s on adaptor events exactly like
client commands.

The borg is adaptor-agnostic: `Borg::spawn` takes a `Box<dyn InputAdaptor>` and
the create route picks the concrete type from `?adaptor=`. Two ship today:

- **`manual`** — the borg master feeds text directly (`ManualTextAdaptor`); the
  text *is* the line.
- **`llm`** — the borg speaks a Claude conversation (`LlmAdaptor`). Each
  submitted text is the counterparty's turn; the adaptor keeps the conversation
  history and calls the Claude Messages API over raw HTTPS (Claude has no
  official Rust SDK). Two techniques mask the model's latency:
  - **Streaming** — the call uses `stream: true`; the reply's SSE stream is
    parsed sentence-by-sentence as tokens arrive, so the borg can start speaking
    the first sentence long before the model finishes.
  - **Canned filler** — while the first real sentence is still in flight, the
    borg speaks short generic filler lines (`BORG_LLM_FILLER`), paced
    identically to real lines. The transition into the model's reply lands
    exactly on a line boundary, hiding the seam between canned and dynamic
    speech.

  Lines stream from a side task into a channel; a single pacing anchor
  (`sleep_until` the instant the previous line's sweep ends) drains it, so the
  cadence is uniform whether the next line is filler, freshly streamed, or
  already buffered. Prompt caching covers the system prompt and the growing
  conversation prefix. Requires `ANTHROPIC_API_KEY`.

The same channel-based trait still fits future **push** (live transcription) and
**timed** (scripted playback) adaptors without touching the borg actor.

Adaptors emit a `LineSpec`: words plus **relative** per-word sweep timing. They
never see `display_at` — absolute anchoring is solely the server's job, keeping
the single clock domain intact and adaptors trivially testable.

### Word timing from plain text

The manual adaptor has no real timing, so it estimates (`timing.rs`): split on
whitespace, weight each word by `max(char_count, 2)` (the floor keeps "a"/"is"
visible), and distribute a total duration — either the master's explicit
override or `word_count · 60 s / WPM`. The last word absorbs the rounding
remainder, so the sweep ends exactly at `total_duration_us`.

## 7. The client

One static `index.html` — vanilla JS + CSS, no build step, embedded in the
binary via `rust-embed`. It has three screens (landing / master / viewer)
toggled by a `<body>` attribute.

The word sweep is pure CSS + one `--p` custom property per word: each word
`<span>` carries a `::before` pseudo-element — a copy of the text in the
highlight color, clipped to `width: calc(var(--p) * 100%)`. The rAF loop sets
`--p` ∈ [0,1] per word per frame, producing a continuous wipe rather than a
stepwise jump. Because the loop is driven by `serverNow()`, the wipe is the
synchronized element — the karaoke "bouncing ball" that all clients share.

`?debug=1` shows a sync-quality overlay; it logs each line's scheduling skew
(`serverNow() - display_at`), so diffing that value between two clients for the
same `seq` measures real inter-client skew.

## 8. Tradeoffs and limitations

- **In-memory, single process.** Borgs vanish on restart; no horizontal
  scaling. A second server instance would not share borgs. Fine for v1; a
  distributed version would need a shared registry and a cross-node fan-out
  bus.
- **No authentication beyond codes.** The join code gates viewing; the master
  code gates sending. Codes are random (join: 31^6 ≈ 9×10⁸; master: 64-bit).
  There is no rate limiting or abuse protection.
- **Clock-sync accuracy bounds sync quality.** Inter-client skew is roughly the
  clock-sync error — a few ms on a LAN, more on mobile. Good enough for reading
  together; not sample-accurate like audio sync.
- **A backgrounded tab pauses `requestAnimationFrame`.** A hidden viewer's sweep
  freezes and fast-forwards on return — acceptable for a foregrounded karaoke
  display.
- **Lagged subscribers skip lines.** If a client falls 64 frames behind the
  broadcast channel it skips stale lines rather than stalling the room.
- **The `llm` adaptor depends on an external API.** Streaming and canned filler
  mask the model's latency — `POST /api/borg/{join}/line` returns as soon as the
  first (filler) line broadcasts. But there is no mid-reply interruption (a new
  turn queues behind the current one), a model error after the filler resolves
  to a short fallback line, and a model slower than speech can still open a gap
  mid-reply once the filler is exhausted.

## 9. Possible future work

- More adaptors: file/script playback, live speech-to-text, MIDI/lyrics timing.
- Per-borg WPM/tempo and theming controls.
- Borg persistence and reconnection of a borg master.
- A measurement harness (headless browsers) reporting inter-client skew.
- Optional WebTransport path once Safari support lands, for jittery networks.
