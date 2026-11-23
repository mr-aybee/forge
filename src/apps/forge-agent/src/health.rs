//! Health checks of the apps on this node (issue #12, D-085).
//!
//! When the agent forwards a deploy to the helper it saves a small
//! descriptor of the app (no environment, no secrets) under
//! `<state>/health/`, with the app's health policy. The signed app command
//! `health` replaces the policy without a redeploy ([`apply`]). A background
//! loop runs Forge's default check of every described app every [`PERIOD`],
//! and each configured HTTP or TCP check at its own period
//! ([`crate::health_probe`]); each heartbeat carries the changes plus a
//! keep-alive per check at half its freshness.
//!
//! Default checks (no configuration needed):
//! - native service: the systemd unit is active, plus a TCP connect to its
//!   `listen_port` on 127.0.0.1 when one is set and no other app on this node
//!   declares the same port;
//! - container: running, and its image `HEALTHCHECK` is not `unhealthy`.
//!
//! Readiness reports the same as liveness unless the policy configures
//! readiness checks. Jobs have no default check.
//!
//! Configured checks: startup checks run until the first success
//! (`succeeded`) or until they failed `failure_threshold` times
//! (`exhausted`), once per workload generation; readiness and liveness
//! checks stay `pending` until every startup check succeeded.
//!
//! Exec and shell checks are run by the node's helper, as the app's own
//! account or in its container ([`crate::health_exec`], D-111): the agent
//! names the check, the helper runs what Forge signed. Headers with a vault
//! reference get their value from Forge just before the probe
//! ([`crate::health_secrets`]). A check that can't be run that way (no
//! Landlock, not Linux, Forge refuses the value) reports `unknown` with the
//! reason; `unknown` is not a failure and a startup check in that state
//! holds nothing back.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

use forge_catalog::apps::{AppSpec, Runtime};
use forge_catalog::health::{ApplySpec, Check, HealthPolicy, Purpose};
use forge_proto::messages::{HealthReport, MAX_HEALTH_REPORTS};
use serde::{Deserialize, Serialize};

/// Time between checks of one app.
pub const PERIOD: Duration = Duration::from_secs(10);
/// Consecutive failures before a check turns failing.
pub const FAILURE_THRESHOLD: u32 = 3;
/// TCP connect timeout.
const CONNECT_TIMEOUT: Duration = Duration::from_secs(2);
/// Checks running at once on this node.
const MAX_CONCURRENT: usize = 8;
/// How long a reported value counts as current: three periods, but never
/// less than about three long-poll holds, so reports arriving with the next
/// heartbeat are still fresh.
pub const FRESH_FOR: Duration = Duration::from_secs(90);

static DIR: OnceLock<PathBuf> = OnceLock::new();
/// A descriptor was written: the loop reloads them at its next tick.
static CHANGED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// What the node needs to check one app: no environment, no secrets.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Descriptor {
    pub environment: String,
    pub name: String,
    pub runtime: Runtime,
    pub listen_port: Option<u16>,
    pub default_checks: bool,
    /// Readiness checks are configured, so the default doesn't report readiness.
    pub has_readiness: bool,
    /// The configured checks (absent in descriptors of older agents).
    #[serde(default)]
    pub policy: HealthPolicy,
    /// The policy revision Forge sent with [`apply`]; 0 after a deploy.
    #[serde(default)]
    pub revision: i64,
    /// Addresses of the fleet's other nodes, as Forge sent them with the
    /// policy: never a probe target.
    #[serde(default)]
    pub fleet: Vec<std::net::IpAddr>,
    /// The copy checked (#73): `a`/`b`, or empty for the single instance.
    #[serde(default)]
    pub slot: String,
}

impl Descriptor {
    #[must_use]
    pub fn from_spec(spec: &AppSpec) -> Self {
        let health = spec.health.clone().unwrap_or_default();
        Self {
            environment: spec.environment.clone(),
            name: spec.name.clone(),
            runtime: spec.runtime,
            listen_port: spec.listen_port,
            default_checks: health.default_checks,
            has_readiness: !health.readiness.is_empty(),
            policy: health,
            revision: 0,
            fleet: Vec::new(),
            slot: spec.slot.clone(),
        }
    }

    fn from_apply(mut a: ApplySpec) -> Self {
        a.fleet.truncate(forge_catalog::health::MAX_FLEET_ADDRESSES);
        Self {
            environment: a.environment,
            name: a.name,
            runtime: a.runtime,
            listen_port: a.listen_port,
            default_checks: a.policy.default_checks,
            has_readiness: !a.policy.readiness.is_empty(),
            policy: a.policy,
            revision: a.revision,
            fleet: a.fleet,
            slot: a.slot,
        }
    }

    fn key(&self) -> String {
        format!("{}-{}", self.environment, self.name)
    }

    /// systemd unit or container name.
    fn unit(&self) -> String {
        if self.slot.is_empty() {
            format!("forge-app-{}", self.key())
        } else {
            format!("forge-app-{}--{}", self.key(), self.slot)
        }
    }
}

/// Sets the descriptor directory (`<state>/health`); called once at start.
pub fn init(state_dir: &Path) {
    let _ = DIR.set(state_dir.join("health"));
}

/// Where the descriptors are kept, once [`init`] ran. One per process: the
/// first agent started in it decides (integration tests run several).
#[must_use]
pub fn descriptor_dir() -> Option<&'static Path> {
    DIR.get().map(PathBuf::as_path)
}

fn file_of(dir: &Path, environment: &str, name: &str) -> PathBuf {
    // Slugs are `[a-z0-9-]`, so `--` can't occur inside either part.
    dir.join(format!("{environment}--{name}.json"))
}

/// Records the app after a successful deploy (best effort: health checking
/// must never fail a deploy).
pub fn on_deployed(spec: &AppSpec) {
    let Some(dir) = DIR.get() else { return };
    let mut d = Descriptor::from_spec(spec);
    // A deploy carries no fleet list: keep the one Forge sent last, until
    // the policy arrives again.
    if let Some(before) = load_one(dir, &d.environment, &d.name) {
        d.fleet = before.fleet;
    }
    if let Err(e) = save(dir, &d) {
        tracing::warn!(app = %d.key(), error = %e, "could not save the health descriptor");
    }
}

fn save(dir: &Path, d: &Descriptor) -> std::io::Result<()> {
    std::fs::create_dir_all(dir)?;
    let tmp = dir.join(format!(".{}.tmp", d.key()));
    std::fs::write(&tmp, serde_json::to_vec(d).unwrap_or_default())?;
    std::fs::rename(tmp, file_of(dir, &d.environment, &d.name))?;
    CHANGED.store(true, std::sync::atomic::Ordering::SeqCst);
    Ok(())
}

/// The signed app command `health`: replaces the health policy of an app
/// that is deployed on this node, without a redeploy. `spec_json` is a
/// [`ApplySpec`]; the policy is validated again here.
pub fn apply(spec_json: &str) -> Result<serde_json::Value, String> {
    let dir = DIR.get().ok_or("health checks are not set up on this agent")?;
    apply_in(dir, spec_json)
}

fn apply_in(dir: &Path, spec_json: &str) -> Result<serde_json::Value, String> {
    let a: ApplySpec = serde_json::from_str(spec_json).map_err(|e| format!("invalid health spec: {e}"))?;
    if !forge_catalog::apps::valid_slug(&a.environment, 40) || !forge_catalog::apps::valid_slug(&a.name, 40) {
        return Err("invalid app name".into());
    }
    if let Some(e) = a.policy.validate().first() {
        return Err(format!("invalid health policy: {}: {}", e.field, e.message));
    }
    if !file_of(dir, &a.environment, &a.name).exists() {
        return Err(format!("{}/{} is not deployed on this node", a.environment, a.name));
    }
    let d = Descriptor::from_apply(a);
    save(dir, &d).map_err(|e| format!("saving the health policy: {e}"))?;
    Ok(serde_json::json!({ "applied": true, "revision": d.revision }))
}

/// Whether the policy in `spec_json` has exec or shell checks: the node's
/// helper must hold it before the agent runs it (D-111). `false` for a
/// spec that doesn't parse; [`apply`] reports that.
#[must_use]
pub fn runs_code(spec_json: &str) -> bool {
    serde_json::from_str::<ApplySpec>(spec_json).is_ok_and(|a| a.policy.runs_code())
}

/// Whether the policy in `spec_json` may be applied only if the node's
/// helper took it too: it has exec or shell checks, or the policy it
/// replaces had some. In the second case the helper still holds command
/// checks that the new policy removes; applying it here alone would leave
/// them runnable there.
#[must_use]
pub fn helper_must_take(spec_json: &str) -> bool {
    let Some(dir) = DIR.get() else { return runs_code(spec_json) };
    helper_must_take_in(dir, spec_json)
}

fn helper_must_take_in(dir: &Path, spec_json: &str) -> bool {
    let Ok(new) = serde_json::from_str::<ApplySpec>(spec_json) else { return false };
    new.policy.runs_code() || load_one(dir, &new.environment, &new.name).is_some_and(|held| held.policy.runs_code())
}

/// The saved descriptor of one app, if there is a readable one.
fn load_one(dir: &Path, environment: &str, name: &str) -> Option<Descriptor> {
    serde_json::from_slice(&std::fs::read(file_of(dir, environment, name)).ok()?).ok()
}

/// Forgets the app after it was removed.
pub fn on_removed(spec: &AppSpec) {
    if let Some(dir) = DIR.get() {
        let _ = std::fs::remove_file(file_of(dir, &spec.environment, &spec.name));
    }
}

fn load_all(dir: &Path) -> Vec<Descriptor> {
    let Ok(entries) = std::fs::read_dir(dir) else { return Vec::new() };
    entries
        .filter_map(Result::ok)
        .filter(|e| e.path().extension().is_some_and(|x| x == "json"))
        .filter_map(|e| serde_json::from_slice::<Descriptor>(&std::fs::read(e.path()).ok()?).ok())
        // The agent wrote these itself, but a damaged or planted file must not
        // name a unit or container outside the app's own (orge-app-<ns>-<app>).
        .filter(|d| forge_catalog::apps::valid_slug(&d.environment, 40) && forge_catalog::apps::valid_slug(&d.name, 40))
        .collect()
}

/// Ports declared by more than one app on this node.
#[must_use]
pub fn ambiguous_ports(apps: &[Descriptor]) -> Vec<u16> {
    let mut count: HashMap<u16, usize> = HashMap::new();
    for p in apps.iter().filter(|a| a.runtime == Runtime::Systemd).filter_map(|a| a.listen_port) {
        *count.entry(p).or_default() += 1;
    }
    let mut out: Vec<u16> = count.into_iter().filter(|(_, n)| *n > 1).map(|(p, _)| p).collect();
    out.sort_unstable();
    out
}

/// One attempt's outcome.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Attempt {
    pub ok: bool,
    pub reason: String,
    pub detail: String,
    pub generation: String,
}

/// The result kept per app and purpose, and what was last reported.
#[derive(Debug, Clone)]
struct Tracked {
    state: &'static str,
    reason: String,
    detail: String,
    generation: String,
    failures: u32,
    successes: u32,
    observed_at_ms: i64,
    /// How long a report of this check counts as current.
    fresh_for: Duration,
    /// The policy revision the check ran under.
    revision: i64,
    reported: Option<(&'static str, String, Instant)>,
    /// `unknown` because the node will not run the check until someone
    /// changes the node or the app: a startup check in this state holds
    /// nothing back.
    permanent: bool,
    /// Periods in a row the node had no place for the check.
    throttled: u32,
}

/// Periods in a row a check may find no free place before that counts as a
/// failed attempt (`timeout`): a check that never runs must not look
/// neutral for ever.
const MAX_THROTTLED: u32 = 3;

/// Thresholds of one check.
#[derive(Debug, Clone, Copy)]
struct Thresholds {
    failures: u32,
    successes: u32,
}

/// The default check's thresholds.
const DEFAULT_THRESHOLDS: Thresholds = Thresholds { failures: FAILURE_THRESHOLD, successes: 1 };

impl Tracked {
    const fn new() -> Self {
        Self {
            state: "pending",
            reason: String::new(),
            detail: String::new(),
            generation: String::new(),
            failures: 0,
            successes: 0,
            observed_at_ms: 0,
            fresh_for: FRESH_FOR,
            revision: 0,
            reported: None,
            permanent: false,
            throttled: 0,
        }
    }

    /// Applies one attempt with the thresholds; a new generation starts
    /// counting afresh.
    fn apply(&mut self, a: Attempt, now_ms: i64, t: Thresholds) {
        if a.generation != self.generation {
            self.failures = 0;
            self.successes = 0;
            self.generation = a.generation;
        }
        if a.ok {
            self.failures = 0;
            self.successes = self.successes.saturating_add(1);
            if self.successes >= t.successes {
                self.state = "passing";
            }
        } else {
            self.successes = 0;
            self.failures = self.failures.saturating_add(1);
            if self.failures >= t.failures {
                self.state = "failing";
            }
        }
        // A pass can carry a note (`ambiguous_port`: only the unit was
        // checked); keep it so it reaches the server.
        self.reason = a.reason;
        self.detail = a.detail;
        self.observed_at_ms = now_ms;
    }

    /// Applies one attempt of a startup check: `waiting` until the first
    /// success (`succeeded`) or `failures` failed attempts (`exhausted`).
    /// Both are final for the generation; a new generation starts again.
    fn apply_startup(&mut self, a: Attempt, now_ms: i64, t: Thresholds) {
        if a.generation != self.generation || !matches!(self.state, "waiting" | "succeeded" | "exhausted") {
            self.failures = 0;
            self.state = "waiting";
            self.generation = a.generation;
        }
        if matches!(self.state, "succeeded" | "exhausted") {
            return;
        }
        if a.ok {
            self.state = "succeeded";
        } else {
            self.failures = self.failures.saturating_add(1);
            if self.failures >= t.failures {
                self.state = "exhausted";
            }
        }
        self.reason = a.reason;
        self.detail = a.detail;
        self.observed_at_ms = now_ms;
    }

    /// A state that needs no attempt (`disabled`, `pending`, `unknown`).
    fn set(&mut self, state: &'static str, reason: &str, detail: &str, now_ms: i64) {
        self.state = state;
        reason.clone_into(&mut self.reason);
        detail.clone_into(&mut self.detail);
        self.failures = 0;
        self.successes = 0;
        self.observed_at_ms = now_ms;
    }
}

/// (environment, app, purpose, check name).
type Key = (String, String, String, String);
/// Results per check.
type TrackedMap = HashMap<Key, Tracked>;

/// Three periods, and never less than [`FRESH_FOR`].
fn fresh_for(c: &Check) -> Duration {
    Duration::from_secs(u64::from(c.period_secs) * 3).max(FRESH_FOR)
}

fn key_of(d: &Descriptor, purpose: Purpose, check: &str) -> Key {
    (d.environment.clone(), d.name.clone(), purpose.as_str().to_owned(), check.to_owned())
}

/// Per (environment, app): the workload generation and when it was first seen.
type Generations = HashMap<(String, String), (String, Instant)>;

/// The checks of every app described on this node.
#[derive(Debug, Clone, Default)]
pub struct Runner {
    tracked: Arc<Mutex<TrackedMap>>,
    /// The workload generation of each app (from the default check's
    /// lookup) and when this agent first saw it.
    generations: Arc<Mutex<Generations>>,
    /// The background loop was started (once).
    started: Arc<std::sync::atomic::AtomicBool>,
    /// Vault-backed header values, held in memory only (D-111).
    secrets: crate::health_secrets::Secrets,
}

impl Runner {
    /// Where Forge is, for asking it for vault-backed header values.
    pub fn set_control(&self, endpoint: &crate::link::Endpoint, tls: Arc<rustls::ClientConfig>) {
        self.secrets.set_control(endpoint, tls);
    }

    /// Starts the background loop once, from inside the agent's async
    /// runtime (the first heartbeat). A no-op before [`init`], after the
    /// first call, and outside a Tokio runtime, so it can never panic at
    /// startup (the lab found `Agent::load` running before the runtime).
    pub fn ensure_started(&self) {
        use std::sync::atomic::Ordering;
        let Some(dir) = DIR.get().cloned() else { return };
        let Ok(handle) = tokio::runtime::Handle::try_current() else { return };
        if self.started.swap(true, Ordering::SeqCst) {
            return;
        }
        let r = self.clone();
        handle.spawn(async move { r.run(dir).await });
    }
    async fn run(self, dir: PathBuf) {
        let sem = Arc::new(tokio::sync::Semaphore::new(MAX_CONCURRENT));
        // A random phase spreads nodes over the period.
        tokio::time::sleep(Duration::from_millis(u64::from(rand_u16()) % 10_000)).await;
        let mut tick = tokio::time::interval(Duration::from_secs(1));
        tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        let mut apps: Vec<Descriptor> = Vec::new();
        let mut loaded: Option<Instant> = None;
        let mut default_due: Option<Instant> = None;
        let mut due: HashMap<Key, Instant> = HashMap::new();
        loop {
            tick.tick().await;
            let now = Instant::now();
            // Vault values whose time is up leave memory, asked for again or not.
            self.secrets.purge(now);
            let changed = CHANGED.swap(false, std::sync::atomic::Ordering::SeqCst);
            if changed || loaded.is_none_or(|t| now.duration_since(t) >= PERIOD) {
                apps = load_all(&dir);
                loaded = Some(now);
                self.forget_missing(&apps);
                due.retain(|k, _| apps.iter().any(|a| a.environment == k.0 && a.name == k.1));
            }
            // The default check, which also finds the workload generation.
            if default_due.is_none_or(|t| now >= t) {
                default_due = Some(now + PERIOD);
                let ambiguous = ambiguous_ports(&apps);
                for d in apps.iter() {
                    let Ok(permit) = sem.clone().try_acquire_owned() else {
                        tracing::debug!("health checks throttled this period");
                        break;
                    };
                    let amb = d.listen_port.is_some_and(|p| ambiguous.contains(&p));
                    let (r, d) = (self.clone(), d.clone());
                    tokio::spawn(async move {
                        let a = check(&d, amb).await;
                        drop(permit);
                        r.note_generation(&d, &a.generation);
                        if d.default_checks {
                            r.record(&d, a);
                        }
                    });
                }
            }
            // Configured checks, each at its own period.
            for d in apps.iter() {
                for (purpose, c) in self.plan(d, now, &mut due) {
                    let Ok(permit) = sem.clone().try_acquire_owned() else {
                        // Try again at the next tick.
                        due.remove(&key_of(d, purpose, &c.name));
                        continue;
                    };
                    let (r, d) = (self.clone(), d.clone());
                    tokio::spawn(async move {
                        let outcome = r.attempt(&d, purpose, &c).await;
                        drop(permit);
                        r.record_configured(&d, purpose, &c, outcome);
                    });
                }
            }
        }
    }

    /// One attempt of a configured check.
    async fn attempt(&self, d: &Descriptor, purpose: Purpose, c: &Check) -> crate::health_probe::Outcome {
        use forge_catalog::health::Probe;
        let timeout = Duration::from_secs(u64::from(c.timeout_secs));
        let none: &[crate::health_secrets::ResolvedHeader] = &[];
        let secrets = match &c.probe {
            Probe::Exec(_) | Probe::Shell(_) => return command_check(d, purpose, c, timeout).await,
            Probe::Http(h) if h.headers.iter().any(|x| x.secret.is_some()) => {
                let req = forge_proto::messages::HealthSecretsRequest {
                    environment: d.environment.clone(),
                    app: d.name.clone(),
                    purpose: purpose.as_str().to_owned(),
                    check: c.name.clone(),
                    policy_revision: d.revision,
                };
                // Refused or unanswered: the check is unknown and nothing
                // is sent to its target.
                match self.secrets.resolve(req).await {
                    Ok(values) => Some(values),
                    Err(unknown) => return unknown,
                }
            }
            Probe::Http(_) | Probe::Tcp(_) => None,
        };
        let secrets = secrets.as_ref().map_or(none, |s| s.as_slice());
        crate::health_probe::run(&c.probe, timeout, crate::health_probe::Context { fleet: &d.fleet, secrets }).await
    }

    fn note_generation(&self, d: &Descriptor, generation: &str) {
        let Ok(mut g) = self.generations.lock() else { return };
        let k = (d.environment.clone(), d.name.clone());
        if g.get(&k).is_none_or(|(known, _)| known != generation) {
            g.insert(k, (generation.to_owned(), Instant::now()));
        }
    }

    fn generation(&self, d: &Descriptor) -> Option<(String, Instant)> {
        self.generations.lock().ok()?.get(&(d.environment.clone(), d.name.clone())).cloned()
    }

    /// The configured checks of `d` to run now. Checks that need no attempt
    /// get their state here: `disabled`, and `pending` (the startup checks
    /// haven't succeeded yet). A check this node can't run is attempted
    /// like any other; the attempt reports `unknown` with the reason.
    fn plan(&self, d: &Descriptor, now: Instant, due: &mut HashMap<Key, Instant>) -> Vec<(Purpose, Check)> {
        let now_ms = now_ms();
        let (generation, since) = self.generation(d).unwrap_or_else(|| (String::new(), now));
        let Ok(mut t) = self.tracked.lock() else { return Vec::new() };
        let entry = |t: &mut TrackedMap, purpose: Purpose, c: &Check| -> Key {
            let k = key_of(d, purpose, &c.name);
            let e = t.entry(k.clone()).or_insert_with(Tracked::new);
            e.fresh_for = fresh_for(c);
            e.revision = d.revision;
            k
        };
        // Startup checks that gate the others: the enabled ones. One this
        // node will never run as it is (a permanent refusal: no Landlock,
        // not Linux, no account) is no evidence and holds nothing back. One
        // that only could not run yet (a policy on its way, a busy node,
        // Forge not answering) stays `waiting` and keeps the gate shut.
        let mut started = true;
        for c in d.policy.startup.iter().filter(|c| c.enabled) {
            let k = entry(&mut t, Purpose::Startup, c);
            if let Some(e) = t.get_mut(&k) {
                if e.generation != generation || !matches!(e.state, "waiting" | "succeeded" | "exhausted" | "unknown") {
                    e.generation.clone_from(&generation);
                    e.set("waiting", "", "", now_ms);
                }
                started &= e.state == "succeeded" || (e.state == "unknown" && e.permanent);
            }
        }
        let fixed = |e: &mut Tracked, state: &'static str, reason: &str, detail: &str| {
            if e.state != state || e.reason != reason {
                e.set(state, reason, detail, now_ms);
            }
            // Keep the evidence time moving so keep-alives stay current.
            e.observed_at_ms = now_ms;
        };
        let mut run = Vec::new();
        for (purpose, c) in d.policy.checks() {
            let k = entry(&mut t, purpose, c);
            let Some(e) = t.get_mut(&k) else { continue };
            if !c.enabled {
                // Startup has no `disabled` state: a disabled startup check
                // is not reported, and gates nothing.
                if purpose != Purpose::Startup {
                    fixed(e, "disabled", "", "");
                }
                continue;
            }
            if purpose == Purpose::Startup {
                if matches!(e.state, "succeeded" | "exhausted") {
                    continue;
                }
            } else if !started {
                fixed(e, "pending", "", "waiting for the startup checks");
                continue;
            }
            if now.duration_since(since) < Duration::from_secs(u64::from(c.initial_delay_secs)) {
                continue;
            }
            if due.get(&k).is_none_or(|at| now >= *at) {
                due.insert(k, now + Duration::from_secs(u64::from(c.period_secs)));
                run.push((purpose, c.clone()));
            }
        }
        run
    }

    fn record_configured(&self, d: &Descriptor, purpose: Purpose, c: &Check, o: crate::health_probe::Outcome) {
        let generation = self.generation(d).map(|(g, _)| g).unwrap_or_default();
        let t = Thresholds { failures: c.failure_threshold, successes: c.success_threshold.max(1) };
        let Ok(mut map) = self.tracked.lock() else { return };
        let e = map.entry(key_of(d, purpose, &c.name)).or_insert_with(Tracked::new);
        e.fresh_for = fresh_for(c);
        e.revision = d.revision;
        use crate::health_probe::{Unknown, Verdict};
        let throttled = o.verdict == Verdict::Unknown && o.unknown == Unknown::Throttled;
        e.throttled = if throttled { e.throttled.saturating_add(1) } else { 0 };
        // No free place several periods in a row is a failed attempt: the
        // check did not answer in time, whatever the reason.
        let o = if throttled && e.throttled >= MAX_THROTTLED {
            crate::health_probe::Outcome::fail(
                "timeout",
                format!("not run for {} periods in a row: {}", e.throttled, o.detail),
            )
        } else {
            o
        };
        if o.verdict == Verdict::Unknown {
            // The check did not run: no failure, no success, counts start
            // over. A startup check that already ended keeps its final state.
            let ended = purpose == Purpose::Startup
                && matches!(e.state, "succeeded" | "exhausted")
                && e.generation == generation;
            if !ended {
                e.generation = generation;
                e.permanent = o.unknown == Unknown::Permanent;
                // A startup check that only could not run yet keeps waiting.
                let state = if purpose == Purpose::Startup && !e.permanent { "waiting" } else { "unknown" };
                e.set(state, o.reason, &o.detail, now_ms());
            }
            return;
        }
        e.permanent = false;
        let a = Attempt { ok: o.ok(), reason: o.reason.to_owned(), detail: o.detail, generation };
        if purpose == Purpose::Startup {
            e.apply_startup(a, now_ms(), t);
        } else {
            e.apply(a, now_ms(), t);
        }
    }

    /// Drops results of apps and checks that are gone.
    fn forget_missing(&self, apps: &[Descriptor]) {
        let by_app: HashMap<(&str, &str), &Descriptor> =
            apps.iter().map(|a| ((a.environment.as_str(), a.name.as_str()), a)).collect();
        if let Ok(mut t) = self.tracked.lock() {
            t.retain(|(ns, app, purpose, check), _| {
                let Some(d) = by_app.get(&(ns.as_str(), app.as_str())) else { return false };
                if check == "default" {
                    return d.default_checks && (purpose != "readiness" || !d.has_readiness);
                }
                d.policy.checks().any(|(p, c)| p.as_str() == purpose && c.name == *check)
            });
        }
        if let Ok(mut g) = self.generations.lock() {
            g.retain(|(ns, app), _| by_app.contains_key(&(ns.as_str(), app.as_str())));
        }
        // Vault values of checks that are gone or disabled leave memory
        // with them.
        self.secrets.retain(|(ns, app, purpose, check)| {
            by_app
                .get(&(ns.as_str(), app.as_str()))
                .is_some_and(|d| d.policy.checks().any(|(p, c)| c.enabled && p.as_str() == purpose && c.name == *check))
        });
    }

    fn record(&self, d: &Descriptor, a: Attempt) {
        let now_ms = now_ms();
        let Ok(mut t) = self.tracked.lock() else { return };
        let purposes: &[Purpose] =
            if d.has_readiness { &[Purpose::Liveness] } else { &[Purpose::Liveness, Purpose::Readiness] };
        for p in purposes {
            let e = t.entry(key_of(d, *p, "default")).or_insert_with(Tracked::new);
            e.revision = d.revision;
            e.apply(a.clone(), now_ms, DEFAULT_THRESHOLDS);
        }
    }

    /// Reports for the next heartbeat: every changed check, plus a keep-alive
    /// for checks last reported more than half their freshness ago.
    #[must_use]
    pub fn drain(&self) -> Vec<HealthReport> {
        let now = Instant::now();
        let Ok(mut t) = self.tracked.lock() else { return Vec::new() };
        let mut out = Vec::new();
        for ((environment, app, purpose, check), s) in t.iter_mut() {
            if out.len() >= MAX_HEALTH_REPORTS || s.observed_at_ms == 0 {
                continue;
            }
            let due = match &s.reported {
                None => true,
                Some((state, reason, at)) => {
                    *state != s.state || *reason != s.reason || now.duration_since(*at) >= s.fresh_for / 2
                }
            };
            if !due {
                continue;
            }
            out.push(HealthReport {
                environment: environment.clone(),
                app: app.clone(),
                purpose: purpose.clone(),
                check: check.clone(),
                state: s.state.into(),
                reason: s.reason.clone(),
                detail: s.detail.chars().take(256).collect(),
                generation: s.generation.clone(),
                observed_at_ms: s.observed_at_ms,
                fresh_for_ms: u64::try_from(s.fresh_for.as_millis()).unwrap_or(90_000),
                policy_revision: s.revision,
            });
            s.reported = Some((s.state, s.reason.clone(), now));
        }
        out
    }
}

/// One exec or shell check: the node's helper runs it, by reference, as
/// the app's own account or in its container (D-111). Every way it can't
/// run is `unknown` with the reason; only a run decides pass or fail.
async fn command_check(d: &Descriptor, purpose: Purpose, c: &Check, timeout: Duration) -> crate::health_probe::Outcome {
    use crate::health_exec::{CheckRef, Refusal};
    use crate::health_probe::{Outcome, Unknown};
    if !cfg!(target_os = "linux") {
        return Outcome::unknown(Unknown::Permanent, "unsupported_executor", Refusal::Unsupported.detail());
    }
    let check = CheckRef {
        environment: d.environment.clone(),
        app: d.name.clone(),
        purpose,
        check: c.name.clone(),
        revision: d.revision,
    };
    // The helper ends the run at the check's timeout; the margin is for its answer.
    let wait = timeout + Duration::from_secs(5);
    let r = crate::helper::call(&crate::helper::HelperRequest::HealthCheck { check }, wait).await;
    if !r.ok {
        // The helper is not running, timed out, or is older than this
        // agent and does not know the request. Which one is in its own
        // words; a restart or an update ends all three, so it is transient.
        let said = crate::health_exec::sanitize(r.error.unwrap_or_default().as_bytes());
        return Outcome::unknown(
            Unknown::Transient,
            "unsupported_executor",
            format!("the node's helper did not run the check (not running, or older than its agent): {said}"),
        );
    }
    match r.data.map(serde_json::from_value::<crate::health_exec::ExecResult>) {
        Some(Ok(result)) => command_outcome(result, timeout),
        _ => Outcome::unknown(
            Unknown::Transient,
            "unsupported_executor",
            "the node's helper sent an answer this agent can't read",
        ),
    }
}

/// What a helper's answer means for the check.
fn command_outcome(result: crate::health_exec::ExecResult, timeout: Duration) -> crate::health_probe::Outcome {
    use crate::health_exec::{ExecResult, Refusal};
    use crate::health_probe::{Outcome, Unknown};
    match result {
        ExecResult::Exited { passed: true, .. } => Outcome::pass(),
        ExecResult::Exited { code, output, .. } if output.is_empty() => {
            Outcome::fail("app_failure", format!("exit code {code}"))
        }
        ExecResult::Exited { code, output, .. } => Outcome::fail("app_failure", format!("exit code {code}: {output}")),
        ExecResult::Signaled { signal } => Outcome::fail("app_failure", format!("ended by signal {signal}")),
        ExecResult::TimedOut => Outcome::fail("timeout", format!("not finished within {} s; ended", timeout.as_secs())),
        ExecResult::StartFailed { detail } => Outcome::fail("invalid_config", detail),
        ExecResult::NotRunning => Outcome::fail("app_failure", "the container is not running"),
        // The previous run outlived its timeout and is still there.
        ExecResult::Refused { why: Refusal::Busy } => Outcome::fail("timeout", Refusal::Busy.detail()),
        ExecResult::Refused { why: Refusal::Throttled } => {
            Outcome::unknown(Unknown::Throttled, "unsupported_executor", Refusal::Throttled.detail())
        }
        ExecResult::Refused { why } => {
            let kind = if why.permanent() { Unknown::Permanent } else { Unknown::Transient };
            Outcome::unknown(kind, "unsupported_executor", why.detail())
        }
    }
}

/// One default check attempt.
async fn check(d: &Descriptor, ambiguous: bool) -> Attempt {
    match d.runtime {
        Runtime::Systemd => check_service(d, ambiguous).await,
        Runtime::Container => check_container(d).await,
    }
}

async fn check_service(d: &Descriptor, ambiguous: bool) -> Attempt {
    let fail = |reason: &str, detail: String, generation: String| Attempt {
        ok: false,
        reason: reason.into(),
        detail,
        generation,
    };
    let (active, generation) = if crate::apps::has_systemd() {
        match crate::service::unit_active_state(&d.unit()).await {
            Ok(Some(s)) => s,
            Ok(None) => return fail("app_failure", "the systemd unit doesn't exist".into(), String::new()),
            Err(e) => return fail("unsupported_executor", e, String::new()),
        }
    } else {
        // No systemd (containers): Forge's supervisor runs the copy and
        // records its PID; the process id stands in for the unit generation.
        let copy = if d.slot.is_empty() { d.key() } else { format!("{}--{}", d.key(), d.slot) };
        match crate::apps::supervised_pid_of(&copy) {
            Some(pid) => ("active".to_owned(), pid.to_string()),
            None => return fail("app_failure", "the supervised process is not running".into(), String::new()),
        }
    };
    if active != "active" && active != "reloading" {
        return fail("app_failure", format!("the systemd unit is {active}"), generation);
    }
    match d.listen_port {
        Some(port) if ambiguous => Attempt {
            ok: true,
            reason: "ambiguous_port".into(),
            detail: format!("another app on this node also declares port {port}; checked the unit only"),
            generation,
        },
        Some(port) => {
            let addr = std::net::SocketAddr::from(([127, 0, 0, 1], port));
            match tokio::time::timeout(CONNECT_TIMEOUT, tokio::net::TcpStream::connect(addr)).await {
                Ok(Ok(_)) => Attempt { ok: true, reason: String::new(), detail: String::new(), generation },
                Ok(Err(e)) => fail("connect_failed", format!("port {port}: {e}"), generation),
                Err(_) => fail("timeout", format!("port {port}: no answer within 2 s"), generation),
            }
        }
        None => Attempt { ok: true, reason: String::new(), detail: String::new(), generation },
    }
}

async fn check_container(d: &Descriptor) -> Attempt {
    use crate::helper::HelperRequest;
    // Read-only and allowed unsigned for `forge-app-*` names (D-068): the
    // agent asks on its own, with no command from Forge.
    let r = crate::helper::call(&HelperRequest::ContainerState { name: d.unit() }, Duration::from_secs(5)).await;
    if !r.ok {
        return Attempt {
            ok: false,
            reason: "app_failure".into(),
            detail: r.error.unwrap_or_else(|| "the container doesn't exist".into()),
            generation: String::new(),
        };
    }
    container_attempt(&r.data.unwrap_or_default())
}

/// The attempt for a container state `{id, running, status, health}`.
fn container_attempt(v: &serde_json::Value) -> Attempt {
    let generation = v["id"].as_str().unwrap_or_default().to_owned();
    let fail =
        |detail: String| Attempt { ok: false, reason: "app_failure".into(), detail, generation: generation.clone() };
    if v["running"].as_bool() != Some(true) {
        let status = v["status"].as_str().filter(|s| !s.is_empty()).unwrap_or("not running");
        return fail(format!("the container is {status}"));
    }
    if v["health"].as_str() == Some("unhealthy") {
        return fail("the image HEALTHCHECK reports unhealthy".into());
    }
    Attempt { ok: true, reason: String::new(), detail: String::new(), generation }
}
fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| i64::try_from(d.as_millis()).unwrap_or(i64::MAX))
}

/// A random value for jitter (not security-relevant).
fn rand_u16() -> u16 {
    use std::hash::{BuildHasher, Hasher};
    let h = std::collections::hash_map::RandomState::new().build_hasher().finish();
    u16::try_from(h & 0xffff).unwrap_or(0)
}
#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    fn svc(name: &str, port: Option<u16>) -> Descriptor {
        Descriptor {
            environment: "uat".into(),
            name: name.into(),
            runtime: Runtime::Systemd,
            listen_port: port,
            default_checks: true,
            has_readiness: false,
            policy: HealthPolicy::default(),
            revision: 0,
            fleet: Vec::new(),
            slot: String::new(),
        }
    }

    /// `svc` with a configured policy (JSON form).
    fn with_policy(name: &str, policy: serde_json::Value) -> Descriptor {
        let policy: HealthPolicy = serde_json::from_value(policy).unwrap();
        Descriptor {
            default_checks: policy.default_checks,
            has_readiness: !policy.readiness.is_empty(),
            policy,
            revision: 7,
            ..svc(name, None)
        }
    }

    fn tcp(name: &str) -> serde_json::Value {
        serde_json::json!({ "name": name, "probe": { "type": "tcp", "port": 9 } })
    }

    fn outcome(ok: bool) -> crate::health_probe::Outcome {
        if ok { crate::health_probe::Outcome::pass() } else { crate::health_probe::Outcome::fail("connect_failed", "") }
    }

    fn reason_of(r: &Runner, d: &Descriptor, purpose: Purpose, check: &str) -> (String, String) {
        let t = r.tracked.lock().unwrap();
        t.get(&key_of(d, purpose, check)).map(|t| (t.reason.clone(), t.detail.clone())).unwrap_or_default()
    }

    /// D-111: what the helper answers decides pass, fail or unknown. Only
    /// a run is evidence; every refusal is `unknown` with its sentence.
    #[test]
    fn helper_answers_map_to_pass_fail_and_unknown() {
        use crate::health_exec::{ExecResult, Refusal};
        use crate::health_probe::Verdict;
        let t = Duration::from_secs(3);
        let of = |r: ExecResult| {
            let o = command_outcome(r, t);
            (o.verdict, o.reason, o.detail)
        };
        assert_eq!(of(ExecResult::Exited { code: 0, passed: true, output: String::new() }).0, Verdict::Pass);
        assert_eq!(
            of(ExecResult::Exited { code: 2, passed: false, output: "db down".into() }),
            (Verdict::Fail, "app_failure", "exit code 2: db down".into())
        );
        assert_eq!(
            of(ExecResult::Exited { code: 1, passed: false, output: String::new() }).2,
            "exit code 1",
            "no dangling colon"
        );
        assert_eq!(of(ExecResult::Signaled { signal: 9 }).1, "app_failure");
        assert_eq!(of(ExecResult::TimedOut), (Verdict::Fail, "timeout", "not finished within 3 s; ended".into()));
        assert_eq!(of(ExecResult::StartFailed { detail: "no such file".into() }).1, "invalid_config");
        assert_eq!(of(ExecResult::NotRunning).0, Verdict::Fail);
        // A run that could not be ended is still a timeout, not a new run.
        assert_eq!(of(ExecResult::Refused { why: Refusal::Busy }).1, "timeout");
        for why in [
            Refusal::NoPolicy,
            Refusal::StalePolicy,
            Refusal::PolicyExpired,
            Refusal::NoSuchCheck,
            Refusal::NoAccount,
            Refusal::NotConfined,
            Refusal::NotContained,
            Refusal::WorkerFailed,
            Refusal::NoDocker,
            Refusal::ForeignContainer,
            Refusal::Unsupported,
            Refusal::Throttled,
        ] {
            let (verdict, reason, detail) = of(ExecResult::Refused { why });
            assert_eq!((verdict, reason), (Verdict::Unknown, "unsupported_executor"), "{why:?}");
            assert_eq!(detail, why.detail());
            // Whether waiting can help travels with it.
            let kind = command_outcome(ExecResult::Refused { why }, t).unknown;
            let expected = match why {
                Refusal::Throttled => crate::health_probe::Unknown::Throttled,
                w if w.permanent() => crate::health_probe::Unknown::Permanent,
                _ => crate::health_probe::Unknown::Transient,
            };
            assert_eq!(kind, expected, "{why:?}");
        }
    }

    /// A node that has no place for a check is fine once; several periods
    /// in a row count as failed attempts, so it cannot look neutral for ever.
    #[test]
    fn repeated_throttling_becomes_a_timeout() {
        use crate::health_exec::{ExecResult, Refusal};
        let r = Runner::default();
        let d = with_policy(
            "api",
            serde_json::json!({ "liveness": [
                { "name": "cmd", "failure_threshold": 2, "probe": { "type": "exec", "argv": ["/bin/true"] } }
            ] }),
        );
        let c = d.policy.liveness[0].clone();
        let throttled = || command_outcome(ExecResult::Refused { why: Refusal::Throttled }, Duration::from_secs(2));
        let pass = || {
            command_outcome(ExecResult::Exited { code: 0, passed: true, output: String::new() }, Duration::from_secs(2))
        };
        r.record_configured(&d, Purpose::Liveness, &c, pass());
        for _ in 1..MAX_THROTTLED {
            r.record_configured(&d, Purpose::Liveness, &c, throttled());
            assert_eq!(state_of(&r, &d, Purpose::Liveness, "cmd"), "unknown");
        }
        // The third in a row is a failed attempt; with the fourth the
        // check's own threshold (2) is reached.
        r.record_configured(&d, Purpose::Liveness, &c, throttled());
        assert_eq!(reason_of(&r, &d, Purpose::Liveness, "cmd").0, "timeout");
        r.record_configured(&d, Purpose::Liveness, &c, throttled());
        assert_eq!(state_of(&r, &d, Purpose::Liveness, "cmd"), "failing");
        assert!(reason_of(&r, &d, Purpose::Liveness, "cmd").1.contains("not run for 4 periods"));
        // One run that gets a place starts the count over.
        r.record_configured(&d, Purpose::Liveness, &c, pass());
        assert_eq!(state_of(&r, &d, Purpose::Liveness, "cmd"), "passing");
        r.record_configured(&d, Purpose::Liveness, &c, throttled());
        assert_eq!(state_of(&r, &d, Purpose::Liveness, "cmd"), "unknown");
    }

    /// A check the node refuses reports `unknown` with the reason, counts
    /// nothing, and a startup check in that state holds nothing back.
    #[test]
    fn checks_that_cannot_run_are_unknown_and_gate_nothing() {
        use crate::health_exec::Refusal;
        use crate::health_probe::Outcome;
        let r = Runner::default();
        let d = with_policy(
            "api",
            serde_json::json!({
                "startup": [{ "name": "boot", "probe": { "type": "exec", "argv": ["/bin/true"] } }],
                "liveness": [tcp("live")]
            }),
        );
        let (now, mut due) = (Instant::now(), HashMap::new());
        let run = r.plan(&d, now, &mut due);
        assert_eq!(run.iter().map(|(_, c)| c.name.as_str()).collect::<Vec<_>>(), ["boot"]);
        let boot = d.policy.startup[0].clone();
        // A refusal that passes on its own (the policy is on its way):
        // the startup check keeps waiting and the gate stays shut.
        let not_yet = command_outcome(
            crate::health_exec::ExecResult::Refused { why: Refusal::StalePolicy },
            Duration::from_secs(2),
        );
        r.record_configured(&d, Purpose::Startup, &boot, not_yet);
        assert_eq!(state_of(&r, &d, Purpose::Startup, "boot"), "waiting");
        assert!(reason_of(&r, &d, Purpose::Startup, "boot").1.contains("another policy revision"));
        let run = r.plan(&d, now + Duration::from_secs(11), &mut due);
        assert_eq!(run.iter().map(|(_, c)| c.name.as_str()).collect::<Vec<_>>(), ["boot"], "liveness still waits");
        assert_eq!(state_of(&r, &d, Purpose::Liveness, "live"), "pending");
        let now = now + Duration::from_secs(11);
        // A refusal the node will repeat for ever releases the gate.
        let refused = || {
            command_outcome(
                crate::health_exec::ExecResult::Refused { why: Refusal::NotConfined },
                Duration::from_secs(2),
            )
        };
        r.record_configured(&d, Purpose::Startup, &boot, refused());
        assert_eq!(state_of(&r, &d, Purpose::Startup, "boot"), "unknown");
        let (reason, detail) = reason_of(&r, &d, Purpose::Startup, "boot");
        assert_eq!(reason, "unsupported_executor");
        assert!(detail.contains("can't confine command checks"), "{detail}");
        // Liveness runs; the startup check is tried again at its period.
        let run = r.plan(&d, now + Duration::from_secs(11), &mut due);
        let names: Vec<&str> = run.iter().map(|(_, c)| c.name.as_str()).collect();
        assert!(names.contains(&"live") && names.contains(&"boot"), "{names:?}");
        assert_eq!(state_of(&r, &d, Purpose::Startup, "boot"), "unknown", "planning does not reset it");
        // The report carries the reason and the revision.
        let report = r.drain().into_iter().find(|h| h.check == "boot").unwrap();
        assert_eq!(
            (report.state.as_str(), report.reason.as_str(), report.policy_revision),
            ("unknown", "unsupported_executor", 7)
        );
        // Once it can run and succeeds, it is final; a later refusal changes nothing.
        r.record_configured(&d, Purpose::Startup, &boot, Outcome::pass());
        assert_eq!(state_of(&r, &d, Purpose::Startup, "boot"), "succeeded");
        r.record_configured(&d, Purpose::Startup, &boot, refused());
        assert_eq!(state_of(&r, &d, Purpose::Startup, "boot"), "succeeded");
        // An unknown readiness result clears a passing state without counting as a failure.
        let live = d.policy.liveness[0].clone();
        r.record_configured(&d, Purpose::Liveness, &live, Outcome::pass());
        assert_eq!(state_of(&r, &d, Purpose::Liveness, "live"), "passing");
        let denied = Outcome::unknown(crate::health_probe::Unknown::Transient, "permission_denied", "refused");
        r.record_configured(&d, Purpose::Liveness, &live, denied);
        assert_eq!(state_of(&r, &d, Purpose::Liveness, "live"), "unknown");
        assert_eq!(r.tracked.lock().unwrap().get(&key_of(&d, Purpose::Liveness, "live")).unwrap().failures, 0);
    }

    /// The fleet's addresses arrive with the policy, are bounded, and are
    /// what the probes of that app are checked against.
    #[test]
    fn the_fleet_list_arrives_with_the_policy() {
        let dir = tempfile::tempdir().unwrap();
        save(dir.path(), &svc("api", None)).unwrap();
        let many: Vec<String> =
            (0..3000u32).map(|i| format!("10.{}.{}.{}", i >> 16, (i >> 8) & 255, i & 255)).collect();
        let spec = serde_json::json!({
            "environment": "uat", "name": "api", "runtime": "systemd", "revision": 3, "fleet": many,
            "policy": { "liveness": [{ "name": "live", "probe": { "type": "tcp", "port": 9 } }] }
        });
        apply_in(dir.path(), &spec.to_string()).unwrap();
        let d = load_one(dir.path(), "uat", "api").unwrap();
        assert_eq!(d.fleet.len(), forge_catalog::health::MAX_FLEET_ADDRESSES);
        assert_eq!(d.fleet[1], "10.0.0.1".parse::<std::net::IpAddr>().unwrap());
        assert!(runs_code(
            &serde_json::json!({
                "environment": "uat", "name": "api", "runtime": "systemd", "revision": 3,
                "policy": { "liveness": [{ "name": "c", "probe": { "type": "shell", "script": "true" } }] }
            })
            .to_string()
        ));
        assert!(!runs_code(&spec.to_string()) && !runs_code("not json"));
        // The helper must take a policy that has command checks, and one
        // that removes them: it still holds the old ones.
        assert!(!helper_must_take_in(dir.path(), &spec.to_string()), "no command checks before or after");
        let with_cmd = serde_json::json!({
            "environment": "uat", "name": "api", "runtime": "systemd", "revision": 4,
            "policy": { "liveness": [{ "name": "c", "probe": { "type": "shell", "script": "true" } }] }
        });
        assert!(helper_must_take_in(dir.path(), &with_cmd.to_string()));
        apply_in(dir.path(), &with_cmd.to_string()).unwrap();
        assert!(helper_must_take_in(dir.path(), &spec.to_string()), "the new policy removes command checks");
        // A descriptor written by an older agent has no list.
        let old = serde_json::json!({ "environment": "uat", "name": "web", "runtime": "systemd", "listen_port": null,
            "default_checks": true, "has_readiness": false });
        assert!(serde_json::from_value::<Descriptor>(old).unwrap().fleet.is_empty());
    }

    fn state_of(r: &Runner, d: &Descriptor, purpose: Purpose, check: &str) -> &'static str {
        r.tracked.lock().unwrap().get(&key_of(d, purpose, check)).map_or("", |t| t.state)
    }

    #[test]
    fn startup_checks_gate_the_others_and_run_once_per_generation() {
        let r = Runner::default();
        let d = with_policy("api", serde_json::json!({ "startup": [tcp("boot")], "liveness": [tcp("live")] }));
        let (now, mut due) = (Instant::now(), HashMap::new());
        // Only the startup check runs; liveness waits.
        let run = r.plan(&d, now, &mut due);
        assert_eq!(run.iter().map(|(p, c)| (*p, c.name.as_str())).collect::<Vec<_>>(), [(Purpose::Startup, "boot")]);
        assert_eq!(state_of(&r, &d, Purpose::Startup, "boot"), "waiting");
        assert_eq!(state_of(&r, &d, Purpose::Liveness, "live"), "pending");
        // Not due again within its period.
        assert!(r.plan(&d, now + Duration::from_secs(1), &mut due).is_empty());
        // It succeeds: liveness runs, startup doesn't run again.
        let boot = d.policy.startup[0].clone();
        r.record_configured(&d, Purpose::Startup, &boot, outcome(true));
        assert_eq!(state_of(&r, &d, Purpose::Startup, "boot"), "succeeded");
        let run = r.plan(&d, now + Duration::from_secs(11), &mut due);
        assert_eq!(run.iter().map(|(p, _)| *p).collect::<Vec<_>>(), [Purpose::Liveness]);
        // A restart (new generation) starts over.
        r.note_generation(&d, "g2");
        let run = r.plan(&d, now + Duration::from_secs(30), &mut due);
        assert_eq!(run.iter().map(|(p, _)| *p).collect::<Vec<_>>(), [Purpose::Startup]);
        assert_eq!(state_of(&r, &d, Purpose::Liveness, "live"), "pending");
    }

    #[test]
    fn startup_exhausts_after_its_failures() {
        let r = Runner::default();
        let d = with_policy("api", serde_json::json!({ "startup": [tcp("boot")] }));
        let boot = d.policy.startup[0].clone();
        r.plan(&d, Instant::now(), &mut HashMap::new());
        for _ in 0..2 {
            r.record_configured(&d, Purpose::Startup, &boot, outcome(false));
        }
        assert_eq!(state_of(&r, &d, Purpose::Startup, "boot"), "waiting");
        r.record_configured(&d, Purpose::Startup, &boot, outcome(false));
        assert_eq!(state_of(&r, &d, Purpose::Startup, "boot"), "exhausted");
        // Final for this generation: no further attempts, a late pass changes nothing.
        assert!(r.plan(&d, Instant::now() + Duration::from_secs(60), &mut HashMap::new()).is_empty());
        r.record_configured(&d, Purpose::Startup, &boot, outcome(true));
        assert_eq!(state_of(&r, &d, Purpose::Startup, "boot"), "exhausted");
    }

    #[test]
    fn disabled_checks_report_without_running_and_command_checks_are_planned() {
        let r = Runner::default();
        let d = with_policy(
            "api",
            serde_json::json!({ "readiness": [
                { "name": "off", "enabled": false, "probe": { "type": "tcp", "port": 9 } },
                { "name": "cmd", "probe": { "type": "exec", "argv": ["/bin/true"] } },
                { "name": "late", "initial_delay_secs": 30, "probe": { "type": "tcp", "port": 9 } }
            ] }),
        );
        r.note_generation(&d, "g1");
        // The command check is due like any other (the helper runs it); the
        // delayed one is not yet.
        let run = r.plan(&d, Instant::now(), &mut HashMap::new());
        assert_eq!(run.iter().map(|(_, c)| c.name.as_str()).collect::<Vec<_>>(), ["cmd"]);
        assert_eq!(state_of(&r, &d, Purpose::Readiness, "off"), "disabled");
        let run = r.plan(&d, Instant::now() + Duration::from_secs(31), &mut HashMap::new());
        assert_eq!(run.iter().map(|(_, c)| c.name.as_str()).collect::<Vec<_>>(), ["cmd", "late"]);
        let reports = r.drain();
        assert!(reports.iter().any(|h| h.check == "off" && h.state == "disabled"));
        assert!(reports.iter().all(|h| h.check != "late" && h.check != "cmd"), "nothing observed yet");
    }

    /// Not Linux: the agent refuses a command check itself, with the reason.
    #[cfg(not(target_os = "linux"))]
    #[tokio::test]
    async fn command_checks_are_refused_off_linux() {
        let d = with_policy(
            "api",
            serde_json::json!({ "liveness": [{ "name": "cmd", "probe": { "type": "exec", "argv": ["x"] } }] }),
        );
        let o = Runner::default().attempt(&d, Purpose::Liveness, &d.policy.liveness[0]).await;
        assert_eq!((o.verdict, o.reason), (crate::health_probe::Verdict::Unknown, "unsupported_executor"));
        assert!(o.detail.contains("Linux"), "{}", o.detail);
    }

    /// A check with a vault header is unknown, and sends nothing, while
    /// Forge has not handed out the value.
    #[tokio::test]
    async fn vault_header_checks_wait_for_forge() {
        let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = l.local_addr().unwrap().port();
        let d = with_policy(
            "api",
            serde_json::json!({ "liveness": [{ "name": "http", "probe": { "type": "http", "port": port,
                "headers": [{ "name": "Authorization", "secret": "vault:abc" }] } }] }),
        );
        let o = Runner::default().attempt(&d, Purpose::Liveness, &d.policy.liveness[0]).await;
        assert_eq!((o.verdict, o.reason), (crate::health_probe::Verdict::Unknown, "location_unavailable"));
        let nobody = tokio::time::timeout(Duration::from_millis(200), l.accept()).await;
        assert!(nobody.is_err(), "the target was contacted without the header value");
    }

    #[test]
    fn success_thresholds_and_removed_checks() {
        let r = Runner::default();
        let d = with_policy(
            "api",
            serde_json::json!({ "readiness": [
                { "name": "slow", "success_threshold": 2, "probe": { "type": "tcp", "port": 9 } }
            ] }),
        );
        let c = d.policy.readiness[0].clone();
        r.record_configured(&d, Purpose::Readiness, &c, outcome(true));
        assert_eq!(state_of(&r, &d, Purpose::Readiness, "slow"), "pending");
        r.record_configured(&d, Purpose::Readiness, &c, outcome(true));
        assert_eq!(state_of(&r, &d, Purpose::Readiness, "slow"), "passing");
        // The policy drops the check, and turns the defaults off.
        r.record(&d, Attempt { ok: true, reason: String::new(), detail: String::new(), generation: String::new() });
        let mut next = with_policy("api", serde_json::json!({ "default_checks": false }));
        next.revision = 8;
        r.forget_missing(std::slice::from_ref(&next));
        assert!(r.tracked.lock().unwrap().is_empty());
    }

    #[test]
    fn a_pushed_policy_replaces_the_descriptor_of_a_deployed_app() {
        let dir = tempfile::tempdir().unwrap();
        let spec = |name: &str, port: u16| {
            serde_json::json!({
                "environment": "uat", "name": name, "runtime": "systemd", "listen_port": 8080, "revision": 42,
                "policy": { "liveness": [{ "name": "live", "probe": { "type": "tcp", "port": port } }] }
            })
            .to_string()
        };
        // Not deployed here: refused.
        assert!(apply_in(dir.path(), &spec("api", 9)).unwrap_err().contains("not deployed"));
        save(dir.path(), &svc("api", Some(8080))).unwrap();
        assert_eq!(apply_in(dir.path(), &spec("api", 9)).unwrap()["revision"], 42);
        let d = load_all(dir.path()).pop().unwrap();
        assert_eq!((d.revision, d.policy.liveness.len(), d.default_checks), (42, 1, true));
        // An invalid policy and a bad name are refused.
        assert!(apply_in(dir.path(), &spec("api", 0)).unwrap_err().contains("invalid health policy"));
        assert!(apply_in(dir.path(), &spec("../x", 9)).is_err());
    }

    #[test]
    fn duplicate_ports_on_a_node_are_ambiguous() {
        let apps = vec![svc("a", Some(8080)), svc("b", Some(8080)), svc("c", Some(9090)), svc("d", None)];
        assert_eq!(ambiguous_ports(&apps), vec![8080]);
        let mut ctr = svc("e", Some(9090));
        ctr.runtime = Runtime::Container;
        assert_eq!(ambiguous_ports(&[svc("c", Some(9090)), ctr]), Vec::<u16>::new(), "containers publish ports");
    }

    #[test]
    fn three_failures_turn_a_check_failing_and_one_success_restores_it() {
        let ok = || Attempt { ok: true, reason: String::new(), detail: String::new(), generation: "g1".into() };
        let bad = || Attempt { ok: false, reason: "timeout".into(), detail: "x".into(), generation: "g1".into() };
        let mut t = Tracked::new();
        assert_eq!(t.state, "pending");
        let d = DEFAULT_THRESHOLDS;
        t.apply(ok(), 1, d);
        assert_eq!(t.state, "passing");
        t.apply(bad(), 2, d);
        t.apply(bad(), 3, d);
        assert_eq!(t.state, "passing", "two failures are not enough");
        t.apply(bad(), 4, d);
        assert_eq!((t.state, t.reason.as_str()), ("failing", "timeout"));
        t.apply(ok(), 5, d);
        assert_eq!(t.state, "passing");
        // A new generation starts counting afresh.
        t.apply(bad(), 6, d);
        t.apply(bad(), 7, d);
        t.apply(Attempt { generation: "g2".into(), ..bad() }, 8, d);
        assert_eq!(t.state, "passing", "the restart reset the count");
    }

    /// Lab regression: starting outside a Tokio runtime must not panic.
    #[test]
    fn starting_outside_a_runtime_does_not_panic() {
        let dir = tempfile::tempdir().unwrap();
        init(dir.path());
        let r = Runner::default();
        r.ensure_started();
        assert!(!r.started.load(std::sync::atomic::Ordering::SeqCst), "not started without a runtime");
        let rt = tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();
        rt.block_on(async {
            r.ensure_started();
            r.ensure_started();
        });
        assert!(r.started.load(std::sync::atomic::Ordering::SeqCst), "started once inside the runtime");
    }

    #[test]
    fn descriptors_with_bad_slugs_are_ignored() {
        let dir = tempfile::tempdir().unwrap();
        let good = svc("api", Some(8080));
        let mut bad = svc("x", None);
        bad.name = "../../etc;rm".into();
        for d in [&good, &bad] {
            std::fs::write(dir.path().join(format!("{}.json", d.name.len())), serde_json::to_vec(d).unwrap()).unwrap();
        }
        assert_eq!(load_all(dir.path()), vec![good]);
    }

    /// Lab regression (check 5): an ambiguous port is a pass whose reason
    /// must reach the server.
    #[test]
    fn a_pass_keeps_its_reason() {
        let mut t = Tracked::new();
        t.apply(
            Attempt {
                ok: true,
                reason: "ambiguous_port".into(),
                detail: "port 8131 twice".into(),
                generation: "g".into(),
            },
            1,
            DEFAULT_THRESHOLDS,
        );
        assert_eq!((t.state, t.reason.as_str()), ("passing", "ambiguous_port"));
        t.apply(
            Attempt { ok: true, reason: String::new(), detail: String::new(), generation: "g".into() },
            2,
            DEFAULT_THRESHOLDS,
        );
        assert_eq!(t.reason, "", "a plain pass clears it");
    }

    /// Lab regression (check 3): container state comes from the unsigned
    /// ContainerState lookup with four fields.
    #[test]
    fn container_state_maps_to_attempts() {
        let a =
            container_attempt(&serde_json::json!({"id": "abc", "running": true, "status": "running", "health": null}));
        assert!(a.ok && a.generation == "abc");
        let a = container_attempt(
            &serde_json::json!({"id": "abc", "running": true, "status": "running", "health": "unhealthy"}),
        );
        assert!(!a.ok && a.detail.contains("HEALTHCHECK"));
        let a =
            container_attempt(&serde_json::json!({"id": "abc", "running": false, "status": "exited", "health": null}));
        assert!(!a.ok && a.detail == "the container is exited");
    }

    #[test]
    fn descriptors_carry_no_environment() {
        let spec: AppSpec = serde_json::from_value(serde_json::json!({
            "environment": "uat", "name": "api", "runtime": "systemd", "command": "x",
            "env": [["DB_PASSWORD", "s3cret"]], "listen_port": 8080
        }))
        .unwrap();
        let d = Descriptor::from_spec(&spec);
        let text = serde_json::to_string(&d).unwrap();
        assert!(!text.contains("s3cret") && !text.contains("DB_PASSWORD"), "{text}");
        assert_eq!((d.listen_port, d.default_checks, d.has_readiness), (Some(8080), true, false));
    }

    #[test]
    fn reports_carry_changes_and_keepalives_only() {
        let r = Runner::default();
        let d = svc("api", None);
        r.record(&d, Attempt { ok: true, reason: String::new(), detail: String::new(), generation: "g".into() });
        let first = r.drain();
        assert_eq!(first.len(), 2, "liveness and readiness");
        assert!(r.drain().is_empty(), "nothing changed");
        for _ in 0..3 {
            r.record(
                &d,
                Attempt { ok: false, reason: "app_failure".into(), detail: String::new(), generation: "g".into() },
            );
        }
        let changed = r.drain();
        assert!(changed.iter().all(|h| h.state == "failing" && h.check == "default"), "{changed:?}");
    }
}
