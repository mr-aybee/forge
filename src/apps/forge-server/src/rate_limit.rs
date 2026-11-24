//! Sliding-window rate limits, one definition for the whole server: the
//! API-wide limit per client address ([`crate::net_access`]) and the webhook
//! limits ([`crate::webhooks`]). Memory is bounded: at most [`MAX_KEYS`]
//! keys; idle ones are dropped, and while the map is full of active keys a
//! new key is refused (never a reset, which would lift every limit).

use std::collections::HashMap;
use std::sync::Mutex;
use std::time::{Duration, Instant};

/// Keys kept at most.
pub const MAX_KEYS: usize = 20_000;

/// Hits per key within a window.
#[derive(Debug)]
pub struct Windows<K> {
    hits: Mutex<HashMap<K, Vec<Instant>>>,
}

impl<K> Default for Windows<K> {
    fn default() -> Self {
        Self { hits: Mutex::new(HashMap::new()) }
    }
}

impl<K: std::hash::Hash + Eq + Clone> Windows<K> {
    /// Counts a hit for `key`; `false` when it is over `limit` in `window`.
    /// A poisoned lock refuses (fail closed).
    pub fn allow(&self, key: &K, limit: usize, window: Duration) -> bool {
        let Ok(mut hits) = self.hits.lock() else { return false };
        let now = Instant::now();
        if hits.len() >= MAX_KEYS && !hits.contains_key(key) {
            hits.retain(|_, v| v.last().is_some_and(|t| now.duration_since(*t) < window));
            if hits.len() >= MAX_KEYS {
                // Full of active keys (a flood from many addresses): refuse
                // newcomers until windows expire; known keys keep working.
                return false;
            }
        }
        let v = hits.entry(key.clone()).or_default();
        v.retain(|t| now.duration_since(*t) < window);
        if v.len() >= limit {
            return false;
        }
        v.push(now);
        true
    }
}

/// The key a client address is limited by: an IPv6 /64 (one host usually
/// owns a whole /64, so per-address keys would not limit it) or the IPv4
/// address.
#[must_use]
pub fn client_key(ip: std::net::IpAddr) -> std::net::IpAddr {
    match ip {
        std::net::IpAddr::V6(v) => {
            let s = v.segments();
            std::net::IpAddr::V6(std::net::Ipv6Addr::new(s[0], s[1], s[2], s[3], 0, 0, 0, 0))
        }
        v4 => v4,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn windows_limit_per_key() {
        let w: Windows<u32> = Windows::default();
        let win = Duration::from_secs(60);
        assert!(w.allow(&1, 2, win));
        assert!(w.allow(&1, 2, win));
        assert!(!w.allow(&1, 2, win));
        assert!(w.allow(&2, 2, win), "keys are counted apart");
    }

    #[test]
    fn ipv6_is_limited_per_64() {
        let a: std::net::IpAddr = "2001:db8:1:2::1".parse().unwrap_or_else(|e| panic!("{e}"));
        let b: std::net::IpAddr = "2001:db8:1:2:ffff::9".parse().unwrap_or_else(|e| panic!("{e}"));
        assert_eq!(client_key(a), client_key(b));
        let v4: std::net::IpAddr = "192.0.2.1".parse().unwrap_or_else(|e| panic!("{e}"));
        assert_eq!(client_key(v4), v4);
    }
}
