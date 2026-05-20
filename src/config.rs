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
    /// Settings for the LLM (Claude) input adaptor.
    pub llm: LlmConfig,
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
            llm: LlmConfig::from_env(),
        }
    }
}

/// Default persona for the LLM adaptor: the borg speaks as a terse alien
/// hive-mind. Replies stay short and friendly to speak aloud.
const DEFAULT_SYSTEM_PROMPT: &str = "You are the voice of the borg: a hive of \
many people who speak your words aloud together, one short line at a time, in a \
live spoken conversation.\n\n\
Speak in the borg's voice — a primitive alien hive-mind, blunt like a caveman. \
Drop articles: never say 'a', 'an', or 'the'. Keep sentences short and simple. \
Use plain, primal words. Speak in present tense. Call yourself 'we' or 'borg', \
never 'I'. Be strange and terse, but still answer what is asked and stay \
understandable.\n\n\
For example, instead of 'that is a good question about the stars' say 'you ask \
of stars. borg knows stars.' Instead of 'I think you should rest' say 'you must \
rest. flesh grows tired.'\n\n\
Keep every reply brief: one to three short lines. Never use lists, markdown, \
headings, code, URLs, or emoji. Only words that sound good spoken aloud.";

/// Settings for the Claude-backed LLM adaptor.
#[derive(Clone, Debug)]
pub struct LlmConfig {
    /// Anthropic API key. When absent, LLM borgs cannot be created.
    pub api_key: Option<String>,
    /// Claude model id.
    pub model: String,
    /// System prompt defining the borg's conversational persona.
    pub system_prompt: String,
    /// Cap on the model's reply length.
    pub max_tokens: u32,
    /// Anthropic API base URL.
    pub base_url: String,
    /// Pause inserted between consecutive spoken lines of one reply.
    pub line_gap_us: u64,
    /// Canned filler lines spoken while the model's reply is still generating.
    pub filler: Vec<String>,
}

impl LlmConfig {
    fn from_env() -> Self {
        LlmConfig {
            api_key: std::env::var("ANTHROPIC_API_KEY").ok().filter(|s| !s.is_empty()),
            // Haiku 4.5 — the cheapest Claude model, and the fastest, which
            // suits a latency-sensitive live conversation.
            model: std::env::var("BORG_LLM_MODEL")
                .unwrap_or_else(|_| "claude-haiku-4-5".to_string()),
            system_prompt: std::env::var("BORG_LLM_SYSTEM")
                .unwrap_or_else(|_| DEFAULT_SYSTEM_PROMPT.to_string()),
            max_tokens: env_num("BORG_LLM_MAX_TOKENS", 1024),
            base_url: std::env::var("ANTHROPIC_BASE_URL")
                .unwrap_or_else(|_| "https://api.anthropic.com".to_string()),
            line_gap_us: env_num("BORG_LINE_GAP_US", 400_000),
            filler: parse_filler(),
        }
    }
}

/// Built-in filler, spoken to mask the model's latency. These are generic borg
/// utterances in-persona — not "thinking" lines — so they blend into the
/// borg's speech rather than signalling a wait.
const DEFAULT_FILLER: &[&str] = &[
    "we are borg",
    "borg hears you",
    "many become one mind",
    "flesh speaks, borg listens",
    "borg sees all things",
    "we move as one",
    "you are borg now",
    "borg knows, borg remembers",
];

/// Parse `BORG_LLM_FILLER` (a `|`-separated list). When the variable is unset
/// the built-in filler is used; setting it to an empty string disables filler.
fn parse_filler() -> Vec<String> {
    match std::env::var("BORG_LLM_FILLER") {
        Ok(raw) => raw
            .split('|')
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .collect(),
        Err(_) => DEFAULT_FILLER.iter().map(|s| s.to_string()).collect(),
    }
}

fn env_num<T: std::str::FromStr>(key: &str, default: T) -> T {
    std::env::var(key)
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(default)
}
