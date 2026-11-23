//! Port tunnels (D-091, D-100): the persisted tunnel record and explicit
//! tunnel grants. The live sockets stay in the control plane's memory; these
//! types give history, restart recovery and the access an administrator
//! hands to a person without an environment role.
//!
//! A tunnel's *target* is a service (a native or container app, a compose
//! stack's service, an App Store package), a node's SSH port, or, for
//! administrators only, a raw port of a node.

use std::fmt;
use std::str::FromStr;

use async_trait::async_trait;
use serde::{Deserialize, Serialize};

use crate::{DomainResult, ServerId, Timestamp, TunnelGrantId, UserId, WorkspaceId};

/// What a tunnel or grant points at.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TunnelTargetKind {
    /// A native or container service (`target_ref` = service id).
    App,
    /// One service of a compose stack (`target_ref` = `<stack id>/<service>`).
    Stack,
    /// An App Store package installed on a node (`target_ref` = installation id).
    Package,
    /// A node's SSH port (`target_ref` = server id).
    Ssh,
    /// A raw port of a node (`target_ref` = server id). Administrators only;
    /// never granted.
    Raw,
}

impl TunnelTargetKind {
    pub const ALL: [TunnelTargetKind; 5] = [Self::App, Self::Stack, Self::Package, Self::Ssh, Self::Raw];

    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::App => "app",
            Self::Stack => "stack",
            Self::Package => "package",
            Self::Ssh => "ssh",
            Self::Raw => "raw",
        }
    }

    /// Whether an administrator may hand this kind out with a grant.
    #[must_use]
    pub const fn grantable(self) -> bool {
        !matches!(self, Self::Raw)
    }
}

impl fmt::Display for TunnelTargetKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl FromStr for TunnelTargetKind {
    type Err = String;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Self::ALL.into_iter().find(|k| k.as_str() == s).ok_or_else(|| format!("unknown tunnel target kind `{s}`"))
    }
}

/// Explicit permission for one person to tunnel to one target, issued by an
/// administrator (`access.manage`) for people without a role on the
/// service's environment. The grant alone opens the door; roles are not
/// checked for the grantee.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TunnelGrant {
    pub id: TunnelGrantId,
    pub workspace_id: WorkspaceId,
    pub user_id: UserId,
    pub target_kind: TunnelTargetKind,
    pub target_ref: String,
    /// Limit to one node; `None` covers every node the target runs on.
    pub server_id: Option<ServerId>,
    /// Remote ports the grant covers; empty means every port of the target.
    pub ports: Vec<u16>,
    /// Principal of the administrator who issued it.
    pub created_by: String,
    pub created_at: Timestamp,
    /// [`crate::NEVER_EXPIRES`] for a grant without an end.
    pub expires_at: Timestamp,
    pub revoked_at: Option<Timestamp>,
    pub note: String,
}

impl TunnelGrant {
    /// Not revoked and not expired at `now`.
    #[must_use]
    pub fn is_active(&self, now: Timestamp) -> bool {
        self.revoked_at.is_none() && now < self.expires_at
    }

    #[must_use]
    pub fn never_expires(&self) -> bool {
        self.expires_at >= crate::NEVER_EXPIRES
    }

    /// `active`, `expired` or `revoked` at `now`.
    #[must_use]
    pub fn status(&self, now: Timestamp) -> &'static str {
        if self.revoked_at.is_some() {
            "revoked"
        } else if now >= self.expires_at {
            "expired"
        } else {
            "active"
        }
    }

    /// Whether this grant lets `user` reach `port` of `target_ref` on `server`.
    #[must_use]
    pub fn covers(
        &self,
        user: UserId,
        kind: TunnelTargetKind,
        target_ref: &str,
        server: ServerId,
        port: u16,
        now: Timestamp,
    ) -> bool {
        self.is_active(now)
            && self.user_id == user
            && self.target_kind == kind
            && self.target_ref == target_ref
            && self.server_id.is_none_or(|s| s == server)
            && (self.ports.is_empty() || self.ports.contains(&port))
    }
}

/// One tunnel as recorded: open while `closed_at` is `None`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TunnelRecord {
    /// 32 hex digits (the live registry's id).
    pub id: String,
    pub workspace_id: WorkspaceId,
    pub user_id: UserId,
    pub server_id: ServerId,
    pub target_kind: TunnelTargetKind,
    pub target_ref: String,
    /// Human label of the target (`web/site`, `seq-b/seq`, `postgresql`, `ssh`).
    pub label: String,
    /// Remote port on the node.
    pub port: u16,
    /// Address the agent dials on the node (loopback or one of its own).
    pub dial_ip: String,
    /// The address the CLI listens on (as it reported it).
    pub bind: String,
    /// The grant that allowed it, when no role did.
    pub grant_id: Option<TunnelGrantId>,
    pub opened_at: Timestamp,
    pub expires_at: Timestamp,
    pub closed_at: Option<Timestamp>,
    /// `closed`, `idle`, `expired`, `access_withdrawn`, `client_closed`,
    /// `server_restart`, …
    pub close_reason: Option<String>,
    pub bytes_up: u64,
    pub bytes_down: u64,
}

impl TunnelRecord {
    #[must_use]
    pub fn never_expires(&self) -> bool {
        self.expires_at >= crate::NEVER_EXPIRES
    }
}

/// Explicit tunnel grants.
#[async_trait]
pub trait TunnelGrantRepository: Send + Sync + fmt::Debug {
    /// Inserts or replaces by id.
    async fn put_tunnel_grant(&self, grant: &TunnelGrant) -> DomainResult<()>;
    async fn get_tunnel_grant(&self, id: TunnelGrantId) -> DomainResult<Option<TunnelGrant>>;
    /// Grants of one user, or every grant, newest first.
    async fn list_tunnel_grants(&self, user: Option<UserId>) -> DomainResult<Vec<TunnelGrant>>;
}

/// Tunnel history and the rows of open tunnels.
#[async_trait]
pub trait TunnelRepository: Send + Sync + fmt::Debug {
    /// Inserts or replaces by id.
    async fn put_tunnel(&self, tunnel: &TunnelRecord) -> DomainResult<()>;
    async fn get_tunnel(&self, id: &str) -> DomainResult<Option<TunnelRecord>>;
    /// Tunnels of one user or everyone's; open ones first, then the newest
    /// closed ones, at most `limit` rows.
    async fn list_tunnels(&self, user: Option<UserId>, limit: u32) -> DomainResult<Vec<TunnelRecord>>;
    /// Marks a tunnel closed with its final counters.
    async fn close_tunnel(
        &self,
        id: &str,
        at: Timestamp,
        reason: &str,
        bytes_up: u64,
        bytes_down: u64,
    ) -> DomainResult<()>;
    /// Marks every open tunnel closed (at start, after a restart: the sockets
    /// are gone). Returns how many rows changed.
    async fn close_open_tunnels(&self, at: Timestamp, reason: &str) -> DomainResult<u64>;
}

#[cfg(test)]
mod tests {
    use super::*;

    fn grant() -> TunnelGrant {
        TunnelGrant {
            id: TunnelGrantId::new(),
            workspace_id: WorkspaceId::new(),
            user_id: UserId::new(),
            target_kind: TunnelTargetKind::App,
            target_ref: "app-1".into(),
            server_id: None,
            ports: vec![],
            created_by: "user:x".into(),
            created_at: Timestamp(0),
            expires_at: Timestamp(1_000),
            revoked_at: None,
            note: String::new(),
        }
    }

    #[test]
    fn kinds_round_trip_and_raw_ports_are_never_granted() {
        for k in TunnelTargetKind::ALL {
            assert_eq!(k.as_str().parse::<TunnelTargetKind>(), Ok(k));
        }
        assert!("vpn".parse::<TunnelTargetKind>().is_err());
        assert!(!TunnelTargetKind::Raw.grantable());
        assert!(TunnelTargetKind::Ssh.grantable());
    }

    #[test]
    fn a_grant_covers_its_target_only_while_active() {
        let g = grant();
        let (node, other) = (ServerId::new(), ServerId::new());
        let now = Timestamp(500);
        assert!(g.covers(g.user_id, TunnelTargetKind::App, "app-1", node, 8080, now));
        assert!(!g.covers(UserId::new(), TunnelTargetKind::App, "app-1", node, 8080, now), "another user");
        assert!(!g.covers(g.user_id, TunnelTargetKind::Stack, "app-1", node, 8080, now), "another kind");
        assert!(!g.covers(g.user_id, TunnelTargetKind::App, "app-2", node, 8080, now), "another target");
        assert!(!g.covers(g.user_id, TunnelTargetKind::App, "app-1", node, 8080, Timestamp(1_000)), "expired");
        let mut narrow = g.clone();
        narrow.server_id = Some(node);
        narrow.ports = vec![443];
        assert!(narrow.covers(g.user_id, TunnelTargetKind::App, "app-1", node, 443, now));
        assert!(!narrow.covers(g.user_id, TunnelTargetKind::App, "app-1", other, 443, now), "another node");
        assert!(!narrow.covers(g.user_id, TunnelTargetKind::App, "app-1", node, 80, now), "another port");
        let mut revoked = g.clone();
        revoked.revoked_at = Some(Timestamp(400));
        assert!(!revoked.covers(g.user_id, TunnelTargetKind::App, "app-1", node, 8080, now));
        assert_eq!(revoked.status(now), "revoked");
        assert_eq!(g.status(Timestamp(2_000)), "expired");
        let mut forever = g;
        forever.expires_at = crate::NEVER_EXPIRES;
        assert!(forever.never_expires() && forever.status(Timestamp(crate::NEVER_EXPIRES.0 - 1)) == "active");
    }
}
