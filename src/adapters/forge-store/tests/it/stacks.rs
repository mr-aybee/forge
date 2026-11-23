//! Compose stack persistence: stacks, revisions and per-node parts.
#![allow(clippy::unwrap_used)]

use forge_core::model::*;
use forge_core::ports::*;
use forge_core::stacks::{Stack, StackKind, StackNode, StackRepository, StackRevision};
use forge_core::*;
use forge_store::Store;
use serde_json::json;

async fn seed() -> (Store, tempfile::TempDir, WorkspaceId, EnvironmentId, ServerId) {
    let dir = tempfile::tempdir().unwrap();
    let s = Store::open_sqlite(&dir.path().join("forge.db")).await.unwrap();
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
        created_at: Timestamp(1),
    };
    s.create_server(&server).await.unwrap();
    (s, dir, org.id, ns.id, server.id)
}

fn rev(stack: StackId, content: &str, at: i64) -> StackRevision {
    StackRevision {
        stack_id: stack,
        revision: 1,
        content: content.into(),
        sha256: format!("{:064x}", at),
        note: None,
        author: Principal::User(UserId::new()),
        created_at: Timestamp(at),
    }
}

#[tokio::test]
async fn stacks_revisions_and_nodes_round_trip() {
    let (store, _d, ws, ns, server) = seed().await;
    let stack = Stack {
        id: StackId::new(),
        workspace_id: ws,
        environment_id: ns,
        name: "shop".into(),
        kind: StackKind::Compose,
        default_server: server,
        dir: None,
        revision: 1,
        deployed_revision: 0,
        env_sealed: Some("sealed".into()),
        env_keys: vec!["DB_PASSWORD".into()],
        created_by: Principal::User(UserId::new()),
        created_at: Timestamp(10),
        updated_at: Timestamp(10),
    };
    store.create_stack(&stack, &rev(stack.id, "services: {}", 10)).await.unwrap();
    // Names are unique per environment.
    let mut dup = stack.clone();
    dup.id = StackId::new();
    assert!(store.create_stack(&dup, &rev(dup.id, "x", 11)).await.is_err());

    let got = store.stack_by_name(ns, "shop").await.unwrap().unwrap();
    assert_eq!(got, stack);
    assert_eq!(store.list_stacks(Some(ns)).await.unwrap().len(), 1);

    assert_eq!(store.add_revision(&rev(stack.id, "v2", 20)).await.unwrap(), 2);
    assert_eq!(store.add_revision(&rev(stack.id, "v3", 30)).await.unwrap(), 3);
    assert_eq!(store.get_stack(stack.id).await.unwrap().unwrap().revision, 3);
    let list = store.list_revisions(stack.id).await.unwrap();
    assert_eq!(list.iter().map(|r| r.revision).collect::<Vec<_>>(), [3, 2, 1]);
    assert!(list[0].content.is_empty(), "lists carry no content");
    assert_eq!(store.get_revision(stack.id, 2).await.unwrap().unwrap().content, "v2");
    assert!(store.get_revision(stack.id, 9).await.unwrap().is_none());

    let mut s2 = got.clone();
    s2.deployed_revision = 3;
    s2.dir = Some("/srv/data/shop".into());
    s2.updated_at = Timestamp(40);
    store.put_stack(&s2).await.unwrap();
    let back = store.get_stack(stack.id).await.unwrap().unwrap();
    assert_eq!((back.deployed_revision, back.dir.as_deref(), back.revision), (3, Some("/srv/data/shop"), 3));

    let node = StackNode {
        stack_id: stack.id,
        server_id: server,
        services: vec!["web".into(), "db".into()],
        selected: vec!["web".into()],
        revision: 3,
        status: "deployed".into(),
        state: json!([{ "service": "web", "state": "running" }]),
        last_job_id: Some(JobId::new()),
        last_error: None,
        updated_at: Timestamp(50),
    };
    store.put_node(&node).await.unwrap();
    let mut n2 = node.clone();
    n2.status = "failed".into();
    n2.last_error = Some("boom".into());
    store.put_node(&n2).await.unwrap();
    assert_eq!(store.list_nodes(stack.id).await.unwrap(), vec![n2.clone()]);
    assert_eq!(store.nodes_on_server(server).await.unwrap(), vec![n2]);

    store.delete_stack(stack.id).await.unwrap();
    assert!(store.get_stack(stack.id).await.unwrap().is_none());
    assert!(store.list_nodes(stack.id).await.unwrap().is_empty());
    assert!(store.get_revision(stack.id, 1).await.unwrap().is_none());
}

/// Revisions saved while other writes commit (agent heartbeats, jobs, a
/// second editor) all succeed with distinct numbers. A read-then-write
/// transaction would fail on SQLite with a busy snapshot error instead.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn concurrent_revisions_get_distinct_numbers() {
    let (store, _d, ws, ns, server) = seed().await;
    let stack = Stack {
        id: StackId::new(),
        workspace_id: ws,
        environment_id: ns,
        name: "shop".into(),
        kind: StackKind::Compose,
        default_server: server,
        dir: None,
        revision: 1,
        deployed_revision: 0,
        env_sealed: None,
        env_keys: vec![],
        created_by: Principal::User(UserId::new()),
        created_at: Timestamp(10),
        updated_at: Timestamp(10),
    };
    store.create_stack(&stack, &rev(stack.id, "v1", 10)).await.unwrap();
    const N: i64 = 24;
    let mut tasks = Vec::new();
    for i in 0..N {
        let s = store.clone();
        let id = stack.id;
        tasks.push(tokio::spawn(async move { s.add_revision(&rev(id, &format!("v{}", i + 2), 100 + i)).await }));
        // Unrelated writers committing in between.
        let s = store.clone();
        let mut other = stack.clone();
        tasks.push(tokio::spawn(async move {
            other.updated_at = Timestamp(1000 + i);
            s.put_stack(&other).await.map(|()| -1)
        }));
    }
    let mut numbers = Vec::new();
    for t in tasks {
        let n = t.await.unwrap().unwrap();
        if n > 0 {
            numbers.push(n);
        }
    }
    numbers.sort_unstable();
    assert_eq!(numbers, (2..=N + 1).collect::<Vec<_>>());
    assert_eq!(store.get_stack(stack.id).await.unwrap().unwrap().revision, N + 1);
    assert_eq!(store.list_revisions(stack.id).await.unwrap().len() as i64, N + 1);
}

fn node(ws: WorkspaceId, hostname: &str) -> Server {
    Server {
        role: NodeRole::Worker,
        display_name: None,
        id: ServerId::new(),
        workspace_id: ws,
        environment_id: None,
        cluster_id: None,
        hostname: hostname.into(),
        os: "linux".into(),
        arch: "x86_64".into(),
        agent_version: "0".into(),
        status: AgentStatus::Online,
        last_seen_at: None,
        cert_serial: Some("ab12".into()),
        cert_not_after: None,
        created_at: Timestamp(1),
    }
}

fn part(stack: StackId, server: ServerId) -> StackNode {
    StackNode {
        stack_id: stack,
        server_id: server,
        services: vec!["web".into()],
        selected: vec![],
        revision: 1,
        status: "deployed".into(),
        state: json!([]),
        last_job_id: None,
        last_error: None,
        updated_at: Timestamp(5),
    }
}

/// Removing a node deletes it and what hangs off it. A stack that used it
/// as default moves to another of its nodes; one on that node alone goes.
/// The revocation entry stays and the name is free for a fresh node.
#[tokio::test]
async fn delete_server_removes_node_data_and_frees_the_name() {
    let (store, _d, ws, ns, gone) = seed().await;
    let other = node(ws, "lab-c");
    store.create_server(&other).await.unwrap();
    let mk = |name: &str| Stack {
        id: StackId::new(),
        workspace_id: ws,
        environment_id: ns,
        name: name.into(),
        kind: StackKind::Compose,
        default_server: gone,
        dir: None,
        revision: 1,
        deployed_revision: 0,
        env_sealed: None,
        env_keys: vec![],
        created_by: Principal::User(UserId::new()),
        created_at: Timestamp(1),
        updated_at: Timestamp(1),
    };
    let shared = mk("shared");
    let alone = mk("alone");
    for s in [&shared, &alone] {
        store.create_stack(s, &rev(s.id, "services: {}", 1)).await.unwrap();
        store.put_node(&part(s.id, gone)).await.unwrap();
    }
    store.put_node(&part(shared.id, other.id)).await.unwrap();
    store.set_node_label(gone, ws, "lab-b", true, Timestamp(2)).await.unwrap();
    store.revoke_certificate("ab12", gone, Timestamp(3)).await.unwrap();

    store.delete_server(gone, Timestamp(9)).await.unwrap();

    assert!(store.get_server(gone).await.is_err());
    assert!(store.find_server_by_hostname(ws, "lab-b").await.unwrap().is_none());
    assert!(store.is_certificate_revoked("ab12").await.unwrap());
    let moved = store.get_stack(shared.id).await.unwrap().unwrap();
    assert_eq!(moved.default_server, other.id);
    assert_eq!(store.list_nodes(shared.id).await.unwrap().len(), 1);
    assert!(store.get_stack(alone.id).await.unwrap().is_none());
    assert!(store.nodes_on_server(gone).await.unwrap().is_empty());
    assert!(store.list_node_labels(Some(ws)).await.unwrap().iter().all(|(id, _, _)| *id != gone));
    // A second delete finds nothing.
    assert!(store.delete_server(gone, Timestamp(10)).await.is_err());

    // The same machine enrolls again as a fresh node and gets its name back.
    let fresh = node(ws, "lab-b");
    store.create_server(&fresh).await.unwrap();
    store.set_node_label(fresh.id, ws, "lab-b", false, Timestamp(11)).await.unwrap();
    assert_eq!(store.find_server_by_hostname(ws, "lab-b").await.unwrap().unwrap().id, fresh.id);
}
