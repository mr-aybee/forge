#![allow(clippy::unwrap_used)]

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use async_trait::async_trait;
use forge_authz::{CedarAuthorizer, PolicyEngine};
use forge_core::model::*;
use forge_core::ports::{AuditRecorder, JobRepository, TenancyRepository};
use forge_core::*;
use forge_jobs::*;
use forge_store::Store;
use serde_json::json;

#[derive(Debug, Default)]
struct NullAudit(AtomicUsize);

#[async_trait]
impl AuditRecorder for NullAudit {
    async fn record(&self, e: AuditEvent) -> DomainResult<AuditEntry> {
        self.0.fetch_add(1, Ordering::SeqCst);
        Ok(AuditEntry {
            seq: 0,
            id: String::new(),
            at: Timestamp(0),
            actor: e.actor,
            action: e.action,
            resource: e.resource,
            workspace_id: e.workspace_id,
            outcome: e.outcome,
            details: e.details,
            prev_hash: String::new(),
            hash: String::new(),
            signature: String::new(),
        })
    }
}

/// Three steps; step 1 blocks until `release` is set, simulating a crash
/// window. Counts how often each step runs.
#[derive(Debug, Default)]
struct ThreeSteps {
    runs: [AtomicUsize; 3],
    block_step1: std::sync::atomic::AtomicBool,
    fail_times: AtomicUsize,
}

#[async_trait]
impl JobHandler for ThreeSteps {
    fn kind(&self) -> &'static str {
        "test.three"
    }
    fn steps(&self) -> &'static [&'static str] {
        &["prepare", "apply", "verify"]
    }
    fn required_action(&self, _payload: &serde_json::Value) -> Action {
        Action::ServerManage
    }
    async fn run_step(&self, index: usize, ctx: &mut StepContext) -> Result<(), StepError> {
        self.runs[index].fetch_add(1, Ordering::SeqCst);

        if index == 1 && self.block_step1.load(Ordering::SeqCst) {
            // Simulates the process dying mid-step: never returns.
            std::future::pending::<()>().await;
        }

        if index == 2 && self.fail_times.load(Ordering::SeqCst) > 0 {
            self.fail_times.fetch_sub(1, Ordering::SeqCst);
            return Err(StepError::Retryable("flaky".into()));
        }

        ctx.set_state(&format!("step{index}"), json!(true));

        Ok(())
    }
}

struct Fixture {
    store: Store,
    engine: Arc<PolicyEngine>,
    authz: Arc<CedarAuthorizer>,
    server: EntityRef,
    org: WorkspaceId,
    _dir: tempfile::TempDir,
}

async fn fixture() -> Fixture {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::open_sqlite(&dir.path().join("j.db")).await.unwrap();
    let org = Workspace { id: WorkspaceId::new(), name: "o".into(), slug: "o".into(), created_at: Timestamp(0) };
    store.create_workspace(&org).await.unwrap();
    let srv = Server {
        role: forge_core::model::NodeRole::Worker,
        display_name: None,
        id: ServerId::new(),
        workspace_id: org.id,
        environment_id: None,
        cluster_id: None,
        hostname: "h".into(),
        os: "linux".into(),
        arch: "x86_64".into(),
        agent_version: "0".into(),
        status: AgentStatus::Online,
        last_seen_at: None,
        cert_serial: None,
        cert_not_after: None,
        drained: false,
        created_at: Timestamp(0),
    };
    forge_core::ports::ServerRepository::create_server(&store, &srv).await.unwrap();
    let engine = Arc::new(PolicyEngine::new().unwrap());
    let authz = Arc::new(CedarAuthorizer::new(engine.clone(), Arc::new(store.clone())));

    Fixture { store, engine, authz, server: EntityRef::new(EntityKind::Server, srv.id), org: org.id, _dir: dir }
}

fn operator(f: &Fixture) -> (UserId, RoleBinding) {
    let user = UserId::new();
    let b = RoleBinding {
        id: RoleBindingId::new(),
        user_id: user,
        role: Role::Operator,
        scope: EntityRef::new(EntityKind::Workspace, f.org),
        created_at: Timestamp(0),
    };
    f.engine.link_binding(&b).unwrap();

    (user, b)
}

fn job_engine(f: &Fixture, handler: Arc<ThreeSteps>, worker: &str, lease: Duration) -> Arc<JobEngine> {
    let mut reg = JobRegistry::new();
    reg.register(handler);

    Arc::new(JobEngine::new(
        Arc::new(f.store.clone()),
        f.authz.clone(),
        Arc::new(NullAudit::default()),
        reg,
        Arc::new(SystemClock),
        EngineConfig {
            worker_id: worker.into(),
            lease,
            poll_interval: Duration::from_millis(20),
            retry: Backoff::new(Duration::from_millis(1), Duration::from_millis(5)),
            concurrency: 2,
        },
    ))
}

fn new_job(f: &Fixture, user: UserId) -> NewJob {
    NewJob {
        kind: "test.three".into(),
        workspace_id: Some(f.org),
        resource: f.server.clone(),
        principal: Principal::User(user),
        context: AuthzContext::default(),
        idempotency_key: None,
        payload: json!({}),
        max_attempts: 3,
    }
}

async fn wait_for_status(store: &Store, id: JobId, status: JobStatus) -> Job {
    for _ in 0..200 {
        let j = store.get_job(id).await.unwrap();
        if j.status == status {
            return j;
        }

        tokio::time::sleep(Duration::from_millis(25)).await;
    }

    panic!("job {id} never reached {status:?}");
}

#[tokio::test]
async fn job_runs_all_steps_and_persists_state() {
    let f = fixture().await;
    let (user, _) = operator(&f);
    let h = Arc::new(ThreeSteps::default());
    let e = job_engine(&f, h.clone(), "w1", Duration::from_secs(5));
    let job = e.submit(new_job(&f, user)).await.unwrap();
    assert!(e.run_once().await.unwrap());
    let done = f.store.get_job(job.id).await.unwrap();
    assert_eq!(done.status, JobStatus::Succeeded);
    assert_eq!(done.current_step, 3);
    assert_eq!(done.state, json!({"step0": true, "step1": true, "step2": true}));
    let steps = f.store.list_steps(job.id).await.unwrap();
    assert_eq!(steps.iter().map(|s| s.status.as_str()).collect::<Vec<_>>(), ["succeeded"; 3]);
}

#[tokio::test]
async fn crashed_worker_job_resumes_from_last_incomplete_step() {
    let f = fixture().await;
    let (user, _) = operator(&f);
    let h = Arc::new(ThreeSteps::default());
    h.block_step1.store(true, Ordering::SeqCst);

    // Worker A claims and "crashes" in step 1 (we abort its task).
    let a = job_engine(&f, h.clone(), "worker-a", Duration::from_millis(300));
    let job = a.submit(new_job(&f, user)).await.unwrap();
    let a_task = tokio::spawn({
        let a = a.clone();
        async move { a.run_once().await }
    });

    // Wait (bounded) until worker A is inside step 1, then "crash" it.
    for _ in 0..200 {
        if h.runs[1].load(Ordering::SeqCst) == 1 {
            break;
        }

        tokio::time::sleep(Duration::from_millis(10)).await;
    }

    a_task.abort();
    assert_eq!(h.runs[1].load(Ordering::SeqCst), 1, "step 1 started");
    let mid = f.store.get_job(job.id).await.unwrap();
    assert_eq!((mid.status, mid.current_step), (JobStatus::Running, 1));

    // Lease expires; worker B resumes at step 1 without redoing step 0.
    h.block_step1.store(false, Ordering::SeqCst);
    tokio::time::sleep(Duration::from_millis(400)).await;
    let b = job_engine(&f, h.clone(), "worker-b", Duration::from_secs(5));
    assert!(b.run_once().await.unwrap());

    let done = wait_for_status(&f.store, job.id, JobStatus::Succeeded).await;
    assert_eq!(done.lease_owner, None);
    assert_eq!(h.runs[0].load(Ordering::SeqCst), 1, "step 0 not repeated");
    assert_eq!(h.runs[1].load(Ordering::SeqCst), 2, "interrupted step re-run");
    assert_eq!(h.runs[2].load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn retryable_failures_back_off_then_succeed() {
    let f = fixture().await;
    let (user, _) = operator(&f);
    let h = Arc::new(ThreeSteps::default());
    h.fail_times.store(2, Ordering::SeqCst);
    let e = job_engine(&f, h.clone(), "w", Duration::from_secs(5));
    let job = e.submit(new_job(&f, user)).await.unwrap();
    let (tx, rx) = tokio::sync::watch::channel(false);
    let runner = tokio::spawn(e.clone().run(rx));
    let done = wait_for_status(&f.store, job.id, JobStatus::Succeeded).await;
    tx.send(true).unwrap();
    runner.await.unwrap();
    assert_eq!(done.attempts, 2);
    assert_eq!(h.runs[0].load(Ordering::SeqCst), 1);
    assert_eq!(h.runs[2].load(Ordering::SeqCst), 3);
}

#[tokio::test]
async fn retries_exhausted_fails_job() {
    let f = fixture().await;
    let (user, _) = operator(&f);
    let h = Arc::new(ThreeSteps::default());
    h.fail_times.store(10, Ordering::SeqCst);
    let e = job_engine(&f, h, "w", Duration::from_secs(5));
    let job = e.submit(new_job(&f, user)).await.unwrap();
    let (tx, rx) = tokio::sync::watch::channel(false);
    let runner = tokio::spawn(e.clone().run(rx));
    let done = wait_for_status(&f.store, job.id, JobStatus::Failed).await;
    tx.send(true).unwrap();
    runner.await.unwrap();
    assert!(done.last_error.unwrap().contains("flaky"));
}

#[tokio::test]
async fn unauthorized_submission_is_denied() {
    let f = fixture().await;
    let stranger = UserId::new();
    let e = job_engine(&f, Arc::new(ThreeSteps::default()), "w", Duration::from_secs(5));
    let err = e.submit(new_job(&f, stranger)).await.unwrap_err();
    assert!(matches!(err, DomainError::Denied(_)));
}

#[tokio::test]
async fn runner_denies_when_role_revoked_after_submission() {
    let f = fixture().await;
    let (user, binding) = operator(&f);
    let h = Arc::new(ThreeSteps::default());
    let e = job_engine(&f, h.clone(), "w", Duration::from_secs(5));
    let job = e.submit(new_job(&f, user)).await.unwrap();

    f.engine.unlink_binding(&binding.id.to_string()).unwrap();
    assert!(e.run_once().await.unwrap());

    let done = f.store.get_job(job.id).await.unwrap();
    assert_eq!(done.status, JobStatus::Failed);
    assert!(done.last_error.unwrap().contains("denied"));
    assert_eq!(h.runs.iter().map(|r| r.load(Ordering::SeqCst)).sum::<usize>(), 0, "no step ran");
}

#[tokio::test]
async fn idempotent_submission_returns_same_job() {
    let f = fixture().await;
    let (user, _) = operator(&f);
    let e = job_engine(&f, Arc::new(ThreeSteps::default()), "w", Duration::from_secs(5));
    let mut nj = new_job(&f, user);
    nj.idempotency_key = Some("deploy-42".into());
    let a = e.submit(nj.clone()).await.unwrap();
    let b = e.submit(nj).await.unwrap();
    assert_eq!(a.id, b.id);
}

#[tokio::test]
async fn cancel_stops_a_running_step_at_once() {
    let f = fixture().await;
    let (user, _) = operator(&f);
    let h = Arc::new(ThreeSteps::default());
    h.block_step1.store(true, Ordering::SeqCst);

    // A long lease: without the cancel signal the step would only stop at
    // the next renewal, 20 seconds away.
    let e = job_engine(&f, h.clone(), "w", Duration::from_secs(60));
    let job = e.submit(new_job(&f, user)).await.unwrap();

    let runner = {
        let e = e.clone();
        tokio::spawn(async move { e.run_once().await })
    };

    for _ in 0..200 {
        if h.runs[1].load(Ordering::SeqCst) == 1 {
            break;
        }

        tokio::time::sleep(Duration::from_millis(10)).await;
    }

    assert_eq!(h.runs[1].load(Ordering::SeqCst), 1, "step 1 is running");
    assert!(e.cancel(job.id, Principal::User(user), AuthzContext::default()).await.unwrap());
    let done = tokio::time::timeout(Duration::from_secs(2), runner).await;
    assert!(done.is_ok(), "the running step stopped right after the cancel");
    assert_eq!(h.runs[2].load(Ordering::SeqCst), 0, "no later step ran");
    assert_eq!(f.store.get_job(job.id).await.unwrap().status, JobStatus::Cancelled);
}

#[tokio::test]
async fn cancel_stops_job_and_requires_permission() {
    let f = fixture().await;
    let (user, _) = operator(&f);
    let h = Arc::new(ThreeSteps::default());
    let e = job_engine(&f, h, "w", Duration::from_secs(5));
    let job = e.submit(new_job(&f, user)).await.unwrap();
    let stranger = Principal::User(UserId::new());
    assert!(e.cancel(job.id, stranger, AuthzContext::default()).await.is_err());
    assert!(e.cancel(job.id, Principal::User(user), AuthzContext::default()).await.unwrap());
    assert!(!e.run_once().await.unwrap(), "cancelled job is not claimable");
}
