//! The helper's own trust in Forge (DECISIONS.md D-068).
//!
//! The helper runs as root; the agent runs as the unprivileged `forge-agent`
//! user. Before D-068 the helper ran whatever the agent asked for, so code
//! running as the agent user was as good as root. Now the agent forwards each
//! command exactly as Forge signed it, and the helper:
//!
//! 1. checks the signature with Forge's command key, pinned in its own
//!    root-only folder (`pin.json`), together with this node's server id;
//! 2. checks target, lifetime, clock skew and replays (recent operation ids
//!    survive helper restarts: each admitted id is appended to `seen.log`,
//!    compacted into `seen.json` every [`COMPACT_EVERY`] ids);
//! 3. derives the request from the command itself
//!    ([`HelperRequest::from_command`]) — the agent cannot choose it.
//!
//! Once a pin exists, unsigned requests are refused, except the agent's own
//! confirmation after a self-update. Without a pin (a node set up before
//! D-068 whose helper has not started since) requests run as before.
//!
//! **Pinning.** The installer pins as root right after a fresh enrollment
//! (`forge-agent-helper pin`). A node enrolled before D-068 is pinned on
//! first use: the first time the new helper starts, it copies the key and
//! server id from the agent's `identity.json`. That file belongs to the agent
//! user, so a node whose agent user was already compromised could pin an
//! attacker's key; re-enrolling the node removes that doubt.

use std::path::{Path, PathBuf};

use forge_core::Timestamp;
use forge_proto::CommandVerifier;
use forge_proto::messages::{CommandBody, SignedCommand};
use prost::Message;
use serde::{Deserialize, Serialize};

use crate::helper::HelperRequest;
use crate::self_update::{HELPER_STATE, ensure_private_dir, read_nofollow, write_nofollow};

const PIN: &str = "pin.json";
const SEEN: &str = "seen.json";
/// Ids admitted since the last compaction, one `[expires_ms,"id"]` line each.
const SEEN_LOG: &str = "seen.log";
const MAX_FILE: u64 = 1024 * 1024;
/// Appended ids after which `seen.json` is rewritten (with fsync) and the
/// log starts over. Rewriting the whole list for every command cost ~10 ms
/// each and capped file transfers (issue #15); an append costs microseconds.
/// An append is not fsynced: it survives a helper restart (page cache), and
/// only a power loss could drop the last ids, of commands that expire within
/// minutes anyway.
const COMPACT_EVERY: usize = 1024;
/// Ids restored after a restart: the verifier's replay capacity. The newest
/// are kept, since only they can still be replayed.
const RESTORE_MAX: usize = 4096;
/// Largest log read back: its last this many bytes (it only grows past the
/// compaction size while compaction keeps failing).
const MAX_LOG: u64 = 16 * 1024 * 1024;

/// What the helper trusts: Forge's command key and this node's id.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Pin {
    pub server_id: String,
    /// Base64url Ed25519 public key that signs commands.
    pub command_public_key: String,
    /// `installer` (pinned as root at enrollment) or `first-use`.
    pub source: String,
}

/// The pin from the agent's `identity.json`.
pub fn pin_from_agent(agent_dir: &Path, source: &str) -> Result<Pin, String> {
    let raw = read_nofollow(&agent_dir.join("identity.json"), MAX_FILE)
        .ok_or_else(|| format!("{}/identity.json: not readable (is the node enrolled?)", agent_dir.display()))?;
    let id: crate::config::AgentIdentity = serde_json::from_slice(&raw).map_err(|e| format!("identity.json: {e}"))?;
    if forge_secrets::crypto::b64url_decode(&id.command_public_key).is_none_or(|k| k.len() != 32) {
        return Err("identity.json: the command key is not an Ed25519 key".into());
    }
    Ok(Pin { server_id: id.server_id, command_public_key: id.command_public_key, source: source.into() })
}

/// Writes the pin into the helper's root-only folder (replacing an old one).
pub fn save_pin(dir: &Path, pin: &Pin) -> Result<(), String> {
    ensure_private_dir(dir)?;
    write_nofollow(&dir.join(PIN), &serde_json::to_vec_pretty(pin).map_err(|e| e.to_string())?)
}

pub fn load_pin(dir: &Path) -> Option<Pin> {
    serde_json::from_slice(&read_nofollow(&dir.join(PIN), MAX_FILE)?).ok()
}

/// The helper's trust state for one run of `forge-agent-helper serve`.
#[derive(Debug)]
pub struct Trust {
    dir: PathBuf,
    verifier: Option<CommandVerifier>,
    pin: Option<Pin>,
    /// Lines appended to `seen.log` since the last compaction.
    appended: std::sync::Mutex<usize>,
}

impl Trust {
    /// Loads the pin from `dir`; with `first_use`, pins from the agent's
    /// identity when there is none yet. Restores remembered operation ids.
    #[must_use]
    pub fn open(dir: &Path, agent_dir: &Path, first_use: bool, now: Timestamp) -> Self {
        let mut pin = load_pin(dir);
        if pin.is_none() && first_use {
            match pin_from_agent(agent_dir, "first-use").and_then(|p| save_pin(dir, &p).map(|()| p)) {
                Ok(p) => {
                    eprintln!("forge-agent-helper: pinned Forge's command key on first use (server {})", p.server_id);
                    pin = Some(p);
                }
                Err(e) => eprintln!("forge-agent-helper: no pin yet ({e}); requests are not verified"),
            }
        }
        let verifier = pin.as_ref().and_then(|p| {
            let key = forge_secrets::crypto::b64url_decode(&p.command_public_key)?;
            Some(CommandVerifier::new(key, p.server_id.clone(), now))
        });
        let mut appended = 0;
        let mut damaged = false;
        if let Some(v) = &verifier {
            let mut all: Vec<(i64, String)> = read_nofollow(&dir.join(SEEN), MAX_FILE)
                .and_then(|raw| serde_json::from_slice::<Vec<(i64, String)>>(&raw).ok())
                .unwrap_or_default();
            if let Some((raw, cut)) = read_log_tail(&dir.join(SEEN_LOG), MAX_LOG) {
                // A crash mid-append leaves a torn last line; an oversized
                // log is compacted so it stops growing.
                damaged = cut || (!raw.is_empty() && !raw.ends_with(b"\n"));
                for line in String::from_utf8_lossy(&raw).lines() {
                    match serde_json::from_str::<(i64, String)>(line) {
                        Ok(entry) => {
                            all.push(entry);
                            appended += 1;
                        }
                        Err(_) => damaged = true,
                    }
                }
            }
            v.restore(newest_unexpired(all, now));
        }
        let t = Self { dir: dir.to_path_buf(), verifier, pin, appended: std::sync::Mutex::new(appended) };
        // Start a clean log when the old one was torn or unreadable, so a
        // later append never joins a broken line.
        if damaged
            && let (Some(v), Ok(mut n)) = (&t.verifier, t.appended.lock())
            && t.compact(v).is_ok()
        {
            *n = 0;
        }
        t
    }

    /// Whether signed commands are verified (a pin exists).
    #[must_use]
    pub fn pinned(&self) -> Option<&Pin> {
        self.pin.as_ref()
    }

    /// Turns a request from the agent into the request the helper runs, or
    /// refuses it.
    pub fn admit(&self, req: HelperRequest, now: Timestamp) -> Result<HelperRequest, String> {
        let HelperRequest::Signed { command } = req else {
            if self.verifier.is_some() && !req.allowed_unsigned() {
                return Err("refused: privileged requests must be commands signed by Forge (D-068)".into());
            }
            // Without a pin the agent is trusted as before, but the time of
            // issue is still never the agent's to choose: it is now.
            return Ok(match req {
                HelperRequest::HealthPolicy { health, .. } => {
                    HelperRequest::HealthPolicy { health, issued_at_ms: now.0 }
                }
                other => other,
            });
        };
        let raw =
            forge_secrets::crypto::b64url_decode(&command).ok_or("refused: the signed command is not base64url")?;
        let signed = SignedCommand::decode(raw.as_slice()).map_err(|_| "refused: malformed signed command")?;
        let body = match &self.verifier {
            Some(v) => {
                let body = v.verify(&signed, now).map_err(|e| format!("refused: {e}"))?;
                self.remember(v, &body);
                body
            }
            // No pin: the agent verified it; nothing more to check here.
            None => CommandBody::decode(signed.body.as_slice()).map_err(|_| "refused: malformed command")?,
        };
        let issued_at_ms = body.issued_at_ms;
        let cmd = body.command.ok_or("refused: empty command")?;
        let req =
            HelperRequest::from_command(&cmd)?.ok_or_else(|| format!("refused: `{}` needs no helper", cmd.name()))?;
        // A container shell runs code as root's Docker client: only a helper
        // that verified the signature itself may start one (D-114).
        if self.verifier.is_none() && matches!(req, HelperRequest::ContainerExec { .. }) {
            return Err("refused: container shells need a pinned helper (re-enroll the node)".into());
        }
        // A health policy carries the time Forge issued it, from the signed
        // body: the helper refuses an older one than it holds (D-111).
        Ok(match req {
            HelperRequest::HealthPolicy { health, .. } => HelperRequest::HealthPolicy { health, issued_at_ms },
            other => other,
        })
    }

    /// Writes every remembered id to `seen.json` (fsync) and starts a new log.
    fn compact(&self, v: &CommandVerifier) -> Result<(), String> {
        let raw = serde_json::to_vec(&v.remembered()).map_err(|e| e.to_string())?;
        write_nofollow(&self.dir.join(SEEN), &raw)?;
        let _ = std::fs::remove_file(self.dir.join(SEEN_LOG));
        Ok(())
    }

    /// Records an admitted id so a restarted helper still refuses it.
    fn remember(&self, v: &CommandVerifier, body: &CommandBody) {
        let Ok(mut appended) = self.appended.lock() else { return };
        if *appended >= COMPACT_EVERY {
            match self.compact(v) {
                // The snapshot holds this id too.
                Ok(()) => {
                    *appended = 0;
                    return;
                }
                // Keep recording in the log; compaction is tried again next time.
                Err(e) => eprintln!("forge-agent-helper: cannot save recent operation ids: {e}"),
            }
        }
        let line = format!("{}\n", serde_json::json!([body.expires_at_ms, body.operation_id]));
        match append_nofollow(&self.dir.join(SEEN_LOG), line.as_bytes()) {
            Ok(()) => *appended += 1,
            Err(e) => eprintln!("forge-agent-helper: cannot record an operation id: {e}"),
        }
    }
}

/// Unexpired entries, each id once (its last entry), the newest
/// [`RESTORE_MAX`] kept.
fn newest_unexpired(all: Vec<(i64, String)>, now: Timestamp) -> Vec<(i64, String)> {
    let mut seen = std::collections::HashSet::new();
    let mut keep: Vec<(i64, String)> =
        all.into_iter().rev().filter(|(exp, id)| *exp > now.0 && seen.insert(id.clone())).take(RESTORE_MAX).collect();
    keep.reverse();
    keep
}

/// The last `max` bytes of the log without following a link at it, from
/// the first whole line on, and whether older bytes were left out. The
/// newest ids are at the end, so those are the ones worth keeping.
fn read_log_tail(path: &Path, max: u64) -> Option<(Vec<u8>, bool)> {
    use std::io::{Read as _, Seek as _, SeekFrom};
    let mut opts = std::fs::OpenOptions::new();
    opts.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        opts.custom_flags(i32::try_from(rustix::fs::OFlags::NOFOLLOW.bits()).unwrap_or(0));
    }
    let mut f = opts.open(path).ok()?;
    let len = f.metadata().ok()?.len();
    let start = len.saturating_sub(max);
    f.seek(SeekFrom::Start(start)).ok()?;
    let mut raw = Vec::new();
    f.take(max).read_to_end(&mut raw).ok()?;
    if start > 0 {
        // The first line may begin before the cut.
        let skip = raw.iter().position(|b| *b == b'\n').map_or(raw.len(), |i| i + 1);
        raw.drain(..skip);
    }
    Some((raw, start > 0))
}

/// Appends to a root-only file without following a link at it.
fn append_nofollow(path: &Path, data: &[u8]) -> Result<(), String> {
    use std::io::Write as _;
    let mut opts = std::fs::OpenOptions::new();
    opts.append(true).create(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        opts.mode(0o600).custom_flags(i32::try_from(rustix::fs::OFlags::NOFOLLOW.bits()).unwrap_or(0));
    }
    let mut f = opts.open(path).map_err(|e| format!("{}: {e}", path.display()))?;
    f.write_all(data).map_err(|e| format!("{}: {e}", path.display()))
}

/// The helper's root-only folder.
#[must_use]
pub fn state_dir() -> PathBuf {
    PathBuf::from(HELPER_STATE)
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use std::time::Duration;

    use forge_proto::Command;
    use forge_proto::CommandSigner;
    use forge_proto::messages::{FabricAction, Ping};

    use super::*;

    const NOW: Timestamp = Timestamp(1_800_000_000_000);

    fn enrolled(signer: &CommandSigner) -> tempfile::TempDir {
        let agent = tempfile::tempdir().unwrap();
        let id = crate::config::AgentIdentity {
            server_id: "srv-1".into(),
            command_public_key: forge_secrets::crypto::b64url(&signer.public_key()),
            heartbeat_interval_secs: 15,
        };
        std::fs::write(agent.path().join("identity.json"), serde_json::to_vec(&id).unwrap()).unwrap();
        agent
    }

    fn signed(signer: &CommandSigner, server: &str, cmd: Command) -> HelperRequest {
        let (_, s) = signer.issue(server, "user:1", cmd, NOW, Duration::from_secs(60));
        HelperRequest::Signed { command: forge_secrets::crypto::b64url(&s.encode_to_vec()) }
    }

    fn list_root() -> Command {
        Command::FabricAction(FabricAction { op_json: r#"{"op":"status","iface":"forge01a1d3"}"#.into() })
    }

    /// Remembered ids survive a restart across a compaction of `seen.log`
    /// into `seen.json` (issue #15: no full rewrite per command).
    #[test]
    fn replays_are_refused_after_a_restart_across_a_compaction() {
        let forge = CommandSigner::generate().unwrap();
        let agent = enrolled(&forge);
        let state = tempfile::tempdir().unwrap();
        let trust = Trust::open(state.path(), agent.path(), true, NOW);
        let cmds: Vec<HelperRequest> = (0..COMPACT_EVERY + 5).map(|_| signed(&forge, "srv-1", list_root())).collect();
        for c in &cmds {
            assert!(trust.admit(c.clone(), NOW).is_ok());
        }
        assert!(state.path().join(SEEN).exists(), "compacted");
        let restarted = Trust::open(state.path(), agent.path(), true, NOW);
        for c in [&cmds[0], &cmds[COMPACT_EVERY - 1], &cmds[COMPACT_EVERY + 4]] {
            assert!(restarted.admit(c.clone(), NOW).unwrap_err().contains("replay"));
        }
    }

    /// Review of #15: the newest ids are the ones restored, even when the
    /// snapshot alone fills the replay capacity.
    #[test]
    fn the_newest_ids_survive_a_restart_when_the_cache_is_full() {
        let forge = CommandSigner::generate().unwrap();
        let agent = enrolled(&forge);
        let state = tempfile::tempdir().unwrap();
        let trust = Trust::open(state.path(), agent.path(), true, NOW);
        // 3000 ids that expire, then 1100 newer ones: the snapshot and log
        // together hold more than the capacity, the old ones expired.
        let old: Vec<HelperRequest> = (0..3000).map(|_| signed(&forge, "srv-1", list_root())).collect();
        for c in &old {
            assert!(trust.admit(c.clone(), NOW).is_ok());
        }
        let later = Timestamp(NOW.0 + 10 * 60 * 1000);
        let new: Vec<HelperRequest> = (0..1100)
            .map(|_| {
                let (_, s) = forge.issue("srv-1", "user:1", list_root(), later, Duration::from_secs(60));
                HelperRequest::Signed { command: forge_secrets::crypto::b64url(&s.encode_to_vec()) }
            })
            .collect();
        for c in &new {
            assert!(trust.admit(c.clone(), later).is_ok());
        }
        let restarted = Trust::open(state.path(), agent.path(), true, later);
        for c in &new[1090..] {
            assert!(restarted.admit(c.clone(), later).unwrap_err().contains("replay"), "the newest are remembered");
        }
        // Expired entries are not restored; the newest of duplicates wins.
        let kept = newest_unexpired(vec![(1, "old".into()), (NOW.0 + 5, "a".into()), (NOW.0 + 9, "a".into())], NOW);
        assert_eq!(kept, vec![(NOW.0 + 9, "a".to_owned())]);
    }

    /// A log over [`MAX_LOG`] still restores its newest ids (from its last
    /// bytes) and is compacted at start.
    #[test]
    fn an_oversized_log_restores_its_newest_ids() {
        let forge = CommandSigner::generate().unwrap();
        let agent = enrolled(&forge);
        let state = tempfile::tempdir().unwrap();
        let trust = Trust::open(state.path(), agent.path(), true, NOW);
        let cmds: Vec<HelperRequest> = (0..5).map(|_| signed(&forge, "srv-1", list_root())).collect();
        for c in &cmds {
            assert!(trust.admit(c.clone(), NOW).is_ok());
        }
        let log = state.path().join(SEEN_LOG);
        let recent = std::fs::read(&log).unwrap();
        // Older expired lines push the file past the cap; the cut lands mid-line.
        let filler = b"[1,\"expired-operation-id-padding\"]\n";
        let mut big = Vec::new();
        big.extend_from_slice(b"[1,\"x");
        while (big.len() as u64) < MAX_LOG + 1000 {
            big.extend_from_slice(filler);
        }
        big.extend_from_slice(&recent);
        std::fs::write(&log, &big).unwrap();
        drop(trust);
        let restarted = Trust::open(state.path(), agent.path(), true, NOW);
        for c in &cmds {
            assert!(restarted.admit(c.clone(), NOW).unwrap_err().contains("replay"), "the newest are remembered");
        }
        assert!(std::fs::metadata(&log).map_or(true, |m| m.len() < 4096), "compacted at start");
        let again = Trust::open(state.path(), agent.path(), true, NOW);
        assert!(again.admit(cmds[0].clone(), NOW).unwrap_err().contains("replay"), "kept by the compaction");
    }

    /// Review of #15: a compaction that cannot write keeps recording in the
    /// log, and a torn log is compacted at start.
    #[test]
    fn ids_are_kept_when_compaction_fails_and_a_torn_log_is_repaired() {
        let forge = CommandSigner::generate().unwrap();
        let agent = enrolled(&forge);
        let state = tempfile::tempdir().unwrap();
        // A non-empty folder at `seen.json`: the rename over it fails.
        std::fs::create_dir_all(state.path().join(SEEN).join("x")).unwrap();
        let trust = Trust::open(state.path(), agent.path(), true, NOW);
        let cmds: Vec<HelperRequest> = (0..COMPACT_EVERY + 5).map(|_| signed(&forge, "srv-1", list_root())).collect();
        for c in &cmds {
            assert!(trust.admit(c.clone(), NOW).is_ok());
        }
        assert!(!state.path().join(SEEN).is_file(), "compaction failed");
        let restarted = Trust::open(state.path(), agent.path(), true, NOW);
        for c in [&cmds[0], &cmds[COMPACT_EVERY + 4]] {
            assert!(restarted.admit(c.clone(), NOW).unwrap_err().contains("replay"), "kept in the log");
        }
        std::fs::remove_dir_all(state.path().join(SEEN)).unwrap();
        // A torn last line: compacted at start, so new appends start clean.
        std::fs::write(state.path().join(SEEN_LOG), format!("[{},\"x\"]\n[{},\"y", NOW.0 + 60_000, NOW.0)).unwrap();
        let repaired = Trust::open(state.path(), agent.path(), true, NOW);
        assert!(state.path().join(SEEN).exists() && !state.path().join(SEEN_LOG).exists());
        assert!(repaired.admit(cmds[1].clone(), NOW).is_ok(), "a new id after repair");
    }

    /// Security review (D-068): code running as the agent user could ask the
    /// root helper for anything. A pinned helper runs only what Forge signed.
    #[test]
    fn a_pinned_helper_runs_only_what_forge_signed() {
        let forge = CommandSigner::generate().unwrap();
        let agent = enrolled(&forge);
        let state = tempfile::tempdir().unwrap();
        let trust = Trust::open(state.path(), agent.path(), true, NOW);
        assert_eq!(trust.pinned().unwrap().source, "first-use");

        // Signed by Forge for this node: derived from the command itself.
        let ok = trust.admit(signed(&forge, "srv-1", list_root()), NOW);
        assert!(matches!(ok, Ok(HelperRequest::Fabric { .. })), "{ok:?}");
        // Replayed: refused, also after a restart of the helper.
        let replay = signed(&forge, "srv-1", list_root());
        assert!(trust.admit(replay.clone(), NOW).is_ok());
        assert!(trust.admit(replay.clone(), NOW).unwrap_err().contains("replay"));
        let restarted = Trust::open(state.path(), agent.path(), true, NOW);
        assert!(restarted.admit(replay, NOW).unwrap_err().contains("replay"));
        // Signed by someone else, or for another node: refused.
        let attacker = CommandSigner::generate().unwrap();
        assert!(trust.admit(signed(&attacker, "srv-1", list_root()), NOW).unwrap_err().contains("signature"));
        assert!(trust.admit(signed(&forge, "srv-2", list_root()), NOW).is_err());
        // Plain requests are refused once pinned; the agent's confirmation is not.
        let plain = HelperRequest::parse(r#"{"op":"fabric","fabric":{"op":"status","iface":"forge01a1d3"}}"#).unwrap();
        assert!(trust.admit(plain, NOW).unwrap_err().contains("signed by Forge"));
        assert!(trust.admit(HelperRequest::ConfirmUpdate {}, NOW).is_ok());
        // A command the agent runs itself never reaches the helper.
        assert!(trust.admit(signed(&forge, "srv-1", Command::Ping(Ping {})), NOW).is_err());
    }

    /// D-111: the command checks the helper may run come only from a policy
    /// Forge signed for this node; the agent can name a check to run, never
    /// say what runs.
    #[test]
    fn command_checks_come_signed_and_are_run_by_reference_only() {
        use forge_proto::messages::AppAction;
        let forge = CommandSigner::generate().unwrap();
        let agent = enrolled(&forge);
        let state = tempfile::tempdir().unwrap();
        let trust = Trust::open(state.path(), agent.path(), true, NOW);
        let spec = |name: &str| {
            serde_json::json!({
                "environment": "uat", "name": "api", "runtime": "systemd", "revision": 7,
                "policy": { "readiness": [{ "name": name, "probe": { "type": "exec", "argv": ["/usr/bin/pg_isready"] } }] }
            })
            .to_string()
        };
        let health = |spec_json: String| {
            Command::AppAction(AppAction { action: "health".into(), spec_json, tail: 0, cursor: String::new() })
        };
        // Signed by Forge for this node: the helper takes the policy.
        match trust.admit(signed(&forge, "srv-1", health(spec("db"))), NOW) {
            Ok(HelperRequest::HealthPolicy { health, issued_at_ms }) => {
                assert_eq!((health.revision, health.policy.readiness[0].name.as_str()), (7, "db"));
                assert_eq!(issued_at_ms, NOW.0, "the time of issue comes from the signed command");
            }
            other => panic!("{other:?}"),
        }
        // Signed by someone else, for another node, or not valid: refused.
        let attacker = CommandSigner::generate().unwrap();
        assert!(trust.admit(signed(&attacker, "srv-1", health(spec("db"))), NOW).is_err());
        assert!(trust.admit(signed(&forge, "srv-2", health(spec("db"))), NOW).is_err());
        assert!(trust.admit(signed(&forge, "srv-1", health(spec("Not A Name"))), NOW).unwrap_err().contains("invalid"));
        // The agent cannot hand the helper a policy of its own, whatever
        // time of issue it claims.
        let plain = HelperRequest::parse(&format!(
            r#"{{"op":"health_policy","health":{},"issued_at_ms":9999999999999}}"#,
            spec("db")
        ))
        .unwrap();
        assert!(trust.admit(plain.clone(), NOW).unwrap_err().contains("signed by Forge"));
        // An unpinned helper trusts the agent as before, but not its clock.
        let none = tempfile::tempdir().unwrap();
        let unpinned = Trust::open(tempfile::tempdir().unwrap().path(), none.path(), true, NOW);
        match unpinned.admit(plain, NOW) {
            Ok(HelperRequest::HealthPolicy { issued_at_ms, .. }) => assert_eq!(issued_at_ms, NOW.0),
            other => panic!("{other:?}"),
        }
        // It may ask for a check by reference, without a signed command...
        let by_ref = r#"{"op":"health_check","check":{"environment":"uat","app":"api","purpose":"readiness","check":"db","revision":7}}"#;
        let req = HelperRequest::parse(by_ref).unwrap();
        assert!(req.allowed_unsigned());
        assert!(matches!(trust.admit(req, NOW), Ok(HelperRequest::HealthCheck { .. })));
        // ...but a reference carries no command, and names only slugs.
        for bad in [
            r#"{"op":"health_check","check":{"environment":"uat","app":"api","purpose":"readiness","check":"db","revision":7,"argv":["id"]}}"#,
            r#"{"op":"health_check","check":{"environment":"uat","app":"api","purpose":"readiness","check":"db","revision":7},"script":"id"}"#,
            r#"{"op":"health_check","check":{"environment":"../etc","app":"api","purpose":"readiness","check":"db","revision":7}}"#,
            r#"{"op":"health_check","check":{"environment":"uat","app":"api","purpose":"readiness","check":"a/../b","revision":7}}"#,
            r#"{"op":"health_check","check":{"environment":"uat","app":"api","purpose":"other","check":"db","revision":7}}"#,
        ] {
            assert!(HelperRequest::parse(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn the_pin_outlives_a_changed_agent_identity() {
        let forge = CommandSigner::generate().unwrap();
        let agent = enrolled(&forge);
        let state = tempfile::tempdir().unwrap();
        save_pin(state.path(), &pin_from_agent(agent.path(), "installer").unwrap()).unwrap();
        // The agent user rewrites identity.json with its own key: ignored.
        let attacker = CommandSigner::generate().unwrap();
        let _ = enrolled(&attacker);
        std::fs::write(
            agent.path().join("identity.json"),
            serde_json::to_vec(&crate::config::AgentIdentity {
                server_id: "srv-1".into(),
                command_public_key: forge_secrets::crypto::b64url(&attacker.public_key()),
                heartbeat_interval_secs: 15,
            })
            .unwrap(),
        )
        .unwrap();
        let trust = Trust::open(state.path(), agent.path(), true, NOW);
        assert_eq!(trust.pinned().unwrap().source, "installer");
        assert!(trust.admit(signed(&attacker, "srv-1", list_root()), NOW).is_err());
        assert!(trust.admit(signed(&forge, "srv-1", list_root()), NOW).is_ok());
    }

    /// D-114 review: only a pinned helper starts a container shell, and only
    /// from a command Forge signed for this node.
    #[test]
    fn container_shells_need_a_pin_and_a_signature() {
        use forge_proto::messages::ContainerExecOpen;
        let forge = CommandSigner::generate().unwrap();
        let shell = || {
            Command::ContainerExecOpen(ContainerExecOpen {
                session_id: "a".repeat(32),
                container: "web-1".into(),
                cols: 80,
                rows: 24,
                stream_token: "b".repeat(64),
                idle_secs: 60,
                expires_at_ms: NOW.0 + 30_000,
                lifetime_secs: 60,
            })
        };
        // Unpinned: refused even though it is signed.
        let unpinned = Trust::open(tempfile::tempdir().unwrap().path(), tempfile::tempdir().unwrap().path(), true, NOW);
        assert!(unpinned.pinned().is_none());
        let e = unpinned.admit(signed(&forge, "srv-1", shell()), NOW).unwrap_err();
        assert!(e.contains("pinned helper"), "{e}");
        // Pinned: admitted from Forge's signature, with the limits it signed.
        let agent = enrolled(&forge);
        let state = tempfile::tempdir().unwrap();
        let pinned = Trust::open(state.path(), agent.path(), true, NOW);
        match pinned.admit(signed(&forge, "srv-1", shell()), NOW) {
            Ok(HelperRequest::ContainerExec { exec }) => {
                assert_eq!((exec.container.as_str(), exec.idle_secs, exec.lifetime_secs), ("web-1", 60, 60));
            }
            other => panic!("{other:?}"),
        }
        let attacker = CommandSigner::generate().unwrap();
        assert!(pinned.admit(signed(&attacker, "srv-1", shell()), NOW).is_err());
        // The capability lookup is read-only and allowed unsigned.
        let caps = HelperRequest::parse(r#"{"op":"capabilities"}"#).unwrap();
        assert!(caps.allowed_unsigned());
        assert!(matches!(pinned.admit(caps, NOW), Ok(HelperRequest::Capabilities {})));
    }

    #[test]
    fn without_a_pin_requests_run_as_before() {
        let state = tempfile::tempdir().unwrap();
        let none = tempfile::tempdir().unwrap();
        let trust = Trust::open(state.path(), none.path(), true, NOW);
        assert!(trust.pinned().is_none());
        let plain = HelperRequest::parse(r#"{"op":"fabric","fabric":{"op":"status","iface":"forge01a1d3"}}"#).unwrap();
        assert!(trust.admit(plain, NOW).is_ok());
    }
}
