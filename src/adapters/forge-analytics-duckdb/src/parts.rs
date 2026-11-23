//! Day files (D-143): which file holds which rows, their names, and the
//! dates they cover.
//!
//! Rows go to the file of their *ingest* day (the worker's clock when the
//! batch is written), so one batch writes exactly one file per family and
//! can commit its rows with the spool file id in one transaction. Queries
//! filter on event time, so the time range each file holds is kept in
//! `meta.duckdb` and only the files whose range overlaps the window are
//! opened.

use std::path::{Path, PathBuf};

pub const DAY_MS: i64 = 86_400_000;
pub const HOUR_MS: i64 = 3_600_000;

/// A group of tables kept in their own files, with one retention.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Family {
    /// `app_logs`; `app_logs_days`.
    Logs,
    /// `node_minute`, `container_minute`; `node_metrics_days`.
    Metrics,
    /// `top_process`; `top_processes_days`.
    Processes,
    /// `node_1h`, `container_1h`, one file per UTC month; `node_rollup_days`.
    Rollups,
}

impl Family {
    pub const ALL: [Family; 4] = [Family::Logs, Family::Metrics, Family::Processes, Family::Rollups];
    /// Families written from spool files, each with its own file-id table.
    pub const SPOOL: [Family; 3] = [Family::Logs, Family::Metrics, Family::Processes];

    #[must_use]
    pub const fn dir(self) -> &'static str {
        match self {
            Family::Logs => "logs",
            Family::Metrics => "metrics",
            Family::Processes => "processes",
            Family::Rollups => "rollups",
        }
    }

    #[must_use]
    pub const fn monthly(self) -> bool {
        matches!(self, Family::Rollups)
    }

    #[must_use]
    pub fn parse(s: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|f| f.dir() == s)
    }
}

/// One file: a family and a day (days since the epoch) or, for monthly
/// families, a month (`year * 12 + month - 1`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct Part {
    pub family: Family,
    pub index: i64,
}

impl Part {
    /// The part holding rows written at `ms`.
    #[must_use]
    pub fn at(family: Family, ms: i64) -> Self {
        let day = ms.div_euclid(DAY_MS);
        let index = if family.monthly() {
            let (y, m, _) = civil(day);
            y * 12 + i64::from(m) - 1
        } else {
            day
        };
        Self { family, index }
    }

    /// The catalog name it is attached as (generated, never input).
    #[must_use]
    pub fn alias(&self) -> String {
        format!("{}_{}", self.family.dir(), self.index.max(0))
    }

    /// `2026-11-22.duckdb`, or `2026-11.duckdb` for a monthly family.
    #[must_use]
    pub fn file_name(&self) -> String {
        if self.family.monthly() {
            let (y, m) = (self.index.div_euclid(12), self.index.rem_euclid(12) + 1);
            format!("{y:04}-{m:02}.duckdb")
        } else {
            let (y, m, d) = civil(self.index);
            format!("{y:04}-{m:02}-{d:02}.duckdb")
        }
    }

    #[must_use]
    pub fn path(&self, root: &Path) -> PathBuf {
        root.join(self.family.dir()).join(self.file_name())
    }

    /// Reads a file name back.
    #[must_use]
    pub fn parse(family: Family, name: &str) -> Option<Self> {
        let stem = name.strip_suffix(".duckdb")?;
        let nums: Vec<i64> = stem.split('-').map(str::parse).collect::<Result<_, _>>().ok()?;
        let index = match (family.monthly(), nums.as_slice()) {
            (true, [y, m]) if (1..=12).contains(m) => y * 12 + m - 1,
            (false, [y, m, d]) if (1..=12).contains(m) && (1..=31).contains(d) => {
                days_from_civil(*y, u32::try_from(*m).ok()?, u32::try_from(*d).ok()?)
            }
            _ => return None,
        };
        let p = Self { family, index };
        (p.file_name() == name).then_some(p)
    }

    /// Start of the day or month, epoch ms.
    #[must_use]
    pub fn start_ms(&self) -> i64 {
        if self.family.monthly() {
            let (y, m) = (self.index.div_euclid(12), self.index.rem_euclid(12) + 1);
            days_from_civil(y, u32::try_from(m).unwrap_or(1), 1) * DAY_MS
        } else {
            self.index * DAY_MS
        }
    }

    /// End (exclusive) of the day or month, epoch ms.
    #[must_use]
    pub fn end_ms(&self) -> i64 {
        Self { family: self.family, index: self.index + 1 }.start_ms()
    }
}

/// `(year, month, day)` of a day number (days since 1970-01-01).
#[must_use]
pub fn civil(days: i64) -> (i64, u32, u32) {
    // Howard Hinnant's civil_from_days.
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if m <= 2 { y + 1 } else { y };
    (y, u32::try_from(m).unwrap_or(1), u32::try_from(d).unwrap_or(1))
}

/// Day number of a civil date.
#[must_use]
pub fn days_from_civil(y: i64, m: u32, d: u32) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = y.div_euclid(400);
    let yoe = y.rem_euclid(400);
    let (m, d) = (i64::from(m), i64::from(d));
    let doy = (153 * (if m > 2 { m - 3 } else { m + 9 }) + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

#[cfg(test)]
#[allow(clippy::unwrap_used)] // reason: tests
mod tests {
    use super::*;

    #[test]
    fn names_round_trip() {
        // 2026-11-22T10:00:00Z
        let ms = 1_795_341_600_000;
        let p = Part::at(Family::Logs, ms);
        assert_eq!(p.file_name(), "2026-11-22.duckdb");
        assert_eq!(Part::parse(Family::Logs, "2026-11-22.duckdb"), Some(p));
        assert!(p.start_ms() <= ms && ms < p.end_ms());
        assert_eq!(p.end_ms() - p.start_ms(), DAY_MS);
        let r = Part::at(Family::Rollups, ms);
        assert_eq!(r.file_name(), "2026-11.duckdb");
        assert_eq!(Part::parse(Family::Rollups, "2026-11.duckdb"), Some(r));
        assert!(r.start_ms() <= ms && ms < r.end_ms());
        assert_eq!(Part { family: Family::Rollups, index: r.index + 1 }.file_name(), "2026-12.duckdb");
        assert_eq!(Part { family: Family::Rollups, index: r.index + 2 }.file_name(), "2027-01.duckdb");
        assert!(Part::parse(Family::Logs, "2026-13-01.duckdb").is_none());
        assert!(Part::parse(Family::Logs, "x.duckdb").is_none());
        assert_eq!(civil(0), (1970, 1, 1));
        assert_eq!(days_from_civil(2000, 2, 29), 11_016);
        assert_eq!(p.alias(), format!("logs_{}", p.index));
    }
}
