//! Live node telemetry kept in memory.
//!
//! Agents attach a resource sample to each heartbeat. Heartbeats are long
//! polls, so samples normally arrive every ~25 s; while somebody is looking at
//! a node ("watching"), the hub answers that node's heartbeats after a short
//! hold instead, which yields a sample every couple of seconds. History is a
//! bounded ring per node and is lost on restart (by design: this is a live
//! view, not a time-series database).

use std::collections::{HashMap, VecDeque};
use std::sync::Mutex;
use std::time::{Duration, Instant};

use forge_core::{ServerId, Timestamp};
use forge_proto::messages::NodeMetrics;
use serde::Serialize;
use serde_json::Value;

/// Most samples kept per node: one hour at the live rate (2 s).
const MAX_SAMPLES: usize = 1_800;
/// Samples older than this are dropped: the console shows at most one
/// hour, and older history comes from the analytics worker. Keeps memory
/// per node small (about 150 samples at the idle heartbeat rate).
const MAX_AGE_MS: i64 = 3_600_000;
/// How long a node stays "watched" after the last metrics request.
const WATCH_TTL: Duration = Duration::from_secs(20);
/// Heartbeat hold for watched nodes.
pub const LIVE_HOLD: Duration = Duration::from_secs(2);
const MAX_PROBES: usize = 240;

/// A mounted volume as last reported (not kept as history).
#[derive(Debug, Clone, Serialize)]
pub struct Volume {
    pub mount_point: String,
    pub device: String,
    pub file_system: String,
    pub total: u64,
    pub available: u64,
    pub used: u64,
    pub removable: bool,
}

#[must_use]
pub fn volumes_from_wire(m: &NodeMetrics) -> Vec<Volume> {
    m.volumes
        .iter()
        .map(|v| Volume {
            mount_point: v.mount_point.clone(),
            device: v.device.clone(),
            file_system: v.file_system.clone(),
            total: v.total_bytes,
            available: v.available_bytes.min(v.total_bytes),
            used: v.total_bytes.saturating_sub(v.available_bytes),
            removable: v.removable,
        })
        .collect()
}

#[derive(Debug, Clone, Copy, Serialize)]
pub struct Sample {
    /// Epoch ms.
    pub t: i64,
    pub cpu: f32,
    pub mem_used: u64,
    pub mem_total: u64,
    pub swap_used: u64,
    pub swap_total: u64,
    pub disk_used: u64,
    pub disk_total: u64,
    pub rx: u64,
    pub tx: u64,
    pub load1: f32,
    pub uptime: u64,
    pub cpus: u32,
}

impl Sample {
    #[must_use]
    pub fn from_wire(t: Timestamp, m: &NodeMetrics) -> Self {
        Self {
            t: t.0,
            cpu: m.cpu_percent,
            mem_used: m.mem_used_bytes,
            mem_total: m.mem_total_bytes,
            swap_used: m.swap_used_bytes,
            swap_total: m.swap_total_bytes,
            disk_used: m.disk_used_bytes,
            disk_total: m.disk_total_bytes,
            rx: m.net_rx_bytes_per_sec,
            tx: m.net_tx_bytes_per_sec,
            load1: m.load1,
            uptime: m.host_uptime_secs,
            cpus: m.cpu_count,
        }
    }
}

/// One connectivity run from a node: `{t, results: [...]}`.
#[derive(Debug, Clone, Serialize)]
pub struct ProbeRun {
    pub t: i64,
    pub results: Value,
}

#[derive(Debug, Default)]
struct Inner {
    samples: HashMap<ServerId, VecDeque<Sample>>,
    watched: HashMap<ServerId, Instant>,
    volumes: HashMap<ServerId, Vec<Volume>>,
    probes: HashMap<ServerId, VecDeque<ProbeRun>>,
    /// Edge nodes: last reported minute of route traffic (epoch ms, sites).
    traffic: HashMap<ServerId, (i64, Vec<forge_proto::messages::SiteTraffic>)>,
}

#[derive(Debug, Default)]
pub struct Telemetry {
    inner: Mutex<Inner>,
}

impl Telemetry {
    fn with<R>(&self, f: impl FnOnce(&mut Inner) -> R) -> R {
        let mut g = self.inner.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        f(&mut g)
    }

    pub fn record(&self, server: ServerId, sample: Sample) {
        self.with(|i| {
            let q = i.samples.entry(server).or_default();
            while q.len() >= MAX_SAMPLES || q.front().is_some_and(|f| sample.t - f.t > MAX_AGE_MS) {
                q.pop_front();
            }
            q.push_back(sample);
            // A node watched live and then idle keeps a large buffer: give
            // the memory back once it is mostly empty.
            if q.capacity() > 256 && q.len() < q.capacity() / 4 {
                q.shrink_to(q.len() * 2);
            }
        });
    }

    /// Replaces an edge's route traffic (an empty list clears it).
    pub fn set_traffic(&self, server: ServerId, at: i64, sites: Vec<forge_proto::messages::SiteTraffic>) {
        self.with(|i| {
            if sites.is_empty() {
                i.traffic.remove(&server);
            } else {
                i.traffic.insert(server, (at, sites));
            }
        });
    }

    /// Route traffic of every edge reported after `fresh_after` (epoch ms).
    #[must_use]
    pub fn traffic(&self, fresh_after: i64) -> Vec<(ServerId, Vec<forge_proto::messages::SiteTraffic>)> {
        self.with(|i| {
            i.traffic.iter().filter(|(_, (t, _))| *t > fresh_after).map(|(s, (_, v))| (*s, v.clone())).collect()
        })
    }

    pub fn set_volumes(&self, server: ServerId, volumes: Vec<Volume>) {
        self.with(|i| {
            i.volumes.insert(server, volumes);
        });
    }

    #[must_use]
    pub fn volumes(&self, server: ServerId) -> Vec<Volume> {
        self.with(|i| i.volumes.get(&server).cloned().unwrap_or_default())
    }

    /// Samples newer than `since` (epoch ms).
    #[must_use]
    pub fn since(&self, server: ServerId, since: i64) -> Vec<Sample> {
        self.with(|i| {
            i.samples.get(&server).map(|q| q.iter().filter(|s| s.t > since).copied().collect()).unwrap_or_default()
        })
    }

    /// The most recent sample of `server`.
    #[must_use]
    pub fn latest(&self, server: ServerId) -> Option<Sample> {
        self.with(|i| i.samples.get(&server).and_then(|q| q.back().copied()))
    }

    /// Marks `server` as being looked at (its heartbeats speed up).
    pub fn watch(&self, server: ServerId) {
        self.with(|i| {
            i.watched.insert(server, Instant::now());
        });
    }

    #[must_use]
    pub fn is_watched(&self, server: ServerId) -> bool {
        self.with(|i| {
            i.watched.retain(|_, at| at.elapsed() < WATCH_TTL);
            i.watched.contains_key(&server)
        })
    }

    pub fn record_probe(&self, server: ServerId, run: ProbeRun) {
        self.with(|i| {
            let q = i.probes.entry(server).or_default();
            if q.len() >= MAX_PROBES {
                q.pop_front();
            }
            q.push_back(run);
        });
    }

    #[must_use]
    pub fn probes(&self, server: ServerId) -> Vec<ProbeRun> {
        self.with(|i| i.probes.get(&server).map(|q| q.iter().cloned().collect()).unwrap_or_default())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ring_is_bounded_and_filtered() {
        let t = Telemetry::default();
        let id = ServerId::new();
        let m = NodeMetrics::default();
        for n in 0..(MAX_SAMPLES as i64 + 10) {
            t.record(id, Sample::from_wire(Timestamp(n), &m));
        }
        assert_eq!(t.since(id, -1).len(), MAX_SAMPLES);
        assert_eq!(t.since(id, MAX_SAMPLES as i64 + 5).len(), 4);
        // An hour later only the new sample is left, and the buffer shrank.
        t.record(id, Sample::from_wire(Timestamp(MAX_AGE_MS + MAX_SAMPLES as i64 + 20), &m));
        assert_eq!(t.since(id, -1).len(), 1, "older than an hour is dropped");
        assert!(t.with(|i| i.samples[&id].capacity()) < 64, "memory given back");
        assert!(!t.is_watched(id));
        t.watch(id);
        assert!(t.is_watched(id));
    }
}
