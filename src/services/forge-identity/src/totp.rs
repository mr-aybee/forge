//! RFC 6238 TOTP (HMAC-SHA1, 30 s step, 6 digits), compatible with standard
//! authenticator apps.

use ring::hmac;

pub const STEP_SECS: i64 = 30;
pub const DIGITS: u32 = 6;

/// A new 160-bit secret.
#[must_use]
pub fn generate_secret() -> Vec<u8> {
    forge_secrets::crypto::random_bytes::<20>().to_vec()
}

/// The code for time step `counter`.
#[must_use]
pub fn code_for_counter(secret: &[u8], counter: u64) -> u32 {
    let key = hmac::Key::new(hmac::HMAC_SHA1_FOR_LEGACY_USE_ONLY, secret);
    let tag = hmac::sign(&key, &counter.to_be_bytes());
    let d = tag.as_ref();
    let offset = usize::from(d[d.len() - 1] & 0x0f);
    let bin = (u32::from(d[offset]) & 0x7f) << 24
        | u32::from(d[offset + 1]) << 16
        | u32::from(d[offset + 2]) << 8
        | u32::from(d[offset + 3]);

    bin % 10u32.pow(DIGITS)
}

/// The code at Unix time `unix_secs`.
#[must_use]
pub fn code_at(secret: &[u8], unix_secs: i64) -> u32 {
    code_for_counter(secret, u64::try_from(unix_secs.max(0) / STEP_SECS).unwrap_or(0))
}

/// Verifies `code` allowing ±1 step of clock drift.
#[must_use]
pub fn verify(secret: &[u8], code: &str, unix_secs: i64) -> bool {
    matched_step(secret, code, unix_secs).is_some()
}

/// The time step (`unix_secs / 30`) whose code `code` is, allowing ±1 step
/// of drift, so a caller can refuse a step that was already used.
#[must_use]
pub fn matched_step(secret: &[u8], code: &str, unix_secs: i64) -> Option<i64> {
    let code = code.trim();
    if code.len() != DIGITS as usize || !code.chars().all(|c| c.is_ascii_digit()) {
        return None;
    }

    let n = code.parse::<u32>().ok()?;
    let mut found = None;

    for drift in [-1i64, 0, 1] {
        let at = unix_secs + drift * STEP_SECS;

        // Evaluate all windows to keep timing independent of which matched.
        if forge_secrets::crypto::constant_time_eq(&code_at(secret, at).to_be_bytes(), &n.to_be_bytes()) {
            found = Some(at.max(0) / STEP_SECS);
        }
    }

    found
}

/// RFC 4648 base32 without padding (authenticator apps expect this).
#[must_use]
pub fn base32_encode(data: &[u8]) -> String {
    const ALPHABET: &[u8; 32] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZ234567";
    let mut out = String::with_capacity(data.len().div_ceil(5) * 8);
    let mut buffer: u32 = 0;
    let mut bits = 0u32;

    for &byte in data {
        buffer = (buffer << 8) | u32::from(byte);
        bits += 8;

        while bits >= 5 {
            out.push(char::from(ALPHABET[((buffer >> (bits - 5)) & 31) as usize]));
            bits -= 5;
        }
    }

    if bits > 0 {
        out.push(char::from(ALPHABET[((buffer << (5 - bits)) & 31) as usize]));
    }

    out
}

/// Decodes base32 (case-insensitive, padding and spaces ignored).
#[must_use]
pub fn base32_decode(s: &str) -> Option<Vec<u8>> {
    let mut out = Vec::new();
    let mut buffer: u32 = 0;
    let mut bits = 0u32;

    for c in s.chars().filter(|c| !matches!(c, '=' | ' ')) {
        let v = match c.to_ascii_uppercase() {
            c @ 'A'..='Z' => c as u32 - 'A' as u32,
            c @ '2'..='7' => c as u32 - '2' as u32 + 26,
            _ => return None,
        };

        buffer = (buffer << 5) | v;
        bits += 5;

        if bits >= 8 {
            out.push(u8::try_from((buffer >> (bits - 8)) & 0xff).ok()?);
            bits -= 8;
        }
    }

    Some(out)
}

/// `otpauth://` provisioning URI for QR codes.
#[must_use]
pub fn provisioning_uri(secret: &[u8], issuer: &str, account: &str) -> String {
    let label = url::form_urlencoded::byte_serialize(format!("{issuer}:{account}").as_bytes()).collect::<String>();
    let issuer_q = url::form_urlencoded::byte_serialize(issuer.as_bytes()).collect::<String>();

    format!(
        "otpauth://totp/{label}?secret={}&issuer={issuer_q}&algorithm=SHA1&digits={DIGITS}&period={STEP_SECS}",
        base32_encode(secret)
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    // RFC 6238 Appendix B, SHA1 secret "12345678901234567890".
    const SECRET: &[u8] = b"12345678901234567890";

    #[test]
    fn rfc6238_vectors() {
        // The RFC lists 8-digit codes; we use the last 6 digits.
        for (t, code8) in
            [(59, 94_287_082u32), (1_111_111_109, 7_081_804), (1_234_567_890, 89_005_924), (2_000_000_000, 69_279_037)]
        {
            assert_eq!(code_at(SECRET, t), code8 % 1_000_000, "t={t}");
        }
    }

    #[test]
    fn verify_allows_one_step_drift() {
        let now = 1_700_000_000;
        let code = format!("{:06}", code_at(SECRET, now - 30));
        assert!(verify(SECRET, &code, now));
        let old = format!("{:06}", code_at(SECRET, now - 90));
        assert!(!verify(SECRET, &old, now));
        assert!(!verify(SECRET, "12345", now));
        assert!(!verify(SECRET, "abcdef", now));
    }

    #[test]
    fn base32_round_trip() {
        assert_eq!(base32_encode(b"foobar"), "MZXW6YTBOI");
        assert_eq!(base32_decode("MZXW6YTBOI").as_deref(), Some(&b"foobar"[..]));
        let s = generate_secret();
        assert_eq!(base32_decode(&base32_encode(&s)), Some(s));
    }
}
