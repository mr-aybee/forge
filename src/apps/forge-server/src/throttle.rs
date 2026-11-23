//! Login throttling: bounds online password guessing per account, per
//! client address and per account-and-address, and finds the client address
//! behind configured reverse proxies.

use std::collections::HashMap;
use std::net::{IpAddr, SocketAddr};
use std::sync::Mutex;
use std::time::{Duration, Instant};

use axum::http::HeaderMap;

const WINDOW: Duration = Duration::from_secs(15 * 60);
const MAX_FAILURES: u32 = 10;
const MAX_KEYS: usize = 100_000;
/// Failures from one address across all accounts (password spraying).
pub const MAX_PER_ADDRESS: u32 = 50;
/// Failures for one account from all addresses (distributed guessing). Far
/// above [`MAX_FAILURES`], so one attacker cannot lock the owner out.
pub const MAX_PER_ACCOUNT: u32 = 100;

/// Throttle keys for one sign-in attempt, each with its limit: account and
/// address, account, and address. A loopback address is almost always a
/// reverse proxy on the same host that is not in `trusted_proxies`: every
/// client would share it, so it gets no address-wide limit (that would let
/// anyone block sign-in for everyone).
#[must_use]
pub fn login_keys(login: &str, ip: IpAddr) -> Vec<(String, u32)> {
    let mut keys = vec![(format!("{login}|{ip}"), MAX_FAILURES), (format!("account:{login}"), MAX_PER_ACCOUNT)];
    if !ip.is_loopback() && !ip.to_canonical().is_loopback() {
        keys.push((format!("ip:{ip}"), MAX_PER_ADDRESS));
    }
    keys
}

/// An IP address or CIDR from `[security] trusted_proxies`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TrustedNet {
    addr: IpAddr,
    prefix: u8,
}

impl std::str::FromStr for TrustedNet {
    type Err = String;
    fn from_str(s: &str) -> Result<Self, String> {
        let s = s.trim();
        let (ip, prefix) = s.split_once('/').map_or((s, None), |(a, p)| (a, Some(p)));
        let addr: IpAddr = ip.parse().map_err(|_| format!("`{s}` is not an IP address or CIDR"))?;
        let max = if addr.is_ipv4() { 32 } else { 128 };
        let prefix = match prefix {
            None => max,
            Some(p) => p.parse::<u8>().ok().filter(|p| *p <= max).ok_or_else(|| format!("`{s}`: bad prefix length"))?,
        };
        Ok(Self { addr, prefix })
    }
}

impl TrustedNet {
    pub(crate) fn contains(&self, ip: IpAddr) -> bool {
        let bits = |ip: IpAddr| -> (u128, u8) {
            match ip {
                IpAddr::V4(v) => (u128::from(u32::from(v)), 32),
                IpAddr::V6(v) => match v.to_ipv4_mapped() {
                    Some(v4) => (u128::from(u32::from(v4)), 32),
                    None => (u128::from(v), 128),
                },
            }
        };
        let ((net, w1), (ip, w2)) = (bits(self.addr), bits(ip));
        if w1 != w2 {
            return false;
        }
        let shift = u32::from(w1 - self.prefix.min(w1));
        shift >= 128 || (net >> shift) == (ip >> shift)
    }
}

/// The client address: the TCP peer, unless the peer is a trusted proxy, in
/// which case the rightmost `X-Forwarded-For` entry that is not itself a
/// trusted proxy. Without trusted proxies the header is ignored (it can be
/// forged by anyone).
#[must_use]
pub fn client_ip(headers: &HeaderMap, peer: SocketAddr, trusted: &[TrustedNet]) -> IpAddr {
    let peer = peer.ip();
    let is_trusted = |ip: IpAddr| trusted.iter().any(|t| t.contains(ip));
    if !is_trusted(peer) {
        return peer;
    }
    let chain: Vec<IpAddr> = headers
        .get_all("x-forwarded-for")
        .iter()
        .filter_map(|v| v.to_str().ok())
        .flat_map(|v| v.split(','))
        .filter_map(|p| p.trim().parse::<IpAddr>().ok())
        .collect();
    chain.into_iter().rev().find(|ip| !is_trusted(*ip)).unwrap_or(peer)
}

/// Whether the browser reached a trusted proxy over HTTPS: the TCP peer is
/// in `trusted` and its last `X-Forwarded-Proto` value is `https`. From any
/// other peer the header is ignored (anyone can send it).
#[must_use]
pub fn forwarded_https(headers: &HeaderMap, peer: SocketAddr, trusted: &[TrustedNet]) -> bool {
    if !trusted.iter().any(|t| t.contains(peer.ip())) {
        return false;
    }
    headers
        .get_all("x-forwarded-proto")
        .iter()
        .filter_map(|v| v.to_str().ok())
        .flat_map(|v| v.split(','))
        .next_back()
        .is_some_and(|p| p.trim().eq_ignore_ascii_case("https"))
}

#[derive(Debug, Default)]
pub struct LoginThrottle {
    failures: Mutex<HashMap<String, (u32, Instant)>>,
}

impl LoginThrottle {
    /// Whether another attempt for `key` is allowed now.
    pub fn allowed(&self, key: &str) -> bool {
        self.allowed_up_to(key, MAX_FAILURES)
    }

    /// Whether another attempt for `key` is allowed with a limit of `max`
    /// failures per window.
    pub fn allowed_up_to(&self, key: &str, max: u32) -> bool {
        let Ok(map) = self.failures.lock() else { return false };
        map.get(key).is_none_or(|(n, since)| *n < max || since.elapsed() > WINDOW)
    }

    pub fn record_failure(&self, key: &str) {
        let Ok(mut map) = self.failures.lock() else { return };
        if map.len() >= MAX_KEYS {
            map.retain(|_, (_, since)| since.elapsed() <= WINDOW);
        }
        let e = map.entry(key.to_owned()).or_insert((0, Instant::now()));
        if e.1.elapsed() > WINDOW {
            *e = (0, Instant::now());
        }
        e.0 += 1;
    }

    pub fn reset(&self, key: &str) {
        if let Ok(mut map) = self.failures.lock() {
            map.remove(key);
        }
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    fn peer(ip: &str) -> SocketAddr {
        SocketAddr::new(ip.parse().unwrap(), 4000)
    }

    fn xff(v: &str) -> HeaderMap {
        let mut h = HeaderMap::new();
        h.insert("x-forwarded-for", v.parse().unwrap());
        h
    }

    /// Security review: the header is trusted only from configured proxies,
    /// and the rightmost untrusted hop is the client.
    #[test]
    fn client_address_trusts_only_configured_proxies() {
        let trusted: Vec<TrustedNet> = ["127.0.0.1", "10.0.0.0/8"].iter().map(|s| s.parse().unwrap()).collect();
        let forged = xff("203.0.113.9");
        assert_eq!(client_ip(&forged, peer("198.51.100.7"), &trusted).to_string(), "198.51.100.7", "direct client");
        assert_eq!(client_ip(&forged, peer("198.51.100.7"), &[]).to_string(), "198.51.100.7", "no proxies configured");
        assert_eq!(client_ip(&forged, peer("127.0.0.1"), &trusted).to_string(), "203.0.113.9", "behind the proxy");
        let spoofed = xff("1.2.3.4, 203.0.113.9, 10.1.2.3");
        assert_eq!(
            client_ip(&spoofed, peer("10.0.0.1"), &trusted).to_string(),
            "203.0.113.9",
            "entries left of the last untrusted hop are the client's own claims"
        );
        assert_eq!(client_ip(&HeaderMap::new(), peer("127.0.0.1"), &trusted).to_string(), "127.0.0.1");
        assert_eq!(client_ip(&xff("garbage"), peer("::ffff:127.0.0.1"), &trusted).to_string(), "::ffff:127.0.0.1");
        assert!("10.0.0.0/33".parse::<TrustedNet>().is_err() && "nope".parse::<TrustedNet>().is_err());
        assert!("2001:db8::/32".parse::<TrustedNet>().unwrap().contains("2001:db8::1".parse().unwrap()));
        assert!("0.0.0.0/0".parse::<TrustedNet>().unwrap().contains("8.8.8.8".parse().unwrap()));
    }

    /// One address cannot lock an account for everyone, and spraying from
    /// one address is limited across accounts.
    #[test]
    fn limits_per_account_address_and_both() {
        let t = LoginThrottle::default();
        let a: IpAddr = "198.51.100.7".parse().unwrap();
        let b: IpAddr = "203.0.113.9".parse().unwrap();
        for _ in 0..MAX_FAILURES {
            for (k, _) in login_keys("admin", a) {
                t.record_failure(&k);
            }
        }
        let ok = |login: &str, ip| login_keys(login, ip).iter().all(|(k, max)| t.allowed_up_to(k, *max));
        assert!(!ok("admin", a), "the attacker's address is stopped for this account");
        assert!(ok("admin", b), "the owner can still sign in from elsewhere");
        for i in 0..MAX_PER_ADDRESS {
            for (k, _) in login_keys(&format!("user{i}"), a) {
                t.record_failure(&k);
            }
        }
        assert!(!ok("someone-else", a), "spraying from one address is stopped");
        let local: IpAddr = "127.0.0.1".parse().unwrap();
        assert_eq!(login_keys("x", local).len(), 2, "an unconfigured local proxy gets no address-wide limit");
    }

    #[test]
    fn locks_after_max_failures() {
        let t = LoginThrottle::default();
        for _ in 0..MAX_FAILURES {
            assert!(t.allowed("a"));
            t.record_failure("a");
        }
        assert!(!t.allowed("a"));
        assert!(t.allowed("b"));
        t.reset("a");
        assert!(t.allowed("a"));
    }
}
