//! Soak mode: a fleet of lightweight simulated nodes against a server that
//! is already running (`--target`), for capacity tests.
//!
//! Each node speaks the agent protocol over its own mTLS HTTP/2 connection,
//! like `forge-agent`, but builds its telemetry instead of reading the host.
//! Like the agent since D-142, analytics data does not travel in
//! heartbeats: every node has its own disk spool under
//! `<state-dir>/<i>/spool` and its own uploader that sends the sealed files
//! over mTLS to the analytics worker's agent listener. On every heartbeat a
//! node appends to its spool the container log lines written since the
//! last one (at a fixed rate per node), and once a minute the closed node
//! minute, its top processes and one sample per container; then it seals
//! the buffer, as the agent does after container lines. The heartbeat
//! carries the live resource sample (without containers or processes) and
//! the spool's status.
//!
//! A line counts as acknowledged when the file holding it leaves the spool
//! by an upload, and as dropped when the spool refuses it or drops its
//! file (cap, age, rejected by the worker).
//!
//! The server holds each heartbeat for `--hold-secs` when it has no command
//! for the node. The heartbeat latency reported here is the round trip minus
//! that hold: the time the server spent on the heartbeat plus the network.
//! Heartbeats answered early with commands are counted, not timed.
//!
//! With `--server-pid`, every report also samples the server's resident
//! memory and open file descriptors (Linux), once before the fleet starts
//! and once `--settle-secs` after it stops, so a soak shows whether they
//! return to the baseline.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::{Duration, Instant};

use anyhow::Context;
use forge_agent::enroll::{HostFacts, enroll};
use forge_agent::link::{Endpoint, Link};
use forge_agent::spool::{Limits, Spool, SpoolError, SystemFreeDisk};
use forge_agent::spool_upload::{self, MakeSink, Target, WorkerSink};
use forge_agent::{AgentConfig, StatePaths};
use forge_core::analytics::{MinuteBucket, NodeSample};
use forge_core::spool::{ContainerLine, ContainerMinuteRow, Labels, Row, Stream, TopProcessRow};
use forge_proto::messages::{CommandBody, CommandResult, DiskVolume, Heartbeat, HeartbeatResponse, NodeMetrics};
use futures::{StreamExt, stream};
use prost::Message;
use serde::Serialize;
use serde_json::json;
use tokio::sync::watch;

use crate::Api;

const REQUEST_TIMEOUT: Duration = Duration::from_secs(90);
/// Sealed files a node remembers the line count of. At one file per
/// heartbeat this covers more than a day of backlog; an older file that is
/// forgotten has its lines counted as dropped.
const MAX_TRACKED_FILES: usize = 4_096;

/// What one soak run is asked to do.
#[derive(Debug, Clone)]
pub struct SoakArgs {
    pub agents: u32,
    pub api: String,
    pub agent_url: String,
    pub login: String,
    pub password: String,
    pub state_dir: PathBuf,
    pub duration: Duration,
    pub report_every: Duration,
    pub hold: Duration,
    pub log_lines_per_sec: f64,
    pub containers: u32,
    /// How many of the containers write the log lines; 0 means all of them.
    pub logging_containers: u32,
    /// Overrides the worker URL from enrollment and heartbeats.
    pub worker_url: Option<String>,
    /// The server process to sample (Linux).
    pub server_pid: Option<u32>,
    /// Wait after the load stops before the last sample.
    pub settle: Duration,
}

/// Counters shared by every node.
#[derive(Debug, Default)]
struct Stats {
    heartbeats: AtomicU64,
    failed: AtomicU64,
    early: AtomicU64,
    lines_generated: AtomicU64,
    lines_sent: AtomicU64,
    lines_acked: AtomicU64,
    lines_dropped: AtomicU64,
    container_samples: AtomicU64,
    /// Uploads the worker answered 429 (over `worker_max_uploads`).
    uploads_busy: AtomicU64,
    /// Uploads deferred for any other reason (503, network, timeout).
    uploads_failed: AtomicU64,
    /// Heartbeat latency (round trip minus hold), ms, since the last report.
    latency: Mutex<Vec<f64>>,
}

/// One report interval (or the whole run).
#[derive(Debug, Clone, Default, Serialize, PartialEq)]
pub struct Report {
    pub elapsed_secs: f64,
    pub agents: u32,
    pub heartbeats: u64,
    pub heartbeats_failed: u64,
    /// Answered early with commands; not timed.
    pub heartbeats_early: u64,
    pub hb_p50_ms: f64,
    pub hb_p95_ms: f64,
    pub hb_p99_ms: f64,
    pub hb_max_ms: f64,
    pub lines_generated: u64,
    /// Appended to the spools.
    pub lines_sent: u64,
    /// In files the worker acknowledged.
    pub lines_acked: u64,
    /// Refused by a spool or in a file the spool dropped.
    pub lines_dropped: u64,
    pub container_samples: u64,
    pub files_uploaded: u64,
    pub files_dropped: u64,
    /// Uploads answered 429 by the worker (busy).
    pub uploads_busy: u64,
    /// Uploads deferred for another reason (503, network, timeout).
    pub uploads_failed: u64,
    /// Sealed files waiting in all spools at report time.
    pub spool_files_waiting: u64,
    pub spool_bytes_waiting: u64,
    /// Acknowledged lines per second over the interval.
    pub acked_per_sec: f64,
    /// Generated lines per second over the interval.
    pub offered_per_sec: f64,
    pub server_rss_mb: Option<f64>,
    pub server_fds: Option<u64>,
    /// Total report only: the highest `server_rss_mb` of any report.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub server_rss_mb_peak: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub server_rss_mb_baseline: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub server_fds_baseline: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub server_rss_mb_after: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub server_fds_after: Option<u64>,
    /// Total report only: sealed files left in all spools after settling.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub spool_files_after: Option<u64>,
}

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
struct Totals {
    heartbeats: u64,
    failed: u64,
    early: u64,
    generated: u64,
    sent: u64,
    acked: u64,
    dropped: u64,
    samples: u64,
    files_uploaded: u64,
    files_dropped: u64,
    uploads_busy: u64,
    uploads_failed: u64,
    files_waiting: u64,
    bytes_waiting: u64,
}

impl Stats {
    /// The counters, with the spools' counters summed.
    fn totals(&self, spools: &[Arc<Spool>]) -> Totals {
        let l = |a: &AtomicU64| a.load(Ordering::Relaxed);
        let mut t = Totals {
            heartbeats: l(&self.heartbeats),
            failed: l(&self.failed),
            early: l(&self.early),
            generated: l(&self.lines_generated),
            sent: l(&self.lines_sent),
            acked: l(&self.lines_acked),
            dropped: l(&self.lines_dropped),
            samples: l(&self.container_samples),
            uploads_busy: l(&self.uploads_busy),
            uploads_failed: l(&self.uploads_failed),
            ..Totals::default()
        };
        let now = now_ms();
        for s in spools {
            let st = s.status(now);
            t.files_uploaded += st.uploaded_files;
            t.files_dropped += st.dropped_files;
            t.files_waiting += st.files;
            t.bytes_waiting += st.bytes;
        }
        t
    }

    fn take_latency(&self) -> Vec<f64> {
        std::mem::take(&mut *self.latency.lock().unwrap_or_else(PoisonError::into_inner))
    }
}

/// The server's resident memory and open descriptors at one moment.
#[derive(Debug, Default, Clone, Copy, PartialEq)]
pub struct ServerSample {
    pub rss_mb: Option<f64>,
    pub fds: Option<u64>,
}

/// `VmRSS` of a `/proc/<pid>/status` text, in MB.
#[allow(clippy::cast_precision_loss)] // reason: memory size for display
#[cfg(any(target_os = "linux", test))]
fn parse_vm_rss_mb(status: &str) -> Option<f64> {
    let line = status.lines().find(|l| l.starts_with("VmRSS:"))?;
    let kb: u64 = line.split_whitespace().nth(1)?.parse().ok()?;
    Some(kb as f64 / 1024.0)
}

#[cfg(target_os = "linux")]
fn read_server(pid: u32) -> ServerSample {
    let proc = PathBuf::from(format!("/proc/{pid}"));
    // A process that is gone or unreadable has no sample: `None` is shown.
    let rss_mb = std::fs::read_to_string(proc.join("status")).ok().and_then(|s| parse_vm_rss_mb(&s));
    let fds = std::fs::read_dir(proc.join("fd")).ok().map(|d| d.count() as u64);
    ServerSample { rss_mb, fds }
}

#[cfg(not(target_os = "linux"))]
fn read_server(_pid: u32) -> ServerSample {
    ServerSample::default()
}

/// Samples the server process, if one was named.
async fn sample_server(pid: Option<u32>) -> ServerSample {
    let Some(pid) = pid else { return ServerSample::default() };
    tokio::task::spawn_blocking(move || read_server(pid)).await.unwrap_or_else(|e| {
        tracing::warn!(error = %e, "sampling the server failed");
        ServerSample::default()
    })
}

/// The report for the interval between `before` and `now` totals.
#[allow(clippy::cast_precision_loss)] // reason: counters for display
fn report(agents: u32, elapsed: f64, secs: f64, before: Totals, now: Totals, mut lat: Vec<f64>) -> Report {
    lat.sort_by(f64::total_cmp);
    let d = |a: u64, b: u64| a.saturating_sub(b);
    let per_sec = |n: u64| if secs > 0.0 { n as f64 / secs } else { 0.0 };
    Report {
        elapsed_secs: elapsed,
        agents,
        heartbeats: d(now.heartbeats, before.heartbeats),
        heartbeats_failed: d(now.failed, before.failed),
        heartbeats_early: d(now.early, before.early),
        hb_p50_ms: crate::percentile(&lat, 0.50),
        hb_p95_ms: crate::percentile(&lat, 0.95),
        hb_p99_ms: crate::percentile(&lat, 0.99),
        hb_max_ms: lat.last().copied().unwrap_or(0.0),
        lines_generated: d(now.generated, before.generated),
        lines_sent: d(now.sent, before.sent),
        lines_acked: d(now.acked, before.acked),
        lines_dropped: d(now.dropped, before.dropped),
        container_samples: d(now.samples, before.samples),
        files_uploaded: d(now.files_uploaded, before.files_uploaded),
        files_dropped: d(now.files_dropped, before.files_dropped),
        uploads_busy: d(now.uploads_busy, before.uploads_busy),
        uploads_failed: d(now.uploads_failed, before.uploads_failed),
        spool_files_waiting: now.files_waiting,
        spool_bytes_waiting: now.bytes_waiting,
        acked_per_sec: per_sec(d(now.acked, before.acked)),
        offered_per_sec: per_sec(d(now.generated, before.generated)),
        ..Report::default()
    }
}

/// Small deterministic generator; the content only needs to vary.
#[derive(Debug, Clone)]
struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        let mut x = self.0;
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        self.0 = x;
        x
    }

    fn below(&mut self, n: u64) -> u64 {
        self.next() % n.max(1)
    }
}

/// The rate at which a node's containers write lines.
#[derive(Debug)]
struct LogState {
    rate: f64,
    /// Fractional lines owed since the last generation.
    owed: f64,
}

impl LogState {
    const fn new(rate: f64) -> Self {
        Self { rate, owed: 0.0 }
    }

    /// The lines written in `secs`.
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss, clippy::cast_precision_loss)] // reason: non-negative line counts
    fn accrue(&mut self, secs: f64) -> u64 {
        self.owed += self.rate * secs;
        let new = self.owed.floor() as u64;
        self.owed -= new as f64;
        new
    }
}

/// Sealed files of a node and the container lines each holds, so a file
/// that leaves the spool is counted as acknowledged or dropped.
#[derive(Debug, Default)]
struct Files {
    /// Stem → lines; stems are time-ordered, so the first is the oldest.
    pending: BTreeMap<String, u64>,
    /// Lines appended but not sealed yet (a seal failed).
    unsealed: u64,
    /// The spool's dropped-files counter at the last check.
    dropped_seen: u64,
}

impl Files {
    /// Records a new file holding the unsealed lines. Returns lines of a
    /// forgotten file (the map is full), counted as dropped.
    fn sealed(&mut self, stem: Option<String>) -> u64 {
        let Some(stem) = stem else { return 0 };
        let lines = std::mem::take(&mut self.unsealed);
        self.pending.insert(stem, lines);
        let mut forgotten = 0;
        while self.pending.len() > MAX_TRACKED_FILES {
            forgotten += self.pending.pop_first().map_or(0, |(_, n)| n);
        }
        forgotten
    }

    /// Settles the files no longer waiting. `dropped_now` is the spool's
    /// dropped-files counter: the files dropped since the last check are
    /// the oldest that left (the spool drops oldest first), the rest were
    /// uploaded. A file rejected by the worker while this runs can be
    /// counted as uploaded; that race is rare and only shifts one file.
    /// Returns (acknowledged, dropped) lines.
    fn settle(&mut self, waiting: impl Fn(&str) -> bool, dropped_now: u64) -> (u64, u64) {
        let gone: Vec<String> = self.pending.keys().filter(|s| !waiting(s)).cloned().collect();
        let mut drops = dropped_now.saturating_sub(self.dropped_seen);
        self.dropped_seen = dropped_now;
        let (mut acked, mut dropped) = (0, 0);
        for stem in gone {
            let n = self.pending.remove(&stem).unwrap_or(0);
            if drops > 0 {
                drops -= 1;
                dropped += n;
            } else {
                acked += n;
            }
        }
        (acked, dropped)
    }
}

const TEMPLATES: [&str; 5] = [
    "GET /api/v1/items/{} responded 200 in {} ms",
    "user {} logged in from 10.0.{}.7",
    "order {} placed for {} EUR",
    "cache miss for key k{} after {} ms",
    "job sync-{} failed: timeout after {} s, retrying",
];

/// What one beat writes to the spool.
#[derive(Debug, Default)]
struct Written {
    lines: u64,
    refused_lines: u64,
    stem: Option<String>,
}

/// One simulated node.
struct SimNode {
    idx: u32,
    link: Link,
    spool: Arc<Spool>,
    /// `--worker-url` overrides the worker the server names.
    fixed_url: bool,
    target: watch::Sender<Option<Target>>,
    files: Files,
    rng: Rng,
    logs: LogState,
    containers: u32,
    /// The first `log_sources` containers write the lines; the rest are silent.
    log_sources: u32,
    started: Instant,
    bucket: Option<MinuteBucket>,
    last_minute: i64,
    results: Vec<CommandResult>,
}

fn now_ms() -> i64 {
    forge_agent::node_analytics::now_ms()
}

fn labels(k: u32) -> Labels {
    Labels {
        forge_app: String::new(),
        forge_stack: String::new(),
        compose_project: "sim".into(),
        compose_service: format!("app-{k}"),
    }
}

fn container_id(idx: u32, k: u32) -> String {
    format!("{idx:08x}{k:04x}")
}

/// The containers that write lines: `logging` of them (at least one, at
/// most all), or all when `logging` is 0.
fn log_sources(containers: u32, logging: u32) -> u32 {
    let containers = containers.max(1);
    if logging == 0 { containers } else { logging.min(containers) }
}

/// `n` container lines of node `idx`, spread over its first `containers`.
fn make_lines(rng: &mut Rng, idx: u32, containers: u32, n: u64, ts: i64) -> Vec<Row> {
    let containers = containers.max(1);
    (0..n)
        .map(|_| {
            let k = u32::try_from(rng.below(u64::from(containers))).unwrap_or(0);
            let t = TEMPLATES[usize::try_from(rng.below(5)).unwrap_or(0)];
            let msg =
                t.replacen("{}", &rng.below(1_000_000).to_string(), 1).replacen("{}", &rng.below(900).to_string(), 1);
            let level = ["INFO", "INFO", "INFO", "DEBUG", "WARN", "ERROR"][usize::try_from(rng.below(6)).unwrap_or(0)];
            Row::ContainerLog(ContainerLine {
                ts,
                stream: if level == "ERROR" { Stream::Stderr } else { Stream::Stdout },
                cid: container_id(idx, k),
                name: format!("sim-app-{k}"),
                image: "sim/app:1".into(),
                labels: labels(k),
                line: format!("{level} [worker-{}] {msg} trace={:016x}", rng.below(8), rng.next()),
            })
        })
        .collect()
}

impl SimNode {
    /// The live resource sample of a heartbeat (no containers or processes:
    /// those go to the spool).
    #[allow(clippy::cast_precision_loss)] // reason: synthetic values
    fn metrics(&mut self) -> NodeMetrics {
        let gib = 1u64 << 30;
        NodeMetrics {
            cpu_percent: 5.0 + (self.rng.below(400) as f32) / 10.0,
            mem_total_bytes: 8 * gib,
            mem_used_bytes: 2 * gib + self.rng.below(2 * gib),
            disk_total_bytes: 100 * gib,
            disk_used_bytes: 40 * gib,
            net_rx_bytes_per_sec: self.rng.below(1 << 20),
            net_tx_bytes_per_sec: self.rng.below(1 << 20),
            load1: (self.rng.below(300) as f32) / 100.0,
            host_uptime_secs: 86_400 + self.started.elapsed().as_secs(),
            cpu_count: 4,
            volumes: vec![DiskVolume {
                mount_point: "/".into(),
                device: "/dev/sda1".into(),
                file_system: "ext4".into(),
                total_bytes: 100 * gib,
                available_bytes: 60 * gib,
                removable: false,
            }],
            ..NodeMetrics::default()
        }
    }

    /// Folds `m` into the open minute, like the agent's `record_node`, and
    /// returns the rows due now: the closed minute, and once a minute the
    /// top processes and one sample per container.
    #[allow(clippy::cast_precision_loss)] // reason: synthetic values
    fn minute_rows(&mut self, m: &NodeMetrics, now: i64) -> Vec<Row> {
        let gib = 1u64 << 30;
        let minute = MinuteBucket::minute_of(now);
        let mut rows = Vec::new();
        match &mut self.bucket {
            Some(b) if b.minute() == minute => {}
            other => {
                if let Some(b) = other.take() {
                    rows.push(Row::NodeMinute(b.close()));
                }
                *other = Some(MinuteBucket::new(minute));
            }
        }
        if let Some(b) = &mut self.bucket {
            b.add(&NodeSample {
                cpu: m.cpu_percent,
                mem_used: m.mem_used_bytes,
                mem_total: m.mem_total_bytes,
                swap_used: m.swap_used_bytes,
                disk_used: m.disk_used_bytes,
                disk_total: m.disk_total_bytes,
                rx: m.net_rx_bytes_per_sec,
                tx: m.net_tx_bytes_per_sec,
                load1: m.load1,
            });
        }
        if minute == self.last_minute {
            return rows;
        }
        self.last_minute = minute;
        for p in 0..10u32 {
            rows.push(Row::TopProcess(TopProcessRow {
                minute,
                pid: 1000 + p,
                name: format!("proc-{p}"),
                user: "root".into(),
                cpu: (self.rng.below(500) as f32) / 10.0,
                mem: self.rng.below(gib),
                cmd: format!("/usr/bin/proc-{p} --serve"),
            }));
        }
        for k in 0..self.containers {
            rows.push(Row::ContainerMinute(ContainerMinuteRow {
                minute,
                cid: container_id(self.idx, k),
                name: format!("sim-app-{k}"),
                image: "sim/app:1".into(),
                labels: labels(k),
                cpu: (self.rng.below(1000) as f32) / 10.0,
                mem_used: self.rng.below(gib),
                mem_limit: gib,
                rx: self.rng.below(1 << 20),
                tx: self.rng.below(1 << 20),
                block_read: self.rng.below(1 << 20),
                block_write: self.rng.below(1 << 20),
                restarts: 0,
            }));
        }
        rows
    }

    /// Appends `rows` (expiring old files first) and seals the buffer, off
    /// the async threads.
    async fn write(&self, rows: Vec<Row>, now: i64) -> Result<Written, SpoolError> {
        let spool = self.spool.clone();
        tokio::task::spawn_blocking(move || {
            spool.tick(now)?;
            let mut w = Written::default();
            for r in &rows {
                let line = matches!(r, Row::ContainerLog(_));
                match spool.append(r, now) {
                    Ok(()) if line => w.lines += 1,
                    Ok(()) => {}
                    Err(_) if line => w.refused_lines += 1,
                    // A refused minute row is lost like on the agent.
                    Err(_) => {}
                }
            }
            w.stem = spool.seal_named(now)?;
            Ok(w)
        })
        .await
        .map_err(|e| SpoolError::Io(e.to_string()))?
    }

    /// Counts the lines of files that left the spool.
    fn settle_files(&mut self, stats: &Stats) {
        let dropped_now = self.spool.status(now_ms()).dropped_files;
        let spool = &self.spool;
        let (acked, dropped) = self.files.settle(|s| spool.contains(s), dropped_now);
        stats.lines_acked.fetch_add(acked, Ordering::Relaxed);
        stats.lines_dropped.fetch_add(dropped, Ordering::Relaxed);
    }

    /// Spools the data written since the last beat.
    async fn spool_data(&mut self, stats: &Stats, m: &NodeMetrics, now: i64, secs_since_last: f64) {
        let generated = self.logs.accrue(secs_since_last);
        stats.lines_generated.fetch_add(generated, Ordering::Relaxed);
        let mut rows = make_lines(&mut self.rng, self.idx, self.log_sources, generated, now);
        let minute = self.minute_rows(m, now);
        let samples = minute.iter().filter(|r| matches!(r, Row::ContainerMinute(_))).count() as u64;
        rows.extend(minute);
        match self.write(rows, now).await {
            Ok(w) => {
                stats.lines_sent.fetch_add(w.lines, Ordering::Relaxed);
                stats.lines_dropped.fetch_add(w.refused_lines, Ordering::Relaxed);
                stats.container_samples.fetch_add(samples, Ordering::Relaxed);
                self.files.unsealed += w.lines;
                let forgotten = self.files.sealed(w.stem);
                stats.lines_dropped.fetch_add(forgotten, Ordering::Relaxed);
            }
            Err(e) => {
                // Lines appended before the error stay unsealed and are
                // counted with the next file; the rest are lost.
                tracing::warn!(node = self.idx, error = %e, "spooling failed");
                stats.lines_dropped.fetch_add(generated, Ordering::Relaxed);
            }
        }
        self.settle_files(stats);
    }

    /// One heartbeat. Returns whether it succeeded.
    async fn beat(&mut self, stats: &Stats, hold: Duration, secs_since_last: f64) -> bool {
        let now = now_ms();
        let metrics = self.metrics();
        self.spool_data(stats, &metrics, now, secs_since_last).await;
        let results = std::mem::take(&mut self.results);
        let hb = Heartbeat {
            agent_version: forge_agent::VERSION.into(),
            uptime_secs: self.started.elapsed().as_secs(),
            rss_bytes: 20 << 20,
            results: results.clone(),
            metrics: Some(metrics),
            container_logs: None,
            spool: Some(self.spool.status(now)),
            ..Heartbeat::default()
        };
        let t = Instant::now();
        let r = self.link.request(http::Method::POST, forge_proto::paths::HEARTBEAT, hb.encode_to_vec()).await;
        let rtt = t.elapsed();
        let Ok(body) = r else {
            self.results = results;
            stats.failed.fetch_add(1, Ordering::Relaxed);
            return false;
        };
        let Ok(resp) = HeartbeatResponse::decode(body.as_slice()) else {
            stats.failed.fetch_add(1, Ordering::Relaxed);
            return false;
        };
        stats.heartbeats.fetch_add(1, Ordering::Relaxed);
        self.follow_worker(&resp);
        if resp.commands.is_empty() && rtt + Duration::from_millis(500) >= hold {
            let ms = rtt.saturating_sub(hold).as_secs_f64() * 1000.0;
            stats.latency.lock().unwrap_or_else(PoisonError::into_inner).push(ms);
        } else {
            stats.early.fetch_add(1, Ordering::Relaxed);
        }
        // Refuse every command, like fleet-sim's other agents: the
        // simulator never changes the host.
        self.results = refusals(&resp);
        true
    }

    /// Points the uploader at the worker the server names, unless
    /// `--worker-url` fixed it.
    fn follow_worker(&self, resp: &HeartbeatResponse) {
        if self.fixed_url {
            return;
        }
        let Some(url) = resp.analytics.as_ref().map(|a| a.url.clone()).filter(|u| !u.is_empty()) else { return };
        self.target.send_if_modified(|t| {
            let same = t.as_ref().is_some_and(|t| t.url == url);
            if !same {
                *t = Some(Target { url, generation: 0 });
            }
            !same
        });
    }

    /// Runs until `stop` and hands the node back for the final count.
    async fn run(mut self, stats: Arc<Stats>, hold: Duration, mut stop: watch::Receiver<bool>) -> Self {
        let mut last = Instant::now();
        let mut failures: u32 = 0;
        loop {
            if *stop.borrow() {
                return self;
            }
            let since = last.elapsed().as_secs_f64();
            last = Instant::now();
            // A heartbeat in flight finishes (the server holds it).
            let ok = self.beat(&stats, hold, since).await;
            let wait = if ok {
                failures = 0;
                Duration::ZERO
            } else {
                failures = failures.saturating_add(1);
                // Full jitter, 1 s doubling to 60 s, like the agent.
                let cap = 2u64.saturating_pow(failures.min(6)).min(60);
                Duration::from_millis(self.rng.below(cap * 1000).max(100))
            };
            tokio::select! {
                () = tokio::time::sleep(wait) => {}
                _ = stop.changed() => return self,
            }
        }
    }
}

/// Results refusing every command in `resp`.
fn refusals(resp: &HeartbeatResponse) -> Vec<CommandResult> {
    resp.commands
        .iter()
        .filter_map(|c| CommandBody::decode(c.body.as_slice()).ok())
        .map(|b| CommandResult {
            operation_id: b.operation_id,
            ok: false,
            output: String::new(),
            error: "refused by fleet-sim".into(),
            error_code: String::new(),
        })
        .collect()
}

/// A loaded node and how its uploader connects.
struct Loaded {
    node: SimNode,
    make: MakeSink,
}

/// Loads node `i` from `dir`, enrolling it first when it has no identity,
/// and opens its spool.
async fn node(api: &Api, agent_url: &str, dir: &Path, i: u32, a: &SoakArgs) -> anyhow::Result<Loaded> {
    let paths = StatePaths::new(dir.join(i.to_string()));
    if paths.load_identity().is_err() {
        let tok = api.post("/api/v1/enrollment-tokens", json!({})).await?;
        let facts = HostFacts { hostname: format!("sim-{i:05}"), os: "linux".into(), arch: "x86_64".into() };
        let mut cfg = AgentConfig::new(agent_url.to_owned());
        cfg.allowed_commands = vec!["ping".into()];
        enroll(&paths, cfg, tok["token"].as_str().context("token")?, facts).await?;
    }
    let p = paths.clone();
    let (config, tls, spool) = tokio::task::spawn_blocking(move || -> anyhow::Result<_> {
        let identity = p.load_identity()?;
        let config = p.load_config()?;
        let tls = forge_agent::tls::mutual(
            &std::fs::read_to_string(p.ca())?,
            &std::fs::read_to_string(p.cert())?,
            &std::fs::read_to_string(p.key())?,
        )?;
        let uuid = uuid::Uuid::parse_str(&identity.server_id).context("server id")?;
        let (spool, _) =
            Spool::open(&p.dir.join("spool"), uuid, Limits::default(), Arc::new(SystemFreeDisk), now_ms())?;
        Ok((config, tls, spool))
    })
    .await??;
    let url = a.worker_url.clone().or(config.analytics_url).filter(|u| !u.is_empty());
    Ok(Loaded {
        node: SimNode {
            idx: i,
            link: Link::new(Endpoint::parse(agent_url)?, tls.clone(), REQUEST_TIMEOUT),
            spool: Arc::new(spool),
            fixed_url: a.worker_url.is_some(),
            target: watch::channel(url.map(|url| Target { url, generation: 0 })).0,
            files: Files::default(),
            rng: Rng(0x9E37_79B9_7F4A_7C15 ^ (u64::from(i) + 1).wrapping_mul(0x2545_F491_4F6C_DD1D)),
            logs: LogState::new(a.log_lines_per_sec),
            containers: a.containers,
            log_sources: log_sources(a.containers, a.logging_containers),
            started: Instant::now(),
            bucket: None,
            last_minute: 0,
            results: Vec::new(),
        },
        make: Box::new(move |t: &Target| {
            Ok(Box::new(WorkerSink::new(&t.url, tls.clone())?) as Box<dyn spool_upload::Sink>)
        }),
    })
}

/// Counts the worker's refusals of one node's uploads.
struct CountingSink {
    inner: Box<dyn spool_upload::Sink>,
    stats: Arc<Stats>,
}

#[async_trait::async_trait]
impl spool_upload::Sink for CountingSink {
    async fn upload(&mut self, stem: &str, body: Vec<u8>) -> spool_upload::Outcome {
        let out = self.inner.upload(stem, body).await;
        if let spool_upload::Outcome::Retry(reason) = &out {
            let busy = reason.starts_with(http::StatusCode::TOO_MANY_REQUESTS.as_str());
            if busy {
                self.stats.uploads_busy.fetch_add(1, Ordering::Relaxed);
            } else if self.stats.uploads_failed.fetch_add(1, Ordering::Relaxed) < 20 {
                // The first few reasons, so a report says why uploads failed.
                eprintln!("{}", json!({ "upload_failed": reason }));
            }
        }
        out
    }
}

/// Wraps `make` so every sink it builds counts refusals into `stats`.
fn counting(make: MakeSink, stats: Arc<Stats>) -> MakeSink {
    Box::new(move |t: &Target| {
        Ok(Box::new(CountingSink { inner: make(t)?, stats: stats.clone() }) as Box<dyn spool_upload::Sink>)
    })
}

/// Runs the soak: enrolls (or reuses) the nodes, runs them for the
/// duration, prints a report line every interval, settles and returns the
/// total.
pub async fn run(a: SoakArgs) -> anyhow::Result<Report> {
    let baseline = sample_server(a.server_pid).await;
    eprintln!("{}", json!({ "server_rss_mb_baseline": baseline.rss_mb, "server_fds_baseline": baseline.fds }));
    let api = Api::login(&a.api, &a.login, &a.password).await?;
    tokio::fs::create_dir_all(&a.state_dir).await?;
    let t0 = Instant::now();
    let loaded: Vec<anyhow::Result<Loaded>> = stream::iter(0..a.agents)
        .map(|i| {
            let (api, a) = (&api, &a);
            async move { node(api, &a.agent_url, &a.state_dir, i, a).await }
        })
        .buffer_unordered(64)
        .collect()
        .await;
    eprintln!("{}", json!({ "ready_secs": t0.elapsed().as_secs_f64(), "agents": a.agents }));
    let stats = Arc::new(Stats::default());
    let (stop, rx) = watch::channel(false);
    let (stop_uploads, uploads_rx) = watch::channel(false);
    let mut tasks = Vec::new();
    let mut uploads = Vec::new();
    let mut spools = Vec::new();
    for l in loaded {
        let Loaded { node, make } = l?;
        let make = counting(make, stats.clone());
        spools.push(node.spool.clone());
        uploads.push(tokio::spawn(spool_upload::run(
            node.spool.clone(),
            node.target.subscribe(),
            make,
            uploads_rx.clone(),
        )));
        tasks.push(tokio::spawn(node.run(stats.clone(), a.hold, rx.clone())));
    }
    let start = Instant::now();
    let (mut before, mut at) = (stats.totals(&spools), Instant::now());
    let mut all = Vec::new();
    let mut peak: Option<f64> = None;
    while start.elapsed() < a.duration {
        let left = a.duration.saturating_sub(start.elapsed());
        tokio::time::sleep(a.report_every.min(left)).await;
        let now = stats.totals(&spools);
        let lat = stats.take_latency();
        all.extend_from_slice(&lat);
        let mut r = report(a.agents, start.elapsed().as_secs_f64(), at.elapsed().as_secs_f64(), before, now, lat);
        let s = sample_server(a.server_pid).await;
        (r.server_rss_mb, r.server_fds) = (s.rss_mb, s.fds);
        peak = max_opt(peak, s.rss_mb);
        eprintln!("{}", serde_json::to_string(&r)?);
        (before, at) = (now, Instant::now());
    }
    let secs = start.elapsed().as_secs_f64();
    let _ = stop.send(true);
    let mut nodes = Vec::new();
    for t in tasks {
        match t.await {
            Ok(n) => nodes.push(n),
            Err(e) => tracing::warn!(error = %e, "a node task failed"),
        }
    }
    // Uploads keep running while the server settles, so the spools drain.
    tokio::time::sleep(a.settle).await;
    for n in &mut nodes {
        n.settle_files(&stats);
    }
    let after = sample_server(a.server_pid).await;
    let _ = stop_uploads.send(true);
    for u in uploads {
        if let Err(e) = u.await {
            tracing::warn!(error = %e, "an upload task failed");
        }
    }
    all.extend(stats.take_latency());
    let end = stats.totals(&spools);
    let mut total = report(a.agents, secs, secs, Totals::default(), end, all);
    (total.server_rss_mb, total.server_fds) = (after.rss_mb, after.fds);
    total.server_rss_mb_peak = max_opt(peak, after.rss_mb);
    total.server_rss_mb_baseline = baseline.rss_mb;
    total.server_fds_baseline = baseline.fds;
    total.server_rss_mb_after = after.rss_mb;
    total.server_fds_after = after.fds;
    total.spool_files_after = Some(end.files_waiting);
    Ok(total)
}

fn max_opt(a: Option<f64>, b: Option<f64>) -> Option<f64> {
    match (a, b) {
        (Some(x), Some(y)) => Some(x.max(y)),
        (x, None) => x,
        (None, y) => y,
    }
}

/// Fails when the run's peak server RSS is above `max_mb`.
pub fn check_rss(r: &Report, max_mb: Option<f64>) -> anyhow::Result<()> {
    match (max_mb, r.server_rss_mb_peak) {
        (Some(max), Some(peak)) if peak > max => {
            anyhow::bail!("server RSS peaked at {peak:.1} MB, above --max-server-rss-mb {max}")
        }
        (Some(_), None) => anyhow::bail!("--max-server-rss-mb needs a server RSS sample (--server-pid, Linux)"),
        _ => Ok(()),
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)] // reason: tests
mod tests {
    use super::*;

    #[test]
    fn lines_accrue_at_the_rate() {
        let mut s = LogState::new(20.0);
        assert_eq!(s.accrue(25.0), 500, "20 lines/s for one 25 s hold");
        assert_eq!(s.accrue(0.04), 0, "fractions carry over");
        assert_eq!(s.accrue(0.01), 1);
        assert_eq!(LogState::new(0.0).accrue(100.0), 0);
    }

    fn sim(dir: &Path, containers: u32) -> SimNode {
        let tls = forge_agent::tls::ca_bootstrap().unwrap();
        let (spool, _) =
            Spool::open(dir, uuid::Uuid::now_v7(), Limits::default(), Arc::new(SystemFreeDisk), now_ms()).unwrap();
        SimNode {
            idx: 3,
            link: Link::new(Endpoint::parse("https://127.0.0.1:1").unwrap(), tls, REQUEST_TIMEOUT),
            spool: Arc::new(spool),
            fixed_url: false,
            target: watch::channel(None).0,
            files: Files::default(),
            rng: Rng(42),
            logs: LogState::new(0.0),
            containers,
            log_sources: containers,
            started: Instant::now(),
            bucket: None,
            last_minute: 0,
            results: Vec::new(),
        }
    }

    fn count(rows: &[Row], f: fn(&Row) -> bool) -> usize {
        rows.iter().filter(|r| f(r)).count()
    }

    #[test]
    fn minute_rows_go_once_a_minute() {
        let dir = tempfile::tempdir().unwrap();
        let mut n = sim(dir.path(), 5);
        let t = 1_700_000_040_000; // a minute boundary
        let m = n.metrics();
        assert!(m.containers.is_empty() && m.top_processes.is_empty(), "the heartbeat carries neither");
        let first = n.minute_rows(&m, t);
        assert_eq!(count(&first, |r| matches!(r, Row::ContainerMinute(_))), 5);
        assert_eq!(count(&first, |r| matches!(r, Row::TopProcess(_))), 10);
        assert_eq!(count(&first, |r| matches!(r, Row::NodeMinute(_))), 0, "no minute closed yet");
        assert!(n.minute_rows(&m, t + 25_000).is_empty(), "same minute");
        let next = n.minute_rows(&m, t + 60_000);
        assert_eq!(next.len(), 16, "the closed minute, processes and samples");
        let Some(Row::NodeMinute(nm)) = next.first() else { panic!("closed minute first") };
        assert_eq!((nm.minute, nm.samples), (t, 2));
        let lines = make_lines(&mut n.rng, 3, 5, 50, t);
        assert_eq!(lines.len(), 50);
        assert!(
            lines
                .iter()
                .all(|l| matches!(l, Row::ContainerLog(c) if c.name.starts_with("sim-app-") && !c.line.is_empty()))
        );
    }

    #[test]
    fn only_the_logging_containers_write_lines() {
        assert_eq!(log_sources(20, 10), 10);
        assert_eq!(log_sources(20, 0), 20, "0 means all");
        assert_eq!(log_sources(5, 10), 5, "never more than there are");
        assert_eq!(log_sources(0, 0), 1);
        let mut rng = Rng(7);
        let lines = make_lines(&mut rng, 3, log_sources(20, 10), 2_000, 1_700_000_000_000);
        let ids: std::collections::BTreeSet<String> = lines
            .iter()
            .filter_map(|l| match l {
                Row::ContainerLog(c) => Some(c.cid.clone()),
                _ => None,
            })
            .collect();
        let want: std::collections::BTreeSet<String> = (0..10).map(|k| container_id(3, k)).collect();
        assert_eq!(ids, want, "lines come from exactly the first 10 containers");
    }

    #[tokio::test]
    async fn a_beat_spools_and_seals_and_acks_are_counted() {
        let dir = tempfile::tempdir().unwrap();
        let mut n = sim(dir.path(), 2);
        n.logs = LogState::new(10.0);
        let stats = Stats::default();
        let m = n.metrics();
        n.spool_data(&stats, &m, now_ms(), 3.0).await;
        assert_eq!(stats.lines_generated.load(Ordering::Relaxed), 30);
        assert_eq!(stats.lines_sent.load(Ordering::Relaxed), 30);
        assert_eq!(stats.container_samples.load(Ordering::Relaxed), 2);
        assert_eq!(n.files.pending.len(), 1, "sealed once per beat");
        let st = n.spool.status(now_ms());
        assert_eq!(st.files, 1);
        let stem = n.files.pending.keys().next().unwrap().clone();
        n.spool.ack(&stem).unwrap();
        n.settle_files(&stats);
        assert_eq!(stats.lines_acked.load(Ordering::Relaxed), 30);
        assert_eq!(stats.lines_dropped.load(Ordering::Relaxed), 0);
        assert!(n.files.pending.is_empty());
    }

    #[test]
    fn files_that_leave_are_acked_or_dropped_oldest_first() {
        let mut f = Files::default();
        for (stem, lines) in [("a", 10), ("b", 20), ("c", 30)] {
            f.unsealed = lines;
            assert_eq!(f.sealed(Some(stem.into())), 0);
        }
        assert_eq!(f.sealed(None), 0, "an empty seal records nothing");
        // "a" and "b" left; one file was dropped: the oldest.
        assert_eq!(f.settle(|s| s == "c", 1), (20, 10));
        assert_eq!(f.settle(|_| false, 1), (30, 0), "no new drop");
        assert!(f.pending.is_empty());

        let mut f = Files::default();
        for i in 0..=MAX_TRACKED_FILES {
            f.unsealed = 1;
            let forgotten = f.sealed(Some(format!("{i:08}")));
            assert_eq!(forgotten, u64::from(i == MAX_TRACKED_FILES));
        }
    }

    #[test]
    fn reports_rates_spool_totals_and_latency_percentiles() {
        let before = Totals { generated: 100, acked: 50, files_uploaded: 4, ..Totals::default() };
        let now = Totals {
            heartbeats: 10,
            generated: 1100,
            acked: 1050,
            files_uploaded: 9,
            files_waiting: 3,
            bytes_waiting: 900,
            ..Totals::default()
        };
        let r = report(2, 10.0, 10.0, before, now, (1..=100).map(f64::from).collect());
        assert_eq!(r.heartbeats, 10);
        assert!((r.offered_per_sec - 100.0).abs() < 1e-9);
        assert!((r.acked_per_sec - 100.0).abs() < 1e-9);
        assert_eq!((r.files_uploaded, r.spool_files_waiting, r.spool_bytes_waiting), (5, 3, 900));
        assert!((r.hb_p50_ms - 51.0).abs() < 1e-9 && (r.hb_p99_ms - 99.0).abs() < 1e-9);
        assert!((r.hb_max_ms - 100.0).abs() < 1e-9);
        let v = serde_json::to_value(&r).unwrap();
        assert!(v.get("server_rss_mb").is_some(), "every line shows the sample");
        assert!(v.get("server_rss_mb_peak").is_none(), "only the total shows the peak");
    }

    #[test]
    fn reads_rss_and_gates_on_the_peak() {
        let status = "Name:\tforge-server\nVmPeak:\t  900 kB\nVmRSS:\t  204800 kB\nThreads:\t9\n";
        assert_eq!(parse_vm_rss_mb(status), Some(200.0));
        assert_eq!(parse_vm_rss_mb("Name: x\n"), None);
        assert_eq!(max_opt(Some(1.0), None), Some(1.0));
        assert_eq!(max_opt(Some(1.0), Some(3.0)), Some(3.0));

        let r = Report { server_rss_mb_peak: Some(250.0), ..Report::default() };
        assert!(check_rss(&r, None).is_ok());
        assert!(check_rss(&r, Some(300.0)).is_ok());
        assert!(check_rss(&r, Some(200.0)).is_err());
        assert!(check_rss(&Report::default(), Some(200.0)).is_err(), "no sample cannot pass a gate");
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn samples_this_process() {
        let s = read_server(std::process::id());
        assert!(s.rss_mb.is_some_and(|m| m > 0.0));
        assert!(s.fds.is_some_and(|n| n > 0));
    }

    #[test]
    fn refuses_every_command() {
        let body = CommandBody { operation_id: "op-1".into(), ..CommandBody::default() };
        let resp = HeartbeatResponse {
            commands: vec![forge_proto::messages::SignedCommand {
                body: body.encode_to_vec(),
                signature: vec![],
                key_id: String::new(),
            }],
            ..HeartbeatResponse::default()
        };
        let r = refusals(&resp);
        assert_eq!(r.len(), 1);
        assert_eq!(r[0].operation_id, "op-1");
        assert!(!r[0].ok);
    }
}
