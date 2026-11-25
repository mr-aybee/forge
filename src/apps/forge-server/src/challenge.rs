//! Login challenge (DECISIONS.md D-064): a self-hosted proof of work that
//! every password sign-in must carry, so guessing passwords costs the
//! guesser CPU time. No third-party service. Challenges are single use,
//! expire after two minutes and live in memory. Difficulty rises for a sign-in
//! name after failed attempts; the per-name throttle still applies.

use std::collections::HashMap;
use std::net::IpAddr;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use forge_secrets::pow;

/// How long a challenge may be solved and used.
pub const TTL: Duration = Duration::from_secs(120);
/// Zero bits normally: about 2^17 hashes, a fraction of a second in a browser.
pub const BASE_BITS: u8 = 17;
/// Extra bits per recent failure for the same sign-in name, up to this many.
const MAX_EXTRA_BITS: u8 = 7;
/// Failures count for this long.
const FAILURE_WINDOW: Duration = Duration::from_secs(15 * 60);
/// Outstanding challenges kept at most (then new ones are refused).
const MAX_PENDING: usize = 20_000;

#[derive(Debug, Clone)]
pub struct Issued {
    pub id: String,
    pub salt_hex: String,
    pub bits: u8,
    pub expires_in: Duration,
}

#[derive(Debug)]
struct Pending {
    salt: [u8; 16],
    bits: u8,
    at: Instant,
}

#[derive(Debug, Default)]
struct Inner {
    pending: HashMap<String, Pending>,
    failures: HashMap<String, (u8, Instant)>,
}

/// Why a challenge answer was refused.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Refusal {
    /// Unknown, used or expired: fetch a new one.
    Invalid,
    /// Issued easier than this sign-in name now needs: fetch a new one with `login`.
    TooEasy,
    /// The nonce does not solve it.
    Wrong,
}

#[derive(Debug, Default)]
pub struct LoginChallenges(Mutex<Inner>);

fn key(login: &str) -> String {
    login.trim().to_lowercase()
}

/// Failures also count per client address, except loopback (a local proxy
/// or the server itself would otherwise stand for every client).
fn ip_key(ip: Option<IpAddr>) -> Option<String> {
    ip.filter(|ip| !ip.is_loopback() && !ip.to_canonical().is_loopback()).map(|ip| format!("ip:{ip}"))
}

impl Inner {
    fn bits_for(&mut self, login: Option<&str>, ip: Option<IpAddr>) -> u8 {
        self.failures.retain(|_, (_, at)| at.elapsed() < FAILURE_WINDOW);
        let count = |k: Option<String>| k.and_then(|k| self.failures.get(&k).map(|(n, _)| *n)).unwrap_or(0);
        let extra = count(login.map(key)).max(count(ip_key(ip))).min(MAX_EXTRA_BITS);

        (BASE_BITS + extra).min(pow::MAX_BITS)
    }
}

impl LoginChallenges {
    /// A new challenge, harder when `login` failed recently. `None` when too
    /// many are outstanding.
    pub fn issue(&self, login: Option<&str>, ip: Option<IpAddr>) -> Option<Issued> {
        let mut m = self.0.lock().ok()?;
        m.pending.retain(|_, p| p.at.elapsed() < TTL);

        if m.pending.len() >= MAX_PENDING {
            return None;
        }

        let bits = m.bits_for(login, ip);
        let salt = forge_secrets::crypto::random_bytes::<16>();
        let id = forge_secrets::crypto::random_token::<18>();
        m.pending.insert(id.clone(), Pending { salt, bits, at: Instant::now() });

        Some(Issued { id, salt_hex: salt.iter().map(|b| format!("{b:02x}")).collect(), bits, expires_in: TTL })
    }

    /// Checks and uses up the challenge `id` for a sign-in as `login`.
    pub fn verify(&self, id: &str, nonce: &str, login: &str, ip: Option<IpAddr>) -> Result<(), Refusal> {
        let Ok(mut m) = self.0.lock() else { return Err(Refusal::Invalid) };
        let p = m.pending.remove(id).filter(|p| p.at.elapsed() < TTL).ok_or(Refusal::Invalid)?;
        if p.bits < m.bits_for(Some(login), ip) {
            return Err(Refusal::TooEasy);
        }

        if pow::check(&p.salt, nonce, p.bits) { Ok(()) } else { Err(Refusal::Wrong) }
    }

    /// A failed password for `login`: its next challenges are harder.
    pub fn failed(&self, login: &str, ip: Option<IpAddr>) {
        if let Ok(mut m) = self.0.lock() {
            for k in std::iter::once(key(login)).chain(ip_key(ip)) {
                let e = m.failures.entry(k).or_insert((0, Instant::now()));
                e.0 = e.0.saturating_add(1);
                e.1 = Instant::now();
            }
        }
    }

    /// A successful sign-in resets the difficulty for `login`; the count for
    /// the address stays, so one success does not hide password spraying.
    pub fn succeeded(&self, login: &str) {
        if let Ok(mut m) = self.0.lock() {
            m.failures.remove(&key(login));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn salt(i: &Issued) -> Vec<u8> {
        pow::salt_from_hex(&i.salt_hex).unwrap_or_default()
    }

    #[test]
    fn single_use_and_checked() {
        let c = LoginChallenges::default();
        let i = c.issue(None, None).unwrap_or_else(|| unreachable!());
        assert_eq!(i.bits, BASE_BITS);
        let n = pow::solve(&salt(&i), i.bits);
        assert_eq!(c.verify(&i.id, "x", "admin", None), Err(Refusal::Wrong), "a wrong nonce uses it up too");
        assert_eq!(c.verify(&i.id, &n, "admin", None), Err(Refusal::Invalid));
        let i = c.issue(None, None).unwrap_or_else(|| unreachable!());
        let n = pow::solve(&salt(&i), i.bits);
        assert_eq!(c.verify(&i.id, &n, "admin", None), Ok(()));
        assert_eq!(c.verify(&i.id, &n, "admin", None), Err(Refusal::Invalid), "reused");
        assert_eq!(c.verify("unknown", &n, "admin", None), Err(Refusal::Invalid));
    }

    #[test]
    fn failures_raise_the_difficulty_for_that_address() {
        let c = LoginChallenges::default();
        let attacker: IpAddr = "203.0.113.7".parse().unwrap_or_else(|_| unreachable!());
        let other: IpAddr = "198.51.100.1".parse().unwrap_or_else(|_| unreachable!());
        let local: IpAddr = "127.0.0.1".parse().unwrap_or_else(|_| unreachable!());

        for name in ["a", "b", "c"] {
            c.failed(name, Some(attacker));
            c.failed(name, Some(local));
        }

        // Spraying many names from one address makes every name harder there.
        assert_eq!(c.issue(Some("d"), Some(attacker)).map(|i| i.bits), Some(BASE_BITS + 3));
        assert_eq!(c.issue(Some("d"), Some(other)).map(|i| i.bits), Some(BASE_BITS));
        assert_eq!(c.issue(Some("d"), Some(local)).map(|i| i.bits), Some(BASE_BITS), "loopback is not counted");
        c.succeeded("a");
        assert_eq!(
            c.issue(Some("a"), Some(attacker)).map(|i| i.bits),
            Some(BASE_BITS + 3),
            "a success keeps the address count"
        );
    }

    #[test]
    fn expired_challenges_are_refused() {
        let c = LoginChallenges::default();
        let i = c.issue(None, None).unwrap_or_else(|| unreachable!());
        let n = pow::solve(&salt(&i), i.bits);

        if let Ok(mut m) = c.0.lock()
            && let Some(p) = m.pending.get_mut(&i.id)
        {
            p.at = Instant::now().checked_sub(TTL + Duration::from_secs(1)).unwrap_or_else(Instant::now);
        }

        assert_eq!(c.verify(&i.id, &n, "admin", None), Err(Refusal::Invalid));
    }

    #[test]
    fn failures_raise_the_difficulty_for_that_name() {
        let c = LoginChallenges::default();
        let easy = c.issue(None, None).unwrap_or_else(|| unreachable!());
        c.failed("Admin", None);
        c.failed("admin ", None);
        assert_eq!(c.issue(Some("ADMIN"), None).map(|i| i.bits), Some(BASE_BITS + 2));
        assert_eq!(c.issue(Some("other"), None).map(|i| i.bits), Some(BASE_BITS));
        let n = pow::solve(&salt(&easy), easy.bits);
        assert_eq!(c.verify(&easy.id, &n, "admin", None), Err(Refusal::TooEasy));
        c.succeeded("admin");
        assert_eq!(c.issue(Some("admin"), None).map(|i| i.bits), Some(BASE_BITS));

        for _ in 0..50 {
            c.failed("x", None);
        }

        assert_eq!(c.issue(Some("x"), None).map(|i| i.bits), Some(BASE_BITS + MAX_EXTRA_BITS));
    }
}
