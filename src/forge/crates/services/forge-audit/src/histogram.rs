//! Entries over time by outcome, for the console's audit chart.

use forge_core::DomainResult;
use forge_core::model::{AuditFilter, AuditOutcome};
use forge_core::ports::AuditRepository;
use serde::Serialize;

/// Most buckets a histogram may ask for.
pub const MAX_BUCKETS: u32 = 200;

const SEC: i64 = 1_000;
const MIN: i64 = 60 * SEC;
const HOUR: i64 = 60 * MIN;
const DAY: i64 = 24 * HOUR;

/// Bucket widths, so bucket edges fall on round times.
const STEPS: [i64; 22] = [
    SEC,
    5 * SEC,
    10 * SEC,
    15 * SEC,
    30 * SEC,
    MIN,
    2 * MIN,
    5 * MIN,
    10 * MIN,
    15 * MIN,
    30 * MIN,
    HOUR,
    2 * HOUR,
    3 * HOUR,
    6 * HOUR,
    12 * HOUR,
    DAY,
    2 * DAY,
    7 * DAY,
    14 * DAY,
    30 * DAY,
    90 * DAY,
];

/// Window used when the log is empty and no `since` was given.
const EMPTY_WINDOW: i64 = HOUR;

/// Matching entries per time bucket, split by outcome.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct AuditHistogram {
    /// Start of the window, epoch ms: `since`, else the first entry in the log.
    pub from: i64,
    /// End of the window, epoch ms (exclusive): `until`, else now.
    pub to: i64,
    /// Width of every bucket.
    pub step_ms: i64,
    /// Every bucket of the window, oldest first, including empty ones.
    pub buckets: Vec<AuditHistogramBucket>,
}

/// One bucket: entries at `t` up to `t + step_ms`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct AuditHistogramBucket {
    pub t: i64,
    pub success: i64,
    pub denied: i64,
    pub failure: i64,
}

/// The smallest round width that splits `span` into at most `buckets` buckets.
fn step_for(span: i64, buckets: u32) -> i64 {
    let n = i64::from(buckets.clamp(1, MAX_BUCKETS));
    let raw = (span + n - 1) / n;

    STEPS.iter().copied().find(|s| *s >= raw).unwrap_or_else(|| ((raw + DAY - 1) / DAY) * DAY)
}

/// Counts `filter` matches per bucket. `now` ends the window when the filter
/// has no `until`. Read-only, so it is safe to call any number of times.
pub(crate) async fn build(
    repo: &dyn AuditRepository,
    filter: &AuditFilter,
    buckets: u32,
    now: i64,
) -> DomainResult<AuditHistogram> {
    let to = filter.until.unwrap_or(now);

    let from = match filter.since {
        Some(s) => s,
        None => repo.entries_from(0, 1).await?.first().map_or(to - EMPTY_WINDOW, |e| e.at.0),
    };

    let to = to.max(from + 1);
    let step_ms = step_for(to - from, buckets);
    let start = from - from.rem_euclid(step_ms);
    let len = usize::try_from((to - start + step_ms - 1) / step_ms).unwrap_or(1).clamp(1, MAX_BUCKETS as usize + 1);
    let mut out: Vec<AuditHistogramBucket> = (0..len)
        .map(|i| AuditHistogramBucket { t: start + step_ms * i as i64, success: 0, denied: 0, failure: 0 })
        .collect();
    let f = AuditFilter { before: None, ..filter.clone() };

    for c in repo.bucket_counts(&f, start, step_ms).await? {
        // An entry written after `now` was read lands in the last bucket.
        let i = usize::try_from(c.bucket).unwrap_or(0).min(len - 1);
        let b = &mut out[i];

        match c.outcome {
            AuditOutcome::Success => b.success += c.count,
            AuditOutcome::Denied => b.denied += c.count,
            AuditOutcome::Failure => b.failure += c.count,
        }
    }

    Ok(AuditHistogram { from, to, step_ms, buckets: out })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn steps_are_round_and_bounded() {
        assert_eq!(step_for(HOUR, 60), MIN);
        assert_eq!(step_for(HOUR, 48), 2 * MIN);
        assert_eq!(step_for(DAY, 48), 30 * MIN);
        assert_eq!(step_for(7 * DAY, 48), 6 * HOUR);
        assert_eq!(step_for(1, 48), SEC);
        assert_eq!(step_for(10 * 365 * DAY, 10), 365 * DAY);
        assert_eq!(step_for(HOUR, 0), HOUR, "zero buckets count as one");
    }
}
