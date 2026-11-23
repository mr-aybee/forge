//! The agent loop: heartbeat/long-poll, verify, execute, report, renew.

use std::sync::Arc;
use std::time::{Duration, Instant};

use anyhow::Context;
use forge_core::{Backoff, Clock, SystemClock};
use forge_proto::messages::{CommandResult, Heartbeat, HeartbeatResponse, RenewRequest, RenewResponse, SignedCommand};
use forge_proto::{CommandVerifier, paths};
use prost::Message;
use tokio::sync::{mpsc, watch};

use crate::config::{AgentConfig, AgentIdentity, StatePaths};
use crate::executor::{CommandExecutor, LocalExecutor};
use crate::link::{Endpoint, Link};
use crate::policy::LocalPolicy;
use crate::{service, tls};

/// Long-poll requests can be held by the server; allow for that.
const REQUEST_TIMEOUT: Duration = Duration::from_secs(90);
const RECONNECT: Backoff = Backoff::new(Duration::from_secs(1), Duration::from_secs(60));
const MAX_RESULTS: usize = 64;
/// How long a file operation may take and still report on the next
/// heartbeat; slower ones report later from the background.
const QUICK_WAIT: Duration = Duration::from_secs(2);
/// Longest wait for the helper's container lines before a heartbeat.
const CONTAINER_POLL: Duration = Duration::from_secs(3);

/// A running agent.
#[derive(Debug)]
pub struct Agent {
    paths: StatePaths,
    identity: AgentIdentity,
    link: Link,
    verifier: CommandVerifier,
    policy: LocalPolicy,
    executor: Arc<dyn CommandExecutor>,
    clock: SystemClock,
    pending_results: Vec<CommandResult>,
    /// Results of long-running commands finished in the background.
    background: (mpsc::UnboundedSender<CommandResult>, mpsc::UnboundedReceiver<CommandResult>),
    /// Background operations still running, by operation id (for cancel).
    running: Arc<std::sync::Mutex<std::collections::HashMap<String, tokio::task::AbortHandle>>>,
    started: Instant,
    sampler: crate::metrics::Sampler,
    /// Default health checks of the apps deployed here.
    health: crate::health::Runner,
    /// Container shells this agent holds open (D-114).
    exec_shells: crate::container_exec::ShellCount,
    /// Container log and metric collection, as the server last said.
    containers: forge_proto::messages::ContainerTelemetryConfig,
    /// The container log batch the spool holds, for the helper.
    container_ack: u64,
    /// Spool, local log listener and uploader (D-142); `None` when the
    /// spool folder cannot be opened (the agent then runs without it).
    analytics: Option<crate::node_analytics::NodeAnalytics>,
}

impl Agent {
    /// A self-update is waiting for this agent to confirm it (issue #4).
    fn paths_marker_exists(&self) -> bool {
        crate::self_update::marker(&self.paths.dir).exists()
    }

    /// Loads an enrolled agent from its state directory.
    pub fn load(paths: StatePaths) -> anyhow::Result<Self> {
        let config: AgentConfig = paths.load_config().context("reading agent.toml")?;
        let identity = paths.load_identity().context("reading identity.json (is the agent enrolled?)")?;
        let dir = paths.dir.clone();
        let agent = Self::from_parts(paths, config, identity, Box::new(LocalExecutor::new(service::platform())))?;
        // Default health checks of the apps deployed here (D-085). The loop
        // starts on the first heartbeat, inside the async runtime.
        crate::health::init(&dir);
        Ok(agent)
    }

    /// Builds an agent with an injected executor (used by tests and fleet-sim).
    pub fn from_parts(
        paths: StatePaths,
        config: AgentConfig,
        identity: AgentIdentity,
        executor: Box<dyn CommandExecutor>,
    ) -> anyhow::Result<Self> {
        let tls = tls::mutual(
            &std::fs::read_to_string(paths.ca())?,
            &std::fs::read_to_string(paths.cert())?,
            &std::fs::read_to_string(paths.key())?,
        )?;
        let analytics = match identity.server_id.parse::<uuid::Uuid>() {
            Ok(node) => match crate::node_analytics::NodeAnalytics::open(
                &paths.dir.join("spool"),
                node,
                config.analytics_url.clone(),
                tls.clone(),
            ) {
                Ok(a) => Some(a),
                Err(e) => {
                    tracing::error!(error = %e, "the analytics spool cannot be opened; node data is not collected");
                    None
                }
            },
            Err(_) => None,
        };
        let link = Link::new(Endpoint::parse(&config.server_url)?, tls, REQUEST_TIMEOUT);
        let public_key =
            forge_secrets::crypto::b64url_decode(&identity.command_public_key).context("bad command key")?;
        let clock = SystemClock;
        let verifier = CommandVerifier::new(public_key, identity.server_id.clone(), clock.now());
        Ok(Self {
            policy: LocalPolicy::new(&config.allowed_commands),
            paths,
            identity,
            link,
            verifier,
            executor: Arc::from(executor),
            clock,
            background: mpsc::unbounded_channel(),
            running: Arc::default(),
            pending_results: Vec::new(),
            started: Instant::now(),
            sampler: crate::metrics::Sampler::new(),
            health: crate::health::Runner::default(),
            exec_shells: crate::container_exec::ShellCount::default(),
            // On until a server says otherwise.
            containers: forge_proto::messages::ContainerTelemetryConfig { enabled: true, exclude: Vec::new() },
            container_ack: 0,
            analytics,
        })
    }

    #[must_use]
    pub fn server_id(&self) -> &str {
        &self.identity.server_id
    }

    /// One heartbeat round trip. Returns how long to wait before the next one.
    pub async fn heartbeat(&mut self) -> anyhow::Result<Duration> {
        while let Ok(r) = self.background.1.try_recv() {
            self.pending_results.push(r);
        }
        let results: Vec<CommandResult> = std::mem::take(&mut self.pending_results);
        let containers = self.poll_containers().await;
        let mut metrics = self.sampler.sample();
        let now = crate::node_analytics::now_ms();
        if let Some(a) = &mut self.analytics {
            a.ensure_started();
            a.record_node(&metrics, now).await;
            if let Some(t) = &containers {
                self.container_ack = a.record_containers(t, now).await;
            }
        }
        // History, top processes and container data go to the spool; the
        // heartbeat keeps the live sample only.
        metrics.top_processes.clear();
        let spool = self.analytics.as_ref().map(crate::node_analytics::NodeAnalytics::status);
        let hb = Heartbeat {
            agent_version: crate::VERSION.into(),
            uptime_secs: self.started.elapsed().as_secs(),
            rss_bytes: current_rss(),
            results: results.clone(),
            metrics: Some(metrics),
            progress: crate::progress::drain(),
            streaming: crate::progress::active(),
            health: {
                // Checks with vault headers ask Forge over the same identity.
                self.health.set_control(self.link.endpoint(), self.link.tls());
                self.health.ensure_started();
                self.health.drain()
            },
            container_logs: None,
            spool,
        };
        let body = match self.link.request(http::Method::POST, paths::HEARTBEAT, hb.encode_to_vec()).await {
            Ok(b) => b,
            Err(e) => {
                // Keep results for the next attempt (bounded).
                self.pending_results = results;
                self.pending_results.truncate(MAX_RESULTS);
                return Err(e);
            }
        };
        let resp = HeartbeatResponse::decode(body.as_slice())?;
        if let Some(c) = &resp.container_telemetry {
            self.containers = c.clone();
        }
        if let Some(a) = &mut self.analytics {
            a.apply(resp.analytics.as_ref());
        }
        for cmd in &resp.commands {
            if let Some(result) = self.handle(cmd).await {
                self.pending_results.push(result);
            }
        }
        self.pending_results.truncate(MAX_RESULTS);
        if resp.renew_certificate
            && let Err(e) = self.renew().await
        {
            tracing::warn!(error = %e, "certificate renewal failed; will retry");
        }
        // Report results immediately; otherwise follow the server's pacing.
        if !self.pending_results.is_empty() || crate::progress::active() {
            return Ok(Duration::ZERO);
        }
        Ok(jitter(Duration::from_secs(u64::from(resp.next_interval_secs))))
    }

    /// Asks the helper for container lines and samples, acknowledging the
    /// batch the spool holds. Nothing when the helper is not running or
    /// cannot collect (no Docker, Windows): collection is best effort.
    async fn poll_containers(&mut self) -> Option<crate::container_telemetry::TelemetryResponse> {
        use crate::container_telemetry::{TelemetryRequest, TelemetryResponse};
        if cfg!(not(unix)) || !tokio::fs::try_exists(crate::helper::SOCKET).await.unwrap_or(false) {
            return None;
        }
        let req = crate::helper::HelperRequest::ContainerTelemetry {
            telemetry: TelemetryRequest {
                enabled: self.containers.enabled,
                exclude: self.containers.exclude.clone(),
                ack: std::mem::take(&mut self.container_ack),
            },
        };
        let r = crate::helper::call(&req, CONTAINER_POLL).await;
        let data = match (r.ok, r.data) {
            (true, Some(d)) => d,
            _ => {
                tracing::debug!(error = ?r.error, "container telemetry unavailable");
                return None;
            }
        };
        match serde_json::from_value::<TelemetryResponse>(data) {
            Ok(t) => Some(t),
            Err(e) => {
                tracing::warn!(error = %e, "unreadable container telemetry from the helper");
                None
            }
        }
    }

    /// Verifies, checks policy, executes. Returns `None` for commands whose
    /// signature fails: their operation id cannot be trusted.
    async fn handle(&self, cmd: &SignedCommand) -> Option<CommandResult> {
        let body = match self.verifier.verify(cmd, self.clock.now()) {
            Ok(b) => b,
            Err(e) => {
                tracing::warn!(error = %e, "rejected command");
                return None;
            }
        };
        let command = body.command.as_ref()?;
        let fail = |error: String| CommandResult {
            operation_id: body.operation_id.clone(),
            ok: false,
            output: String::new(),
            error,
            error_code: String::new(),
        };
        if let Err(e) = self.policy.check(command) {
            tracing::warn!(op = %body.operation_id, principal = %body.principal, "{e}");
            // An update this node's policy does not allow needs a reinstall
            // or a policy change: the control plane skips the node.
            if matches!(command, forge_proto::Command::AgentUpdate(_)) {
                return Some(CommandResult {
                    error_code: forge_proto::messages::RefusalCode::Unsupported.as_str().into(),
                    ..fail(e)
                });
            }
            return Some(fail(e));
        }
        tracing::info!(op = %body.operation_id, command = command.name(), principal = %body.principal, "executing command");
        let to_result = |r: Result<serde_json::Value, String>, op: String| match r {
            Ok(v) => CommandResult { operation_id: op, ok: true, output: v.to_string(), ..Default::default() },
            Err(e) => CommandResult { operation_id: op, ok: false, error: e, ..Default::default() },
        };
        let long_running = match command {
            forge_proto::Command::PackageAction(_) => true,
            forge_proto::Command::ContainerAction(c) => c.op == "pull",
            forge_proto::Command::AppAction(a) => matches!(a.action.as_str(), "deploy" | "follow"),
            forge_proto::Command::SiteAction(s) => matches!(s.action.as_str(), "apply" | "renew"),
            // File operations range from a listing to a 1 GiB copy or a 50 s
            // search: never let them hold up heartbeats or other commands.
            forge_proto::Command::FileAction(_) => true,
            // Object store installs wait for downloads and quorum.
            forge_proto::Command::StorageAction(_) => true,
            // Compose deploys pull images and wait for health checks.
            forge_proto::Command::StackAction(_) => true,
            // Restarting dockerd can take a minute.
            forge_proto::Command::DockerTlsAction(_) => true,
            _ => false,
        };
        // Stop a running background operation: its task is dropped, which
        // closes the helper connection, and the helper kills the recipe's
        // whole process group.
        // Port tunnels (D-091) run in the agent itself: dial a local port,
        // then stream to the control plane. In the background, so a slow dial
        // never holds up other commands; the result follows on a heartbeat.
        if let forge_proto::Command::TunnelOpen(t) = command {
            let (t, endpoint, tls, tx, op) = (
                t.clone(),
                self.link.endpoint().clone(),
                self.link.tls(),
                self.background.0.clone(),
                body.operation_id,
            );
            tokio::spawn(async move {
                let r = crate::tunnel::open(t, endpoint, tls).await;
                if let Err(e) = &r {
                    tracing::warn!(op = %op, error = %e, "tunnel connection refused");
                }
                let _ = tx.send(to_result(r, op));
            });
            return None;
        }
        // Container shells (D-114): checked here, then the helper starts the
        // exec and the agent dials back in the background.
        if let forge_proto::Command::ContainerExecOpen(x) = command {
            let checked = crate::container_exec::validate_open(x, crate::tunnel::now_ms())
                .and_then(|_| crate::container_exec::Slot::take(&self.exec_shells, crate::container_exec::MAX_EXEC));
            let slot = match checked {
                Ok(s) => s,
                Err(e) => return Some(fail(e)),
            };
            let (x, endpoint, tls, tx, op, exec, signed) = (
                x.clone(),
                self.link.endpoint().clone(),
                self.link.tls(),
                self.background.0.clone(),
                body.operation_id,
                self.executor.clone(),
                cmd.clone(),
            );
            tokio::spawn(async move {
                let pipe = exec.open_exec(&signed).await;
                let r = crate::exec_stream::run(x, pipe, slot, endpoint, tls).await;
                if let Err(e) = &r {
                    tracing::warn!(op = %op, error = %e, "container shell not opened");
                }
                let _ = tx.send(to_result(r, op));
            });
            return None;
        }
        if let forge_proto::Command::CancelAction(c) = command {
            let handle = self.running.lock().ok().and_then(|mut r| r.remove(&c.operation_id));
            let found = handle.is_some();
            if let Some(h) = handle {
                h.abort();
                let _ = self.background.0.send(CommandResult {
                    operation_id: c.operation_id.clone(),
                    ok: false,
                    output: String::new(),
                    error: "cancelled".into(),
                    error_code: String::new(),
                });
            }
            return Some(to_result(Ok(serde_json::json!({ "cancelled": found })), body.operation_id));
        }
        if long_running {
            // Installs take minutes: run them in the background so heartbeats
            // (and other commands) keep flowing; report on a later heartbeat.
            let (exec, tx, cmd, op, signed) =
                (self.executor.clone(), self.background.0.clone(), command.clone(), body.operation_id, cmd.clone());
            let running = self.running.clone();
            let key = op.clone();
            // File operations are mostly quick (an upload chunk, a read, a
            // listing): wait briefly so the answer goes back on the very next
            // heartbeat instead of after the server's hold. A transfer is a
            // chain of such operations, so this sets its speed (issue #15).
            // Anything slower carries on in the background as before.
            let quick = matches!(command, forge_proto::Command::FileAction(_));
            let (quick_tx, quick_rx) = tokio::sync::oneshot::channel::<CommandResult>();
            let task = tokio::spawn(async move {
                let r = crate::progress::scope(op.clone(), exec.execute(&cmd, &signed)).await;
                if let Ok(mut m) = running.lock() {
                    m.remove(&op);
                }
                let result = to_result(r, op);
                // Nobody waits any more (timed out, or not a quick one).
                if let Err(result) = quick_tx.send(result) {
                    let _ = tx.send(result);
                }
            });
            if let Ok(mut m) = self.running.lock() {
                m.retain(|_, h| !h.is_finished());
                m.insert(key, task.abort_handle());
            }
            if quick && let Ok(Ok(result)) = tokio::time::timeout(QUICK_WAIT, quick_rx).await {
                return Some(result);
            }
            return None;
        }
        if matches!(command, forge_proto::Command::AgentUpdate(_)) {
            let r = self.executor.execute_coded(command, cmd).await;
            return Some(match r {
                Ok(v) => to_result(Ok(v), body.operation_id),
                Err(e) => CommandResult {
                    operation_id: body.operation_id,
                    ok: false,
                    error: e.message,
                    error_code: e.code.map(|c| c.as_str().to_owned()).unwrap_or_default(),
                    ..Default::default()
                },
            });
        }
        Some(to_result(self.executor.execute(command, cmd).await, body.operation_id))
    }

    /// Rotates the client certificate with a new key.
    async fn renew(&mut self) -> anyhow::Result<()> {
        let hostname = sysinfo::System::host_name().unwrap_or_else(|| "agent".into());
        let csr = forge_pki::AgentCsr::generate(&hostname)?;
        let body = self
            .link
            .request(http::Method::POST, paths::RENEW, RenewRequest { csr_pem: csr.csr_pem }.encode_to_vec())
            .await?;
        let resp = RenewResponse::decode(body.as_slice())?;
        self.paths.save_credentials(&csr.key_pem, &resp.cert_pem)?;
        let ca = std::fs::read_to_string(self.paths.ca())?;
        let tls = tls::mutual(&ca, &resp.cert_pem, &csr.key_pem)?;
        if let Some(a) = &self.analytics {
            a.set_tls(tls.clone());
        }
        self.link.set_tls(tls);
        tracing::info!("agent certificate renewed");
        Ok(())
    }

    /// Runs until `shutdown` is set. Reconnects with exponential backoff and
    /// full jitter so a fleet does not stampede a restarted control plane.
    pub async fn run(mut self, mut shutdown: watch::Receiver<bool>) {
        let mut failures: u32 = 0;
        let mut confirmed = false;
        loop {
            if *shutdown.borrow() {
                break;
            }
            // A stop ends a heartbeat in flight too (a slow server or a long
            // command), so the Windows service and systemd stop promptly.
            // Work cut short this way is what a crash would cut short; job
            // steps are idempotent.
            let beat = tokio::select! {
                biased;
                _ = shutdown.changed() => break,
                r = self.heartbeat() => r,
            };
            let wait = match beat {
                Ok(next) => {
                    if failures > 0 {
                        tracing::info!("reconnected to control plane");
                    }
                    // After a self-update: we reached the control plane, so
                    // tell the helper to keep this version (issue #4).
                    if !confirmed && self.paths_marker_exists() {
                        let r = crate::helper::call(
                            &crate::helper::HelperRequest::ConfirmUpdate {},
                            Duration::from_secs(10),
                        )
                        .await;
                        // Only the new helper confirms, and only for this
                        // agent's version: an old helper still running
                        // keeps its watchdog armed.
                        if r.ok
                            && r.data.as_ref().is_some_and(|d| d["confirmed"] == true && d["version"] == crate::VERSION)
                        {
                            tracing::info!(version = crate::VERSION, "self-update confirmed");
                            confirmed = true;
                        }
                    } else {
                        confirmed = true;
                    }
                    failures = 0;
                    next
                }
                Err(e) => {
                    let d = RECONNECT.delay(failures, unit_random());
                    failures = failures.saturating_add(1);
                    tracing::warn!(error = %e, retry_in = ?d, "heartbeat failed");
                    d
                }
            };
            tokio::select! {
                () = tokio::time::sleep(wait) => {}
                _ = shutdown.changed() => {}
            }
        }
        tracing::info!("agent stopped");
    }
}

/// ±10 % jitter around `d`.
fn jitter(d: Duration) -> Duration {
    d.mul_f64(0.9 + 0.2 * unit_random())
}

pub(crate) fn unit_random() -> f64 {
    let b = forge_secrets::crypto::random_bytes::<8>();
    #[allow(clippy::cast_precision_loss)]
    let v = (u64::from_le_bytes(b) >> 11) as f64 / (1u64 << 53) as f64;
    v
}

/// Resident set size of this process, in bytes.
fn current_rss() -> u64 {
    use sysinfo::{ProcessRefreshKind, ProcessesToUpdate, System};
    let Ok(pid) = sysinfo::get_current_pid() else { return 0 };
    let mut sys = System::new();
    sys.refresh_processes_specifics(ProcessesToUpdate::Some(&[pid]), true, ProcessRefreshKind::nothing().with_memory());
    sys.process(pid).map_or(0, sysinfo::Process::memory)
}
