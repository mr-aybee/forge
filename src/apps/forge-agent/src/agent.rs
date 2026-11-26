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

/// Least time between two compose state pushes.
const STACK_PUSH_GAP: Duration = Duration::from_secs(10);
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
    /// Compose container rows last pushed (hash) and when: names follow
    /// replicas between deploys (`plans/native-mesh.md` 1.8).
    stack_push: (u64, Option<Instant>),
    /// Spool, local log listener and uploader (D-142); `None` when the
    /// spool folder cannot be opened (the agent then runs without it).
    analytics: Option<crate::node_analytics::NodeAnalytics>,
    /// The control-plane keys this agent pins (D-167); empty for an agent
    /// enrolled before, which checks the CA and host name until Forge
    /// announces its keys over that link.
    pins: crate::pins::PinSet,
    /// The pin file exists but is unreadable: every connection is refused
    /// until it is fixed (fail closed).
    pins_broken: bool,
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
        let endpoint = Endpoint::parse(&config.server_url)?;

        // Fail closed: only a missing file means "not pinned yet", and not
        // while an enrollment's pins still wait for the root helper.
        crate::pins::check_adopted(&paths.dir).map_err(|m| {
            tracing::error!("{m}");
            anyhow::anyhow!(m)
        })?;
        let pins = crate::pins::load(&paths.pins()).map_err(|e| {
            let m = crate::pins::unreadable_message(&e, &paths.pins());
            tracing::error!(error = %e, "{m}");
            anyhow::anyhow!(m)
        })?;
        let tls = tls::for_agent(
            &pins.fingerprints(),
            &endpoint.authority(),
            &std::fs::read_to_string(paths.ca())?,
            &std::fs::read_to_string(paths.cert())?,
            &std::fs::read_to_string(paths.key())?,
        )?;
        let analytics_url = config.analytics_url.clone();

        let analytics = match identity.server_id.parse::<uuid::Uuid>() {
            Ok(node) => match crate::node_analytics::NodeAnalytics::open(
                &paths.dir.join("spool"),
                node,
                analytics_url,
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

        let link = Link::new(endpoint, tls, REQUEST_TIMEOUT);
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
            stack_push: (0, None),
            analytics,
            pins,
            pins_broken: false,
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
        let (stack_states, stack_hash) = self.poll_stack_states().await.unzip();
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
            pinned_server_keys: self.pins.fingerprints(),
            stack_states,
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

        // Only a delivered push counts: a failed heartbeat resends it
        if let Some(h) = stack_hash {
            self.stack_push = (h, Some(Instant::now()));
        }

        let resp = HeartbeatResponse::decode(body.as_slice())?;
        if let Some(c) = &resp.container_telemetry {
            self.containers = c.clone();
        }

        if let Some(signed) = &resp.server_keys {
            self.apply_server_keys(signed).await;
        }

        if let Some(a) = &mut self.analytics {
            let cfg = resp.analytics.map(|mut c| {
                c.url = crate::link::analytics_url(self.link.endpoint(), &c.url, c.port);
                c
            });
            a.apply(cfg.as_ref());
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

    /// The compose container rows to push with this heartbeat and their
    /// hash, when they changed since the last delivered push and the gap
    /// has passed. The caller records the hash once the heartbeat succeeds.
    async fn poll_stack_states(&mut self) -> Option<(forge_proto::messages::StackStates, u64)> {
        if cfg!(not(unix)) || !tokio::fs::try_exists(crate::helper::SOCKET).await.unwrap_or(false) {
            return None;
        }

        if self.stack_push.1.is_some_and(|at| at.elapsed() < STACK_PUSH_GAP) {
            return None;
        }

        let r = crate::helper::call(&crate::helper::HelperRequest::StackStates {}, CONTAINER_POLL).await;

        let Some(data) = r.data.filter(|_| r.ok) else {
            tracing::debug!(error = ?r.error, "compose container states unavailable");
            return None;
        };

        let states_json = data.to_string();
        let h = states_json
            .bytes()
            .fold(0xcbf2_9ce4_8422_2325_u64, |h, b| (h ^ u64::from(b)).wrapping_mul(0x0100_0000_01b3));

        if h == self.stack_push.0 {
            return None;
        }

        Some((forge_proto::messages::StackStates { states_json }, h))
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
            // Joining a swarm waits for the managers' consensus.
            forge_proto::Command::SwarmAction(_) => true,
            // Pulls, pushes and prunes move layers.
            forge_proto::Command::DockerAction(_) => true,
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
            // Docker and Swarm commands are the same: mostly reads and small
            // changes (lists, inspects, node updates) that a person waits
            // for; pulls, pushes and joins carry on in the background.
            let quick = matches!(
                command,
                forge_proto::Command::FileAction(_)
                    | forge_proto::Command::DockerAction(_)
                    | forge_proto::Command::SwarmAction(_)
            );
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

    /// Pins the control-plane keys Forge announced (D-167). The reply came
    /// over this agent's verified link (pinned key, or the CA and host name
    /// for an agent enrolled before). Where root owns the pin file the root
    /// helper writes it after checking Forge's signature itself; otherwise
    /// the agent checks it and writes its own file.
    async fn apply_server_keys(&mut self, signed: &forge_proto::messages::SignedServerKeys) {
        let now = self.clock.now();

        let Some(public) = forge_secrets::crypto::b64url_decode(&self.identity.command_public_key) else {
            return;
        };

        let keys = match forge_proto::verify_server_keys(&public, signed, &self.identity.server_id, now) {
            Ok(k) => k,
            Err(e) => {
                tracing::warn!(error = %e, "ignoring a control-plane key announcement");
                return;
            }
        };

        // Only when something would change: most heartbeats change nothing.
        match self.pins.clone().apply_verified(&keys, now.0) {
            Ok(false) => return,
            Err(e @ (crate::pins::PinError::Stale { .. } | crate::pins::PinError::Replayed(_))) => {
                tracing::warn!(error = %e, "ignoring a control-plane key announcement");
                return;
            }
            // Let the writer report anything else (a full set, say).
            Ok(true) | Err(_) => {}
        }

        let path = self.paths.pins();

        let result = if crate::pins::root_owned(&self.paths.dir) {
            let encoded = forge_secrets::crypto::b64url(&prost::Message::encode_to_vec(signed));
            let r = crate::helper::call(
                &crate::helper::HelperRequest::ServerPins { keys: encoded },
                Duration::from_secs(10),
            )
            .await;
            if r.ok { Ok(()) } else { Err(r.error.unwrap_or_else(|| "the helper refused".into())) }
        } else {
            let (signed, server_id) = (signed.clone(), self.identity.server_id.clone());
            tokio::task::spawn_blocking(move || crate::pins::apply_signed(&path, &signed, &public, &server_id, now))
                .await
                .map_err(|e| e.to_string())
                .and_then(|r| r.map(|_| ()).map_err(|e| e.to_string()))
        };

        match result {
            Ok(()) => self.reload_pins().await,
            Err(e) => {
                tracing::warn!(error = %e, "could not pin the control-plane keys; will retry");

                // The pin file may be what failed: re-read it (fails closed).
                self.reload_pins().await;
            }
        }
    }

    /// Re-reads the pin file; when it changed, reconnects with the new set.
    async fn reload_pins(&mut self) {
        let path = self.paths.pins();
        let loaded = tokio::task::spawn_blocking(move || crate::pins::load(&path)).await;
        let set = match loaded {
            Ok(Ok(s)) => s,
            Ok(Err(e)) => {
                // Fail closed: never fall back to the CA and host name.
                let m = crate::pins::unreadable_message(&e, &self.paths.pins());
                tracing::error!(error = %e, "{m}");

                if !self.pins_broken {
                    match self.rebuild_tls(&[], true).await {
                        Ok(()) => self.pins_broken = true,
                        Err(e) => tracing::warn!(error = %e, "could not close the link after a pin-file error"),
                    }
                }

                self.note_link_error(Some(&m)).await;

                return;
            }
            Err(e) => {
                tracing::warn!(error = %e, "reading the pinned control-plane keys failed");
                return;
            }
        };

        if !self.pins_broken && set.fingerprints() == self.pins.fingerprints() {
            self.pins = set;
            return;
        }

        if let Err(e) = self.rebuild_tls(&set.fingerprints(), false).await {
            tracing::warn!(error = %e, "could not apply the pinned control-plane keys");
            return;
        }

        let shown: Vec<String> = set.fingerprints().iter().map(|f| forge_pki::short_fingerprint(f)).collect();
        tracing::info!(pins = %shown.join(", "), "control-plane key pins updated");
        self.pins = set;
        self.pins_broken = false;
    }

    /// New TLS settings for the link and the spool upload: by `pins` (CA
    /// and host name when empty), or with `closed` accepting no server.
    async fn rebuild_tls(&mut self, pins: &[String], closed: bool) -> anyhow::Result<()> {
        let (ca, cert, key) = (self.paths.ca(), self.paths.cert(), self.paths.key());
        let (ca, cert, key) = tokio::task::spawn_blocking(move || -> std::io::Result<(String, String, String)> {
            Ok((std::fs::read_to_string(ca)?, std::fs::read_to_string(cert)?, std::fs::read_to_string(key)?))
        })
        .await??;
        let endpoint = self.link.endpoint().authority();

        let tls = if closed {
            // No key is pinned, so the verifier refuses every server.
            tls::mutual_pinned(&[], &endpoint, &cert, &key)?
        } else {
            tls::for_agent(pins, &endpoint, &ca, &cert, &key)?
        };

        if let Some(a) = &self.analytics {
            a.set_tls(tls.clone());
        }

        self.link.set_tls(tls);

        Ok(())
    }

    /// Records (or with `None` clears) a link failure for `forge-agent status`.
    async fn note_link_error(&self, error: Option<&str>) {
        let path = self.paths.link_error();

        let r = match error {
            Some(e) => tokio::fs::write(&path, e).await,
            None => match tokio::fs::remove_file(&path).await {
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
                other => other,
            },
        };

        if let Err(e) = r {
            tracing::debug!(error = %e, "could not record the link status");
        }
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
        let tls = tls::for_agent(
            &self.pins.fingerprints(),
            &self.link.endpoint().authority(),
            &ca,
            &resp.cert_pem,
            &csr.key_pem,
        )?;

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
                        self.note_link_error(None).await;
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
                    let text = format!("{e:#}");
                    if text.contains(crate::pins::MISMATCH_MARK) {
                        tracing::error!(error = %text, retry_in = ?d, "the control plane presented a key this node does not pin");
                        self.note_link_error(Some(&text)).await;
                    } else {
                        tracing::warn!(error = %e, retry_in = ?d, "heartbeat failed");
                    }

                    // Pins added by hand (`forge-agent pins add`) apply at the
                    // next attempt, without re-enrolling.
                    self.reload_pins().await;
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
