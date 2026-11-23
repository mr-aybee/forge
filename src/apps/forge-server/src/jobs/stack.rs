//! `stack.action`: deploys or operates one compose stack on one node (issue #7).
//!
//! Payload (no secrets): `{"stack_id", "server_id", "op", "revision",
//! "services", "pull", "action", "volumes", "service", "tail", "privileged"}`.
//! `op` is `deploy`, `service` (start/stop/restart/pull), `down`, `ps` or
//! `logs`. The dispatch step plans the stack again from the stored revision,
//! unseals the `.env` only into the signed command, and sends the node its
//! part; the node helper checks the policy again before running Compose.
//! Steps: dispatch → await_result → record (status, services registered as
//! services).

use std::collections::BTreeSet;
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use forge_catalog::stacks::{STACK_RUNTIME, StackOp, project_name, with_dependencies};
use forge_core::files::FileRepository;
use forge_core::model::{AgentStatus, Service, ServiceInstance};
use forge_core::ports::{ClusterRepository, FabricRepository, ServerRepository, ServiceRepository, TenancyRepository};
use forge_core::stacks::{Stack, StackNode, StackRepository};
use forge_core::{Action, Clock, ServerId, ServiceId, StackId};
use forge_jobs::{JobHandler, StepContext, StepError};
use forge_proto::messages::StackAction;
use forge_proto::{Command, CommandSigner};
use forge_secrets::Envelope;
use serde::Deserialize;
use serde_json::{Value, json};

use crate::agent_hub::CommandQueue;
use crate::stacks::{Deps, node_part, ns_slug, open_env, plan, stack_dir};

const COMMAND_TTL: Duration = Duration::from_secs(120);

#[derive(Debug, Clone, Deserialize)]
pub struct Payload {
    pub stack_id: StackId,
    pub server_id: ServerId,
    pub op: String,
    #[serde(default)]
    pub revision: i64,
    #[serde(default)]
    pub services: Vec<String>,
    #[serde(default)]
    pub pull: bool,
    #[serde(default)]
    pub action: String,
    #[serde(default)]
    pub volumes: bool,
    #[serde(default)]
    pub service: String,
    #[serde(default)]
    pub tail: u32,
    /// Submitted with `stack.privileged` (re-authorized at every step).
    #[serde(default)]
    pub privileged: bool,
}

#[derive(Debug)]
pub struct StackJob {
    pub hub: Arc<CommandQueue>,
    pub signer: Arc<CommandSigner>,
    pub servers: Arc<dyn ServerRepository>,
    pub clusters: Arc<dyn ClusterRepository>,
    pub fabrics: Arc<dyn FabricRepository>,
    pub files: Arc<dyn FileRepository>,
    pub tenancy: Arc<dyn TenancyRepository>,
    pub stacks: Arc<dyn StackRepository>,
    pub apps: Arc<dyn ServiceRepository>,
    pub envelope: Envelope,
    pub clock: Arc<dyn Clock>,
}

fn payload(ctx: &StepContext) -> Result<Payload, StepError> {
    serde_json::from_value(ctx.job.payload.clone()).map_err(|e| StepError::Fatal(format!("bad payload: {e}")))
}

fn retry(e: impl std::fmt::Display) -> StepError {
    StepError::Retryable(e.to_string())
}

fn fatal(e: impl std::fmt::Display) -> StepError {
    StepError::Fatal(e.to_string())
}

/// Node status while an operation runs.
fn busy(p: &Payload) -> Option<&'static str> {
    match (p.op.as_str(), p.action.as_str()) {
        ("deploy", _) => Some("deploying"),
        ("down", _) => Some("removing"),
        ("service", "stop") => Some("stopping"),
        ("service", "start" | "restart") => Some("starting"),
        ("service", "pull") => Some("pulling"),
        _ => None,
    }
}

impl StackJob {
    fn deps(&self) -> Deps<'_> {
        Deps {
            servers: self.servers.as_ref(),
            clusters: self.clusters.as_ref(),
            fabrics: self.fabrics.as_ref(),
            files: self.files.as_ref(),
        }
    }

    async fn load(&self, p: &Payload) -> Result<(Stack, String), StepError> {
        let stack = self
            .stacks
            .get_stack(p.stack_id)
            .await
            .map_err(retry)?
            .ok_or_else(|| fatal("the stack no longer exists"))?;
        let ns = self.tenancy.get_environment(stack.environment_id).await.map_err(retry)?;
        Ok((stack, ns_slug(&ns.name)))
    }

    async fn node(&self, p: &Payload) -> Result<Option<StackNode>, StepError> {
        Ok(self.stacks.list_nodes(p.stack_id).await.map_err(retry)?.into_iter().find(|n| n.server_id == p.server_id))
    }

    async fn set_status(
        &self,
        p: &Payload,
        ctx: &StepContext,
        status: Option<&str>,
        error: Option<String>,
    ) -> Result<(), StepError> {
        let now = self.clock.now();
        let mut n = self.node(p).await?.unwrap_or(StackNode {
            stack_id: p.stack_id,
            server_id: p.server_id,
            services: Vec::new(),
            selected: Vec::new(),
            revision: 0,
            status: "pending".into(),
            state: json!([]),
            last_job_id: None,
            last_error: None,
            updated_at: now,
        });
        if let Some(s) = status {
            s.clone_into(&mut n.status);
        }
        n.last_error = error;
        n.last_job_id = Some(ctx.job.id);
        n.updated_at = now;
        self.stacks.put_node(&n).await.map_err(retry)
    }

    /// A deploy the node rejected: Compose may have created some containers
    /// (an image pull or a health check failed later), so the node keeps the
    /// planned service names. Logs, start/stop, refresh and down then work on
    /// what is there; the deployed revision and the services stay as they were.
    async fn deploy_failed(&self, p: &Payload, ctx: &StepContext, error: &str) -> Result<(), StepError> {
        self.set_status(p, ctx, Some("failed"), Some(error.to_owned())).await?;
        let planned: Vec<String> =
            ctx.state_value("services").and_then(|v| serde_json::from_value(v.clone()).ok()).unwrap_or_default();
        let Some(mut n) = self.node(p).await? else { return Ok(()) };
        let mut changed = false;
        for s in planned {
            if !n.services.contains(&s) {
                n.services.push(s);
                changed = true;
            }
        }
        if changed {
            self.stacks.put_node(&n).await.map_err(retry)?;
        }
        Ok(())
    }

    async fn build_op(&self, p: &Payload) -> Result<(StackOp, Vec<String>), StepError> {
        let (stack, ns) = self.load(p).await?;
        let project = project_name(&ns, &stack.name);
        let dir = stack_dir(stack.dir.as_deref(), &ns, &stack.name);
        let op = match p.op.as_str() {
            "deploy" => {
                let rev = self
                    .stacks
                    .get_revision(stack.id, p.revision)
                    .await
                    .map_err(retry)?
                    .ok_or_else(|| fatal(format!("revision {} not found", p.revision)))?;
                let env = open_env(&self.envelope, stack.id, stack.env_sealed.as_deref()).map_err(fatal)?;
                let pl = plan(
                    &self.deps(),
                    stack.workspace_id,
                    stack.default_server,
                    &ns,
                    &stack.name,
                    stack.dir.as_deref(),
                    &rev.content,
                    &env,
                    p.privileged,
                )
                .await
                .map_err(fatal)?;
                pl.report.ensure(p.privileged).map_err(fatal)?;
                let node = pl
                    .node(p.server_id)
                    .ok_or_else(|| fatal(format!("revision {} has no services on this node", p.revision)))?;
                // Ticked services on this node plus everything they depend on.
                let wanted: Vec<String> = p.services.iter().filter(|s| node.services.contains(*s)).cloned().collect();
                let selected: Vec<String> = if p.services.is_empty() {
                    Vec::new()
                } else {
                    with_dependencies(&pl.interpolated, &wanted)
                        .into_iter()
                        .filter(|s| node.services.contains(s))
                        .collect()
                };
                if !p.services.is_empty() && selected.is_empty() {
                    return Err(fatal("none of the chosen services runs on this node"));
                }
                let part =
                    node_part(&pl, node, &ns, &stack.name, stack.dir.as_deref(), &env, p.privileged).map_err(fatal)?;
                let all: Vec<String> = node.services.iter().cloned().collect();
                return Ok((StackOp::Deploy { part: Box::new(part), services: selected, pull: p.pull }, all));
            }
            "service" => StackOp::Service { project, dir, action: p.action.clone(), services: p.services.clone() },
            "down" => StackOp::Down { project, dir, volumes: p.volumes },
            "ps" => StackOp::Ps { project, dir },
            "logs" => StackOp::Logs { project, dir, service: p.service.clone(), tail: p.tail },
            other => return Err(fatal(format!("unknown stack operation `{other}`"))),
        };
        Ok((op, Vec::new()))
    }

    async fn dispatch(&self, ctx: &mut StepContext) -> Result<(), StepError> {
        let p = payload(ctx)?;
        let server = self.servers.get_server(p.server_id).await.map_err(fatal)?;
        if server.status == AgentStatus::Revoked {
            return Err(fatal("server is revoked"));
        }
        let (op, services) = match self.build_op(&p).await {
            Ok(x) => x,
            Err(e) => {
                if p.op == "deploy" {
                    let msg = match &e {
                        StepError::Fatal(m) | StepError::Retryable(m) => m.clone(),
                    };
                    self.set_status(&p, ctx, Some("failed"), Some(msg)).await?;
                }
                return Err(e);
            }
        };
        op.validate().map_err(fatal)?;
        ctx.set_state("services", json!(services));
        let op_json = serde_json::to_string(&op).map_err(fatal)?;
        let (body, signed) = self.signer.issue(
            &p.server_id.to_string(),
            &ctx.job.principal.to_string(),
            Command::StackAction(StackAction { op_json }),
            self.clock.now(),
            COMMAND_TTL,
        );
        self.hub.enqueue(p.server_id, &body.operation_id, signed).map_err(StepError::Retryable)?;
        ctx.set_state("operation_id", json!(body.operation_id));
        if let Some(b) = busy(&p) {
            self.set_status(&p, ctx, Some(b), None).await?;
        }
        Ok(())
    }

    async fn await_result(&self, ctx: &mut StepContext) -> Result<(), StepError> {
        let p = payload(ctx)?;
        let op = match ctx.state_value("operation_id").and_then(Value::as_str).map(str::to_owned) {
            Some(op) if self.hub.is_tracked(&op) => op,
            _ => {
                self.dispatch(ctx).await?;
                ctx.state_value("operation_id").and_then(Value::as_str).unwrap_or_default().to_owned()
            }
        };
        let wait = match p.op.as_str() {
            "deploy" | "down" => Duration::from_secs(35 * 60),
            "service" if p.action == "pull" => Duration::from_secs(35 * 60),
            _ => Duration::from_secs(150),
        };
        let result = self.hub.wait(&op, wait).await.ok_or_else(|| retry("the node did not report a result in time"))?;
        if !result.ok {
            if p.op == "deploy" {
                self.deploy_failed(&p, ctx, &result.error).await?;
            } else if busy(&p).is_some() {
                self.set_status(&p, ctx, Some("deployed"), Some(result.error.clone())).await?;
            }
            return Err(StepError::Fatal(result.error));
        }
        let out: Value = serde_json::from_str(&result.output).unwrap_or(Value::Null);
        ctx.set_state("output", out);
        Ok(())
    }

    async fn record(&self, ctx: &mut StepContext) -> Result<(), StepError> {
        let p = payload(ctx)?;
        let out = ctx.state_value("output").cloned().unwrap_or(Value::Null);
        let now = self.clock.now();
        match p.op.as_str() {
            "logs" => Ok(()),
            "down" => {
                self.stacks.delete_node(p.stack_id, p.server_id).await.map_err(retry)?;
                self.deregister(&p).await
            }
            "deploy" => {
                let services: Vec<String> = ctx
                    .state_value("services")
                    .and_then(|v| serde_json::from_value(v.clone()).ok())
                    .unwrap_or_default();
                let state = out.get("services").cloned().unwrap_or(json!([]));
                let node = StackNode {
                    stack_id: p.stack_id,
                    server_id: p.server_id,
                    services: services.clone(),
                    selected: p.services.clone(),
                    revision: p.revision,
                    status: "deployed".into(),
                    state: state.clone(),
                    last_job_id: Some(ctx.job.id),
                    last_error: None,
                    updated_at: now,
                };
                self.stacks.put_node(&node).await.map_err(retry)?;
                if let Some(mut s) = self.stacks.get_stack(p.stack_id).await.map_err(retry)? {
                    if s.deployed_revision != p.revision {
                        // Only this column: rewriting the whole row could undo an
                        // edit saved while the deploy ran, and must not change the
                        // edit version (`updated_at`) editors compare.
                        self.stacks.set_deployed_revision(p.stack_id, p.revision).await.map_err(retry)?;
                        s.deployed_revision = p.revision;
                    }
                    self.register(&s, &p, &services, &state, ctx).await?;
                }
                Ok(())
            }
            _ => {
                // `service` and `ps`: refresh the reported state.
                let Some(mut n) = self.node(&p).await? else { return Ok(()) };
                if let Some(s) = out.get("services") {
                    n.state = s.clone();
                }
                if busy(&p).is_some() {
                    "deployed".clone_into(&mut n.status);
                }
                n.last_error = None;
                n.last_job_id = Some(ctx.job.id);
                n.updated_at = now;
                self.stacks.put_node(&n).await.map_err(retry)?;
                if let Some(s) = self.stacks.get_stack(p.stack_id).await.map_err(retry)? {
                    self.sync_states(&s, &p, &n.state).await?;
                }
                Ok(())
            }
        }
    }

    /// Services registered from this stack, by service name.
    async fn registered(&self, stack: &Stack) -> Result<Vec<Service>, StepError> {
        Ok(self
            .apps
            .list_services(Some(stack.environment_id))
            .await
            .map_err(retry)?
            .into_iter()
            .filter(|a| a.runtime == STACK_RUNTIME && a.spec["stack_id"] == json!(stack.id))
            .collect())
    }

    /// Registers each service of this node as a service (runtime
    /// `compose-service`) placed on the node.
    async fn register(
        &self,
        stack: &Stack,
        p: &Payload,
        services: &[String],
        state: &Value,
        ctx: &StepContext,
    ) -> Result<(), StepError> {
        let now = self.clock.now();
        let existing = self.registered(stack).await?;
        for svc in services {
            let reported = service_state(state, svc);
            let spec = json!({
                "stack": stack.name,
                "stack_id": stack.id,
                "service": svc,
                "image": reported.as_ref().and_then(|r| r["image"].as_str()),
                "ports": reported.as_ref().map(|r| r["ports"].clone()).unwrap_or(json!([])),
            });
            let app = match existing.iter().find(|a| a.spec["service"] == json!(svc)) {
                Some(a) => {
                    let mut a = a.clone();
                    a.spec = spec;
                    a.updated_at = now;
                    a
                }
                None => Service {
                    id: ServiceId::new(),
                    workspace_id: stack.workspace_id,
                    environment_id: stack.environment_id,
                    name: format!("{}-{svc}", stack.name),
                    runtime: STACK_RUNTIME.into(),
                    spec,
                    env: Vec::new(),
                    created_by: ctx.job.principal.clone(),
                    created_at: now,
                    updated_at: now,
                },
            };
            self.apps.put_service(&app).await.map_err(retry)?;
            self.apps
                .put_instance(&ServiceInstance {
                    service_id: app.id,
                    server_id: p.server_id,
                    role: "active".into(),
                    status: "deployed".into(),
                    state: reported.as_ref().and_then(|r| r["state"].as_str()).map(str::to_owned),
                    last_job_id: Some(ctx.job.id),
                    last_error: None,
                    updated_at: now,
                })
                .await
                .map_err(retry)?;
        }
        // Services no longer in this node's part leave it.
        let keep: BTreeSet<&String> = services.iter().collect();
        for a in existing {
            let svc = a.spec["service"].as_str().unwrap_or_default().to_owned();
            if !keep.contains(&svc) {
                self.remove_instance(&a, p.server_id).await?;
            }
        }
        Ok(())
    }

    async fn sync_states(&self, stack: &Stack, p: &Payload, state: &Value) -> Result<(), StepError> {
        for a in self.registered(stack).await? {
            let svc = a.spec["service"].as_str().unwrap_or_default();
            let Some(mut i) =
                self.apps.list_instances(a.id).await.map_err(retry)?.into_iter().find(|i| i.server_id == p.server_id)
            else {
                continue;
            };
            i.state = service_state(state, svc).and_then(|r| r["state"].as_str().map(str::to_owned));
            i.updated_at = self.clock.now();
            self.apps.put_instance(&i).await.map_err(retry)?;
        }
        Ok(())
    }

    async fn remove_instance(&self, a: &Service, server: ServerId) -> Result<(), StepError> {
        self.apps.delete_instance(a.id, server).await.map_err(retry)?;
        if self.apps.list_instances(a.id).await.map_err(retry)?.is_empty() {
            self.apps.delete_service(a.id).await.map_err(retry)?;
        }
        Ok(())
    }

    async fn deregister(&self, p: &Payload) -> Result<(), StepError> {
        let Some(stack) = self.stacks.get_stack(p.stack_id).await.map_err(retry)? else { return Ok(()) };
        for a in self.registered(&stack).await? {
            self.remove_instance(&a, p.server_id).await?;
        }
        Ok(())
    }
}

/// The reported `ps` entry of one service.
fn service_state(state: &Value, service: &str) -> Option<Value> {
    state.as_array()?.iter().find(|s| s["service"] == service).cloned()
}

#[async_trait]
impl JobHandler for StackJob {
    fn kind(&self) -> &'static str {
        "stack.action"
    }

    fn steps(&self) -> &'static [&'static str] {
        &["dispatch", "await_result", "record"]
    }

    fn required_action(&self, payload: &Value) -> Action {
        if payload.get("privileged").and_then(Value::as_bool) == Some(true) {
            return Action::StackPrivileged;
        }
        match payload.get("op").and_then(Value::as_str) {
            Some("ps" | "logs") => Action::View,
            _ => Action::StackManage,
        }
    }

    async fn run_step(&self, index: usize, ctx: &mut StepContext) -> Result<(), StepError> {
        match index {
            0 => self.dispatch(ctx).await,
            1 => self.await_result(ctx).await,
            _ => self.record(ctx).await,
        }
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    #[test]
    fn statuses_and_actions_follow_the_operation() {
        let p = |op: &str, action: &str| Payload {
            stack_id: StackId::new(),
            server_id: ServerId::new(),
            op: op.into(),
            revision: 1,
            services: Vec::new(),
            pull: false,
            action: action.into(),
            volumes: false,
            service: String::new(),
            tail: 0,
            privileged: false,
        };
        assert_eq!(busy(&p("deploy", "")), Some("deploying"));
        assert_eq!(busy(&p("service", "stop")), Some("stopping"));
        assert_eq!(busy(&p("ps", "")), None);
        let st = json!([{ "service": "web", "state": "running" }]);
        assert_eq!(service_state(&st, "web").unwrap()["state"], "running");
        assert!(service_state(&st, "db").is_none());
    }
}
