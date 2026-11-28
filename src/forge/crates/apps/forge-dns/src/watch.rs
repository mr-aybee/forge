//! Holds the current map and reloads it when `names.json` changes. The file
//! is polled (mtime and size) instead of watched, to keep the binary small.

use std::path::PathBuf;
use std::sync::{Arc, RwLock};
use std::time::{Duration, SystemTime};

use crate::zone::Zone;

/// How often the file is checked for a change.
pub const POLL_INTERVAL: Duration = Duration::from_secs(2);

/// The current map; `None` until a valid file was read.
#[derive(Debug, Default)]
pub struct Current {
    zone: RwLock<Option<Arc<Zone>>>,
}

impl Current {
    /// The map in use. A poisoned lock still holds a whole map: swaps
    /// replace the `Arc` in one assignment.
    #[must_use]
    pub fn get(&self) -> Option<Arc<Zone>> {
        match self.zone.read() {
            Ok(g) => g.clone(),
            Err(p) => p.into_inner().clone(),
        }
    }

    /// Replaces the map in one step.
    pub fn set(&self, zone: Zone) {
        let zone = Some(Arc::new(zone));

        match self.zone.write() {
            Ok(mut g) => *g = zone,
            Err(p) => *p.into_inner() = zone,
        }
    }
}

/// What identifies one version of the file. A platform without mtimes
/// falls back to the size alone (`None`), still a usable change signal.
type Stamp = (Option<SystemTime>, u64);

/// The file as last seen.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Seen {
    Never,
    Missing,
    At(Stamp),
}

/// Polls one file and swaps the map on change.
#[derive(Debug)]
pub struct Watcher {
    pub path: PathBuf,
    pub ttl: Option<u32>,
    pub current: Arc<Current>,
    pub interval: Duration,
}

impl Watcher {
    /// Loads the file now, then checks it every `interval` forever.
    pub async fn run(self) {
        let mut seen = Seen::Never;
        let mut ticker = tokio::time::interval(self.interval);
        ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);

        loop {
            ticker.tick().await;
            seen = self.check(seen).await;
        }
    }

    async fn check(&self, seen: Seen) -> Seen {
        let now = match tokio::fs::metadata(&self.path).await {
            Ok(m) => Seen::At((m.modified().ok(), m.len())),
            Err(e) => {
                // Log once per disappearance; the old map stays in use.
                if seen != Seen::Missing {
                    tracing::error!(path = %self.path.display(), error = %e, "names file unreadable");
                }

                return Seen::Missing;
            }
        };

        if now != seen {
            self.reload().await;
        }

        now
    }

    async fn reload(&self) {
        let bytes = match tokio::fs::read(&self.path).await {
            Ok(b) => b,
            Err(e) => {
                tracing::error!(path = %self.path.display(), error = %e, "reading names file");
                return;
            }
        };

        match Zone::parse(&bytes, self.ttl) {
            Ok(zone) => {
                self.current.set(zone);
                tracing::info!(path = %self.path.display(), "names loaded");
            }
            Err(e) => {
                let kept =
                    if self.current.get().is_some() { "keeping the previous names" } else { "refusing the zones" };
                tracing::error!(path = %self.path.display(), error = %e, "invalid names file, {kept}");
            }
        }
    }
}
