//! Log searches (D-128), moved from forge-server (D-141). They run under
//! the runtime limits of D-136: a cap on searches at once (more answer
//! busy), a maximum lookback, and per-query timeout, threads and memory
//! passed to the engine. The query's scope (which services and nodes) is
//! decided by forge-server from the caller's permissions.

use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use forge_analytics_proto::WorkerError;
use forge_analytics_proto::tuning::Tuning;
use forge_core::Timestamp;
use forge_core::analytics::{Histogram, HistogramBucket, MAX_HISTOGRAM_WINDOW_MS};
use forge_core::logs::{LogBucket, LogEvent, LogQuery};
use tokio::sync::watch;

use crate::analytics::Analytics;

const DAY_MS: i64 = 24 * 3_600_000;
const HISTOGRAM_BUCKETS: i64 = 60;

/// Histogram steps people read easily, in ms.
const STEPS: [i64; 14] = [
    1_000,
    5_000,
    10_000,
    30_000,
    60_000,
    300_000,
    600_000,
    1_800_000,
    3_600_000,
    10_800_000,
    21_600_000,
    43_200_000,
    86_400_000,
    604_800_000,
];

/// The smallest readable step that splits `window` into at most 60 buckets.
#[must_use]
pub fn histogram_step(window: i64) -> i64 {
    let want = (window / HISTOGRAM_BUCKETS).max(1);
    STEPS.iter().copied().find(|s| *s >= want).unwrap_or(STEPS[STEPS.len() - 1])
}

/// Folds level counts into one bucket per step, empty steps included.
#[must_use]
pub fn fold_histogram(from: i64, to: i64, step: i64, rows: &[LogBucket]) -> Vec<HistogramBucket> {
    let start = from - from.rem_euclid(step);
    let mut buckets: Vec<HistogramBucket> = (0..)
        .map(|i| start + i * step)
        .take_while(|t| *t < to)
        .map(|t| HistogramBucket { t, counts: HashMap::new() })
        .collect();
    for r in rows {
        if let Ok(i) = usize::try_from((r.t - start).div_euclid(step))
            && let Some(b) = buckets.get_mut(i)
        {
            *b.counts.entry(r.level).or_default() += r.count;
        }
    }
    buckets
}

/// Counts a running search; released on drop.
struct SearchSlot<'a>(&'a AtomicUsize);

impl Drop for SearchSlot<'_> {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::AcqRel);
    }
}

/// The log search service.
#[derive(Debug)]
pub struct Search {
    analytics: Arc<Analytics>,
    tuning: watch::Receiver<Tuning>,
    running: AtomicUsize,
}

impl Search {
    #[must_use]
    pub fn new(analytics: Arc<Analytics>, tuning: watch::Receiver<Tuning>) -> Self {
        Self { analytics, tuning, running: AtomicUsize::new(0) }
    }

    /// Takes a search slot, or `SearchBusy` when all are in use.
    fn slot(&self) -> Result<SearchSlot<'_>, WorkerError> {
        let max = self.tuning.borrow().search_max_concurrent as usize;
        let running = self.running.fetch_add(1, Ordering::AcqRel);
        let slot = SearchSlot(&self.running);
        if running >= max.max(1) {
            return Err(WorkerError::SearchBusy);
        }
        Ok(slot)
    }

    /// `q` with the engine limits and the lookback applied: a window that
    /// starts earlier than the lookback allows is cut to it.
    fn limited(&self, mut q: LogQuery, now: Timestamp) -> LogQuery {
        let t = *self.tuning.borrow();
        q.limits = t.search_limits();
        if t.search_max_lookback_days > 0 {
            let earliest = now.0 - i64::from(t.search_max_lookback_days) * DAY_MS;
            q.from = Some(Timestamp(q.from.map_or(earliest, |f| f.0.max(earliest))));
        }
        q
    }

    /// One page of events.
    pub async fn query(&self, q: LogQuery, now: Timestamp) -> Result<Vec<LogEvent>, WorkerError> {
        let store = self.analytics.usable()?;
        if q.is_empty() {
            return Ok(Vec::new());
        }
        let _slot = self.slot()?;
        let q = self.limited(q, now);
        store.query_logs(&q).await.map_err(|e| WorkerError::Unavailable { reason: e.to_string() })
    }

    /// Counts per level over `q.from..q.to` (default: the last 24 hours).
    pub async fn histogram(&self, q: LogQuery, now: Timestamp) -> Result<Histogram, WorkerError> {
        let store = self.analytics.usable()?;
        let to = q.to.map_or(now.0, |t| t.0);
        let from = q.from.map_or(to - 24 * 3_600_000, |t| t.0);
        if to <= from || to - from > MAX_HISTOGRAM_WINDOW_MS {
            return Err(WorkerError::Invalid { reason: "`from` must be before `to`, at most 31 days apart".into() });
        }
        let _slot = self.slot()?;
        let mut q = self.limited(q, now);
        // The lookback may move the start later.
        let from = q.from.map_or(from, |f| f.0.max(from)).min(to - 1);
        let step = histogram_step(to - from);
        q.from = Some(Timestamp(from - from.rem_euclid(step)));
        q.to = Some(Timestamp(to));
        let rows = if q.is_empty() {
            Vec::new()
        } else {
            store.log_histogram(&q, step).await.map_err(|e| WorkerError::Unavailable { reason: e.to_string() })?
        };
        Ok(Histogram { from, to, step_ms: step, buckets: fold_histogram(from, to, step, &rows) })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use forge_core::logs::LogLevel;

    #[test]
    fn picks_readable_steps() {
        assert_eq!(histogram_step(3_600_000), 60_000);
        assert_eq!(histogram_step(24 * 3_600_000), 1_800_000);
        assert_eq!(histogram_step(60_000), 1_000);
        assert_eq!(histogram_step(i64::MAX / 2), 604_800_000);
    }

    #[test]
    fn folds_buckets() {
        let rows = vec![
            LogBucket { t: 60_000, level: LogLevel::Error, count: 2 },
            LogBucket { t: 60_000, level: LogLevel::Information, count: 5 },
            LogBucket { t: 180_000, level: LogLevel::Error, count: 1 },
            LogBucket { t: 9_999_999, level: LogLevel::Error, count: 1 },
        ];
        let b = fold_histogram(30_000, 240_000, 60_000, &rows);
        assert_eq!(b.len(), 4);
        assert_eq!(b[0].t, 0);
        assert!(b[0].counts.is_empty());
        assert_eq!(b[1].counts[&LogLevel::Information], 5);
        assert_eq!(b[3].counts[&LogLevel::Error], 1);
    }
}
