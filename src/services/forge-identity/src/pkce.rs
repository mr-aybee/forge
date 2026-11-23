//! PKCE (RFC 7636), S256 only.

use forge_secrets::crypto::{b64url, constant_time_eq, sha256};

/// `BASE64URL(SHA256(verifier))`.
#[must_use]
pub fn s256_challenge(verifier: &str) -> String {
    b64url(&sha256(verifier.as_bytes()))
}

/// Validates the verifier format (43–128 unreserved characters).
#[must_use]
pub fn verifier_is_valid(verifier: &str) -> bool {
    (43..=128).contains(&verifier.len())
        && verifier.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '.' | '_' | '~'))
}

#[must_use]
pub fn verify(verifier: &str, challenge: &str) -> bool {
    verifier_is_valid(verifier) && constant_time_eq(s256_challenge(verifier).as_bytes(), challenge.as_bytes())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rfc7636_appendix_b() {
        let v = "dBjftJeZ4CVP-mB92K27uhbUJU1p1r_wW1gFWFOEjXk";
        assert_eq!(s256_challenge(v), "E9Melhoa2OwvFrEMTJguCHaoeK1t8URWbuGJSstw-cM");
        assert!(verify(v, "E9Melhoa2OwvFrEMTJguCHaoeK1t8URWbuGJSstw-cM"));
        assert!(!verify("short", "x"));
    }
}
