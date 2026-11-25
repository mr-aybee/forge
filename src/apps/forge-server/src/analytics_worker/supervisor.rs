//! Runs `forge-analytics-worker` as a child of forge-server (D-141).
//!
//! - Spawned with `kill_on_drop`, its stdin held open: the first line is
//!   the control secret, and the worker exits when stdin closes, so it never
//!   outlives forge-server.
//! - It is restarted with back-off when it exits, when its socket stops
//!   answering pings ([`MAX_MISSED_PINGS`] in a row), and when its resident
//!   memory passes `[analytics] worker_memory_limit_mb`. Every restart is
//!   audited (`analytics.worker.restart`) and shown on the Analytics page.
//! - A worker that speaks another protocol version is not restarted in a
//!   loop: it is marked incompatible until forge-server restarts.
//! - On shutdown stdin is closed and the worker gets [`STOP_WAIT`] to flush
//!   its buffers before it is killed.

use std::path::PathBuf;
use std::process::Stdio;
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use forge_analytics_proto::WorkerError;
use forge_analytics_proto::config::{AgentTls, PASSWORD_ENV, READY_PREFIX, WorkerConfig};
use forge_analytics_proto::tuning::Tuning;
use forge_core::model::{AuditEvent, AuditOutcome};
use forge_core::ports::AuditRecorder;
use forge_core::{Backoff, Clock, EntityRef, Timestamp};
use serde::Serialize;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::{Child, ChildStdin};
use tokio::sync::watch;

use super::client::{CallError, WorkerClient};
use super::maps::MapsPublisher;

/// Audit action of a worker restart.
pub const AUDIT_WORKER_RESTART: &str = "analytics.worker.restart";
/// Pings missed in a row before the worker is restarted.
pub const MAX_MISSED_PINGS: u32 = 3;
const CHECK_EVERY: Duration = Duration::from_secs(5);
const PING_WAIT: Duration = Duration::from_secs(5);
/// Longest wait for the worker's `ready` line.
const READY_WAIT: Duration = Duration::from_secs(30);
/// Longest wait for the worker to stop on its own.
const STOP_WAIT: Duration = Duration::from_secs(15);
/// A run this long resets the back-off.
const HEALTHY_RUN: Duration = Duration::from_secs(300);
const BACKOFF: Backoff = Backoff::new(Duration::from_secs(1), Duration::from_secs(60));

/// What the worker is doing.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum WorkerStatus {
    /// Analytics is off: no worker runs.
    Off,
    Starting,
    Running,
    /// Stopped; waiting out the back-off before the next start.
    Restarting,
    /// Speaks another protocol version (a stale binary); not restarted.
    Incompatible,
    Stopped,
}

/// Why the worker was restarted.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum RestartReason {
    /// It exited on its own (crash or error).
    Exited,
    /// Its socket stopped answering.
    Unresponsive,
    /// Its resident memory passed `worker_memory_limit_mb`.
    MemoryLimit,
    /// It could not be started or did not report ready.
    StartFailed,
}

/// The worker as the console shows it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, schemars::JsonSchema)]
pub struct WorkerState {
    pub status: WorkerStatus,
    pub pid: Option<u32>,
    /// Resident memory at the last check, bytes.
    pub rss_bytes: Option<u64>,
    /// `worker_memory_limit_mb` in bytes (`null`: no limit).
    pub memory_limit_bytes: Option<u64>,
    /// Restarts since forge-server started.
    pub restarts: u64,
    /// When the running worker started, epoch ms.
    pub started_at: Option<i64>,
    pub last_restart_at: Option<i64>,
    pub last_restart_reason: Option<RestartReason>,
    /// Why the last start failed, or why it is incompatible.
    pub last_error: Option<String>,
}

/// How to run the worker.
#[derive(Debug, Clone)]
pub struct SupervisorConfig {
    pub binary: PathBuf,
    /// Directory for the configuration file and socket (mode 0700).
    pub dir: PathBuf,
    pub config: WorkerConfig,
    /// The analytics database password, passed in the environment.
    pub password: Option<String>,
    /// Restart above this RSS, bytes (`None`: no limit).
    pub memory_limit_bytes: Option<u64>,
    /// Issues the agent listener's certificate (D-142): a fresh one at each
    /// start, handed over on stdin after the secret, and a new one sent to
    /// the running worker before it expires (D-145).
    pub listener_certs: Option<Arc<dyn ListenerCerts>>,
}

/// A certificate for the worker's agent listener with its validity.
#[derive(Debug, Clone)]
pub struct ListenerCert {
    pub tls: AgentTls,
    pub not_before: Timestamp,
    pub not_after: Timestamp,
}

/// Issues the agent listener's certificates (the Forge CA).
pub trait ListenerCerts: Send + Sync + std::fmt::Debug {
    fn issue(&self, now: Timestamp) -> Result<ListenerCert, forge_pki::PkiError>;
    /// Changes when the key certificates are issued for changes (D-167): a
    /// certificate issued under another generation is renewed at once.
    fn generation(&self) -> u64 {
        0
    }
}

/// Wiring for [`WorkerSupervisor::new`].
#[derive(Debug)]
pub struct SupervisorDeps {
    pub config: Option<SupervisorConfig>,
    pub client: Arc<WorkerClient>,
    pub maps: Arc<MapsPublisher>,
    pub audit: Arc<dyn AuditRecorder>,
    pub clock: Arc<dyn Clock>,
    pub tuning: watch::Receiver<Tuning>,
}

/// The supervisor. `config` is `None` when analytics is off.
#[derive(Debug)]
pub struct WorkerSupervisor {
    d: SupervisorDeps,
    state: Mutex<WorkerState>,
    listener_cert: ListenerCertState,
}

/// The validity of the listener certificate the running worker serves, and
/// its renewal (D-145).
#[derive(Debug, Default)]
struct ListenerCertState(Mutex<Option<(Timestamp, Timestamp, u64)>>, Mutex<Option<Timestamp>>);

/// What one renewal check did.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Renewal {
    NotDue,
    Renewed,
    /// Issuing or sending failed; tried again at the next check.
    Failed(String),
}

/// Most one renewal failure warning per this long; the rest are debug lines.
const RENEW_WARN_EVERY: i64 = 3_600_000;
/// With less than this left and no renewal, the worker is restarted, which
/// hands it a fresh certificate.
const RENEW_LAST_CHANCE: i64 = 24 * 3_600_000;

impl ListenerCertState {
    /// The worker (re)started with `cert`.
    fn started(&self, cert: Option<&ListenerCert>, generation: u64) {
        *self.0.lock().unwrap_or_else(PoisonError::into_inner) = cert.map(|c| (c.not_before, c.not_after, generation));
    }

    /// Due at two thirds of its life, like agent certificates, or at once
    /// when the key changed (D-167).
    fn due(&self, now: Timestamp, generation: u64) -> bool {
        self.0
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .is_some_and(|(nb, na, g)| g != generation || forge_pki::renewal_due(nb, na, now))
    }

    /// Less than [`RENEW_LAST_CHANCE`] left: restart the worker instead.
    fn expiring(&self, now: Timestamp) -> bool {
        self.0
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .is_some_and(|(_, na, _)| now.0 >= na.0 - RENEW_LAST_CHANCE)
    }

    /// When due, issues a new certificate and hands it to `send`; records it
    /// once sent. A failure is tried again at the next check (the current
    /// certificate still has a third of its life) and warned about at most
    /// once an hour.
    async fn renew<F, Fut, E>(&self, certs: &dyn ListenerCerts, now: Timestamp, send: F) -> Renewal
    where
        F: FnOnce(AgentTls) -> Fut,
        Fut: std::future::Future<Output = Result<(), E>>,
        E: std::fmt::Display,
    {
        let generation = certs.generation();
        if !self.due(now, generation) {
            return Renewal::NotDue;
        }

        let result = match certs.issue(now) {
            Ok(cert) => {
                let validity = (cert.not_before, cert.not_after, generation);
                send(cert.tls).await.map(|()| validity).map_err(|e| format!("sending it to the worker: {e}"))
            }
            Err(e) => Err(format!("issuing it: {e}")),
        };

        match result {
            Ok(validity) => {
                *self.0.lock().unwrap_or_else(PoisonError::into_inner) = Some(validity);
                *self.1.lock().unwrap_or_else(PoisonError::into_inner) = None;
                tracing::info!(not_after = validity.1.0, "analytics worker listener certificate renewed");
                Renewal::Renewed
            }
            Err(e) => {
                let mut warned = self.1.lock().unwrap_or_else(PoisonError::into_inner);
                if warned.is_none_or(|w| now.0 - w.0 >= RENEW_WARN_EVERY) {
                    *warned = Some(now);
                    tracing::warn!(error = %e, "renewing the analytics worker's listener certificate failed; retrying");
                } else {
                    tracing::debug!(error = %e, "renewing the analytics worker's listener certificate failed");
                }

                Renewal::Failed(e)
            }
        }
    }
}

/// How one run of the worker ended.
enum Ended {
    Restart(RestartReason, Option<String>),
    Incompatible(String),
    Shutdown,
}

/// Resident memory of `pid`, bytes (Linux; `None` elsewhere or when gone).
async fn rss(pid: u32) -> Option<u64> {
    let status = tokio::fs::read_to_string(format!("/proc/{pid}/status")).await.ok()?;
    let kb = status.lines().find_map(|l| l.strip_prefix("VmRSS:"))?.split_whitespace().next()?.parse::<u64>().ok()?;

    Some(kb * 1024)
}

impl WorkerSupervisor {
    #[must_use]
    pub fn new(d: SupervisorDeps) -> Self {
        let state = WorkerState {
            status: if d.config.is_some() { WorkerStatus::Starting } else { WorkerStatus::Off },
            pid: None,
            rss_bytes: None,
            memory_limit_bytes: d.config.as_ref().and_then(|c| c.memory_limit_bytes),
            restarts: 0,
            started_at: None,
            last_restart_at: None,
            last_restart_reason: None,
            last_error: None,
        };
        Self { d, state: Mutex::new(state), listener_cert: ListenerCertState::default() }
    }

    fn set<R>(&self, f: impl FnOnce(&mut WorkerState) -> R) -> R {
        f(&mut self.state.lock().unwrap_or_else(PoisonError::into_inner))
    }

    #[must_use]
    pub fn state(&self) -> WorkerState {
        self.set(|s| s.clone())
    }

    /// Writes the configuration file (no secrets) in a 0700 directory.
    async fn write_config(c: &SupervisorConfig) -> std::io::Result<PathBuf> {
        let (dir, path) = (c.dir.clone(), c.dir.join("worker.json"));
        let json = serde_json::to_vec_pretty(&c.config).map_err(std::io::Error::other)?;
        let p = path.clone();
        tokio::task::spawn_blocking(move || -> std::io::Result<()> {
            std::fs::create_dir_all(&dir)?;
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o700))?;
            }
            forge_secrets::write_private(&p, &json)
        })
        .await
        .map_err(std::io::Error::other)??;

        Ok(path)
    }

    /// Starts the worker; returns it, its stdin and its endpoint.
    async fn start(
        c: &SupervisorConfig,
        secret: &str,
        tls: Option<&AgentTls>,
    ) -> Result<(Child, ChildStdin, String), String> {
        let path = Self::write_config(c).await.map_err(|e| format!("writing the worker configuration: {e}"))?;
        let mut cmd = tokio::process::Command::new(&c.binary);
        cmd.arg("--config")
            .arg(&path)
            .env_remove(PASSWORD_ENV)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .kill_on_drop(true);

        if let Some(pw) = &c.password {
            cmd.env(PASSWORD_ENV, pw);
        }

        let mut child = cmd.spawn().map_err(|e| format!("cannot start {}: {e}", c.binary.display()))?;
        let mut stdin = child.stdin.take().ok_or("the worker has no stdin")?;
        let stdout = child.stdout.take().ok_or("the worker has no stdout")?;
        stdin.write_all(format!("{secret}\n").as_bytes()).await.map_err(|e| format!("handing over the secret: {e}"))?;

        if let Some(tls) = tls {
            let mut line = serde_json::to_vec(tls).map_err(|e| format!("encoding the agent listener identity: {e}"))?;
            line.push(b'\n');
            stdin.write_all(&line).await.map_err(|e| format!("handing over the agent listener identity: {e}"))?;
        }

        stdin.flush().await.map_err(|e| format!("handing over the secret: {e}"))?;
        let mut line = String::new();
        let ready = tokio::time::timeout(READY_WAIT, BufReader::new(stdout).read_line(&mut line)).await;
        match ready {
            Ok(Ok(n)) if n > 0 => {}
            Ok(_) => return Err("the worker exited before it was ready".into()),
            Err(_) => return Err("the worker did not report ready in time".into()),
        }

        let endpoint = line.trim().strip_prefix(READY_PREFIX).ok_or("the worker sent an unexpected first line")?;

        Ok((child, stdin, endpoint.to_owned()))
    }

    async fn audit_restart(&self, reason: RestartReason, detail: Option<&str>) {
        let details = serde_json::json!({ "reason": reason, "detail": detail });
        let ev = AuditEvent::new("system:forge", AUDIT_WORKER_RESTART, EntityRef::platform(), AuditOutcome::Success)
            .with_details(details);

        if let Err(e) = self.d.audit.record(ev).await {
            tracing::error!(error = %e, "audit write failed");
        }
    }

    /// Runs the worker until shutdown, restarting it as needed.
    pub async fn run(self: Arc<Self>, mut shutdown: watch::Receiver<bool>) {
        let Some(config) = self.d.config.clone() else { return };
        let mut attempt = 0u32;

        loop {
            let started = std::time::Instant::now();
            let ended = self.run_once(&config, &mut shutdown).await;
            self.d.client.disconnect();
            self.set(|s| {
                s.pid = None;
                s.rss_bytes = None;
                s.started_at = None;
            });

            let (reason, detail) = match ended {
                Ended::Shutdown => {
                    self.set(|s| s.status = WorkerStatus::Stopped);
                    return;
                }
                Ended::Incompatible(why) => {
                    tracing::error!(error = %why, "the analytics worker is incompatible; not restarting it");
                    self.set(|s| {
                        s.status = WorkerStatus::Incompatible;
                        s.last_error = Some(why);
                    });
                    let _ = shutdown.changed().await;

                    return;
                }
                Ended::Restart(reason, detail) => (reason, detail),
            };

            if started.elapsed() > HEALTHY_RUN {
                attempt = 0;
            }

            let now = self.d.clock.now().0;
            self.set(|s| {
                s.status = WorkerStatus::Restarting;
                s.restarts += 1;
                s.last_restart_at = Some(now);
                s.last_restart_reason = Some(reason);

                if detail.is_some() {
                    s.last_error.clone_from(&detail);
                }
            });
            tracing::warn!(?reason, detail = detail.as_deref().unwrap_or(""), "restarting the analytics worker");
            self.audit_restart(reason, detail.as_deref()).await;
            let wait = BACKOFF.ceiling(attempt);
            attempt = attempt.saturating_add(1);
            tokio::select! {
                () = tokio::time::sleep(wait) => {}
                _ = shutdown.changed() => {
                    self.set(|s| s.status = WorkerStatus::Stopped);
                    return;
                }
            }
        }
    }

    /// One run: start, connect, watch, stop.
    async fn run_once(&self, config: &SupervisorConfig, shutdown: &mut watch::Receiver<bool>) -> Ended {
        self.set(|s| s.status = WorkerStatus::Starting);
        let secret = forge_secrets::crypto::random_token::<32>();

        // A fresh listener certificate at every start (D-145).
        let generation = config.listener_certs.as_ref().map_or(0, |c| c.generation());

        let cert = match config.listener_certs.as_ref().map(|c| c.issue(self.d.clock.now())).transpose() {
            Ok(c) => c,
            Err(e) => return Ended::Restart(RestartReason::StartFailed, Some(format!("listener certificate: {e}"))),
        };

        self.listener_cert.started(cert.as_ref(), generation);

        let (mut child, stdin, endpoint) = match Self::start(config, &secret, cert.as_ref().map(|c| &c.tls)).await {
            Ok(x) => x,
            Err(e) => return Ended::Restart(RestartReason::StartFailed, Some(e)),
        };

        let pid = child.id();
        self.d.client.connect_to(endpoint, secret);

        match self.d.client.ping(PING_WAIT).await {
            Ok(_) => {}
            Err(CallError::Worker(e @ WorkerError::ProtocolMismatch { .. })) => {
                stop(child, stdin).await;
                return Ended::Incompatible(e.to_string());
            }
            Err(e) => {
                stop(child, stdin).await;
                return Ended::Restart(RestartReason::StartFailed, Some(e.to_string()));
            }
        }

        // The worker started with the tuning in its configuration file;
        // send the current one in case it changed since.
        let current = *self.d.tuning.borrow();
        if let Err(e) = self.d.client.set_tuning(current).await {
            tracing::warn!(error = %e, "sending the analytics settings to the worker failed");
        }

        self.d.maps.nudge();
        let now = self.d.clock.now().0;
        self.set(|s| {
            s.status = WorkerStatus::Running;
            s.pid = pid;
            s.started_at = Some(now);
        });
        tracing::info!(pid = ?pid, "analytics worker running");
        let ended = self.watch(config, &mut child, pid, shutdown).await;
        stop(child, stdin).await;

        ended
    }

    /// Sends the running worker a new listener certificate once the current
    /// one is due (D-145). A failure shows in the worker's `last_error`; with
    /// less than a day left the worker is restarted, which hands it a fresh
    /// certificate on its standard input.
    async fn renew_listener_cert(&self, config: &SupervisorConfig) -> Option<Ended> {
        let certs = config.listener_certs.as_ref()?;
        let (client, now) = (&self.d.client, self.d.clock.now());
        if let Renewal::Failed(e) = self.listener_cert.renew(certs.as_ref(), now, |tls| client.set_agent_tls(tls)).await
        {
            let e = format!("listener certificate renewal: {e}");
            self.set(|s| s.last_error = Some(e.clone()));

            if self.listener_cert.expiring(now) {
                return Some(Ended::Restart(RestartReason::Unresponsive, Some(e)));
            }
        }

        None
    }

    /// Watches a running worker until it must be restarted or shutdown.
    async fn watch(
        &self,
        config: &SupervisorConfig,
        child: &mut Child,
        pid: Option<u32>,
        shutdown: &mut watch::Receiver<bool>,
    ) -> Ended {
        let mut tuning = self.d.tuning.clone();
        let mut missed = 0;

        loop {
            tokio::select! {
                status = child.wait() => {
                    let detail = status.map_or_else(|e| e.to_string(), |s| s.to_string());
                    return Ended::Restart(RestartReason::Exited, Some(detail));
                }
                _ = shutdown.changed() => return Ended::Shutdown,
                changed = tuning.changed() => {
                    if changed.is_ok() {
                        let t = *tuning.borrow_and_update();
                        if let Err(e) = self.d.client.set_tuning(t).await {
                            tracing::warn!(error = %e, "sending the analytics settings to the worker failed");
                        }
                    }
                }
                () = tokio::time::sleep(CHECK_EVERY) => {
                    if let Some(ended) = self.renew_listener_cert(config).await {
                        return ended;
                    }
                    if self.d.client.ping(PING_WAIT).await.is_ok() {
                        missed = 0;
                    } else {
                        missed += 1;
                        if missed >= MAX_MISSED_PINGS {
                            return Ended::Restart(RestartReason::Unresponsive, Some(format!("{missed} pings unanswered")));
                        }
                    }
                    let used = match pid { Some(p) => rss(p).await, None => None };
                    self.set(|s| s.rss_bytes = used);
                    if let (Some(used), Some(limit)) = (used, config.memory_limit_bytes)
                        && used > limit
                    {
                        let mib = |b: u64| b / (1024 * 1024);
                        return Ended::Restart(
                            RestartReason::MemoryLimit,
                            Some(format!("{} MiB resident, limit {} MiB", mib(used), mib(limit))),
                        );
                    }
                }
            }
        }
    }
}

/// Closes stdin (the worker flushes and exits), then kills it after [`STOP_WAIT`].
async fn stop(mut child: Child, stdin: ChildStdin) {
    drop(stdin);

    if tokio::time::timeout(STOP_WAIT, child.wait()).await.is_err() {
        tracing::warn!("the analytics worker did not stop in time; killing it");
        let _ = child.start_kill();
        let _ = tokio::time::timeout(Duration::from_secs(5), child.wait()).await;
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)] // reason: tests
mod tests {
    use super::*;

    const DAY: i64 = 24 * 3600 * 1000;

    /// Issues 90-day certificates and counts them.
    #[derive(Debug, Default)]
    struct Fake(Mutex<u32>);

    impl ListenerCerts for Fake {
        fn issue(&self, now: Timestamp) -> Result<ListenerCert, forge_pki::PkiError> {
            let mut n = self.0.lock().unwrap();
            *n += 1;

            Ok(ListenerCert {
                tls: AgentTls { cert_pem: format!("cert {n}"), key_pem: "key".into() },
                not_before: now,
                not_after: Timestamp(now.0 + 90 * DAY),
            })
        }
    }

    /// A long-running worker gets a new certificate at two thirds of the
    /// current one's life, again for each new one, and a failed hand-over
    /// is tried again at the next check: never an expired certificate.
    #[tokio::test]
    async fn the_listener_certificate_is_renewed_before_it_expires() {
        let (certs, state) = (Fake::default(), ListenerCertState::default());
        let start = Timestamp(1_800_000_000_000);
        state.started(Some(&certs.issue(start).unwrap()), 0);
        let sent: Mutex<Vec<String>> = Mutex::new(Vec::new());
        let ok = |tls: AgentTls| {
            sent.lock().unwrap().push(tls.cert_pem);
            async { Ok::<(), String>(()) }
        };
        assert_eq!(state.renew(&certs, Timestamp(start.0 + 59 * DAY), ok).await, Renewal::NotDue);
        let due = Timestamp(start.0 + 60 * DAY);
        let failed = state.renew(&certs, due, |_| async { Err::<(), _>("worker busy") }).await;
        assert!(matches!(failed, Renewal::Failed(e) if e.contains("worker busy")));
        assert!(state.due(due, 0) && !state.expiring(due), "still due after a failed hand-over");
        assert!(state.expiring(Timestamp(start.0 + 89 * DAY)), "a day before expiry: restart for a new one");
        assert_eq!(state.renew(&certs, due, ok).await, Renewal::Renewed);
        assert_eq!(sent.lock().unwrap().as_slice(), ["cert 3"]);
        assert!(!state.due(Timestamp(due.0 + 59 * DAY), 0) && state.due(Timestamp(due.0 + 60 * DAY), 0));

        // Each renewal is checked well before its expiry, so the worker never
        // serves an expired one however long it runs.
        let mut now = due;

        for _ in 0..10 {
            now = Timestamp(now.0 + 60 * DAY);
            assert_eq!(state.renew(&certs, now, ok).await, Renewal::Renewed);
        }

        assert_eq!(sent.lock().unwrap().len(), 11);

        // A restart starts over with the certificate handed over at start.
        state.started(None, 0);
        assert!(!state.due(Timestamp(now.0 + 1_000 * DAY), 0), "no listener: nothing to renew");
    }

    /// D-167: after the control-plane key switches, the worker gets a
    /// certificate for the new key at the next check, however young the
    /// current one is.
    #[tokio::test]
    async fn a_new_key_is_handed_to_the_worker_at_once() {
        #[derive(Debug, Default)]
        struct Switching(Fake, std::sync::atomic::AtomicU64);
        impl ListenerCerts for Switching {
            fn issue(&self, now: Timestamp) -> Result<ListenerCert, forge_pki::PkiError> {
                self.0.issue(now)
            }
            fn generation(&self) -> u64 {
                self.1.load(std::sync::atomic::Ordering::SeqCst)
            }
        }
        let (certs, state) = (Switching::default(), ListenerCertState::default());
        let start = Timestamp(1_800_000_000_000);
        state.started(Some(&certs.issue(start).unwrap()), certs.generation());
        let ok = |_: AgentTls| async { Ok::<(), String>(()) };
        assert_eq!(state.renew(&certs, Timestamp(start.0 + DAY), ok).await, Renewal::NotDue);
        certs.1.store(1, std::sync::atomic::Ordering::SeqCst);
        assert_eq!(state.renew(&certs, Timestamp(start.0 + DAY), ok).await, Renewal::Renewed);
        assert_eq!(state.renew(&certs, Timestamp(start.0 + 2 * DAY), ok).await, Renewal::NotDue);
    }
}
