//! `app.action`: deploys or operates one service on one node.
//!
//! Payload: `{"service_id", "server_id", "action", "tail"}` — no secrets. The
//! dispatch step builds the [`AppSpec`], unsealing secret environment values
//! only into the signed command (memory and the mTLS link); the node helper
//! validates the spec again before writing anything.
//!
//! Host bind mounts of a container outside the app's own folder
//! (`/srv/forge/apps/<ns>/<app>/`) and the node's approved file roots need
//! `stack.privileged` (administrators, MFA), like compose stacks: the API
//! lists them in the payload as `privileged_mounts`, which makes the job
//! re-authorize `stack.privileged` at every step, and a deploy refuses any
//! such mount that the payload does not list (an edit after submission).
//! System paths, the Docker socket and Forge's own folders stay refused for
//! everyone ([`AppSpec::validate`]).

use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use forge_catalog::stacks::{normalize_abs, within};
use forge_catalog::{AppSpec, Runtime};
use forge_core::files::FileRepository;
use forge_core::model::{AgentStatus, AuditEvent, AuditOutcome, Service};
use forge_core::ports::{ServerRepository, ServiceRepository};
use forge_core::{Action, Clock, EntityKind, EntityRef, ServerId, ServiceId};
use forge_jobs::{JobHandler, StepContext, StepError};
use forge_proto::messages::AppAction;
use forge_proto::{Command, CommandSigner};
use forge_secrets::Envelope;
use serde::Deserialize;
use serde_json::{Value, json};

use crate::agent_hub::CommandQueue;

const COMMAND_TTL: Duration = Duration::from_secs(120);

/// AAD binding a sealed value to its service and variable.
#[must_use]
pub fn env_aad(app: ServiceId, key: &str) -> String {
    format!("app:{app}:env:{key}")
}

/// Builds the node spec. With `envelope`, secret values are unsealed; without
/// it (validation), they are replaced by a placeholder.
pub fn build_spec(app: &Service, envelope: Option<&Envelope>) -> Result<AppSpec, String> {
    let mut v = app.spec.clone();
    let mut env = Vec::new();

    for e in &app.env {
        let value = if e.secret {
            match (envelope, &e.sealed) {
                (Some(env), Some(sealed)) => String::from_utf8(
                    env.open(sealed, &env_aad(app.id, &e.key)).map_err(|_| format!("cannot unseal `{}`", e.key))?,
                )
                .map_err(|_| format!("`{}` is not UTF-8", e.key))?,
                (Some(_), None) => return Err(format!("secret `{}` has no value", e.key)),
                (None, _) => "secret".to_owned(),
            }
        } else {
            e.value.clone().unwrap_or_default()
        };

        env.push(json!([e.key, value]));
    }

    // `mesh` (default on) is the service's choice; the job decides the
    // network per node (`AppJob::attach_mesh`), so it never reaches a node.
    if let Some(m) = v.as_object_mut() {
        m.remove("mesh");
    }

    v["name"] = json!(app.name);
    v["runtime"] = json!(app.runtime);
    v["env"] = Value::Array(env);
    let spec: AppSpec = serde_json::from_value(v).map_err(|e| format!("invalid service: {e}"))?;
    spec.validate()?;

    Ok(spec)
}

/// Replaces `${forge.settings.<key>}` in environment values with instance settings.
pub fn expand_settings(spec: &mut AppSpec, settings: &[(String, String)]) -> Result<(), String> {
    const OPEN: &str = "${forge.settings.";

    for (k, v) in &mut spec.env {
        let mut out = String::new();
        let mut rest = v.as_str();

        while let Some(at) = rest.find(OPEN) {
            out.push_str(&rest[..at]);
            let tail = &rest[at + OPEN.len()..];
            let end = tail.find('}').ok_or_else(|| format!("unterminated setting reference in `{k}`"))?;
            let name = &tail[..end];
            let value = settings
                .iter()
                .find(|(s, _)| s == name)
                .map(|(_, v)| v.as_str())
                .ok_or_else(|| format!("`{k}` references unknown setting `{name}`"))?;
            out.push_str(value);
            rest = &tail[end + 1..];
        }

        out.push_str(rest);
        *v = out;
    }

    Ok(())
}

#[derive(Debug, Clone, Deserialize)]
struct Payload {
    service_id: ServiceId,
    server_id: ServerId,
    action: String,
    #[serde(default)]
    tail: u32,
    /// `follow`: where the previous segment stopped.
    #[serde(default)]
    cursor: String,
    /// Container volumes outside the app folder and the approved roots that
    /// the submitter was authorized for (`stack.privileged`).
    #[serde(default)]
    privileged_mounts: Vec<String>,
}

/// The container volumes of `spec` whose host path lies outside the app's own
/// folder and the approved file `roots`: they need `stack.privileged`. Empty
/// for native apps (no volumes).
#[must_use]
pub fn privileged_mounts(spec: &AppSpec, roots: &[String]) -> Vec<String> {
    if spec.runtime != Runtime::Container {
        return Vec::new();
    }

    let own = spec.home();
    let allowed: Vec<String> =
        std::iter::once(own).chain(roots.iter().cloned()).filter_map(|r| normalize_abs(&r)).collect();

    let mounts = spec.volumes.iter().filter(|v| {
        // A named volume is Docker's storage, never a host folder.
        let Some(host) = forge_catalog::apps::volume_host_path(v) else { return false };
        normalize_abs(host).is_none_or(|h| !allowed.iter().any(|r| within(&h, r)))
    });

    // Added capabilities, devices and sysctls need the same approval, as
    // on the stack path (`stacks::check`).
    mounts.cloned().chain(spec.options().privileged()).collect()
}

#[derive(Debug)]
pub struct AppJob {
    pub hub: Arc<CommandQueue>,
    pub signer: Arc<CommandSigner>,
    pub servers: Arc<dyn ServerRepository>,
    pub apps: Arc<dyn ServiceRepository>,
    pub settings: Arc<dyn forge_core::ports::SettingsRepository>,
    pub installations: Arc<dyn forge_core::ports::InstallationRepository>,
    /// Approved file roots per node (bind mounts may use them).
    pub files: Arc<dyn FileRepository>,
    /// Buckets attached to the service: their keys and stores (D-106).
    pub vault: Arc<dyn forge_core::ports::VaultRepository>,
    pub stores: Arc<dyn forge_core::storage::ObjectStoreRepository>,
    /// Records the account a native app runs as after a deploy (D-081).
    pub audit: Arc<dyn forge_core::ports::AuditRecorder>,
    pub envelope: Envelope,
    pub clock: Arc<dyn Clock>,
    /// Which copy (slot) of a service serves on each node (#73).
    pub deploys: Arc<dyn forge_core::deploy::DeploymentRepository>,
    /// Cluster networks and node facts: whether a container joins the
    /// node's cluster container network (`plans/native-mesh.md` 1.7).
    pub fabrics: Arc<dyn forge_core::ports::FabricRepository>,
    pub facts: Arc<dyn forge_core::tags::FactsRepository>,
}

/// Whether a service wants the cluster container network: containers only,
/// unless its spec sets `"mesh": false`.
#[must_use]
pub fn mesh_wanted(app: &Service) -> bool {
    app.runtime == "container" && app.spec.get("mesh") != Some(&Value::Bool(false))
}

/// Instance status while an action runs.
#[must_use]
pub fn busy_status(action: &str) -> Option<&'static str> {
    match action {
        "deploy" => Some("deploying"),
        "remove" => Some("removing"),
        "start" => Some("starting"),
        "stop" => Some("stopping"),
        "restart" => Some("restarting"),
        _ => None,
    }
}

fn payload(ctx: &StepContext) -> Result<Payload, StepError> {
    serde_json::from_value(ctx.job.payload.clone()).map_err(|e| StepError::Fatal(format!("bad payload: {e}")))
}

fn retry(e: impl std::fmt::Display) -> StepError {
    StepError::Retryable(e.to_string())
}

impl AppJob {
    /// Audits the account a native app now runs as on the node (D-081): the
    /// node resolves it (`<ns>-<app>`, hashed on a clash), so the API's
    /// `service.deploy` record cannot carry it.
    async fn audit_account(&self, ctx: &StepContext, p: &Payload, user: &str) {
        let Ok(Some(app)) = self.apps.get_service(p.service_id).await else { return };
        let action = if p.action == "prepare" { "service.instance.prepared" } else { "service.instance.deployed" };
        let ev = AuditEvent::new(
            &ctx.job.principal,
            action,
            EntityRef::new(EntityKind::Server, p.server_id),
            AuditOutcome::Success,
        )
        .in_org(Some(app.workspace_id))
        .with_details(json!({ "service": app.name, "service_id": p.service_id, "user": user }));

        if let Err(e) = self.audit.record(ev).await {
            tracing::error!(error = %e, "audit write failed");
        }
    }

    /// Creates (`present`) or removes the app's automatic file root on the
    /// node (F-05). Idempotent, so a repeated step is harmless.
    async fn sync_app_root(&self, ctx: &StepContext, p: &Payload, present: bool) -> Result<(), StepError> {
        let Some(app) = self.apps.get_service(p.service_id).await.map_err(retry)? else { return Ok(()) };
        let actor = ctx.job.principal.to_string();

        if present {
            let server = self.servers.get_server(p.server_id).await.map_err(retry)?;
            crate::app_roots::ensure(self.files.as_ref(), self.audit.as_ref(), &actor, self.clock.now(), &app, &server)
                .await
                .map_err(retry)?;
        } else {
            crate::app_roots::remove(self.files.as_ref(), self.audit.as_ref(), &actor, &app, p.server_id)
                .await
                .map_err(retry)?;
        }

        Ok(())
    }

    async fn update(
        &self,
        p: &Payload,
        ctx: &StepContext,
        f: impl FnOnce(&mut forge_core::model::ServiceInstance),
    ) -> Result<(), StepError> {
        let instances = self.apps.list_instances(p.service_id).await.map_err(retry)?;
        let Some(mut i) = instances.into_iter().find(|i| i.server_id == p.server_id) else { return Ok(()) };
        f(&mut i);
        i.last_job_id = Some(ctx.job.id);
        i.updated_at = self.clock.now();

        self.apps.put_instance(&i).await.map_err(retry)
    }

    /// Waits (up to 45 minutes) until every runtime the app needs is
    /// installed on the node; fails when one of their installs failed.
    ///
    /// A container also waits for a Docker Engine install that is running on
    /// the node (the deploy request starts one when the node has none). An
    /// engine Forge has no record of is not waited for: it was installed
    /// outside Forge, or the node reports that it is missing.
    async fn wait_for_runtimes(&self, spec: &AppSpec, server: ServerId) -> Result<(), StepError> {
        let container = spec.runtime == Runtime::Container;
        if spec.dependencies.is_empty() && !container {
            return Ok(());
        }

        let deadline = tokio::time::Instant::now() + Duration::from_secs(45 * 60);
        let mut waited_for_engine = false;

        loop {
            let have = self.installations.list_installations(Some(server)).await.map_err(retry)?;
            let mut waiting = Vec::new();

            if container {
                match engine_wait(&have, waited_for_engine) {
                    EngineWait::Ready => {}
                    EngineWait::Installing => {
                        waited_for_engine = true;
                        waiting.push("Docker Engine".to_owned());
                    }
                    EngineWait::Failed(why) => return Err(StepError::Fatal(why)),
                }
            }

            for d in &spec.dependencies {
                let name = forge_catalog::find(&d.package).map_or(d.package.as_str(), |p| p.name);

                match have.iter().find(|x| x.package == d.package && x.version == d.version) {
                    Some(x) if x.status == "installed" => {}
                    Some(x) if x.status == "failed" => {
                        return Err(StepError::Fatal(format!("{name} {} failed to install on this node", d.version)));
                    }
                    Some(_) => waiting.push(format!("{name} {}", d.version)),
                    None => {
                        return Err(StepError::Fatal(format!(
                            "{name} {} is not installed on this node; deploy again to install it",
                            d.version
                        )));
                    }
                }
            }

            if waiting.is_empty() {
                return Ok(());
            }

            if tokio::time::Instant::now() >= deadline {
                return Err(StepError::Fatal(format!("timed out waiting for {}", waiting.join(", "))));
            }

            tokio::time::sleep(Duration::from_secs(5)).await;
        }
    }

    /// The spec `app` runs with on a node, before any slot is applied:
    /// secrets unsealed, setting references expanded and attached buckets'
    /// variables added.
    pub async fn resolved_spec(&self, app: &Service) -> Result<AppSpec, StepError> {
        let mut spec = build_spec(app, Some(&self.envelope)).map_err(StepError::Fatal)?;
        let settings = self.settings.all_settings().await.map_err(retry)?;
        expand_settings(&mut spec, &settings).map_err(StepError::Fatal)?;

        // Attached buckets add their endpoint and key (D-106); a variable the
        // service defines itself wins.
        let attached = crate::app_storage::deploy_env(self.vault.as_ref(), self.stores.as_ref(), &self.envelope, app)
            .await
            .map_err(|e| StepError::Fatal(e.to_string()))?;
        crate::app_storage::merge_env(&mut spec.env, attached);
        spec.validate().map_err(StepError::Fatal)?;

        Ok(spec)
    }

    /// Connects a container to `server`'s cluster container network when the
    /// service wants it, the network is up there and the agent can take the
    /// field; otherwise leaves `spec` as it is.
    pub async fn attach_mesh(&self, app: &Service, spec: &mut AppSpec, server: ServerId) -> Result<(), StepError> {
        if !mesh_wanted(app) {
            return Ok(());
        }

        let node = self.servers.get_server(server).await.map_err(retry)?;
        let fabric = crate::stacks::fabric_nodes(self.fabrics.as_ref(), node.workspace_id).await;
        let capable = crate::stacks::mesh_capable(self.facts.as_ref(), server).await;
        let (net, _) = crate::stacks::decide_mesh(fabric.get(&server.to_string()), capable, false, &node.hostname);
        spec.mesh_network = net;

        Ok(())
    }

    /// Refuses a spec with step-1 container options for an agent that does
    /// not report the `container_options` fact: older agents refuse unknown
    /// fields, so the command would fail there with a less useful message.
    pub async fn ensure_agent_takes(&self, spec: &AppSpec, server: ServerId) -> Result<(), StepError> {
        if !spec.options().needs_new_agent() {
            return Ok(());
        }

        // A failed read or a node that has not reported facts yet (just
        // enrolled or updated) is retried; only facts without the flag mean
        // an older agent.
        let facts = self.facts.facts_of(server).await.map_err(retry)?;

        let Some((facts, _)) = facts else {
            return Err(retry("the node has not reported its facts yet"));
        };

        if facts.get("container_options").and_then(Value::as_bool) == Some(true) {
            return Ok(());
        }

        Err(StepError::Fatal(
            "this node's agent is too old for the restart policy, capabilities, ulimits, tmpfs, DNS, sysctls, devices, init or networks this service uses; update the agent first".into(),
        ))
    }

    /// Puts `spec` into the copy that serves on `server` (#73): a service
    /// rolled out side by side runs in slot `a` or `b` with its own ports;
    /// one deployed only in place keeps its single instance.
    pub async fn serving_slot(&self, spec: &mut AppSpec, app: ServiceId, server: ServerId) -> Result<(), StepError> {
        let slots = self.deploys.list_slots(app).await.map_err(retry)?;
        let serving = slots
            .iter()
            .filter(|s| s.server_id == server)
            .max_by_key(|s| (s.weight, s.state == "ready", s.release_number))
            .map(|s| s.slot.clone());
        let Some(slot) = serving.as_deref().and_then(forge_catalog::deploy::Slot::parse) else { return Ok(()) };

        self.into_slot(spec, app, slot).await.map(|_| ())
    }

    /// Rewrites `spec` for `slot` with the slot's host ports (allocated on
    /// first use). Returns service port → slot port.
    pub async fn into_slot(
        &self,
        spec: &mut AppSpec,
        app: ServiceId,
        slot: forge_catalog::deploy::Slot,
    ) -> Result<std::collections::BTreeMap<u16, u16>, StepError> {
        let count = match spec.runtime {
            Runtime::Container => spec.ports.len(),
            _ => usize::from(spec.listen_port.is_some()),
        };

        let range = forge_catalog::deploy::SLOT_PORTS;
        let ports = self
            .deploys
            .slot_ports(app, slot.as_str(), u32::try_from(count).unwrap_or(u32::MAX), (*range.start(), *range.end()))
            .await
            .map_err(retry)?;

        forge_catalog::deploy::into_slot(spec, slot, &ports).map_err(StepError::Fatal)
    }

    /// Signs `action` for `spec`, sends it to `server` and waits up to
    /// `wait` for the node's answer. `Ok` carries the node's output.
    pub async fn send(
        &self,
        principal: &str,
        server: ServerId,
        action: &str,
        spec: &AppSpec,
        wait: Duration,
    ) -> Result<Value, StepError> {
        let node = self.servers.get_server(server).await.map_err(|e| StepError::Fatal(e.to_string()))?;
        if node.status == AgentStatus::Revoked {
            return Err(StepError::Fatal("server is revoked".into()));
        }

        self.ensure_agent_takes(spec, server).await?;
        let spec_json = serde_json::to_string(spec).map_err(|e| StepError::Fatal(e.to_string()))?;
        let command =
            Command::AppAction(AppAction { action: action.to_owned(), spec_json, tail: 0, cursor: String::new() });
        let (body, signed) = self.signer.issue(&server.to_string(), principal, command, self.clock.now(), COMMAND_TTL);
        self.hub.enqueue(server, &body.operation_id, signed).map_err(StepError::Retryable)?;
        let result = self
            .hub
            .wait(&body.operation_id, wait)
            .await
            .ok_or_else(|| retry(format!("{} did not answer in time", node.hostname)))?;

        if !result.ok {
            return Err(StepError::Fatal(result.error));
        }

        Ok(serde_json::from_str(&result.output).unwrap_or(Value::Null))
    }

    /// Creates the service's automatic file root on `server` (F-05), as a
    /// deploy does. Idempotent.
    pub async fn ensure_app_root(&self, actor: &str, app: &Service, server: ServerId) -> Result<(), StepError> {
        let node = self.servers.get_server(server).await.map_err(retry)?;
        crate::app_roots::ensure(self.files.as_ref(), self.audit.as_ref(), actor, self.clock.now(), app, &node)
            .await
            .map_err(retry)?;

        Ok(())
    }

    /// Waits for the runtimes `spec` needs on `server` (Docker Engine, App
    /// Store runtimes).
    pub async fn runtimes_ready(&self, spec: &AppSpec, server: ServerId) -> Result<(), StepError> {
        self.wait_for_runtimes(spec, server).await
    }

    async fn dispatch(&self, ctx: &mut StepContext) -> Result<(), StepError> {
        let p = payload(ctx)?;
        let app = self
            .apps
            .get_service(p.service_id)
            .await
            .map_err(retry)?
            .ok_or_else(|| StepError::Fatal("service not found".into()))?;
        let server = self.servers.get_server(p.server_id).await.map_err(|e| StepError::Fatal(e.to_string()))?;
        if server.status == AgentStatus::Revoked {
            return Err(StepError::Fatal("server is revoked".into()));
        }

        let mut spec = self.resolved_spec(&app).await?;

        if p.action == "deploy" {
            self.wait_for_runtimes(&spec, p.server_id).await?;
        }

        self.serving_slot(&mut spec, app.id, p.server_id).await?;
        self.attach_mesh(&app, &mut spec, p.server_id).await?;
        self.ensure_agent_takes(&spec, p.server_id).await?;

        if p.action == "deploy" {
            let roots = crate::stacks::bind_roots(self.files.as_ref(), p.server_id).await;
            if let Some(v) = privileged_mounts(&spec, &roots).into_iter().find(|v| !p.privileged_mounts.contains(v)) {
                return Err(StepError::Fatal(format!(
                    "`{v}` (a volume outside the app folder and the approved file roots, a capability, a device or a sysctl) needs stack.privileged                      (an administrator with MFA): deploy again as one"
                )));
            }
        }

        let spec_json = serde_json::to_string(&spec).map_err(|e| StepError::Fatal(e.to_string()))?;
        let command = Command::AppAction(AppAction {
            action: p.action.clone(),
            spec_json,
            tail: p.tail,
            cursor: p.cursor.clone(),
        });
        let (body, signed) = self.signer.issue(
            &p.server_id.to_string(),
            &ctx.job.principal.to_string(),
            command,
            self.clock.now(),
            COMMAND_TTL,
        );
        self.hub.enqueue(p.server_id, &body.operation_id, signed).map_err(StepError::Retryable)?;
        ctx.set_state("operation_id", json!(body.operation_id));
        let busy = busy_status(&p.action);

        self.update(&p, ctx, |i| {
            if let Some(b) = busy {
                i.status = b.into();
            }

            i.last_error = None;
        })
        .await
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

        let wait =
            if matches!(p.action.as_str(), "deploy") { Duration::from_secs(35 * 60) } else { Duration::from_secs(90) };
        let result = self.hub.wait(&op, wait).await.ok_or_else(|| retry("the node did not report a result in time"))?;
        if !result.ok {
            let message = result.error.clone();
            let deploy = p.action == "deploy";
            self.update(&p, ctx, |i| {
                if deploy {
                    i.status = "failed".into();
                } else if i.status.ends_with("ing") || i.status == "running-job" {
                    i.status = "deployed".into();
                }

                i.last_error = Some(message.clone());
            })
            .await?;

            return Err(StepError::Fatal(message));
        }

        let out: Value = serde_json::from_str(&result.output).unwrap_or(Value::Null);
        ctx.set_state("output", out.clone());

        if p.action == "remove" {
            self.sync_app_root(ctx, &p, false).await?;
            return self.apps.delete_instance(p.service_id, p.server_id).await.map_err(retry);
        }

        if matches!(p.action.as_str(), "deploy" | "prepare") {
            if let Some(user) = out.get("user").and_then(Value::as_str) {
                self.audit_account(ctx, &p, user).await;
            }

            self.sync_app_root(ctx, &p, true).await?;
        }

        let state = out.get("state").and_then(Value::as_str).map(str::to_owned);

        self.update(&p, ctx, |i| settle(i, &p.action, state)).await
    }
}

/// Records a successful action on the instance. `reported` is the state the
/// node answered with. Not every answer has one (a container's `follow`
/// segment only carries log lines): the last known state then stays, so
/// reading logs never makes a running instance look stopped (issue #64).
fn settle(i: &mut forge_core::model::ServiceInstance, action: &str, reported: Option<String>) {
    if i.status != "pending" || action == "deploy" {
        i.status = "deployed".into();
    }

    if reported.is_some() {
        i.state = reported;
    }
}

/// What a container deploy does about Forge's Docker Engine installation on the node.
#[derive(Debug, PartialEq, Eq)]
enum EngineWait {
    /// Go on: installed, or nothing to wait for.
    Ready,
    /// An install (or another package action) is running.
    Installing,
    /// The install this deploy waited for failed.
    Failed(String),
}

/// `waited`: this deploy already saw the install running. Only then is a
/// failed record this deploy's problem; an older failure (a failed update of
/// a working engine) says nothing about whether Docker answers now.
fn engine_wait(installations: &[forge_core::model::Installation], waited: bool) -> EngineWait {
    use crate::container_runtime::Recorded;

    match crate::container_runtime::recorded(installations) {
        Recorded::Busy => EngineWait::Installing,
        Recorded::Failed if waited => {
            let why = crate::container_runtime::installation(installations)
                .and_then(|i| i.last_error.as_deref())
                .unwrap_or("see its install job");
            EngineWait::Failed(format!(
                "Docker Engine failed to install on this node ({why}); fix it or install Docker Engine there, then deploy again"
            ))
        }
        Recorded::Installed | Recorded::None | Recorded::Failed => EngineWait::Ready,
    }
}

#[async_trait]
impl JobHandler for AppJob {
    fn kind(&self) -> &'static str {
        "app.action"
    }

    fn steps(&self) -> &'static [&'static str] {
        &["dispatch", "await_result"]
    }

    fn required_action(&self, payload: &Value) -> Action {
        if payload.get("privileged_mounts").and_then(Value::as_array).is_some_and(|m| !m.is_empty()) {
            return Action::StackPrivileged;
        }

        match payload.get("action").and_then(Value::as_str) {
            Some("status" | "logs" | "follow") => Action::View,
            _ => Action::Deploy,
        }
    }

    async fn run_step(&self, index: usize, ctx: &mut StepContext) -> Result<(), StepError> {
        match index {
            0 => self.dispatch(ctx).await,
            _ => self.await_result(ctx).await,
        }
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    #[test]
    fn expands_setting_references() {
        let mut spec: AppSpec = serde_json::from_value(json!({
            "environment": "uat", "name": "api", "runtime": "systemd", "command": "x",
            "env": [["URL", "https://${forge.settings.company.domain}/api"], ["PLAIN", "v"]]
        }))
        .unwrap();
        let settings = vec![("company.domain".to_owned(), "acme.test".to_owned())];
        expand_settings(&mut spec, &settings).unwrap();
        assert_eq!(spec.env[0].1, "https://acme.test/api");
        spec.env[1].1 = "${forge.settings.missing}".into();
        assert!(expand_settings(&mut spec, &settings).is_err());
    }

    fn instance(status: &str, state: Option<&str>) -> forge_core::model::ServiceInstance {
        forge_core::model::ServiceInstance {
            service_id: ServiceId::new(),
            server_id: ServerId::new(),
            role: "active".into(),
            status: status.into(),
            state: state.map(str::to_owned),
            last_job_id: None,
            last_error: None,
            updated_at: forge_core::Timestamp(0),
        }
    }

    /// Issue #64: `forge services list` showed 0/2 running after two good
    /// deploys. A container's `follow` segment (the console's live log view,
    /// `forge services logs --follow`) answers without a state, and that used to
    /// overwrite the state the deploy had recorded.
    #[test]
    fn an_answer_without_a_state_keeps_the_last_known_one() {
        let mut i = instance("deploying", None);
        settle(&mut i, "deploy", Some("active".into()));
        assert_eq!((i.status.as_str(), i.state.as_deref()), ("deployed", Some("active")));
        settle(&mut i, "follow", None);
        assert_eq!(i.state.as_deref(), Some("active"), "following logs changes nothing");

        // A reported state still replaces it.
        settle(&mut i, "stop", Some("exited".into()));
        assert_eq!(i.state.as_deref(), Some("exited"));

        // A placement that was never deployed stays pending for read-only actions.
        let mut p = instance("pending", None);
        settle(&mut p, "status", Some("not-deployed".into()));
        assert_eq!(p.status, "pending");
    }

    #[test]
    fn a_container_deploy_waits_for_a_docker_install_that_is_running() {
        let docker = |status: &str, error: Option<&str>| forge_core::model::Installation {
            id: forge_core::InstallationId::new(),
            workspace_id: forge_core::WorkspaceId::new(),
            server_id: ServerId::new(),
            package: forge_catalog::apps::DOCKER_PACKAGE.into(),
            version: "stable".into(),
            params: std::collections::BTreeMap::new(),
            status: status.into(),
            state: None,
            env: std::collections::BTreeMap::new(),
            last_job_id: None,
            last_error: error.map(str::to_owned),
            versions: None,
            update_policy: "notify".into(),
            created_at: forge_core::Timestamp(0),
            updated_at: forge_core::Timestamp(0),
        };
        assert_eq!(engine_wait(&[], false), EngineWait::Ready, "installed outside Forge, or the node will say");
        assert_eq!(engine_wait(&[docker("installed", None)], false), EngineWait::Ready);
        assert_eq!(engine_wait(&[docker("installing", None)], false), EngineWait::Installing);

        // The install this deploy waited for failed: say so, with the reason.
        let failed = [docker("failed", Some("apt: no route to download.docker.com"))];
        match engine_wait(&failed, true) {
            EngineWait::Failed(why) => {
                assert!(why.starts_with("Docker Engine failed to install on this node"), "{why}");
                assert!(why.contains("no route to download.docker.com"), "{why}");
            }
            other => panic!("{other:?}"),
        }

        // An old failure (say a failed update of a working engine) does not stop a deploy.
        assert_eq!(engine_wait(&failed, false), EngineWait::Ready);
    }

    fn container(volumes: &[&str]) -> AppSpec {
        serde_json::from_value(json!({
            "environment": "uat", "name": "api", "runtime": "container", "image": "nginx:1",
            "volumes": volumes,
        }))
        .unwrap()
    }

    /// Host paths the deny-list misses but that lead to root on the node or
    /// to another tenant's data need `stack.privileged` (security regression).
    #[test]
    fn mounts_outside_the_app_folder_and_approved_roots_are_privileged() {
        let roots = vec!["/srv/data".to_owned()];

        for v in [
            "/var/spool/cron:/c",                    // a root crontab
            "/var/lib/dpkg:/d",                      // package scripts run as root by apt
            "/var/lib/rpm:/r",                       // the same for dnf on RHEL-likes
            "/etc/yum.repos.d:/y",                   // package sources dnf trusts
            "/home:/h",                              // users' ~/.ssh/authorized_keys
            "/srv/forge/apps/prod/db/data:/d",       // another environment's app
            "/srv/forge/apps/uat/api-other/data:/d", // a sibling app with a longer name
            "/opt/vendor:/o",                        // binaries of root services
            "/var/lib/postgresql:/p",                // another database's files
            "/srv/data/../../var/spool/cron:/c",     // `..` out of an approved root
        ] {
            let spec = container(&[v]);
            assert_eq!(privileged_mounts(&spec, &roots), vec![v.to_owned()], "{v}");
        }

        // The app's own folder and approved roots stay plain deploys.
        for v in ["/srv/forge/apps/uat/api/data:/data:ro", "/srv/forge/apps/uat/api:/app", "/srv/data/api:/d"] {
            assert!(privileged_mounts(&container(&[v]), &roots).is_empty(), "{v}");
        }

        // Without approved roots only the app folder is plain.
        assert_eq!(privileged_mounts(&container(&["/srv/data/api:/d"]), &[]).len(), 1);

        // A named volume is Docker storage, not a host folder: never privileged.
        assert!(privileged_mounts(&container(&["pgdata:/var/lib/postgresql/data"]), &[]).is_empty());

        // Native apps have no volumes.
        let native: AppSpec = serde_json::from_value(json!({
            "environment": "uat", "name": "api", "runtime": "systemd", "command": "x"
        }))
        .unwrap();
        assert!(privileged_mounts(&native, &roots).is_empty());
    }
}
