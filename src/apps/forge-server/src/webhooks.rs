//! Deploy webhooks (D-159): creation, rotation, signature checks and rate
//! limits. The URL token is shown once and stored as its SHA-256; the
//! signing secret is shown once and stored sealed. A delivery runs as the
//! webhook's creator (see `api::hooks`), never with more rights.

use std::sync::Arc;
use std::time::Duration;

use forge_core::webhooks::{Signature, Webhook, WebhookRepository, WebhookTarget, tag_ok};
use forge_core::{Clock, DomainError, DomainResult, Principal, WebhookId, WorkspaceId};
use forge_secrets::Envelope;
use forge_secrets::crypto::{constant_time_eq, random_token, sha256_hex};

/// Token prefix: tells a Forge webhook URL apart in logs and secret scanners.
pub const TOKEN_PREFIX: &str = "whk_";
/// How far a `forge` signature's timestamp may be from now.
const SIGNATURE_WINDOW_SECS: i64 = 300;

/// Deliveries per webhook and per source address in one window.
#[derive(Debug, Clone, Copy)]
pub struct Limits {
    pub per_hook: usize,
    pub per_ip: usize,
    pub window: Duration,
}

impl Default for Limits {
    fn default() -> Self {
        Self { per_hook: 10, per_ip: 60, window: Duration::from_secs(60) }
    }
}

pub use crate::rate_limit::Windows;

/// What a webhook is created or changed with.
#[derive(Debug, Clone)]
pub struct WebhookInput {
    pub name: String,
    pub target: WebhookTarget,
    pub signature: Signature,
    pub tag_pattern: Option<String>,
    pub enabled: bool,
}

/// What only the creator sees, once.
#[derive(Debug, Clone, serde::Serialize)]
pub struct Minted {
    pub webhook: Webhook,
    /// The full URL path token.
    pub token: String,
    /// The signing secret (when the signature needs one).
    pub secret: Option<String>,
}

/// Why a delivery is refused.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum Rejection {
    #[error("the webhook is disabled")]
    Disabled,
    #[error("the signature is missing or wrong")]
    BadSignature,
    #[error("the timestamp is missing or too old")]
    Stale,
    #[error("this webhook does not let a delivery choose the tag")]
    TagNotAllowed,
    #[error("the tag does not match the webhook's pattern")]
    TagRefused,
    #[error("this delivery was already received")]
    Replayed,
}

/// How long a delivery id or signature is remembered against replays (it
/// covers the `forge` timestamp window twice).
const REPLAY_WINDOW: Duration = Duration::from_secs(600);

#[derive(Debug)]
pub struct WebhookService {
    pub repo: Arc<dyn WebhookRepository>,
    pub jobs: Arc<dyn forge_core::ports::JobRepository>,
    pub services: Arc<dyn forge_core::ports::ServiceRepository>,
    pub envelope: Envelope,
    pub clock: Arc<dyn Clock>,
    pub limits: Limits,
    pub per_hook: Windows<WebhookId>,
    pub per_ip: Windows<std::net::IpAddr>,
    /// Delivery ids and signatures seen recently, per webhook.
    pub seen: Windows<(WebhookId, String)>,
}

fn aad(id: WebhookId) -> String {
    format!("webhook:{id}")
}

fn invalid(m: &str) -> DomainError {
    DomainError::Invalid(m.into())
}

fn check(input: &WebhookInput) -> DomainResult<()> {
    let name = input.name.trim();
    if name.is_empty() || name.len() > 100 || name.chars().any(char::is_control) {
        return Err(invalid("the name is 1 to 100 characters"));
    }
    if let Some(p) = &input.tag_pattern {
        if p.is_empty() || p.len() > 128 || !tag_ok(&p.replace('*', "x")) {
            return Err(invalid("the tag pattern is a tag with `*` wildcards, e.g. v*"));
        }
        // Choosing what to deploy needs more than a URL that may leak.
        if input.signature == Signature::None {
            return Err(invalid("a webhook that may change the tag needs a signature"));
        }
    }
    Ok(())
}

/// HMAC-SHA256 of `data` with `key`, hex.
fn hmac_hex(key: &[u8], data: &[u8]) -> String {
    let k = ring::hmac::Key::new(ring::hmac::HMAC_SHA256, key);
    hex::encode(ring::hmac::sign(&k, data).as_ref())
}

/// The headers a signature check reads.
#[derive(Debug, Default, Clone)]
pub struct SignatureHeaders<'a> {
    pub hub_signature_256: Option<&'a str>,
    /// `X-GitHub-Delivery`: GitHub's id of this delivery.
    pub github_delivery: Option<&'a str>,
    pub gitlab_token: Option<&'a str>,
    /// `X-Gitlab-Event-UUID`: GitLab's id of this delivery.
    pub gitlab_event: Option<&'a str>,
    pub forge_signature: Option<&'a str>,
    pub forge_timestamp: Option<&'a str>,
}

impl WebhookService {
    fn mint_token() -> (String, String, String) {
        let token = format!("{TOKEN_PREFIX}{}", random_token::<32>());
        let hash = sha256_hex(token.as_bytes());
        let hint = token.chars().take(TOKEN_PREFIX.len() + 4).collect();
        (token, hash, hint)
    }

    fn mint_secret(&self, id: WebhookId, signature: Signature) -> DomainResult<(Option<String>, Option<String>)> {
        if signature == Signature::None {
            return Ok((None, None));
        }
        let secret = random_token::<32>();
        let sealed = self.envelope.seal(secret.as_bytes(), &aad(id)).map_err(DomainError::internal)?;
        Ok((Some(secret), Some(sealed)))
    }

    pub async fn create(&self, ws: WorkspaceId, by: &Principal, input: WebhookInput) -> DomainResult<Minted> {
        check(&input)?;
        let id = WebhookId::new();
        let (token, token_hash, token_hint) = Self::mint_token();
        let (secret, secret_sealed) = self.mint_secret(id, input.signature)?;
        let now = self.clock.now();
        let webhook = Webhook {
            id,
            workspace_id: ws,
            name: input.name.trim().to_owned(),
            target: input.target,
            token_hash,
            token_hint,
            signature: input.signature,
            secret_sealed,
            tag_pattern: input.tag_pattern,
            enabled: input.enabled,
            created_by: by.clone(),
            created_at: now,
            updated_at: now,
            last_delivery_at: None,
        };
        self.repo.create_webhook(&webhook).await?;
        Ok(Minted { webhook, token, secret })
    }

    /// A new token (and secret): the old URL stops working at once. The
    /// webhook now acts as `by`, who holds the new URL.
    pub async fn rotate(&self, id: WebhookId, by: &Principal) -> DomainResult<Minted> {
        let mut w = self.get(id).await?;
        w.created_by = by.clone();
        let (token, token_hash, token_hint) = Self::mint_token();
        let (secret, secret_sealed) = self.mint_secret(id, w.signature)?;
        w.token_hash = token_hash;
        w.token_hint = token_hint;
        w.secret_sealed = secret_sealed;
        w.updated_at = self.clock.now();
        self.repo.update_webhook(&w).await?;
        Ok(Minted { webhook: w, token, secret })
    }

    /// Changes name, tag pattern and enabled; the target and the signature
    /// kind stay (create a new webhook for another).
    /// The webhook then acts as `by`: whoever changes what it may deploy
    /// answers for it.
    pub async fn update(&self, id: WebhookId, by: &Principal, input: WebhookInput) -> DomainResult<Webhook> {
        check(&input)?;
        let mut w = self.get(id).await?;
        w.created_by = by.clone();
        if input.target != w.target || input.signature != w.signature {
            return Err(invalid("the target and the signature cannot change; create a new webhook"));
        }
        w.name = input.name.trim().to_owned();
        w.tag_pattern = input.tag_pattern;
        w.enabled = input.enabled;
        w.updated_at = self.clock.now();
        self.repo.update_webhook(&w).await?;
        Ok(w)
    }

    pub async fn get(&self, id: WebhookId) -> DomainResult<Webhook> {
        self.repo.get_webhook(id).await?.ok_or_else(|| DomainError::NotFound("webhook not found".into()))
    }

    pub async fn delete(&self, id: WebhookId) -> DomainResult<()> {
        self.repo.delete_webhook(id).await
    }

    /// The webhook of a URL token (looked up by its hash).
    pub async fn by_token(&self, token: &str) -> DomainResult<Option<Webhook>> {
        if !token.starts_with(TOKEN_PREFIX) || token.len() > 128 {
            return Ok(None);
        }
        self.repo.webhook_by_token_hash(&sha256_hex(token.as_bytes())).await
    }

    /// Points `app`'s image at `tag` (a delivery that chose one).
    pub async fn set_tag(
        &self,
        app: &forge_core::model::Service,
        tag: &str,
    ) -> DomainResult<forge_core::model::Service> {
        let image = app.spec.get("image").and_then(serde_json::Value::as_str).unwrap_or_default();
        if image.is_empty() {
            return Err(invalid("the service has no image to retag"));
        }
        let mut next = app.clone();
        next.spec["image"] = serde_json::Value::String(retag(image, tag));
        next.updated_at = self.clock.now();
        self.services.put_service(&next).await?;
        Ok(next)
    }

    /// The status of a job this webhook started (nothing else is shown).
    pub async fn job_status(&self, w: &Webhook, job: forge_core::JobId) -> DomainResult<serde_json::Value> {
        let started = self
            .repo
            .list_deliveries(w.id, forge_core::webhooks::DELIVERIES_KEPT)
            .await?
            .iter()
            .any(|d| d.job_id == Some(job));
        if !started {
            return Err(DomainError::NotFound("no such job for this webhook".into()));
        }
        let j = self.jobs.get_job(job).await?;
        // Status only: error text can name nodes and paths (the console shows it).
        Ok(serde_json::json!({ "id": j.id, "kind": j.kind, "status": j.status }))
    }

    /// Rate limits: per source address first (it also covers bad tokens),
    /// then per webhook.
    pub fn allow_ip(&self, ip: std::net::IpAddr) -> bool {
        self.per_ip.allow(&ip, self.limits.per_ip, self.limits.window)
    }

    pub fn allow_hook(&self, id: WebhookId) -> bool {
        self.per_hook.allow(&id, self.limits.per_hook, self.limits.window)
    }

    /// Checks a delivery's signature and tag. `Ok(tag)` is the tag to
    /// deploy, if the delivery chose one.
    pub fn verify(
        &self,
        w: &Webhook,
        headers: &SignatureHeaders<'_>,
        body: &[u8],
        tag: Option<&str>,
    ) -> Result<Option<String>, Rejection> {
        if !w.enabled {
            return Err(Rejection::Disabled);
        }
        if w.signature != Signature::None {
            let sealed = w.secret_sealed.as_deref().ok_or(Rejection::BadSignature)?;
            let secret = self.envelope.open(sealed, &aad(w.id)).map_err(|_| Rejection::BadSignature)?;
            let ok = match w.signature {
                Signature::None => true,
                Signature::Github => headers
                    .hub_signature_256
                    .and_then(|h| h.strip_prefix("sha256="))
                    .is_some_and(|sig| constant_time_eq(sig.as_bytes(), hmac_hex(&secret, body).as_bytes())),
                Signature::Gitlab => headers.gitlab_token.is_some_and(|t| constant_time_eq(t.as_bytes(), &secret)),
                Signature::Forge => {
                    let ts: i64 = headers.forge_timestamp.and_then(|t| t.parse().ok()).ok_or(Rejection::Stale)?;
                    let now = self.clock.now().0 / 1000;
                    if (now - ts).abs() > SIGNATURE_WINDOW_SECS {
                        return Err(Rejection::Stale);
                    }
                    let mut signed = format!("{ts}.").into_bytes();
                    signed.extend_from_slice(body);
                    headers
                        .forge_signature
                        .and_then(|h| h.strip_prefix("sha256="))
                        .is_some_and(|sig| constant_time_eq(sig.as_bytes(), hmac_hex(&secret, &signed).as_bytes()))
                }
            };
            if !ok {
                return Err(Rejection::BadSignature);
            }
            // The same signed delivery is accepted once: CI ids, else the
            // signature (which covers the timestamp for `forge`).
            let marker = match w.signature {
                Signature::Github => headers.github_delivery.or(headers.hub_signature_256),
                Signature::Gitlab => headers.gitlab_event,
                Signature::Forge => headers.forge_signature,
                Signature::None => None,
            };
            if let Some(m) = marker
                && !self.seen.allow(&(w.id, m.chars().take(200).collect()), 1, REPLAY_WINDOW)
            {
                return Err(Rejection::Replayed);
            }
        }
        match (tag, &w.tag_pattern) {
            (None, _) => Ok(None),
            (Some(_), None) => Err(Rejection::TagNotAllowed),
            (Some(t), Some(p)) => {
                if tag_ok(t) && forge_core::webhooks::tag_matches(p, t) {
                    Ok(Some(t.to_owned()))
                } else {
                    Err(Rejection::TagRefused)
                }
            }
        }
    }
}

/// `repo[:tag]` or `repo@digest` with its tag replaced by `tag`. A `:` in a
/// registry host (before the last `/`) is not a tag.
#[must_use]
pub fn retag(image: &str, tag: &str) -> String {
    let base = image.split_once('@').map_or(image, |(b, _)| b);
    let repo = match base.rsplit_once(':') {
        Some((r, t)) if !t.contains('/') => r,
        _ => base,
    };
    format!("{repo}:{tag}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn retag_keeps_the_repository() {
        assert_eq!(retag("nginx:1.27", "1.28"), "nginx:1.28");
        assert_eq!(retag("ghcr.io/acme/app", "v2"), "ghcr.io/acme/app:v2");
        assert_eq!(retag("registry:5000/team/app:v1", "v2"), "registry:5000/team/app:v2");
        assert_eq!(retag("app@sha256:abc", "v3"), "app:v3");
    }

    #[test]
    fn hmac_matches_a_known_vector() {
        // RFC 4231 test case 2.
        assert_eq!(
            hmac_hex(b"Jefe", b"what do ya want for nothing?"),
            "5bdcc146bf60754e6a042426089575c75a003f089d2739839dec58b964ec3843"
        );
    }
}
