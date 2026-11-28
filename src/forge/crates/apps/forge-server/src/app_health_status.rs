//! A service's health per node (D-085), from the stored observations.
//! Shared by the health API and the deployment orchestrator (#73), which
//! gates a rollout on it.

use forge_catalog::health::{HealthPolicy, Purpose};
use forge_core::model::Service;
use forge_core::observation::ObservationQuery;
use forge_core::ports::ServiceRepository;
use forge_core::{DomainResult, EntityKind, EntityRef, Timestamp};
use serde::Serialize;
use serde_json::Value;

/// The service's health policy (the default when it has none).
pub fn policy_of(app: &Service) -> HealthPolicy {
    app.spec.get("health").and_then(|v| serde_json::from_value(v.clone()).ok()).unwrap_or_default()
}

/// One check's current result on one node.
#[derive(Debug, Serialize)]
pub struct InstanceCheck {
    pub purpose: String,
    pub name: String,
    /// `passing`, `failing`, `unknown`, …; `unknown` once stale.
    pub state: String,
    pub reason: Option<forge_core::observation::ReasonCode>,
    pub detail: Option<String>,
    pub stale: bool,
    pub default: bool,
    /// When the node's last report arrived; `null` before the first one.
    pub received_at: Option<Timestamp>,
}

/// The service's health on one node.
#[derive(Debug, Serialize)]
pub struct InstanceHealth {
    pub server_id: forge_core::ServerId,
    pub status: forge_health::InstanceStatus,
    pub checks: Vec<InstanceCheck>,
}

fn state_name(v: forge_core::observation::ObservedValue) -> String {
    serde_json::to_value(v)
        .ok()
        .and_then(|j| j.get("state").and_then(Value::as_str).map(str::to_owned))
        .unwrap_or_default()
}

/// Per-node status from the stored observations.
pub async fn instances(
    obs: &dyn ObservationQuery,
    apps: &dyn ServiceRepository,
    now: Timestamp,
    app: &Service,
    policy: &HealthPolicy,
) -> DomainResult<Vec<InstanceHealth>> {
    use forge_core::observation::ObservationFilter;
    let filter = ObservationFilter {
        resource: Some(EntityRef::new(EntityKind::Service, app.id)),
        server: None,
        subject_prefix: Some("health.".into()),
    };
    let rows = obs.latest(&filter, 1_000).await?;
    let mut out = Vec::new();

    for inst in apps.list_instances(app.id).await? {
        let (mut startup, mut readiness, mut liveness, mut checks) = (vec![], vec![], vec![], vec![]);

        // Configured checks the node hasn't reported yet.
        let mut waiting: Vec<(Purpose, &str)> = policy.checks().map(|(p, c)| (p, c.name.as_str())).collect();

        for l in rows.iter().filter(|l| l.key.server == Some(inst.server_id)) {
            let mut parts = l.key.subject.as_str().splitn(3, '.').skip(1);
            let (Some(purpose), Some(name)) = (parts.next(), parts.next()) else { continue };
            let default = name == "default";

            // Results of checks the policy no longer has are not part of
            // the status (the node stops reporting them; the rows age out).
            let current = if default {
                policy.default_checks && (purpose != "readiness" || policy.readiness.is_empty())
            } else {
                let before = waiting.len();
                waiting.retain(|(p, n)| !(p.as_str() == purpose && *n == name));
                waiting.len() != before
            };

            if !current {
                continue;
            }

            let effective = l.effective(now);
            let required = default || policy.readiness.iter().find(|c| c.name == name).is_none_or(|c| c.required);
            let v = forge_health::CheckValue { value: effective, required: purpose != "readiness" || required };

            match purpose {
                "startup" => startup.push(v),
                "readiness" => readiness.push(v),
                "liveness" => liveness.push(v),
                _ => continue,
            }

            checks.push(InstanceCheck {
                purpose: purpose.to_owned(),
                name: name.to_owned(),
                state: state_name(effective),
                reason: l.reason,
                detail: l.detail.clone(),
                stale: !l.is_fresh(now),
                default,
                received_at: Some(l.received_at),
            });
        }

        for (purpose, name) in waiting {
            use forge_core::observation::{HealthState, ObservedValue, StartupState};
            let enabled = policy.checks().any(|(p, c)| p == purpose && c.name == name && c.enabled);
            let (value, state) = match (enabled, purpose) {
                (false, _) => (ObservedValue::Health(HealthState::Disabled), "disabled"),
                (true, Purpose::Startup) => (ObservedValue::Startup(StartupState::Unknown), "unknown"),
                (true, _) => (ObservedValue::Health(HealthState::Unknown), "unknown"),
            };

            let required = policy.readiness.iter().find(|c| c.name == name).is_none_or(|c| c.required);
            let v = forge_health::CheckValue { value, required: purpose != Purpose::Readiness || required };

            match purpose {
                // A disabled startup check gates nothing.
                Purpose::Startup if !enabled => {}
                Purpose::Startup => startup.push(v),
                Purpose::Readiness => readiness.push(v),
                Purpose::Liveness => liveness.push(v),
            }

            checks.push(InstanceCheck {
                purpose: purpose.as_str().to_owned(),
                name: name.to_owned(),
                state: state.to_owned(),
                reason: None,
                detail: enabled.then(|| "no result from this node yet".to_owned()),
                stale: false,
                default: false,
                received_at: None,
            });
        }

        out.push(InstanceHealth {
            server_id: inst.server_id,
            status: forge_health::instance_status(&startup, &readiness, &liveness),
            checks,
        });
    }

    Ok(out)
}
