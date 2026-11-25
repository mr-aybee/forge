//! `storage.action`: runs one object store operation on one member node.
//!
//! Payload: `{"store_id", "server_id", "action"}` — no credential. For
//! `install` the bootstrap credential is unsealed from the vault inside the
//! step and travels only in the signed command (memory and mTLS).
//! Steps: `dispatch` → `await_result`, which records the node's report on the
//! store and member. Operations are idempotent, so a re-dispatch after a
//! control-plane restart is safe.

use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use forge_catalog::storage::{Runtime, StoreCredential, StoreOp, StoreReport, StoreSpec};
use forge_core::model::AgentStatus;
use forge_core::ports::{InstallationRepository, ServerRepository, VaultRepository};
use forge_core::storage::{ObjectStore, ObjectStoreRepository, StoreMember, TOPOLOGY_DISTRIBUTED};
use forge_core::storage_settings::{RustfsSettings, StoreSettingsRepository};
use forge_core::{Action, Clock, ObjectStoreId, ServerId, VaultItemId};
use forge_jobs::{JobHandler, StepContext, StepError};
use forge_proto::messages::StorageAction;
use forge_proto::{Command, CommandSigner};
use serde::Deserialize;
use serde_json::{Value, json};

use crate::agent_hub::CommandQueue;

const COMMAND_TTL: Duration = Duration::from_secs(120);
/// Downloads and first starts can take a while; the node stops at 45 minutes.
const RESULT_WAIT: Duration = Duration::from_secs(50 * 60);
/// How long a rollback waits for the node to clean up after a failed install.
const CLEANUP_WAIT: Duration = Duration::from_secs(5 * 60);

/// Operations the job accepts.
pub const ACTIONS: &[&str] =
    &["install", "start", "stop", "restart", "status", "uninstall", "disable", "enable", "reconfigure"];

/// Statuses of an action in progress.
pub const BUSY: &[&str] = &[
    "installing",
    "removing",
    "starting",
    "stopping",
    "restarting",
    "checking",
    "adopting",
    "disabling",
    "enabling",
    "reconfiguring",
];

/// What a `reconfigure` changed, so a failed recreate (the node rolls the
/// container back) also rolls the store record back.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, Deserialize)]
pub struct PreviousSettings {
    pub version: String,
    pub s3_port: u16,
    pub console_port: Option<u16>,
    pub region: String,
    pub endpoint: String,
    pub settings: Option<RustfsSettings>,
}

#[derive(Debug, Clone, Deserialize)]
struct Payload {
    store_id: ObjectStoreId,
    server_id: ServerId,
    action: String,
    /// `uninstall` only: also delete the data paths.
    #[serde(default)]
    delete_data: bool,
    /// `reconfigure` only: the settings before the change.
    #[serde(default)]
    previous: Option<PreviousSettings>,
}

#[derive(Debug)]
pub struct StorageJob {
    pub hub: Arc<CommandQueue>,
    pub signer: Arc<CommandSigner>,
    pub servers: Arc<dyn ServerRepository>,
    pub stores: Arc<dyn ObjectStoreRepository>,
    pub settings: Arc<dyn StoreSettingsRepository>,
    pub vault: Arc<dyn VaultRepository>,
    pub installations: Arc<dyn InstallationRepository>,
    pub audit: Arc<dyn forge_core::ports::AuditRecorder>,
    pub envelope: forge_secrets::Envelope,
    pub clock: Arc<dyn Clock>,
}

/// AAD binding a vault item's sealed secret to its id (same as the vault API).
#[must_use]
pub fn vault_aad(id: VaultItemId) -> String {
    format!("vault:{id}")
}

/// Status shown while an action runs.
#[must_use]
pub fn busy_status(action: &str) -> &'static str {
    match action {
        "install" => "installing",
        "uninstall" => "removing",
        "start" => "starting",
        "stop" => "stopping",
        "restart" => "restarting",
        "adopt" => "adopting",
        "disable" => "disabling",
        "enable" => "enabling",
        "reconfigure" => "reconfiguring",
        _ => "checking",
    }
}

/// The node-side spec for a store member. A distributed store lists every
/// drive of every member, in member order, so all members get the identical
/// layout.
pub fn spec(
    store: &ObjectStore,
    member: &StoreMember,
    all: &[StoreMember],
    settings: Option<&RustfsSettings>,
) -> Result<StoreSpec, String> {
    let runtime = match store.runtime.as_str() {
        "native" => Runtime::Native,
        "docker" => Runtime::Docker,
        other => return Err(format!("unknown runtime `{other}`")),
    };

    let spec = StoreSpec {
        provider: store.provider.clone(),
        slug: store.slug.clone(),
        version: store.version.clone(),
        runtime,
        s3_port: store.s3_port,
        console_port: store.console_port,
        data_paths: member.data_paths.clone(),
        peers: if store.topology == TOPOLOGY_DISTRIBUTED {
            all.iter()
                .flat_map(|m| {
                    m.data_paths.iter().map(|p| forge_catalog::storage::peer(&m.endpoint_host, store.s3_port, p))
                })
                .collect()
        } else {
            Vec::new()
        },
        require_mount: member.require_mount,
        // The App Store data path is allowed for the adopted container only.
        adopted: member.data_paths == [forge_catalog::storage::LEGACY_DATA] || settings.is_some_and(|s| s.adopted),
        owner: store.id.to_string(),
        // `us-east-1` is the engine's default: left out for older agents.
        region: if store.region == "us-east-1" { String::new() } else { store.region.clone() },
        docker: settings.filter(|_| runtime == Runtime::Docker).cloned(),
    };
    spec.validate()?;

    Ok(spec)
}

fn payload(ctx: &StepContext) -> Result<Payload, StepError> {
    serde_json::from_value(ctx.job.payload.clone()).map_err(|e| StepError::Fatal(format!("bad payload: {e}")))
}

fn retry(e: impl std::fmt::Display) -> StepError {
    StepError::Retryable(e.to_string())
}

impl StorageJob {
    async fn load_spec(
        &self,
        store: &ObjectStore,
        member: &StoreMember,
        all: &[StoreMember],
    ) -> Result<StoreSpec, StepError> {
        let settings = self.settings.get_store_settings(store.id).await.map_err(retry)?;
        spec(store, member, all, settings.as_ref()).map_err(StepError::Fatal)
    }

    async fn load(&self, p: &Payload) -> Result<(ObjectStore, StoreMember, Vec<StoreMember>), StepError> {
        let store = self
            .stores
            .get_object_store(p.store_id)
            .await
            .map_err(retry)?
            .ok_or_else(|| StepError::Fatal("the object store no longer exists".into()))?;
        let all = self.stores.list_store_members(p.store_id).await.map_err(retry)?;
        let member = all
            .iter()
            .find(|m| m.server_id == p.server_id)
            .cloned()
            .ok_or_else(|| StepError::Fatal("the node is not a member of this object store".into()))?;

        Ok((store, member, all))
    }

    /// After `adopt`: seals the container's credential as the store's root
    /// credential and hands the container from the App Store to the store.
    async fn adopted(&self, store: &mut ObjectStore, out: &Value) -> Result<(), StepError> {
        let c = out.get("credential").filter(|c| !c.is_null()).cloned();
        let cred: StoreCredential = c
            .and_then(|c| serde_json::from_value(c).ok())
            .filter(|c: &StoreCredential| c.validate().is_ok())
            .ok_or_else(|| StepError::Fatal("the node did not report the container's credential".into()))?;
        let now = self.clock.now();
        let id = store.credential_id;
        let sealed = self
            .envelope
            .seal(cred.secret_key.as_bytes(), &vault_aad(id))
            .map_err(|_| StepError::Fatal("cannot seal the credential".into()))?;
        let item = forge_core::model::VaultItem {
            id,
            workspace_id: store.workspace_id,
            name: format!("{} (object store root)", store.name),
            username: cred.access_key,
            url: store.endpoint.clone(),
            notes: "Root credential of an adopted RustFS container (Object Storage).".into(),
            tags: vec!["object-storage".into()],
            links: Vec::new(),
            sealed,
            created_by: store.created_by.clone(),
            created_at: now,
            updated_at: now,
            rotated_at: now,
        };
        self.vault.put_vault_item(&item).await.map_err(retry)?;

        // The App Store no longer manages the container (its own adoption only).
        let legacy = self.settings.get_store_settings(store.id).await.map_err(retry)?.is_none_or(|s| !s.adopted);

        for i in self.installations.list_installations(None).await.map_err(retry)? {
            if legacy && i.package == "rustfs" && i.workspace_id == store.workspace_id {
                let members = self.stores.list_store_members(store.id).await.map_err(retry)?;
                if members.iter().any(|m| m.server_id == i.server_id) {
                    self.installations.delete_installation(i.id).await.map_err(retry)?;
                }
            }
        }

        Ok(())
    }

    async fn restore(&self, store: &mut ObjectStore, prev: &PreviousSettings) -> Result<(), StepError> {
        store.version.clone_from(&prev.version);
        store.s3_port = prev.s3_port;
        store.console_port = prev.console_port;
        store.region.clone_from(&prev.region);
        store.endpoint.clone_from(&prev.endpoint);

        // A Docker store without a settings row ran with the defaults.
        if store.runtime == "docker" {
            let s = prev.settings.clone().unwrap_or_default();
            self.settings.put_store_settings(store.id, &s, self.clock.now()).await.map_err(retry)?;
        }

        Ok(())
    }

    async fn credential(&self, store: &ObjectStore) -> Result<StoreCredential, StepError> {
        let item = self
            .vault
            .get_vault_item(store.credential_id)
            .await
            .map_err(retry)?
            .filter(|i| i.workspace_id == store.workspace_id)
            .ok_or_else(|| StepError::Fatal("the store's credential is missing from the vault".into()))?;
        let secret = self
            .envelope
            .open(&item.sealed, &vault_aad(item.id))
            .map_err(|_| StepError::Fatal("cannot unseal the store's credential".into()))?;
        let secret_key =
            String::from_utf8(secret).map_err(|_| StepError::Fatal("the store's secret key is not text".into()))?;

        Ok(StoreCredential { access_key: item.username, secret_key })
    }

    async fn dispatch(&self, ctx: &mut StepContext) -> Result<(), StepError> {
        let p = payload(ctx)?;
        if !ACTIONS.contains(&p.action.as_str()) && p.action != "adopt" {
            return Err(StepError::Fatal(format!("unknown object store action `{}`", p.action)));
        }

        let (store, member, all) = self.load(&p).await?;
        let mut spec = self.load_spec(&store, &member, &all).await?;

        // An agent that predates ownership markers rejects the field: retried without it.
        let legacy = ctx.state_value("legacy_agent").and_then(Value::as_bool).unwrap_or(false);
        if legacy {
            if p.delete_data {
                return Err(StepError::Fatal(old_agent_message(&member)));
            }

            spec.owner.clear();
        }

        let server = self.servers.get_server(p.server_id).await.map_err(|e| StepError::Fatal(e.to_string()))?;
        if server.status == AgentStatus::Revoked {
            return Err(StepError::Fatal("server is revoked".into()));
        }

        let op = match p.action.as_str() {
            "install" => StoreOp::Install { spec, credential: self.credential(&store).await? },
            "start" | "enable" => StoreOp::Start { spec },
            "stop" | "disable" => StoreOp::Stop { spec },
            "restart" => StoreOp::Restart { spec },
            "uninstall" => {
                let protect = if p.delete_data { self.protected_paths(&store, p.server_id).await? } else { Vec::new() };
                StoreOp::Uninstall { spec, delete_data: p.delete_data, protect }
            }
            "adopt" => StoreOp::Adopt { spec },
            "reconfigure" => StoreOp::Reconfigure { spec, credential: self.credential(&store).await? },
            _ => StoreOp::Status { spec },
        };

        let op_json = serde_json::to_string(&op).map_err(|e| StepError::Fatal(e.to_string()))?;
        let (body, signed) = self.signer.issue(
            &p.server_id.to_string(),
            &ctx.job.principal.to_string(),
            Command::StorageAction(StorageAction { op_json }),
            self.clock.now(),
            COMMAND_TTL,
        );
        self.hub.enqueue(p.server_id, &body.operation_id, signed).map_err(StepError::Retryable)?;
        ctx.set_state("operation_id", json!(body.operation_id));

        if p.action != "status" {
            let mut m = member;
            m.status = busy_status(&p.action).into();
            self.stores.put_store_member(&m).await.map_err(retry)?;
            let mut s = store;
            s.status = busy_status(&p.action).into();
            s.last_error = None;
            s.last_job_id = Some(ctx.job.id);
            s.updated_at = self.clock.now();
            self.stores.put_object_store(&s).await.map_err(retry)?;
        }

        Ok(())
    }

    async fn await_result(&self, ctx: &mut StepContext) -> Result<(), StepError> {
        let p = payload(ctx)?;
        let op = ctx.state_value("operation_id").and_then(Value::as_str).map(str::to_owned);
        let op = match op {
            Some(op) if self.hub.is_tracked(&op) => op,
            _ => {
                self.dispatch(ctx).await?;
                ctx.state_value("operation_id").and_then(Value::as_str).unwrap_or_default().to_owned()
            }
        };

        let result = self
            .hub
            .wait(&op, RESULT_WAIT)
            .await
            .ok_or_else(|| StepError::Retryable("the node did not report a result in time".into()))?;
        let (mut store, mut member, mut all) = self.load(&p).await?;
        let now = self.clock.now();
        store.last_job_id = Some(ctx.job.id);
        store.updated_at = now;

        if !result.ok {
            let err: Value = serde_json::from_str(&result.error).unwrap_or_else(|_| json!({ "error": result.error }));
            let tail: Vec<&str> = err
                .get("output")
                .and_then(Value::as_str)
                .map(|o| o.lines().filter(|l| !l.trim().is_empty()).collect())
                .unwrap_or_default();
            let tail = tail[tail.len().saturating_sub(12)..].join("\n");
            let base = err.get("error").and_then(Value::as_str).unwrap_or("object store action failed");
            let old_agent = base.contains(OLD_AGENT_ERROR);
            let legacy = ctx.state_value("legacy_agent").and_then(Value::as_bool).unwrap_or(false);
            if old_agent && !legacy && !p.delete_data {
                ctx.set_state("legacy_agent", json!(true));
                tracing::info!(node = %member.endpoint_host, "agent predates ownership markers; sending without them");
                self.dispatch(ctx).await?;

                return Box::pin(self.await_result(ctx)).await;
            }

            let message = if old_agent && p.delete_data {
                old_agent_message(&member)
            } else if tail.is_empty() {
                base.to_owned()
            } else {
                format!("{base}\n{tail}")
            };

            ctx.set_state("output", err.get("output").cloned().unwrap_or(Value::Null));
            member.status = if p.action == "install" { "failed".into() } else { "error".into() };
            member.observed_at = Some(now);
            self.stores.put_store_member(&member).await.map_err(retry)?;
            replace(&mut all, &member);
            store.status = aggregate(&all);
            store.last_error =
                Some(if all.len() > 1 { format!("{}: {message}", member.endpoint_host) } else { message.clone() });
            self.stores.put_object_store(&store).await.map_err(retry)?;

            if p.action == "reconfigure"
                && let Some(prev) = &p.previous
            {
                // The node put the previous container back: so does the record.
                self.restore(&mut store, prev).await?;
                self.stores.put_object_store(&store).await.map_err(retry)?;
            }

            if p.action == "install" && never_ran(&store, &all) {
                self.roll_back(ctx, &p, &store, &member, &all, &message).await?;
            }

            return Err(StepError::Fatal(message));
        }

        let out: Value = serde_json::from_str(&result.output).unwrap_or(Value::Null);
        ctx.set_state("output", out.get("output").cloned().unwrap_or(Value::Null));
        let report: StoreReport =
            out.get("report").cloned().and_then(|r| serde_json::from_value(r).ok()).unwrap_or_default();
        ctx.set_state("report", json!(report));

        if p.action == "adopt" {
            self.adopted(&mut store, &out).await?;
            let env =
                forge_catalog::storage::adopted_env(out.get("output").and_then(Value::as_str).unwrap_or_default());

            if let Some(mut s) = self.settings.get_store_settings(store.id).await.map_err(retry)?
                && s.adopted
                && !env.is_empty()
            {
                s.env = env;
                self.settings.put_store_settings(store.id, &s, now).await.map_err(retry)?;
            }
        }

        member.observed_at = Some(now);

        if p.action == "uninstall" {
            // Data paths and the vault credential stay (recovery/adoption).
            // The store goes once its last member is removed.
            member.status = "removed".into();
            self.stores.put_store_member(&member).await.map_err(retry)?;
            replace(&mut all, &member);

            if all.iter().all(|m| m.status == "removed") {
                self.stores.delete_object_store(store.id).await.map_err(retry)?;
            }

            return Ok(());
        }

        member.status = match report.state.as_str() {
            "running" => "running".into(),
            // A disabled member stays disabled until it is enabled again.
            "stopped" if p.action == "disable" || (p.action == "status" && member.status == "disabled") => {
                "disabled".into()
            }
            "stopped" => "stopped".into(),
            "not-installed" => "missing".into(),
            _ => "unknown".into(),
        };

        if !report.version.is_empty() {
            store.deployed_version = Some(report.version.clone());
        }

        member.capacity_bytes = report.capacity_bytes;
        member.free_bytes = report.free_bytes;
        member.dedicated = report.dedicated;
        self.stores.put_store_member(&member).await.map_err(retry)?;
        replace(&mut all, &member);
        store.status = aggregate(&all);

        if store.status == "running" {
            store.last_error = None;
        }

        self.stores.put_object_store(&store).await.map_err(retry)
    }
}

impl StorageJob {
    /// A first install failed on this member: removes what the install left
    /// on the node (best effort; the uninstall is idempotent and keeps the
    /// data paths) and, once every member has failed, deletes the store
    /// record and its vault credential. The job and its log stay in Jobs.
    async fn roll_back(
        &self,
        ctx: &StepContext,
        p: &Payload,
        store: &ObjectStore,
        member: &StoreMember,
        all: &[StoreMember],
        error: &str,
    ) -> Result<(), StepError> {
        if let Ok(mut spec) = self.load_spec(store, member, all).await {
            if ctx.state_value("legacy_agent").and_then(Value::as_bool).unwrap_or(false) {
                spec.owner.clear();
            }

            let op = StoreOp::Uninstall { spec, delete_data: false, protect: Vec::new() };
            if let Ok(op_json) = serde_json::to_string(&op) {
                let (body, signed) = self.signer.issue(
                    &p.server_id.to_string(),
                    &ctx.job.principal.to_string(),
                    Command::StorageAction(StorageAction { op_json }),
                    self.clock.now(),
                    COMMAND_TTL,
                );

                if self.hub.enqueue(p.server_id, &body.operation_id, signed).is_ok() {
                    let _ = self.hub.wait(&body.operation_id, CLEANUP_WAIT).await;
                }
            }
        }

        let members = self.stores.list_store_members(store.id).await.map_err(retry)?;
        if !members.iter().all(|m| m.status == "failed" || m.status == "removed") {
            // Other members are still installing; the last failure rolls back.
            return Ok(());
        }

        // The credential goes first: once the record is gone a retry can no
        // longer find it, and the secret would stay behind in the vault.
        if let Ok(Some(item)) = self.vault.get_vault_item(store.credential_id).await
            && item.workspace_id == store.workspace_id
        {
            self.vault.delete_vault_item(item.id).await.map_err(retry)?;
        }

        self.stores.delete_object_store(store.id).await.map_err(retry)?;
        let ev = forge_core::model::AuditEvent::new(
            &ctx.job.principal,
            "storage.install",
            forge_core::EntityRef::new(forge_core::EntityKind::Server, p.server_id),
            forge_core::model::AuditOutcome::Failure,
        )
        .in_org(Some(store.workspace_id))
        .with_details(json!({
            "store_id": store.id,
            "name": store.name,
            "rolled_back": true,
            "job_id": ctx.job.id,
            "error": error.lines().next().unwrap_or_default(),
        }));

        if let Err(e) = self.audit.record(ev).await {
            tracing::error!(error = %e, "audit write failed");
        }

        Ok(())
    }
}

/// What an agent that predates ownership markers answers: its strict parser
/// rejects the `owner` and `delete_data` fields.
pub const OLD_AGENT_ERROR: &str = "invalid object store operation";

/// Shown when deleting data needs a newer agent.
#[must_use]
pub fn old_agent_message(member: &StoreMember) -> String {
    format!(
        "update the agent on {} to delete data: it predates data ownership markers. Nothing was removed on that node",
        member.endpoint_host
    )
}

impl StorageJob {
    /// Other stores' data paths on `server`, which a delete must never touch.
    async fn protected_paths(&self, store: &ObjectStore, server: ServerId) -> Result<Vec<String>, StepError> {
        let members = self.stores.list_node_store_members(server).await.map_err(retry)?;
        Ok(members.into_iter().filter(|m| m.store_id != store.id).flat_map(|m| m.data_paths).collect())
    }
}

/// Whether the store has never run on any member: a failed first install,
/// which is rolled back instead of leaving a store behind.
#[must_use]
pub fn never_ran(store: &ObjectStore, members: &[StoreMember]) -> bool {
    store.deployed_version.is_none()
        && !members.iter().any(|m| matches!(m.status.as_str(), "running" | "stopped" | "disabled" | "degraded"))
}

fn replace(all: &mut [StoreMember], m: &StoreMember) {
    if let Some(slot) = all.iter_mut().find(|x| x.server_id == m.server_id) {
        slot.clone_from(m);
    }
}

/// Store status from its members: `running` when all run, `degraded` when
/// some do (a distributed store keeps serving within its parity), otherwise
/// the members' common state, or `failed`/`error` if any member reported one.
#[must_use]
pub fn aggregate(members: &[StoreMember]) -> String {
    let live: Vec<&StoreMember> = members.iter().filter(|m| m.status != "removed").collect();
    if live.is_empty() {
        return "removed".into();
    }

    // Members still being worked on keep the store busy.
    if let Some(busy) = live.iter().find(|m| m.status == "pending" || BUSY.contains(&m.status.as_str())) {
        return if busy.status == "pending" { "installing".into() } else { busy.status.clone() };
    }

    let running = live.iter().filter(|m| m.status == "running").count();
    if running == live.len() {
        return "running".into();
    }

    if running > 0 {
        return "degraded".into();
    }

    for s in ["failed", "error", "missing", "disabled", "stopped"] {
        if live.iter().any(|m| m.status == s) {
            return s.into();
        }
    }

    "unknown".into()
}

#[async_trait]
impl JobHandler for StorageJob {
    fn kind(&self) -> &'static str {
        "storage.action"
    }

    fn steps(&self) -> &'static [&'static str] {
        &["dispatch", "await_result"]
    }

    fn required_action(&self, payload: &Value) -> Action {
        match payload.get("action").and_then(Value::as_str) {
            Some("status") => Action::ServerManage,
            _ => Action::StorageManage,
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
mod tests {
    use forge_core::storage::StoreMember;
    use forge_core::{ObjectStoreId, ServerId};

    use super::{aggregate, busy_status};

    #[test]
    fn busy_statuses() {
        assert_eq!(busy_status("install"), "installing");
        assert_eq!(busy_status("status"), "checking");
    }

    fn members(states: &[&str]) -> Vec<StoreMember> {
        states
            .iter()
            .map(|s| StoreMember {
                store_id: ObjectStoreId::new(),
                server_id: ServerId::new(),
                data_paths: vec!["/srv/d0".into()],
                endpoint_host: "h".into(),
                status: (*s).into(),
                capacity_bytes: None,
                free_bytes: None,
                require_mount: false,
                dedicated: None,
                observed_at: None,
            })
            .collect()
    }

    #[test]
    fn store_status_follows_members() {
        assert_eq!(aggregate(&members(&["running", "running"])), "running");
        assert_eq!(aggregate(&members(&["running", "stopped"])), "degraded");
        assert_eq!(aggregate(&members(&["running", "installing"])), "installing");
        assert_eq!(aggregate(&members(&["pending", "running"])), "installing");
        assert_eq!(aggregate(&members(&["failed", "stopped"])), "failed");
        assert_eq!(aggregate(&members(&["stopped", "stopped"])), "stopped");
        assert_eq!(aggregate(&members(&["removed", "running"])), "running");
        assert_eq!(aggregate(&members(&["removed"])), "removed");
    }
}
