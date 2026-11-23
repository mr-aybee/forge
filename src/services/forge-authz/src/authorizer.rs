use std::sync::Arc;

use async_trait::async_trait;
use forge_core::ports::{Authorizer, ResourceResolver};
use forge_core::{AccessRequest, DomainError, DomainResult, EntityRef, ResourceRef};

use crate::engine::{AuthzError, PolicyEngine};

/// Policy id of the step-up MFA guardrail (`policies/guardrails.cedar`).
const MFA_GUARD: &str = "guard:sensitive-requires-mfa";
/// Prefix of denials caused only by a missing MFA step-up.
pub const MFA_REQUIRED_PREFIX: &str = "mfa_required:";

/// [`Authorizer`] backed by Cedar. Resolves the resource's ancestry through
/// the injected [`ResourceResolver`], then evaluates.
#[derive(Debug, Clone)]
pub struct CedarAuthorizer {
    engine: Arc<PolicyEngine>,
    resolver: Arc<dyn ResourceResolver>,
}

impl CedarAuthorizer {
    #[must_use]
    pub fn new(engine: Arc<PolicyEngine>, resolver: Arc<dyn ResourceResolver>) -> Self {
        Self { engine, resolver }
    }

    #[must_use]
    pub fn engine(&self) -> &Arc<PolicyEngine> {
        &self.engine
    }
}

#[async_trait]
impl Authorizer for CedarAuthorizer {
    async fn authorize(&self, request: &AccessRequest) -> DomainResult<()> {
        let resource = self.resolver.resolve(&request.resource).await?;
        self.decide(request, &resource)
    }

    async fn is_allowed_many(&self, requests: &[AccessRequest]) -> Vec<bool> {
        let entities: Vec<EntityRef> = requests.iter().map(|r| r.resource.clone()).collect();
        let resolved = self.resolver.resolve_many(&entities).await;
        let uniform = requests
            .windows(2)
            .all(|w| w[0].principal == w[1].principal && w[0].action == w[1].action && w[0].context == w[1].context);
        if uniform
            && let Some(first) = requests.first()
            && resolved.iter().all(Result::is_ok)
        {
            let resources: Vec<ResourceRef> = resolved.into_iter().filter_map(Result::ok).collect();
            if let Ok(d) = self.engine.evaluate_many(&first.principal, first.action, &resources, first.context) {
                return d.into_iter().map(|d| d.allowed).collect();
            }
            return vec![false; requests.len()];
        }
        requests.iter().zip(resolved).map(|(r, res)| res.and_then(|res| self.decide(r, &res)).is_ok()).collect()
    }

    async fn holds_over(&self, requests: &[AccessRequest]) -> Vec<bool> {
        let entities: Vec<EntityRef> = requests.iter().map(|r| r.resource.clone()).collect();
        let resolved = self.resolver.resolve_many(&entities).await;
        requests
            .iter()
            .zip(resolved)
            .map(|(r, res)| {
                res.ok().is_some_and(|res| {
                    self.engine.holds_over(&r.principal, r.action, &res, r.context).is_ok_and(|d| d.allowed)
                })
            })
            .collect()
    }
}

impl CedarAuthorizer {
    /// The decision for an already-resolved resource.
    fn decide(&self, request: &AccessRequest, resource: &ResourceRef) -> DomainResult<()> {
        let resource = resource.clone();
        let decision = match self.engine.evaluate(&request.principal, request.action, &resource, request.context) {
            Ok(d) => d,
            // A request that does not fit the schema (wrong resource type for
            // the action, invalid hierarchy) is denied, never an internal error.
            Err(AuthzError::Request(msg)) => {
                tracing::info!(action = %request.action, resource = %request.resource, %msg, "authorization request rejected by schema");
                return Err(DomainError::Denied(format!("{} on {}", request.action, request.resource)));
            }
            Err(e) => return Err(DomainError::internal(e)),
        };
        if decision.allowed {
            Ok(())
        } else {
            tracing::info!(
                principal = %request.principal,
                action = %request.action,
                resource = %request.resource,
                errors = ?decision.errors,
                "authorization denied"
            );
            if decision.reasons.iter().any(|r| r == MFA_GUARD) {
                // The role allows it; only the step-up is missing. Say so,
                // so clients can ask for a code instead of showing a denial.
                return Err(DomainError::Denied(format!(
                    "{MFA_REQUIRED_PREFIX} {} needs an MFA-verified sign-in",
                    request.action
                )));
            }
            Err(DomainError::Denied(format!("{} on {}", request.action, request.resource)))
        }
    }
}
