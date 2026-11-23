//! Host resource sampling, attached to every heartbeat.

use std::collections::HashSet;
use std::time::Instant;

use forge_proto::messages::{DiskVolume, NodeMetrics};
use sysinfo::{CpuRefreshKind, Disks, MemoryRefreshKind, Networks, RefreshKind, System};

/// Keeps the previous readings so CPU and network figures are averaged over
/// the interval between two heartbeats.
#[derive(Debug)]
pub struct Sampler {
    sys: System,
    disks: Disks,
    nets: Networks,
    last: Instant,
    /// When the top-process list was last attached (once a minute).
    last_top: Option<Instant>,
}

/// How often the heartbeat carries the top-process list.
const TOP_EVERY: std::time::Duration = std::time::Duration::from_secs(60);
/// Processes per list (by CPU and by memory).
const TOP_N: usize = 10;

impl Default for Sampler {
    fn default() -> Self {
        Self::new()
    }
}

impl Sampler {
    #[must_use]
    pub fn new() -> Self {
        let sys = System::new_with_specifics(
            RefreshKind::nothing()
                .with_cpu(CpuRefreshKind::nothing().with_cpu_usage())
                .with_memory(MemoryRefreshKind::everything()),
        );
        Self {
            sys,
            disks: Disks::new_with_refreshed_list(),
            nets: Networks::new_with_refreshed_list(),
            last: Instant::now(),
            last_top: None,
        }
    }

    pub fn sample(&mut self) -> NodeMetrics {
        let elapsed = self.last.elapsed().as_secs_f64().max(0.001);
        self.last = Instant::now();
        self.sys.refresh_cpu_usage();
        self.sys.refresh_memory();
        self.disks.refresh(true);
        self.nets.refresh(true);

        // Count each device once (bind mounts and overlays repeat them).
        let mut seen = HashSet::new();
        let mut mounts = HashSet::new();
        let (mut disk_total, mut disk_avail) = (0u64, 0u64);
        let mut volumes = Vec::new();
        for d in self.disks.list() {
            let fs = d.file_system().to_string_lossy().into_owned();
            let mount = d.mount_point().to_string_lossy().into_owned();
            // Skip pseudo file systems and bind-mounted files (e.g. /etc/hosts in containers).
            if d.total_space() > 0
                && !matches!(fs.as_str(), "tmpfs" | "devtmpfs" | "squashfs" | "proc" | "sysfs" | "nsfs")
                && !mount.starts_with("/etc/")
                && !mount.starts_with("/snap/")
                && volumes.len() < 64
                && mounts.insert(mount.clone())
            {
                volumes.push(DiskVolume {
                    mount_point: mount,
                    device: d.name().to_string_lossy().into_owned(),
                    file_system: fs.clone(),
                    total_bytes: d.total_space(),
                    available_bytes: d.available_space(),
                    removable: d.is_removable(),
                });
            }
            if d.is_removable() || d.total_space() == 0 || !seen.insert((d.name().to_owned(), d.total_space())) {
                continue;
            }
            disk_total = disk_total.saturating_add(d.total_space());
            disk_avail = disk_avail.saturating_add(d.available_space());
        }
        let (mut rx, mut tx) = (0u64, 0u64);
        for (name, n) in &self.nets {
            if name == "lo" || name.starts_with("Loopback") {
                continue;
            }
            rx = rx.saturating_add(n.received());
            tx = tx.saturating_add(n.transmitted());
        }
        #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss, clippy::cast_precision_loss)]
        let per_sec = |b: u64| (b as f64 / elapsed) as u64;
        #[allow(clippy::cast_possible_truncation)]
        let load1 = System::load_average().one as f32;
        // In a container (or a systemd slice with a memory limit) report the
        // limit, not the host's memory.
        let (mut mem_total, mut mem_used) = (self.sys.total_memory(), self.sys.used_memory());
        if let Some(l) = self.sys.cgroup_limits()
            && l.total_memory > 0
            && l.total_memory < mem_total
        {
            mem_total = l.total_memory;
            mem_used = l.total_memory.saturating_sub(l.free_memory);
        }
        NodeMetrics {
            cpu_percent: self.sys.global_cpu_usage().clamp(0.0, 100.0),
            mem_total_bytes: mem_total,
            mem_used_bytes: mem_used,
            swap_total_bytes: self.sys.total_swap(),
            swap_used_bytes: self.sys.used_swap(),
            disk_total_bytes: disk_total,
            disk_used_bytes: disk_total.saturating_sub(disk_avail),
            net_rx_bytes_per_sec: per_sec(rx),
            net_tx_bytes_per_sec: per_sec(tx),
            load1,
            host_uptime_secs: System::uptime(),
            cpu_count: u32::try_from(self.sys.cpus().len()).unwrap_or(u32::MAX),
            volumes,
            traffic: route_traffic(),
            top_processes: self.top_due(),
            // Filled by the agent from the helper's container samples.
            containers: Vec::new(),
        }
    }
}

/// Edge nodes: the last minute of proxied traffic, for the console.
impl Sampler {
    /// The top processes when a minute has passed since the last list.
    fn top_due(&mut self) -> Vec<forge_proto::messages::TopProcess> {
        if self.last_top.is_some_and(|t| t.elapsed() < TOP_EVERY) {
            return Vec::new();
        }
        self.last_top = Some(Instant::now());
        crate::processes::top(TOP_N)
    }
}

fn route_traffic() -> Vec<forge_proto::messages::SiteTraffic> {
    // Whole bytes per second are plenty for a console figure.
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    let b = |x: f64| x.max(0.0).round() as u64;
    crate::ingress::traffic_snapshot()
        .unwrap_or_default()
        .into_iter()
        .take(500)
        .map(|(site, t)| forge_proto::messages::SiteTraffic {
            site,
            requests: t.requests,
            in_bytes_per_sec: b(t.in_bps),
            out_bytes_per_sec: b(t.out_bps),
            upstreams: t
                .upstreams
                .into_iter()
                .take(200)
                .map(|(address, u)| forge_proto::messages::UpstreamTraffic {
                    address,
                    requests: u.requests,
                    sent_bytes_per_sec: b(u.sent_bps),
                    received_bytes_per_sec: b(u.received_bps),
                    errors: u.errors,
                    p95_ms: u.p95_ms,
                })
                .collect(),
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn samples_are_sane() {
        let mut s = Sampler::new();
        let m = s.sample();
        assert!(m.mem_total_bytes > 0 && m.mem_used_bytes <= m.mem_total_bytes);
        assert!((0.0..=100.0).contains(&m.cpu_percent));
        assert!(m.cpu_count > 0);
    }
}
