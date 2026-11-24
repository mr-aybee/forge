//! `agent.update` (GitHub issue #4): updates one node's agent and helper to
//! the version of the agent binaries this control plane hands out
//! ([`AgentDist::version`], from `MANIFEST.json`).
//!
//! Payload: `{"server_id": "…", "target"?: "<version>", "rollout"?: "<job>",
//! "deadline_ms"?: <epoch ms>}`. A `target` set at submit time must still be
//! the version on offer; otherwise the job fails instead of sending other
//! binaries. A job of a rollout (`rollout` set) **skips** a node that cannot
//! take the update: offline (the rollout already waited for it), not Linux,
//! no answer by `deadline_ms` (the end of its batch), or refused by the node
//! as `unsupported` or `downgrade`. It succeeds with `state.skipped` and
//! `state.skip_reason`, so the rollout carries on. A single update fails
//! instead. A `busy` node is retried with backoff. It stops when its rollout
//! was cancelled. Steps (idempotent):
//! 1. `send`: unless the node already runs the target version or a newer
//!    one ([`crate::agent_update_policy`]), send a signed
//!    `AgentUpdate` with the SHA-256 of this release's binaries for the
//!    node's architecture. The node downloads, verifies, swaps and restarts.
//! 2. `await`: wait until the node reports the target version (or a newer
//!    one) again, then for facts that show the helper on it too. If it
//!    never does, the node's helper rolls back on its own within minutes;
//!    the step fails as soon as the node is back on another version.
//!
//! Authorized as `server.command` (MFA-guarded), re-checked per step.

use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use forge_core::model::{AgentStatus, Job, JobFilter, JobStatus, Server};
use forge_core::ports::{JobRepository, ServerRepository};
use forge_core::{Action, Clock, DomainResult, ServerId};
use forge_jobs::{JobHandler, StepContext, StepError};
use forge_proto::messages::{AgentUpdate, RefusalCode};
use forge_proto::{Command, CommandSigner};
use serde_json::{Value, json};

use crate::agent_hub::CommandQueue;
use crate::agent_update_policy::has_reached;
use crate::install::AgentDist;

/// Where nodes download agent binaries (D-167): `node_public_url`, else
/// the console address (pinned, else the one an administrator last signed
/// in on). Never a localhost default: with no address known the update
/// fails with [`NO_DOWNLOAD_ADDRESS`].
#[derive(Debug, Clone)]
pub struct DownloadBase {
    /// `node_public_url` from forge.toml.
    pub node_public_url: Option<String>,
    pub address: forge_core::public_address::PublicAddress,
}

impl DownloadBase {
    #[must_use]
    pub fn resolve(&self) -> Option<String> {
        self.node_public_url
            .clone()
            .or_else(|| Some(self.address.get()))
            .map(|u| u.trim_end_matches('/').to_owned())
            .filter(|u| !u.is_empty())
    }
}

/// Why an update cannot start before Forge knows an address of its own.
pub const NO_DOWNLOAD_ADDRESS: &str = "Forge does not know an address nodes can download from yet: sign in to the console on the address nodes use, or set public_url or node_public_url in forge.toml";

#[derive(Debug)]
pub struct AgentUpdateJob {
    pub hub: Arc<CommandQueue>,
    pub signer: Arc<CommandSigner>,
    pub servers: Arc<dyn ServerRepository>,
    pub dist: Arc<AgentDist>,
    pub clock: Arc<dyn Clock>,
    /// Where nodes download the new binaries from.
    pub download: DownloadBase,
    /// How long to wait for the node to come back on the new version.
    pub come_back: Duration,
    /// Parent rollouts (cancelled → stop).
    pub jobs: Arc<dyn JobRepository>,
    /// Node facts: the helper's version after the update.
    pub facts: Arc<dyn forge_core::tags::FactsRepository>,
}

/// How long `await` waits, after the agent is back on the target, for facts
/// that name the helper's version.
const HELPER_WAIT: Duration = Duration::from_secs(90);

/// What the node's facts say about its helper after the update.
#[derive(Debug, Clone, PartialEq, Eq)]
enum HelperCheck {
    /// Facts collected after the update name the target version or newer.
    Reached(String),
    /// They name an older version: the helper was not replaced.
    Behind(String),
    /// Collected after the update, and the agent says its helper did not
    /// answer: the node has no working helper (and so no watchdog).
    Silent,
    /// Collected after the update, without a helper version from an agent
    /// that does not report one (older agents).
    Unreported,
    /// No facts collected since the update yet.
    NotYet,
}

fn helper_check(facts: Option<&(Value, forge_core::Timestamp)>, target: &str, sent_ms: i64) -> HelperCheck {
    let Some((f, _)) = facts.filter(|(_, at)| at.millis() > sent_ms) else { return HelperCheck::NotYet };
    match f.get("self_update").and_then(|c| c.get("helper_version")).and_then(Value::as_str) {
        Some(v) if !v.is_empty() && has_reached(v, target) => HelperCheck::Reached(v.to_owned()),
        Some(v) if !v.is_empty() => HelperCheck::Behind(v.to_owned()),
        _ if f.get("helper_ok") == Some(&Value::Bool(false)) => HelperCheck::Silent,
        _ => HelperCheck::Unreported,
    }
}

/// Why a rollout's update left a node alone (`state.skip_reason`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SkipReason {
    /// Already on the target or a newer version.
    Current,
    /// Offline until the end of the batch.
    Offline,
    /// Cannot self-update (not Linux, no systemd or curl, `update` not allowed).
    Unsupported,
    /// The node runs a newer version and refused the downgrade.
    Downgrade,
}

/// Longest single wait for the node's answer to the update command.
const ANSWER_WAIT: Duration = Duration::from_secs(15 * 60);

/// Held while checking for a queued or running update of a node and then
/// submitting one, by the single-node API and by rollouts alike, so one node
/// never gets two concurrent `AgentUpdate` commands. Also serializes rollout
/// submissions. One control plane serves a database, so a process-wide lock
/// makes the check and the submit atomic.
pub static SUBMIT_LOCK: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

/// Queued (or waiting to retry) and running jobs of `kind`.
pub async fn active_jobs(jobs: &dyn JobRepository, kind: &str) -> DomainResult<Vec<Job>> {
    let mut out = Vec::new();
    for status in [JobStatus::Running, JobStatus::Queued] {
        let filter = JobFilter { workspace_id: None, status: Some(status), limit: 500 };
        out.extend(jobs.list_jobs(&filter).await?.into_iter().filter(|j| j.kind == kind));
    }
    Ok(out)
}

/// The queued or running `agent.update` job of node `id`, if any.
pub async fn active_update(jobs: &dyn JobRepository, id: ServerId) -> DomainResult<Option<Job>> {
    let id = id.to_string();
    Ok(active_jobs(jobs, "agent.update")
        .await?
        .into_iter()
        .find(|j| j.payload.get("server_id").and_then(Value::as_str) == Some(id.as_str())))
}

/// How long after the update was sent the old agent may still be running.
/// The node restarts through a systemd timer. Agents up to D-104 set it
/// without an accuracy, so it fired 3 to 63 seconds after the reply (seen on
/// the lab: 52 and 62 seconds); newer ones restart within about 4 seconds.
/// A heartbeat later than this comes from an agent started since.
const OLD_AGENT_GONE_AFTER: Duration = Duration::from_secs(120);

/// True when the node's helper has restored another version: the node is
/// online with a heartbeat from after the old agent stopped (`sent_ms` is
/// when the update was sent), and it does not report `target`.
fn rolled_back(node: &Server, target: &str, sent_ms: i64) -> bool {
    let gone_ms = sent_ms.saturating_add(i64::try_from(OLD_AGENT_GONE_AFTER.as_millis()).unwrap_or(i64::MAX));
    node.status == AgentStatus::Online
        && !has_reached(&node.agent_version, target)
        && node.last_seen_at.is_some_and(|seen| seen.millis() > gone_ms)
}

/// The version this job updates to: the agents on offer, which must match
/// the payload's `target` when the submitter recorded one.
fn job_target<'a>(payload: &Value, dist: &'a AgentDist) -> Result<&'a str, StepError> {
    let offered = dist.version();
    match payload.get("target").and_then(Value::as_str) {
        Some(t) if t != offered => Err(StepError::Fatal(format!(
            "this update was for {t}, but this control plane now hands out agents {offered}; start a new update"
        ))),
        _ => Ok(offered),
    }
}

/// Records a skip in the job state.
fn skip(ctx: &mut StepContext, reason: SkipReason, why: &str) {
    ctx.state["skipped"] = json!(why);
    ctx.state["skip_reason"] = json!(reason);
}

fn server_id(payload: &Value) -> Result<ServerId, StepError> {
    payload
        .get("server_id")
        .and_then(Value::as_str)
        .and_then(|s| s.parse().ok())
        .ok_or_else(|| StepError::Fatal("payload needs server_id".into()))
}

impl AgentUpdateJob {
    /// How long to wait for the node's answer: at most until the end of the
    /// rollout's batch (`deadline_ms`), never more than [`ANSWER_WAIT`].
    fn answer_wait(&self, payload: &Value) -> Duration {
        match payload.get("deadline_ms").and_then(Value::as_i64) {
            Some(ms) => {
                ANSWER_WAIT.min(Duration::from_millis(u64::try_from(ms - self.clock.now().millis()).unwrap_or(0)))
            }
            None => ANSWER_WAIT,
        }
    }

    /// The agent is back on the target: the helper must be too. Facts are
    /// collected when the agent reports a new version; this waits briefly
    /// for them. A helper on an older version, or one the agent says does
    /// not answer, fails the update; an agent that does not report it, or
    /// no facts in time, leaves it marked `helper: "unverified"`.
    async fn await_helper(
        &self,
        ctx: &mut StepContext,
        id: ServerId,
        target: &str,
        sent_ms: i64,
    ) -> Result<(), StepError> {
        let deadline = tokio::time::Instant::now() + HELPER_WAIT;
        loop {
            let facts = self.facts.facts_of(id).await.map_err(|e| StepError::Retryable(e.to_string()))?;
            match helper_check(facts.as_ref(), target, sent_ms) {
                HelperCheck::Reached(v) => {
                    ctx.state["helper"] = json!(v);
                    return Ok(());
                }
                HelperCheck::Behind(v) => {
                    return Err(StepError::Fatal(format!(
                        "the agent runs {target}, but the node's helper still runs {v}: the helper was not replaced"
                    )));
                }
                HelperCheck::Silent => {
                    return Err(StepError::Fatal(format!(
                        "the agent runs {target}, but the node's helper does not answer: check forge-agent-helper on the node"
                    )));
                }
                HelperCheck::Unreported => {
                    ctx.state["helper"] = json!("unverified");
                    return Ok(());
                }
                HelperCheck::NotYet if tokio::time::Instant::now() >= deadline => {
                    ctx.state["helper"] = json!("unverified");
                    return Ok(());
                }
                HelperCheck::NotYet => tokio::time::sleep(Duration::from_secs(5)).await,
            }
        }
    }

    /// Fails with `msg`, or for a rollout's job skips the node.
    fn skip_or_fail(ctx: &mut StepContext, reason: SkipReason, msg: String) -> Result<(), StepError> {
        if ctx.job.payload.get("rollout").is_some() {
            skip(ctx, reason, &msg);
            Ok(())
        } else {
            Err(StepError::Fatal(msg))
        }
    }

    /// Stops when the rollout this job belongs to was cancelled.
    async fn check_rollout(&self, payload: &Value) -> Result<(), StepError> {
        let Some(parent) = payload.get("rollout").and_then(Value::as_str).and_then(|s| s.parse().ok()) else {
            return Ok(());
        };
        let j = self.jobs.get_job(parent).await.map_err(|e| StepError::Retryable(e.to_string()))?;
        if j.status == JobStatus::Cancelled {
            return Err(StepError::Fatal("the rollout was cancelled; this node was not updated".into()));
        }
        Ok(())
    }

    /// Step `send`: skip, send the signed update, read the node's answer.
    /// Nothing waits inside the step for the node: a rollout waits for
    /// offline nodes itself (one worker slot), and a busy node is retried
    /// with the engine's backoff, so each attempt is authorized again.
    async fn send(&self, ctx: &mut StepContext, id: ServerId, target: &str, node: Server) -> Result<(), StepError> {
        if has_reached(&node.agent_version, target) {
            skip(ctx, SkipReason::Current, "already on or past the target version");
            return Ok(());
        }
        self.check_rollout(&ctx.job.payload).await?;
        if !node.os.eq_ignore_ascii_case("linux") {
            return Self::skip_or_fail(
                ctx,
                SkipReason::Unsupported,
                "self-update is available for Linux nodes; reinstall Windows agents".into(),
            );
        }
        if node.status != AgentStatus::Online {
            // A rollout waited for the node already; a single update retries
            // with the engine's backoff (no worker slot held meanwhile).
            if ctx.job.payload.get("rollout").is_some() {
                skip(ctx, SkipReason::Offline, "the node went offline; it was not updated");
                return Ok(());
            }
            return Err(StepError::Retryable("the node is offline".into()));
        }
        let arch = node.arch.clone();
        let sum = |bin: &str| self.dist.binary("linux", &arch, bin).map(|(_, s)| s.to_owned());
        let (Some(agent_sha256), Some(helper_sha256)) = (sum("forge-agent"), sum("forge-agent-helper")) else {
            return Err(StepError::Fatal(format!("this control plane has no agent binaries for linux/{arch}")));
        };
        let (body, signed) = self.signer.issue(
            &id.to_string(),
            &ctx.job.principal.to_string(),
            Command::AgentUpdate(AgentUpdate {
                version: target.to_owned(),
                download_base: self.download.resolve().ok_or_else(|| StepError::Fatal(NO_DOWNLOAD_ADDRESS.into()))?,
                arch,
                agent_sha256,
                helper_sha256,
                // Rollouts never move a node backwards (`ahead` nodes are
                // skipped); the node refuses an older version anyway.
                allow_downgrade: false,
            }),
            self.clock.now(),
            Duration::from_secs(240),
        );
        self.hub.enqueue(id, &body.operation_id, signed).map_err(|e| StepError::Retryable(e.to_string()))?;
        let Some(r) = self.hub.wait(&body.operation_id, self.answer_wait(&ctx.job.payload)).await else {
            if ctx.job.payload.get("deadline_ms").is_some() && self.answer_wait(&ctx.job.payload).is_zero() {
                return Self::skip_or_fail(
                    ctx,
                    SkipReason::Offline,
                    "the node did not answer before the end of its batch".into(),
                );
            }
            return Err(StepError::Retryable("the node did not answer".into()));
        };
        if r.ok {
            ctx.state["from"] = json!(node.agent_version);
            ctx.state["to"] = json!(target);
            ctx.state["sent_ms"] = json!(self.clock.now().millis());
            ctx.state["node"] = serde_json::from_str(&r.output).unwrap_or(Value::Null);
            return Ok(());
        }
        let msg = format!("the node refused the update: {}", r.error);
        match RefusalCode::parse(&r.error_code) {
            Some(RefusalCode::Unsupported) => Self::skip_or_fail(ctx, SkipReason::Unsupported, msg),
            Some(RefusalCode::Downgrade) => Self::skip_or_fail(ctx, SkipReason::Downgrade, msg),
            // Another update runs on the node: the engine asks again later.
            Some(RefusalCode::Busy) => Err(StepError::Retryable(msg)),
            None => Err(StepError::Fatal(msg)),
        }
    }
}

#[async_trait]
impl JobHandler for AgentUpdateJob {
    fn kind(&self) -> &'static str {
        "agent.update"
    }

    fn steps(&self) -> &'static [&'static str] {
        &["send", "await"]
    }

    fn required_action(&self, _payload: &Value) -> Action {
        Action::ServerCommand
    }

    async fn run_step(&self, index: usize, ctx: &mut StepContext) -> Result<(), StepError> {
        let id = server_id(&ctx.job.payload)?;
        let dist = self.dist.clone();
        let target = job_target(&ctx.job.payload, &dist)?;
        let node = self.servers.get_server(id).await.map_err(|e| StepError::Fatal(format!("node: {e}")))?;
        match index {
            0 => self.send(ctx, id, target, node).await,
            _ => {
                if ctx.state.get("skipped").is_some() {
                    return Ok(());
                }
                // A step run again after a crash has no better time than now.
                let sent_ms =
                    ctx.state.get("sent_ms").and_then(Value::as_i64).unwrap_or_else(|| self.clock.now().millis());
                let deadline = tokio::time::Instant::now() + self.come_back;
                loop {
                    let n = self.servers.get_server(id).await.map_err(|e| StepError::Retryable(e.to_string()))?;
                    if has_reached(&n.agent_version, target) && n.status == AgentStatus::Online {
                        ctx.state["reported"] = json!(n.agent_version);
                        return self.await_helper(ctx, id, target, sent_ms).await;
                    }
                    if rolled_back(&n, target, sent_ms) {
                        return Err(StepError::Fatal(format!(
                            "the node is back on {}: the new agent did not stay up and the node's helper restored the previous one",
                            n.agent_version
                        )));
                    }
                    if tokio::time::Instant::now() >= deadline {
                        return Err(StepError::Fatal(format!(
                            "the node did not come back on {target} (it reports {}); its helper restores the previous agent automatically",
                            n.agent_version
                        )));
                    }
                    tokio::time::sleep(Duration::from_secs(3)).await;
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use forge_core::model::NodeRole;
    use forge_core::{Timestamp, WorkspaceId};

    use super::*;

    #[test]
    fn the_helper_must_reach_the_target_after_the_update() {
        let sent = 1_000;
        let at = |ms| forge_core::Timestamp(ms);
        let f = |v: &str| json!({ "self_update": { "helper_version": v } });
        assert_eq!(helper_check(None, "2.0.0", sent), HelperCheck::NotYet);
        // Facts from before the update say nothing about it.
        assert_eq!(helper_check(Some(&(f("1.0.0"), at(sent))), "2.0.0", sent), HelperCheck::NotYet);
        assert_eq!(
            helper_check(Some(&(f("2.0.0"), at(sent + 1))), "2.0.0", sent),
            HelperCheck::Reached("2.0.0".into())
        );
        assert_eq!(
            helper_check(Some(&(f("2.1.0"), at(sent + 1))), "2.0.0", sent),
            HelperCheck::Reached("2.1.0".into())
        );
        assert_eq!(helper_check(Some(&(f("1.0.0"), at(sent + 1))), "2.0.0", sent), HelperCheck::Behind("1.0.0".into()));
        assert_eq!(helper_check(Some(&(json!({}), at(sent + 1))), "2.0.0", sent), HelperCheck::Unreported);
        let silent = json!({ "helper_ok": false });
        assert_eq!(helper_check(Some(&(silent, at(sent + 1))), "2.0.0", sent), HelperCheck::Silent);
    }

    #[test]
    fn the_target_is_the_offered_version_and_must_match_the_payload() {
        let dir = tempfile::tempdir().unwrap_or_else(|e| panic!("{e}"));
        crate::install::DistManifest { version: "2.0.0".into(), files: std::collections::BTreeMap::new() }
            .write(dir.path())
            .unwrap_or_else(|e| panic!("{e}"));
        let dist = AgentDist::scan(dir.path());
        assert_eq!(job_target(&json!({}), &dist).ok(), Some("2.0.0"));
        assert_eq!(job_target(&json!({ "target": "2.0.0" }), &dist).ok(), Some("2.0.0"));
        assert!(matches!(job_target(&json!({ "target": "1.9.0" }), &dist), Err(StepError::Fatal(_))));
    }

    fn node(version: &str, status: AgentStatus, seen_ms: i64) -> Server {
        Server {
            id: ServerId::new(),
            workspace_id: WorkspaceId::new(),
            environment_id: None,
            cluster_id: None,
            role: NodeRole::default(),
            display_name: None,
            hostname: "n".into(),
            os: "linux".into(),
            arch: "x86_64".into(),
            agent_version: version.into(),
            status,
            last_seen_at: Some(Timestamp(seen_ms)),
            cert_serial: None,
            cert_not_after: None,
            created_at: Timestamp(0),
        }
    }

    #[test]
    fn a_rollback_is_a_fresh_heartbeat_on_another_version() {
        let sent = 1_000_000;
        let gone = sent + 120_000;
        // The old agent's last heartbeats, before it stops: keep waiting. An
        // older agent's restart timer can take just over a minute.
        assert!(!rolled_back(&node("0.1.0", AgentStatus::Online, sent + 2_000), "0.2.0", sent));
        assert!(!rolled_back(&node("0.1.0", AgentStatus::Online, sent + 63_000), "0.2.0", sent));
        assert!(!rolled_back(&node("0.1.0", AgentStatus::Online, gone), "0.2.0", sent));
        // Offline, or on the target: not a rollback.
        assert!(!rolled_back(&node("0.1.0", AgentStatus::Offline, gone + 1), "0.2.0", sent));
        assert!(!rolled_back(&node("0.2.0", AgentStatus::Online, gone + 1), "0.2.0", sent));
        assert!(!rolled_back(&node("0.2.1", AgentStatus::Online, gone + 1), "0.2.0", sent), "past the target");
        // A heartbeat on the old version after the old agent is gone.
        assert!(rolled_back(&node("0.1.0", AgentStatus::Online, gone + 1), "0.2.0", sent));
    }
}
