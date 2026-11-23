#![allow(clippy::unwrap_used)]

use std::sync::Mutex;

use forge_core::alerts::{AlarmFilter, AlertRule, Severity};
use forge_core::model::{AgentStatus, Environment, NodeRole, Server, Workspace};
use forge_core::ports::{ServerRepository, TenancyRepository};
use forge_core::{EnvironmentId, Principal, UserId};
use forge_rules::Value;
use forge_store::Store;

use super::*;

const MIN: i64 = 60_000;

#[derive(Debug, Default)]
struct Fake(Mutex<Vec<(String, SeriesSample)>>);

#[async_trait]
impl SeriesSource for Fake {
    async fn latest(&self, subject: &str) -> DomainResult<Vec<SeriesSample>> {
        Ok(self.0.lock().unwrap().iter().filter(|(s, _)| s == subject).map(|(_, x)| x.clone()).collect())
    }
}

impl Fake {
    fn set(&self, subject: &str, server: ServerId, at: i64, v: f64) {
        let mut g = self.0.lock().unwrap();
        g.retain(|(s, x)| !(s == subject && x.resource.id == server.to_string()));
        g.push((
            subject.into(),
            SeriesSample {
                resource: EntityRef::new(EntityKind::Server, server),
                server: None,
                sample: Sample { at, valid_until: at + 2 * MIN, value: Some(Value::Num(v)) },
            },
        ));
    }
}

struct Env {
    store: Arc<Store>,
    source: Arc<Fake>,
    eval: Evaluator,
    ws: WorkspaceId,
    node: ServerId,
    _dir: tempfile::TempDir,
}

async fn env() -> Env {
    let dir = tempfile::tempdir().unwrap();
    let store = Arc::new(Store::open_sqlite(&dir.path().join("forge.db")).await.unwrap());
    let ws = Workspace { id: WorkspaceId::new(), name: "Acme".into(), slug: "acme".into(), created_at: Timestamp(1) };
    store.create_workspace(&ws).await.unwrap();
    let ns = Environment {
        id: EnvironmentId::new(),
        workspace_id: ws.id,
        cluster_id: None,
        project_id: None,
        name: "prod".into(),
        protected: false,
        created_at: Timestamp(1),
    };
    store.create_environment(&ns).await.unwrap();
    let node = Server {
        id: ServerId::new(),
        workspace_id: ws.id,
        environment_id: Some(ns.id),
        cluster_id: None,
        role: NodeRole::Worker,
        display_name: None,
        hostname: "web-1".into(),
        os: "linux".into(),
        arch: "x86_64".into(),
        agent_version: "0".into(),
        status: AgentStatus::Online,
        last_seen_at: None,
        cert_serial: None,
        cert_not_after: None,
        created_at: Timestamp(1),
    };
    store.create_server(&node).await.unwrap();
    let source = Arc::new(Fake::default());
    let eval =
        Evaluator { alerts: store.clone(), state: store.clone(), source: source.clone(), resolver: store.clone() };
    Env { store, source, eval, ws: ws.id, node: node.id, _dir: dir }
}

fn cpu_rule(ws: WorkspaceId) -> AlertRule {
    AlertRule {
        id: AlertRuleId::new(),
        workspace_id: ws,
        name: "CPU high".into(),
        description: String::new(),
        enabled: true,
        severity: Severity::Major,
        scope: RuleScope::Servers { ids: vec![] },
        subject: "node.cpu.percent".into(),
        spec: json!({
            "trigger": {"kind": "threshold", "cmp": "gt", "value": 85.0},
            "mode": {"mode": "sustained", "for_ms": 5 * MIN},
            "recovery": {"condition": {"kind": "threshold", "cmp": "lt", "value": 75.0}, "hold_ms": 2 * MIN},
        }),
        revision: 1,
        created_by: Principal::User(UserId::new()),
        created_at: Timestamp(1),
        updated_at: Timestamp(1),
    }
}

/// Runs one pass a minute from `from` with the given CPU values.
async fn run(e: &Env, from: i64, values: &[f64]) -> PassReport {
    let mut total = PassReport::default();
    for (i, v) in values.iter().enumerate() {
        let at = (from + i64::try_from(i).unwrap()) * MIN;
        e.source.set("node.cpu.percent", e.node, at, *v);
        let r = e.eval.pass(Timestamp(at)).await.unwrap();
        total.fired += r.fired;
        total.resolved += r.resolved;
        total.ended += r.ended;
    }
    total
}

async fn alarms(e: &Env) -> Vec<forge_core::alerts::Alarm> {
    e.store.list_alarms(&AlarmFilter::default(), 50).await.unwrap()
}

#[tokio::test]
async fn a_sustained_breach_opens_one_alarm_and_recovery_resolves_it() {
    let e = env().await;
    let rule = cpu_rule(e.ws);
    e.store.create_rule(&rule).await.unwrap();
    let r = run(&e, 0, &[90.0; 5]).await;
    assert_eq!(r.fired, 0, "5 samples span 4 minutes");
    let r = run(&e, 5, &[90.0; 5]).await;
    assert_eq!(r.fired, 1);
    let a = alarms(&e).await;
    assert_eq!(a.len(), 1);
    assert_eq!((a[0].state, a[0].resource.id.clone()), (AlarmState::Firing, e.node.to_string()));
    assert_eq!(a[0].fired_value, json!({"type": "num", "value": 90.0}));
    // Still above the recovery threshold: still one firing alarm.
    run(&e, 10, &[80.0; 5]).await;
    assert_eq!(alarms(&e).await.iter().filter(|a| a.state == AlarmState::Firing).count(), 1);
    let r = run(&e, 15, &[50.0; 3]).await;
    assert_eq!(r.resolved, 1);
    // A recurrence is a new episode linked to the first.
    run(&e, 20, &[95.0; 7]).await;
    let all = alarms(&e).await;
    assert_eq!(all.len(), 2);
    assert_eq!(all[0].previous, Some(all[1].id));
    assert_eq!(all[1].state, AlarmState::Resolved);
}

#[tokio::test]
async fn trackers_survive_a_restarted_evaluator() {
    let e = env().await;
    e.store.create_rule(&cpu_rule(e.ws)).await.unwrap();
    run(&e, 0, &[90.0; 5]).await;
    // A new evaluator over the same store continues the 5-minute timer.
    let fresh = Evaluator {
        alerts: e.store.clone(),
        state: e.store.clone(),
        source: e.source.clone(),
        resolver: e.store.clone(),
    };
    e.source.set("node.cpu.percent", e.node, 5 * MIN, 90.0);
    assert_eq!(fresh.pass(Timestamp(5 * MIN)).await.unwrap().fired, 1);
}

#[tokio::test]
async fn other_workspaces_and_scopes_are_not_evaluated() {
    let e = env().await;
    let mut other = cpu_rule(WorkspaceId::new());
    other.name = "elsewhere".into();
    e.store.create_rule(&other).await.unwrap();
    let mut narrow = cpu_rule(e.ws);
    narrow.scope = RuleScope::Servers { ids: vec![ServerId::new()] };
    e.store.create_rule(&narrow).await.unwrap();
    run(&e, 0, &[99.0; 10]).await;
    assert!(alarms(&e).await.is_empty(), "a rule never sees another workspace's or out-of-scope nodes");
    // Unknown resources are dropped too.
    e.source.set("node.cpu.percent", ServerId::new(), 11 * MIN, 99.0);
    assert_eq!(e.eval.pass(Timestamp(11 * MIN)).await.unwrap().series, 0);
}

#[tokio::test]
async fn silence_does_not_resolve_and_rule_changes_end_the_alarm() {
    let e = env().await;
    let mut rule = cpu_rule(e.ws);
    e.store.create_rule(&rule).await.unwrap();
    run(&e, 0, &[90.0; 7]).await;
    assert_eq!(alarms(&e).await[0].state, AlarmState::Firing);
    // The node stops reporting: passes continue, nothing resolves.
    for m in 10..40 {
        e.eval.pass(Timestamp(m * MIN)).await.unwrap();
    }
    assert_eq!(alarms(&e).await[0].state, AlarmState::Firing, "unknown is not recovery");
    // An edit ends the alarm explicitly (no fabricated recovery).
    rule.revision = 2;
    e.store.update_rule(&rule).await.unwrap();
    let r = e.eval.pass(Timestamp(41 * MIN)).await.unwrap();
    assert_eq!(r.ended, 1);
    let a = &alarms(&e).await[0];
    assert_eq!((a.state, a.end_reason.as_deref()), (AlarmState::Ended, Some("the rule changed")));
    // Deleting the rule forgets its trackers.
    e.store.delete_rule(rule.id).await.unwrap();
    e.eval.pass(Timestamp(42 * MIN)).await.unwrap();
    assert!(e.store.trackers().await.unwrap().is_empty());
}

#[tokio::test]
async fn invalid_rules_are_skipped_not_fatal() {
    let e = env().await;
    let mut bad = cpu_rule(e.ws);
    bad.spec = json!({"trigger": "nonsense"});
    e.store.create_rule(&bad).await.unwrap();
    e.store.create_rule(&cpu_rule(e.ws)).await.unwrap();
    let r = e.eval.pass(Timestamp(0)).await.unwrap();
    assert_eq!(r.invalid_rules, 1);
}

#[test]
fn installation_and_certificate_scopes_match_their_own_kind() {
    let inst = EntityRef::new(EntityKind::Installation, "i1");
    let cert = EntityRef::new(EntityKind::Certificate, "c1");
    let node = EntityRef::new(EntityKind::Server, ServerId::new());
    let all_inst = RuleScope::Installations { ids: vec![] };
    assert!(in_scope(&all_inst, &inst));
    assert!(!in_scope(&all_inst, &node), "a node is not an installation");
    assert!(!in_scope(&all_inst, &cert));
    assert!(in_scope(&RuleScope::Installations { ids: vec!["i1".into()] }, &inst));
    assert!(!in_scope(&RuleScope::Installations { ids: vec!["i2".into()] }, &inst));
    assert!(in_scope(&RuleScope::Certificates { ids: vec![] }, &cert));
    assert!(!in_scope(&RuleScope::Certificates { ids: vec!["c2".into()] }, &cert));
    assert!(!in_scope(&RuleScope::Servers { ids: vec![] }, &inst));
    assert_eq!(RuleScope::Installations { ids: vec!["a".into(), "b".into()] }.len(), 2);
    assert!(RuleScope::Certificates { ids: vec![] }.is_empty());
}
