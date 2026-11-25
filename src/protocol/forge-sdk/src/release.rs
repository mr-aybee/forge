//! Verifying Orbyn Forge releases (D-037), shared by the server's self-update
//! and `forge update` (D-076).
//!
//! A release publishes per-platform binaries, `SHA256SUMS` and
//! `SHA256SUMS.sig`: an Ed25519 signature over the sums file, base64. The
//! public key is compiled in; a downloaded file is trusted only when its
//! SHA-256 is listed in a sums file whose signature checks out.

use std::cmp::Ordering;
use std::collections::BTreeMap;

use base64::Engine;

/// Public key of the Orbyn Forge release signing key (Ed25519, base64url).
pub const RELEASE_PUBLIC_KEY: &str = "6yf7l7jBEw6u446vqWAgt7P7bqEuaY5tpbYnWPHBmbg";

/// Where releases are published (GitHub "latest release" document).
pub const DEFAULT_FEED: &str = "https://api.github.com/repos/orbyntech/forge/releases/latest";

/// Verifies `sig_b64` (Ed25519) over `data` with a base64url public key.
///
/// # Errors
/// When the key or signature is malformed, or the signature does not match.
pub fn verify_signature(public_key_b64url: &str, data: &[u8], sig_b64: &str) -> Result<(), String> {
    let pk = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(public_key_b64url.trim())
        .map_err(|_| "the release public key is not valid base64url")?;
    let sig = base64::engine::general_purpose::STANDARD
        .decode(sig_b64.trim())
        .map_err(|_| "the release signature is not valid base64")?;

    ring::signature::UnparsedPublicKey::new(&ring::signature::ED25519, pk)
        .verify(data, &sig)
        .map_err(|_| "the release signature does not match the Forge release key".to_owned())
}

/// `sha256  name` lines → name → lower-case hex digest.
#[must_use]
pub fn parse_sums(text: &str) -> BTreeMap<String, String> {
    text.lines()
        .filter_map(|l| {
            let mut f = l.split_whitespace();
            let (h, n) = (f.next()?, f.next()?);
            (h.len() == 64 && h.bytes().all(|b| b.is_ascii_hexdigit()))
                .then(|| (n.trim_start_matches('*').to_owned(), h.to_ascii_lowercase()))
        })
        .collect()
}

/// Lower-case hex SHA-256.
#[must_use]
pub fn sha256_hex(b: &[u8]) -> String {
    use std::fmt::Write as _;
    ring::digest::digest(&ring::digest::SHA256, b).as_ref().iter().fold(String::with_capacity(64), |mut s, x| {
        let _ = write!(s, "{x:02x}");
        s
    })
}

/// Orders two versions of Forge's own binaries (server, agent, CLI).
///
/// A version is `X.Y.Z` with an optional pre-release suffix after the first
/// `-`: `1.0.0-rc-01`, `1.0.0-rc.2`, `0.1.1-dev.20261122.162457`. The numeric
/// core is compared part by part (a missing part is 0). A version with a
/// suffix sorts **below** the same core without one, so a release candidate
/// or a dev build of `1.0.0` is older than `1.0.0` itself. Two suffixes are
/// compared identifier by identifier (split at `.` and `-`): numbers
/// numerically, words alphabetically, a number before a word, and the
/// shorter list first when one is a prefix of the other. Build metadata after
/// `+` is ignored. A leading `v` is ignored.
///
/// Packages managed on nodes keep Debian ordering
/// (`forge_catalog::versions::compare`); this is only for Forge itself.
#[must_use]
pub fn compare_release(a: &str, b: &str) -> Ordering {
    let (ca, pa) = split_release(a);
    let (cb, pb) = split_release(b);

    cmp_core(ca, cb).then_with(|| match (pa, pb) {
        (None, None) => Ordering::Equal,
        (None, Some(_)) => Ordering::Greater,
        (Some(_), None) => Ordering::Less,
        (Some(x), Some(y)) => cmp_pre(x, y),
    })
}

/// Whether `v` carries a pre-release suffix (`-rc-01`, `-dev.…`).
#[must_use]
pub fn is_prerelease(v: &str) -> bool {
    split_release(v).1.is_some()
}

/// `(core, suffix)`: `v1.2.3-rc.1+meta` → (`1.2.3`, `rc.1`).
fn split_release(v: &str) -> (&str, Option<&str>) {
    let v = v.trim();
    let v = v.strip_prefix('v').unwrap_or(v);
    let v = v.split_once('+').map_or(v, |(x, _)| x);
    match v.split_once('-') {
        Some((core, pre)) if !pre.is_empty() => (core, Some(pre)),
        Some((core, _)) => (core, None),
        None => (v, None),
    }
}

fn cmp_core(a: &str, b: &str) -> Ordering {
    let (mut x, mut y) = (a.split('.'), b.split('.'));

    loop {
        match (x.next(), y.next()) {
            (None, None) => return Ordering::Equal,
            (p, q) => {
                // A missing or empty part is 0: `1.0` = `1.0.0`, and an
                // unknown (empty) version is older than any release.
                let o = cmp_ident(nonzero(p), nonzero(q));
                if o != Ordering::Equal {
                    return o;
                }
            }
        }
    }
}

fn nonzero(s: Option<&str>) -> &str {
    s.filter(|s| !s.is_empty()).unwrap_or("0")
}

fn cmp_pre(a: &str, b: &str) -> Ordering {
    let sep = |c: char| c == '.' || c == '-';
    let (mut x, mut y) = (a.split(sep), b.split(sep));

    loop {
        match (x.next(), y.next()) {
            (None, None) => return Ordering::Equal,
            (None, Some(_)) => return Ordering::Less,
            (Some(_), None) => return Ordering::Greater,
            (Some(p), Some(q)) => {
                let o = cmp_ident(p, q);
                if o != Ordering::Equal {
                    return o;
                }
            }
        }
    }
}

/// Numbers numerically (any length, leading zeros ignored), before words;
/// words by bytes.
fn cmp_ident(a: &str, b: &str) -> Ordering {
    let num = |s: &str| !s.is_empty() && s.bytes().all(|c| c.is_ascii_digit());
    match (num(a), num(b)) {
        (true, true) => {
            let (a, b) = (a.trim_start_matches('0'), b.trim_start_matches('0'));
            a.len().cmp(&b.len()).then_with(|| a.cmp(b))
        }
        (true, false) => Ordering::Less,
        (false, true) => Ordering::Greater,
        (false, false) => a.cmp(b),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sums_parse_and_hash_matches() {
        let s = parse_sums("ab  x\n0000000000000000000000000000000000000000000000000000000000000000 *forge-cli-x\n");
        assert_eq!(s.len(), 1);
        assert_eq!(s["forge-cli-x"].len(), 64);
        assert_eq!(sha256_hex(b"abc"), "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad");
    }

    #[test]
    fn signatures_are_checked() {
        use ring::signature::KeyPair;
        let rng = ring::rand::SystemRandom::new();
        let Ok(pkcs8) = ring::signature::Ed25519KeyPair::generate_pkcs8(&rng) else { return };
        let Ok(kp) = ring::signature::Ed25519KeyPair::from_pkcs8(pkcs8.as_ref()) else { return };
        let public = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(kp.public_key().as_ref());
        let sig = base64::engine::general_purpose::STANDARD.encode(kp.sign(b"sums").as_ref());
        assert!(verify_signature(&public, b"sums", &sig).is_ok());
        assert!(verify_signature(&public, b"other", &sig).is_err());
        assert!(verify_signature(RELEASE_PUBLIC_KEY, b"sums", &sig).is_err());
    }

    #[test]
    fn release_versions_order_prereleases_below_the_final() {
        use Ordering::{Equal, Greater, Less};
        let table = [
            ("1.0.0", "1.0.0", Equal),
            ("v1.0.0", "1.0.0", Equal),
            ("1.0", "1.0.0", Equal),
            ("1.0.0+build.7", "1.0.0", Equal),
            ("1.0.1", "1.0.0", Greater),
            ("1.10.0", "1.9.9", Greater),
            ("2.0.0", "1.99.99", Greater),
            // A release candidate or a dev build of X.Y.Z is older than X.Y.Z.
            ("1.0.0-rc-01", "1.0.0", Less),
            ("1.0.0-rc.1", "1.0.0", Less),
            ("1.0.0-dev.20261122.162457", "1.0.0", Less),
            ("1.0.0-rc-01", "0.9.9", Greater),
            ("1.0.0-rc-02", "1.0.0-rc-01", Greater),
            ("1.0.0-rc-10", "1.0.0-rc-9", Greater),
            ("1.0.0-rc.10", "1.0.0-rc.2", Greater),
            ("1.0.0-dev.20261122.162457", "1.0.0-rc-01", Less),
            ("0.1.1-dev.20261112.153001", "0.1.1-dev.20261112.153000", Greater),
            ("0.1.1-dev.20261113.090000", "0.1.1-dev.20261112.235959", Greater),
            // Dev builds are stamped with the next version: above the last release.
            ("0.1.1-dev.20261112.153000", "0.1.0", Greater),
            ("1.0.0-rc", "1.0.0-rc.1", Less),
            ("1.0.0-alpha", "1.0.0-beta", Less),
            ("1.0.0-1", "1.0.0-alpha", Less),
            ("", "0.0.1", Less),
            ("", "0.0.0", Equal),
        ];

        for (a, b, want) in table {
            assert_eq!(compare_release(a, b), want, "{a} vs {b}");
            assert_eq!(compare_release(b, a), want.reverse(), "{b} vs {a}");
        }

        assert!(is_prerelease("1.0.0-rc-01"));
        assert!(is_prerelease("0.1.1-dev.1"));
        assert!(!is_prerelease("1.0.0"));
        assert!(!is_prerelease("1.0.0+meta"));
    }
}
