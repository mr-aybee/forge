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

/// D-107: the owner of an object store round-trips, is changed only through
/// `set_object_store_owner`, and survives a later save of the store.
#[tokio::test]
async fn object_store_owner_round_trips_and_survives_a_save() {
    use forge_core::storage::{ObjectStore, ObjectStoreRepository};
    let (s, _dir) = store().await;
    let (ws, ns) = seed_workspace(&s).await;
    let new_store = |name: &str, owner: Option<EnvironmentId>| ObjectStore {
        id: ObjectStoreId::new(),
        workspace_id: ws.id,
        environment_id: owner,
        name: name.into(),
        slug: name.to_lowercase(),
        provider: "rustfs".into(),
        mode: "managed".into(),
        region: "us-east-1".into(),
        path_style: true,
        version: "latest".into(),
        deployed_version: None,
        runtime: "native".into(),
        topology: "standalone".into(),
        s3_port: 9000,
        console_port: None,
        endpoint: "http://node:9000".into(),
        credential_id: VaultItemId::new(),
        status: "running".into(),
        last_error: None,
        last_job_id: None,
        created_by: Principal::User(UserId::new()),
        created_at: Timestamp(1),
        updated_at: Timestamp(1),
    };
    let shared = new_store("Shared", None);
    let owned = new_store("Owned", Some(ns.id));
    s.put_object_store(&shared).await.unwrap();
    s.put_object_store(&owned).await.unwrap();
    assert_eq!(s.get_object_store(shared.id).await.unwrap().unwrap(), shared);
    assert_eq!(s.get_object_store(owned.id).await.unwrap().unwrap(), owned);

    s.set_object_store_owner(shared.id, Some(ns.id), Timestamp(7)).await.unwrap();
    // A job that loaded the store before the change saves its copy: the
    // status is written, the owner is not.
    let mut stale = shared.clone();
    stale.status = "stopped".into();
    s.put_object_store(&stale).await.unwrap();
    let got = s.get_object_store(shared.id).await.unwrap().unwrap();
    assert_eq!((got.environment_id, got.status.as_str()), (Some(ns.id), "stopped"));

    // Neither an unknown store nor an unknown environment is recorded.
    let missing = s.set_object_store_owner(ObjectStoreId::new(), Some(ns.id), Timestamp(8)).await;
    assert!(matches!(missing, Err(DomainError::NotFound(_))), "{missing:?}");
    let missing = s.set_object_store_owner(shared.id, Some(EnvironmentId::new()), Timestamp(8)).await;
    assert!(matches!(missing, Err(DomainError::NotFound(_))), "{missing:?}");

    // An environment that owns a store is kept, in the delete's own transaction.
    let kept = s.delete_environment(ns.id).await;
    assert!(matches!(kept, Err(DomainError::Conflict(_))), "{kept:?}");
    assert!(s.get_environment(ns.id).await.is_ok());

    s.set_object_store_owner(shared.id, None, Timestamp(9)).await.unwrap();
    let cleared = s.get_object_store(shared.id).await.unwrap().unwrap();
    assert_eq!((cleared.environment_id, cleared.updated_at), (None, Timestamp(9)));
    let listed = s.list_object_stores(Some(ws.id)).await.unwrap();
    assert_eq!(listed.iter().filter(|o| o.environment_id == Some(ns.id)).count(), 1);
}

/// D-108: bucket grants round-trip, a revocation is the only later change,
/// the live list leaves out revoked and expired rows, and the grants of a
/// store are deleted with it.
#[tokio::test]
async fn bucket_grants_round_trip_and_go_with_their_store() {
    use forge_core::storage::{BucketAccess, BucketGrant, BucketGrantRepository, ObjectStore, ObjectStoreRepository};
    let (s, _dir) = store().await;
    let (ws, _) = seed_workspace(&s).await;
    let user = |name: &str| User {
        id: UserId::new(),
        username: Some(name.into()),
        email: None,
        display_name: name.into(),
        password_hash: None,
        totp_secret_sealed: None,
        oidc_subject: None,
        disabled: false,
        created_at: Timestamp(1),
    };
    let (ada, bob) = (user("ada"), user("bob"));
    s.create_user(&ada).await.unwrap();
    s.create_user(&bob).await.unwrap();
    let new_store = |name: &str| ObjectStore {
        id: ObjectStoreId::new(),
        workspace_id: ws.id,
        environment_id: None,
        name: name.into(),
        slug: name.to_lowercase(),
        provider: "rustfs".into(),
        mode: "managed".into(),
        region: "us-east-1".into(),
        path_style: true,
        version: "latest".into(),
        deployed_version: None,
        runtime: "native".into(),
        topology: "standalone".into(),
        s3_port: 9000,
        console_port: None,
        endpoint: "http://node:9000".into(),
        credential_id: VaultItemId::new(),
        status: "running".into(),
        last_error: None,
        last_job_id: None,
        created_by: Principal::User(UserId::new()),
        created_at: Timestamp(1),
        updated_at: Timestamp(1),
    };
    let (media, logs) = (new_store("Media"), new_store("Logs"));
    s.put_object_store(&media).await.unwrap();
    s.put_object_store(&logs).await.unwrap();
    let grant = |who: &User, on: &ObjectStore, bucket: &str, created: i64, expires: Timestamp| BucketGrant {
        id: BucketGrantId::new(),
        workspace_id: ws.id,
        user_id: who.id,
        store_id: on.id,
        bucket: bucket.into(),
        access: BucketAccess::ReadWrite,
        created_by: format!("user:{}", bob.id),
        created_at: Timestamp(created),
        expires_at: expires,
        revoked_at: None,
        note: "for the launch".into(),
    };
    let forever = grant(&ada, &media, "photos", 10, NEVER_EXPIRES);
    let short = grant(&ada, &media, "videos", 20, Timestamp(1_000));
    let mut read = grant(&ada, &logs, "app", 30, NEVER_EXPIRES);
    read.access = BucketAccess::Read;
    let other = grant(&bob, &media, "photos", 40, NEVER_EXPIRES);
    for g in [&forever, &short, &read, &other] {
        s.put_bucket_grant(g).await.unwrap();
    }
    assert_eq!(s.get_bucket_grant(forever.id).await.unwrap().unwrap(), forever);
    assert_eq!(s.get_bucket_grant(read.id).await.unwrap().unwrap(), read);
    assert!(s.get_bucket_grant(BucketGrantId::new()).await.unwrap().is_none());

    // One person's live grants, newest first; an expiry is exclusive.
    let ids = |v: Vec<BucketGrant>| v.into_iter().map(|g| g.id).collect::<Vec<_>>();
    assert_eq!(ids(s.list_live_bucket_grants(ada.id, Timestamp(999)).await.unwrap()), [read.id, short.id, forever.id]);
    assert_eq!(ids(s.list_live_bucket_grants(ada.id, Timestamp(1_000)).await.unwrap()), [read.id, forever.id]);
    // A store's grants, newest first and bounded.
    assert_eq!(ids(s.list_store_bucket_grants(media.id, None, 10).await.unwrap()), [other.id, short.id, forever.id]);
    assert_eq!(ids(s.list_store_bucket_grants(media.id, None, 1).await.unwrap()), [other.id]);
    // Only the live ones: the limit counts live rows, not the rows skipped.
    let live = |at: i64, limit: u32| s.list_store_bucket_grants(media.id, Some(Timestamp(at)), limit);
    assert_eq!(ids(live(999, 10).await.unwrap()), [other.id, short.id, forever.id]);
    assert_eq!(ids(live(1_000, 2).await.unwrap()), [other.id, forever.id]);

    // Saving again changes the revocation and nothing else.
    let mut revoked = forever.clone();
    revoked.revoked_at = Some(Timestamp(50));
    revoked.access = BucketAccess::Read;
    revoked.bucket = "elsewhere".into();
    s.put_bucket_grant(&revoked).await.unwrap();
    let got = s.get_bucket_grant(forever.id).await.unwrap().unwrap();
    assert_eq!(got, BucketGrant { revoked_at: Some(Timestamp(50)), ..forever.clone() });
    assert_eq!(ids(s.list_live_bucket_grants(ada.id, Timestamp(999)).await.unwrap()), [read.id, short.id]);
    // A revocation is final: a stale copy without it does not bring the
    // grant back, and a later revocation does not move the first one.
    s.put_bucket_grant(&forever).await.unwrap();
    assert_eq!(s.get_bucket_grant(forever.id).await.unwrap().unwrap().revoked_at, Some(Timestamp(50)));
    let mut later = forever.clone();
    later.revoked_at = Some(Timestamp(60));
    s.put_bucket_grant(&later).await.unwrap();
    assert_eq!(s.get_bucket_grant(forever.id).await.unwrap().unwrap().revoked_at, Some(Timestamp(50)));
    assert_eq!(ids(s.list_live_bucket_grants(ada.id, Timestamp(999)).await.unwrap()), [read.id, short.id]);
    // The live grants of one bucket, whoever holds them.
    let of_bucket = |bucket: &'static str, at: i64| s.list_live_grants_of_bucket(media.id, bucket, Timestamp(at));
    assert_eq!(ids(of_bucket("photos", 999).await.unwrap()), [other.id], "the revoked one is left out");
    assert_eq!(ids(of_bucket("videos", 999).await.unwrap()), [short.id]);
    assert!(of_bucket("videos", 1_000).await.unwrap().is_empty(), "expired");
    assert!(of_bucket("app", 0).await.unwrap().is_empty(), "a bucket of another store");
    // Users by id, for the names in a list.
    let mut found: Vec<UserId> =
        s.get_users(&[bob.id, UserId::new(), ada.id]).await.unwrap().iter().map(|u| u.id).collect();
    found.sort();
    let mut want = vec![ada.id, bob.id];
    want.sort();
    assert_eq!(found, want);
    assert!(s.get_users(&[]).await.unwrap().is_empty());

    // The grants of a store go with it; the other store keeps its own.
    s.delete_object_store(media.id).await.unwrap();
    assert!(s.list_store_bucket_grants(media.id, None, 10).await.unwrap().is_empty());
    assert!(s.get_bucket_grant(other.id).await.unwrap().is_none());
    assert!(s.list_live_bucket_grants(bob.id, Timestamp(0)).await.unwrap().is_empty());
    assert_eq!(ids(s.list_live_bucket_grants(ada.id, Timestamp(0)).await.unwrap()), [read.id]);
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
        updated_at: Timestamp(1),
    };
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
