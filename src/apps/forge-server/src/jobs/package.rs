//! `package.action`: runs one App Store recipe action on one node.
//!
//! Payload: `{"server_id", "installation_id", "package", "version", "action", "params"}`.
//! The catalog validates it before submission and again on the node.
//! Steps: `dispatch` (signed `PackageAction` to the agent) → `await_result`
//! (the agent reports when the recipe finishes; installs can take minutes),
//! which also records the outcome on the installation.

use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use forge_core::model::{AgentStatus, PackageVersions};
use forge_core::ports::{InstallationRepository, ServerRepository};
use forge_core::{Action, Clock, InstallationId, ServerId};
use forge_jobs::{JobHandler, StepContext, StepError};
use forge_proto::messages::PackageAction;
use forge_proto::{Command, CommandSigner};
use serde::Deserialize;
use serde_json::{Value, json};

use crate::agent_hub::CommandQueue;

/// How long the agent has to pick up the signed command.
const COMMAND_TTL: Duration = Duration::from_secs(120);
/// How long a recipe may run (the node helper stops it at 45 minutes).
const RESULT_WAIT: Duration = Duration::from_secs(50 * 60);

#[derive(Debug, Clone, Deserialize)]
struct Payload {
    server_id: ServerId,
    installation_id: InstallationId,
    package: String,
    version: String,
    action: String,
    #[serde(default)]
    params: BTreeMap<String, String>,
}

#[derive(Debug)]
pub struct PackageJob {
    pub hub: Arc<CommandQueue>,
    pub signer: Arc<CommandSigner>,
    pub servers: Arc<dyn ServerRepository>,
    pub installations: Arc<dyn InstallationRepository>,
    pub clock: Arc<dyn Clock>,
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
        "upgrade" => "updating",
        _ => "checking",
    }
}

fn payload(ctx: &StepContext) -> Result<Payload, StepError> {
    serde_json::from_value(ctx.job.payload.clone()).map_err(|e| StepError::Fatal(format!("bad payload: {e}")))
}

impl PackageJob {
    async fn dispatch(&self, ctx: &mut StepContext) -> Result<(), StepError> {
        let p = payload(ctx)?;
        forge_catalog::validate(&p.package, &p.version, &p.action, &p.params).map_err(StepError::Fatal)?;
        let server = self.servers.get_server(p.server_id).await.map_err(|e| StepError::Fatal(e.to_string()))?;
        if server.status == AgentStatus::Revoked {
            return Err(StepError::Fatal("server is revoked".into()));
        }

        let command = Command::PackageAction(PackageAction {
            package: p.package.clone(),
            version: p.version.clone(),
            action: p.action.clone(),
            params: p.params.clone(),
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
        let action = p.action.clone();

        self.update(&p, ctx, |i| {
            if !forge_catalog::READ_ONLY_ACTIONS.contains(&action.as_str()) {
                i.status = busy_status(&action).into();
            }

            i.last_error = None;
        })
        .await
    }

    async fn update(
        &self,
        p: &Payload,
        ctx: &StepContext,
        f: impl FnOnce(&mut forge_core::model::Installation),
    ) -> Result<(), StepError> {
        let Some(mut i) = self
            .installations
            .get_installation(p.installation_id)
            .await
            .map_err(|e| StepError::Retryable(e.to_string()))?
        else {
            return Ok(());
        };

        f(&mut i);
        i.last_job_id = Some(ctx.job.id);
        i.updated_at = self.clock.now();

        self.installations.put_installation(&i).await.map_err(|e| StepError::Retryable(e.to_string()))
    }

    async fn await_result(&self, ctx: &mut StepContext) -> Result<(), StepError> {
        let p = payload(ctx)?;
        let op = ctx.state_value("operation_id").and_then(Value::as_str).map(str::to_owned);

        // After a control-plane restart the waiter is gone; recipes are
        // idempotent, so dispatch again and wait.
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

        if !result.ok {
            let err: Value = serde_json::from_str(&result.error).unwrap_or_else(|_| json!({ "error": result.error }));

            // The last lines of the recipe's output usually say why.
            let tail: Vec<&str> = err
                .get("output")
                .and_then(Value::as_str)
                .map(|o| o.lines().filter(|l| !l.trim().is_empty()).collect::<Vec<_>>())
                .unwrap_or_default();
            let tail = tail[tail.len().saturating_sub(12)..].join("\n");
            let base = err.get("error").and_then(Value::as_str).unwrap_or("recipe failed");
            let message = if tail.is_empty() { base.to_owned() } else { format!("{base}\n{tail}") };
            ctx.set_state("output", err.get("output").cloned().unwrap_or(Value::Null));
            let failed_install = p.action == "install";
            self.update(&p, ctx, |i| {
                if failed_install {
                    i.status = "failed".into();
                } else if i.status.ends_with("ing") {
                    i.status = "installed".into();
                }

                i.last_error = Some(message.clone());
            })
            .await?;

            return Err(StepError::Fatal(message));
        }

        let out: Value = serde_json::from_str(&result.output).unwrap_or(Value::Null);
        ctx.set_state("output", out.get("output").cloned().unwrap_or(Value::Null));
        let state = out.get("state").and_then(Value::as_str).map(str::to_owned);
        ctx.set_state("state", json!(state));

        if p.action == "uninstall" {
            self.installations
                .delete_installation(p.installation_id)
                .await
                .map_err(|e| StepError::Retryable(e.to_string()))?;
            return Ok(());
        }

        let env: BTreeMap<String, String> =
            out.get("env").cloned().and_then(|v| serde_json::from_value(v).ok()).unwrap_or_default();

        if matches!(p.action.as_str(), "versions" | "upgrade") {
            return self.record_versions(&p, ctx, state.as_deref().unwrap_or_default(), &out, env).await;
        }

        self.update(&p, ctx, |i| {
            i.status = if state.as_deref() == Some("not-installed") { "missing".into() } else { "installed".into() };
            i.state = state.clone();

            if !env.is_empty() {
                i.env = env;
            }

            i.last_error = None;
        })
        .await
    }
}

impl PackageJob {
    /// `versions` and `upgrade` end with the node's version report. After an
    /// upgrade the service must run the new build (same upstream version).
    async fn record_versions(
        &self,
        p: &Payload,
        ctx: &mut StepContext,
        last_line: &str,
        out: &Value,
        env: BTreeMap<String, String>,
    ) -> Result<(), StepError> {
        let Some(nv) = forge_catalog::versions::NodeVersions::parse(last_line) else {
            return Err(StepError::Fatal("the node did not report package versions".into()));
        };

        let upgraded = p.action == "upgrade";
        let run_state = upgraded
            .then(|| {
                out.get("output").and_then(Value::as_str).and_then(|o| {
                    o.lines().rev().find_map(|l| l.strip_prefix("forge: state: ").map(|s| s.trim().to_owned()))
                })
            })
            .flatten();
        let stale = upgraded && !nv.running.is_empty() && !runs_build(&nv.running, &nv.installed);
        ctx.set_state("versions", json!(nv));
        let now = self.clock.now();
        let message = format!("the service still reports {} after the update; restart it to finish", nv.running);
        self.update(p, ctx, |i| {
            i.versions = Some(PackageVersions {
                manager: nv.manager.clone(),
                package: nv.package.clone(),
                installed: nv.installed.clone(),
                candidate: nv.candidate.clone(),
                running: nv.running.clone(),
                checked_at: now,
            });

            if upgraded {
                i.status = "installed".into();

                if run_state.is_some() {
                    i.state.clone_from(&run_state);
                }

                if !env.is_empty() {
                    i.env = env;
                }

                i.last_error = stale.then(|| message.clone());
            }
        })
        .await?;

        if stale {
            return Err(StepError::Fatal(message));
        }

        Ok(())
    }
}

/// Whether a running version (`17.7 (Ubuntu 17.7-1)`, `7.4.1`) belongs to an
/// installed build (`17.7-1.pgdg24.04+1`): its numbers are a prefix.
#[must_use]
pub fn runs_build(running: &str, installed: &str) -> bool {
    let (r, i) = (forge_catalog::versions::numbers(running), forge_catalog::versions::numbers(installed));
    !r.is_empty() && (i.starts_with(&r) || r.starts_with(&i))
}

#[async_trait]
impl JobHandler for PackageJob {
    fn kind(&self) -> &'static str {
        "package.action"
    }

    fn steps(&self) -> &'static [&'static str] {
        &["dispatch", "await_result"]
    }

    fn required_action(&self, payload: &Value) -> Action {
        match payload.get("action").and_then(Value::as_str) {
            Some("status" | "versions") => Action::ServerManage,
            _ => Action::PackageManage,
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
    use super::{busy_status, runs_build};

    /// The job step outlasts the node's recipe limit (45 min), which itself
    /// includes up to five minutes of dpkg lock wait (#17).
    #[test]
    fn the_step_outlasts_the_recipe_and_the_dpkg_lock_wait() {
        assert!(super::RESULT_WAIT >= std::time::Duration::from_secs(45 * 60 + 300));
    }

    #[test]
    fn running_version_matches_build() {
        assert!(runs_build("17.7 (Ubuntu 17.7-1.pgdg24.04+1)", "17.7-1.pgdg24.04+1"));
        assert!(!runs_build("17.6", "17.7-1.pgdg24.04+1"));
        assert!(runs_build("7.4.1", "5:7.4.1-1rl1~noble1"));
        assert!(runs_build("1.26.2", "1.26.2-1~noble"));
    }

    #[test]
    fn busy_statuses() {
        assert_eq!(busy_status("install"), "installing");
        assert_eq!(busy_status("uninstall"), "removing");
        assert_eq!(busy_status("status"), "checking");
    }
}
