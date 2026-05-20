# borg

A super-low-latency **synchronized karaoke / teleprompter** system.

A central Rust service broadcasts a line of text to a group of connected web
clients — a **borg** — which display it karaoke-style with a word-by-word
highlight sweep. Every client runs the sweep in lockstep: not by racing packets,
but by agreeing on *when* to display.

## How it works

- A client visits the page and either **creates** a borg (becoming the **borg
  master**, who receives a secret master code) or **joins** one with a short
  join code.
- The master sends text. It is turned into a word-timed line by an **input
  adaptor** and fanned out to every client.
- **Synchronization** is solved at the protocol layer, not the transport:
  1. Each client estimates its offset from the server clock (Cristian/NTP
     four-timestamp sync over `GET /api/time`, keeping the minimum-RTT sample).
  2. The server stamps each line with an absolute `display_at` timestamp
     slightly in the future.
  3. Every client schedules its word sweep for that instant off its own synced
     clock — so all highlight bars move together.
- The lead time is **adaptive and capped**: it tracks the slowest client's
  reported latency, so one bad connection lags slightly instead of delaying the
  whole room.

Transport is **Server-Sent Events** for the receive path (matches the
server→many broadcast shape, free `EventSource` auto-reconnect) plus plain HTTP
for create/join, send, and clock-sync. The broadcast hot path serializes each
line exactly once and shares it as `Bytes` — no per-client work.

## Run

```sh
cargo run            # listens on 127.0.0.1:8080
```

Open <http://127.0.0.1:8080>, create a borg, and share the join code (or the
`/?b=CODE` link). Append `?debug=1` to the URL for a sync-quality overlay.

## Configuration (environment variables)

| Variable | Default | Meaning |
|---|---|---|
| `BORG_BIND` | `127.0.0.1:8080` | bind address |
| `BORG_WPM` | `200` | default reading rate for the manual adaptor |
| `BORG_LEAD_MIN_US` | `80000` | minimum display lead time |
| `BORG_LEAD_CAP_US` | `400000` | maximum display lead time |
| `BORG_JITTER_US` | `25000` | margin added to the slowest client's latency |
| `BORG_DEFAULT_ONE_WAY_US` | `60000` | latency assumed before a client reports |
| `ANTHROPIC_API_KEY` | _(unset)_ | enables the `llm` adaptor; required to create LLM borgs |
| `BORG_LLM_MODEL` | `claude-haiku-4-5` | Claude model for the `llm` adaptor |
| `BORG_LLM_SYSTEM` | _(built-in)_ | system prompt / persona for the `llm` adaptor |
| `BORG_LLM_MAX_TOKENS` | `1024` | cap on an LLM reply's length |
| `BORG_LLM_FILLER` | _(built-in set)_ | pipe-separated filler lines; empty string disables filler |
| `BORG_LINE_GAP_US` | `400000` | pause between spoken lines of one LLM reply |
| `ANTHROPIC_BASE_URL` | `https://api.anthropic.com` | Anthropic API base URL |

## HTTP API

| Method | Path | Purpose |
|---|---|---|
| `POST` | `/api/borg` | create a borg (optional `?wpm=`, `?adaptor=manual\|llm`) |
| `POST` | `/api/borg/{join}/join` | join an existing borg |
| `GET` | `/api/borg/{join}/stream?client_id=` | SSE receive stream |
| `POST` | `/api/borg/{join}/line` | master sends a line |
| `GET` | `/api/time?t0=` | clock-sync round trip |
| `POST` | `/api/borg/{join}/rtt` | client reports measured latency |

## Adaptors

An input adaptor is a pluggable source of lines (`InputAdaptor` in
`src/adaptor/`). Two ship today; choose one when creating a borg via
`POST /api/borg?adaptor=<name>` (the web UI has a dropdown):

- **`manual`** (default) — the master types lines directly; per-word sweep
  timing is estimated from a words-per-minute rate.
- **`llm`** — the borg becomes the spoken voice of a Claude conversation. Each
  text submitted is treated as the counterparty's turn; the adaptor keeps the
  conversation history, calls the Claude Messages API, and speaks the reply as a
  paced sequence of karaoke lines. The model's latency is masked two ways: the
  reply is **streamed** (spoken sentence-by-sentence as it generates), and
  **canned filler lines** cover the gap until the first real sentence is ready —
  the borg starts talking immediately and blends into the model's words on a
  line boundary. Requires `ANTHROPIC_API_KEY`; the model is `BORG_LLM_MODEL`
  (default `claude-haiku-4-5` — cheapest and fastest).

The trait is channel-based so future push (live transcription) and
timed-playback adaptors drop in without touching the borg actor.

## Tests

```sh
cargo test           # unit tests: code generation, word-timing estimator
```

## Project layout

```
src/
  main.rs        startup, router, graceful shutdown
  config.rs      environment-driven configuration
  clock.rs       monotonic server clock
  codes.rs       join / master / client code generation
  protocol.rs    all wire types (single source of truth)
  timing.rs      WPM word-timing estimator
  adaptor/       InputAdaptor trait + manual & llm (Claude) adaptors
  borg.rs        per-borg actor: fan-out + adaptive lead time
  registry.rs    code -> actor lookup
  routes/        HTTP handlers (lifecycle, stream, send, sync)
  static_files.rs  serves the embedded client
assets/index.html  the entire web client (no build step)
```
