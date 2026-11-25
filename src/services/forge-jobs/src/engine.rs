use std::sync::Arc;
use std::time::Duration;

use forge_core::model::{AuditEvent, AuditOutcome, Job, JobStatus, NewJob};
use forge_core::ports::{AuditRecorder, Authorizer, JobRepository};
use forge_core::{AccessRequest, Action, AuthzContext, Backoff, Clock, DomainError, DomainResult, EntityRef, JobId};
use serde::Serialize;
use tokio::sync::{Notify, Semaphore, broadcast, watch};

use crate::handler::{JobRegistry, StepContext, StepError};

/// Engine tuning. Defaults suit the small profile.
#[derive(Debug, Clone)]
pub struct EngineConfig {
    pub worker_id: String,
    /// Lease length; renewed every `lease / 3` while a step runs.
    pub lease: Duration,
    /// Idle poll interval when no job is runnable.
    pub poll_interval: Duration,
    pub retry: Backoff,
    /// Maximum jobs run concurrently by this worker.
    pub concurrency: usize,
}

impl Default for EngineConfig {
    fn default() -> Self {
        Self {
            worker_id: format!("worker-{}", forge_secrets::crypto::random_token::<6>()),
            lease: Duration::from_secs(30),
            poll_interval: Duration::from_millis(500),
            retry: Backoff::new(Duration::from_secs(1), Duration::from_secs(300)),
            concurrency: 8,
        }
    }
}

/// Live job progress, fanned out to WebSocket subscribers.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct JobEvent {
    pub job_id: JobId,
    pub kind: String,
    pub status: JobStatus,
    pub step: Option<String>,
}

/// The engine: submits, claims and executes jobs.
#[derive(Debug)]
pub struct JobEngine {
    repo: Arc<dyn JobRepository>,
    authz: Arc<dyn Authorizer>,
    audit: Arc<dyn AuditRecorder>,
    registry: JobRegistry,
    clock: Arc<dyn Clock>,
    config: EngineConfig,
    events: broadcast::Sender<JobEvent>,
    wake: Notify,
    /// Steps running in this process, woken at once when their job is
    /// cancelled (instead of at the next lease renewal).
    cancels: std::sync::Mutex<std::collections::HashMap<JobId, Arc<Notify>>>,
}

impl JobEngine {
    /// Creates the job engine.
    #[must_use]
    pub fn new(
        repo: Arc<dyn JobRepository>,
        authz: Arc<dyn Authorizer>,
        audit: Arc<dyn AuditRecorder>,
        registry: JobRegistry,
        clock: Arc<dyn Clock>,
        config: EngineConfig,
    ) -> Self {
        let (events, _) = broadcast::channel(256);
        Self { repo, authz, audit, registry, clock, config, events, wake: Notify::new(), cancels: Default::default() }
    }

    /// Subscribes to job events.
    #[must_use]
    pub fn subscribe(&self) -> broadcast::Receiver<JobEvent> {
        self.events.subscribe()
    }

    /// The registered job handlers.
    #[must_use]
    pub fn registry(&self) -> &JobRegistry {
        &self.registry
    }

    /// Validates, authorizes (`job.create` and the handler's action) and
    /// enqueues a job. Idempotent when `idempotency_key` is set.
    pub async fn submit(&self, job: NewJob) -> DomainResult<Job> {
        let handler = self
            .registry
            .get(&job.kind)
            .ok_or_else(|| DomainError::Invalid(format!("unknown job kind `{}`", job.kind)))?;

        for action in [Action::JobCreate, handler.required_action(&job.payload)] {
            let req = AccessRequest::new(job.principal.clone(), action, job.resource.clone(), job.context);
            if let Err(e) = self.authz.authorize(&req).await {
                self.audit_job_event(
                    &job.principal.to_string(),
                    "job.submit",
                    &job.resource,
                    job.workspace_id,
                    AuditOutcome::Denied,
                    &job.kind,
                )
                .await;
                return Err(e);
            }
        }

        let (created, is_new) = self.repo.insert_job(job, self.clock.now()).await?;
        if is_new {
            self.audit_job_event(
                &created.principal.to_string(),
                "job.submit",
                &EntityRef::new(forge_core::EntityKind::Job, created.id),
                created.workspace_id,
                AuditOutcome::Success,
                &created.kind,
            )
            .await;
            self.emit(&created, None);
            self.wake.notify_one();
        }

        Ok(created)
    }

    /// Cancels a job after authorizing `job.cancel`.
    pub async fn cancel(&self, id: JobId, principal: forge_core::Principal, ctx: AuthzContext) -> DomainResult<bool> {
        let resource = EntityRef::new(forge_core::EntityKind::Job, id);
        self.authz.authorize(&AccessRequest::new(principal.clone(), Action::JobCancel, resource.clone(), ctx)).await?;
        let job = self.repo.get_job(id).await?;
        let cancelled = self.repo.cancel_job(id, self.clock.now()).await?;
        if cancelled {
            self.audit_job_event(
                &principal.to_string(),
                "job.cancel",
                &resource,
                job.workspace_id,
                AuditOutcome::Success,
                &job.kind,
            )
            .await;
            let mut j = job;
            j.status = JobStatus::Cancelled;
            self.emit(&j, None);

            // Stop the step now if it runs in this process.
            if let Some(n) = self.cancels.lock().ok().and_then(|c| c.get(&id).cloned()) {
                n.notify_one();
            }
        }

        Ok(cancelled)
    }

    /// Claims and fully processes at most one job. Returns `true` if a job ran.
    pub async fn run_once(&self) -> DomainResult<bool> {
        let now = self.clock.now();

        let Some(job) = self.repo.claim_job(&self.config.worker_id, now, now.plus(self.config.lease)).await? else {
            return Ok(false);
        };

        self.execute(job).await;

        Ok(true)
    }

    /// Worker loop. Runs until `shutdown` becomes `true`.
    pub async fn run(self: Arc<Self>, mut shutdown: watch::Receiver<bool>) {
        let permits = Arc::new(Semaphore::new(self.config.concurrency.max(1)));
        tracing::info!(worker = %self.config.worker_id, "job engine started");

        loop {
            if *shutdown.borrow() {
                break;
            }

            let Ok(permit) = permits.clone().acquire_owned().await else { break };
            let now = self.clock.now();
            match self.repo.claim_job(&self.config.worker_id, now, now.plus(self.config.lease)).await {
                Ok(Some(job)) => {
                    let me = self.clone();
                    tokio::spawn(async move {
                        me.execute(job).await;
                        drop(permit);
                    });
                }
                Ok(None) => {
                    drop(permit);
                    tokio::select! {
                        () = self.wake.notified() => {}
                        () = tokio::time::sleep(self.config.poll_interval) => {}
                        _ = shutdown.changed() => {}
                    }
                }
                Err(e) => {
                    drop(permit);
                    tracing::warn!(error = %e, "job claim failed");
                    tokio::time::sleep(self.config.poll_interval).await;
                }
            }
        }

        tracing::info!(worker = %self.config.worker_id, "job engine stopped");
    }

    /// Runs the remaining steps of a claimed job.
    async fn execute(&self, job: Job) {
        use tracing::Instrument;
        let span = tracing::info_span!("job", id = %job.id, kind = %job.kind);
        self.execute_steps(job).instrument(span).await;
    }

    async fn execute_steps(&self, job: Job) {
        let id = job.id;
        let token = job.lease_token;

        let Some(handler) = self.registry.get(&job.kind) else {
            self.finish(&job, JobStatus::Failed, Some(&format!("no handler for kind `{}`", job.kind))).await;
            return;
        };

        let steps = handler.steps();
        let mut ctx = StepContext { state: job.state.clone(), job };
        let start = ctx.job.current_step as usize;
        if start > 0 {
            tracing::info!(resume_at = start, "resuming job after interruption");
        }

        for (index, name) in steps.iter().enumerate().skip(start) {
            // Enforcement point: re-authorize before every step.
            let req = AccessRequest::new(
                ctx.job.principal.clone(),
                handler.required_action(&ctx.job.payload),
                ctx.job.resource.clone(),
                ctx.job.context,
            );

            if let Err(e) = self.authz.authorize(&req).await {
                let msg = format!("denied at step `{name}`: {e}");
                self.audit_job_event(
                    &ctx.job.principal.to_string(),
                    "job.step",
                    &ctx.job.resource,
                    ctx.job.workspace_id,
                    AuditOutcome::Denied,
                    &ctx.job.kind,
                )
                .await;
                self.finish(&ctx.job, JobStatus::Failed, Some(&msg)).await;

                return;
            }

            let idx = u32::try_from(index).unwrap_or(u32::MAX);
            if let Err(e) = self.repo.start_step(id, token, idx, name, self.clock.now()).await {
                tracing::warn!(error = %e, "lost lease before step start");
                return;
            }

            self.emit(&ctx.job, Some(name));

            let outcome = match self.run_step_with_lease(handler.as_ref(), index, &mut ctx, token).await {
                Ok(o) => o,
                Err(e) => {
                    tracing::warn!(error = %e, step = name, "lease lost during step; another worker will resume");
                    return;
                }
            };

            match outcome {
                Ok(()) => {
                    if let Err(e) = self.repo.complete_step(id, token, idx, &ctx.state, self.clock.now()).await {
                        tracing::warn!(error = %e, "lost lease after step");
                        return;
                    }
                }
                Err(StepError::Retryable(msg)) if ctx.job.attempts + 1 < ctx.job.max_attempts => {
                    let delay = self.config.retry.delay(ctx.job.attempts, unit_random());
                    let next = self.clock.now().plus(delay);
                    tracing::info!(step = name, ?delay, error = %msg, "step failed, retrying");

                    if let Err(e) = self.repo.schedule_retry(id, token, next, &msg, self.clock.now()).await {
                        tracing::warn!(error = %e, "could not schedule retry");
                    }

                    let mut j = ctx.job.clone();
                    j.status = JobStatus::Queued;
                    self.emit(&j, Some(name));

                    return;
                }
                Err(StepError::Retryable(msg) | StepError::Fatal(msg)) => {
                    self.finish(&ctx.job, JobStatus::Failed, Some(&format!("step `{name}`: {msg}"))).await;
                    return;
                }
            }
        }

        self.finish(&ctx.job, JobStatus::Succeeded, None).await;
    }

    /// Runs one step while renewing the lease. Returns `Err` if the lease is
    /// lost (crash takeover or cancellation); the step future is dropped.
    async fn run_step_with_lease(
        &self,
        handler: &dyn crate::JobHandler,
        index: usize,
        ctx: &mut StepContext,
        token: i64,
    ) -> DomainResult<Result<(), StepError>> {
        let id = ctx.job.id;
        let renew_every = self.config.lease / 3;
        let cancelled = Arc::new(Notify::new());

        if let Ok(mut c) = self.cancels.lock() {
            c.insert(id, cancelled.clone());
        }

        let step = handler.run_step(index, ctx);
        tokio::pin!(step);
        let mut ticker = tokio::time::interval(renew_every);
        ticker.tick().await; // first tick is immediate

        let out = loop {
            tokio::select! {
                res = &mut step => break Ok(res),
                () = cancelled.notified() => break Err(DomainError::Conflict("job cancelled".into())),
                _ = ticker.tick() => {
                    let until = self.clock.now().plus(self.config.lease);
                    if let Err(e) = self.repo.renew_lease(id, token, until).await {
                        break Err(e);
                    }
                }
            }
        };

        if let Ok(mut c) = self.cancels.lock() {
            c.remove(&id);
        }

        out
    }

    async fn finish(&self, job: &Job, status: JobStatus, error: Option<&str>) {
        match self.repo.finish_job(job.id, job.lease_token, status, error, self.clock.now()).await {
            Ok(()) => {
                let outcome =
                    if status == JobStatus::Succeeded { AuditOutcome::Success } else { AuditOutcome::Failure };
                self.audit_job_event(
                    &job.principal.to_string(),
                    "job.finish",
                    &EntityRef::new(forge_core::EntityKind::Job, job.id),
                    job.workspace_id,
                    outcome,
                    &job.kind,
                )
                .await;
                let mut j = job.clone();
                j.status = status;
                j.last_error = error.map(str::to_owned);
                self.emit(&j, None);
                tracing::info!(status = status.as_str(), error, "job finished");
            }
            Err(e) => tracing::warn!(error = %e, "could not finish job (lease lost?)"),
        }
    }

    fn emit(&self, job: &Job, step: Option<&str>) {
        // No subscribers is fine.
        let _ = self.events.send(JobEvent {
            job_id: job.id,
            kind: job.kind.clone(),
            status: job.status,
            step: step.map(str::to_owned),
        });
    }

    async fn audit_job_event(
        &self,
        actor: &str,
        action: &str,
        resource: &EntityRef,
        org: Option<forge_core::WorkspaceId>,
        outcome: AuditOutcome,
        kind: &str,
    ) {
        let ev = AuditEvent::new(actor, action, resource, outcome)
            .in_org(org)
            .with_details(serde_json::json!({ "kind": kind }));

        if let Err(e) = self.audit.record(ev).await {
            tracing::error!(error = %e, "failed to write audit entry");
        }
    }
}

fn unit_random() -> f64 {
    let b = forge_secrets::crypto::random_bytes::<8>();

    // 53 random bits → [0, 1)
    #[allow(clippy::cast_precision_loss)]
    let v = (u64::from_le_bytes(b) >> 11) as f64 / (1u64 << 53) as f64;
    v
}
