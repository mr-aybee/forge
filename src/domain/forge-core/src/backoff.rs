//! Exponential backoff with full jitter, shared by job retries and agent reconnects.

use std::time::Duration;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Backoff {
    pub base: Duration,
    pub max: Duration,
}

impl Backoff {
    #[must_use]
    pub const fn new(base: Duration, max: Duration) -> Self {
        Self { base, max }
    }

    /// Upper bound for `attempt` (0-based): `min(max, base * 2^attempt)`.
    #[must_use]
    pub fn ceiling(&self, attempt: u32) -> Duration {
        let factor = 1u32.checked_shl(attempt.min(31)).unwrap_or(u32::MAX);
        self.base.checked_mul(factor).map_or(self.max, |d| d.min(self.max))
    }

    /// "Full jitter": uniformly random in `[0, ceiling]`. `unit` must be a
    /// random number in `[0, 1)`, injected so this stays deterministic in tests.
    #[must_use]
    pub fn delay(&self, attempt: u32, unit: f64) -> Duration {
        self.ceiling(attempt).mul_f64(unit.clamp(0.0, 1.0))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn grows_exponentially_and_caps() {
        let b = Backoff::new(Duration::from_millis(100), Duration::from_secs(5));
        assert_eq!(b.ceiling(0), Duration::from_millis(100));
        assert_eq!(b.ceiling(3), Duration::from_millis(800));
        assert_eq!(b.ceiling(10), Duration::from_secs(5));
        assert_eq!(b.ceiling(100), Duration::from_secs(5));
    }

    #[test]
    fn jitter_stays_within_bounds() {
        let b = Backoff::new(Duration::from_millis(100), Duration::from_secs(5));
        assert_eq!(b.delay(2, 0.0), Duration::ZERO);
        assert_eq!(b.delay(2, 0.5), Duration::from_millis(200));
        assert!(b.delay(2, 2.0) <= b.ceiling(2));
    }
}
