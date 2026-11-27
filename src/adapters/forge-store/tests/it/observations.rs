//! Observation storage: latest values, history, the transition feed,
//! idempotent replay and retention (migration 0018, D-085).
#![allow(clippy::unwrap_used)]

use forge_core::model::*;
use forge_core::observation::*;
use forge_core::ports::*;
use forge_core::*;
use forge_store::Store;

async fn store() -> (Store, tempfile::TempDir) {
    let dir = tempfile::tempdir().unwrap();
    let s = Store::open_sqlite(&dir.path().join("forge.db")).await.unwrap();

    (s, dir)
}

fn key(subject: &str, server: Option<ServerId>) -> ObservationKey {
    ObservationKey {
        resource: EntityRef::new(EntityKind::Server, "node-1"),
        server,
        subject: Subject::new(subject).unwrap(),
    }
}

fn latest(k: &ObservationKey, v: HealthState, at: i64) -> Latest {
    Latest {
        key: k.clone(),
        value: ObservedValue::Health(v),
        reason: (v == HealthState::Failing).then_some(ReasonCode::Timeout),
        detail: Some("connect timed out after 2s".into()),
        source: ObservationSource::Probe { policy: "p1".into(), check: "web".into() },
        executor: None,
        generation: Some("gen-1".into()),
        observed_at: Timestamp(at),
        received_at: Timestamp(at),
        fresh_until: Timestamp(at + 10_000),
        skewed: false,
        changed_at: Timestamp(at),
    }
}

fn transition(k: &ObservationKey, from: Option<HealthState>, to: HealthState, at: i64) -> Transition {
    Transition {
        seq: 0,
        key: k.clone(),
        from: from.map(ObservedValue::Health),
        to: ObservedValue::Health(to),
        reason: None,
        generation: Some("gen-1".into()),
        at: Timestamp(at),
        evidence_at: Timestamp(at),
    }
}

fn batch(k: &ObservationKey, from: Option<HealthState>, to: HealthState, at: i64) -> ObservationBatch {
    let l = latest(k, to, at);
    ObservationBatch {
        latest: vec![l.clone()],
        history: vec![l],
        transitions: vec![transition(k, from, to, at)],
        stale_noted: vec![],
    }
}

#[tokio::test]
async fn values_round_trip_and_transitions_get_increasing_seqs() {
    let (s, _d) = store().await;
    let server = ServerId::new();
    let k = key("health.readiness.web", Some(server));
    let t1 = s.apply(&batch(&k, None, HealthState::Passing, 1_000)).await.unwrap();
    let t2 = s.apply(&batch(&k, Some(HealthState::Passing), HealthState::Failing, 2_000)).await.unwrap();
    assert_eq!((t1[0].seq, t2[0].seq), (1, 2));

    let got = s.latest_for(std::slice::from_ref(&k)).await.unwrap();
    assert_eq!(got, vec![latest(&k, HealthState::Failing, 2_000)]);
    assert_eq!(s.head().await.unwrap(), 2);
    let feed = s.since(0, 10).await.unwrap();
    assert_eq!(feed.len(), 2);
    assert_eq!(feed[1].from, Some(ObservedValue::Health(HealthState::Passing)));
    assert_eq!(feed[1].key.server, Some(server));
    assert!(s.since(2, 10).await.unwrap().is_empty());
    let hist = s.history(&k, Timestamp(0), 10).await.unwrap();
    assert_eq!(hist.len(), 2);
    assert_eq!(hist[0].value, ObservedValue::Health(HealthState::Failing), "newest first");
}

#[tokio::test]
async fn replaying_a_batch_adds_no_duplicates() {
    // A crash after commit but before the agent saw the ack resends the batch.
    let (s, _d) = store().await;
    let k = key("health.liveness.web", None);
    let b = batch(&k, None, HealthState::Passing, 1_000);
    assert_eq!(s.apply(&b).await.unwrap().len(), 1);
    assert!(s.apply(&b).await.unwrap().is_empty(), "replayed transition stored once");
    assert_eq!(s.head().await.unwrap(), 1);
    assert_eq!(s.history(&k, Timestamp(0), 10).await.unwrap().len(), 1);
}

#[tokio::test]
async fn stale_values_are_listed_until_noted() {
    let (s, _d) = store().await;
    let k = key("health.readiness.web", None);
    s.apply(&batch(&k, None, HealthState::Passing, 1_000)).await.unwrap();
    assert!(s.stale_since(Timestamp(10_999), 10).await.unwrap().is_empty());
    let stale = s.stale_since(Timestamp(11_000), 10).await.unwrap();
    assert_eq!(stale.len(), 1);
    let t = stale_transition(&stale[0], Timestamp(11_000)).unwrap();
    s.apply(&ObservationBatch { transitions: vec![t], stale_noted: vec![k.clone()], ..Default::default() })
        .await
        .unwrap();
    assert!(s.stale_since(Timestamp(20_000), 10).await.unwrap().is_empty());

    // A fresh value clears the mark.
    s.apply(&batch(&k, Some(HealthState::Unknown), HealthState::Passing, 30_000)).await.unwrap();
    assert_eq!(s.stale_since(Timestamp(40_000), 10).await.unwrap().len(), 1);
}

#[tokio::test]
async fn the_filter_selects_by_resource_server_and_subject_prefix() {
    let (s, _d) = store().await;
    let (a, b) = (ServerId::new(), ServerId::new());

    for (subj, srv) in [("health.readiness.web", a), ("health.liveness.web", a), ("node.reachable", b)] {
        s.apply(&batch(&key(subj, Some(srv)), None, HealthState::Passing, 1_000)).await.unwrap();
    }

    let f = |server, prefix: Option<&str>| ObservationFilter {
        resource: Some(EntityRef::new(EntityKind::Server, "node-1")),
        server,
        subject_prefix: prefix.map(str::to_owned),
    };
    assert_eq!(s.latest(&f(None, None), 10).await.unwrap().len(), 3);
    assert_eq!(s.latest(&f(Some(a), None), 10).await.unwrap().len(), 2);
    assert_eq!(s.latest(&f(None, Some("health.")), 10).await.unwrap().len(), 2);

    // `_` in a prefix is literal, not a LIKE wildcard.
    assert!(s.latest(&f(None, Some("health_")), 10).await.unwrap().is_empty());
    assert_eq!(s.latest(&f(None, None), 1).await.unwrap().len(), 1, "limit applies");
}

#[tokio::test]
async fn retention_keeps_the_newest_transition_so_seqs_never_restart() {
    let (s, _d) = store().await;
    let k = key("health.readiness.web", None);
    s.apply(&batch(&k, None, HealthState::Passing, 1_000)).await.unwrap();
    s.apply(&batch(&k, Some(HealthState::Passing), HealthState::Failing, 2_000)).await.unwrap();
    s.sweep(Timestamp(5_000), i64::MAX).await.unwrap();
    assert_eq!(s.head().await.unwrap(), 2, "newest row kept");
    assert!(s.history(&k, Timestamp(0), 10).await.unwrap().is_empty());
    let t = s.apply(&batch(&k, Some(HealthState::Failing), HealthState::Passing, 6_000)).await.unwrap();
    assert_eq!(t[0].seq, 3);
}

/// A workspace with a node and a service placed on it.
async fn seeded() -> (Store, tempfile::TempDir, ServerId, ServiceId) {
    let (s, d) = store().await;
    let org = Workspace { id: WorkspaceId::new(), name: "Acme".into(), slug: "acme".into(), created_at: Timestamp(1) };
    s.create_workspace(&org).await.unwrap();
    let ns = Environment {
        id: EnvironmentId::new(),
        workspace_id: org.id,
        cluster_id: None,
        project_id: None,
        name: "web".into(),
        protected: false,
        created_at: Timestamp(1),
    };
    s.create_environment(&ns).await.unwrap();
    let server = Server {
        role: NodeRole::Worker,
        display_name: None,
        id: ServerId::new(),
        workspace_id: org.id,
        environment_id: None,
        cluster_id: None,
        hostname: "lab-b".into(),
        os: "linux".into(),
        arch: "x86_64".into(),
        agent_version: "0".into(),
        status: AgentStatus::Online,
        last_seen_at: None,
        cert_serial: None,
        cert_not_after: None,
        drained: false,
        created_at: Timestamp(1),
    };
    s.create_server(&server).await.unwrap();
    let app = Service {
        id: ServiceId::new(),
        workspace_id: org.id,
        environment_id: ns.id,
        name: "api".into(),
        runtime: "systemd".into(),
        spec: serde_json::json!({}),
        env: vec![],
        created_by: Principal::User(UserId::new()),
        created_at: Timestamp(1),
        updated_at: Timestamp(1),
        version: 0,
        source: String::new(),
        secrets_changed_at: None,
        adopted_from: None,
    };
    s.put_service(&app).await.unwrap();
    s.put_instance(&ServiceInstance {
        service_id: app.id,
        server_id: server.id,
        role: "active".into(),
        status: "deployed".into(),
        state: None,
        last_job_id: None,
        last_error: None,
        updated_at: Timestamp(1),
        desired: forge_core::model::InstanceDesired::Running,
        deployed_release: None,
        exit_code: None,
    })
    .await
    .unwrap();

    (s, d, server.id, app.id)
}

#[tokio::test]
async fn placements_answer_only_for_the_node_an_instance_runs_on() {
    let (s, _d, server, app) = seeded().await;
    assert!(s.is_placed_on(&app.to_string(), server).await.unwrap());
    assert!(!s.is_placed_on(&app.to_string(), ServerId::new()).await.unwrap());
    assert!(!s.is_placed_on(&ServiceId::new().to_string(), server).await.unwrap());
}

#[tokio::test]
async fn observations_of_deleted_resources_are_removed() {
    let (s, _d, server, app) = seeded().await;
    let node_key = ObservationKey {
        resource: EntityRef::new(EntityKind::Server, server),
        server: None,
        subject: Subject::new("health.readiness.node").unwrap(),
    };
    let app_key = ObservationKey {
        resource: EntityRef::new(EntityKind::Service, app),
        server: Some(server),
        subject: Subject::new("health.readiness.api").unwrap(),
    };
    let gone_key = ObservationKey {
        resource: EntityRef::new(EntityKind::Server, ServerId::new()),
        server: None,
        subject: Subject::new("health.readiness.node").unwrap(),
    };

    for k in [&node_key, &app_key, &gone_key] {
        s.apply(&batch(k, None, HealthState::Passing, 1_000)).await.unwrap();
    }

    assert_eq!(s.delete_orphans().await.unwrap(), 2, "the unknown node's latest and history rows");
    s.delete_service(app).await.unwrap();
    assert_eq!(s.delete_orphans().await.unwrap(), 2, "the deleted app's rows");
    let left = s.latest_for(&[node_key.clone(), app_key, gone_key]).await.unwrap();
    assert_eq!(left.len(), 1);
    assert_eq!(left[0].key, node_key);
    assert_eq!(s.head().await.unwrap(), 3, "transitions stay");
}

#[tokio::test]
async fn reconciler_columns_round_trip_and_status_swaps_are_conditional() {
    let (s, _d, server, app) = seeded().await;
    let mut i = s.list_instances(app).await.unwrap().remove(0);
    assert_eq!((i.desired, i.deployed_release.as_deref(), i.exit_code), (InstanceDesired::Running, None, None));

    i.desired = InstanceDesired::Stopped;
    i.deployed_release = Some("4".into());
    s.put_instance(&i).await.unwrap();

    let seen = InstanceObservation { state: Some("exited".into()), exit_code: Some(137), at: Timestamp(5) };
    assert!(s.observe_instance(app, server, &seen).await.unwrap());
    assert!(!s.observe_instance(app, ServerId::new(), &seen).await.unwrap(), "no such instance");

    let got = s.list_instances(app).await.unwrap().remove(0);
    assert_eq!(
        (got.desired, got.deployed_release.as_deref(), got.state.as_deref(), got.exit_code, got.status.as_str()),
        (InstanceDesired::Stopped, Some("4"), Some("exited"), Some(137), "deployed")
    );

    let swap = |from: &str, to: &str, error: Option<&str>| StatusSwap {
        from: from.into(),
        to: to.into(),
        error: error.map(str::to_owned),
        at: Timestamp(6),
    };
    assert!(!s.swap_instance_status(app, server, &swap("failed", "stranded", None)).await.unwrap());
    assert!(s.swap_instance_status(app, server, &swap("deployed", "stranded", Some("offline"))).await.unwrap());
    let got = s.list_instances(app).await.unwrap().remove(0);
    assert_eq!((got.status.as_str(), got.last_error.as_deref()), ("stranded", Some("offline")));
    assert!(s.swap_instance_status(app, server, &swap("stranded", "deployed", Some(""))).await.unwrap());
    assert_eq!(s.list_instances(app).await.unwrap()[0].last_error, None, "an empty error clears it");

    s.set_drained(server, true).await.unwrap();
    assert!(s.get_server(server).await.unwrap().drained);
}
