//! In-memory command delivery between jobs and long-polling agents.
//!
//! Durability comes from the job engine: if the process restarts, the
//! `agent.command` job re-dispatches, so nothing here needs persisting.

use std::collections::{HashMap, VecDeque};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use forge_core::ServerId;
use forge_proto::messages::{CommandResult, SignedCommand};
use serde_json::Value;
use tokio::sync::{Notify, oneshot};

const MAX_PENDING_PER_SERVER: usize = 256;

#[derive(Debug, Default)]
struct Slot {
    /// `(operation id, command)` in order.
    pending: VecDeque<(String, SignedCommand)>,
    notify: Arc<Notify>,
}

#[derive(Debug, Default)]
pub struct CommandQueue {
    slots: Mutex<HashMap<ServerId, Slot>>,
    /// Who may answer each operation: the node it was queued for, and the result channel.
    waiters: Mutex<HashMap<String, (ServerId, oneshot::Sender<CommandResult>)>>,
    receivers: Mutex<HashMap<String, oneshot::Receiver<CommandResult>>>,
    /// Commands a node's agent has taken and not answered yet: the node and
    /// when it took them. An agent that restarts after taking a command
    /// forgets it (its unsent results live in memory), so its waiter is
    /// dropped at once instead of after the full timeout.
    delivered: Mutex<HashMap<String, (ServerId, Instant)>>,
    /// When each node's agent last asked this process for commands.
    polled: Mutex<HashMap<ServerId, Instant>>,
    /// Live output of running operations (bounded tail), for the console.
    live: Mutex<HashMap<String, String>>,
    /// Parts of a job result that must never reach the job table (revealed
    /// container environment values), keyed by job id. The API call waiting
    /// on the job takes them once; leftovers expire after [`TRANSIENT_TTL`].
    transient: Mutex<HashMap<String, (Instant, Value)>>,
}

/// How long an untaken transient result is kept.
const TRANSIENT_TTL: Duration = Duration::from_secs(60);

/// Most command bytes handed to an agent in one heartbeat answer.
const MAX_TAKE_BYTES: usize = 3 * 1024 * 1024;

/// Slack when comparing an agent's uptime with the time since it took a
/// command: uptime is whole seconds and measured on the node.
const RESTART_SLACK: Duration = Duration::from_secs(3);

/// Live output kept per operation (the tail beyond this is dropped).
const MAX_LIVE: usize = 256 * 1024;

impl CommandQueue {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Queues a signed command and registers interest in its result.
    pub fn enqueue(&self, server: ServerId, operation_id: &str, cmd: SignedCommand) -> Result<(), String> {
        let (tx, rx) = oneshot::channel();

        {
            let mut slots = self.slots.lock().map_err(|_| "lock poisoned")?;
            let slot = slots.entry(server).or_default();
            if slot.pending.len() >= MAX_PENDING_PER_SERVER {
                return Err("too many pending commands for this server".into());
            }

            slot.pending.push_back((operation_id.to_owned(), cmd));
            slot.notify.notify_one();
        }

        self.waiters.lock().map_err(|_| "lock poisoned")?.insert(operation_id.to_owned(), (server, tx));
        self.receivers.lock().map_err(|_| "lock poisoned")?.insert(operation_id.to_owned(), rx);

        Ok(())
    }

    /// Whether this process is still waiting for `operation_id` (false after a restart).
    #[must_use]
    pub fn is_tracked(&self, operation_id: &str) -> bool {
        self.receivers.lock().is_ok_and(|r| r.contains_key(operation_id))
    }

    /// Waits for the result of `operation_id`.
    pub async fn wait(&self, operation_id: &str, timeout: Duration) -> Option<CommandResult> {
        let rx = self.receivers.lock().ok()?.remove(operation_id)?;
        let out = tokio::time::timeout(timeout, rx).await.ok().and_then(Result::ok);

        if let Ok(mut w) = self.waiters.lock() {
            w.remove(operation_id);
        }

        if let Ok(mut d) = self.delivered.lock() {
            d.remove(operation_id);
        }

        out
    }

    /// Whether `from` is the node `operation_id` was queued for.
    fn sent_to(&self, from: ServerId, operation_id: &str) -> bool {
        self.waiters.lock().is_ok_and(|w| w.get(operation_id).is_some_and(|(target, _)| *target == from))
    }

    /// Appends live output, from node `from`, for an operation this process is
    /// waiting for. Output for another node's operation is dropped.
    pub fn append_progress(&self, from: ServerId, operation_id: &str, text: &str) {
        let tracked = self.sent_to(from, operation_id);
        if !tracked {
            return;
        }

        if let Ok(mut live) = self.live.lock() {
            let buf = live.entry(operation_id.to_owned()).or_default();
            buf.push_str(text);

            if buf.len() > MAX_LIVE {
                let mut cut = buf.len() - MAX_LIVE;

                while !buf.is_char_boundary(cut) {
                    cut += 1;
                }

                buf.drain(..cut);
            }
        }
    }

    /// Live output so far (`None` once the operation finished or if unknown).
    pub fn live_output(&self, operation_id: &str) -> Option<String> {
        self.live.lock().ok()?.get(operation_id).cloned()
    }

    /// Delivers a result reported by node `from`. Unknown ids, and results
    /// for an operation queued for another node, are ignored: operation ids
    /// are not secret (they appear in job details), so the id alone proves nothing.
    pub fn resolve(&self, from: ServerId, result: CommandResult) -> bool {
        let tx = self.waiters.lock().ok().and_then(|mut w| {
            if w.get(&result.operation_id).is_some_and(|(target, _)| *target == from) {
                w.remove(&result.operation_id).map(|(_, tx)| tx)
            } else {
                None
            }
        });
        let Some(tx) = tx else { return false };

        if let Ok(mut d) = self.delivered.lock() {
            d.remove(&result.operation_id);
        }

        if let Ok(mut live) = self.live.lock() {
            live.remove(&result.operation_id);
        }

        let _ = tx.send(result);

        true
    }

    /// Whether `server`'s agent asked this process for commands within
    /// `window`: it holds a live link here and will pick up a new command.
    /// The stored node status cannot say that (it outlives a restart).
    #[must_use]
    pub fn polled_within(&self, server: ServerId, window: Duration) -> bool {
        self.polled.lock().is_ok_and(|p| p.get(&server).is_some_and(|at| at.elapsed() <= window))
    }

    /// Removes up to `max` pending commands for `server`.
    pub fn take(&self, server: ServerId, max: usize) -> Vec<SignedCommand> {
        if let Ok(mut polled) = self.polled.lock() {
            polled.insert(server, Instant::now());
        }

        let Ok(mut slots) = self.slots.lock() else { return vec![] };
        let Some(slot) = slots.get_mut(&server) else { return vec![] };

        // At most `max` commands and `MAX_TAKE_BYTES` (agents read answers of
        // up to 4 MiB; file chunks are about 350 KB each), always at least one.
        let (mut n, mut bytes) = (0, 0);

        for (_, c) in slot.pending.iter().take(max) {
            let size = c.body.len() + c.signature.len() + 64;
            if n > 0 && bytes + size > MAX_TAKE_BYTES {
                break;
            }

            n += 1;
            bytes += size;
        }

        let taken: Vec<(String, SignedCommand)> = slot.pending.drain(..n).collect();
        drop(slots);

        if let Ok(mut d) = self.delivered.lock() {
            let now = Instant::now();

            for (op, _) in &taken {
                d.insert(op.clone(), (server, now));
            }
        }

        taken.into_iter().map(|(_, c)| c).collect()
    }

    /// Notes `server`'s agent uptime from a heartbeat. Commands it took
    /// before it (re)started will never be answered: their waiters are
    /// dropped now, so the job sees no answer and retries instead of
    /// waiting out its timeout. Returns the operation ids given up.
    pub fn agent_uptime(&self, server: ServerId, uptime: Duration) -> Vec<String> {
        self.forget_before_restart(server, uptime, Instant::now())
    }

    fn forget_before_restart(&self, server: ServerId, uptime: Duration, now: Instant) -> Vec<String> {
        let lost: Vec<String> = match self.delivered.lock() {
            Ok(mut d) => {
                let lost: Vec<String> = d
                    .iter()
                    .filter(|(_, (s, at))| *s == server && now.saturating_duration_since(*at) > uptime + RESTART_SLACK)
                    .map(|(op, _)| op.clone())
                    .collect();

                for op in &lost {
                    d.remove(op);
                }

                lost
            }
            Err(_) => return Vec::new(),
        };

        if let Ok(mut w) = self.waiters.lock() {
            for op in &lost {
                // Dropping the sender ends `wait` with no result.
                w.remove(op);
            }
        }

        lost
    }

    /// Notifier fired when a command is queued for `server`.
    pub fn notifier(&self, server: ServerId) -> Arc<Notify> {
        self.slots.lock().map(|mut s| s.entry(server).or_default().notify.clone()).unwrap_or_default()
    }

    /// Keeps `value` in memory for the caller waiting on job `job_id`
    /// (see [`Self::take_transient`]). Expired entries are swept here.
    pub fn stash_transient(&self, job_id: &str, value: Value) {
        if let Ok(mut t) = self.transient.lock() {
            let now = Instant::now();
            t.retain(|_, (at, _)| now.duration_since(*at) < TRANSIENT_TTL);
            t.insert(job_id.to_owned(), (now, value));
        }
    }

    /// Removes and returns the transient part of job `job_id`'s result, if
    /// this process still holds it.
    pub fn take_transient(&self, job_id: &str) -> Option<Value> {
        let (at, v) = self.transient.lock().ok()?.remove(job_id)?;
        (at.elapsed() < TRANSIENT_TTL).then_some(v)
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    #[test]
    fn transient_results_are_taken_once() {
        let q = CommandQueue::new();
        q.stash_transient("job-1", serde_json::json!({ "env": [{ "key": "A", "value": "1" }] }));
        assert!(q.take_transient("job-2").is_none());
        assert_eq!(q.take_transient("job-1"), Some(serde_json::json!({ "env": [{ "key": "A", "value": "1" }] })));
        assert!(q.take_transient("job-1").is_none(), "taken once");
    }

    #[test]
    fn a_heartbeat_answer_stays_within_the_agents_read_limit() {
        let q = CommandQueue::new();
        let s = ServerId::new();

        // Ten upload chunks of about 350 KB each (base64 of 256 KiB).
        for i in 0..10 {
            let cmd = SignedCommand { body: vec![0; 350 * 1024], ..SignedCommand::default() };
            q.enqueue(s, &format!("op{i}"), cmd).unwrap();
        }

        let first = q.take(s, 16);
        let bytes: usize = first.iter().map(|c| c.body.len()).sum();
        assert!(!first.is_empty() && bytes <= MAX_TAKE_BYTES, "{} commands, {bytes} bytes", first.len());
        assert_eq!(first.len() + q.take(s, 16).len() + q.take(s, 16).len(), 10, "nothing lost, in order");

        // One command bigger than the budget still goes out on its own.
        q.enqueue(s, "big", SignedCommand { body: vec![0; MAX_TAKE_BYTES + 1], ..SignedCommand::default() }).unwrap();
        assert_eq!(q.take(s, 16).len(), 1);
    }

    #[tokio::test]
    async fn enqueue_take_resolve() {
        let q = CommandQueue::new();
        let s = ServerId::new();
        q.enqueue(s, "op1", SignedCommand::default()).unwrap();
        assert!(q.is_tracked("op1"));
        assert_eq!(q.take(s, 10).len(), 1);
        assert!(q.take(s, 10).is_empty());
        assert!(q.resolve(s, CommandResult { operation_id: "op1".into(), ok: true, ..Default::default() }));
        let r = q.wait("op1", Duration::from_secs(1)).await.unwrap();
        assert!(r.ok);
        assert!(!q.is_tracked("op1"));
    }

    #[test]
    fn a_node_is_live_once_its_agent_polled() {
        let q = CommandQueue::new();
        let s = ServerId::new();
        assert!(!q.polled_within(s, Duration::from_secs(60)), "never polled");
        assert!(q.take(s, 1).is_empty());
        assert!(q.polled_within(s, Duration::from_secs(60)));
        assert!(!q.polled_within(ServerId::new(), Duration::from_secs(60)), "another node");
    }

    /// A rollout waited out the whole deploy timeout (35 min) when the
    /// node's agent restarted after taking the command: the agent keeps
    /// unsent results in memory only. The heartbeat's uptime now gives the
    /// command up at once.
    #[tokio::test]
    async fn a_command_taken_before_the_agent_restarted_ends_at_once() {
        let q = CommandQueue::new();
        let s = ServerId::new();
        q.enqueue(s, "deploy", SignedCommand::default()).unwrap();
        q.enqueue(s, "later", SignedCommand::default()).unwrap();
        assert_eq!(q.take(s, 1).len(), 1, "the agent took `deploy`");

        // Seconds later the agent reports 1 s of uptime: it restarted.
        let lost = q.forget_before_restart(s, Duration::from_secs(1), Instant::now() + Duration::from_secs(10));
        assert_eq!(lost, ["deploy"]);
        let t = Instant::now();
        assert!(q.wait("deploy", Duration::from_secs(35 * 60)).await.is_none());
        assert!(t.elapsed() < Duration::from_secs(1), "no 35-minute wait");

        // A command still queued is delivered after the restart as usual.
        assert_eq!(q.take(s, 10).len(), 1);

        // An agent that kept running is left alone; another node is not affected.
        q.enqueue(s, "ok", SignedCommand::default()).unwrap();
        q.take(s, 1);
        assert!(q.agent_uptime(s, Duration::from_secs(3_600)).is_empty());
        assert!(
            q.forget_before_restart(ServerId::new(), Duration::ZERO, Instant::now() + Duration::from_secs(60))
                .is_empty()
        );
        assert!(q.is_tracked("ok"));
    }

    #[tokio::test]
    async fn wait_times_out() {
        let q = CommandQueue::new();
        q.enqueue(ServerId::new(), "op", SignedCommand::default()).unwrap();
        assert!(q.wait("op", Duration::from_millis(20)).await.is_none());
    }

    #[test]
    fn live_output_is_bounded_ignored_when_untracked_and_cleared_by_the_result() {
        let q = CommandQueue::new();

        // An operation nobody waits for (stale, or forged by an agent) is dropped.
        q.append_progress(ServerId::new(), "unknown", "x");
        assert!(q.live_output("unknown").is_none());

        let s = ServerId::new();
        q.enqueue(s, "op1", SignedCommand::default()).unwrap();
        q.append_progress(
            s, "op1", "line 1
",
        );
        q.append_progress(
            s, "op1", "line 2
",
        );
        assert_eq!(
            q.live_output("op1").unwrap(),
            "line 1
line 2
"
        );

        // A chatty recipe cannot grow memory without bound: the tail is kept.
        q.append_progress(s, "op1", &"y".repeat(MAX_LIVE + 1000));
        let out = q.live_output("op1").unwrap();
        assert!(out.len() <= MAX_LIVE && out.ends_with('y'));

        // The final result replaces the live view.
        q.resolve(s, CommandResult { operation_id: "op1".into(), ok: true, ..Default::default() });
        assert!(q.live_output("op1").is_none());
    }

    #[tokio::test]
    async fn results_and_output_from_another_node_are_dropped() {
        let q = CommandQueue::new();
        let (target, other) = (ServerId::new(), ServerId::new());
        q.enqueue(target, "op1", SignedCommand::default()).unwrap();

        // Another enrolled node that learned the operation id.
        q.append_progress(other, "op1", "forged output");
        assert!(q.live_output("op1").is_none());
        let forged =
            CommandResult { operation_id: "op1".into(), ok: true, output: "forged".into(), ..Default::default() };
        assert!(!q.resolve(other, forged));

        // The real node still delivers.
        q.append_progress(target, "op1", "real output");
        assert_eq!(q.live_output("op1").as_deref(), Some("real output"));
        let real = CommandResult { operation_id: "op1".into(), ok: false, output: "real".into(), ..Default::default() };
        assert!(q.resolve(target, real));
        let r = q.wait("op1", Duration::from_secs(1)).await.unwrap();
        assert_eq!(r.output, "real");
        assert!(!r.ok);
    }
}
