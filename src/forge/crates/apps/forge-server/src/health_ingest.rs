//! Health check results from heartbeats → observations (D-085).
//!
//! A node names an app by `(environment, app)` slugs. Only apps placed on the
//! reporting node resolve; everything else is dropped, and the recorder's
//! reporting rule checks the placement again.
//!
//! The other direction is here too: [`push`] sends an app's health policy
//! to a node with the signed app command `health`, so a saved policy takes
//! effect without a redeploy. Each report names the policy revision it ran
//! under; a node that reports another revision than the stored policy's
//! (it was offline when the policy was saved, or it was just redeployed)
//! gets the policy again, at most once per [`PUSH_RETRY`].

use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};
use std::time::Duration;

use forge_catalog::health::{ApplySpec, HealthPolicy};
use forge_core::model::Service;
use forge_core::observation::{
    HealthState, Observation, ObservationKey, ObservationSource, ObservedValue, ReasonCode, StartupState, Subject,
};
use forge_core::ports::ServiceRepository;
use forge_core::{EntityKind, EntityRef, ServerId, ServiceId, Timestamp};
use forge_proto::Command;
use forge_proto::messages::{AppAction, HealthReport, MAX_HEALTH_REPORTS};

use crate::state::AppState;

/// The principal recorded in policy pushes.
pub const SYSTEM_PRINCIPAL: &str = "system:health";
/// How long to wait before sending the same policy to the same node again
/// (an agent too old for the command refuses it every time).
pub const PUSH_RETRY: Duration = Duration::from_secs(600);

/// What a node needs to run `app`'s checks. `None` when the stored
/// service can't be described (no environment slug, unknown runtime).
#[must_use]
pub fn apply_spec(app: &Service) -> Option<ApplySpec> {
    let environment = app.spec.get("environment")?.as_str()?.to_owned();
    let runtime = serde_json::from_value(serde_json::Value::from(app.runtime.as_str())).ok()?;
    let listen_port =
        app.spec.get("listen_port").and_then(serde_json::Value::as_u64).and_then(|p| u16::try_from(p).ok());
    let policy: HealthPolicy =
        app.spec.get("health").and_then(|v| serde_json::from_value(v.clone()).ok()).unwrap_or_default();
    let revision = forge_catalog::health::revision(listen_port, &policy);

    Some(ApplySpec {
        environment,
        name: app.name.clone(),
        runtime,
        listen_port,
        policy,
        revision,
        fleet: Vec::new(),
        slot: String::new(),
    })
}

/// [`apply_spec`] for the copy that serves on a node (#73): a slot's
/// listen port and probe ports are its own, and the node checks its unit or
/// container. The revision stays the policy's: the slot's form follows from
/// it, so a node never reports a revision Forge does not know.
#[must_use]
pub fn node_apply_spec(app: &Service, serving: Option<&forge_core::deploy::SlotRecord>) -> Option<ApplySpec> {
    let mut spec = apply_spec(app)?;
    let Some(slot) = serving.filter(|s| !s.slot.is_empty()) else { return Some(spec) };
    let map = crate::deploy::port_map(slot);
    if let Some(p) = spec.listen_port.and_then(|p| map.get(&p)) {
        spec.listen_port = Some(*p);
    }

    forge_catalog::deploy::retarget(&mut spec.policy, &map);
    spec.slot.clone_from(&slot.slot);

    Some(spec)
}

/// Last push per node and app: the revision sent and when (epoch ms).
type Pushes = HashMap<(ServerId, ServiceId), (i64, i64)>;

fn pushes() -> &'static Mutex<Pushes> {
    static P: OnceLock<Mutex<Pushes>> = OnceLock::new();
    P.get_or_init(|| Mutex::new(HashMap::new()))
}

/// Whether to send `revision` now, given the last push `(revision, at)`.
fn push_due(last: Option<(i64, i64)>, revision: i64, now: i64) -> bool {
    let retry = i64::try_from(PUSH_RETRY.as_millis()).unwrap_or(i64::MAX);
    last.is_none_or(|(sent, at)| sent != revision || now.saturating_sub(at) >= retry)
}

/// Sends `app`'s health policy to `server` (in the background). With
/// `force`, at once (the policy was just saved); otherwise only when the
/// same revision wasn't sent within [`PUSH_RETRY`].
pub fn push(state: &AppState, app: &Service, server: ServerId, force: bool) {
    let (state, app) = (state.clone(), app.clone());
    tokio::spawn(async move {
        // The copy that serves there decides the unit and ports (#73).
        let slots = match forge_core::deploy::DeploymentRepository::list_slots(&state.store, app.id).await {
            Ok(s) => s,
            Err(e) => {
                tracing::warn!(%server, app = %app.name, error = %e, "reading the service's copies failed");
                return;
            }
        };

        let Some(spec) = node_apply_spec(&app, crate::deploy::serving(&slots, server)) else { return };
        push_spec(&state, &app, server, spec, force);
    });
}

fn push_spec(state: &AppState, app: &Service, server: ServerId, mut spec: ApplySpec, force: bool) {
    let now = state.clock.now().0;

    {
        let Ok(mut p) = pushes().lock() else { return };
        if !force && !push_due(p.get(&(server, app.id)).copied(), spec.revision, now) {
            return;
        }

        p.insert((server, app.id), (spec.revision, now));
    }

    // Only a check that probes another host needs to know the fleet; every
    // other policy is queued at once.
    if spec.policy.checks().all(|(_, c)| c.probe.remote_host().is_none()) {
        send(state, server, &app.name, &spec);
        return;
    }

    let (state, service_id, workspace, app) = (state.clone(), app.id, app.workspace_id, app.name.clone());
    tokio::spawn(async move {
        match fleet_addresses(&state, server, workspace).await {
            Ok(fleet) => {
                spec.fleet = fleet;
                send(&state, server, &app, &spec);
            }
            // Without the list the node could not refuse the fleet's
            // addresses: nothing is sent, and the push is not counted as
            // done, so the node's next report tries again.
            Err(e) => {
                tracing::warn!(%server, %app, error = %e, "the fleet list for a health policy could not be built");

                if let Ok(mut p) = pushes().lock() {
                    p.remove(&(server, service_id));
                }
            }
        }
    });
}

/// Signs and queues the policy for `server`; the node's answer is logged.
fn send(state: &AppState, server: ServerId, app: &str, spec: &ApplySpec) {
    let Ok(spec_json) = serde_json::to_string(spec) else { return };
    let command = Command::AppAction(AppAction {
        action: forge_catalog::health::APPLY_ACTION.into(),
        spec_json,
        tail: 0,
        cursor: String::new(),
        registry_auth: String::new(),
    });
    let (body, signed) =
        state.signer.issue(&server.to_string(), SYSTEM_PRINCIPAL, command, state.clock.now(), Duration::from_secs(120));

    if let Err(e) = state.hub.enqueue(server, &body.operation_id, signed) {
        tracing::debug!(%server, %app, error = %e, "queueing the health policy failed");
        return;
    }

    let (state, app) = (state.clone(), app.to_owned());
    tokio::spawn(async move {
        match state.hub.wait(&body.operation_id, Duration::from_secs(90)).await {
            Some(r) if r.ok => tracing::debug!(%server, %app, "health policy applied on the node"),
            Some(r) => tracing::debug!(%server, %app, error = %r.error, "the node refused the health policy"),
            None => tracing::debug!(%server, %app, "the node did not answer the health policy"),
        }
    });
}

/// The addresses of every node except `target`, as Forge knows them now:
/// where each agent connects from, the addresses in its facts (its own and
/// its interfaces') and its fabric address. A node refuses to probe them
/// (D-111). An address `target` itself has is left out: nodes behind one
/// NAT share the address their agents connect from. Sorted, at most
/// [`MAX_FLEET_ADDRESSES`](forge_catalog::health::MAX_FLEET_ADDRESSES).
///
/// Only the nodes of the service's `workspace` are listed. A store
/// error is an error: the caller sends nothing rather than a policy whose
/// list is missing a source. A list cut at the limit is logged.
pub async fn fleet_addresses(
    state: &AppState,
    target: ServerId,
    workspace: forge_core::WorkspaceId,
) -> forge_core::DomainResult<Vec<std::net::IpAddr>> {
    use forge_core::ports::{FabricRepository, ServerRepository};
    let servers = state.store.list_servers(Some(workspace)).await?;
    let facts = forge_core::tags::FactsRepository::all_facts(&state.store).await?;
    let fabrics = state.store.list_fabrics(Some(workspace)).await?;
    let known: Vec<NodeAddresses> = servers
        .iter()
        .map(|s| NodeAddresses {
            node: s.id.to_string(),
            live: crate::facts::live_address(s.id),
            facts: facts.get(&s.id).map(|(f, _)| f.clone()),
        })
        .collect();
    let leases: Vec<forge_catalog::fabric::Lease> = fabrics
        .iter()
        // A fabric whose stored leases don't parse contributes nothing; its
        // own page reports that.
        .flat_map(|f| serde_json::from_value::<Vec<forge_catalog::fabric::Lease>>(f.leases.clone()).unwrap_or_default())
        .collect();
    let (fleet, cut) = fleet_of(&target.to_string(), &known, &leases);
    if cut > 0 {
        tracing::warn!(
            %target,
            left_out = cut,
            limit = forge_catalog::health::MAX_FLEET_ADDRESSES,
            "the fleet list of a health policy was cut: the node will not refuse the addresses left out"
        );
    }

    Ok(fleet)
}

/// What Forge knows about where one node is.
#[derive(Debug, Clone)]
struct NodeAddresses {
    node: String,
    /// Where its agent connects from, in this process.
    live: Option<String>,
    /// Its saved facts (`address`, `interfaces[].address`).
    facts: Option<serde_json::Value>,
}

/// [`fleet_addresses`] from the gathered data, and how many addresses the
/// limit left out.
fn fleet_of(
    target: &str,
    nodes: &[NodeAddresses],
    leases: &[forge_catalog::fabric::Lease],
) -> (Vec<std::net::IpAddr>, usize) {
    use std::collections::BTreeSet;
    use std::net::IpAddr;
    let canonical = |s: &str| s.parse::<IpAddr>().ok().map(forge_catalog::health::canonical);
    let (mut own, mut others) = (BTreeSet::new(), BTreeSet::new());

    for n in nodes {
        let set = if n.node == target { &mut own } else { &mut others };
        set.extend(n.live.as_deref().and_then(canonical));

        if let Some(f) = &n.facts {
            set.extend(f["address"].as_str().and_then(canonical));

            for i in f["interfaces"].as_array().into_iter().flatten() {
                set.extend(i["address"].as_str().and_then(canonical));
            }
        }
    }

    for l in leases.iter().filter(|l| l.kind == forge_catalog::fabric::LeaseKind::Node) {
        let ip = IpAddr::V4(l.cidr.first());

        if l.owner == target {
            own.insert(ip)
        } else {
            others.insert(ip)
        };
    }

    let mut all: Vec<IpAddr> =
        others.into_iter().filter(|ip| !own.contains(ip) && !ip.is_loopback() && !ip.is_unspecified()).collect();
    let cut = all.len().saturating_sub(forge_catalog::health::MAX_FLEET_ADDRESSES);
    all.truncate(forge_catalog::health::MAX_FLEET_ADDRESSES);

    (all, cut)
}

fn reason(s: &str) -> Option<ReasonCode> {
    Some(match s {
        "" => return None,
        "app_failure" => ReasonCode::AppFailure,
        "timeout" => ReasonCode::Timeout,
        "connect_failed" => ReasonCode::ConnectFailed,
        "tls" => ReasonCode::Tls,
        "auth" => ReasonCode::Auth,
        "invalid_config" => ReasonCode::InvalidConfig,
        "unsupported_executor" => ReasonCode::UnsupportedExecutor,
        "location_unavailable" => ReasonCode::LocationUnavailable,
        "permission_denied" => ReasonCode::PermissionDenied,
        "ambiguous_port" => ReasonCode::AmbiguousPort,
        // Unknown codes from newer agents: keep the state, drop the reason.
        _ => return None,
    })
}

fn value(purpose: &str, state: &str) -> Option<ObservedValue> {
    if purpose == "startup" {
        return Some(ObservedValue::Startup(match state {
            "waiting" => StartupState::Waiting,
            "succeeded" => StartupState::Succeeded,
            "exhausted" => StartupState::Exhausted,
            "unknown" => StartupState::Unknown,
            _ => return None,
        }));
    }

    Some(ObservedValue::Health(match state {
        "passing" => HealthState::Passing,
        "failing" => HealthState::Failing,
        "pending" => HealthState::Pending,
        "unknown" => HealthState::Unknown,
        "disabled" => HealthState::Disabled,
        _ => return None,
    }))
}

/// The observation for one report, given the app it resolved to. `None`
/// for malformed reports.
#[must_use]
pub fn observation(server: ServerId, service_id: &str, r: &HealthReport) -> Option<Observation> {
    if !matches!(r.purpose.as_str(), "startup" | "readiness" | "liveness") {
        return None;
    }

    let subject = Subject::new(format!("health.{}.{}", r.purpose, r.check)).ok()?;

    Some(Observation {
        key: ObservationKey {
            resource: EntityRef::new(EntityKind::Service, service_id),
            server: Some(server),
            subject,
        },
        value: value(&r.purpose, &r.state)?,
        reason: reason(&r.reason),
        detail: (!r.detail.is_empty()).then(|| r.detail.chars().take(256).collect()),
        source: ObservationSource::Probe { policy: format!("rev-{}", r.policy_revision), check: r.check.clone() },
        executor: Some(server),
        generation: (!r.generation.is_empty()).then(|| r.generation.chars().take(128).collect()),
        observed_at: Timestamp(r.observed_at_ms),
        fresh_for_ms: r.fresh_for_ms,
    })
}

/// Records the reports of one heartbeat (in the background: never fails it).
pub fn ingest(state: &AppState, server: ServerId, reports: Vec<HealthReport>) {
    if reports.is_empty() {
        return;
    }

    let state = state.clone();
    tokio::spawn(async move {
        // Apps placed on this node, by their slugs.
        let mut by_slug: HashMap<(String, String), Service> = HashMap::new();
        let instances = state.store.list_all_instances(None).await.unwrap_or_default();

        for i in instances.iter().filter(|i| i.server_id == server) {
            if let Ok(Some(app)) = state.store.get_service(i.service_id).await {
                let ns = app.spec.get("environment").and_then(serde_json::Value::as_str).unwrap_or_default().to_owned();
                by_slug.insert((ns, app.name.clone()), app);
            }
        }

        let obs: Vec<Observation> = reports
            .iter()
            .take(MAX_HEALTH_REPORTS)
            .filter_map(|r| {
                let app = by_slug.get(&(r.environment.clone(), r.app.clone()))?;
                observation(server, &app.id.to_string(), r)
            })
            .collect();

        // A node that checks under another policy revision gets the stored
        // one. A policy with command checks or a remote target is sent
        // again every [`PUSH_RETRY`] while the node reports: the helper
        // runs command checks only from a policy it heard of recently, and
        // the fleet list is refreshed with it (D-111).
        for ((ns, name), app) in &by_slug {
            let Some(spec) = apply_spec(app) else { continue };
            let reporting = || reports.iter().filter(|r| r.environment == *ns && r.app == *name);
            let refresh = spec.policy.runs_code() || spec.policy.checks().any(|(_, c)| c.probe.remote_host().is_some());
            if reporting().any(|r| r.policy_revision != spec.revision) || (refresh && reporting().next().is_some()) {
                push(&state, app, server, false);
            }
        }

        // Reports for apps not placed here, malformed ones, or beyond the cap.
        let dropped = reports.len().saturating_sub(obs.len());
        if dropped > 0 {
            tracing::warn!(%server, dropped, "dropped health reports (unknown app, not placed on this node, or malformed)");
        }

        if obs.is_empty() {
            return;
        }

        if let Err(e) = state.health.record_from_agent(server, obs).await {
            tracing::warn!(%server, error = %e, "recording health check results failed");
        }
    });
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    fn report(purpose: &str, state: &str) -> HealthReport {
        HealthReport {
            environment: "uat".into(),
            app: "api".into(),
            purpose: purpose.into(),
            check: "default".into(),
            state: state.into(),
            reason: "ambiguous_port".into(),
            detail: "x".repeat(400),
            generation: "abc".into(),
            observed_at_ms: 5,
            fresh_for_ms: 90_000,
            policy_revision: 0,
        }
    }

    #[test]
    fn reports_map_to_instance_observations() {
        let s = ServerId::new();
        let o = observation(s, "app-1", &report("readiness", "passing")).unwrap();
        assert_eq!(o.key.subject.as_str(), "health.readiness.default");
        assert_eq!((o.key.server, o.executor), (Some(s), Some(s)));
        assert_eq!(o.value, ObservedValue::Health(HealthState::Passing));
        assert_eq!(o.reason, Some(ReasonCode::AmbiguousPort));
        assert_eq!(o.detail.unwrap().chars().count(), 256);
        assert_eq!(
            observation(s, "a", &report("startup", "exhausted")).unwrap().value,
            ObservedValue::Startup(StartupState::Exhausted)
        );
    }

    #[test]
    fn a_policy_is_sent_again_only_when_it_changed_or_after_the_retry_gap() {
        assert!(push_due(None, 5, 1_000));
        assert!(!push_due(Some((5, 1_000)), 5, 2_000), "same revision, just sent");
        assert!(push_due(Some((5, 1_000)), 6, 2_000), "another revision");
        assert!(push_due(Some((5, 1_000)), 5, 1_000 + 600_000), "the retry gap passed");
    }

    /// D-111: the list a node refuses to probe is every other node's
    /// addresses, never its own.
    #[test]
    fn the_fleet_list_names_the_other_nodes_only() {
        use forge_catalog::fabric::{Cidr, Lease, LeaseKind};
        let node = |id: &str, live: Option<&str>, facts: serde_json::Value| NodeAddresses {
            node: id.into(),
            live: live.map(str::to_owned),
            facts: (!facts.is_null()).then_some(facts),
        };
        let nodes = [
            node("a", Some("10.0.0.1"), serde_json::json!({ "address": "10.0.0.1" })),
            node(
                "b",
                Some("::ffff:10.0.0.2"),
                serde_json::json!({ "address": "192.0.2.9", "interfaces": [
                    { "interface": "eth0", "address": "10.0.0.2" }, { "interface": "eth1", "address": "172.16.0.2" }
                ] }),
            ),
            // Behind the same NAT as `a`, never connected in this process.
            node("c", None, serde_json::json!({ "address": "10.0.0.1", "interfaces": [{ "address": "10.0.0.3" }] })),
            node("d", None, serde_json::Value::Null),
        ];
        let lease = |owner: &str, kind, ip: [u8; 4], prefix| Lease {
            kind,
            owner: owner.into(),
            cidr: Cidr::new(std::net::Ipv4Addr::from(ip), prefix).unwrap(),
            pinned: false,
        };
        let leases = [
            lease("a", LeaseKind::Node, [100, 64, 0, 1], 32),
            lease("b", LeaseKind::Node, [100, 64, 0, 2], 32),
            lease("b", LeaseKind::Block, [100, 64, 8, 0], 24),
        ];
        let ips = |v: (Vec<std::net::IpAddr>, usize)| v.0.iter().map(ToString::to_string).collect::<Vec<_>>();
        assert_eq!(
            ips(fleet_of("a", &nodes, &leases)),
            ["10.0.0.2", "10.0.0.3", "100.64.0.2", "172.16.0.2", "192.0.2.9"],
            "not its own address (shared with c), not its own lease, no instance blocks"
        );
        assert_eq!(ips(fleet_of("d", &nodes, &leases)).len(), 7, "a node Forge knows no address of gets all others");

        // Bounded.
        let many: Vec<NodeAddresses> = (0..3000u32)
            .map(|i| node(&format!("n{i}"), Some(&format!("10.{}.{}.1", i >> 8, i & 255)), serde_json::Value::Null))
            .collect();
        let (kept, cut) = fleet_of("x", &many, &[]);
        assert_eq!((kept.len(), cut), (forge_catalog::health::MAX_FLEET_ADDRESSES, 3000 - kept.len()));
    }

    #[test]
    fn malformed_reports_are_dropped() {
        let s = ServerId::new();
        assert!(observation(s, "a", &report("bogus", "passing")).is_none());
        assert!(observation(s, "a", &report("liveness", "great")).is_none());
        let mut bad = report("liveness", "passing");
        bad.check = "Bad Name".into();
        assert!(observation(s, "a", &bad).is_none());
    }
}
