//! Temporary SFTP access grants: issue, list, revoke, authenticate.
//!
//! A grant is issued by a signed-in user with a recent MFA check, for roots
//! and operations that user holds right now. The token is random, shown once
//! and stored only as a SHA-256 hash. Every operation made with the grant
//! re-reads it (revocation and expiry take effect at once) and is authorized
//! as its creator, so losing a role removes the access too.

use std::collections::HashMap;
use std::net::IpAddr;

use forge_core::files::{FileGrant, NEVER_EXPIRES, NODE_ROOT};
use forge_core::model::AuditOutcome;
use forge_core::model::Server;
use forge_core::{Action, Principal, ServerId, Timestamp};
use forge_secrets::crypto::{constant_time_eq, random_token, sha256_hex};
use serde::{Deserialize, Serialize};
use serde_json::json;

use crate::{Actor, FileError, FileResult, FileService};

/// What a user asks for when issuing access.
#[derive(Debug, Clone, Deserialize)]
pub struct NewGrant {
    pub name: String,
    pub roots: Vec<String>,
    #[serde(default)]
    pub write: bool,
    #[serde(default)]
    pub delete: bool,
    /// Lifetime in minutes (at most the configured maximum). Ignored with
    /// `never_expires`.
    #[serde(default)]
    pub expires_in_minutes: u32,
    /// No expiry: only when `[sftp] allow_never_expiry` is on, and only for
    /// callers with `file.admin`.
    #[serde(default)]
    pub never_expires: bool,
    #[serde(default)]
    pub allowed_ips: Vec<String>,
    #[serde(default = "one")]
    pub max_sessions: u32,
    #[serde(default)]
    pub bandwidth_kbps: Option<u32>,
    #[serde(default)]
    pub transfer_limit: Option<u64>,
}

fn one() -> u32 {
    1
}

/// Changes to a grant; absent fields stay as they are. `null` clears the
/// bandwidth and transfer limits.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GrantChanges {
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub roots: Option<Vec<String>>,
    #[serde(default)]
    pub write: Option<bool>,
    #[serde(default)]
    pub delete: Option<bool>,
    /// New lifetime in minutes, counted from now.
    #[serde(default)]
    pub expires_in_minutes: Option<u32>,
    /// `true`: never expires (same rules as issuing).
    #[serde(default)]
    pub never_expires: Option<bool>,
    #[serde(default)]
    pub allowed_ips: Option<Vec<String>>,
    #[serde(default)]
    pub max_sessions: Option<u32>,
    #[serde(default, deserialize_with = "present")]
    pub bandwidth_kbps: Option<Option<u32>>,
    #[serde(default, deserialize_with = "present")]
    pub transfer_limit: Option<Option<u64>>,
}

/// Tells a present `null` (`Some(None)`) from an absent field (`None`).
fn present<'de, D: serde::Deserializer<'de>, T: Deserialize<'de>>(d: D) -> Result<Option<Option<T>>, D::Error> {
    Option::<T>::deserialize(d).map(Some)
}

/// Fields that changed, as `{field: {before, after}}`. The token hash and
/// usage counters are never included.
#[must_use]
pub fn grant_diff(before: &FileGrant, after: &FileGrant) -> serde_json::Map<String, serde_json::Value> {
    let (b, a) = (json!(before), json!(after));
    let mut out = serde_json::Map::new();

    for key in [
        "name",
        "roots",
        "write",
        "delete",
        "expires_at",
        "allowed_ips",
        "max_sessions",
        "bandwidth_kbps",
        "transfer_limit",
        "owner",
    ] {
        if b[key] != a[key] {
            out.insert(key.to_owned(), json!({ "before": b[key], "after": a[key] }));
        }
    }

    out
}

/// Whether `after` takes anything away that open sessions may be using:
/// a folder, an operation, a client address, a session, bandwidth or
/// transfer allowance, or lifetime.
#[must_use]
pub fn narrows(before: &FileGrant, after: &FileGrant) -> bool {
    let tighter = |b: Option<u64>, a: Option<u64>| match (b, a) {
        (_, None) => false,
        (None, Some(_)) => true,
        (Some(b), Some(a)) => a < b,
    };

    before.roots.iter().any(|r| !after.roots.contains(r))
        || (before.write && !after.write)
        || (before.delete && !after.delete)
        || before.allowed_ips != after.allowed_ips
        || after.max_sessions < before.max_sessions
        || tighter(before.bandwidth_kbps.map(u64::from), after.bandwidth_kbps.map(u64::from))
        || tighter(before.transfer_limit, after.transfer_limit)
        || after.expires_at.0 < before.expires_at.0
}

/// Whether `after` gives anything `before` did not: a folder, upload or delete.
#[must_use]
pub fn widens(before: &FileGrant, after: &FileGrant) -> bool {
    after.roots.iter().any(|r| !before.roots.contains(r))
        || (after.write && !before.write)
        || (after.delete && !before.delete)
}

/// A newly issued grant and its token (shown once).
#[derive(Debug, Clone, Serialize)]
pub struct IssuedGrant {
    pub grant: FileGrant,
    pub token: String,
}

/// Whether `ip` matches an entry of `list` (addresses or CIDR ranges).
#[must_use]
pub fn ip_allowed(list: &[String], ip: Option<IpAddr>) -> bool {
    if list.is_empty() {
        return true;
    }

    let Some(ip) = ip else { return false };

    list.iter().any(|entry| match entry.split_once('/') {
        None => entry.parse::<IpAddr>().is_ok_and(|a| a == ip),
        Some((net, bits)) => match (net.parse::<IpAddr>(), bits.parse::<u32>(), ip) {
            (Ok(IpAddr::V4(n)), Ok(b), IpAddr::V4(a)) if b <= 32 => {
                let mask = if b == 0 { 0 } else { u32::MAX << (32 - b) };
                u32::from(n) & mask == u32::from(a) & mask
            }
            (Ok(IpAddr::V6(n)), Ok(b), IpAddr::V6(a)) if b <= 128 => {
                let mask = if b == 0 { 0 } else { u128::MAX << (128 - b) };
                u128::from(n) & mask == u128::from(a) & mask
            }
            _ => false,
        },
    })
}

fn valid_ip_entry(e: &str) -> bool {
    match e.split_once('/') {
        None => e.parse::<IpAddr>().is_ok(),
        Some((n, b)) => match (n.parse::<IpAddr>(), b.parse::<u32>()) {
            (Ok(IpAddr::V4(_)), Ok(b)) => b <= 32,
            (Ok(IpAddr::V6(_)), Ok(b)) => b <= 128,
            _ => false,
        },
    }
}

impl FileService {
    fn grants_repo(&self) -> FileResult<&std::sync::Arc<dyn forge_core::files::FileGrantRepository>> {
        self.grants
            .as_ref()
            .ok_or_else(|| FileError::Invalid("temporary SFTP access is not enabled on this server".into()))
    }

    /// Issues temporary access. Needs a recent MFA check and, right now,
    /// every right the grant carries on every root it covers.
    pub async fn create_grant(&self, actor: &Actor, server_id: ServerId, req: NewGrant) -> FileResult<IssuedGrant> {
        let repo = self.grants_repo()?.clone();

        let Actor::User { ctx, .. } = actor else {
            return Err(FileError::Denied("temporary access cannot issue more access".into()));
        };

        let server = self.server(server_id).await?;

        if !ctx.mfa {
            self.denied(actor, "file.grant.create", &server, "MFA required").await;
            return Err(FileError::StepUp("creating SFTP access needs a recent MFA check".into()));
        }

        let now = self.clock.now();
        let minutes = if req.never_expires { None } else { Some(req.expires_in_minutes) };
        let expires_at =
            self.expiry(actor, &server, "file.grant.create", req.never_expires, minutes).await?.unwrap_or(now);
        let token = random_token::<32>();
        let grant = FileGrant {
            id: format!("fg{}", crate::random_hex(8)),
            workspace_id: server.workspace_id,
            server_id,
            name: req.name,
            roots: req.roots,
            write: req.write,
            delete: req.delete,
            token_hash: sha256_hex(token.as_bytes()),
            created_by: actor.key(),
            owner: actor.key(),
            created_at: now,
            expires_at,
            revoked_at: None,
            allowed_ips: req.allowed_ips,
            max_sessions: req.max_sessions,
            bandwidth_kbps: req.bandwidth_kbps,
            transfer_limit: req.transfer_limit,
            bytes_transferred: 0,
            last_used_at: None,
        };
        let grant = self.checked(actor, &server, grant).await?;
        repo.put_grant(&grant).await.map_err(|e| FileError::Internal(e.to_string()))?;
        self.record(
            actor,
            "file.grant.create",
            &server,
            AuditOutcome::Success,
            json!({
                "grant": grant.id, "name": grant.name, "roots": grant.roots, "write": grant.write,
                "delete": grant.delete, "expires_at": grant.expires_at.0, "never_expires": grant.never_expires(),
                "allowed_ips": grant.allowed_ips,
            }),
        )
        .await;

        Ok(IssuedGrant { grant, token })
    }

    /// The expiry asked for: `never` (allowed only when the administrator
    /// enabled it, and only to `file.admin`) or `minutes` from now within
    /// the configured maximum. `None` when neither is given.
    async fn expiry(
        &self,
        actor: &Actor,
        server: &Server,
        action: &str,
        never: bool,
        minutes: Option<u32>,
    ) -> FileResult<Option<Timestamp>> {
        if never {
            if !self.allow_never_expiry {
                self.denied(actor, action, server, "never-expiring access is disabled").await;
                return Err(FileError::Denied(
                    "access that never expires is disabled; an administrator can enable `[sftp] allow_never_expiry`"
                        .into(),
                ));
            }

            if self.require(actor, Action::FileAdmin, server).await.is_err() {
                return Err(FileError::Denied("only file administrators may issue access that never expires".into()));
            }

            return Ok(Some(Timestamp(NEVER_EXPIRES)));
        }

        let Some(m) = minutes else { return Ok(None) };
        if m == 0 || i64::from(m) > self.max_grant_minutes {
            return Err(FileError::Invalid(format!(
                "the expiry must be between 1 minute and {} hours",
                self.max_grant_minutes / 60
            )));
        }

        Ok(Some(Timestamp(self.clock.now().0 + i64::from(m) * 60_000)))
    }

    /// Normalises and validates a grant's settings. The issuer must hold
    /// everything the grant carries, on every root, right now.
    async fn checked(&self, actor: &Actor, server: &Server, mut g: FileGrant) -> FileResult<FileGrant> {
        g.name = g.name.trim().to_owned();

        if g.name.is_empty() || g.name.len() > 120 {
            return Err(FileError::Invalid("the name must be 1–120 characters".into()));
        }

        g.roots.sort();
        g.roots.dedup();

        if g.roots.is_empty() || g.roots.len() > 8 {
            return Err(FileError::Invalid("choose between 1 and 8 roots".into()));
        }

        if g.max_sessions == 0 || g.max_sessions > 10 {
            return Err(FileError::Invalid("allow between 1 and 10 sessions".into()));
        }

        g.allowed_ips = g.allowed_ips.iter().map(|s| s.trim().to_owned()).filter(|s| !s.is_empty()).collect();

        if g.allowed_ips.len() > 32 || !g.allowed_ips.iter().all(|e| valid_ip_entry(e)) {
            return Err(FileError::Invalid("allowed addresses must be IPs or CIDR ranges (at most 32)".into()));
        }

        if g.bandwidth_kbps == Some(0) || g.transfer_limit == Some(0) {
            return Err(FileError::Invalid("limits must be greater than 0".into()));
        }

        let mut needed = vec![Action::FileRead];

        if g.write {
            needed.push(Action::FileWrite);
        }

        if g.delete {
            needed.push(Action::FileDelete);
        }

        for a in &needed {
            self.require(actor, *a, server).await?;
        }

        for r in &g.roots {
            let resolved = self.resolve(actor, server, r).await?;
            if resolved.read_only && (g.write || g.delete) {
                return Err(FileError::Invalid(format!("root `{r}` is read-only; issue read-only access to it")));
            }
        }

        Ok(g)
    }

    /// A grant of this server the caller may manage: its creator, or
    /// `file.admin`. Changing it also needs a recent MFA check.
    async fn managed_grant(
        &self,
        actor: &Actor,
        server: &Server,
        id: &str,
        action: &str,
        needs_mfa: bool,
    ) -> FileResult<FileGrant> {
        let repo = self.grants_repo()?.clone();

        let Actor::User { ctx, .. } = actor else {
            return Err(FileError::Denied("temporary access cannot manage access".into()));
        };

        let g = repo
            .get_grant(id)
            .await
            .map_err(|e| FileError::Internal(e.to_string()))?
            .filter(|g| g.server_id == server.id)
            .ok_or_else(|| FileError::NotFound("access not found".into()))?;

        if g.created_by != actor.key() && g.owner != actor.key() {
            self.require(actor, Action::FileAdmin, server).await?;
        }

        if needs_mfa && !ctx.mfa {
            self.denied(actor, action, server, "MFA required").await;
            return Err(FileError::StepUp("changing SFTP access needs a recent MFA check".into()));
        }

        Ok(g)
    }

    /// Changes a grant without changing its password. Same rules as issuing
    /// it; open sessions close when access narrows, so they reconnect with
    /// the new settings (every operation re-reads the grant anyway).
    pub async fn update_grant(
        &self,
        actor: &Actor,
        server_id: ServerId,
        id: &str,
        changes: GrantChanges,
    ) -> FileResult<FileGrant> {
        let repo = self.grants_repo()?.clone();
        let server = self.server(server_id).await?;
        let before = self.managed_grant(actor, &server, id, "file.grant.update", true).await?;
        if before.revoked_at.is_some() {
            return Err(FileError::Conflict("revoked access cannot be changed".into()));
        }

        let mut after = before.clone();

        if let Some(v) = changes.name {
            after.name = v;
        }

        if let Some(v) = changes.roots {
            after.roots = v;
        }

        if let Some(v) = changes.write {
            after.write = v;
        }

        if let Some(v) = changes.delete {
            after.delete = v;
        }

        if let Some(v) = changes.allowed_ips {
            after.allowed_ips = v;
        }

        if let Some(v) = changes.max_sessions {
            after.max_sessions = v;
        }

        if let Some(v) = changes.bandwidth_kbps {
            after.bandwidth_kbps = v;
        }

        if let Some(v) = changes.transfer_limit {
            after.transfer_limit = v;
        }

        let never = changes.never_expires == Some(true);

        if changes.never_expires == Some(false) && changes.expires_in_minutes.is_none() && before.never_expires() {
            return Err(FileError::Invalid("give `expires_in_minutes` to set an expiry".into()));
        }

        let minutes = if never { None } else { changes.expires_in_minutes };
        if let Some(at) = self.expiry(actor, &server, "file.grant.update", never, minutes).await? {
            after.expires_at = at;
        }

        let mut after = self.checked(actor, &server, after).await?;

        // Widening makes the editor the owner: the grant then acts with the
        // editor's rights, never with rights its previous owner did not hold.
        if widens(&before, &after) {
            after.owner = actor.key();
        }

        let diff = grant_diff(&before, &after);
        if diff.is_empty() {
            return Ok(before);
        }

        repo.put_grant(&after).await.map_err(|e| FileError::Internal(e.to_string()))?;

        // A new owner also means open sessions act for someone else: close them.
        let narrowed = narrows(&before, &after) || before.owner != after.owner;
        self.record(
            actor,
            "file.grant.update",
            &server,
            AuditOutcome::Success,
            json!({ "grant": after.id, "changes": diff, "sessions_closed": narrowed }),
        )
        .await;

        if narrowed {
            let _ = self.grant_drops.send(after.id.clone());
        }

        Ok(after)
    }

    /// Replaces a grant's password; the new one is in this answer only.
    /// Open sessions close.
    pub async fn reset_grant_password(&self, actor: &Actor, server_id: ServerId, id: &str) -> FileResult<IssuedGrant> {
        let repo = self.grants_repo()?.clone();
        let server = self.server(server_id).await?;
        let mut g = self.managed_grant(actor, &server, id, "file.grant.reset_password", true).await?;
        if g.revoked_at.is_some() {
            return Err(FileError::Conflict("revoked access cannot be changed".into()));
        }

        let token = random_token::<32>();
        g.token_hash = sha256_hex(token.as_bytes());
        repo.put_grant(&g).await.map_err(|e| FileError::Internal(e.to_string()))?;
        self.record(actor, "file.grant.reset_password", &server, AuditOutcome::Success, json!({ "grant": g.id })).await;
        let _ = self.grant_drops.send(g.id.clone());

        Ok(IssuedGrant { grant: g, token })
    }

    /// Grants on a server: all of them with `file.admin`, else the caller's own.
    pub async fn list_grants(&self, actor: &Actor, server_id: ServerId) -> FileResult<Vec<FileGrant>> {
        let repo = self.grants_repo()?.clone();
        let server = self.server(server_id).await?;
        self.require(actor, Action::FileRead, &server).await?;
        let admin = self.can(actor, Action::FileAdmin, server_id).await;
        let all = repo.list_grants(server_id).await.map_err(|e| FileError::Internal(e.to_string()))?;
        let me = actor.key();

        Ok(all.into_iter().filter(|g| admin || g.created_by == me || g.owner == me).collect())
    }

    /// One grant, visible as in [`Self::list_grants`].
    pub async fn get_grant(&self, actor: &Actor, server_id: ServerId, id: &str) -> FileResult<FileGrant> {
        self.list_grants(actor, server_id)
            .await?
            .into_iter()
            .find(|g| g.id == id)
            .ok_or_else(|| FileError::NotFound("access not found".into()))
    }

    /// Grants across the fleet the caller may see, with their server: on
    /// each server where the caller holds `file.read`, all grants with
    /// `file.admin`, else the caller's own. Servers it cannot see are
    /// skipped silently (no denial per server).
    pub async fn list_all_grants(&self, actor: &Actor) -> FileResult<Vec<(FileGrant, Server)>> {
        let repo = self.grants_repo()?.clone();
        let all = repo.list_all_grants().await.map_err(|e| FileError::Internal(e.to_string()))?;
        let mut seen: HashMap<ServerId, Option<(Server, bool)>> = HashMap::new();
        let mut out = Vec::new();
        let mut ids: Vec<ServerId> = all.iter().map(|g| g.server_id).collect();
        ids.sort_unstable();
        ids.dedup();

        for id in ids {
            let entry = match self.servers.get_server(id).await {
                Ok(s) if self.can(actor, Action::FileRead, s.id).await => {
                    let admin = self.can(actor, Action::FileAdmin, s.id).await;
                    Some((s, admin))
                }
                _ => None,
            };

            seen.insert(id, entry);
        }

        for g in all {
            if let Some(Some((s, admin))) = seen.get(&g.server_id)
                && (*admin || g.created_by == actor.key() || g.owner == actor.key())
            {
                let s = s.clone();
                out.push((g, s));
            }
        }

        Ok(out)
    }

    /// Whether the caller may issue access that never expires on a server
    /// (enabled by the administrator, and `file.admin`). Not audited.
    pub async fn can_issue_never_expiring(&self, actor: &Actor, server_id: ServerId) -> bool {
        self.allow_never_expiry && self.can(actor, Action::FileAdmin, server_id).await
    }

    /// Revokes a grant (its creator, or `file.admin`). Open sessions close
    /// at once.
    pub async fn revoke_grant(&self, actor: &Actor, server_id: ServerId, id: &str) -> FileResult<FileGrant> {
        let repo = self.grants_repo()?.clone();
        let server = self.server(server_id).await?;
        let mut g = self.managed_grant(actor, &server, id, "file.grant.revoke", false).await?;
        if g.revoked_at.is_none() {
            g.revoked_at = Some(self.clock.now());
            repo.put_grant(&g).await.map_err(|e| FileError::Internal(e.to_string()))?;
        }

        self.record(actor, "file.grant.revoke", &server, AuditOutcome::Success, json!({ "grant": g.id })).await;
        let _ = self.grant_drops.send(g.id.clone());

        Ok(g)
    }

    /// Checks an SFTP login. Every failure gives the same answer.
    pub async fn authenticate(&self, user: &str, token: &str, peer: Option<IpAddr>) -> FileResult<Actor> {
        let generic = || FileError::Denied("invalid credentials".into());
        let repo = self.grants_repo()?.clone();

        if !(user.len() == 18 && user.starts_with("fg") && user[2..].bytes().all(|b| b.is_ascii_hexdigit())) {
            return Err(generic());
        }

        let g = repo.get_grant(user).await.map_err(|e| FileError::Internal(e.to_string()))?.ok_or_else(generic)?;
        let hash = sha256_hex(token.as_bytes());
        if !constant_time_eq(hash.as_bytes(), g.token_hash.as_bytes()) {
            return Err(generic());
        }

        if !g.is_active(self.clock.now()) || !ip_allowed(&g.allowed_ips, peer) {
            return Err(generic());
        }

        let creator: Principal = g.owner.parse().map_err(|_| generic())?;

        Ok(Actor::Grant { grant: Box::new(g), creator })
    }

    /// The grant behind an actor, re-read now (active or an error).
    pub async fn current_grant(&self, actor: &Actor) -> FileResult<Option<FileGrant>> {
        let Actor::Grant { grant, .. } = actor else { return Ok(None) };
        let repo = self.grants_repo()?.clone();
        let g = repo
            .get_grant(&grant.id)
            .await
            .map_err(|e| FileError::Internal(e.to_string()))?
            .filter(|g| g.is_active(self.clock.now()))
            .ok_or_else(|| FileError::Denied("this access has expired or was revoked".into()))?;

        Ok(Some(g))
    }

    /// Checks a grant actor against one operation: root, operations and
    /// transfer allowance. Runs before the creator's own Cedar checks.
    pub(crate) async fn check_grant(&self, actor: &Actor, root_id: &str, actions: &[Action]) -> FileResult<()> {
        let Some(g) = self.current_grant(actor).await? else { return Ok(()) };
        if !g.roots.iter().any(|r| r == root_id) {
            return Err(FileError::Denied("this access does not cover that folder".into()));
        }

        for a in actions {
            let ok = match a {
                Action::FileRead | Action::FileNode => true,
                Action::FileWrite => g.write,
                Action::FileDelete => g.delete,
                _ => false,
            };

            if !ok {
                return Err(FileError::Denied(format!("this access does not allow `{a}`")));
            }
        }

        if root_id == NODE_ROOT && !g.roots.iter().any(|r| r == NODE_ROOT) {
            return Err(FileError::Denied("this access does not cover the whole node".into()));
        }

        if g.transfer_limit.is_some_and(|l| g.bytes_transferred >= l) {
            return Err(FileError::TooLarge("this access has used up its transfer allowance".into()));
        }

        // The owner must hold every right this operation uses right now, on
        // this node (read, upload/create, delete/replace). Checked against the
        // grant as stored, so an ownership change applies at once.
        let owner: Principal =
            g.owner.parse().map_err(|_| FileError::Denied("this access has no valid owner".into()))?;
        let as_owner = Actor::Grant { grant: Box::new(g.clone()), creator: owner };

        for a in actions {
            if !self.can(&as_owner, *a, g.server_id).await {
                return Err(FileError::Denied(format!("the owner of this access no longer holds `{a}` on this node")));
            }
        }

        Ok(())
    }

    /// Whether a grant may still be used: not revoked or expired, and its
    /// creator still holds `file.read` on the server (losing the role ends it).
    pub async fn grant_is_active(&self, id: &str) -> bool {
        let Some(repo) = self.grants.as_ref() else { return false };
        let Ok(Some(g)) = repo.get_grant(id).await else { return false };
        if !g.is_active(self.clock.now()) {
            return false;
        }

        let Ok(creator) = g.owner.parse::<Principal>() else { return false };
        let server = g.server_id;
        let actor = Actor::Grant { grant: Box::new(g), creator };

        self.can(&actor, Action::FileRead, server).await
    }

    /// Adds transferred bytes to a grant's usage.
    pub async fn grant_usage(&self, actor: &Actor, bytes: u64) {
        if let (Actor::Grant { grant, .. }, Some(repo)) = (actor, self.grants.as_ref())
            && bytes > 0
            && let Err(e) = repo.add_grant_usage(&grant.id, bytes, self.clock.now()).await
        {
            tracing::warn!(error = %e, "grant usage not recorded");
        }
    }

    /// Records a gateway event (connect, disconnect, refused login) for a grant.
    pub async fn grant_event(&self, actor: &Actor, action: &str, outcome: AuditOutcome, details: serde_json::Value) {
        if let Actor::Grant { grant, .. } = actor
            && let Ok(server) = self.server(grant.server_id).await
        {
            self.record(actor, action, &server, outcome, details).await;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn grant() -> FileGrant {
        FileGrant {
            id: "fg0000000000000001".into(),
            workspace_id: forge_core::WorkspaceId::new(),
            server_id: ServerId::new(),
            name: "vendor".into(),
            roots: vec!["fr_a".into(), "fr_b".into()],
            write: true,
            delete: false,
            token_hash: "secret-hash".into(),
            created_by: "user:x".into(),
            owner: "user:x".into(),
            created_at: Timestamp(0),
            expires_at: Timestamp(1_000),
            revoked_at: None,
            allowed_ips: vec![],
            max_sessions: 2,
            bandwidth_kbps: None,
            transfer_limit: Some(100),
            bytes_transferred: 5,
            last_used_at: None,
        }
    }

    #[test]
    fn diffs_show_before_and_after_without_secrets() {
        let a = grant();
        let mut b = a.clone();
        b.name = "renamed".into();
        b.token_hash = "other".into();
        b.bytes_transferred = 99;
        let d = grant_diff(&a, &b);
        assert_eq!(d.len(), 1, "{d:?}");
        assert_eq!(d["name"], json!({"before": "vendor", "after": "renamed"}));
        assert!(!serde_json::to_string(&d).unwrap_or_default().contains("hash"));
    }

    #[test]
    fn narrowing_is_detected_and_widening_is_not() {
        let a = grant();
        let widen = |f: &dyn Fn(&mut FileGrant)| {
            let mut b = a.clone();
            f(&mut b);
            narrows(&a, &b)
        };
        assert!(!widen(&|b| b.name = "x".into()));
        assert!(!widen(&|b| b.delete = true));
        assert!(!widen(&|b| b.roots.push("fr_c".into())));
        assert!(!widen(&|b| b.transfer_limit = None));
        assert!(!widen(&|b| b.expires_at = Timestamp(NEVER_EXPIRES)));
        assert!(!widen(&|b| b.max_sessions = 5));
        assert!(widen(&|b| b.write = false));
        assert!(widen(&|b| b.roots = vec!["fr_a".into()]));
        assert!(widen(&|b| b.allowed_ips = vec!["10.0.0.0/8".into()]));
        assert!(widen(&|b| b.bandwidth_kbps = Some(10)));
        assert!(widen(&|b| b.transfer_limit = Some(50)));
        assert!(widen(&|b| b.max_sessions = 1));
        assert!(widen(&|b| b.expires_at = Timestamp(500)));
    }

    #[test]
    fn widening_means_new_folders_or_operations() {
        let a = grant();
        let mut b = a.clone();
        b.write = false;
        b.roots.pop();
        assert!(!widens(&a, &b), "narrowing keeps the owner");
        b.delete = true;
        assert!(widens(&a, &b));
        let mut c = a.clone();
        c.roots = vec!["fr_c".into()];
        assert!(widens(&a, &c));
        let mut d = a.clone();
        d.name = "x".into();
        d.expires_at = Timestamp(NEVER_EXPIRES);
        assert!(!widens(&a, &d), "expiry and restrictions do not move ownership");
    }

    #[test]
    fn changes_tell_null_from_absent() {
        let c: GrantChanges = serde_json::from_value(json!({"bandwidth_kbps": null})).unwrap_or_default();
        assert_eq!(c.bandwidth_kbps, Some(None));
        assert_eq!(c.transfer_limit, None);
        assert!(serde_json::from_value::<GrantChanges>(json!({"token_hash": "x"})).is_err());
    }

    #[test]
    fn ip_allowlists() {
        let ip = |s: &str| s.parse::<IpAddr>().ok();
        assert!(ip_allowed(&[], ip("203.0.113.9")));
        let list = vec!["203.0.113.0/24".to_owned(), "2001:db8::/32".to_owned(), "198.51.100.7".to_owned()];
        assert!(ip_allowed(&list, ip("203.0.113.9")));
        assert!(!ip_allowed(&list, ip("203.0.114.9")));
        assert!(ip_allowed(&list, ip("2001:db8::1")));
        assert!(ip_allowed(&list, ip("198.51.100.7")));
        assert!(!ip_allowed(&list, ip("198.51.100.8")));
        assert!(!ip_allowed(&list, None));
        assert!(
            valid_ip_entry("10.0.0.0/8")
                && valid_ip_entry("::1")
                && !valid_ip_entry("10.0.0.0/33")
                && !valid_ip_entry("x")
        );
    }
}
