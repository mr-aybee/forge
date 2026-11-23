//! Node measurements as observations (#11 slice 2, D-085 amendment, D-118).
//!
//! Every heartbeat carries the node's telemetry. These numbers from it become
//! observations about the node itself, recorded through
//! `ObservationRecorder::record_from_agent` (so the reporting rule and the
//! executor apply as for any agent report): `node.cpu.percent`,
//! `node.mem.used_percent`, `node.disk.used_percent` (root filesystem),
//! `node.disk.max_used_percent` (the fullest local volume, its mount in the
//! detail), `node.swap.used_percent`, `node.load.per_cpu` and
//! `node.uptime.seconds`. Alert rules read them like any other observation;
//! `forge_alerts::catalog` describes them.

use forge_core::observation::{Observation, ObservationKey, ObservationSource, ObservedValue, Subject, Unit};
use forge_core::{EntityKind, EntityRef, ServerId, Timestamp};
use forge_proto::messages::{DiskVolume, NodeMetrics};

/// At most one measurement batch per node this often: the agent chooses its
/// heartbeat pace (review of #34).
pub const MIN_INTERVAL_MS: i64 = 10_000;

fn last_recorded() -> &'static std::sync::Mutex<std::collections::HashMap<ServerId, i64>> {
    static M: std::sync::OnceLock<std::sync::Mutex<std::collections::HashMap<ServerId, i64>>> =
        std::sync::OnceLock::new();
    M.get_or_init(|| std::sync::Mutex::new(std::collections::HashMap::new()))
}

/// Whether this node's measurements may be recorded at `now`; marks them
/// recorded when so.
#[must_use]
pub fn due(server: ServerId, now: Timestamp) -> bool {
    let Ok(mut m) = last_recorded().lock() else { return false };
    if m.get(&server).is_some_and(|t| now.0 - t < MIN_INTERVAL_MS) {
        return false;
    }
    m.insert(server, now.0);
    true
}

/// Thousandths of a percent, or `None` when the total is unknown.
fn percent_milli(used: u64, total: u64) -> Option<i64> {
    if total == 0 {
        return None;
    }
    let p = u128::from(used.min(total)) * 100_000 / u128::from(total);
    i64::try_from(p).ok()
}

/// The root filesystem: `/` on Linux, `C:\` on Windows.
fn root_volume(m: &NodeMetrics) -> Option<(u64, u64)> {
    m.volumes
        .iter()
        .find(|v| v.mount_point == "/" || v.mount_point.eq_ignore_ascii_case("C:\\"))
        .map(|v| (v.total_bytes.saturating_sub(v.available_bytes), v.total_bytes))
}

/// Filesystems that are not local storage: memory-backed, container layers,
/// read-only images and kernel views. They are often 100 % full by design.
const VIRTUAL_FILESYSTEMS: &[&str] = &[
    "tmpfs",
    "devtmpfs",
    "ramfs",
    "overlay",
    "overlayfs",
    "squashfs",
    "iso9660",
    "udf",
    "proc",
    "sysfs",
    "cgroup",
    "cgroup2",
    "devpts",
    "efivarfs",
    "nsfs",
    "autofs",
    "fuse.lxcfs",
];

/// Whether a volume counts for `node.disk.max_used_percent`.
fn is_local_storage(v: &DiskVolume) -> bool {
    let fs = v.file_system.to_ascii_lowercase();
    !v.removable && v.total_bytes > 0 && !VIRTUAL_FILESYSTEMS.contains(&fs.as_str())
}

/// The fullest local volume: its mount point and use in thousandths of a
/// percent. `None` when no volume qualifies.
fn fullest_volume(m: &NodeMetrics) -> Option<(String, i64)> {
    m.volumes
        .iter()
        .filter(|v| is_local_storage(v))
        .filter_map(|v| {
            percent_milli(v.total_bytes.saturating_sub(v.available_bytes), v.total_bytes)
                .map(|p| (v.mount_point.chars().take(256).collect::<String>(), p))
        })
        .max_by_key(|(_, p)| *p)
}

/// Swap in use. A node without swap uses none of it: `0`, not unknown, so a
/// rule on it has evidence.
fn swap_milli(m: &NodeMetrics) -> Option<i64> {
    if m.swap_total_bytes == 0 { Some(0) } else { percent_milli(m.swap_used_bytes, m.swap_total_bytes) }
}

/// One-minute load per CPU, in thousandths. Unknown when the agent does not
/// report its CPU count (older agents, Windows).
fn load_per_cpu_milli(m: &NodeMetrics) -> Option<i64> {
    if m.cpu_count == 0 || !m.load1.is_finite() || m.load1 < 0.0 {
        return None;
    }
    #[allow(clippy::cast_possible_truncation)] // reason: a load is far below i64::MAX
    let v = (f64::from(m.load1) / f64::from(m.cpu_count) * 1000.0).round() as i64;
    Some(v)
}

/// Uptime in thousandths of a second. `0` means "not reported".
fn uptime_milli(m: &NodeMetrics) -> Option<i64> {
    (m.host_uptime_secs > 0).then(|| i64::try_from(m.host_uptime_secs).unwrap_or(i64::MAX / 1000).saturating_mul(1000))
}

/// The node's measurements from one heartbeat. `fresh_ms`: how long they
/// count (three heartbeat holds).
#[must_use]
pub fn from_metrics(server: ServerId, m: &NodeMetrics, now: Timestamp, fresh_ms: u64) -> Vec<Observation> {
    let cpu = m.cpu_percent.is_finite().then(|| {
        #[allow(clippy::cast_possible_truncation)]
        let v = (f64::from(m.cpu_percent.clamp(0.0, 100.0)) * 1000.0).round() as i64;
        v
    });
    let disk = root_volume(m).and_then(|(used, total)| percent_milli(used, total));
    let fullest = fullest_volume(m);
    let values = [
        ("node.cpu.percent", cpu, Unit::Percent, None),
        ("node.mem.used_percent", percent_milli(m.mem_used_bytes, m.mem_total_bytes), Unit::Percent, None),
        ("node.disk.used_percent", disk, Unit::Percent, None),
        (
            "node.disk.max_used_percent",
            fullest.as_ref().map(|(_, p)| *p),
            Unit::Percent,
            fullest.as_ref().map(|(mount, _)| mount.clone()),
        ),
        ("node.swap.used_percent", swap_milli(m), Unit::Percent, None),
        ("node.load.per_cpu", load_per_cpu_milli(m), Unit::Ratio, None),
        ("node.uptime.seconds", uptime_milli(m), Unit::Seconds, None),
    ];
    values
        .into_iter()
        .filter_map(|(subject, milli, unit, detail)| {
            Some(Observation {
                key: ObservationKey {
                    resource: EntityRef::new(EntityKind::Server, server),
                    server: None,
                    subject: Subject::new(subject).ok()?,
                },
                value: ObservedValue::Measure { milli, unit },
                reason: None,
                detail,
                source: ObservationSource::Agent,
                executor: None,
                generation: None,
                observed_at: now,
                fresh_for_ms: fresh_ms,
            })
        })
        .collect()
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use forge_proto::messages::DiskVolume;

    use super::*;

    #[test]
    fn a_node_records_at_most_every_ten_seconds() {
        let s = ServerId::new();
        assert!(due(s, Timestamp(100_000)));
        assert!(!due(s, Timestamp(102_000)), "a fast heartbeat is skipped");
        assert!(!due(s, Timestamp(109_999)));
        assert!(due(s, Timestamp(110_000)));
        assert!(due(ServerId::new(), Timestamp(102_000)), "per node");
    }

    #[test]
    fn telemetry_becomes_node_measures() {
        let m = NodeMetrics {
            cpu_percent: 85.25,
            mem_total_bytes: 8_000,
            mem_used_bytes: 2_000,
            swap_total_bytes: 1_000,
            swap_used_bytes: 600,
            load1: 3.0,
            cpu_count: 2,
            host_uptime_secs: 300,
            volumes: vec![
                DiskVolume { mount_point: "/data".into(), total_bytes: 100, available_bytes: 10, ..Default::default() },
                DiskVolume { mount_point: "/".into(), total_bytes: 1_000, available_bytes: 250, ..Default::default() },
            ],
            ..Default::default()
        };
        let o = from_metrics(ServerId::new(), &m, Timestamp(5), 75_000);
        let v: Vec<(String, ObservedValue)> = o.iter().map(|o| (o.key.subject.as_str().to_owned(), o.value)).collect();
        let p = |milli| ObservedValue::Measure { milli, unit: Unit::Percent };
        assert_eq!(
            v,
            vec![
                ("node.cpu.percent".into(), p(Some(85_250))),
                ("node.mem.used_percent".into(), p(Some(25_000))),
                ("node.disk.used_percent".into(), p(Some(75_000))),
                ("node.disk.max_used_percent".into(), p(Some(90_000))),
                ("node.swap.used_percent".into(), p(Some(60_000))),
                ("node.load.per_cpu".into(), ObservedValue::Measure { milli: Some(1_500), unit: Unit::Ratio }),
                ("node.uptime.seconds".into(), ObservedValue::Measure { milli: Some(300_000), unit: Unit::Seconds }),
            ],
            "root disk is `/`, the fullest volume is /data"
        );
        let max = o.iter().find(|o| o.key.subject.as_str() == "node.disk.max_used_percent").unwrap();
        assert_eq!(max.detail.as_deref(), Some("/data"), "the mount is in the detail");
        assert!(o.iter().all(|o| o.fresh_for_ms == 75_000 && o.observed_at == Timestamp(5)));
        // Unknown totals give unknown values, never zero; a node without
        // swap uses none.
        let empty = from_metrics(
            ServerId::new(),
            &NodeMetrics { cpu_percent: f32::NAN, ..Default::default() },
            Timestamp(5),
            1,
        );
        for o in &empty {
            if o.key.subject.as_str() == "node.swap.used_percent" {
                assert_eq!(o.value, p(Some(0)));
            } else {
                assert!(o.value.is_unknown(), "{}", o.key.subject);
            }
        }
    }

    #[test]
    fn the_fullest_volume_skips_removable_and_virtual_filesystems() {
        let vol = |mount: &str, fs: &str, avail: u64, removable: bool| DiskVolume {
            mount_point: mount.into(),
            file_system: fs.into(),
            total_bytes: 100,
            available_bytes: avail,
            removable,
            ..Default::default()
        };
        let m = NodeMetrics {
            volumes: vec![
                vol("/", "ext4", 50, false),
                vol("/run", "tmpfs", 0, false),
                vol("/snap/core", "squashfs", 0, false),
                vol("/var/lib/docker/overlay2/x", "overlay", 0, false),
                vol("/media/usb", "vfat", 0, true),
                vol("/srv", "xfs", 20, false),
            ],
            ..Default::default()
        };
        assert_eq!(fullest_volume(&m), Some(("/srv".into(), 80_000)));
        assert_eq!(fullest_volume(&NodeMetrics::default()), None);
    }
}
