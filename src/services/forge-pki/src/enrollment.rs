//! One-time enrollment tokens.
//!
//! Display format: `<secret>.<server-key-sha256>.<ca-sha256>` (D-167), or
//! `<secret>.<ca-sha256>` before it. Only `sha256(secret)` is stored. The
//! CA fingerprint lets the agent verify the CA it downloads; the server key
//! fingerprint is the control-plane key the agent pins, so it can reach the
//! control plane by any address.
//!
//! An agent older than D-167 splits at the last dot: it checks the CA as
//! before and sends `<secret>.<server-key-sha256>` as the secret, which
//! [`secret_part`] reduces to the secret (a secret never contains a dot).

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
    /// The control-plane key to pin; `None` in tokens from before D-167.
    pub server_key_sha256: Option<String>,
}

/// A token pinning the CA and, when given, the control-plane key.
#[must_use]
pub fn generate(ca_fingerprint: &str, server_key_sha256: Option<&str>) -> NewEnrollmentToken {
    let secret = random_token::<32>();

    let display = match server_key_sha256 {
        Some(key) => format!("{secret}.{key}.{ca_fingerprint}"),
        None => format!("{secret}.{ca_fingerprint}"),
    };

    NewEnrollmentToken { secret_hash: hash_secret(&secret), display }
}

#[must_use]
pub fn hash_secret(secret: &str) -> String {
    sha256_hex(secret_part(secret).as_bytes())
}

/// The secret of what an agent sent as one: an agent older than D-167 sends
/// `<secret>.<server-key-sha256>`.
#[must_use]
pub fn secret_part(sent: &str) -> &str {
    sent.split('.').next().unwrap_or(sent)
}

fn is_fingerprint(s: &str) -> bool {
    s.len() == 64 && s.chars().all(|c| c.is_ascii_hexdigit())
}

/// Splits a display token. Returns `None` if it is malformed.
#[must_use]
pub fn parse(token: &str) -> Option<ParsedToken> {
    let (rest, fp) = token.trim().rsplit_once('.')?;
    let (secret, key) = match rest.split_once('.') {
        Some((s, k)) => (s, Some(k)),
        None => (rest, None),
    };

    let ok = is_fingerprint(fp) && secret.len() >= 32 && key.is_none_or(is_fingerprint);

    ok.then(|| ParsedToken {
        secret: secret.to_owned(),
        ca_fingerprint: fp.to_lowercase(),
        server_key_sha256: key.map(str::to_lowercase),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trip() {
        let fp = "ab".repeat(32);
        let t = generate(&fp, None);
        let p = parse(&t.display).expect("parses");
        assert_eq!(p.ca_fingerprint, fp);
        assert_eq!(p.server_key_sha256, None);
        assert_eq!(hash_secret(&p.secret), t.secret_hash);
    }

    #[test]
    fn the_server_key_rides_along_and_old_agents_still_enroll() {
        let (ca, key) = ("ab".repeat(32), "CD".repeat(32));
        let t = generate(&ca, Some(&key));
        let p = parse(&t.display).expect("parses");
        assert_eq!((p.ca_fingerprint.as_str(), p.server_key_sha256.as_deref()), (ca.as_str(), Some(&*"cd".repeat(32))));
        assert_eq!(hash_secret(&p.secret), t.secret_hash);

        // An old agent splits at the last dot and sends `<secret>.<key>`.
        let (old_secret, old_ca) = t.display.rsplit_once('.').expect("dot");
        assert_eq!(old_ca, ca);
        assert_eq!(hash_secret(old_secret), t.secret_hash);
    }

    #[test]
    fn malformed_tokens_rejected() {
        assert!(parse("nope").is_none());
        assert!(parse("short.abcd").is_none());
        assert!(parse(&format!("{}.{}", "a".repeat(43), "zz".repeat(32))).is_none());
        assert!(parse(&format!("{}.xyz.{}", "a".repeat(43), "ab".repeat(32))).is_none());
    }
}
