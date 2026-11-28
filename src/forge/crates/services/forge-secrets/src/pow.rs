//! Proof of work for the login challenge (DECISIONS.md D-064): find a nonce
//! so that SHA-256(salt || nonce) starts with `bits` zero bits. The server
//! checks one hash; a client tries about 2^bits nonces. The console solves it
//! in a Web Worker with the same rules.

use crate::crypto::sha256;

/// Hardest challenge the server hands out.
pub const MAX_BITS: u8 = 26;
/// Longest nonce accepted (decimal counters fit easily).
pub const MAX_NONCE_LEN: usize = 32;

/// Number of leading zero bits of `digest`.
#[must_use]
pub fn leading_zero_bits(digest: &[u8]) -> u32 {
    let mut n = 0;

    for b in digest {
        if *b == 0 {
            n += 8;
        } else {
            return n + b.leading_zeros();
        }
    }

    n
}

/// Whether `nonce` solves the challenge `(salt, bits)`. The nonce is ASCII
/// digits or letters, at most [`MAX_NONCE_LEN`] long.
#[must_use]
pub fn check(salt: &[u8], nonce: &str, bits: u8) -> bool {
    if nonce.is_empty() || nonce.len() > MAX_NONCE_LEN || !nonce.bytes().all(|b| b.is_ascii_alphanumeric()) {
        return false;
    }

    let mut input = Vec::with_capacity(salt.len() + nonce.len());
    input.extend_from_slice(salt);
    input.extend_from_slice(nonce.as_bytes());

    leading_zero_bits(&sha256(&input)) >= u32::from(bits)
}

/// Finds a nonce for `(salt, bits)` (clients, tools and tests).
#[must_use]
pub fn solve(salt: &[u8], bits: u8) -> String {
    let mut n: u64 = 0;

    loop {
        let s = n.to_string();
        if check(salt, &s, bits) {
            return s;
        }

        n += 1;
    }
}

/// Hex decoding of a challenge salt.
#[must_use]
pub fn salt_from_hex(hex: &str) -> Option<Vec<u8>> {
    if !hex.len().is_multiple_of(2) || hex.len() > 128 {
        return None;
    }

    (0..hex.len()).step_by(2).map(|i| u8::from_str_radix(hex.get(i..i + 2)?, 16).ok()).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn solves_and_checks() {
        assert_eq!(leading_zero_bits(&[0, 0, 0x10, 0xff]), 19);
        assert_eq!(leading_zero_bits(&[0x80]), 0);
        let salt = salt_from_hex("00112233445566778899aabbccddeeff").unwrap_or_default();
        let n = solve(&salt, 12);
        assert!(check(&salt, &n, 12));
        assert!(!check(&salt, "", 0), "empty nonce");
        assert!(!check(&salt, "1 2", 0), "only letters and digits");
        assert!(!check(&salt, &"1".repeat(MAX_NONCE_LEN + 1), 0), "too long");
        assert!(salt_from_hex("abc").is_none() && salt_from_hex("zz").is_none());
    }
}
