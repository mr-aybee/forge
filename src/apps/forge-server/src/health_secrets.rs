//! Vault-backed header values for HTTP health checks (issue #12, D-111).
//!
//! A health policy names a credential header by reference (`vault:<id>`).
//! The value is never part of the policy sent to a node. When a node is
//! about to run such a check it asks for the values of that one check over
//! its mTLS link, and [`HealthSecrets::resolve`] decides, every time:
//!
//! 1. the node asks no more often than [`RATE_LIMIT`] per minute;
//! 2. the service is placed on the asking node, and the node runs the
//!    stored policy's revision;
//! 3. the stored policy still has that check, enabled, and the references
//!    are read from the stored policy, never from the request;
//! 4. somebody saved exactly this revision of the policy
//!    ([`HealthAuthority`]): a policy that changed some other way (the
//!    listening port, a write that bypassed the health endpoint) has no
//!    authority until it is saved again;
//! 5. that person still exists, is not disabled, is still a platform
//!    administrator and still passes `secret.read` on the workspace: the
//!    same rule as revealing the item, checked against their roles now;
//! 6. every referenced item still exists in the service's workspace;
//! 7. the audit entry for this answer, when one is due, was written. A
//!    value is never released on an answer that could not be audited.
//!
//! A grant carries the values and [`GRANT_TTL`]; the node keeps them in
//! memory for that long and asks again, so a revoked role or a deleted item
//! takes effect within that time. A refusal carries a code only.
//!
//! Audit (`service.health.secret_use`): the first answer for a check on
//! a node, every change of the answer (granted, refused, another reason,
//! another policy revision or saver), and one entry per day while nothing
//! changes. Never every probe, never a value. Requests that name no real
//! check on the node are refused without an audit entry, so a node cannot
//! fill the log; they are counted and logged at most once a minute.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use forge_catalog::health::{HealthPolicy, Probe};
use forge_core::model::{AuditEvent, AuditOutcome, Service};
use forge_core::ports::{
    AuditRecorder, Authorizer, HealthAuthority, HealthAuthorityRepository, RoleBindingRepository, ServiceRepository,
    UserRepository, VaultRepository,
};
use forge_core::{
    AccessRequest, Action, AuthzContext, Clock, DomainError, DomainResult, EntityKind, EntityRef, Principal, Role,
    ServerId, ServiceId, Timestamp, VaultItemId,
};
use forge_proto::messages::{
    HealthSecretHeader, HealthSecretsRequest, HealthSecretsResponse, health_secret_denial as code,
};
use forge_secrets::Envelope;

/// Audit action of a vault value handed to, or refused to, a node's check.
pub const AUDIT_SECRET_USE: &str = "service.health.secret_use";
/// How long a node may reuse granted values.
pub const GRANT_TTL: Duration = Duration::from_secs(300);
/// How long a node waits before it asks again after a refusal.
pub const DENIAL_TTL: Duration = Duration::from_secs(60);
/// Requests one node may make per minute. A node asks once per check every
/// [`GRANT_TTL`] or [`DENIAL_TTL`]: 30 checks per service leave room.
pub const RATE_LIMIT: u32 = 120;
const MINUTE_MS: i64 = 60_000;
const DAY_MS: i64 = 86_400_000;
/// Nodes and checks remembered at most; older entries are dropped first.
const MAX_REMEMBERED: usize = 50_000;

/// Why a node gets no values. The code travels to the node; the node's
/// report says what the operator should do.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Denial {
    /// No such service placed on the asking node.
    NotPlaced,
    /// The node runs another policy revision than the stored one.
    StalePolicy,
    /// The stored policy has no enabled HTTP check of that name with a
    /// vault reference.
    NoSuchCheck,
    /// Nobody is on record as having saved this revision of the policy.
    NoAuthority,
    /// The person who saved it no longer exists.
    SaverGone,
    SaverDisabled,
    /// They may no longer use vault items.
    NotAllowed,
    /// A referenced item no longer exists in the workspace.
    ItemMissing,
    /// The node asked more often than [`RATE_LIMIT`].
    RateLimited,
    Internal,
}

impl Denial {
    #[must_use]
    pub const fn code(self) -> &'static str {
        match self {
            Self::NotPlaced => code::NOT_PLACED,
            Self::StalePolicy => code::STALE_POLICY,
            Self::NoSuchCheck => code::NO_SUCH_CHECK,
            Self::NoAuthority => code::NO_AUTHORITY,
            Self::SaverGone => code::SAVER_GONE,
            Self::SaverDisabled => code::SAVER_DISABLED,
            Self::NotAllowed => code::NOT_ALLOWED,
            Self::ItemMissing => code::ITEM_MISSING,
            Self::RateLimited => code::RATE_LIMITED,
            Self::Internal => code::INTERNAL,
        }
    }

    /// Refusals about a check that exists are audited; requests that name
    /// nothing real are not (a node could ask for anything).
    const fn audited(self) -> bool {
        !matches!(self, Self::NotPlaced | Self::StalePolicy | Self::NoSuchCheck | Self::RateLimited)
    }
}

/// The ports and keys the decision needs.
#[derive(Debug, Clone)]
pub struct Ports {
    pub apps: Arc<dyn ServiceRepository>,
    pub authority: Arc<dyn HealthAuthorityRepository>,
    pub vault: Arc<dyn VaultRepository>,
    pub users: Arc<dyn UserRepository>,
    pub bindings: Arc<dyn RoleBindingRepository>,
    pub authz: Arc<dyn Authorizer>,
    pub audit: Arc<dyn AuditRecorder>,
    pub envelope: Envelope,
    pub clock: Arc<dyn Clock>,
}

/// What was last audited for one check on one node.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Audited {
    outcome: &'static str,
    revision: i64,
    saver: Option<Principal>,
    day: i64,
}

/// (node, service, `<purpose>.<check>`).
type CheckKey = (ServerId, ServiceId, String);

/// Requests of one node in the current minute.
#[derive(Debug, Clone, Copy)]
struct Window {
    minute: i64,
    count: u32,
}

/// Refusals that leave no audit entry, since the last log line.
#[derive(Debug, Default)]
struct Unaudited {
    count: u64,
    logged_minute: i64,
}

/// Hands vault-backed header values to nodes, or refuses.
#[derive(Debug)]
pub struct HealthSecrets {
    ports: Ports,
    audited: Mutex<HashMap<CheckKey, Audited>>,
    rates: Mutex<HashMap<ServerId, Window>>,
    unaudited: Mutex<Unaudited>,
}

/// One check of a stored policy with vault references.
struct Wanted<'a> {
    app: &'a Service,
    revision: i64,
    /// (header name, vault item) in policy order.
    headers: Vec<(String, VaultItemId)>,
}

/// The vault references of the check `req` names in `policy`, in order.
/// `None` when there is no such enabled HTTP check with a reference, or a
/// reference is not an item id.
fn references(policy: &HealthPolicy, purpose: &str, check: &str) -> Option<Vec<(String, VaultItemId)>> {
    let (_, c) = policy.checks().find(|(p, c)| p.as_str() == purpose && c.name == check && c.enabled)?;
    let Probe::Http(h) = &c.probe else { return None };
    let refs: Option<Vec<(String, VaultItemId)>> = h
        .headers
        .iter()
        .filter_map(|hd| hd.secret.as_deref().map(|s| (hd.name.as_str(), s)))
        .map(|(name, s)| Some((name.to_owned(), s.strip_prefix("vault:")?.parse().ok()?)))
        .collect();

    refs.filter(|r| !r.is_empty())
}

/// Whether one more request fits in `w`'s minute; counts it when it does.
fn admit(w: &mut Window, minute: i64) -> bool {
    if w.minute != minute {
        *w = Window { minute, count: 0 };
    }

    if w.count >= RATE_LIMIT {
        return false;
    }

    w.count += 1;

    true
}

impl HealthSecrets {
    #[must_use]
    pub fn new(ports: Ports) -> Self {
        Self {
            ports,
            audited: Mutex::new(HashMap::new()),
            rates: Mutex::new(HashMap::new()),
            unaudited: Mutex::new(Unaudited::default()),
        }
    }

    /// Stores a changed health policy (`app` carries it) together with who
    /// saved it, atomically: the record names `by` and the policy's
    /// revision when the policy refers to vault items, and is removed when
    /// it does not. The caller has just authorized `by` for every item.
    pub async fn save_policy(&self, app: &Service, policy: &HealthPolicy, by: Principal) -> DomainResult<()> {
        let refers = policy.checks().any(|(_, c)| match &c.probe {
            Probe::Http(h) => h.has_secret(),
            _ => false,
        });
        let revision = crate::health_ingest::apply_spec(app).map_or(0, |s| s.revision);
        let saved = refers.then(|| HealthAuthority {
            service_id: app.id,
            saved_by: by,
            saved_at: self.ports.clock.now(),
            revision,
        });

        self.ports.authority.save_health_policy(app, saved.as_ref()).await
    }

    /// The answer for one request of the node `server`.
    pub async fn resolve(&self, server: ServerId, req: &HealthSecretsRequest) -> HealthSecretsResponse {
        let now = self.ports.clock.now();
        if !self.within_rate(server, now) {
            return self.refuse_quietly(server, Denial::RateLimited, now);
        }

        let placed = match self.ports.apps.list_server_services(server).await {
            Ok(p) => p,
            Err(e) => {
                tracing::error!(%server, error = %e, "listing a node's services failed");
                return refuse(Denial::Internal);
            }
        };

        let wanted = match Self::wanted(&placed, req) {
            Ok(w) => w,
            Err(denial) => return self.refuse_quietly(server, denial, now),
        };

        let authority = match self.ports.authority.get_health_authority(wanted.app.id).await {
            Ok(a) => a,
            Err(e) => {
                tracing::error!(app = %wanted.app.name, error = %e, "reading who saved a health policy failed");
                return refuse(Denial::Internal);
            }
        };

        // The authority holds for the revision that person saved only.
        let saver = authority.filter(|a| a.revision == wanted.revision).map(|a| a.saved_by);
        let answer = match &saver {
            Some(saver) => self.decide(&wanted, saver).await,
            None => Err(Denial::NoAuthority),
        };

        // Audit first: no value leaves on an answer that was not recorded.
        if self.audit(server, &wanted, req, answer.as_ref().err().copied(), saver, now).await.is_err() {
            return refuse(Denial::Internal);
        }

        match answer {
            Ok(headers) => HealthSecretsResponse {
                granted: true,
                headers,
                ttl_secs: u32::try_from(GRANT_TTL.as_secs()).unwrap_or(300),
                denial: String::new(),
            },
            Err(denial) => refuse(denial),
        }
    }

    /// Counts the request against the node's minute.
    fn within_rate(&self, server: ServerId, now: Timestamp) -> bool {
        let minute = now.0.div_euclid(MINUTE_MS);
        let Ok(mut rates) = self.rates.lock() else { return false };
        if rates.len() >= MAX_REMEMBERED {
            rates.retain(|_, w| w.minute == minute);
        }

        admit(rates.entry(server).or_insert(Window { minute, count: 0 }), minute)
    }

    /// A refusal that is not audited: counted, and logged at most once a
    /// minute with the count since the last line.
    fn refuse_quietly(&self, server: ServerId, denial: Denial, now: Timestamp) -> HealthSecretsResponse {
        let minute = now.0.div_euclid(MINUTE_MS);

        if let Ok(mut u) = self.unaudited.lock() {
            u.count += 1;

            if u.logged_minute != minute {
                tracing::warn!(
                    count = u.count,
                    %server,
                    reason = denial.code(),
                    "health check vault requests refused without an audit entry since the last report (last one shown)"
                );
                *u = Unaudited { count: 0, logged_minute: minute };
            }
        }

        refuse(denial)
    }

    /// The stored check the request names, on a service of this node.
    fn wanted<'a>(placed: &'a [Service], req: &HealthSecretsRequest) -> Result<Wanted<'a>, Denial> {
        let app = placed
            .iter()
            .find(|a| {
                a.name == req.app
                    && a.spec.get("environment").and_then(serde_json::Value::as_str) == Some(&req.environment)
            })
            .ok_or(Denial::NotPlaced)?;
        let spec = crate::health_ingest::apply_spec(app).ok_or(Denial::NotPlaced)?;
        if spec.revision != req.policy_revision {
            return Err(Denial::StalePolicy);
        }

        let headers = references(&spec.policy, &req.purpose, &req.check).ok_or(Denial::NoSuchCheck)?;

        Ok(Wanted { app, revision: spec.revision, headers })
    }

    /// Checks the saver's authority now and opens the items.
    async fn decide(&self, w: &Wanted<'_>, saver: &Principal) -> Result<Vec<HealthSecretHeader>, Denial> {
        let p = &self.ports;
        let internal = |e: DomainError| {
            tracing::error!(app = %w.app.name, error = %e, "deciding on a health check's vault value failed");
            Denial::Internal
        };
        let Principal::User(uid) = saver;

        let user = match p.users.get_user(*uid).await {
            Ok(u) => u,
            Err(DomainError::NotFound(_)) => return Err(Denial::SaverGone),
            Err(e) => return Err(internal(e)),
        };

        if user.disabled {
            return Err(Denial::SaverDisabled);
        }

        // The vault is for platform administrators, as in the vault API.
        let bindings = p.bindings.list_bindings_for_user(*uid).await.map_err(internal)?;
        if !bindings.iter().any(|b| b.role == Role::PlatformAdmin) {
            return Err(Denial::NotAllowed);
        }

        // `secret.read` has an MFA guardrail. A probe has no session: the
        // save that named this person passed it with an MFA-verified
        // session, and what is checked again here is what they may do now.
        let workspace = EntityRef::new(EntityKind::Workspace, w.app.workspace_id);
        let ask = AccessRequest::new(saver.clone(), Action::SecretRead, workspace, AuthzContext { mfa: true });
        if !p.authz.is_allowed(&ask).await {
            return Err(Denial::NotAllowed);
        }

        let mut headers = Vec::with_capacity(w.headers.len());

        for (name, id) in &w.headers {
            let item = match p.vault.get_vault_item(*id).await.map_err(internal)? {
                Some(i) if i.workspace_id == w.app.workspace_id => i,
                _ => return Err(Denial::ItemMissing),
            };

            let plain = p.envelope.open(&item.sealed, &crate::jobs::storage::vault_aad(*id)).map_err(|_| {
                tracing::error!(app = %w.app.name, item = %id, "a vault item for a health check could not be opened");
                Denial::Internal
            })?;
            let value = String::from_utf8(plain).map_err(|_| Denial::ItemMissing)?;
            headers.push(HealthSecretHeader { name: name.clone(), value });
        }

        Ok(headers)
    }

    /// Writes the audit entry when the answer for this check on this node
    /// is new, changed, or a day old, and remembers it only once it is
    /// written. `Err` when an entry was due and could not be written.
    async fn audit(
        &self,
        server: ServerId,
        w: &Wanted<'_>,
        req: &HealthSecretsRequest,
        denial: Option<Denial>,
        saver: Option<Principal>,
        now: Timestamp,
    ) -> Result<(), ()> {
        if denial.is_some_and(|d| !d.audited()) {
            return Ok(());
        }

        let on_behalf_of = saver.as_ref().map(ToString::to_string);
        let entry = Audited {
            outcome: denial.map_or("granted", Denial::code),
            revision: w.revision,
            saver,
            day: now.0.div_euclid(DAY_MS),
        };
        let key: CheckKey = (server, w.app.id, format!("{}.{}", req.purpose, req.check));
        if self.audited.lock().map_err(|_| ())?.get(&key) == Some(&entry) {
            return Ok(());
        }

        let items: Vec<String> = w.headers.iter().map(|(_, id)| id.to_string()).collect();
        let details = serde_json::json!({
            "service": w.app.name,
            "purpose": req.purpose,
            "check": req.check,
            "policy_revision": w.revision,
            "on_behalf_of": on_behalf_of,
            "items": items,
            "reason": denial.map(Denial::code),
        });
        let outcome = if denial.is_some() { AuditOutcome::Denied } else { AuditOutcome::Success };
        let resource = EntityRef::new(EntityKind::Service, w.app.id).to_string();
        let event = AuditEvent::new(format!("agent:{server}"), AUDIT_SECRET_USE, resource, outcome)
            .in_org(Some(w.app.workspace_id))
            .with_details(details);

        if let Err(e) = self.ports.audit.record(event).await {
            tracing::error!(error = %e, "audit write failed; the vault value is not released");
            return Err(());
        }

        let mut seen = self.audited.lock().map_err(|_| ())?;
        if seen.len() >= MAX_REMEMBERED {
            // Forgetting costs one more audit entry per check, never a miss.
            seen.clear();
        }

        seen.insert(key, entry);

        Ok(())
    }

    /// Forgets what was audited for a service's checks (it was
    /// deleted, or is no longer placed anywhere).
    pub fn forget(&self, app: ServiceId) {
        if let Ok(mut seen) = self.audited.lock() {
            seen.retain(|(_, a, _), _| *a != app);
        }
    }
}

fn refuse(denial: Denial) -> HealthSecretsResponse {
    HealthSecretsResponse {
        granted: false,
        headers: Vec::new(),
        ttl_secs: u32::try_from(DENIAL_TTL.as_secs()).unwrap_or(60),
        denial: denial.code().to_owned(),
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    fn policy(v: serde_json::Value) -> HealthPolicy {
        serde_json::from_value(v).unwrap()
    }

    #[test]
    fn references_come_from_the_stored_check_in_order() {
        let a = VaultItemId::new();
        let b = VaultItemId::new();
        let p = policy(serde_json::json!({
            "readiness": [
                { "name": "api", "probe": { "type": "http", "port": 8080, "headers": [
                    { "name": "Authorization", "secret": format!("vault:{a}") },
                    { "name": "Accept", "value": "application/json" },
                    { "name": "X-Api-Key", "secret": format!("vault:{b}") }
                ] } },
                { "name": "plain", "probe": { "type": "http", "port": 8080 } },
                { "name": "off", "enabled": false, "probe": { "type": "http", "port": 8080, "headers": [
                    { "name": "Authorization", "secret": format!("vault:{a}") }
                ] } },
                { "name": "tcp", "probe": { "type": "tcp", "port": 8080 } }
            ]
        }));
        let refs = references(&p, "readiness", "api").unwrap();
        assert_eq!(refs, vec![("Authorization".to_owned(), a), ("X-Api-Key".to_owned(), b)]);

        for (purpose, name) in [
            ("readiness", "plain"),
            ("readiness", "off"),
            ("readiness", "tcp"),
            ("readiness", "nope"),
            ("liveness", "api"),
        ] {
            assert!(references(&p, purpose, name).is_none(), "{purpose}.{name}");
        }

        // A reference that is not an item id names nothing.
        let odd = policy(serde_json::json!({ "liveness": [{ "name": "x", "probe": { "type": "http", "port": 1,
            "headers": [{ "name": "Authorization", "secret": "vault:not-an-id" }] } }] }));
        assert!(references(&odd, "liveness", "x").is_none());
    }

    #[test]
    fn only_refusals_about_a_real_check_are_audited() {
        for d in [Denial::NotPlaced, Denial::StalePolicy, Denial::NoSuchCheck, Denial::RateLimited] {
            assert!(!d.audited(), "{d:?}");
        }

        for d in
            [Denial::NoAuthority, Denial::SaverGone, Denial::SaverDisabled, Denial::NotAllowed, Denial::ItemMissing]
        {
            assert!(d.audited(), "{d:?}");
        }

        let r = refuse(Denial::NotAllowed);
        assert!(!r.granted && r.headers.is_empty() && r.denial == "not_allowed" && r.ttl_secs == 60);
    }

    #[test]
    fn a_node_gets_a_bounded_number_of_answers_per_minute() {
        let mut w = Window { minute: 7, count: 0 };

        for _ in 0..RATE_LIMIT {
            assert!(admit(&mut w, 7));
        }

        assert!(!admit(&mut w, 7) && !admit(&mut w, 7), "over the limit in the same minute");
        assert!(admit(&mut w, 8), "the next minute starts over");
        assert_eq!(w.count, 1);
    }
}
