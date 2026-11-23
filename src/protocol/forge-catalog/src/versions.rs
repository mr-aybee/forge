//! Package build strings (`17.6-1.pgdg24.04+1`, `2:7.0.15-1`, `1.26.2-1.el9`):
//! validation, ordering and how big a step an update is.
//!
//! Ordering follows the Debian algorithm (epoch, then upstream version, then
//! revision; letters before digits, `~` before everything). It also orders
//! RPM `version-release` strings and plain semver correctly for the builds
//! package repositories publish.

use std::cmp::Ordering;

use serde::{Deserialize, Serialize};

/// A build string Forge accepts from users and nodes.
#[must_use]
pub fn valid_build(b: &str) -> bool {
    !b.is_empty()
        && b.len() <= 64
        && b.as_bytes()[0].is_ascii_alphanumeric()
        && b.bytes().all(|c| c.is_ascii_alphanumeric() || matches!(c, b'.' | b'+' | b'~' | b':' | b'_' | b'-'))
}

fn split(v: &str) -> (u64, &str, &str) {
    let (epoch, rest) = match v.split_once(':') {
        Some((e, r)) if e.bytes().all(|b| b.is_ascii_digit()) => (e.parse().unwrap_or(0), r),
        _ => (0, v),
    };
    match rest.rsplit_once('-') {
        Some((u, r)) => (epoch, u, r),
        None => (epoch, rest, ""),
    }
}

fn order(c: u8) -> i32 {
    match c {
        b'~' => -1,
        c if c.is_ascii_alphabetic() => i32::from(c),
        0 => 0,
        c => i32::from(c) + 256,
    }
}

/// Debian's `verrevcmp` on one part.
fn cmp_part(a: &str, b: &str) -> Ordering {
    let (a, b) = (a.as_bytes(), b.as_bytes());
    let (mut i, mut j) = (0, 0);
    while i < a.len() || j < b.len() {
        // Non-digit prefix.
        while (i < a.len() && !a[i].is_ascii_digit()) || (j < b.len() && !b[j].is_ascii_digit()) {
            let ac = if i < a.len() && !a[i].is_ascii_digit() { order(a[i]) } else { 0 };
            let bc = if j < b.len() && !b[j].is_ascii_digit() { order(b[j]) } else { 0 };
            if ac != bc {
                return ac.cmp(&bc);
            }
            if i < a.len() && !a[i].is_ascii_digit() {
                i += 1;
            }
            if j < b.len() && !b[j].is_ascii_digit() {
                j += 1;
            }
        }
        // Numeric run.
        while i < a.len() && a[i] == b'0' {
            i += 1;
        }
        while j < b.len() && b[j] == b'0' {
            j += 1;
        }
        let (si, sj) = (i, j);
        while i < a.len() && a[i].is_ascii_digit() {
            i += 1;
        }
        while j < b.len() && b[j].is_ascii_digit() {
            j += 1;
        }
        let (na, nb) = (&a[si..i], &b[sj..j]);
        match na.len().cmp(&nb.len()).then_with(|| na.cmp(nb)) {
            Ordering::Equal => {}
            o => return o,
        }
    }
    Ordering::Equal
}

/// Orders two builds.
#[must_use]
pub fn compare(a: &str, b: &str) -> Ordering {
    let (ea, ua, ra) = split(a);
    let (eb, ub, rb) = split(b);
    ea.cmp(&eb).then_with(|| cmp_part(ua, ub)).then_with(|| cmp_part(ra, rb))
}

/// The leading numeric components of the upstream version: `2:17.6-1` → [17, 6].
#[must_use]
pub fn numbers(build: &str) -> Vec<u64> {
    let (_, upstream, _) = split(build);
    upstream.split(|c: char| !c.is_ascii_digit()).take_while(|s| !s.is_empty()).filter_map(|s| s.parse().ok()).collect()
}

/// How big an update is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum UpdateKind {
    /// Same upstream version, new package revision (security backports).
    Revision,
    Patch,
    Minor,
    Major,
}

/// Classifies `from` → `to`; `None` when `to` is not newer.
#[must_use]
pub fn classify(from: &str, to: &str) -> Option<UpdateKind> {
    if compare(to, from) != Ordering::Greater {
        return None;
    }
    let (a, b) = (numbers(from), numbers(to));
    let at = |v: &[u64], i: usize| v.get(i).copied().unwrap_or(0);
    Some(if at(&a, 0) != at(&b, 0) {
        UpdateKind::Major
    } else if at(&a, 1) != at(&b, 1) {
        // PostgreSQL 10+ and similar use two components: 17.6 → 17.7 is a patch.
        if a.len() == 2 && b.len() == 2 { UpdateKind::Patch } else { UpdateKind::Minor }
    } else if a != b {
        UpdateKind::Patch
    } else {
        UpdateKind::Revision
    })
}

/// What the node reports for an installation (`versions` action).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct NodeVersions {
    /// `apt`, `dnf`, `yum`, `winget` or `manual`.
    pub manager: String,
    #[serde(default)]
    pub package: String,
    #[serde(default)]
    pub installed: String,
    #[serde(default)]
    pub candidate: String,
    #[serde(default)]
    pub running: String,
}

impl NodeVersions {
    /// Parses the last line of the recipe output.
    #[must_use]
    pub fn parse(output: &str) -> Option<Self> {
        let line = output.lines().rev().find(|l| l.trim_start().starts_with('{'))?;
        let v: Self = serde_json::from_str(line.trim()).ok()?;
        let ok = |s: &str| s.is_empty() || valid_build(s) || s.len() <= 120;
        (ok(&v.installed) && ok(&v.candidate)).then_some(v)
    }

    /// The update the node's repositories offer, if any.
    #[must_use]
    pub fn update(&self) -> Option<UpdateKind> {
        if self.installed.is_empty() || self.candidate.is_empty() {
            return None;
        }
        classify(&self.installed, &self.candidate)
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    #[test]
    fn orders_like_dpkg() {
        assert_eq!(compare("17.6-1.pgdg24.04+1", "17.7-1.pgdg24.04+1"), Ordering::Less);
        assert_eq!(compare("1:1.0", "2.0"), Ordering::Greater);
        assert_eq!(compare("1.0~rc1", "1.0"), Ordering::Less);
        assert_eq!(compare("1.10", "1.9"), Ordering::Greater);
        assert_eq!(compare("7.0.15-1", "7.0.15-1ubuntu0.1"), Ordering::Less);
        assert_eq!(compare("1.26.2-1.el9", "1.26.2-1.el9"), Ordering::Equal);
        assert_eq!(compare("5:7.2.4-1rl1~noble1", "5:7.4.1-1rl1~noble1"), Ordering::Less);
    }

    #[test]
    fn classifies_updates() {
        assert_eq!(classify("17.6-1.pgdg24.04+1", "17.7-1.pgdg24.04+1"), Some(UpdateKind::Patch));
        assert_eq!(classify("7.2.4-1", "7.4.1-1"), Some(UpdateKind::Minor));
        assert_eq!(classify("7.2.4-1", "7.2.5-1"), Some(UpdateKind::Patch));
        assert_eq!(classify("1.24.0-2", "1.24.0-2ubuntu0.1"), Some(UpdateKind::Revision));
        assert_eq!(classify("16.4-1", "17.0-1"), Some(UpdateKind::Major));
        assert_eq!(classify("17.7-1", "17.6-1"), None);
    }

    #[test]
    fn validates_builds() {
        assert!(valid_build("2:17.6-1.pgdg24.04+1"));
        assert!(valid_build("1.0~rc1"));
        assert!(!valid_build("1.0; rm -rf /"));
        assert!(!valid_build("-1"));
        assert!(!valid_build(""));
    }

    #[test]
    fn parses_node_output() {
        let out = "forge: refreshing\n{\"manager\":\"apt\",\"package\":\"postgresql-17\",\"installed\":\"17.6-1\",\"candidate\":\"17.7-1\",\"running\":\"17.6 (Ubuntu 17.6-1)\"}\n";
        let v = NodeVersions::parse(out).unwrap();
        assert_eq!(v.candidate, "17.7-1");
        assert_eq!(v.update(), Some(UpdateKind::Patch));
        assert_eq!(NodeVersions::parse("{\"manager\":\"manual\"}").unwrap().update(), None);
    }
}
