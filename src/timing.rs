//! Estimating word-by-word sweep timing for a plain line of text.

use crate::protocol::{LineSpec, WordTiming};

/// Minimum weight units for a word, so tiny words ("a", "is") still sweep.
const FLOOR: u64 = 2;

/// Turn a plain line of text into a word-timed [`LineSpec`].
///
/// `wpm` sets the reading rate when `override_us` is `None`. Each word is
/// weighted by its character count (floored), and the total duration is
/// distributed proportionally. The last word absorbs any rounding remainder,
/// so `words.last().end_us == total_duration_us` exactly.
pub fn estimate_line(text: &str, wpm: u32, override_us: Option<u64>) -> LineSpec {
    let raw: Vec<&str> = text.split_whitespace().collect();
    if raw.is_empty() {
        return LineSpec { words: Vec::new(), total_duration_us: 0 };
    }

    let total_us: u64 = override_us.unwrap_or_else(|| {
        let micros_per_word = 60_000_000u64 / wpm.max(1) as u64;
        micros_per_word * raw.len() as u64
    });

    let weights: Vec<u64> = raw
        .iter()
        .map(|w| (w.chars().count() as u64).max(FLOOR))
        .collect();
    let weight_sum: u64 = weights.iter().sum();

    let mut words = Vec::with_capacity(raw.len());
    let mut cursor: u64 = 0;
    for (i, w) in raw.iter().enumerate() {
        let dur = if i + 1 == raw.len() {
            total_us.saturating_sub(cursor)
        } else {
            total_us * weights[i] / weight_sum
        };
        words.push(WordTiming {
            text: (*w).to_string(),
            start_us: cursor,
            end_us: cursor + dur,
        });
        cursor += dur;
    }

    LineSpec { words, total_duration_us: total_us }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_text_yields_no_words() {
        let s = estimate_line("   \t  ", 200, None);
        assert!(s.words.is_empty());
        assert_eq!(s.total_duration_us, 0);
    }

    #[test]
    fn word_count_matches_input() {
        let s = estimate_line("we are the borg", 200, None);
        assert_eq!(s.words.len(), 4);
        assert_eq!(s.words[0].text, "we");
        assert_eq!(s.words[3].text, "borg");
    }

    #[test]
    fn last_word_ends_exactly_at_total() {
        for text in ["resistance is utterly futile here", "a", "one two"] {
            let s = estimate_line(text, 173, None);
            assert_eq!(s.words.last().unwrap().end_us, s.total_duration_us);
        }
    }

    #[test]
    fn duration_override_sets_total() {
        let s = estimate_line("one two three", 200, Some(3_000_000));
        assert_eq!(s.total_duration_us, 3_000_000);
        assert_eq!(s.words.last().unwrap().end_us, 3_000_000);
    }

    #[test]
    fn wpm_math_is_correct() {
        // 4 words at 240 wpm => 250_000 us/word => 1_000_000 us total.
        let s = estimate_line("a b c d", 240, None);
        assert_eq!(s.total_duration_us, 1_000_000);
    }

    #[test]
    fn words_are_contiguous_and_ordered() {
        let s = estimate_line("the quick brown fox jumps over", 200, None);
        let mut prev = 0;
        for w in &s.words {
            assert_eq!(w.start_us, prev);
            assert!(w.end_us >= w.start_us);
            prev = w.end_us;
        }
    }

    #[test]
    fn longer_words_get_more_time() {
        let s = estimate_line("hi extraordinarily", 200, Some(1_000_000));
        let short = s.words[0].end_us - s.words[0].start_us;
        let long = s.words[1].end_us - s.words[1].start_us;
        assert!(long > short, "long word should sweep slower: {long} vs {short}");
    }
}
