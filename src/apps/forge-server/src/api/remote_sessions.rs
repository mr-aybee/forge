//! Open terminal and exec sessions, and revoking them (issue #5).
//!
//! * `GET /api/v1/terminal-sessions` — your open sessions; `?all=true` lists
//!   everyone's (`user.manage`).
//! * `POST /api/v1/terminal-sessions/{id}/revoke` — ends a session within a
//!   second. Its owner may revoke it, and so may anyone with `user.manage`.
//!
//! The registry lives in the server process: a session is listed while its
//! WebSocket is open, and a restart ends every session anyway.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use axum::Json;
use axum::extract::{Path, Query, State};
use forge_core::{Action, EntityKind, EntityRef, ServerId, Timestamp, UserId};
use serde::Deserialize;
use serde_json::{Value, json};
use tokio::sync::watch;

use crate::auth::Caller;
use crate::error::{ApiError, ApiResult};
use crate::state::AppState;

#[derive(Debug, Clone)]
struct Entry {
    user_id: UserId,
    user: String,
    server_id: ServerId,
    login: String,
    kind: &'static str,
    client: &'static str,
    started_at: Timestamp,
    /// The sign-in that opened it (session hash, or `apikey:<id>`).
    credential: String,
    revoke: watch::Sender<bool>,
}

/// Open sessions by id.
#[derive(Debug, Clone, Default)]
pub struct RemoteSessions {
    inner: Arc<Mutex<HashMap<String, Entry>>>,
    /// Places held for sessions being opened (checked, not yet
    /// registered), so concurrent opens cannot pass a limit together.
    /// Locked after `inner`, never before.
    reserved: Arc<Mutex<HashMap<u64, Hold>>>,
    next_hold: Arc<std::sync::atomic::AtomicU64>,
}

#[derive(Debug, Clone, Copy)]
struct Hold {
    kind: &'static str,
    user_id: UserId,
    server_id: ServerId,
}

/// Most open sessions of one kind, per user and per node.
#[derive(Debug, Clone, Copy)]
pub struct Limits {
    pub per_user: usize,
    pub per_node: usize,
}

/// Why a place was refused.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum Refusal {
    #[error("you already have {0} open sessions of this kind")]
    PerUser(usize),
    #[error("this node already has {0} open sessions of this kind")]
    PerNode(usize),
}

/// A held place; dropping it gives the place back (register first).
#[derive(Debug)]
pub struct Reservation {
    id: u64,
    sessions: RemoteSessions,
}

impl Drop for Reservation {
    fn drop(&mut self) {
        if let Ok(mut r) = self.sessions.reserved.lock() {
            r.remove(&self.id);
        }
    }
}

/// Removes its session from the registry when dropped.
#[derive(Debug)]
pub struct Registration {
    pub id: String,
    pub revoked: watch::Receiver<bool>,
    sessions: RemoteSessions,
}

impl Drop for Registration {
    fn drop(&mut self) {
        if let Ok(mut m) = self.sessions.inner.lock() {
            m.remove(&self.id);
        }
    }
}

/// What a new session is: `terminal` or `exec`, from the `console` or the `cli`.
#[derive(Debug, Clone)]
pub struct NewSession {
    pub user_id: UserId,
    pub user: String,
    pub server_id: ServerId,
    pub login: String,
    pub kind: &'static str,
    pub client: &'static str,
    pub started_at: Timestamp,
    /// The sign-in that opened it (`Session::id_hash`; `apikey:<id>` for keys).
    pub credential: String,
}

impl RemoteSessions {
    #[must_use]
    pub fn register(&self, s: NewSession) -> Registration {
        // Hex, so an id never starts with `-` (it is a CLI argument).
        let id = hex::encode(forge_secrets::crypto::random_bytes::<16>());
        let (tx, rx) = watch::channel(false);

        if let Ok(mut m) = self.inner.lock() {
            m.insert(
                id.clone(),
                Entry {
                    user_id: s.user_id,
                    user: s.user,
                    server_id: s.server_id,
                    login: s.login,
                    kind: s.kind,
                    client: s.client,
                    started_at: s.started_at,
                    credential: s.credential,
                    revoke: tx,
                },
            );
        }

        Registration { id, revoked: rx, sessions: self.clone() }
    }

    /// Ends every open session whose sign-in matches `pred(credential, user)`;
    /// returns how many. Used when an API key is revoked: a terminal opened
    /// with it ends at once, not only at the next re-check.
    pub fn end_matching(&self, pred: impl Fn(&str, UserId) -> bool) -> usize {
        let Ok(m) = self.inner.lock() else { return 0 };
        let mut n = 0;

        for e in m.values().filter(|e| pred(&e.credential, e.user_id)) {
            let _ = e.revoke.send(true);
            n += 1;
        }

        n
    }

    /// Open sessions of `kind` that `user` holds, and that `server` holds.
    /// Reserved places count too.
    #[must_use]
    pub fn count_open(&self, kind: &str, user: UserId, server: ServerId) -> (usize, usize) {
        let Ok(m) = self.inner.lock() else { return (0, 0) };
        let Ok(r) = self.reserved.lock() else { return (0, 0) };

        Self::count(&m, &r, kind, user, server)
    }

    fn count(
        m: &HashMap<String, Entry>,
        r: &HashMap<u64, Hold>,
        kind: &str,
        user: UserId,
        server: ServerId,
    ) -> (usize, usize) {
        let entries = m.values().filter(|e| e.kind == kind).map(|e| (e.user_id, e.server_id));
        let holds = r.values().filter(|h| h.kind == kind).map(|h| (h.user_id, h.server_id));
        let all: Vec<(UserId, ServerId)> = entries.chain(holds).collect();

        (all.iter().filter(|(u, _)| *u == user).count(), all.iter().filter(|(_, s)| *s == server).count())
    }

    /// Holds a place for a session of `kind` when `user` and `server` are
    /// under `limits`, counting open sessions and held places in one step.
    /// Register the session, then drop the reservation.
    pub fn reserve(
        &self,
        kind: &'static str,
        user: UserId,
        server: ServerId,
        limits: Limits,
    ) -> Result<Reservation, Refusal> {
        // A poisoned lock refuses rather than lifting the limit.
        let m = self.inner.lock().map_err(|_| Refusal::PerNode(limits.per_node))?;
        let mut r = self.reserved.lock().map_err(|_| Refusal::PerNode(limits.per_node))?;
        let (mine, on_node) = Self::count(&m, &r, kind, user, server);
        if mine >= limits.per_user {
            return Err(Refusal::PerUser(limits.per_user));
        }

        if on_node >= limits.per_node {
            return Err(Refusal::PerNode(limits.per_node));
        }

        let id = self.next_hold.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        r.insert(id, Hold { kind, user_id: user, server_id: server });

        Ok(Reservation { id, sessions: self.clone() })
    }

    /// Ends the sessions opened with API key `key_id`.
    pub fn end_api_key(&self, key_id: &str) -> usize {
        let cred = format!("apikey:{key_id}");
        self.end_matching(|c, _| c == cred)
    }

    /// Ends every session `user` opened with any API key.
    pub fn end_user_api_keys(&self, user: UserId) -> usize {
        self.end_matching(|c, u| u == user && c.starts_with("apikey:"))
    }

    fn snapshot(&self) -> Vec<(String, Entry)> {
        self.inner.lock().map(|m| m.iter().map(|(k, v)| (k.clone(), v.clone())).collect()).unwrap_or_default()
    }

    fn get(&self, id: &str) -> Option<Entry> {
        self.inner.lock().ok().and_then(|m| m.get(id).cloned())
    }
}

fn view(id: &str, e: &Entry) -> Value {
    json!({
        "id": id,
        "user_id": e.user_id,
        "user": e.user,
        "server_id": e.server_id,
        "login": e.login,
        "kind": e.kind,
        "client": e.client,
        "started_at": e.started_at,
    })
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct ListQuery {
    #[serde(default)]
    pub all: bool,
}

/// `GET /terminal-sessions`.
pub async fn list(State(state): State<AppState>, caller: Caller, Query(q): Query<ListQuery>) -> ApiResult<Json<Value>> {
    if q.all {
        caller.authorize(Action::UserManage, EntityRef::platform()).await?;
    }

    let me = caller.session.user.id;
    let mut items: Vec<(String, Entry)> =
        state.remote_sessions.snapshot().into_iter().filter(|(_, e)| q.all || e.user_id == me).collect();
    items.sort_by_key(|(_, e)| std::cmp::Reverse(e.started_at.0));

    Ok(Json(json!({ "sessions": items.iter().map(|(id, e)| view(id, e)).collect::<Vec<_>>() })))
}

/// `POST /terminal-sessions/{id}/revoke`.
pub async fn revoke(State(state): State<AppState>, caller: Caller, Path(id): Path<String>) -> ApiResult<Json<Value>> {
    let entry = state.remote_sessions.get(&id).ok_or_else(|| ApiError::not_found("no open session with this id"))?;
    if entry.user_id != caller.session.user.id && !caller.can(Action::UserManage, EntityRef::platform()).await {
        // Someone else's session: do not reveal that it exists.
        return Err(ApiError::not_found("no open session with this id"));
    }

    let _ = entry.revoke.send(true);
    let resource = EntityRef::new(EntityKind::Server, entry.server_id);
    caller
        .audit(
            "terminal.revoke",
            &resource,
            None,
            json!({ "session": id, "owner": entry.user, "login": entry.login, "kind": entry.kind }),
        )
        .await;

    Ok(Json(json!({ "revoked": true, "session": view(&id, &entry) })))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn registration_lists_until_dropped_and_revoke_signals() {
        let reg = RemoteSessions::default();
        let r = reg.register(NewSession {
            user_id: UserId::new(),
            user: "alice".into(),
            server_id: ServerId::new(),
            login: "root".into(),
            kind: "terminal",
            client: "cli",
            started_at: Timestamp(1),
            credential: "h".into(),
        });
        assert_eq!(reg.snapshot().len(), 1);
        assert!(r.id.len() == 32 && r.id.bytes().all(|b| b.is_ascii_hexdigit()), "ids are hex: {}", r.id);
        let e = reg.get(&r.id).unwrap_or_else(|| unreachable!());
        let _ = e.revoke.send(true);
        assert!(*r.revoked.borrow());
        drop(r);
        assert!(reg.snapshot().is_empty());
    }

    /// D-114 review: places are held from the check to the registration, so
    /// concurrent opens cannot pass the limit together.
    #[test]
    #[allow(clippy::unwrap_used)] // a test: a panic is the failure report
    fn places_are_reserved_until_registered() {
        let reg = RemoteSessions::default();
        let limits = Limits { per_user: 2, per_node: 3 };
        let (alice, bob, node) = (UserId::new(), UserId::new(), ServerId::new());
        let a1 = reg.reserve("container", alice, node, limits).unwrap();
        let a2 = reg.reserve("container", alice, node, limits).unwrap();
        assert_eq!(reg.reserve("container", alice, node, limits).unwrap_err(), Refusal::PerUser(2));
        let b1 = reg.reserve("container", bob, node, limits).unwrap();
        assert_eq!(reg.reserve("container", bob, node, limits).unwrap_err(), Refusal::PerNode(3));
        assert!(reg.reserve("terminal", bob, node, limits).is_ok(), "other kinds are counted apart");

        // Registering then releasing keeps the place taken.
        let open = reg.register(NewSession {
            user_id: alice,
            user: "a".into(),
            server_id: node,
            login: "agent".into(),
            kind: "container",
            client: "cli",
            started_at: Timestamp(1),
            credential: "h".into(),
        });
        drop(a1);
        assert_eq!(reg.count_open("container", alice, node), (2, 3));
        assert!(reg.reserve("container", alice, node, limits).is_err());
        drop((a2, b1, open));
        assert_eq!(reg.count_open("container", alice, node), (0, 0));
        assert!(reg.reserve("container", alice, node, limits).is_ok());
    }

    /// #13 review: revoking an API key ends the sessions opened with it.
    #[test]
    fn revoking_a_key_ends_its_sessions_only() {
        let reg = RemoteSessions::default();
        let (alice, bob) = (UserId::new(), UserId::new());
        let open = |user, credential: &str| {
            reg.register(NewSession {
                user_id: user,
                user: "u".into(),
                server_id: ServerId::new(),
                login: "ops".into(),
                kind: "exec",
                client: "cli",
                started_at: Timestamp(1),
                credential: credential.into(),
            })
        };
        let k1 = open(alice, "apikey:k1");
        let k2 = open(alice, "apikey:k2");
        let web = open(alice, "sessionhash");
        let other = open(bob, "apikey:k9");
        assert_eq!(reg.end_api_key("k1"), 1);
        assert!(*k1.revoked.borrow() && !*k2.revoked.borrow());
        assert_eq!(reg.end_user_api_keys(alice), 2, "k1 again and k2");
        assert!(*k2.revoked.borrow());
        assert!(!*web.revoked.borrow(), "a browser session is not a key");
        assert!(!*other.revoked.borrow(), "another user's key");
    }
}
