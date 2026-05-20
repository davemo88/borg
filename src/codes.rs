//! Generation of borg join codes, master codes, and client ids.

use rand::{Rng, RngExt};

/// Ambiguity-free alphabet for human-read join codes (no `0 1 I O L`).
const JOIN_ALPHABET: &[u8] = b"23456789ABCDEFGHJKMNPQRSTUVWXYZ";
const JOIN_LEN: usize = 6;

const HEX: &[u8] = b"0123456789abcdef";
const MASTER_HEX_LEN: usize = 16;
const CLIENT_HEX_LEN: usize = 12;

fn pick(rng: &mut impl Rng, alphabet: &[u8]) -> char {
    alphabet[rng.random_range(0..alphabet.len())] as char
}

/// A short, human-friendly borg join code, e.g. `"K7QM9X"`.
pub fn join_code() -> String {
    let mut rng = rand::rng();
    (0..JOIN_LEN).map(|_| pick(&mut rng, JOIN_ALPHABET)).collect()
}

/// A secret master code, e.g. `"M-7f3a91b2c8d4e6a0"` (64 bits of entropy).
pub fn master_code() -> String {
    let mut rng = rand::rng();
    let mut s = String::with_capacity(2 + MASTER_HEX_LEN);
    s.push_str("M-");
    for _ in 0..MASTER_HEX_LEN {
        s.push(pick(&mut rng, HEX));
    }
    s
}

/// An opaque per-connection client id, e.g. `"c-3e9a17b4c2d0"`.
pub fn client_id() -> String {
    let mut rng = rand::rng();
    let mut s = String::with_capacity(2 + CLIENT_HEX_LEN);
    s.push_str("c-");
    for _ in 0..CLIENT_HEX_LEN {
        s.push(pick(&mut rng, HEX));
    }
    s
}

/// Normalize a user-entered join code: drop separators/whitespace, uppercase.
pub fn normalize_join_code(input: &str) -> String {
    input
        .chars()
        .filter(|c| c.is_ascii_alphanumeric())
        .map(|c| c.to_ascii_uppercase())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    #[test]
    fn join_code_length_and_alphabet() {
        for _ in 0..2000 {
            let c = join_code();
            assert_eq!(c.chars().count(), JOIN_LEN);
            for ch in c.chars() {
                assert!(JOIN_ALPHABET.contains(&(ch as u8)), "unexpected char {ch}");
                assert!(!"01ILO".contains(ch), "ambiguous char {ch} leaked in");
            }
        }
    }

    #[test]
    fn master_code_format() {
        for _ in 0..2000 {
            let c = master_code();
            assert!(c.starts_with("M-"));
            assert_eq!(c.len(), 2 + MASTER_HEX_LEN);
            assert!(c[2..].chars().all(|ch| ch.is_ascii_hexdigit()));
        }
    }

    #[test]
    fn join_codes_are_varied() {
        let set: HashSet<_> = (0..200).map(|_| join_code()).collect();
        assert!(set.len() > 190, "join codes not random enough: {}", set.len());
    }

    #[test]
    fn normalize_handles_separators_and_case() {
        assert_eq!(normalize_join_code(" k7q-m9x "), "K7QM9X");
        assert_eq!(normalize_join_code("K7QM9X"), "K7QM9X");
        assert_eq!(normalize_join_code("k7q m9x"), "K7QM9X");
    }
}
