//! Vault-backed header values of HTTP health checks (issue #12, D-111).
//!
//! A policy names a credential header by reference (`vault:<id>`); the
//! value is never in the policy sent to the node and never written to its
//! disk. When a check with such headers is due, the agent asks Forge for
//! the values of that one check over its own mTLS link
//! ([`forge_proto::paths::HEALTH_SECRETS`]). Forge decides each time (the
//! policy still names the item; the person who last saved the policy may
//! still use it) and answers with the values and how long they may be
//! reused. They are kept here in memory for that long, per check and policy
//! revision, and nowhere else: not in logs, reports or `Debug` output.
//!
//! A refusal is kept for a short time too, so a refused check does not ask
//! again every period. Either way the check reports `unknown` and no
//! request goes to its target.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use forge_proto::messages::{
    HealthSecretsRequest, HealthSecretsResponse, MAX_HEALTH_SECRET_HEADERS, health_secret_denial as denial,
};
use prost::Message;
use rustls::ClientConfig;

use crate::health_probe::Outcome;
use crate::link::{Endpoint, Link};

/// Longest reuse of granted values, whatever Forge allows.
const MAX_TTL: Duration = Duration::from_secs(600);
/// Longest reuse of a refusal.
const MAX_DENIAL_TTL: Duration = Duration::from_secs(60);
/// Shortest reuse of any answer.
const MIN_TTL: Duration = Duration::from_secs(5);
/// How long an unanswered request is not repeated.
const RETRY: Duration = Duration::from_secs(15);
/// How long one request to Forge may take.
const REQUEST_TIMEOUT: Duration = Duration::from_secs(10);

/// One header value from the vault. `Debug` never prints it.
#[derive(Clone, PartialEq, Eq)]
pub struct ResolvedHeader {
    pub name: String,
    value: String,
}

impl ResolvedHeader {
    #[must_use]
    pub fn new(name: impl Into<String>, value: impl Into<String>) -> Self {
        Self { name: name.into(), value: value.into() }
    }

    #[must_use]
    pub fn value(&self) -> &str {
        &self.value
    }
}

impl std::fmt::Debug for ResolvedHeader {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ResolvedHeader").field("name", &self.name).field("value", &"<redacted>").finish()
    }
}

impl Drop for ResolvedHeader {
    fn drop(&mut self) {
        // Best effort: do not leave the value in freed memory.
        let mut bytes = std::mem::take(&mut self.value).into_bytes();
        bytes.fill(0);
    }
}

/// Where Forge is and how this agent proves who it is.
#[derive(Clone)]
struct Control {
    endpoint: Endpoint,
    tls: Arc<ClientConfig>,
}

/// (environment, app, purpose, check).
pub type Key = (String, String, String, String);

/// Why a check gets no values: the reason code and sentence it reports.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Denied {
    reason: &'static str,
    detail: &'static str,
}

struct Entry {
    revision: i64,
    until: Instant,
    answer: Result<Arc<Vec<ResolvedHeader>>, Denied>,
}

/// The values the agent currently holds, and its way to ask for more.
#[derive(Clone, Default)]
pub struct Secrets {
    control: Arc<Mutex<Option<Control>>>,
    cache: Arc<Mutex<HashMap<Key, Entry>>>,
}

impl std::fmt::Debug for Secrets {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let held = self.cache.lock().map_or(0, |c| c.len());
        f.debug_struct("Secrets").field("held", &held).finish_non_exhaustive()
    }
}

/// What a refusal code from Forge means for the check.
fn denied(code: &str) -> Denied {
    match code {
        denial::NOT_ALLOWED | denial::SAVER_GONE | denial::SAVER_DISABLED | denial::NO_AUTHORITY => Denied {
            reason: "permission_denied",
            detail: "Forge refused the vault value: the person who last saved this policy may no longer use it; \
                     save the policy again",
        },
        denial::ITEM_MISSING => {
            Denied { reason: "invalid_config", detail: "a vault item this check refers to no longer exists" }
        }
        denial::STALE_POLICY => Denied {
            reason: "invalid_config",
            detail: "this node runs another policy revision than Forge holds; waiting for the current one",
        },
        denial::NOT_PLACED | denial::NO_SUCH_CHECK => {
            Denied { reason: "invalid_config", detail: "Forge has no such check for a service on this node" }
        }
        _ => Denied { reason: "location_unavailable", detail: "Forge could not hand out the vault value" },
    }
}

const NO_ANSWER: Denied =
    Denied { reason: "location_unavailable", detail: "Forge did not answer the request for the vault value" };

/// The cache entry for Forge's answer (or its absence) and how long it holds.
fn entry_of(answer: Option<HealthSecretsResponse>, revision: i64, now: Instant) -> Entry {
    let Some(resp) = answer else { return Entry { revision, until: now + RETRY, answer: Err(NO_ANSWER) } };
    let ttl = Duration::from_secs(u64::from(resp.ttl_secs)).max(MIN_TTL);

    if resp.granted && resp.headers.len() <= MAX_HEALTH_SECRET_HEADERS {
        let headers = resp.headers.into_iter().map(|h| ResolvedHeader::new(h.name, h.value)).collect();
        Entry { revision, until: now + ttl.min(MAX_TTL), answer: Ok(Arc::new(headers)) }
    } else {
        Entry { revision, until: now + ttl.min(MAX_DENIAL_TTL), answer: Err(denied(&resp.denial)) }
    }
}

impl Secrets {
    /// Tells where Forge is; called with every heartbeat, so a renewed
    /// certificate is picked up.
    pub fn set_control(&self, endpoint: &Endpoint, tls: Arc<ClientConfig>) {
        if let Ok(mut c) = self.control.lock() {
            *c = Some(Control { endpoint: endpoint.clone(), tls });
        }
    }

    /// Drops every answer whose time is up, used again or not: a value
    /// does not stay in memory because its check stopped running.
    pub fn purge(&self, now: Instant) {
        if let Ok(mut c) = self.cache.lock() {
            c.retain(|_, e| now < e.until);
        }
    }

    /// Drops what is held for checks that are gone or disabled.
    pub fn retain(&self, keep: impl Fn(&Key) -> bool) {
        if let Ok(mut c) = self.cache.lock() {
            c.retain(|k, _| keep(k));
        }
    }

    fn held(&self, key: &Key, revision: i64, now: Instant) -> Option<Result<Arc<Vec<ResolvedHeader>>, Denied>> {
        let c = self.cache.lock().ok()?;
        c.get(key).filter(|e| e.revision == revision && now < e.until).map(|e| e.answer.clone())
    }

    /// The vault-backed header values of the check `req` names: from
    /// memory while Forge's last answer holds, else from Forge. `Err` is
    /// what the check reports instead of running.
    pub async fn resolve(&self, req: HealthSecretsRequest) -> Result<Arc<Vec<ResolvedHeader>>, Outcome> {
        let key: Key = (req.environment.clone(), req.app.clone(), req.purpose.clone(), req.check.clone());

        // Every refusal here passes on its own or with a new save: transient.
        let unknown = |d: Denied| Outcome::unknown(crate::health_probe::Unknown::Transient, d.reason, d.detail);

        if let Some(answer) = self.held(&key, req.policy_revision, Instant::now()) {
            return answer.map_err(unknown);
        }

        let control = self.control.lock().ok().and_then(|c| c.clone());
        let answer = match control {
            Some(c) => fetch(c, &req).await,
            None => None,
        };

        let entry = entry_of(answer, req.policy_revision, Instant::now());
        let answer = entry.answer.clone();

        if let Ok(mut c) = self.cache.lock() {
            c.insert(key, entry);
        }

        answer.map_err(unknown)
    }
}

/// Asks Forge over a connection of its own (the agent's link is held by the
/// long-poll). `None` when there is no usable answer; the error is logged
/// without the request's or the answer's content.
async fn fetch(control: Control, req: &HealthSecretsRequest) -> Option<HealthSecretsResponse> {
    let mut link = Link::new(control.endpoint, control.tls, REQUEST_TIMEOUT);
    match link.request(http::Method::POST, forge_proto::paths::HEALTH_SECRETS, req.encode_to_vec()).await {
        Ok(body) => HealthSecretsResponse::decode(body.as_slice()).ok(),
        Err(e) => {
            // The server's error text for this path never contains a value.
            tracing::debug!(app = %req.app, check = %req.check, error = %e, "asking Forge for a vault header failed");
            None
        }
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use forge_proto::messages::HealthSecretHeader;

    use super::*;

    fn req(revision: i64) -> HealthSecretsRequest {
        HealthSecretsRequest {
            environment: "uat".into(),
            app: "api".into(),
            purpose: "readiness".into(),
            check: "http".into(),
            policy_revision: revision,
        }
    }

    fn granted(ttl_secs: u32) -> HealthSecretsResponse {
        HealthSecretsResponse {
            granted: true,
            headers: vec![HealthSecretHeader { name: "Authorization".into(), value: "Bearer s3cret".into() }],
            ttl_secs,
            denial: String::new(),
        }
    }

    fn key() -> Key {
        ("uat".into(), "api".into(), "readiness".into(), "http".into())
    }

    #[test]
    fn values_never_appear_in_debug_output() {
        let h = ResolvedHeader::new("Authorization", "Bearer s3cret");
        let s = Secrets::default();
        s.cache.lock().unwrap().insert(key(), entry_of(Some(granted(300)), 1, Instant::now()));

        for text in [format!("{h:?}"), format!("{s:?}"), format!("{:?}", granted(300))] {
            assert!(!text.contains("s3cret"), "{text}");
        }

        assert_eq!(h.value(), "Bearer s3cret");
    }

    #[test]
    fn answers_are_reused_only_for_their_revision_and_lifetime() {
        let s = Secrets::default();
        let now = Instant::now();
        s.cache.lock().unwrap().insert(key(), entry_of(Some(granted(300)), 4, now));
        assert!(s.held(&key(), 4, now).unwrap().is_ok());
        assert!(s.held(&key(), 5, now).is_none(), "another policy revision asks again");
        assert!(s.held(&key(), 4, now + Duration::from_secs(301)).is_none(), "expired");

        // Forge cannot make the node keep a value for long.
        s.cache.lock().unwrap().insert(key(), entry_of(Some(granted(86_400)), 4, now));
        assert!(s.held(&key(), 4, now + MAX_TTL + Duration::from_secs(1)).is_none());

        // Removed checks are forgotten.
        s.retain(|_| false);
        assert!(s.held(&key(), 4, now).is_none());

        // Time alone removes a value too, whether or not anyone asks again.
        s.cache.lock().unwrap().insert(key(), entry_of(Some(granted(300)), 4, now));
        s.purge(now + Duration::from_secs(299));
        assert_eq!(s.cache.lock().unwrap().len(), 1);
        s.purge(now + Duration::from_secs(300));
        assert!(s.cache.lock().unwrap().is_empty(), "expired values leave memory");
    }

    #[test]
    fn refusals_and_silence_become_unknown_with_a_reason() {
        let now = Instant::now();
        let refused = |code: &str| {
            let resp =
                HealthSecretsResponse { granted: false, headers: Vec::new(), ttl_secs: 3600, denial: code.into() };
            let e = entry_of(Some(resp), 1, now);
            assert!(e.until <= now + MAX_DENIAL_TTL, "a refusal is not kept for long");
            e.answer.unwrap_err()
        };
        assert_eq!(refused("not_allowed").reason, "permission_denied");
        assert_eq!(refused("saver_disabled").reason, "permission_denied");
        assert_eq!(refused("item_missing").reason, "invalid_config");
        assert_eq!(refused("stale_policy").reason, "invalid_config");
        assert_eq!(refused("something-new").reason, "location_unavailable");
        let silent = entry_of(None, 1, now);
        assert_eq!((silent.answer.unwrap_err(), silent.until), (NO_ANSWER, now + RETRY));

        // More headers than a check can have is not an answer to trust.
        let mut many = granted(300);
        many.headers = vec![many.headers[0].clone(); MAX_HEALTH_SECRET_HEADERS + 1];
        assert!(entry_of(Some(many), 1, now).answer.is_err());
    }

    #[tokio::test]
    async fn without_a_connection_the_check_is_unknown_and_nothing_is_sent() {
        let s = Secrets::default();
        let o = s.resolve(req(1)).await.unwrap_err();
        assert_eq!((o.verdict, o.reason), (crate::health_probe::Verdict::Unknown, "location_unavailable"));

        // The miss is remembered briefly instead of asked again every period.
        assert!(s.held(&key(), 1, Instant::now()).is_some_and(|a| a.is_err()));
    }
}
