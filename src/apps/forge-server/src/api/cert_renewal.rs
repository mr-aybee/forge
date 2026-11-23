//! Keeps Let's Encrypt certificates on the edges valid without anyone acting.
//!
//! Every 6 hours (and a minute after start) each Let's Encrypt endpoint is
//! checked on each of its edges (`certbot certificates`). A certificate with
//! [`RENEW_AT_DAYS`] days or fewer left, or missing, is renewed on that edge
//! (`certbot --force-renewal`, `nginx -t`, reload). Certbot's own timer
//! normally renews at 30 days; this is the backstop that guarantees it.
//! Failed renewals are retried on the next pass, then every hour once inside
//! the window. Results are kept per edge in `edge_status[edge].cert` and every
//! renewal is audited (`site.certificate.renew`).
//!
//! Commands run as the principal who last applied the endpoint, re-authorized
//! by the job runner on every step.
//!
//! * `POST /api/v1/sites/{id}/renew` — renew now on every edge (`proxy.manage`, MFA)

use std::time::Duration;

use axum::Json;
use axum::extract::{Path, State};
use forge_catalog::sites::Tls;
use forge_core::model::{AuditEvent, AuditOutcome, Site};
use forge_core::ports::SiteRepository;
use forge_core::{Action, AuthzContext, EntityKind, EntityRef, Principal, ServerId};
use serde_json::{Value, json};
use tokio::sync::watch;

use super::fabric_reconcile::AppliedBy;
use super::sites::{edges_now, per_edge, run_as, spec_of_site};
use crate::auth::{Caller, Ws};
use crate::error::{ApiError, ApiResult};
use crate::state::AppState;

/// Renew when this many days or fewer are left.
pub const RENEW_AT_DAYS: i64 = 5;
const PASS: Duration = Duration::from_secs(6 * 3600);
const RETRY: Duration = Duration::from_secs(3600);
const WAIT: Duration = Duration::from_secs(240);

/// Whether a certificate in this state must be renewed now.
#[must_use]
pub fn due(cert: &Value) -> bool {
    if cert.get("missing").is_some() {
        return true;
    }
    cert.get("days_left").and_then(Value::as_i64).is_some_and(|d| d <= RENEW_AT_DAYS)
}

/// Checks (and if due, or `force`, renews) one endpoint on every edge.
/// Returns whether any renewal failed.
async fn check_site(state: &AppState, site: &mut Site, principal: Principal, ctx: AuthzContext, force: bool) -> bool {
    let Ok(spec) = spec_of_site(site) else { return false };
    if spec.tls != Tls::Acme {
        return false;
    }
    let Ok(edges) = edges_now(state, site).await else { return false };
    let Ok(per) = per_edge(state, &spec, &edges).await else { return false };
    let mut failed = false;
    for (edge, spec) in per {
        let now = state.clock.now().0;
        let key = edge.to_string();
        let mut cert = json!({ "checked_at": now });
        if !force {
            let r = run_as(
                state,
                principal.clone(),
                ctx,
                edge,
                json!({ "type": "site", "action": "status", "spec": spec }),
                WAIT,
            )
            .await;
            match r {
                Ok(r) if r["status"] == "succeeded" => {
                    if let Some(c) = r["result"]["certificate"].as_object() {
                        for (k, v) in c {
                            cert[k] = v.clone();
                        }
                    }
                }
                Ok(r) => cert["error"] = r["error"].clone(),
                Err(e) => cert["error"] = json!(e.message),
            }
        }
        if force || due(&cert) {
            let before = cert.get("days_left").cloned();
            let r = run_as(
                state,
                principal.clone(),
                ctx,
                edge,
                json!({ "type": "site", "action": "renew", "spec": spec }),
                WAIT,
            )
            .await;
            let (ok, err) = match r {
                Ok(r) if r["status"] == "succeeded" => {
                    if let Some(c) = r["result"]["certificate"].as_object() {
                        for (k, v) in c {
                            cert[k] = v.clone();
                        }
                    }
                    (true, None)
                }
                Ok(r) => (false, r["error"].as_str().map(str::to_owned).or(Some("renewal did not finish".into()))),
                Err(e) => (false, Some(e.message)),
            };
            if let Some(o) = cert.as_object_mut() {
                o.remove("missing");
            }
            if ok {
                cert["renewed_at"] = json!(now);
                cert["renew_error"] = Value::Null;
            } else {
                failed = true;
                cert["renew_error"] = json!(err);
            }
            let ev = AuditEvent::new(
                principal.to_string(),
                "site.certificate.renew",
                EntityRef::new(EntityKind::Server, edge),
                if ok { AuditOutcome::Success } else { AuditOutcome::Failure },
            )
            .in_org(Some(site.workspace_id))
            .with_details(json!({ "site": site.name, "days_left_before": before, "forced": force, "error": err, "automatic": !force }));
            if let Err(e) = state.audit.append(ev).await {
                tracing::error!(error = %e, "audit write failed");
            }
        }
        if !site.edge_status.is_object() {
            site.edge_status = json!({});
        }
        if !site.edge_status[&key].is_object() {
            site.edge_status[&key] = json!({});
        }
        site.edge_status[&key]["cert"] = cert;
    }
    if let Err(e) = state.store.put_site(site).await {
        tracing::warn!(error = %e, "saving certificate status failed");
    }
    failed
}

/// One pass over every Let's Encrypt endpoint. Returns whether something is
/// still due (a failed renewal), so the loop comes back sooner.
async fn pass(state: &AppState) -> bool {
    let Ok(sites) = state.store.list_sites().await else { return true };
    let mut retry = false;
    for mut site in sites {
        if site.config["spec"]["tls"] != "acme" {
            continue;
        }
        let Some(by) =
            site.edge_status.get("_applied_by").cloned().and_then(|v| serde_json::from_value::<AppliedBy>(v).ok())
        else {
            continue; // never applied
        };
        retry |= check_site(state, &mut site, by.principal, by.context, false).await;
    }
    retry
}

pub(crate) async fn run(state: AppState, mut shutdown: watch::Receiver<bool>) {
    let mut wait = Duration::from_secs(60);
    loop {
        tokio::select! {
            () = tokio::time::sleep(wait) => {}
            _ = shutdown.changed() => break,
        }
        wait = if pass(&state).await { RETRY } else { PASS };
    }
}

/// Renew now on every edge, regardless of the days left.
pub async fn renew_now(
    State(state): State<AppState>,
    caller: Caller,
    Ws(ws): Ws,
    Path(id): Path<String>,
) -> ApiResult<Json<Value>> {
    let mut site = super::sites::load_visible(&state, &caller, ws, &id).await?;
    if site.config["spec"]["tls"] != "acme" {
        return Err(ApiError::bad_request(
            "only Let's Encrypt certificates are renewed by Forge; upload a new certificate from your CA instead",
        ));
    }
    let edges = edges_now(&state, &site).await?;
    if edges.is_empty() {
        return Err(ApiError::bad_request("this endpoint has no edge nodes"));
    }
    for e in edges {
        caller.authorize(Action::ProxyManage, EntityRef::new(EntityKind::Server, e)).await?;
    }
    let failed = check_site(&state, &mut site, caller.principal(), caller.ctx(), true).await;
    let certs: serde_json::Map<String, Value> = site
        .edge_status
        .as_object()
        .map(|m| {
            m.iter()
                .filter(|(k, _)| k.parse::<ServerId>().is_ok())
                .map(|(k, v)| (k.clone(), v["cert"].clone()))
                .collect()
        })
        .unwrap_or_default();
    Ok(Json(json!({ "ok": !failed, "certificates": certs })))
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::due;

    #[test]
    fn renews_at_five_days_or_when_missing() {
        assert!(!due(&json!({ "days_left": 30 })));
        assert!(!due(&json!({ "days_left": 6 })));
        assert!(due(&json!({ "days_left": 5 })));
        assert!(due(&json!({ "days_left": -1 })));
        assert!(due(&json!({ "missing": true })));
        assert!(!due(&json!({ "error": "agent offline" })), "unknown state: check again, do not hammer");
    }
}
