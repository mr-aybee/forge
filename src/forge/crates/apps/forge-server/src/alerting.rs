//! Runs alert evaluation (#11, D-087) every `[alerts] evaluate_interval_ms`.
//!
//! Evidence comes from [`ControlPlaneSource`]. The control plane computes
//! some subjects itself on every pass (D-118): each node's agent status
//! (`node.agent.status`) and agent certificate (`node.cert.expires_in_days`),
//! each service instance's state and deployment status
//! (`app.instance.state`, `app.deploy.status`), each package installation's
//! state and status (`package.state`, `package.status`) and each ingress
//! certificate's expiry (`tls.cert.expires_in_days`). Every other subject
//! comes from observations (D-085): health states and node measurements
//! such as `node.cpu.percent`.

use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use forge_alerts::catalog::{
    APP_DEPLOY_STATUS, NODE_AGENT_STATUS, NODE_CERT_EXPIRES, PACKAGE_STATE, PACKAGE_STATUS, SERVICE_INSTANCE_STATE,
    TLS_CERT_EXPIRES,
};
use forge_alerts::{Evaluator, SeriesSample, SeriesSource};
use forge_core::model::{AgentStatus, Installation, Server, ServiceInstance, TlsCertificate};
use forge_core::ports::{CertificateRepository, InstallationRepository, ServerRepository, ServiceRepository};
use forge_core::{Clock, DomainResult, EntityKind, EntityRef, Timestamp};
use forge_rules::{Sample, Value};
use tokio::sync::watch;

use crate::state::AppState;

const DAY_MS: f64 = 86_400_000.0;

/// Evidence the control plane has without asking a node.
#[derive(Debug, Clone)]
pub struct ControlPlaneSource {
    pub store: Arc<forge_store::Store>,
    pub clock: Arc<dyn Clock>,
    /// Time between two evaluation passes.
    pub interval: Duration,
}

/// One pass's time and how long its evidence counts.
#[derive(Debug, Clone, Copy)]
struct Pass {
    now: i64,
    valid_ms: i64,
}

impl Pass {
    fn sample(self, resource: EntityRef, server: Option<forge_core::ServerId>, value: Option<Value>) -> SeriesSample {
        SeriesSample { resource, server, sample: Sample { at: self.now, valid_until: self.now + self.valid_ms, value } }
    }

    fn days_until(self, t: Timestamp) -> Value {
        #[allow(clippy::cast_precision_loss)] // reason: millisecond spans fit an f64 exactly enough for days
        let days = (t.0 - self.now) as f64 / DAY_MS;
        Value::Num((days * 1000.0).round() / 1000.0)
    }
}

/// A recorded state as evidence: missing or empty is unknown.
fn state_value(s: Option<&str>) -> Option<Value> {
    s.filter(|s| !s.is_empty()).map(|s| Value::State(s.to_owned()))
}

/// `node.agent.status` and `node.cert.expires_in_days` of every node that
/// is not revoked.
fn server_samples(servers: &[Server], subject: &str, p: Pass) -> Vec<SeriesSample> {
    servers
        .iter()
        .filter(|s| s.status != AgentStatus::Revoked)
        .filter_map(|s| {
            let value = if subject == NODE_AGENT_STATUS {
                Some(Value::State(s.status.as_str().into()))
            } else {
                // A node that never received a certificate has no expiry.
                Some(p.days_until(s.cert_not_after?))
            };

            Some(p.sample(EntityRef::new(EntityKind::Server, s.id), None, value))
        })
        .collect()
}

/// `app.instance.state` and `app.deploy.status`: one series per instance
/// (the service on one node).
fn instance_samples(instances: &[ServiceInstance], subject: &str, p: Pass) -> Vec<SeriesSample> {
    instances
        .iter()
        .map(|i| {
            let v = if subject == SERVICE_INSTANCE_STATE { i.state.as_deref() } else { Some(i.status.as_str()) };
            p.sample(EntityRef::new(EntityKind::Service, i.service_id), Some(i.server_id), state_value(v))
        })
        .collect()
}

/// `package.state` and `package.status`: one series per installation.
fn installation_samples(installs: &[Installation], subject: &str, p: Pass) -> Vec<SeriesSample> {
    installs
        .iter()
        .map(|i| {
            let v = if subject == PACKAGE_STATE { i.state.as_deref() } else { Some(i.status.as_str()) };
            p.sample(EntityRef::new(EntityKind::Installation, i.id), Some(i.server_id), state_value(v))
        })
        .collect()
}

/// `tls.cert.expires_in_days`: one series per uploaded certificate.
fn certificate_samples(certs: &[TlsCertificate], p: Pass) -> Vec<SeriesSample> {
    certs
        .iter()
        .map(|c| p.sample(EntityRef::new(EntityKind::Certificate, c.id), None, Some(p.days_until(c.not_after))))
        .collect()
}

#[async_trait]
impl SeriesSource for ControlPlaneSource {
    async fn latest(&self, subject: &str) -> DomainResult<Vec<SeriesSample>> {
        // The values are current as of this pass; they count until two
        // passes later, so a slow pass never reads as missing evidence.
        let p =
            Pass { now: self.clock.now().0, valid_ms: i64::try_from(self.interval.as_millis()).unwrap_or(15_000) * 2 };

        match subject {
            NODE_AGENT_STATUS | NODE_CERT_EXPIRES => {
                Ok(server_samples(&self.store.list_servers(None).await?, subject, p))
            }
            SERVICE_INSTANCE_STATE | APP_DEPLOY_STATUS => {
                Ok(instance_samples(&self.store.list_all_instances(None).await?, subject, p))
            }
            PACKAGE_STATE | PACKAGE_STATUS => {
                Ok(installation_samples(&self.store.list_installations(None).await?, subject, p))
            }
            TLS_CERT_EXPIRES => Ok(certificate_samples(&self.store.list_certificates(None).await?, p)),
            _ => observed(&self.store, subject).await,
        }
    }
}

/// Observations (D-085) of one subject: health states and measurements.
/// Each value counts until its `fresh_until`; an unknown value is no
/// evidence.
async fn observed(store: &forge_store::Store, subject: &str) -> DomainResult<Vec<SeriesSample>> {
    use forge_core::observation::{ObservationFilter, ObservationQuery, ObservedValue};
    let filter = ObservationFilter { subject_prefix: Some(subject.to_owned()), ..ObservationFilter::default() };
    let rows = store.latest(&filter, 10_000).await?;

    Ok(rows
        .into_iter()
        .filter(|l| l.key.subject.as_str() == subject)
        .map(|l| {
            let value = match l.value {
                #[allow(clippy::cast_precision_loss)]
                ObservedValue::Measure { milli: Some(m), .. } => Some(Value::Num(m as f64 / 1000.0)),
                v if v.is_unknown() => None,
                v => serde_json::to_value(v).ok().and_then(|j| {
                    j.get("state").and_then(serde_json::Value::as_str).map(|s| Value::State(s.to_owned()))
                }),
            };

            SeriesSample {
                resource: l.key.resource.clone(),
                server: l.key.server,
                sample: Sample { at: l.observed_at.0, valid_until: l.fresh_until.0, value },
            }
        })
        .collect())
}

fn evaluator(state: &AppState) -> Evaluator {
    let store = Arc::new(state.store.clone());
    Evaluator {
        alerts: store.clone(),
        state: store.clone(),
        source: Arc::new(ControlPlaneSource {
            store: store.clone(),
            clock: state.clock.clone(),
            interval: interval(state),
        }),
        resolver: store,
    }
}

/// Time between two evaluation passes.
fn interval(state: &AppState) -> Duration {
    Duration::from_millis(state.config.alerts.evaluate_interval_ms)
}

/// Background loop.
pub(crate) async fn run(state: AppState, mut shutdown: watch::Receiver<bool>) {
    let eval = evaluator(&state);
    let mut tick = tokio::time::interval(interval(&state));

    loop {
        tokio::select! {
            _ = tick.tick() => {}
            _ = shutdown.changed() => break,
        }

        match eval.pass(state.clock.now()).await {
            Ok(r) if r.fired + r.resolved + r.ended > 0 => {
                tracing::info!(fired = r.fired, resolved = r.resolved, ended = r.ended, "alert pass");
            }
            Ok(_) => {}
            Err(e) => tracing::warn!(error = %e, "alert pass failed"),
        }
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)] // reason: tests
mod tests {
    use forge_core::model::NodeRole;
    use forge_core::{CertificateId, InstallationId, Principal, ServerId, ServiceId, UserId, WorkspaceId};

    use super::*;

    const P: Pass = Pass { now: 1_000_000, valid_ms: 30_000 };

    fn node(status: AgentStatus, cert: Option<i64>) -> Server {
        Server {
            id: ServerId::new(),
            workspace_id: WorkspaceId::new(),
            environment_id: None,
            cluster_id: None,
            role: NodeRole::Worker,
            display_name: None,
            hostname: "db-1".into(),
            os: "linux".into(),
            arch: "x86_64".into(),
            agent_version: "0".into(),
            status,
            last_seen_at: None,
            cert_serial: None,
            cert_not_after: cert.map(Timestamp),
            drained: false,
            created_at: Timestamp(1),
        }
    }

    fn values(s: &[SeriesSample]) -> Vec<Option<Value>> {
        s.iter().map(|s| s.sample.value.clone()).collect()
    }

    #[test]
    fn nodes_give_status_and_certificate_days() {
        let in_ten_days = P.now + 10 * 86_400_000;
        let nodes = [
            node(AgentStatus::Online, Some(in_ten_days)),
            node(AgentStatus::Offline, None),
            node(AgentStatus::Revoked, Some(in_ten_days)),
        ];
        let status = server_samples(&nodes, NODE_AGENT_STATUS, P);
        assert_eq!(values(&status), vec![Some(Value::State("online".into())), Some(Value::State("offline".into()))]);
        let cert = server_samples(&nodes, NODE_CERT_EXPIRES, P);
        assert_eq!(values(&cert), vec![Some(Value::Num(10.0))], "no certificate, no series; revoked skipped");
        assert_eq!(cert[0].sample.at, P.now);
        assert_eq!(cert[0].sample.valid_until, P.now + 30_000);
    }

    #[test]
    fn instances_give_state_and_deploy_status_per_node() {
        let i = ServiceInstance {
            service_id: ServiceId::new(),
            server_id: ServerId::new(),
            role: "active".into(),
            status: "failed".into(),
            state: None,
            last_job_id: None,
            last_error: None,
            updated_at: Timestamp(1),
            desired: forge_core::model::InstanceDesired::Running,
            deployed_release: None,
            exit_code: None,
        };
        let st = instance_samples(std::slice::from_ref(&i), SERVICE_INSTANCE_STATE, P);
        assert_eq!(values(&st), vec![None], "no reported state is unknown");
        assert_eq!(st[0].resource, EntityRef::new(EntityKind::Service, i.service_id));
        assert_eq!(st[0].server, Some(i.server_id));
        let dep = instance_samples(&[i], APP_DEPLOY_STATUS, P);
        assert_eq!(values(&dep), vec![Some(Value::State("failed".into()))]);
    }

    #[test]
    fn installations_give_state_and_status() {
        let i = Installation {
            id: InstallationId::new(),
            workspace_id: WorkspaceId::new(),
            server_id: ServerId::new(),
            package: "postgresql".into(),
            version: "17".into(),
            params: Default::default(),
            status: "installed".into(),
            state: Some("stopped".into()),
            env: Default::default(),
            last_job_id: None,
            last_error: None,
            versions: None,
            update_policy: "notify".into(),
            created_at: Timestamp(1),
            updated_at: Timestamp(1),
        };
        let st = installation_samples(std::slice::from_ref(&i), PACKAGE_STATE, P);
        assert_eq!(values(&st), vec![Some(Value::State("stopped".into()))]);
        assert_eq!(st[0].resource, EntityRef::new(EntityKind::Installation, i.id));
        assert_eq!(st[0].server, Some(i.server_id));
        assert_eq!(
            values(&installation_samples(&[i], PACKAGE_STATUS, P)),
            vec![Some(Value::State("installed".into()))]
        );
    }

    #[test]
    fn certificates_count_days_and_go_negative_once_expired() {
        let cert = |not_after| TlsCertificate {
            id: CertificateId::new(),
            workspace_id: WorkspaceId::new(),
            name: "www".into(),
            domains: vec!["www.example.com".into()],
            issuer: "CA".into(),
            not_before: Timestamp(0),
            not_after: Timestamp(not_after),
            fingerprint: String::new(),
            chain_pem: String::new(),
            sealed_key: String::new(),
            created_by: Principal::User(UserId::new()),
            created_at: Timestamp(0),
            updated_at: Timestamp(0),
        };
        let s = certificate_samples(&[cert(P.now + 36 * 3_600_000), cert(P.now - 86_400_000)], P);
        assert_eq!(values(&s), vec![Some(Value::Num(1.5)), Some(Value::Num(-1.0))]);
        assert_eq!(s[0].resource.kind, EntityKind::Certificate);
    }
}
