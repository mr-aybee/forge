//! One-time enrollment tokens.
//!
//! Display format: `<secret>.<ca-sha256-hex>`. Only `sha256(secret)` is
//! stored. The fingerprint lets the agent verify the CA it downloads.

use forge_secrets::crypto::{random_token, sha256_hex};

/// A newly generated token. `display` is shown to the operator exactly once.
#[derive(Debug, Clone)]
pub struct NewEnrollmentToken {
    pub display: String,
    pub secret_hash: String,
}

/// A token as typed into the agent installer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParsedToken {
    pub secret: String,
    pub ca_fingerprint: String,
}

#[must_use]
pub fn generate(ca_fingerprint: &str) -> NewEnrollmentToken {
    let secret = random_token::<32>();
    NewEnrollmentToken { secret_hash: hash_secret(&secret), display: format!("{secret}.{ca_fingerprint}") }
}

#[must_use]
pub fn hash_secret(secret: &str) -> String {
    sha256_hex(secret.as_bytes())
}

/// Splits a display token. Returns `None` if it is malformed.
#[must_use]
pub fn parse(token: &str) -> Option<ParsedToken> {
    let (secret, fp) = token.trim().rsplit_once('.')?;
    let fp_ok = fp.len() == 64 && fp.chars().all(|c| c.is_ascii_hexdigit());
    (fp_ok && secret.len() >= 32).then(|| ParsedToken { secret: secret.to_owned(), ca_fingerprint: fp.to_lowercase() })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trip() {
        let fp = "ab".repeat(32);
        let t = generate(&fp);
        let p = parse(&t.display).expect("parses");
        assert_eq!(p.ca_fingerprint, fp);
        assert_eq!(hash_secret(&p.secret), t.secret_hash);
    }

    #[test]
    fn malformed_tokens_rejected() {
        assert!(parse("nope").is_none());
        assert!(parse("short.abcd").is_none());
        assert!(parse(&format!("{}.{}", "a".repeat(43), "zz".repeat(32))).is_none());
    }
}
