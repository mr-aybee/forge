//! Time abstraction so that leases, expiry and replay windows are testable.

use serde::{Deserialize, Serialize};
use std::sync::atomic::{AtomicI64, Ordering};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

/// `expires_at` of something that never expires (9999-12-31T23:59:59.999Z).
/// A real timestamp, so every expiry comparison keeps working; small enough
/// for every `BIGINT` column, for JavaScript numbers and for saturating
/// deadline arithmetic. Used by SFTP grants, tunnels and tunnel grants, each
/// behind its own `allow_never_expiry` setting, and by bucket grants, which
/// have no end unless one is given (D-108).
pub const NEVER_EXPIRES: Timestamp = Timestamp(253_402_300_799_999);

/// Milliseconds since the Unix epoch (UTC). Stored as `BIGINT` everywhere.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct Timestamp(pub i64);

impl Timestamp {
    #[must_use]
    pub fn millis(self) -> i64 {
        self.0
    }

    #[must_use]
    pub fn plus(self, d: Duration) -> Self {
        Self(self.0.saturating_add(i64::try_from(d.as_millis()).unwrap_or(i64::MAX)))
    }

    #[must_use]
    pub fn minus(self, d: Duration) -> Self {
        Self(self.0.saturating_sub(i64::try_from(d.as_millis()).unwrap_or(i64::MAX)))
    }
}

/// Source of the current time.
pub trait Clock: Send + Sync + std::fmt::Debug {
    fn now(&self) -> Timestamp;
}

/// Wall-clock implementation.
#[derive(Debug, Default, Clone, Copy)]
pub struct SystemClock;

impl Clock for SystemClock {
    fn now(&self) -> Timestamp {
        let ms = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |d| i64::try_from(d.as_millis()).unwrap_or(i64::MAX));
        Timestamp(ms)
    }
}

/// Manually advanced clock for tests.
#[derive(Debug, Default)]
pub struct ManualClock(AtomicI64);

impl ManualClock {
    #[must_use]
    pub fn new(start: Timestamp) -> Self {
        Self(AtomicI64::new(start.0))
    }

    pub fn advance(&self, d: Duration) {
        self.0.fetch_add(i64::try_from(d.as_millis()).unwrap_or(i64::MAX), Ordering::SeqCst);
    }

    pub fn set(&self, t: Timestamp) {
        self.0.store(t.0, Ordering::SeqCst);
    }
}

impl Clock for ManualClock {
    fn now(&self) -> Timestamp {
        Timestamp(self.0.load(Ordering::SeqCst))
    }
}
