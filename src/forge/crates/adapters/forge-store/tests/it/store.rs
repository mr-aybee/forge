#![allow(clippy::unwrap_used)]

use forge_core::model::*;
use forge_core::ports::*;
use forge_core::*;
use forge_store::Store;
use serde_json::json;

async fn store() -> (Store, tempfile::TempDir) {
    let dir = tempfile::tempdir().unwrap();
    let s = Store::open_sqlite(&dir.path().join("forge.db")).await.unwrap();

    (s, dir)
}

async fn seed_workspace(s: &Store) -> (Workspace, Environment) {
    let org = Workspace {
        id: WorkspaceId::new(),
        name: "Acme".into(),
        slug: format!("acme-{}", WorkspaceId::new()),
        created_at: Timestamp(1),
    };
    s.create_workspace(&org).await.unwrap();
    let env = Environment {
        id: EnvironmentId::new(),
        workspace_id: org.id,
        cluster_id: None,
        project_id: None,
        name: "prod".into(),
        protected: true,
        created_at: Timestamp(1),
    };
    s.create_environment(&env).await.unwrap();

    (org, env)
}

fn new_job(key: Option<&str>) -> NewJob {
    NewJob {
        kind: "test.kind".into(),
        workspace_id: None,
        resource: EntityRef::platform(),
        principal: Principal::User(UserId::new()),
        context: AuthzContext::default(),
        idempotency_key: key.map(str::to_owned),
        payload: json!({"n": 1}),
        max_attempts: 3,
    }
}

#[tokio::test]
async fn migrations_apply_and_tenancy_round_trips() {
    let (s, _d) = store().await;
    let (org, env) = seed_workspace(&s).await;
    assert_eq!(s.get_workspace(org.id).await.unwrap(), org);
    assert_eq!(s.list_environments(org.id, None).await.unwrap(), vec![env.clone()]);
    assert!(s.get_environment(env.id).await.unwrap().protected);
}

#[tokio::test]
async fn duplicate_slug_is_a_conflict() {
    let (s, _d) = store().await;
    let (org, _) = seed_workspace(&s).await;
    let dup = Workspace { id: WorkspaceId::new(), ..org };
    assert!(matches!(s.create_workspace(&dup).await, Err(DomainError::Conflict(_))));
}

#[tokio::test]
async fn resolver_builds_full_ancestry() {
    let (s, _d) = store().await;
    let (org, env) = seed_workspace(&s).await;
    let server = Server {
        role: forge_core::model::NodeRole::Worker,
        display_name: None,
        id: ServerId::new(),
        workspace_id: org.id,
        environment_id: Some(env.id),
        cluster_id: None,
        hostname: "web-01".into(),
        os: "linux".into(),
        arch: "x86_64".into(),
        agent_version: "0.1.0".into(),
        status: AgentStatus::Pending,
        last_seen_at: None,
        cert_serial: None,
        cert_not_after: None,
        drained: false,
        created_at: Timestamp(1),
    };
    s.create_server(&server).await.unwrap();
    let r = s.resolve(&EntityRef::new(EntityKind::Server, server.id)).await.unwrap();
    assert_eq!(
        r.ancestors,
        vec![EntityRef::new(EntityKind::Environment, env.id), EntityRef::new(EntityKind::Workspace, org.id),]
    );

    // Unknown ids do not resolve (fail closed).
    assert!(s.resolve(&EntityRef::new(EntityKind::Server, ServerId::new())).await.is_err());
}

#[tokio::test]
async fn idempotency_key_returns_existing_job() {
    let (s, _d) = store().await;
    let (a, created_a) = s.insert_job(new_job(Some("k1")), Timestamp(10)).await.unwrap();
    let (b, created_b) = s.insert_job(new_job(Some("k1")), Timestamp(11)).await.unwrap();
    assert!(created_a);
    assert!(!created_b);
    assert_eq!(a.id, b.id);
    let (_, created_c) = s.insert_job(new_job(None), Timestamp(12)).await.unwrap();
    let (_, created_d) = s.insert_job(new_job(None), Timestamp(12)).await.unwrap();
    assert!(created_c && created_d, "jobs without keys are never deduplicated");
}

#[tokio::test]
async fn claim_is_exclusive_and_fenced() {
    let (s, _d) = store().await;
    let (job, _) = s.insert_job(new_job(None), Timestamp(10)).await.unwrap();

    let a = s.claim_job("w1", Timestamp(20), Timestamp(1_020)).await.unwrap().unwrap();
    assert_eq!(a.id, job.id);
    assert!(s.claim_job("w2", Timestamp(21), Timestamp(1_021)).await.unwrap().is_none(), "lease held");

    // Lease expires: another worker takes over with a new fencing token.
    let b = s.claim_job("w2", Timestamp(2_000), Timestamp(3_000)).await.unwrap().unwrap();
    assert_eq!(b.lease_token, a.lease_token + 1);

    // The stale worker can no longer write.
    let stale = s.complete_step(job.id, a.lease_token, 0, &json!({}), Timestamp(2_001)).await;
    assert!(matches!(stale, Err(DomainError::LeaseLost(_))));
    s.start_step(job.id, b.lease_token, 0, "one", Timestamp(2_002)).await.unwrap();
    s.complete_step(job.id, b.lease_token, 0, &json!({"done": 1}), Timestamp(2_003)).await.unwrap();

    let j = s.get_job(job.id).await.unwrap();
    assert_eq!(j.current_step, 1);
    assert_eq!(j.state, json!({"done": 1}));
}

#[tokio::test]
async fn retry_requeues_with_delay_and_counts_attempts() {
    let (s, _d) = store().await;
    let (job, _) = s.insert_job(new_job(None), Timestamp(10)).await.unwrap();
    let c = s.claim_job("w", Timestamp(10), Timestamp(100)).await.unwrap().unwrap();
    s.start_step(job.id, c.lease_token, 0, "one", Timestamp(11)).await.unwrap();
    s.schedule_retry(job.id, c.lease_token, Timestamp(500), "boom", Timestamp(12)).await.unwrap();
    assert!(s.claim_job("w", Timestamp(100), Timestamp(200)).await.unwrap().is_none(), "not yet due");
    let again = s.claim_job("w", Timestamp(500), Timestamp(600)).await.unwrap().unwrap();
    assert_eq!(again.attempts, 1);
    assert_eq!(again.last_error.as_deref(), Some("boom"));
}

#[tokio::test]
async fn cancel_stops_running_job_writes() {
    let (s, _d) = store().await;
    let (job, _) = s.insert_job(new_job(None), Timestamp(10)).await.unwrap();
    let c = s.claim_job("w", Timestamp(10), Timestamp(100)).await.unwrap().unwrap();
    assert!(s.cancel_job(job.id, Timestamp(11)).await.unwrap());
    assert!(!s.cancel_job(job.id, Timestamp(12)).await.unwrap(), "already terminal");
    assert!(s.renew_lease(job.id, c.lease_token, Timestamp(200)).await.is_err());
}

#[tokio::test]
async fn enrollment_token_is_single_use_and_expires() {
    let (s, _d) = store().await;
    let (org, _) = seed_workspace(&s).await;
    let t = EnrollmentToken {
        id: "t1".into(),
        workspace_id: org.id,
        environment_id: None,
        token_hash: "h1".into(),
        created_by: Principal::User(UserId::new()),
        expires_at: Timestamp(1_000),
        used_at: None,
        server_id: None,
    };
    s.create_token(&t).await.unwrap();
    assert!(s.consume_token("h1", Timestamp(2_000), ServerId::new()).await.unwrap().is_none(), "expired");
    let t2 = EnrollmentToken { id: "t2".into(), token_hash: "h2".into(), expires_at: Timestamp(5_000), ..t };
    s.create_token(&t2).await.unwrap();
    assert!(s.consume_token("h2", Timestamp(100), ServerId::new()).await.unwrap().is_some());
    assert!(s.consume_token("h2", Timestamp(101), ServerId::new()).await.unwrap().is_none(), "reused");
}

#[tokio::test]
async fn audit_seq_is_unique() {
    let (s, _d) = store().await;
    let e = AuditEntry {
        seq: 1,
        id: "a".into(),
        at: Timestamp(1),
        actor: "user:x".into(),
        action: "test".into(),
        resource: "platform:root".into(),
        workspace_id: None,
        outcome: AuditOutcome::Success,
        details: json!({}),
        prev_hash: "0".into(),
        hash: "h".into(),
        signature: "s".into(),
    };
    s.insert_entry(&e).await.unwrap();
    let dup = AuditEntry { id: "b".into(), ..e };
    assert!(matches!(s.insert_entry(&dup).await, Err(DomainError::Conflict(_))));
}

#[tokio::test]
async fn cluster_membership_adds_second_parent() {
    use forge_core::ports::ClusterRepository;
    let (s, _d) = store().await;
    let (org, env) = seed_workspace(&s).await;
    let cluster = Cluster {
        id: ClusterId::new(),
        workspace_id: org.id,
        name: "edge".into(),
        description: String::new(),
        environment_id: None,
        abbr: String::new(),
        icon: None,
        mode: Default::default(),
        created_at: Timestamp(1),
    };
    s.create_cluster(&cluster).await.unwrap();
    let server = Server {
        role: forge_core::model::NodeRole::Worker,
        display_name: None,
        id: ServerId::new(),
        workspace_id: org.id,
        environment_id: Some(env.id),
        cluster_id: None,
        hostname: "n".into(),
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
    s.assign_server(server.id, Some(cluster.id), Some(env.id)).await.unwrap();
    let r = s.resolve(&EntityRef::new(EntityKind::Server, server.id)).await.unwrap();
    let parents = &r.parents.iter().find(|(e, _)| e == &r.entity).unwrap().1;
    assert!(parents.contains(&EntityRef::new(EntityKind::Cluster, cluster.id)));
    assert!(parents.contains(&EntityRef::new(EntityKind::Environment, env.id)));

    // Deleting a cluster unassigns its nodes.
    s.delete_cluster(cluster.id).await.unwrap();
    assert_eq!(s.get_server(server.id).await.unwrap().cluster_id, None);
}

#[tokio::test]
async fn expired_secrets_are_purged() {
    use forge_core::ports::SecretRepository;
    let (s, _d) = store().await;
    let (org, _) = seed_workspace(&s).await;
    let mk = |expires| SecretRecord {
        id: SecretId::new(),
        workspace_id: org.id,
        kind: "ssh.bootstrap".into(),
        label: "x".into(),
        sealed: "v1.sealed".into(),
        expires_at: expires,
        created_by: Principal::User(UserId::new()),
        created_at: Timestamp(1),
    };
    let (soon, kept) = (mk(Some(Timestamp(100))), mk(None));
    s.put_secret(&soon).await.unwrap();
    s.put_secret(&kept).await.unwrap();
    assert_eq!(s.purge_expired(Timestamp(50)).await.unwrap(), 0);
    assert_eq!(s.purge_expired(Timestamp(100)).await.unwrap(), 1);
    assert!(s.get_secret(soon.id).await.unwrap().is_none());
    assert!(s.get_secret(kept.id).await.unwrap().is_some());
}

#[tokio::test]
async fn resolve_many_matches_resolve_for_servers() {
    use forge_core::ports::ClusterRepository;
    let (s, _d) = store().await;
    let (org, env) = seed_workspace(&s).await;
    let cluster = Cluster {
        id: ClusterId::new(),
        workspace_id: org.id,
        name: "edge".into(),
        description: String::new(),
        environment_id: None,
        abbr: String::new(),
        icon: None,
        mode: Default::default(),
        created_at: Timestamp(1),
    };
    s.create_cluster(&cluster).await.unwrap();
    let mk = |host: &str, ns: Option<EnvironmentId>| Server {
        role: forge_core::model::NodeRole::Worker,
        display_name: None,
        id: ServerId::new(),
        workspace_id: org.id,
        environment_id: ns,
        cluster_id: None,
        hostname: host.into(),
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
    let (a, b, c) = (mk("a", Some(env.id)), mk("b", None), mk("c", Some(env.id)));

    for x in [&a, &b, &c] {
        s.create_server(x).await.unwrap();
    }

    s.assign_server(c.id, Some(cluster.id), Some(env.id)).await.unwrap();
    let mut refs: Vec<EntityRef> = [&a, &b, &c].iter().map(|x| EntityRef::new(EntityKind::Server, x.id)).collect();
    refs.push(EntityRef::new(EntityKind::Server, ServerId::new())); // unknown
    let many = s.resolve_many(&refs).await;
    assert_eq!(many.len(), 4);

    for (r, m) in refs.iter().zip(many) {
        match (s.resolve(r).await, m) {
            (Ok(one), Ok(batch)) => {
                assert_eq!(one.entity, batch.entity);
                let mut x = one.ancestors.clone();
                let mut y = batch.ancestors.clone();
                x.sort_by_key(ToString::to_string);
                y.sort_by_key(ToString::to_string);
                assert_eq!(x, y, "ancestors of {r}");
            }
            (Err(_), Err(_)) => {}
            (one, batch) => panic!("{r}: single {:?} vs batch {:?}", one.is_ok(), batch.is_ok()),
        }
    }
}

fn cluster_in(ws: WorkspaceId, name: &str) -> Cluster {
    Cluster {
        id: ClusterId::new(),
        workspace_id: ws,
        name: name.into(),
        description: String::new(),
        environment_id: None,
        abbr: String::new(),
        icon: None,
        mode: Default::default(),
        created_at: Timestamp(1),
    }
}

fn environment_in(ws: WorkspaceId, cluster: ClusterId, name: &str) -> Environment {
    Environment {
        id: EnvironmentId::new(),
        workspace_id: ws,
        cluster_id: Some(cluster),
        project_id: None,
        name: name.into(),
        protected: false,
        created_at: Timestamp(2),
    }
}

/// D-056: an environment name is unique inside its cluster, not across clusters,
/// and a move into a cluster that already has the name is refused.
#[tokio::test]
async fn environment_names_are_unique_per_cluster() {
    let (s, _d) = store().await;
    let (org, _) = seed_workspace(&s).await;
    let (a, b) = (cluster_in(org.id, "a"), cluster_in(org.id, "b"));
    s.create_cluster(&a).await.unwrap();
    s.create_cluster(&b).await.unwrap();
    let dev_a = environment_in(org.id, a.id, "dev");
    s.create_environment(&dev_a).await.unwrap();
    let dup = environment_in(org.id, a.id, "dev");
    assert!(matches!(s.create_environment(&dup).await, Err(DomainError::Conflict(_))));
    let dev_b = environment_in(org.id, b.id, "dev");
    s.create_environment(&dev_b).await.unwrap();
    let qa_b = environment_in(org.id, b.id, "qa");
    s.create_environment(&qa_b).await.unwrap();

    assert_eq!(s.list_environments(org.id, Some(b.id)).await.unwrap(), vec![dev_b.clone(), qa_b.clone()]);
    assert!(matches!(s.set_environment_cluster(dev_b.id, a.id).await, Err(DomainError::Conflict(_))));
    s.set_environment_cluster(qa_b.id, a.id).await.unwrap();
    assert_eq!(s.get_environment(qa_b.id).await.unwrap().cluster_id, Some(a.id));

    // The name is free again in b.
    s.create_environment(&environment_in(org.id, b.id, "qa")).await.unwrap();

    // The resolver: environment → workspace (the cluster is placement only).
    let r = s.resolve(&EntityRef::new(EntityKind::Environment, qa_b.id)).await.unwrap();
    assert_eq!(r.ancestors, vec![EntityRef::new(EntityKind::Workspace, org.id)]);
}

/// D-084: a DNS label (set or derived) is unique within a workspace.
#[tokio::test]
async fn node_labels_are_unique_per_workspace() {
    let (s, _d) = store().await;
    let (org, env) = seed_workspace(&s).await;
    let node = |host: &str| Server {
        role: forge_core::model::NodeRole::Worker,
        display_name: None,
        id: ServerId::new(),
        workspace_id: org.id,
        environment_id: Some(env.id),
        cluster_id: None,
        hostname: host.into(),
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
    let (a, b) = (node("a"), node("b"));
    s.create_server(&a).await.unwrap();
    s.create_server(&b).await.unwrap();

    // A derived label is claimed once and never replaced by a later claim.
    assert!(s.claim_node_label(a.id, org.id, "web", Timestamp(2)).await.unwrap());
    assert!(!s.claim_node_label(a.id, org.id, "other", Timestamp(2)).await.unwrap());
    assert!(matches!(s.claim_node_label(b.id, org.id, "web", Timestamp(2)).await, Err(DomainError::Conflict(_))));

    // Set and derived labels share one environment.
    assert!(matches!(s.set_node_label(b.id, org.id, "web", true, Timestamp(2)).await, Err(DomainError::Conflict(_))));
    s.set_node_label(a.id, org.id, "web-1", true, Timestamp(3)).await.unwrap();
    s.set_node_label(b.id, org.id, "web", true, Timestamp(3)).await.unwrap();
    let mut labels = s.list_node_labels(Some(org.id)).await.unwrap();
    labels.sort_by(|x, y| x.1.cmp(&y.1));
    assert_eq!(labels, vec![(b.id, "web".to_owned(), true), (a.id, "web-1".to_owned(), true)]);
    s.set_node_label(a.id, org.id, "a", false, Timestamp(4)).await.unwrap();
    assert!(s.list_node_labels(None).await.unwrap().contains(&(a.id, "a".to_owned(), false)));
    assert!(s.list_node_labels(Some(WorkspaceId::new())).await.unwrap().is_empty());
}

/// Swarm state (D-154): one swarm row per cluster, one member row per node,
/// sealed tokens kept as given; both go when the cluster is deleted.
#[tokio::test]
async fn swarm_rows_round_trip_and_go_with_the_cluster() {
    use forge_core::swarm::{ClusterSwarm, MemberRole, MemberState, SwarmMember, SwarmRepository};
    let (store, _d) = store().await;
    let (ws, _) = seed_workspace(&store).await;
    let cluster = cluster_in(ws.id, "swarm-eu");
    store.create_cluster(&cluster).await.unwrap();
    let swarm = ClusterSwarm {
        cluster_id: cluster.id,
        swarm_id: "s1".into(),
        port: 2377,
        default_addr_pool: vec!["10.20.0.0/16".into()],
        autolock: true,
        tokens_sealed: Some("sealed".into()),
        founder: None,
        updated_at: Timestamp(1),
    };

    // Only the first claim wins; the recorded swarm id is found again.
    assert!(store.claim_swarm(&swarm).await.unwrap());
    assert!(!store.claim_swarm(&swarm).await.unwrap());
    assert_eq!(store.cluster_of_swarm("s1").await.unwrap(), Some(cluster.id));
    assert_eq!(store.cluster_of_swarm("").await.unwrap(), None);
    store.put_swarm(&swarm).await.unwrap();
    assert_eq!(store.get_swarm(cluster.id).await.unwrap(), Some(swarm.clone()));
    let server = Server {
        role: forge_core::model::NodeRole::Worker,
        display_name: None,
        id: ServerId::new(),
        workspace_id: ws.id,
        environment_id: None,
        cluster_id: Some(cluster.id),
        hostname: "lab-a".into(),
        os: "linux".into(),
        arch: "x86_64".into(),
        agent_version: "0.1.0".into(),
        status: AgentStatus::Pending,
        last_seen_at: None,
        cert_serial: None,
        cert_not_after: None,
        drained: false,
        created_at: Timestamp(1),
    };
    store.create_server(&server).await.unwrap();
    let mut m = SwarmMember {
        server_id: server.id,
        cluster_id: cluster.id,
        node_id: String::new(),
        role: MemberRole::Manager,
        advertise_addr: String::new(),
        manager_addr: String::new(),
        state: MemberState::Joining,
        error: String::new(),
        updated_at: Timestamp(2),
    };
    store.put_member(&m).await.unwrap();
    m.state = MemberState::Joined;
    m.node_id = "n1".into();
    store.put_member(&m).await.unwrap();
    assert_eq!(store.list_members(cluster.id).await.unwrap(), vec![m.clone()]);
    assert_eq!(store.get_member(server.id).await.unwrap(), Some(m));
    store.delete_cluster(cluster.id).await.unwrap();
    assert!(store.get_swarm(cluster.id).await.unwrap().is_none());
    assert!(store.get_member(server.id).await.unwrap().is_none());
}

/// Removing a node takes its swarm membership; the swarm row goes only with
/// the last member, so the cluster's swarm can be founded again.
#[tokio::test]
async fn removing_the_last_swarm_member_clears_the_swarm() {
    use forge_core::swarm::{ClusterSwarm, MemberRole, MemberState, SwarmMember, SwarmRepository};
    let (store, _d) = store().await;
    let (ws, _) = seed_workspace(&store).await;
    let cluster = cluster_in(ws.id, "swarm-lab");
    store.create_cluster(&cluster).await.unwrap();
    let swarm = ClusterSwarm {
        cluster_id: cluster.id,
        swarm_id: "s1".into(),
        port: 2377,
        default_addr_pool: Vec::new(),
        autolock: false,
        tokens_sealed: None,
        founder: None,
        updated_at: Timestamp(1),
    };
    store.put_swarm(&swarm).await.unwrap();
    let mut ids = Vec::new();

    for host in ["lab-a", "lab-b"] {
        let server = Server {
            role: forge_core::model::NodeRole::Worker,
            display_name: None,
            id: ServerId::new(),
            workspace_id: ws.id,
            environment_id: None,
            cluster_id: Some(cluster.id),
            hostname: host.into(),
            os: "linux".into(),
            arch: "x86_64".into(),
            agent_version: "0.1.0".into(),
            status: AgentStatus::Online,
            last_seen_at: None,
            cert_serial: None,
            cert_not_after: None,
            created_at: Timestamp(1),
            drained: false,
        };
        store.create_server(&server).await.unwrap();
        let member = SwarmMember {
            server_id: server.id,
            cluster_id: cluster.id,
            node_id: host.into(),
            role: MemberRole::Manager,
            advertise_addr: String::new(),
            manager_addr: String::new(),
            state: MemberState::Joined,
            error: String::new(),
            updated_at: Timestamp(2),
        };
        store.put_member(&member).await.unwrap();
        ids.push(server.id);
    }

    // One member left: the swarm stays.
    store.delete_server(ids[0], Timestamp(3)).await.unwrap();
    assert!(store.get_swarm(cluster.id).await.unwrap().is_some());

    // The last member gone: so is the swarm.
    store.delete_server(ids[1], Timestamp(4)).await.unwrap();
    assert!(store.get_swarm(cluster.id).await.unwrap().is_none());
    assert!(store.list_members(cluster.id).await.unwrap().is_empty());
}

/// Registries (D-158): one per host per workspace, the sealed secret kept as
/// given.
#[tokio::test]
async fn registries_round_trip_one_per_host() {
    use forge_core::registries::{Registry, RegistryKind, RegistryRepository};
    let (store, _d) = store().await;
    let (ws, _) = seed_workspace(&store).await;
    let mut r = Registry {
        id: RegistryId::new(),
        workspace_id: ws.id,
        name: "GitHub".into(),
        kind: RegistryKind::Ghcr,
        host: "ghcr.io".into(),
        username: "bot".into(),
        secret_sealed: "sealed".into(),
        allow_push: false,
        created_by: Principal::User(UserId::new()),
        created_at: Timestamp(1),
        updated_at: Timestamp(1),
    };
    store.create_registry(&r).await.unwrap();
    let dup = Registry { id: RegistryId::new(), ..r.clone() };
    assert!(matches!(store.create_registry(&dup).await, Err(DomainError::Conflict(_))));
    r.username = "robot".into();
    r.updated_at = Timestamp(2);
    store.update_registry(&r).await.unwrap();
    assert_eq!(store.get_registry(r.id).await.unwrap(), Some(r.clone()));
    assert_eq!(store.list_registries(ws.id).await.unwrap(), vec![r.clone()]);
    store.delete_registry(r.id).await.unwrap();
    assert!(store.get_registry(r.id).await.unwrap().is_none());

    assert!(matches!(store.update_registry(&r).await, Err(DomainError::NotFound(_))), "no silent update")
}

/// Webhooks (D-159): found by token hash; deliveries stay bounded and go
/// with the hook.
#[tokio::test]
async fn webhooks_round_trip_and_keep_bounded_deliveries() {
    use forge_core::webhooks::*;
    let (store, _d) = store().await;
    let (ws, _) = seed_workspace(&store).await;
    let w = Webhook {
        id: WebhookId::new(),
        workspace_id: ws.id,
        name: "ci".into(),
        target: WebhookTarget::Service { id: ServiceId::new() },
        token_hash: "h1".into(),
        token_hint: "whk_ab".into(),
        signature: Signature::Github,
        secret_sealed: Some("sealed".into()),
        tag_pattern: Some("v*".into()),
        enabled: true,
        created_by: Principal::User(UserId::new()),
        created_at: Timestamp(1),
        updated_at: Timestamp(1),
        last_delivery_at: None,
    };
    store.create_webhook(&w).await.unwrap();
    assert_eq!(store.webhook_by_token_hash("h1").await.unwrap().map(|x| x.id), Some(w.id));
    assert!(store.webhook_by_token_hash("h2").await.unwrap().is_none());

    for i in 0..(DELIVERIES_KEPT + 5) {
        let d = Delivery {
            id: WebhookDeliveryId::new(),
            webhook_id: w.id,
            at: Timestamp(i64::from(i) + 10),
            outcome: DeliveryOutcome::Accepted,
            reason: String::new(),
            job_id: None,
            tag: Some("v1".into()),
            source_ip: "192.0.2.1".into(),
        };
        store.add_delivery(&d).await.unwrap();
    }

    let all = store.list_deliveries(w.id, 1000).await.unwrap();
    assert_eq!(all.len(), DELIVERIES_KEPT as usize);
    assert_eq!(all[0].at, Timestamp(i64::from(DELIVERIES_KEPT) + 14), "newest first");
    assert_eq!(store.get_webhook(w.id).await.unwrap().unwrap().last_delivery_at, Some(all[0].at));
    store.delete_webhook(w.id).await.unwrap();
    assert!(store.list_deliveries(w.id, 10).await.unwrap().is_empty());
}
