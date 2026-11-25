//! Deploy webhooks (D-159): a secret URL CI calls to deploy a service or a
//! compose stack. The URL token and the signing secret are stored as a hash
//! and sealed respectively; a delivery runs as the webhook's creator, whose
//! permissions are checked again every time.

use async_trait::async_trait;
use serde::{Deserialize, Serialize};

use crate::access::Principal;
use crate::clock::Timestamp;
use crate::error::{DomainError, DomainResult};
use crate::ids::{JobId, ServiceId, StackId, WebhookDeliveryId, WebhookId, WorkspaceId};

/// What a webhook deploys.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum WebhookTarget {
    Service { id: ServiceId },
    Stack { id: StackId },
}

/// How a delivery proves it comes from CI, besides the secret URL.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Signature {
    /// The URL alone.
    #[default]
    None,
    /// GitHub: `X-Hub-Signature-256: sha256=<hmac of the body>`.
    Github,
    /// GitLab: `X-Gitlab-Token: <secret>`.
    Gitlab,
    /// `X-Forge-Timestamp: <unix seconds>` and `X-Forge-Signature:
    /// sha256=<hmac of "<timestamp>.<body>">`, within five minutes.
    Forge,
}

impl Signature {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::None => "none",
            Self::Github => "github",
            Self::Gitlab => "gitlab",
            Self::Forge => "forge",
        }
    }
}

impl std::str::FromStr for Signature {
    type Err = DomainError;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "none" => Ok(Self::None),
            "github" => Ok(Self::Github),
            "gitlab" => Ok(Self::Gitlab),
            "forge" => Ok(Self::Forge),
            other => Err(DomainError::internal(format!("unknown webhook signature {other:?}"))),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Webhook {
    pub id: WebhookId,
    pub workspace_id: WorkspaceId,
    pub name: String,
    pub target: WebhookTarget,
    /// SHA-256 of the URL token. Never serialized.
    #[serde(skip)]
    pub token_hash: String,
    /// The first characters of the token, to tell URLs apart.
    pub token_hint: String,
    pub signature: Signature,
    /// The signing secret, sealed. Never serialized.
    #[serde(skip)]
    pub secret_sealed: Option<String>,
    /// Image tags a delivery may deploy (`*` matches any characters);
    /// `None`: the tag cannot be changed by a delivery.
    pub tag_pattern: Option<String>,
    pub enabled: bool,
    /// Whose permissions a delivery runs with.
    pub created_by: Principal,
    pub created_at: Timestamp,
    pub updated_at: Timestamp,
    pub last_delivery_at: Option<Timestamp>,
}

/// What happened to a delivery.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DeliveryOutcome {
    /// A deployment started.
    Accepted,
    /// Bad signature, tag, or a disabled hook.
    Rejected,
    RateLimited,
    /// The deployment could not start (permissions, state).
    Failed,
}

impl DeliveryOutcome {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Accepted => "accepted",
            Self::Rejected => "rejected",
            Self::RateLimited => "rate_limited",
            Self::Failed => "failed",
        }
    }
}

impl std::str::FromStr for DeliveryOutcome {
    type Err = DomainError;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "accepted" => Ok(Self::Accepted),
            "rejected" => Ok(Self::Rejected),
            "rate_limited" => Ok(Self::RateLimited),
            "failed" => Ok(Self::Failed),
            other => Err(DomainError::internal(format!("unknown delivery outcome {other:?}"))),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Delivery {
    pub id: WebhookDeliveryId,
    pub webhook_id: WebhookId,
    pub at: Timestamp,
    pub outcome: DeliveryOutcome,
    pub reason: String,
    pub job_id: Option<JobId>,
    /// The tag a delivery asked for, if any.
    pub tag: Option<String>,
    pub source_ip: String,
}

/// Deliveries kept per webhook.
pub const DELIVERIES_KEPT: u32 = 100;

#[async_trait]
pub trait WebhookRepository: Send + Sync + std::fmt::Debug {
    async fn list_webhooks(&self, ws: WorkspaceId) -> DomainResult<Vec<Webhook>>;
    async fn get_webhook(&self, id: WebhookId) -> DomainResult<Option<Webhook>>;
    async fn webhook_by_token_hash(&self, hash: &str) -> DomainResult<Option<Webhook>>;
    async fn create_webhook(&self, w: &Webhook) -> DomainResult<()>;
    async fn update_webhook(&self, w: &Webhook) -> DomainResult<()>;
    async fn delete_webhook(&self, id: WebhookId) -> DomainResult<()>;
    /// Records a delivery and keeps the newest [`DELIVERIES_KEPT`].
    async fn add_delivery(&self, d: &Delivery) -> DomainResult<()>;
    async fn list_deliveries(&self, id: WebhookId, limit: u32) -> DomainResult<Vec<Delivery>>;
    /// Remembers a delivery's replay marker (D-168). `Ok(true)` when it is
    /// new, `Ok(false)` when this webhook already saw it. Markers older than
    /// `forget_before` are dropped in the same call, so the table stays
    /// bounded; the check is atomic (a unique key), so two servers or two
    /// concurrent deliveries cannot both claim one marker.
    async fn claim_replay_marker(
        &self,
        id: WebhookId,
        marker: &str,
        at: Timestamp,
        forget_before: Timestamp,
    ) -> DomainResult<bool>;
}

/// Whether `tag` matches `pattern` (`*` matches any run of characters).
#[must_use]
pub fn tag_matches(pattern: &str, tag: &str) -> bool {
    let parts: Vec<&str> = pattern.split('*').collect();
    let (first, last) = (parts[0], parts[parts.len() - 1]);
    if parts.len() == 1 {
        return pattern == tag;
    }
    if !tag.starts_with(first) || !tag[first.len()..].ends_with(last) || tag.len() < first.len() + last.len() {
        return false;
    }
    let mut rest = &tag[first.len()..tag.len() - last.len()];
    for mid in &parts[1..parts.len() - 1] {
        match rest.find(mid) {
            Some(i) => rest = &rest[i + mid.len()..],
            None => return false,
        }
    }
    true
}

/// A Docker image tag: letters, digits, `_`, `.`, `-`, at most 128.
#[must_use]
pub fn tag_ok(tag: &str) -> bool {
    (1..=128).contains(&tag.len())
        && tag.bytes().next().is_some_and(|b| b.is_ascii_alphanumeric() || b == b'_')
        && tag.bytes().all(|b| b.is_ascii_alphanumeric() || matches!(b, b'_' | b'.' | b'-'))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tag_patterns() {
        assert!(tag_matches("v*", "v1.2.3"));
        assert!(tag_matches("*", "anything"));
        assert!(tag_matches("release-*-prod", "release-42-prod"));
        assert!(!tag_matches("release-*-prod", "release-42-dev"));
        assert!(!tag_matches("v*", "latest"));
        assert!(tag_matches("main", "main"));
        assert!(!tag_matches("ab*ba", "aba"), "the ends may not overlap");
    }

    #[test]
    fn tags() {
        assert!(tag_ok("v1.2.3"));
        assert!(tag_ok("sha-0123abc"));
        assert!(!tag_ok("../x"));
        assert!(!tag_ok("a b"));
        assert!(!tag_ok(""));
        assert!(!tag_ok("-x"));
    }
}
