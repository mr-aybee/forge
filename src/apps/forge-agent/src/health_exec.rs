//! Command health checks (exec and shell probes), run by the root helper
//! (issue #12, D-111).
//!
//! The invariant (D-085): a command check runs only as the service's
//! own account, confined, or inside the service's own container. Never
//! as root, the agent's user or the helper's user. Where this node cannot
//! guarantee that, the check does not run and the helper answers with a
//! [`Refusal`].
//!
//! **What the helper runs.** Never anything the agent sends. The helper
//! keeps the command checks of each app in its own root-only folder
//! (`<helper state>/health/`), taken from commands Forge signed: the app
//! command `health` ([`store`]) and a deploy ([`on_app_action`]). The agent
//! asks by reference only ([`CheckRef`]: app, purpose, check name and the
//! policy revision), so code running as the agent's user can start a check
//! Forge authorized, at the pace the helper allows, and nothing else. A
//! stored policy remembers when Forge issued it: an older command is
//! refused, and a policy the helper has not heard of for
//! [`MAX_POLICY_AGE`] is not run (Forge sends it again every 10 minutes).
//!
//! **Native apps.** The helper resolves the app's account on this node,
//! creates a cgroup for the run (`/sys/fs/cgroup/forge-health/<check>`,
//! with a process and a memory limit), starts its own binary as
//! `health-worker` under the app's uid and gid (no other groups, cleared
//! environment), moves it into the cgroup and only then hands it the
//! command on stdin. The worker checks its identity and confines itself
//! with Landlock before it starts the command in the app's folder with a
//! minimal environment: none of the app's own variables. When the run ends
//! or its timeout passes, the kernel kills everything in the cgroup: a
//! command cannot leave it by starting a session or a daemon.
//!
//! What Landlock allows a native check: read and execute beneath the
//! system folders and the app's folder, read a few devices, write only to
//! `/dev/null`. It cannot create, write, truncate, rename or delete a
//! file. It can still change the mode and times of files the app's account
//! owns (Landlock does not cover those), connect to sockets by path, and
//! use the network. Landlock ABI 3 (truncate) is the least accepted; a
//! node below that, or without cgroup v2, runs no command check.
//!
//! **Containers.** The helper runs the command with the Engine API's exec
//! in the app's own container (`forge-app-<ns>-<app>`, carrying Forge's
//! label for that app), as the container's default user, without a TTY or
//! a shell of ours.
//!
//! Both: the check's timeout ends the run, output is read to its end and
//! leaves the helper only when the check asks for it (`show_output`: a
//! short sanitized tail of a failed run), one check never runs twice at
//! once, and an app and the node each have a limit of running checks.

use std::path::{Path, PathBuf};
use std::time::Duration;

use forge_catalog::apps::{AppSpec, Runtime, valid_slug};
use forge_catalog::health::{ApplySpec, Check, HealthPolicy, Interpreter, Probe, Purpose};
use serde::{Deserialize, Serialize};

use crate::self_update::{HELPER_STATE, ensure_private_dir, read_nofollow, write_nofollow};

/// Command checks running at once on this node.
pub const MAX_RUNNING: usize = 4;
/// Command checks of one app running at once.
pub const MAX_PER_APP: usize = 2;
/// How long a check waits for a free place before it is turned away.
pub const QUEUE_WAIT: Duration = Duration::from_secs(2);
/// A stored policy older than this is not run: Forge sends the policy of
/// an app with command checks again every 10 minutes, so three sendings
/// were missed.
pub const MAX_POLICY_AGE: Duration = Duration::from_secs(30 * 60);
/// Longest output detail of a failed run that leaves the helper, in bytes.
pub const MAX_DETAIL: usize = 160;
/// Largest stored policy read back (30 checks of scripts and arguments).
const MAX_STORED: u64 = 16 * 1024 * 1024;

/// One check of one app, by reference: what the agent may ask the helper
/// to run. No command, arguments or script.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CheckRef {
    pub environment: String,
    pub app: String,
    pub purpose: Purpose,
    pub check: String,
    /// The policy revision the agent runs; the helper refuses another one
    /// than it holds.
    pub revision: i64,
}

/// A reference that names no app or check.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum RefError {
    #[error("invalid app name")]
    App,
    #[error("invalid check name")]
    Check,
}

impl CheckRef {
    pub fn validate(&self) -> Result<(), RefError> {
        if !valid_slug(&self.environment, 40) || !valid_slug(&self.app, 40) {
            return Err(RefError::App);
        }

        if !forge_catalog::health::valid_name(&self.check) {
            return Err(RefError::Check);
        }

        Ok(())
    }

    /// `<environment>/<app>/<purpose>/<check>`: the check in logs and locks.
    #[must_use]
    pub fn key(&self) -> String {
        format!("{}/{}/{}/{}", self.environment, self.app, self.purpose.as_str(), self.check)
    }
}

/// Why the helper did not run a check.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Refusal {
    /// The helper holds no signed policy for the app.
    NoPolicy,
    /// The helper holds another revision than the agent asked for.
    StalePolicy,
    /// The helper has not heard of the policy for [`MAX_POLICY_AGE`].
    PolicyExpired,
    /// The signed policy has no enabled command check of that name.
    NoSuchCheck,
    /// Native app: its account or folder is missing or not Forge's.
    NoAccount,
    /// Native app: the kernel does not enforce Landlock with truncate
    /// control (ABI 3).
    NotConfined,
    /// Native app: no cgroup v2 with process and memory limits to hold
    /// the run.
    NotContained,
    /// The worker did not start or did not answer as expected.
    WorkerFailed,
    /// Container app: Docker is not reachable.
    NoDocker,
    /// Container app: the container of that name is not Forge's for the app.
    ForeignContainer,
    /// Not a Linux node, or an app runtime that has no checks.
    Unsupported,
    /// The previous run of this check has not ended.
    Busy,
    /// No place became free in [`QUEUE_WAIT`], or this check was started
    /// less than a period ago: try the next period.
    Throttled,
}

impl Refusal {
    /// The sentence reported with the check (the console shows it).
    #[must_use]
    pub const fn detail(self) -> &'static str {
        match self {
            Self::NoPolicy => "this node's helper has no signed policy for the service yet",
            Self::StalePolicy => "this node's helper holds another policy revision; waiting for the current one",
            Self::PolicyExpired => {
                "this node's helper has not received the policy from Forge for 30 minutes; waiting for it"
            }
            Self::NoSuchCheck => "the signed policy on this node has no such command check",
            Self::NoAccount => "the service's own account or folder is missing on this node; deploy it again",
            Self::NotConfined => {
                "this node can't confine command checks (its kernel does not enforce Landlock ABI 3 or later)"
            }
            Self::NotContained => {
                "this node can't contain command checks (no cgroup v2 with the pids and memory controllers)"
            }
            Self::WorkerFailed => "the confined check worker could not start on this node",
            Self::NoDocker => "Docker is not reachable on this node",
            Self::ForeignContainer => "the container of that name on this node is not Forge's for this service",
            Self::Unsupported => "command checks need a Linux node and a service or container service",
            Self::Busy => "the previous run of this check has not ended",
            Self::Throttled => "too many command checks are running on this node",
        }
    }

    /// Whether the node will keep refusing until someone changes it (its
    /// kernel, its OS, the app's account). The other refusals pass on
    /// their own: a startup check waits for them.
    #[must_use]
    pub const fn permanent(self) -> bool {
        matches!(
            self,
            Self::NoAccount | Self::NotConfined | Self::NotContained | Self::ForeignContainer | Self::Unsupported
        )
    }
}

/// What the helper answers for one [`CheckRef`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "result", rename_all = "snake_case", deny_unknown_fields)]
pub enum ExecResult {
    /// The command ran to its end. `passed`: its exit code is one the
    /// policy expects. `output` is empty unless the check asks for the
    /// output of failed runs (`show_output`).
    Exited {
        code: i32,
        passed: bool,
        #[serde(default)]
        output: String,
    },
    /// A signal ended the command.
    Signaled {
        signal: i32,
    },
    /// The check's timeout ended it.
    TimedOut,
    /// The command could not be started (not found, not executable).
    StartFailed {
        detail: String,
    },
    /// Container app: the container is not running.
    NotRunning,
    Refused {
        why: Refusal,
    },
}

/// Why the helper did not take a policy.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum StoreError {
    #[error("invalid app name")]
    InvalidName,
    #[error("invalid health policy: {field}: {message}")]
    InvalidPolicy { field: String, message: String },
    /// Forge issued this command before the one the helper already holds.
    #[error("the helper holds a newer health policy for this service")]
    Older,
    #[error("{0}")]
    Io(String),
}

/// What the helper answers for a stored policy.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Applied {
    pub stored: bool,
    pub revision: i64,
    pub command_checks: usize,
}

/// The command checks of one app as Forge signed them, kept by the helper.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Stored {
    environment: String,
    name: String,
    runtime: Runtime,
    revision: i64,
    /// When Forge issued the command (its clock): an older one is refused.
    issued_at_ms: i64,
    /// When the helper last received it (its clock): see [`MAX_POLICY_AGE`].
    stored_at_ms: i64,
    checks: Vec<StoredCheck>,
    /// The slot whose container exec checks run in (#73); empty for the
    /// single instance.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    slot: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct StoredCheck {
    purpose: Purpose,
    check: Check,
}

/// When a policy was issued and when it arrived, in epoch milliseconds.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Times {
    pub issued_at_ms: i64,
    pub now_ms: i64,
}

impl Stored {
    fn new(environment: &str, name: &str, runtime: Runtime, revision: i64, policy: &HealthPolicy, at: Times) -> Self {
        let checks = policy
            .checks()
            .filter(|(_, c)| c.enabled && c.probe.runs_code())
            .map(|(purpose, c)| StoredCheck { purpose, check: c.clone() })
            .collect();
        Self {
            environment: environment.to_owned(),
            name: name.to_owned(),
            runtime,
            revision,
            issued_at_ms: at.issued_at_ms,
            stored_at_ms: at.now_ms,
            checks,
            slot: String::new(),
        }
    }
}

/// Where the helper keeps the signed command checks.
#[must_use]
pub fn state_dir() -> PathBuf {
    Path::new(HELPER_STATE).join("health")
}

fn file_of(dir: &Path, environment: &str, name: &str) -> PathBuf {
    // Slugs are `[a-z0-9-]` and never end in `-`, so `--` separates them.
    dir.join(format!("{environment}--{name}.json"))
}

fn load(dir: &Path, environment: &str, name: &str) -> Option<Stored> {
    let s: Stored = serde_json::from_slice(&read_nofollow(&file_of(dir, environment, name), MAX_STORED)?).ok()?;
    (s.environment == environment && s.name == name).then_some(s)
}

/// Writes the policy, unless the helper holds one Forge issued later. A
/// policy without command checks is written too: it is what remembers the
/// time of issue, so an older command with checks cannot come back.
fn save(dir: &Path, s: &Stored) -> Result<(), StoreError> {
    if !valid_slug(&s.environment, 40) || !valid_slug(&s.name, 40) {
        return Err(StoreError::InvalidName);
    }

    if load(dir, &s.environment, &s.name).is_some_and(|held| held.issued_at_ms > s.issued_at_ms) {
        return Err(StoreError::Older);
    }

    if let Some(parent) = dir.parent() {
        ensure_private_dir(parent).map_err(StoreError::Io)?;
    }

    ensure_private_dir(dir).map_err(StoreError::Io)?;
    let raw = serde_json::to_vec(s).map_err(|e| StoreError::Io(e.to_string()))?;

    write_nofollow(&file_of(dir, &s.environment, &s.name), &raw).map_err(StoreError::Io)
}

/// The signed app command `health`: replaces the command checks the helper
/// holds for the app. `spec` comes from a command the helper verified;
/// `at.issued_at_ms` is when Forge issued it.
pub fn store(dir: &Path, spec: &ApplySpec, at: Times) -> Result<Applied, StoreError> {
    if let Some(e) = spec.policy.validate().into_iter().next() {
        return Err(StoreError::InvalidPolicy { field: e.field, message: e.message });
    }

    let mut s = Stored::new(&spec.environment, &spec.name, spec.runtime, spec.revision, &spec.policy, at);
    s.slot.clone_from(&spec.slot);
    save(dir, &s)?;

    Ok(Applied { stored: true, revision: s.revision, command_checks: s.checks.len() })
}

/// Follows a signed deploy or removal: a deploy carries the app's policy
/// (revision 0, as the agent reports it until Forge sends the policy
/// again); a removal drops it. Best effort: it never fails the app action.
pub fn on_app_action(dir: &Path, action: &str, spec: &AppSpec, now_ms: i64) {
    // A slot (#73) gets its checks when traffic moves to it.
    if !spec.slot.is_empty() {
        return;
    }

    let r = match action {
        "deploy" => {
            let policy = spec.health.clone().unwrap_or_default();

            // A deploy is the newest word on the app: issued "now".
            let at = Times { issued_at_ms: now_ms, now_ms };
            save(dir, &Stored::new(&spec.environment, &spec.name, spec.runtime, 0, &policy, at))
        }
        "remove" => match std::fs::remove_file(file_of(dir, &spec.environment, &spec.name)) {
            Err(e) if e.kind() != std::io::ErrorKind::NotFound => Err(StoreError::Io(e.to_string())),
            _ => Ok(()),
        },
        _ => return,
    };

    if let Err(e) = r {
        tracing::warn!(app = %spec.key(), error = %e, "could not update the stored command checks");
    }
}

/// The stored check `r` names, or why there is none.
pub fn lookup(dir: &Path, r: &CheckRef, now_ms: i64) -> Result<(Runtime, Check, String), Refusal> {
    r.validate().map_err(|_| Refusal::NoSuchCheck)?;
    let s = load(dir, &r.environment, &r.app).ok_or(Refusal::NoPolicy)?;
    if s.revision != r.revision {
        return Err(Refusal::StalePolicy);
    }

    let max_age = i64::try_from(MAX_POLICY_AGE.as_millis()).unwrap_or(i64::MAX);
    if now_ms.saturating_sub(s.stored_at_ms) > max_age {
        return Err(Refusal::PolicyExpired);
    }

    let found = s.checks.into_iter().find(|c| c.purpose == r.purpose && c.check.name == r.check);

    found.map(|c| (s.runtime, c.check, s.slot)).ok_or(Refusal::NoSuchCheck)
}

/// The command of a native run: program and arguments, and what it reads
/// on stdin (a shell probe's script; the interpreter is named explicitly
/// and no shell of ours ever sees the text).
#[must_use]
pub fn native_command(probe: &Probe) -> Option<(Vec<String>, Option<String>)> {
    match probe {
        Probe::Exec(e) => Some((e.argv.clone(), None)),
        Probe::Shell(s) => Some((vec![interpreter(s.interpreter).to_owned()], Some(s.script.clone()))),
        Probe::Http(_) | Probe::Tcp(_) => None,
    }
}

/// The command of a run inside a container: the exec'd argv. A shell
/// probe's script is one argument of its interpreter (`sh -c <script>`):
/// the Engine API's exec gives a command no stdin without a hijacked
/// connection. The script is therefore in the process list, of the
/// container and of the host.
#[must_use]
pub fn container_command(probe: &Probe) -> Option<Vec<String>> {
    match probe {
        Probe::Exec(e) => Some(e.argv.clone()),
        Probe::Shell(s) => Some(vec![interpreter(s.interpreter).to_owned(), "-c".to_owned(), s.script.clone()]),
        Probe::Http(_) | Probe::Tcp(_) => None,
    }
}

const fn interpreter(i: Interpreter) -> &'static str {
    match i {
        Interpreter::Sh => "sh",
        Interpreter::Bash => "bash",
    }
}

/// The exit codes that pass, and whether a failed run's output is reported.
fn expectations(probe: &Probe) -> (&[i32], bool) {
    match probe {
        Probe::Exec(e) => (&e.expected_exit_codes, e.show_output),
        Probe::Shell(s) => (&s.expected_exit_codes, s.show_output),
        Probe::Http(_) | Probe::Tcp(_) => (&[], false),
    }
}

/// A run's output as it may leave the helper: its end, printable ASCII
/// only, runs of anything else as one space, at most [`MAX_DETAIL`] bytes.
#[must_use]
pub fn sanitize(output: &[u8]) -> String {
    let mut out = String::new();
    let mut gap = false;

    for b in output {
        if b.is_ascii_graphic() {
            if gap && !out.is_empty() {
                out.push(' ');
            }

            gap = false;
            out.push(char::from(*b));
        } else {
            gap = true;
        }
    }

    let cut = out.len().saturating_sub(MAX_DETAIL);

    out.split_off(cut)
}

/// The result of a run that ended with `code`. The output leaves the
/// helper only for a failed run of a check that asks for it.
#[must_use]
pub fn exited(probe: &Probe, code: i32, output: &[u8]) -> ExecResult {
    let (expected, show_output) = expectations(probe);
    let passed = expected.contains(&code);

    ExecResult::Exited { code, passed, output: if passed || !show_output { String::new() } else { sanitize(output) } }
}

#[cfg(not(target_os = "linux"))]
mod other {
    use super::{CheckRef, ExecResult, PathBuf, Refusal};

    /// Runs command checks; on this system it refuses them all.
    #[derive(Debug)]
    pub struct Runner;

    impl Runner {
        #[must_use]
        pub fn new(_dir: PathBuf) -> Self {
            Self
        }

        #[allow(clippy::unused_async)] // the Linux form awaits; one signature for both
        pub async fn run(&self, _r: &CheckRef) -> ExecResult {
            ExecResult::Refused { why: Refusal::Unsupported }
        }
    }
}

#[cfg(not(target_os = "linux"))]
pub use other::Runner;

#[cfg(target_os = "linux")]
pub use linux::{RunOptions, Runner, landlock_abi, worker_main};

#[cfg(target_os = "linux")]
mod linux {
    use std::collections::HashMap;
    use std::sync::{Arc, Mutex};
    use std::time::{Duration, Instant};

    use forge_core::{Clock, SystemClock};
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::sync::{OwnedSemaphorePermit, Semaphore};

    use super::{
        Check, CheckRef, Deserialize, ExecResult, MAX_PER_APP, MAX_RUNNING, Path, PathBuf, QUEUE_WAIT, Refusal,
        Runtime, Serialize, container_command, exited, lookup, native_command,
    };

    /// Largest command the worker reads (64 arguments of 4096 bytes, or a
    /// 16 KiB script).
    const MAX_JOB: usize = 512 * 1024;
    /// Largest answer the helper reads from a worker.
    const MAX_ANSWER: usize = 16 * 1024;
    /// The shortest time between two starts of one check: a policy's
    /// shortest period. The agent keeps the period; this holds for anyone
    /// else who can reach the helper's socket.
    const MIN_INTERVAL: Duration = Duration::from_millis(900);
    /// Bytes of a run's output kept (its end); the rest is read and dropped.
    const TAIL: usize = 512;
    /// What a command finds on its `PATH`.
    const PATH: &str = "/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin";
    /// Readable and executable for a command, besides the app's folder.
    const SYSTEM: &[&str] = &["/usr", "/bin", "/sbin", "/lib", "/lib32", "/lib64", "/etc", "/opt", "/proc", "/sys"];
    /// Devices a command may read.
    const DEVICES: &[&str] = &["/dev/zero", "/dev/random", "/dev/urandom"];
    /// The least Landlock ABI a command check runs under: the first that
    /// controls truncation, so a check can neither write nor empty a file.
    const MIN_LANDLOCK_ABI: u8 = 3;
    /// The cgroup v2 mount and the group that holds one cgroup per run.
    const CGROUP_ROOT: &str = "/sys/fs/cgroup";
    const CGROUP_PARENT: &str = "forge-health";
    /// Processes and threads one run may have at once (worker included).
    const PIDS_MAX: u32 = 64;
    /// Memory one run may use.
    const MEMORY_MAX: u64 = 256 * 1024 * 1024;

    /// How runs are started (tests point the worker at a copy of the
    /// helper that the app's account can execute; production uses
    /// [`Default`]).
    #[derive(Debug, Clone, Default)]
    #[doc(hidden)]
    pub struct RunOptions {
        /// The helper binary to start as `health-worker` (default: this one).
        pub worker_exe: Option<PathBuf>,
        /// Lets the worker run without Landlock. Exists only in builds
        /// with the `test-unconfined` feature, which the root test of a
        /// kernel without Landlock uses; release builds do not have it.
        #[cfg(feature = "test-unconfined")]
        pub allow_unconfined: bool,
    }

    impl RunOptions {
        #[cfg(feature = "test-unconfined")]
        const fn unconfined(&self) -> bool {
            self.allow_unconfined
        }

        #[cfg(not(feature = "test-unconfined"))]
        #[allow(clippy::unused_self)] // one signature with the test build
        const fn unconfined(&self) -> bool {
            false
        }
    }

    /// One check's place: whether it runs now, and when it last started.
    #[derive(Debug)]
    struct Place {
        active: bool,
        started: Instant,
    }

    /// The helper's state for command checks: the signed policies' folder,
    /// who runs now, and the runs that could not be ended. One per helper
    /// process, owned by its serve loop.
    #[derive(Debug)]
    pub struct Runner {
        dir: PathBuf,
        opts: RunOptions,
        node: Arc<Semaphore>,
        apps: Mutex<HashMap<String, Arc<Semaphore>>>,
        places: Arc<Mutex<HashMap<String, Place>>>,
        stuck: Arc<crate::docker::StuckRuns>,
    }

    /// Holds a check's place while it runs, so it cannot overlap itself.
    struct Running {
        places: Arc<Mutex<HashMap<String, Place>>>,
        key: String,
    }

    impl Drop for Running {
        fn drop(&mut self) {
            if let Ok(mut all) = self.places.lock()
                && let Some(p) = all.get_mut(&self.key)
            {
                p.active = false;
            }
        }
    }

    impl Runner {
        #[must_use]
        pub fn new(dir: PathBuf) -> Self {
            Self::with_options(dir, RunOptions::default())
        }

        #[doc(hidden)]
        #[must_use]
        pub fn with_options(dir: PathBuf, opts: RunOptions) -> Self {
            Self {
                dir,
                opts,
                node: Arc::new(Semaphore::new(MAX_RUNNING)),
                apps: Mutex::new(HashMap::new()),
                places: Arc::new(Mutex::new(HashMap::new())),
                stuck: Arc::new(crate::docker::StuckRuns::default()),
            }
        }

        /// Takes the check's place, or says why not: it runs now, or it
        /// started less than [`MIN_INTERVAL`] ago.
        fn enter(&self, key: String, now: Instant) -> Result<Running, Refusal> {
            // The lock is released before a `Running` exists: its `Drop`
            // takes the lock, and must only ever free a place it took.
            {
                let mut all = self.places.lock().map_err(|_| Refusal::WorkerFailed)?;

                // Places of checks that are gone do not pile up.
                all.retain(|_, p| p.active || now.saturating_duration_since(p.started) < Duration::from_secs(3600));

                match all.get_mut(&key) {
                    Some(p) if p.active => return Err(Refusal::Busy),
                    Some(p) if now.saturating_duration_since(p.started) < MIN_INTERVAL => {
                        return Err(Refusal::Throttled);
                    }
                    Some(p) => *p = Place { active: true, started: now },
                    None => {
                        all.insert(key.clone(), Place { active: true, started: now });
                    }
                }
            }

            Ok(Running { places: self.places.clone(), key })
        }

        /// The app's limit of running checks.
        fn app_limit(&self, r: &CheckRef) -> Result<Arc<Semaphore>, Refusal> {
            let mut apps = self.apps.lock().map_err(|_| Refusal::WorkerFailed)?;

            // Limits nobody holds are dropped; a new one starts full.
            apps.retain(|_, s| Arc::strong_count(s) > 1);
            let s = apps
                .entry(format!("{}/{}", r.environment, r.app))
                .or_insert_with(|| Arc::new(Semaphore::new(MAX_PER_APP)));

            Ok(s.clone())
        }

        /// Waits up to [`QUEUE_WAIT`] for a place of the app and of the node.
        async fn queue(&self, r: &CheckRef) -> Result<(OwnedSemaphorePermit, OwnedSemaphorePermit), Refusal> {
            let app = self.app_limit(r)?;
            let both = async {
                let a = app.acquire_owned().await.ok()?;
                let n = self.node.clone().acquire_owned().await.ok()?;
                Some((a, n))
            };

            match tokio::time::timeout(QUEUE_WAIT, both).await {
                Ok(Some(permits)) => Ok(permits),
                Ok(None) | Err(_) => Err(Refusal::Throttled),
            }
        }

        /// Runs the check `r` names. Never fails: every problem is an
        /// [`ExecResult`].
        pub async fn run(&self, r: &CheckRef) -> ExecResult {
            let (dir, wanted, now_ms) = (self.dir.clone(), r.clone(), SystemClock.now().0);
            let found = tokio::task::spawn_blocking(move || lookup(&dir, &wanted, now_ms)).await;
            let (runtime, check, slot) = match found {
                Ok(Ok(f)) => f,
                Ok(Err(why)) => return ExecResult::Refused { why },
                Err(_) => return ExecResult::Refused { why: Refusal::WorkerFailed },
            };

            let _running = match self.enter(r.key(), Instant::now()) {
                Ok(place) => place,
                Err(why) => return ExecResult::Refused { why },
            };

            let _permits = match self.queue(r).await {
                Ok(p) => p,
                Err(why) => return ExecResult::Refused { why },
            };

            match runtime {
                Runtime::Systemd => native(r, &check, &self.opts).await,
                Runtime::Container => container(r, &check, &slot, &self.stuck).await,
            }
        }
    }

    /// The cgroup of one run: everything the worker starts stays in it,
    /// whatever session or process group it makes for itself, and the
    /// kernel kills all of it at once.
    struct Cage {
        path: PathBuf,
    }

    impl Cage {
        /// Creates (or empties and reuses) the cgroup for `r` with its
        /// limits. `None` where this node has no cgroup v2 that can do it.
        async fn create(r: &CheckRef) -> Option<Self> {
            use tokio::fs;
            let root = Path::new(CGROUP_ROOT);
            let has =
                |controllers: &str| ["pids", "memory"].iter().all(|c| controllers.split_whitespace().any(|x| x == *c));

            // cgroup v2 only: v1 and hybrid layouts have no such file here.
            let available = fs::read_to_string(root.join("cgroup.controllers")).await.ok()?;
            if !has(&available) {
                return None;
            }

            let parent = root.join(CGROUP_PARENT);
            match fs::create_dir(&parent).await {
                Ok(()) => {}
                Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {}
                Err(_) => return None,
            }

            // The controllers must be handed down twice: root to the
            // parent, the parent to the run. Writing what is already set
            // is not an error; whether it worked is read back.
            let _ = fs::write(root.join("cgroup.subtree_control"), "+pids +memory").await;

            if !has(&fs::read_to_string(parent.join("cgroup.controllers")).await.ok()?) {
                return None;
            }

            let _ = fs::write(parent.join("cgroup.subtree_control"), "+pids +memory").await;
            let path = parent.join(format!("{}--{}--{}--{}", r.environment, r.app, r.purpose.as_str(), r.check));
            let cage = Self { path };
            match fs::create_dir(&cage.path).await {
                Ok(()) => {}
                // Left by a run that was cut short: nothing of it may live on.
                Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => cage.kill(),
                Err(_) => return None,
            }

            // Without the kill switch and both limits there is no cage.
            if fs::metadata(cage.path.join("cgroup.kill")).await.is_err() {
                return None;
            }

            fs::write(cage.path.join("pids.max"), PIDS_MAX.to_string()).await.ok()?;
            fs::write(cage.path.join("memory.max"), MEMORY_MAX.to_string()).await.ok()?;

            // Where the kernel has them: no swap, and an out-of-memory kill takes the whole run.
            let _ = fs::write(cage.path.join("memory.swap.max"), "0").await;
            let _ = fs::write(cage.path.join("memory.oom.group"), "1").await;

            Some(cage)
        }

        /// Moves a process into the cgroup; its children are born in it.
        async fn add(&self, pid: u32) -> bool {
            tokio::fs::write(self.path.join("cgroup.procs"), pid.to_string()).await.is_ok()
        }

        /// Kills every process in the cgroup. A write to a cgroup control
        /// file does not block, so it is done in place, also from `Drop`.
        fn kill(&self) {
            let _ = std::fs::write(self.path.join("cgroup.kill"), "1");
        }

        /// Kills what is left and removes the cgroup once it is empty.
        async fn end(self) {
            self.kill();
            for _ in 0..50 {
                if tokio::fs::remove_dir(&self.path).await.is_ok() {
                    return;
                }

                tokio::time::sleep(Duration::from_millis(20)).await;
            }
            // Still dying: the next run of the check empties and reuses it.
        }
    }

    impl Drop for Cage {
        fn drop(&mut self) {
            // Also when the agent disconnects mid-run and the future is dropped.
            self.kill();
        }
    }

    /// What the helper hands the worker on stdin.
    #[derive(Debug, Serialize, Deserialize)]
    #[serde(deny_unknown_fields)]
    struct Job {
        argv: Vec<String>,
        /// A shell probe's script, read by its interpreter.
        stdin: Option<String>,
        /// The app's account name (for `USER` and `LOGNAME`).
        account: String,
    }

    /// The worker's one-line answer.
    #[derive(Debug, Serialize, Deserialize)]
    #[serde(deny_unknown_fields)]
    struct Answer {
        /// Landlock: `full`, `partial`, `none` or `error: <reason>`.
        confined: String,
        /// The Landlock ABI of the kernel the worker confined itself
        /// under (0: none).
        abi: u8,
        outcome: WorkerOutcome,
    }

    #[derive(Debug, Serialize, Deserialize)]
    #[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
    enum WorkerOutcome {
        /// The worker is not the app's account, or got no valid command.
        Refused {
            reason: String,
        },
        /// Landlock is not enforced as required: nothing was started.
        NotConfined,
        StartFailed {
            detail: String,
        },
        Exited {
            code: i32,
            output: Vec<u8>,
        },
        Signaled {
            signal: i32,
        },
    }

    async fn native(r: &CheckRef, check: &Check, opts: &RunOptions) -> ExecResult {
        let refused = |why| ExecResult::Refused { why };
        let Some((argv, stdin)) = native_command(&check.probe) else { return refused(Refusal::NoSuchCheck) };

        // The account is resolved here, on this node, never from a request.
        let Ok((account, uid, gid)) = crate::apps::app_account(&r.environment, &r.app).await else {
            return refused(Refusal::NoAccount);
        };

        if uid == 0 || gid == 0 {
            return refused(Refusal::NoAccount);
        }

        let base = forge_catalog::apps::app_home(&r.environment, &r.app);

        let Ok(input) = serde_json::to_vec(&Job { argv, stdin, account }) else {
            return refused(Refusal::WorkerFailed);
        };

        // This binary, by its absolute path: never a PATH lookup.
        let exe = match &opts.worker_exe {
            Some(p) => p.clone(),
            None => match std::env::current_exe() {
                Ok(p) => p,
                Err(_) => return refused(Refusal::WorkerFailed),
            },
        };

        // Before anything runs as the app: without a cage, no run.
        let Some(cage) = Cage::create(r).await else { return refused(Refusal::NotContained) };
        let limit = Duration::from_secs(u64::from(check.timeout_secs.max(1)));
        let mut cmd = tokio::process::Command::new(exe);
        cmd.args(["health-worker", &uid.to_string(), &gid.to_string(), &base, &limit.as_secs().to_string()]);

        if opts.unconfined() {
            cmd.arg(UNCONFINED_OK);
        }

        // Setting the uid as root also clears the supplementary groups; the
        // worker checks all of it again before it starts anything.
        let spawned = cmd
            .uid(uid)
            .gid(gid)
            .env_clear()
            .current_dir("/")
            .process_group(0)
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::null())
            .kill_on_drop(true)
            .spawn();
        let Ok(mut child) = spawned else { return refused(Refusal::WorkerFailed) };

        // The worker starts nothing before it has read its command, and it
        // gets the command only once it is in the cage.
        let caged = match child.id() {
            Some(pid) => cage.add(pid).await,
            None => false,
        };

        let (Some(mut stdin), Some(mut stdout), true) = (child.stdin.take(), child.stdout.take(), caged) else {
            let _ = child.kill().await;
            cage.end().await;

            return refused(if caged { Refusal::WorkerFailed } else { Refusal::NotContained });
        };

        let exchange = async {
            stdin.write_all(&input).await.ok()?;
            drop(stdin);
            let mut out = Vec::new();
            (&mut stdout).take(MAX_ANSWER as u64 + 1).read_to_end(&mut out).await.ok()?;
            (out.len() <= MAX_ANSWER).then_some(out)
        };
        let answer = tokio::time::timeout(limit, exchange).await;

        // On a timeout and after a normal end alike: whatever the command
        // started dies with the cage, wherever it moved itself.
        cage.end().await;
        let _ = tokio::time::timeout(Duration::from_secs(2), child.wait()).await;

        let out = match answer {
            Err(_) => return ExecResult::TimedOut,
            Ok(None) => return refused(Refusal::WorkerFailed),
            Ok(Some(out)) => out,
        };

        let Ok(answer) = serde_json::from_slice::<Answer>(&out) else { return refused(Refusal::WorkerFailed) };
        let enforced = answer.abi >= MIN_LANDLOCK_ABI && matches!(answer.confined.as_str(), "full" | "partial");
        tracing::debug!(app = %r.key(), landlock = %answer.confined.chars().take(40).collect::<String>(), abi = answer.abi,
            "command check worker answered");

        match answer.outcome {
            WorkerOutcome::NotConfined => {
                tracing::warn!(landlock = %answer.confined.chars().take(200).collect::<String>(), abi = answer.abi,
                    "command checks do not run on this node: Landlock ABI 3 is not enforced");
                refused(Refusal::NotConfined)
            }
            WorkerOutcome::Refused { reason } => {
                tracing::warn!(app = %r.key(), reason = %reason.chars().take(200).collect::<String>(),
                    "the health check worker refused to run");
                refused(Refusal::WorkerFailed)
            }
            // A worker that says it ran without confinement is not believed
            // (unless a test build allowed exactly that).
            _ if !enforced && !opts.unconfined() => refused(Refusal::NotConfined),
            WorkerOutcome::StartFailed { detail } => {
                ExecResult::StartFailed { detail: super::sanitize(detail.as_bytes()) }
            }
            WorkerOutcome::Exited { code, output } => exited(&check.probe, code, &output),
            WorkerOutcome::Signaled { signal } => ExecResult::Signaled { signal },
        }
    }

    async fn container(r: &CheckRef, check: &Check, slot: &str, stuck: &Arc<crate::docker::StuckRuns>) -> ExecResult {
        use crate::docker::{ExecEnd, ExecTarget, exec_in_app};

        let Some(cmd) = container_command(&check.probe) else {
            return ExecResult::Refused { why: Refusal::NoSuchCheck };
        };

        let target = ExecTarget {
            container: if slot.is_empty() {
                format!("forge-app-{}-{}", r.environment, r.app)
            } else {
                format!("forge-app-{}-{}--{slot}", r.environment, r.app)
            },
            // The label Forge gives the app's container: the name alone
            // cannot tell `a-b`/`c` from `a`/`b-c`.
            app_label: format!("{}/{}", r.environment, r.app),
            check: r.key(),
        };
        let limit = Duration::from_secs(u64::from(check.timeout_secs.max(1)));
        match exec_in_app(&target, &cmd, limit, stuck).await {
            Ok(ExecEnd::Exited { code, output }) => exited(&check.probe, code, &output),
            Ok(ExecEnd::TimedOut) => ExecResult::TimedOut,
            Ok(ExecEnd::NotRunning) => ExecResult::NotRunning,
            Ok(ExecEnd::Foreign) => ExecResult::Refused { why: Refusal::ForeignContainer },
            Ok(ExecEnd::StillRunning) => ExecResult::Refused { why: Refusal::Busy },
            Ok(ExecEnd::StartFailed { detail }) => {
                ExecResult::StartFailed { detail: super::sanitize(detail.as_bytes()) }
            }
            Err(e) => {
                tracing::debug!(app = %r.key(), error = %e, "docker exec for a health check failed");
                ExecResult::Refused { why: Refusal::NoDocker }
            }
        }
    }

    /// The worker argument that lets it run without Landlock (test builds).
    const UNCONFINED_OK: &str = "--unconfined-ok";

    /// `forge-agent-helper health-worker <uid> <gid> <base> <timeout-secs>`:
    /// one command from stdin, its answer as one JSON line on stdout.
    #[must_use]
    pub fn worker_main(args: &[String]) -> i32 {
        let mut confined = (String::from("not attempted"), 0);
        let outcome = worker(args, &mut confined).unwrap_or_else(|e| WorkerOutcome::Refused { reason: e.to_string() });
        match serde_json::to_string(&Answer { confined: confined.0, abi: confined.1, outcome }) {
            Ok(line) => {
                println!("{line}");
                0
            }
            Err(_) => 1,
        }
    }

    /// Splits off the test-only argument; release builds know no such one.
    #[cfg(feature = "test-unconfined")]
    fn unconfined_ok(args: &[String]) -> (&[String], bool) {
        match args.split_last() {
            Some((last, rest)) if last == UNCONFINED_OK => (rest, true),
            _ => (args, false),
        }
    }

    #[cfg(not(feature = "test-unconfined"))]
    const fn unconfined_ok(args: &[String]) -> (&[String], bool) {
        (args, false)
    }

    /// Why the worker started nothing. The helper logs the sentence and
    /// answers `Refusal::WorkerFailed` for every one of them.
    #[derive(Debug, thiserror::Error)]
    enum WorkerError {
        #[error("usage: health-worker <uid> <gid> <base> <timeout-secs>")]
        Usage,
        #[error("bad {0}")]
        Argument(&'static str),
        /// Not every id of this process is the app's (the file worker's check).
        #[error("{0}")]
        Identity(String),
        #[error("the command is too large")]
        TooLarge,
        #[error("invalid command: {0}")]
        Command(#[from] serde_json::Error),
        #[error("{0}")]
        Io(#[from] std::io::Error),
    }

    fn worker(args: &[String], confined: &mut (String, u8)) -> Result<WorkerOutcome, WorkerError> {
        let (args, unconfined_ok) = unconfined_ok(args);
        let [uid, gid, base, timeout] = args else { return Err(WorkerError::Usage) };
        let uid: u32 = uid.parse().map_err(|_| WorkerError::Argument("uid"))?;
        let gid: u32 = gid.parse().map_err(|_| WorkerError::Argument("gid"))?;
        let timeout: u64 = timeout.parse().map_err(|_| WorkerError::Argument("timeout"))?;
        let status = std::fs::read_to_string("/proc/self/status")?;
        crate::files::app_worker::check_identity(&status, uid, gid).map_err(WorkerError::Identity)?;
        limit_resources(timeout);

        // Before anything is started: Landlock binds this process and every
        // child, and sets `no_new_privs`, so no setuid program gains rights.
        let abi = landlock_abi();
        *confined = (confine(base), abi);
        let enforced = abi >= MIN_LANDLOCK_ABI && matches!(confined.0.as_str(), "full" | "partial");
        if !enforced && !unconfined_ok {
            return Ok(WorkerOutcome::NotConfined);
        }

        let mut input = Vec::new();
        std::io::Read::read_to_end(&mut std::io::Read::take(std::io::stdin(), MAX_JOB as u64 + 1), &mut input)?;

        if input.len() > MAX_JOB {
            return Err(WorkerError::TooLarge);
        }

        let job: Job = serde_json::from_slice(&input)?;

        Ok(run_command(&job, base))
    }

    /// Starts the command in the app's folder with a minimal environment and
    /// waits for it. Its output is read to the end; only the end is kept.
    fn run_command(job: &Job, base: &str) -> WorkerOutcome {
        use std::io::{Read as _, Write as _};
        use std::os::unix::process::ExitStatusExt as _;
        let failed = |detail: String| WorkerOutcome::StartFailed { detail };
        let Some((program, rest)) = job.argv.split_first() else { return failed("no program".into()) };
        let mut cmd = std::process::Command::new(program);

        // None of the app's own variables: a check gets what any process of
        // the account would find, not the app's secrets.
        cmd.args(rest)
            .env_clear()
            .env("PATH", PATH)
            .env("HOME", base)
            .env("USER", &job.account)
            .env("LOGNAME", &job.account)
            .env("LANG", "C.UTF-8")
            .current_dir(base);

        let (mut reader, writer) = match std::io::pipe() {
            Ok(p) => p,
            Err(e) => return failed(e.to_string()),
        };

        match writer.try_clone() {
            Ok(w) => cmd.stdout(w).stderr(writer),
            Err(e) => return failed(e.to_string()),
        };

        match &job.stdin {
            None => cmd.stdin(std::process::Stdio::null()),
            Some(script) => {
                // A script fits in the pipe's buffer (at most 16 KiB), so
                // writing it before the interpreter starts cannot block.
                let filled = std::io::pipe().and_then(|(r, mut w)| w.write_all(script.as_bytes()).map(|()| r));
                match filled {
                    Ok(r) => cmd.stdin(r),
                    Err(e) => return failed(e.to_string()),
                }
            }
        };

        let mut child = match cmd.spawn() {
            Ok(c) => c,
            Err(e) => return failed(format!("cannot start `{program}`: {e}")),
        };

        // Close this process's ends of the output pipe, or it never ends.
        drop(cmd);
        let mut tail: Vec<u8> = Vec::new();
        let mut chunk = [0u8; 8192];

        loop {
            match reader.read(&mut chunk) {
                Ok(0) | Err(_) => break,
                Ok(n) => {
                    tail.extend_from_slice(chunk.get(..n).unwrap_or_default());
                    let cut = tail.len().saturating_sub(TAIL);
                    tail.drain(..cut);
                }
            }
        }

        match child.wait() {
            Ok(s) => match (s.code(), s.signal()) {
                (Some(code), _) => WorkerOutcome::Exited { code, output: tail },
                (None, Some(signal)) => WorkerOutcome::Signaled { signal },
                (None, None) => failed("the command ended without a status".into()),
            },
            Err(e) => failed(e.to_string()),
        }
    }

    /// The Landlock ABI this kernel has (0: none), found by asking for one
    /// right of each version as a hard requirement. Creating a ruleset
    /// restricts nothing.
    #[must_use]
    pub fn landlock_abi() -> u8 {
        use landlock::{AccessFs, AccessNet, CompatLevel, Compatible, Ruleset, RulesetAttr, Scope};
        let hard = || Ruleset::default().set_compatibility(CompatLevel::HardRequirement);
        let fs = |right: AccessFs| hard().handle_access(right).and_then(|r| r.create()).is_ok();

        if hard().handle_access(AccessFs::Execute).and_then(|r| r.scope(Scope::Signal)).and_then(|r| r.create()).is_ok()
        {
            6
        } else if fs(AccessFs::IoctlDev) {
            5
        } else if hard().handle_access(AccessNet::BindTcp).and_then(|r| r.create()).is_ok() {
            4
        } else if fs(AccessFs::Truncate) {
            3
        } else if fs(AccessFs::Refer) {
            2
        } else {
            u8::from(fs(AccessFs::Execute))
        }
    }

    /// Landlock: read and execute beneath the system folders and the app's
    /// folder, read a few devices, write only to `/dev/null`. No rule
    /// allows creating, writing, truncating, renaming or removing a file
    /// anywhere, and every filesystem right through ABI 3 (truncate) is a
    /// hard requirement: a kernel without them is an error here, never a
    /// weaker run. Device ioctls (ABI 5) and the scoping of signals and
    /// abstract UNIX sockets (ABI 6) are added where the kernel has them.
    /// Not covered by Landlock at any ABI: changing the mode, owner or
    /// times of a file the account owns, connecting to a socket by path,
    /// and the network, which is left open because most checks ask the app
    /// over it. Returns the level reached, or `error: <reason>`.
    fn confine(base: &str) -> String {
        use landlock::{
            ABI, Access, AccessFs, CompatLevel, Compatible, Ruleset, RulesetAttr, RulesetCreatedAttr, RulesetError,
            RulesetStatus, Scope, path_beneath_rules,
        };
        let read = AccessFs::from_read(ABI::V3);
        let null = AccessFs::ReadFile | AccessFs::WriteFile | AccessFs::Truncate | AccessFs::IoctlDev;
        let status = (|| -> Result<RulesetStatus, RulesetError> {
            Ok(Ruleset::default()
                .set_compatibility(CompatLevel::HardRequirement)
                .handle_access(AccessFs::from_all(ABI::V3))?
                .set_compatibility(CompatLevel::BestEffort)
                .handle_access(AccessFs::IoctlDev)?
                .scope(Scope::AbstractUnixSocket | Scope::Signal)?
                .create()?
                .add_rules(path_beneath_rules(SYSTEM.iter().copied().chain([base]), read))?
                .add_rules(path_beneath_rules(DEVICES, AccessFs::ReadFile))?
                .add_rules(path_beneath_rules(["/dev/null"], null))?
                .restrict_self()?
                .ruleset)
        })();

        match status {
            Ok(RulesetStatus::FullyEnforced) => "full".into(),
            Ok(RulesetStatus::PartiallyEnforced) => "partial".into(),
            Ok(RulesetStatus::NotEnforced) => "none".into(),
            Err(e) => format!("error: {}", e.to_string().chars().take(200).collect::<String>()),
        }
    }

    /// No core dumps, bounded open files, CPU time and file size. Best
    /// effort: the helper's timeout and the cage hold either way.
    fn limit_resources(timeout_secs: u64) {
        use rustix::process::{Resource, Rlimit, setrlimit};
        let set = |r: Resource, v: u64| {
            let _ = setrlimit(r, Rlimit { current: Some(v), maximum: Some(v) });
        };
        set(Resource::Core, 0);
        set(Resource::Nofile, 256);
        set(Resource::Cpu, timeout_secs.saturating_add(5));
        set(Resource::Fsize, 16 * 1024 * 1024);
    }

    #[cfg(test)]
    #[allow(clippy::unwrap_used)]
    mod tests {
        use super::{CheckRef, Duration, Instant, MAX_PER_APP, MIN_INTERVAL, Refusal, Runner};

        fn check(app: &str, name: &str) -> CheckRef {
            CheckRef {
                environment: "uat".into(),
                app: app.into(),
                purpose: forge_catalog::health::Purpose::Readiness,
                check: name.into(),
                revision: 1,
            }
        }

        /// A second run of a check is turned away, turning it away neither
        /// blocks nor frees the first run's place, and starts keep their
        /// distance whoever asks.
        #[test]
        fn a_check_holds_its_place_and_its_pace() {
            let r = Runner::new(std::env::temp_dir());
            let key = || check("api", "one").key();
            let t0 = Instant::now();
            let first = r.enter(key(), t0).unwrap();
            assert_eq!(r.enter(key(), t0).err(), Some(Refusal::Busy));
            assert_eq!(r.enter(key(), t0).err(), Some(Refusal::Busy), "a refusal frees nothing");
            drop(first);
            assert_eq!(r.enter(key(), t0 + Duration::from_millis(100)).err(), Some(Refusal::Throttled));
            let again = r.enter(key(), t0 + MIN_INTERVAL).unwrap();

            // Another check is not held up by this one.
            assert!(r.enter(check("api", "two").key(), t0).is_ok());
            drop(again);

            // Places of checks nobody ran for a long time are forgotten.
            let later = t0 + Duration::from_secs(7200);
            let _new = r.enter(check("api", "three").key(), later).unwrap();
            assert_eq!(r.places.lock().unwrap().len(), 1);
        }

        /// One app cannot take every place on the node, and a check waits
        /// a short while for a place before it is turned away.
        #[tokio::test(start_paused = true)]
        async fn an_app_has_its_own_limit_inside_the_nodes() {
            let r = Runner::new(std::env::temp_dir());
            let mut held = Vec::new();

            for i in 0..MAX_PER_APP {
                held.push(r.queue(&check("api", &format!("c{i}"))).await.unwrap());
            }

            // The app is full: another of its checks waits, then is turned away.
            let started = tokio::time::Instant::now();
            assert_eq!(r.queue(&check("api", "more")).await.err(), Some(Refusal::Throttled));
            assert!(started.elapsed() >= super::QUEUE_WAIT);

            // Another app still gets a place at once.
            let other = r.queue(&check("web", "c")).await.unwrap();

            // A place that frees up within the wait is taken.
            let more = check("api", "more");
            let (got, ()) = tokio::join!(r.queue(&more), async {
                tokio::time::sleep(Duration::from_millis(500)).await;
                held.pop();
            });
            assert!(got.is_ok());
            drop(other);
        }
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    const AT: Times = Times { issued_at_ms: 1_000, now_ms: 5_000 };

    fn policy(v: serde_json::Value) -> HealthPolicy {
        serde_json::from_value(v).unwrap()
    }

    fn spec(revision: i64, p: serde_json::Value) -> ApplySpec {
        ApplySpec {
            environment: "uat".into(),
            name: "api".into(),
            runtime: Runtime::Systemd,
            listen_port: None,
            policy: policy(p),
            revision,
            fleet: Vec::new(),
            slot: String::new(),
        }
    }

    fn check_ref(purpose: Purpose, check: &str, revision: i64) -> CheckRef {
        CheckRef { environment: "uat".into(), app: "api".into(), purpose, check: check.into(), revision }
    }

    fn commands() -> serde_json::Value {
        serde_json::json!({
            "readiness": [
                { "name": "cmd", "probe": { "type": "exec", "argv": ["/bin/true", "-x"], "expected_exit_codes": [0, 3] } },
                { "name": "off", "enabled": false, "probe": { "type": "exec", "argv": ["/bin/false"] } },
                { "name": "tcp", "probe": { "type": "tcp", "port": 80 } }
            ],
            "liveness": [{ "name": "cmd", "probe": { "type": "shell", "interpreter": "bash", "script": "exit 0" } }]
        })
    }

    fn tcp_only() -> serde_json::Value {
        serde_json::json!({ "liveness": [{ "name": "t", "probe": { "type": "tcp", "port": 1 } }] })
    }

    /// The helper keeps only what Forge signed, and runs by reference: the
    /// check must be an enabled command check of the stored revision.
    #[test]
    fn checks_are_found_by_reference_in_the_signed_policy_only() {
        let dir = tempfile::tempdir().unwrap();
        let state = dir.path().join("health");
        let now = AT.now_ms;
        assert_eq!(lookup(&state, &check_ref(Purpose::Readiness, "cmd", 7), now), Err(Refusal::NoPolicy));
        let stored = store(&state, &spec(7, commands()), AT).unwrap();
        assert_eq!(stored, Applied { stored: true, revision: 7, command_checks: 2 });
        let (runtime, c, _) = lookup(&state, &check_ref(Purpose::Readiness, "cmd", 7), now).unwrap();
        assert_eq!((runtime, c.name.as_str()), (Runtime::Systemd, "cmd"));

        // Same name, other purpose: the other check.
        let (_, live, _) = lookup(&state, &check_ref(Purpose::Liveness, "cmd", 7), now).unwrap();
        assert!(matches!(live.probe, Probe::Shell(_)));

        // Another revision, a disabled check, a check that runs no code, an
        // unknown name, another app and a name that is no slug: refused.
        assert_eq!(lookup(&state, &check_ref(Purpose::Readiness, "cmd", 8), now), Err(Refusal::StalePolicy));

        for name in ["off", "tcp", "nope"] {
            assert_eq!(
                lookup(&state, &check_ref(Purpose::Readiness, name, 7), now),
                Err(Refusal::NoSuchCheck),
                "{name}"
            );
        }

        assert_eq!(lookup(&state, &check_ref(Purpose::Startup, "cmd", 7), now), Err(Refusal::NoSuchCheck));
        let other = CheckRef { app: "web".into(), ..check_ref(Purpose::Readiness, "cmd", 7) };
        assert_eq!(lookup(&state, &other, now), Err(Refusal::NoPolicy));
        let escape = CheckRef { app: "../x".into(), ..check_ref(Purpose::Readiness, "cmd", 7) };
        assert!(lookup(&state, &escape, now).is_err() && escape.validate().is_err());

        // An invalid policy is refused.
        let bad =
            spec(1, serde_json::json!({ "liveness": [{ "name": "X", "probe": { "type": "exec", "argv": ["a"] } }] }));
        assert!(matches!(store(&state, &bad, AT), Err(StoreError::InvalidPolicy { .. })));
    }

    /// A stored policy is not run for ever, and an older signed command
    /// cannot bring back what a newer one replaced.
    #[test]
    fn stored_policies_expire_and_older_commands_are_refused() {
        let dir = tempfile::tempdir().unwrap();
        let state = dir.path().join("health");
        let cmd = check_ref(Purpose::Readiness, "cmd", 7);
        store(&state, &spec(7, commands()), AT).unwrap();
        let max = i64::try_from(MAX_POLICY_AGE.as_millis()).unwrap();
        assert!(lookup(&state, &cmd, AT.now_ms + max).is_ok());
        assert_eq!(lookup(&state, &cmd, AT.now_ms + max + 1), Err(Refusal::PolicyExpired));

        // The helper's clock went back: not expired, not a panic.
        assert!(lookup(&state, &cmd, 0).is_ok());

        // Forge sends the same policy again: fresh from then on.
        let again = Times { issued_at_ms: AT.issued_at_ms + 600_000, now_ms: AT.now_ms + 600_000 };
        store(&state, &spec(7, commands()), again).unwrap();
        assert!(lookup(&state, &cmd, again.now_ms + max).is_ok());

        // A newer policy without command checks replaces it...
        let newer = Times { issued_at_ms: again.issued_at_ms + 1, now_ms: again.now_ms + 1 };
        assert_eq!(store(&state, &spec(9, tcp_only()), newer).unwrap().command_checks, 0);
        assert_eq!(lookup(&state, &check_ref(Purpose::Readiness, "cmd", 9), newer.now_ms), Err(Refusal::NoSuchCheck));

        // ...and the older command, delivered late or replayed, is refused.
        assert_eq!(store(&state, &spec(7, commands()), again), Err(StoreError::Older));
        assert_eq!(lookup(&state, &cmd, newer.now_ms), Err(Refusal::StalePolicy));
    }

    #[test]
    fn a_deploy_stores_the_policy_at_revision_zero_and_a_removal_drops_it() {
        let dir = tempfile::tempdir().unwrap();
        let state = dir.path().join("health");
        let app: AppSpec = serde_json::from_value(serde_json::json!({
            "environment": "uat", "name": "api", "runtime": "container", "image": "nginx:1.27",
            "health": commands()
        }))
        .unwrap();
        on_app_action(&state, "deploy", &app, 9_000);
        let (runtime, _, _) = lookup(&state, &check_ref(Purpose::Readiness, "cmd", 0), 9_000).unwrap();
        assert_eq!(runtime, Runtime::Container);
        on_app_action(&state, "restart", &app, 9_500);
        assert!(
            lookup(&state, &check_ref(Purpose::Readiness, "cmd", 0), 9_500).is_ok(),
            "other actions change nothing"
        );

        // A policy command issued before the deploy does not undo it.
        assert_eq!(store(&state, &spec(7, commands()), AT), Err(StoreError::Older));
        on_app_action(&state, "remove", &app, 9_900);
        assert_eq!(lookup(&state, &check_ref(Purpose::Readiness, "cmd", 0), 9_900), Err(Refusal::NoPolicy));
    }

    /// Exec probes are an argv; a shell probe names its interpreter and the
    /// script is its stdin (native) or one argument (container). Nothing is
    /// ever joined into a command line.
    #[test]
    fn commands_are_built_without_a_shell_of_ours() {
        let p = policy(commands());
        let exec = &p.readiness[0].probe;
        assert_eq!(native_command(exec), Some((vec!["/bin/true".into(), "-x".into()], None)));
        assert_eq!(container_command(exec), Some(vec!["/bin/true".into(), "-x".into()]));
        let tricky: Probe = serde_json::from_value(serde_json::json!({
            "type": "shell", "script": "echo \"$(id)\"; rm -rf / # '\n"
        }))
        .unwrap();
        let script = "echo \"$(id)\"; rm -rf / # '\n".to_owned();
        assert_eq!(native_command(&tricky), Some((vec!["sh".into()], Some(script.clone()))));
        assert_eq!(container_command(&tricky), Some(vec!["sh".into(), "-c".into(), script]));
        assert_eq!(container_command(&p.liveness[0].probe).unwrap()[0], "bash");
        assert_eq!(native_command(&p.readiness[2].probe), None);
        assert_eq!(container_command(&p.readiness[2].probe), None);
    }

    /// Exit codes decide. Output leaves the helper only for a failed run of
    /// a check that asks for it, and then only a short clean tail.
    #[test]
    fn exit_codes_decide_and_output_is_an_opt_in() {
        let p = policy(commands());
        let exec = &p.readiness[0].probe;
        let quiet = |code, passed| ExecResult::Exited { code, passed, output: String::new() };
        assert_eq!(exited(exec, 3, b"secret output"), quiet(3, true));
        assert_eq!(exited(exec, 1, b"password=hunter2"), quiet(1, false), "no output without the opt-in");
        let shown: Probe = serde_json::from_value(serde_json::json!({
            "type": "exec", "argv": ["x"], "show_output": true
        }))
        .unwrap();
        assert_eq!(exited(&shown, 0, b"fine"), quiet(0, true), "a pass reports nothing");
        let failed = exited(&shown, 1, b"line one\r\n\x1b[31mred\x00\xff  tail\n");
        assert_eq!(failed, ExecResult::Exited { code: 1, passed: false, output: "line one [31mred tail".into() });
        assert_eq!(sanitize("x".repeat(1000).as_bytes()).len(), MAX_DETAIL);
        assert_eq!(sanitize(b"\n\n"), "");

        // The wire form the agent parses.
        let v = serde_json::to_value(ExecResult::Refused { why: Refusal::NotConfined }).unwrap();
        assert_eq!(v, serde_json::json!({ "result": "refused", "why": "not_confined" }));
        assert!(Refusal::NotConfined.detail().contains("can't confine"));
    }

    /// Only what the node cannot change by waiting is permanent.
    #[test]
    fn refusals_are_permanent_only_when_waiting_cannot_help() {
        for why in [
            Refusal::NoAccount,
            Refusal::NotConfined,
            Refusal::NotContained,
            Refusal::ForeignContainer,
            Refusal::Unsupported,
        ] {
            assert!(why.permanent(), "{why:?}");
        }

        for why in [
            Refusal::NoPolicy,
            Refusal::StalePolicy,
            Refusal::PolicyExpired,
            Refusal::NoSuchCheck,
            Refusal::WorkerFailed,
            Refusal::NoDocker,
            Refusal::Busy,
            Refusal::Throttled,
        ] {
            assert!(!why.permanent(), "{why:?}");
        }
    }

    #[cfg(not(target_os = "linux"))]
    #[tokio::test]
    async fn other_systems_refuse_to_run_command_checks() {
        let dir = tempfile::tempdir().unwrap();
        let r = Runner::new(dir.path().to_path_buf()).run(&check_ref(Purpose::Readiness, "cmd", 7)).await;
        assert_eq!(r, ExecResult::Refused { why: Refusal::Unsupported });
    }
}
