//! The embedded analytics engine (D-127): the ClickHouse engine, run by
//! the analytics worker as a supervised child process on `127.0.0.1`
//! (moved from forge-server, D-141).
//!
//! - The engine binary is a separate download (`forge-server analytics
//!   install`), so `forge-server` stays small. Linux only.
//! - Data, configuration and the generated password live in
//!   `<data_dir>/analytics`, readable by the service user only.
//! - It listens on loopback only, with a memory cap and throttled merges, so
//!   it fits a small VM. A crash restarts it with back-off; the control
//!   plane keeps running and history pages report it unavailable.
//! - Its limits need no privileges and touch only the engine, never
//!   Forge's own processes (D-140): the memory budget is the engine's own
//!   `max_server_memory_usage`, and a CPU cap pins the engine to whole CPUs
//!   (`sched_setaffinity`) and sizes its thread pools to match. An
//!   administrator can restart it from the console; Forge keeps serving
//!   meanwhile.

use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use anyhow::Context;
use forge_analytics_proto::config::EmbeddedPaths;
use forge_analytics_proto::tuning::Tuning;
use tokio::sync::{Notify, watch};

/// The password of the engine's `forge` user: created on first use, kept in
/// `<data_dir>/analytics/secret` (mode 0600). Blocking: call at start-up or
/// inside `spawn_blocking`.
pub fn password(paths: &EmbeddedPaths) -> anyhow::Result<String> {
    if let Ok(s) = std::fs::read_to_string(&paths.secret) {
        let s = s.trim().to_string();
        if !s.is_empty() {
            return Ok(s);
        }
    }

    std::fs::create_dir_all(&paths.root).context("creating the analytics directory")?;
    let pw = forge_secrets::crypto::random_token::<24>();
    forge_secrets::write_private(&paths.secret, pw.as_bytes()).context("writing the analytics secret")?;

    Ok(pw)
}

fn xml_escape(s: &str) -> String {
    s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;").replace('"', "&quot;")
}

/// Writes `config.xml` and `users.xml` for the engine. Blocking.
/// Turns off the engine's own logging and profiling; only the crash log is
/// kept, with a 6-hour TTL. Its system log tables
/// (trace, metric, part, query, …) flush and merge all the time and grew
/// larger than Forge's data; with them on, an idle engine used about twice
/// the CPU and memory. `remove` only applies to files merged from
/// `config.d/`, not to the main config, so these live there.
const QUIET: &str = r#"<clickhouse>
    <query_log remove="1"/>
    <query_thread_log remove="1"/>
    <query_views_log remove="1"/>
    <query_metric_log remove="1"/>
    <part_log remove="1"/>
    <trace_log remove="1"/>
    <metric_log remove="1"/>
    <asynchronous_metric_log remove="1"/>
    <asynchronous_insert_log remove="1"/>
    <text_log remove="1"/>
    <session_log remove="1"/>
    <processors_profile_log remove="1"/>
    <opentelemetry_span_log remove="1"/>
    <error_log remove="1"/>
    <latency_log remove="1"/>
    <blob_storage_log remove="1"/>
    <backup_log remove="1"/>
    <s3queue_log remove="1"/>
    <filesystem_cache_log remove="1"/>
    <zookeeper_log remove="1"/>
    <!-- Kept: written only when the engine crashes, and expired after 6 hours. -->
    <crash_log replace="1">
        <database>system</database>
        <table>crash_log</table>
        <engine>ENGINE = MergeTree ORDER BY event_time TTL event_time + INTERVAL 6 HOUR DELETE</engine>
        <flush_interval_milliseconds>7500</flush_interval_milliseconds>
    </crash_log>
    <asynchronous_metrics_update_period_s>60</asynchronous_metrics_update_period_s>
    <total_memory_profiler_step>0</total_memory_profiler_step>
    <total_memory_tracker_sample_probability>0</total_memory_tracker_sample_probability>
</clickhouse>
"#;

/// Concurrent inserts the engine admits: every spool import the worker may
/// run at once (each runs its table inserts one after another), plus the
/// worker's own flushes (node minutes, log batches). Fewer made the engine
/// answer `TOO_MANY_SIMULTANEOUS_QUERIES` under load, and a file refused
/// after its first table insert was stored twice on resend.
const INSERT_SLOTS: u32 = forge_analytics_proto::config::MAX_UPLOADS + 8;
/// Concurrent searches and reads the engine admits.
const SELECT_SLOTS: u32 = 24;
const _: () = assert!(INSERT_SLOTS > forge_analytics_proto::config::MAX_UPLOADS, "every import fits");

/// What Forge writes into the engine's configuration.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EngineConfig {
    pub port: u16,
    /// `max_server_memory_usage`, MiB.
    pub memory_mb: u32,
    /// CPUs the engine is pinned to (`None`: not pinned).
    pub cpus: Option<u32>,
}

impl EngineConfig {
    #[must_use]
    pub fn of(port: u16, t: &Tuning) -> Self {
        Self { port, memory_mb: t.memory_budget_mb, cpus: cpu_cap(t.cpu_quota_vcpu) }
    }
}

/// The engine's thread settings for a CPU cap.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Threads {
    /// `max_threads` of one query.
    pub max_threads: u32,
    /// `concurrent_threads_soft_limit_num` (`0`: no limit).
    pub concurrent_soft_limit: u32,
    pub background_pool: u32,
    pub background_schedule_pool: u32,
}

impl Threads {
    /// Without a cap: small pools that fit a small VM. With a cap of `n`
    /// CPUs: query threads match `n`, and the merge pools grow with it.
    /// The merge pool keeps at least 2 threads, the minimum the
    /// `merge_tree` entries below fit in.
    #[must_use]
    pub fn for_cpus(cpus: Option<u32>) -> Self {
        match cpus {
            None => Self { max_threads: 2, concurrent_soft_limit: 0, background_pool: 2, background_schedule_pool: 4 },
            Some(n) => Self {
                max_threads: n.max(1),
                concurrent_soft_limit: n.max(1),
                background_pool: n.clamp(2, 16),
                background_schedule_pool: n.saturating_mul(2).clamp(4, 32),
            },
        }
    }
}

/// Whole CPUs for a quota in vCPUs: rounded up, at most the CPUs this host
/// has (`None`: no cap).
#[must_use]
pub fn cpu_cap(quota_vcpu: f64) -> Option<u32> {
    if !quota_vcpu.is_finite() || quota_vcpu <= 0.0 {
        return None;
    }

    let host = std::thread::available_parallelism().map_or(1, |n| u32::try_from(n.get()).unwrap_or(u32::MAX));

    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)] // reason: validated to 0.1..=256 vCPU
    let want = quota_vcpu.ceil() as u32;
    Some(want.clamp(1, host.max(1)))
}

pub fn write_config(paths: &EmbeddedPaths, c: &EngineConfig, password: &str) -> anyhow::Result<()> {
    std::fs::create_dir_all(&paths.data).context("creating the analytics data directory")?;
    let data = xml_escape(&format!("{}/", paths.data.display()));
    let users = xml_escape(&paths.users.display().to_string());
    let bytes = u64::from(c.memory_mb.max(256)) * 1024 * 1024;
    let port = c.port;
    let th = Threads::for_cpus(c.cpus);
    let (inserts, selects) = (INSERT_SLOTS, SELECT_SLOTS);
    let queries = INSERT_SLOTS + SELECT_SLOTS;
    let (pool, sched, soft, max_threads) =
        (th.background_pool, th.background_schedule_pool, th.concurrent_soft_limit, th.max_threads);
    let config = format!(
        r#"<clickhouse>
    <logger><level>warning</level><console>1</console></logger>
    <listen_host>127.0.0.1</listen_host>
    <http_port>{port}</http_port>
    <path>{data}</path>
    <tmp_path>{data}tmp/</tmp_path>
    <user_files_path>{data}user_files/</user_files_path>
    <format_schema_path>{data}format_schemas/</format_schema_path>
    <user_directories><users_xml><path>{users}</path></users_xml></user_directories>
    <max_server_memory_usage>{bytes}</max_server_memory_usage>
    <!-- Track the engine's own RSS: it shares forge-server's cgroup, whose
         usage would otherwise count against the budget. -->
    <memory_worker_use_cgroup>0</memory_worker_use_cgroup>
    <mark_cache_size>67108864</mark_cache_size>
    <uncompressed_cache_size>0</uncompressed_cache_size>
    <concurrent_threads_soft_limit_num>{soft}</concurrent_threads_soft_limit_num>
    <background_pool_size>{pool}</background_pool_size>
    <background_schedule_pool_size>{sched}</background_schedule_pool_size>
    <!-- These must fit in background_pool_size * ratio (at least 2 * 2), or the engine refuses to start. -->
    <merge_tree>
        <number_of_free_entries_in_pool_to_execute_mutation>2</number_of_free_entries_in_pool_to_execute_mutation>
        <number_of_free_entries_in_pool_to_lower_max_size_of_merge>2</number_of_free_entries_in_pool_to_lower_max_size_of_merge>
        <number_of_free_entries_in_pool_to_execute_optimize_entire_partition>2</number_of_free_entries_in_pool_to_execute_optimize_entire_partition>
    </merge_tree>
    <!-- Inserts and searches are limited apart, so a full set of spool
         imports never makes the engine refuse a search, or the reverse. -->
    <max_concurrent_queries>{queries}</max_concurrent_queries>
    <max_concurrent_insert_queries>{inserts}</max_concurrent_insert_queries>
    <max_concurrent_select_queries>{selects}</max_concurrent_select_queries>
</clickhouse>
"#
    );
    let hash = forge_secrets::crypto::sha256_hex(password.as_bytes());
    let users_xml = format!(
        r#"<clickhouse>
    <profiles><default><max_memory_usage>{bytes}</max_memory_usage><max_threads>{max_threads}</max_threads><log_queries>0</log_queries><query_profiler_real_time_period_ns>0</query_profiler_real_time_period_ns><query_profiler_cpu_time_period_ns>0</query_profiler_cpu_time_period_ns></default></profiles>
    <users>
        <forge>
            <password_sha256_hex>{hash}</password_sha256_hex>
            <networks><ip>127.0.0.1</ip><ip>::1</ip></networks>
            <profile>default</profile>
            <quota>default</quota>
            <access_management>0</access_management>
        </forge>
    </users>
    <quotas><default/></quotas>
</clickhouse>
"#
    );
    forge_secrets::write_private(&paths.config, config.as_bytes()).context("writing config.xml")?;
    let overrides = paths.config.with_file_name("config.d");
    std::fs::create_dir_all(&overrides).context("creating the analytics config.d directory")?;
    forge_secrets::write_private(&overrides.join("forge-quiet.xml"), QUIET.as_bytes())
        .context("writing config.d/forge-quiet.xml")?;
    forge_secrets::write_private(&paths.users, users_xml.as_bytes()).context("writing users.xml")?;

    Ok(())
}

/// The URL the analytics adapter uses for the embedded engine.
#[must_use]
pub fn url(port: u16, database: &str) -> String {
    format!("http://forge@127.0.0.1:{port}/{database}")
}

/// CPU time and resident memory of the engine process.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ProcessUsage {
    pub memory_bytes: u64,
    /// CPU time of all its threads, µs.
    pub cpu_usec: u64,
}

/// Why the engine could not be pinned to its CPUs.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum PinError {
    #[error("CPU pinning is available on Linux only")]
    Unsupported,
    #[error("cannot set the analytics engine's CPU affinity: {0}")]
    Affinity(String),
}

/// The running engine: its process and restart signal. Shared by the
/// supervisor and the analytics settings service.
#[derive(Debug)]
pub struct ManagedEngine {
    pub paths: EmbeddedPaths,
    port: u16,
    /// PID of the running engine (`0`: not running).
    pid: AtomicU32,
    restart: Notify,
    /// Why the last pinning failed (`None`: it worked, or no cap is set).
    pin_error: Mutex<Option<PinError>>,
}

impl ManagedEngine {
    #[must_use]
    pub fn new(paths: EmbeddedPaths, port: u16) -> Self {
        Self { paths, port, pid: AtomicU32::new(0), restart: Notify::new(), pin_error: Mutex::new(None) }
    }

    #[must_use]
    pub fn pid(&self) -> Option<u32> {
        Some(self.pid.load(Ordering::Acquire)).filter(|p| *p != 0)
    }

    /// Asks the supervisor to stop the engine and start it again at once.
    pub fn request_restart(&self) {
        self.restart.notify_one();
    }

    fn set_pin_error(&self, e: Option<PinError>) {
        *self.pin_error.lock().unwrap_or_else(PoisonError::into_inner) = e;
    }

    /// Why the engine's CPU cap is not in effect (`None`: it is, or none is set).
    #[must_use]
    pub fn pin_error(&self) -> Option<PinError> {
        self.pin_error.lock().unwrap_or_else(PoisonError::into_inner).clone()
    }

    /// Memory and CPU time of the engine, from `/proc`. Blocking.
    #[must_use]
    pub fn usage(&self) -> Option<ProcessUsage> {
        proc_usage(self.pid()?)
    }

    /// The CPUs the running engine may use (`None`: not running or unknown). Blocking.
    #[must_use]
    pub fn allowed_cpus(&self) -> Option<u32> {
        affinity::count(self.pid()?).ok()
    }

    /// Rewrites the engine's configuration (memory budget, thread pools).
    /// The engine reloads the memory budget and query threads on its own;
    /// the pools are sized when it starts. Blocking.
    pub fn write_config(&self, t: &Tuning) -> anyhow::Result<()> {
        let pw = password(&self.paths)?;
        write_config(&self.paths, &EngineConfig::of(self.port, t), &pw)
    }

    /// Pins the running engine (every thread) to the CPUs of `t`'s cap, or
    /// lets it use all of Forge's CPUs without one. Returns the CPUs it may
    /// now use (`None`: the engine is not running; it is pinned at start).
    /// Blocking.
    pub fn pin(&self, t: &Tuning) -> Result<Option<u32>, PinError> {
        let Some(pid) = self.pid() else { return Ok(None) };
        let res = affinity::pin_process(pid, cpu_cap(t.cpu_quota_vcpu)).and_then(|()| affinity::count(pid));
        self.set_pin_error(res.as_ref().err().cloned());

        res.map(Some)
    }
}

/// CPU time (µs) and resident memory (bytes) of a process from `/proc`.
/// `None` where there is no `/proc` (Windows) or the process is gone.
#[must_use]
pub fn proc_usage(pid: u32) -> Option<ProcessUsage> {
    // `/proc/<pid>/stat` reports CPU time in USER_HZ ticks, which the
    // kernel ABI fixes at 100 per second.
    const TICK_USEC: u64 = 10_000;
    let stat = std::fs::read_to_string(format!("/proc/{pid}/stat")).ok()?;

    // Fields after the command name, which is in parentheses and may hold spaces.
    let rest = stat.get(stat.rfind(')')? + 2..)?;
    let f: Vec<&str> = rest.split_whitespace().collect();

    // utime and stime are fields 14 and 15 of the line, 12 and 13 here;
    // they cover every thread of the process.
    let ticks = f.get(11)?.parse::<u64>().ok()? + f.get(12)?.parse::<u64>().ok()?;
    let status = std::fs::read_to_string(format!("/proc/{pid}/status")).ok()?;
    let rss_kb = status
        .lines()
        .find_map(|l| l.strip_prefix("VmRSS:"))
        .and_then(|v| v.split_whitespace().next()?.parse::<u64>().ok())?;

    Some(ProcessUsage { memory_bytes: rss_kb * 1024, cpu_usec: ticks * TICK_USEC })
}

/// CPU affinity through `rustix`'s safe `sched_setaffinity`: nothing here
/// ever changes the affinity of a worker thread that keeps running.
#[cfg(target_os = "linux")]
mod affinity {
    use rustix::process::Pid;
    use rustix::thread::{CpuSet, sched_getaffinity, sched_setaffinity};

    use super::PinError;

    fn err(e: impl std::fmt::Display) -> PinError {
        PinError::Affinity(e.to_string())
    }

    /// The first `cap` CPUs Forge may use (all of them for `None`).
    pub fn cpu_set(cap: Option<u32>) -> Result<CpuSet, PinError> {
        let allowed = sched_getaffinity(None).map_err(err)?;
        let Some(cap) = cap else { return Ok(allowed) };
        let mut set = CpuSet::new();
        let take = usize::try_from(cap).unwrap_or(usize::MAX);

        for cpu in (0..CpuSet::MAX_CPU).filter(|c| allowed.is_set(*c)).take(take) {
            set.set(cpu);
        }

        Ok(set)
    }

    /// Pins the calling thread. Only for the short-lived thread that spawns
    /// the engine: the engine inherits the mask, and the thread then exits.
    pub fn pin_this_thread(cap: Option<u32>) -> Result<(), PinError> {
        if cap.is_none() {
            return Ok(());
        }

        sched_setaffinity(None, &cpu_set(cap)?).map_err(err)
    }

    fn tids(pid: u32) -> Result<Vec<i32>, PinError> {
        let dir = std::fs::read_dir(format!("/proc/{pid}/task")).map_err(err)?;
        Ok(dir.filter_map(|e| e.ok()?.file_name().to_str()?.parse().ok()).collect())
    }

    /// Pins every thread of `pid`. New threads inherit their creator's
    /// mask, so a pass that finds no new thread has pinned them all.
    pub fn pin_process(pid: u32, cap: Option<u32>) -> Result<(), PinError> {
        let set = cpu_set(cap)?;
        let mut done = std::collections::HashSet::new();

        for _ in 0..5 {
            let mut new = 0;

            for tid in tids(pid)? {
                if done.contains(&tid) {
                    continue;
                }

                let Some(p) = Pid::from_raw(tid) else { continue };

                // A thread that exited meanwhile is not an error.
                match sched_setaffinity(Some(p), &set) {
                    Ok(()) | Err(rustix::io::Errno::SRCH) => {}
                    Err(e) => return Err(err(e)),
                }

                done.insert(tid);
                new += 1;
            }

            if new == 0 {
                return Ok(());
            }
        }

        Ok(())
    }

    /// CPUs the process's main thread may use.
    pub fn count(pid: u32) -> Result<u32, PinError> {
        let p = i32::try_from(pid).ok().and_then(Pid::from_raw).ok_or_else(|| err("bad pid"))?;
        Ok(sched_getaffinity(Some(p)).map_err(err)?.count())
    }
}

#[cfg(not(target_os = "linux"))]
mod affinity {
    use super::PinError;

    pub fn pin_this_thread(cap: Option<u32>) -> Result<(), PinError> {
        if cap.is_some() { Err(PinError::Unsupported) } else { Ok(()) }
    }

    pub fn pin_process(_pid: u32, cap: Option<u32>) -> Result<(), PinError> {
        pin_this_thread(cap)
    }

    pub fn count(_pid: u32) -> Result<u32, PinError> {
        Err(PinError::Unsupported)
    }
}

/// The engine's command line. The engine's watchdog is off: by default
/// the binary forks, and the parent only waits for the child. The process
/// Forge spawns must be the engine itself, or the CPU and memory it reports
/// (`/proc/<pid>`) and the CPU pinning would apply to the idle watchdog.
fn engine_command(paths: &EmbeddedPaths) -> tokio::process::Command {
    let mut cmd = tokio::process::Command::new(&paths.binary);
    cmd.arg("server")
        .arg(format!("--config-file={}", paths.config.display()))
        .env("CLICKHOUSE_WATCHDOG_ENABLE", "0")
        .current_dir(&paths.root)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::piped())
        .kill_on_drop(true);

    cmd
}

/// Starts the engine pinned to `cap` CPUs. The spawn runs on a thread of
/// its own that pins itself first, so the engine inherits the mask from
/// its first instruction and no worker thread that keeps running is ever pinned. If
/// pinning fails the engine still starts, unpinned, and the error is
/// returned beside it.
async fn spawn_pinned(
    paths: &EmbeddedPaths,
    cap: Option<u32>,
) -> std::io::Result<(tokio::process::Child, Option<PinError>)> {
    let mut cmd = engine_command(paths);
    let handle = tokio::runtime::Handle::current();
    let (tx, rx) = tokio::sync::oneshot::channel();
    std::thread::Builder::new().name("forge-analytics-spawn".into()).spawn(move || {
        // The child is registered with the worker's runtime, which reaps it.
        let _rt = handle.enter();
        let pinned = affinity::pin_this_thread(cap).err();

        // The receiver only goes away when the supervisor stops.
        let _ = tx.send(cmd.spawn().map(|c| (c, pinned)));
    })?;

    rx.await.map_err(|_| std::io::Error::other("the spawn thread stopped"))?
}

/// Runs the engine until shutdown, restarting it with back-off when it
/// exits, and at once when asked to. Never fails the worker.
pub async fn supervise(
    engine: Arc<ManagedEngine>,
    tuning: watch::Receiver<Tuning>,
    mut shutdown: watch::Receiver<bool>,
) {
    let paths = engine.paths.clone();
    let mut backoff = Duration::from_secs(2);

    loop {
        if !tokio::fs::try_exists(&paths.binary).await.unwrap_or(false) {
            tracing::warn!(
                binary = %paths.binary.display(),
                "the embedded analytics engine is not installed; run `forge-server analytics install`"
            );
            tokio::select! {
                () = tokio::time::sleep(Duration::from_secs(60)) => continue,
                _ = shutdown.changed() => return,
            }
        }

        stale::stop(&paths).await;
        let started = std::time::Instant::now();
        let t = *tuning.borrow();

        // The thread pools in the configuration follow the CPU cap.
        let e = engine.clone();
        match tokio::task::spawn_blocking(move || e.write_config(&t)).await {
            Ok(Ok(())) => {}
            Ok(Err(err)) => {
                tracing::warn!(error = %format!("{err:#}"), "cannot rewrite the analytics engine configuration")
            }
            Err(err) => tracing::warn!(error = %err, "rewriting the analytics engine configuration panicked"),
        }

        let cap = cpu_cap(t.cpu_quota_vcpu);
        let mut child = match spawn_pinned(&paths, cap).await {
            Ok((c, pinned)) => {
                if let Some(err) = &pinned {
                    tracing::warn!(error = %err, "the analytics engine runs without its CPU cap");
                }

                engine.set_pin_error(pinned);
                c
            }
            Err(e) => {
                tracing::error!(error = %e, "cannot start the embedded analytics engine");
                tokio::select! {
                    () = tokio::time::sleep(backoff) => {}
                    _ = shutdown.changed() => return,
                }
                backoff = (backoff * 2).min(Duration::from_secs(300));

                continue;
            }
        };

        tracing::info!(cpus = ?cap, "embedded analytics engine started");
        engine.pid.store(child.id().unwrap_or(0), Ordering::Release);
        stale::record(&paths, child.id()).await;
        let mut asked = false;
        tokio::select! {
            () = engine.restart.notified() => {
                asked = true;
                tracing::info!("restarting the embedded analytics engine on request");
                let _ = child.start_kill();
                let _ = tokio::time::timeout(Duration::from_secs(30), child.wait()).await;
            }
            status = child.wait() => {
                let mut tail = String::new();
                if let Some(mut err) = child.stderr.take() {
                    use tokio::io::AsyncReadExt;
                    let mut buf = Vec::new();
                    let _ = err.read_to_end(&mut buf).await;
                    tail = String::from_utf8_lossy(&buf).lines().rev().take(3).collect::<Vec<_>>().join(" | ");
                }
                tracing::error!(status = ?status.ok(), last = %tail, "embedded analytics engine exited; restarting");
            }
            _ = shutdown.changed() => {
                let _ = child.start_kill();
                let _ = tokio::time::timeout(Duration::from_secs(10), child.wait()).await;
                engine.pid.store(0, Ordering::Release);
                stale::record(&paths, None).await;
                return;
            }
        }
        engine.pid.store(0, Ordering::Release);
        stale::record(&paths, None).await;

        if asked {
            backoff = Duration::from_secs(2);
            continue;
        }

        // Reset the back-off after a healthy run.
        if started.elapsed() > Duration::from_secs(300) {
            backoff = Duration::from_secs(2);
        }

        tokio::select! {
            () = tokio::time::sleep(backoff) => {}
            _ = shutdown.changed() => return,
        }
        backoff = (backoff * 2).min(Duration::from_secs(300));
    }
}

/// An engine left behind by a worker that was killed outright (its
/// `kill_on_drop` never ran) still holds the port. Its PID is kept in a
/// file; before starting a new engine, a process with that PID is stopped,
/// but only when its command line names the engine binary, so a reused PID
/// is never touched.
mod stale {
    use std::path::Path;
    use std::time::Duration;

    use forge_analytics_proto::config::EmbeddedPaths;

    /// Records the running engine's PID (`None`: none runs).
    pub async fn record(paths: &EmbeddedPaths, pid: Option<u32>) {
        let res = match pid {
            Some(p) => tokio::fs::write(&paths.pid_file, p.to_string()).await,
            None => match tokio::fs::remove_file(&paths.pid_file).await {
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
                other => other,
            },
        };

        if let Err(e) = res {
            tracing::warn!(error = %e, "cannot record the analytics engine's pid");
        }
    }

    /// Whether `/proc/<pid>/cmdline` names `binary`.
    fn is_engine(pid: u32, binary: &Path) -> bool {
        let Ok(cmd) = std::fs::read(format!("/proc/{pid}/cmdline")) else { return false };
        let want = binary.as_os_str().as_encoded_bytes();

        cmd.split(|b| *b == 0).next().is_some_and(|argv0| argv0 == want)
    }

    #[cfg(target_os = "linux")]
    fn kill(pid: u32) -> bool {
        let Some(p) = i32::try_from(pid).ok().and_then(rustix::process::Pid::from_raw) else { return false };
        rustix::process::kill_process(p, rustix::process::Signal::KILL).is_ok()
    }

    #[cfg(not(target_os = "linux"))]
    fn kill(_pid: u32) -> bool {
        false
    }

    /// Whether `pid` still runs (a zombie waiting to be reaped does not).
    async fn alive(pid: u32) -> bool {
        let Ok(stat) = tokio::fs::read_to_string(format!("/proc/{pid}/stat")).await else { return false };
        let state = stat.rfind(')').and_then(|i| stat.get(i + 2..i + 3));

        state.is_some_and(|s| s != "Z" && s != "X")
    }

    /// Stops a stale engine, waiting up to 10 seconds for it to go.
    pub async fn stop(paths: &EmbeddedPaths) {
        let Ok(raw) = tokio::fs::read_to_string(&paths.pid_file).await else { return };
        let Ok(pid) = raw.trim().parse::<u32>() else { return };
        let binary = paths.binary.clone();
        let found = tokio::task::spawn_blocking(move || is_engine(pid, &binary) && kill(pid)).await.unwrap_or(false);
        if !found {
            record(paths, None).await;
            return;
        }

        tracing::warn!(pid, "stopped an analytics engine left by a previous worker");
        let deadline = tokio::time::Instant::now() + Duration::from_secs(10);

        while alive(pid).await && tokio::time::Instant::now() < deadline {
            tokio::time::sleep(Duration::from_millis(100)).await;
        }

        record(paths, None).await;
    }

    #[cfg(all(test, target_os = "linux"))]
    #[allow(clippy::unwrap_used)] // reason: tests
    mod tests {
        use super::*;

        #[tokio::test]
        async fn only_a_process_running_the_engine_binary_is_stopped() {
            let dir = tempfile::tempdir().unwrap();
            let paths = EmbeddedPaths::under(dir.path(), Some("/bin/sleep".into()));
            let mut sleeper = std::process::Command::new("/bin/sleep").arg("30").spawn().unwrap();
            let mut other = std::process::Command::new("/bin/sh").args(["-c", "sleep 30"]).spawn().unwrap();
            record(&paths, Some(other.id())).await;
            stop(&paths).await;
            assert!(other.try_wait().unwrap().is_none(), "a reused pid running something else is left alone");
            record(&paths, Some(sleeper.id())).await;
            stop(&paths).await;
            let deadline = std::time::Instant::now() + Duration::from_secs(10);

            while sleeper.try_wait().unwrap().is_none() {
                assert!(std::time::Instant::now() < deadline, "the stale engine was not stopped");
                std::thread::sleep(Duration::from_millis(20));
            }

            assert!(!paths.pid_file.exists());
            other.kill().unwrap();
            other.wait().unwrap();
        }
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)] // reason: tests
mod tests {
    use super::*;

    #[test]
    fn writes_loopback_config_with_hashed_password() {
        let dir = tempfile::tempdir().unwrap();
        let p = EmbeddedPaths::under(&dir.path().join("analytics"), None);
        let pw = password(&p).unwrap();
        assert_eq!(password(&p).unwrap(), pw, "the password is kept");
        write_config(&p, &EngineConfig { port: 18123, memory_mb: 512, cpus: None }, &pw).unwrap();
        let cfg = std::fs::read_to_string(&p.config).unwrap();
        assert!(cfg.contains("<listen_host>127.0.0.1</listen_host>"));
        assert!(cfg.contains("<http_port>18123</http_port>"));
        assert!(
            cfg.contains(&format!("<max_concurrent_insert_queries>{INSERT_SLOTS}</max_concurrent_insert_queries>"))
        );
        let users = std::fs::read_to_string(&p.users).unwrap();
        assert!(!users.contains(&pw), "only the hash is stored");
        assert!(users.contains("<log_queries>0</log_queries>"));
        let quiet = std::fs::read_to_string(p.config.with_file_name("config.d").join("forge-quiet.xml")).unwrap();
        assert!(quiet.contains(r#"<trace_log remove="1"/>"#), "system logs are off in config.d");
        assert_eq!(url(18123, "forge_op_analytics"), "http://forge@127.0.0.1:18123/forge_op_analytics");
    }

    #[test]
    fn a_cpu_cap_rounds_up_and_sizes_the_thread_pools() {
        assert_eq!(cpu_cap(0.0), None);
        assert_eq!(cpu_cap(0.1), Some(1));
        let host = std::thread::available_parallelism().unwrap().get();
        assert_eq!(cpu_cap(1.5), Some(2.min(u32::try_from(host).unwrap())));
        assert_eq!(cpu_cap(256.0), Some(u32::try_from(host).unwrap()), "never more than the host has");
        assert_eq!(Threads::for_cpus(None).concurrent_soft_limit, 0);
        let t = Threads::for_cpus(Some(1));
        assert_eq!((t.max_threads, t.concurrent_soft_limit, t.background_pool), (1, 1, 2));
        let t = Threads::for_cpus(Some(8));
        assert_eq!(
            (t.max_threads, t.concurrent_soft_limit, t.background_pool, t.background_schedule_pool),
            (8, 8, 8, 16)
        );

        let dir = tempfile::tempdir().unwrap();
        let p = EmbeddedPaths::under(&dir.path().join("analytics"), None);
        write_config(&p, &EngineConfig { port: 18123, memory_mb: 512, cpus: Some(4) }, "pw").unwrap();
        let cfg = std::fs::read_to_string(&p.config).unwrap();
        assert!(cfg.contains("<max_server_memory_usage>536870912</max_server_memory_usage>"), "{cfg}");
        assert!(cfg.contains("<concurrent_threads_soft_limit_num>4</concurrent_threads_soft_limit_num>"), "{cfg}");
        assert!(cfg.contains("<background_pool_size>4</background_pool_size>"), "{cfg}");
        assert!(std::fs::read_to_string(&p.users).unwrap().contains("<max_threads>4</max_threads>"));
    }

    #[test]
    fn the_engine_runs_without_its_watchdog() {
        let dir = tempfile::tempdir().unwrap();
        let cmd = engine_command(&EmbeddedPaths::under(dir.path(), None));
        let envs: Vec<_> = cmd.as_std().get_envs().collect();
        assert!(
            envs.contains(&(std::ffi::OsStr::new("CLICKHOUSE_WATCHDOG_ENABLE"), Some(std::ffi::OsStr::new("0")))),
            "the spawned pid must be the engine, not its watchdog: {envs:?}"
        );
    }

    #[test]
    fn reads_own_process_usage() {
        let u = proc_usage(std::process::id());
        assert!(cfg!(not(target_os = "linux")) || u.is_some_and(|u| u.memory_bytes > 0));
    }

    /// Pins a child (not this process) and reads the mask back.
    #[cfg(target_os = "linux")]
    #[test]
    fn pins_another_process_without_touching_this_one() {
        let before = rustix::thread::sched_getaffinity(None).unwrap().count();
        let mut child = std::process::Command::new("sleep").arg("5").spawn().unwrap();
        affinity::pin_process(child.id(), Some(1)).unwrap();
        assert_eq!(affinity::count(child.id()).unwrap(), 1);
        affinity::pin_process(child.id(), None).unwrap();
        assert_eq!(affinity::count(child.id()).unwrap(), before, "no cap: all of Forge's CPUs");
        assert_eq!(rustix::thread::sched_getaffinity(None).unwrap().count(), before);
        child.kill().unwrap();
        child.wait().unwrap();
    }
}
