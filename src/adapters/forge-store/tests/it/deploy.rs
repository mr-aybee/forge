//! Releases, the deployment lock, events and slot ports (#73).

use forge_core::deploy::*;
use forge_core::model::EnvVar;
use forge_core::*;
use forge_store::Store;
use serde_json::json;

async fn store() -> (Store, tempfile::TempDir) {
    let dir = tempfile::tempdir().unwrap();
    let s = Store::open_sqlite(&dir.path().join("forge.db")).await.unwrap();
    (s, dir)
}

fn release(app: ServiceId) -> Release {
    Release {
        id: ReleaseId::new(),
        service_id: app,
        workspace_id: WorkspaceId::new(),
        number: 0,
        spec: json!({ "image": "nginx:1" }),
        env: vec![EnvVar { key: "TOKEN".into(), value: None, secret: true, sealed: Some("sealed-blob".into()) }],
        artifact: json!({ "kind": "image", "image": "nginx:1", "digest": null }),
        policy: json!({}),
        source: Some("apiVersion: forge/v1".into()),
        source_sha256: Some("ab".repeat(32)),
        description: "first".into(),
        created_by: Principal::User(UserId::new()),
        created_at: Timestamp(1),
    }
}

fn deployment(app: ServiceId, rel: &Release) -> Deployment {
    Deployment {
        id: DeploymentId::new(),
        service_id: app,
        workspace_id: rel.workspace_id,
        release_id: rel.id,
        release_number: rel.number,
        from_release: 0,
        kind: DeploymentKind::Apply,
        strategy: "rolling".into(),
        state: DeploymentState::Pending,
        request: None,
        job_id: None,
        plan: json!({ "steps": [] }),
        step: 0,
        error: None,
        created_by: rel.created_by.clone(),
        created_at: Timestamp(2),
        updated_at: Timestamp(2),
        finished_at: None,
    }
}

#[tokio::test]
async fn releases_are_numbered_per_service_and_keep_sealed_secrets() {
    let (s, _d) = store().await;
    let app = ServiceId::new();
    let a = release(app);
    assert_eq!(s.create_release(&a).await.unwrap(), 1);
    assert_eq!(s.create_release(&release(app)).await.unwrap(), 2);
    assert_eq!(s.create_release(&release(ServiceId::new())).await.unwrap(), 1, "another service counts alone");
    let back = s.release_by_number(app, 1).await.unwrap().unwrap();
    assert_eq!(back.env[0].sealed.as_deref(), Some("sealed-blob"), "kept at rest");
    assert_eq!(back.source.as_deref(), Some("apiVersion: forge/v1"));
    let list = s.list_releases(app, 10).await.unwrap();
    assert_eq!(list.iter().map(|r| r.number).collect::<Vec<_>>(), [2, 1]);
    assert!(list[0].source.is_none(), "lists leave the document out");
    for _ in 0..3 {
        s.create_release(&release(app)).await.unwrap();
    }
    // Keep the newest two and release 1 (it serves).
    assert_eq!(s.prune_releases(app, 2, &[1]).await.unwrap(), 2);
    let left: Vec<i64> = s.list_releases(app, 10).await.unwrap().iter().map(|r| r.number).collect();
    assert_eq!(left, [5, 4, 1]);
}

#[tokio::test]
async fn one_active_deployment_per_service() {
    let (s, _d) = store().await;
    let app = ServiceId::new();
    let mut rel = release(app);
    rel.number = s.create_release(&rel).await.unwrap();
    let mut first = deployment(app, &rel);
    s.create_deployment(&first).await.unwrap();
    let second = deployment(app, &rel);
    match s.create_deployment(&second).await {
        Err(DomainError::Conflict(m)) => assert!(m.contains("already has a deployment"), "{m}"),
        other => panic!("{other:?}"),
    }
    assert!(s.get_deployment(second.id).await.unwrap().is_none(), "nothing half-written");
    assert_eq!(s.active_deployment(app).await.unwrap().unwrap().id, first.id);
    s.request_deployment(first.id, DeploymentRequest::Pause, Timestamp(3)).await.unwrap();
    assert_eq!(s.get_deployment(first.id).await.unwrap().unwrap().request, Some(DeploymentRequest::Pause));
    // Saving the job's view never drops a request made meanwhile.
    s.put_deployment(&first).await.unwrap();
    assert_eq!(s.get_deployment(first.id).await.unwrap().unwrap().request, Some(DeploymentRequest::Pause));
    s.clear_request(first.id).await.unwrap();
    assert_eq!(s.get_deployment(first.id).await.unwrap().unwrap().request, None);
    // Finishing frees the service.
    first.state = DeploymentState::Succeeded;
    first.finished_at = Some(Timestamp(4));
    s.put_deployment(&first).await.unwrap();
    assert!(s.active_deployment(app).await.unwrap().is_none());
    assert_eq!(s.current_release(app).await.unwrap(), rel.number);
    s.create_deployment(&second).await.unwrap();
    assert_eq!(s.list_deployments(app, 10).await.unwrap().len(), 2);
}

#[tokio::test]
async fn events_append_in_order() {
    let (s, _d) = store().await;
    let id = DeploymentId::new();
    for m in ["one", "two", "three"] {
        let e = DeploymentEvent {
            deployment_id: id,
            seq: 0,
            at: Timestamp(1),
            level: "info".into(),
            server_id: None,
            message: m.into(),
        };
        s.add_event(&e).await.unwrap();
    }
    let all = s.list_events(id, 0, 10).await.unwrap();
    assert_eq!(
        all.iter().map(|e| (e.seq, e.message.as_str())).collect::<Vec<_>>(),
        [(1, "one"), (2, "two"), (3, "three")]
    );
    assert_eq!(s.list_events(id, 2, 10).await.unwrap().len(), 1);
}

#[tokio::test]
async fn slot_ports_are_stable_and_unique_across_services() {
    let (s, _d) = store().await;
    let (a, b) = (ServiceId::new(), ServiceId::new());
    let pa = s.slot_ports(a, "a", 2, (20_000, 20_010)).await.unwrap();
    assert_eq!(pa, [20_000, 20_001]);
    assert_eq!(s.slot_ports(a, "a", 2, (20_000, 20_010)).await.unwrap(), pa, "the same ports next time");
    let pb = s.slot_ports(b, "a", 1, (20_000, 20_010)).await.unwrap();
    assert_eq!(pb, [20_002]);
    assert_eq!(s.slot_ports(a, "b", 1, (20_000, 20_010)).await.unwrap(), [20_003]);
    // More ports later (a port mapping was added) keep the old ones.
    assert_eq!(s.slot_ports(a, "a", 3, (20_000, 20_010)).await.unwrap(), [20_000, 20_001, 20_004]);
    assert!(s.slot_ports(b, "b", 10, (20_000, 20_010)).await.is_err(), "the pool runs out");

    let slot = SlotRecord {
        service_id: a,
        server_id: ServerId::new(),
        slot: "a".into(),
        release_number: 1,
        weight: 100,
        state: "ready".into(),
        ports: json!({ "8080": 20_000 }),
        updated_at: Timestamp(1),
    };
    s.put_slot(&slot).await.unwrap();
    s.put_slot(&SlotRecord { weight: 0, ..slot.clone() }).await.unwrap();
    let got = s.list_slots_of(&[a, b]).await.unwrap();
    assert_eq!((got.len(), got[0].weight), (1, 0));
    s.delete_slot(a, slot.server_id, "a").await.unwrap();
    assert!(s.list_slots(a).await.unwrap().is_empty());
}
