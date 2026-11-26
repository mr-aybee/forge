//! `agent.command`: sends one signed command to an agent and waits for the result.
//!
//! Payload: `{"server_id": "...", "command": {"type": "ping" | "collect_facts" | "service_status", "name"?: "..."}}`

use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use forge_core::model::AgentStatus;
use forge_core::ports::{Authorizer, CertificateRepository, ServerRepository, SiteRepository};
use forge_core::{AccessRequest, Action, Clock, EntityKind, EntityRef, ServerId};
use forge_jobs::{JobHandler, StepContext, StepError};
use forge_proto::messages::{
    CollectFacts, ContainerAction, DnsAction, FabricAction, IngressAction, NetProbe, Ping, ServiceStatus, SiteAction,
};
use forge_proto::{Command, CommandSigner};
use serde_json::{Value, json};

use crate::agent_hub::CommandQueue;

#[derive(Debug)]
pub struct AgentCommandJob {
    pub hub: Arc<CommandQueue>,
    pub signer: Arc<CommandSigner>,
    pub servers: Arc<dyn ServerRepository>,
    pub certificates: Arc<dyn CertificateRepository>,
    /// Sites, to check that an uploaded key goes only to the site's edges.
    pub sites: Arc<dyn SiteRepository>,
    /// Re-checks `certificate.use` on the workspace before a key is unsealed.
    pub authz: Arc<dyn Authorizer>,
    pub envelope: forge_secrets::Envelope,
    pub clock: Arc<dyn Clock>,
    /// Command validity and result wait time.
    pub ttl: Duration,
}

/// Container operations that only read state.
pub const CONTAINER_READ: &[&str] = &["info", "list", "images", "inspect", "logs", "stats"];
/// Container operations that change state (`container.manage`).
pub const CONTAINER_WRITE: &[&str] = &["start", "stop", "restart", "remove", "pull"];

/// Parses the `command` object of the payload.
pub fn parse_command(v: &Value) -> Result<Command, String> {
    match v.get("type").and_then(Value::as_str) {
        Some("ping") => Ok(Command::Ping(Ping {})),
        Some("collect_facts") => Ok(Command::CollectFacts(CollectFacts {})),
        Some("service_status") => {
            let name = v.get("name").and_then(Value::as_str).ok_or("service_status needs `name`")?;
            Ok(Command::ServiceStatus(ServiceStatus { name: name.to_owned() }))
        }
        Some("container") => {
            let s = |k: &str| v.get(k).and_then(Value::as_str).unwrap_or_default().to_owned();
            let op = s("op");
            if !CONTAINER_READ.contains(&op.as_str()) && !CONTAINER_WRITE.contains(&op.as_str()) {
                return Err(format!("unsupported container operation `{op}`"));
            }

            let tail = v.get("tail").and_then(Value::as_u64).map_or(0, |t| u32::try_from(t.min(5000)).unwrap_or(5000));

            // Values travel only for `inspect`, and only when asked.
            let reveal_env = op == "inspect" && reveals_env(v);
            Ok(Command::ContainerAction(ContainerAction {
                op,
                id: s("id"),
                image: s("image"),
                tail,
                since: s("since"),
                reveal_env,
            }))
        }
        Some("probe") => {
            let targets: Vec<String> = v
                .get("targets")
                .and_then(Value::as_array)
                .ok_or("probe needs `targets`")?
                .iter()
                .filter_map(|t| t.as_str().map(str::to_owned))
                .collect();

            if targets.is_empty() || targets.len() > 32 || targets.iter().any(|t| t.len() > 260) {
                return Err("probe: between 1 and 32 targets".into());
            }

            let count =
                v.get("count").and_then(Value::as_u64).map_or(4, |c| u32::try_from(c.clamp(1, 10)).unwrap_or(4));
            Ok(Command::NetProbe(NetProbe { targets, count }))
        }
        Some("dns") => {
            let op = v.get("op").cloned().ok_or("dns needs `op`")?;
            Ok(Command::DnsAction(DnsAction { op_json: op.to_string() }))
        }
        Some("fabric") => {
            let op: forge_catalog::fabric::FabricOp =
                serde_json::from_value(v.get("op").cloned().unwrap_or(Value::Null))
                    .map_err(|e| format!("invalid fabric operation: {e}"))?;
            op.validate()?;
            Ok(Command::FabricAction(FabricAction { op_json: serde_json::to_string(&op).map_err(|e| e.to_string())? }))
        }
        Some("ingress") => {
            let op: forge_catalog::ingress::IngressOp =
                serde_json::from_value(v.get("op").cloned().unwrap_or(Value::Null))
                    .map_err(|e| format!("invalid ingress operation: {e}"))?;
            op.validate()?;
            Ok(Command::IngressAction(IngressAction {
                op_json: serde_json::to_string(&op).map_err(|e| e.to_string())?,
            }))
        }
        Some("site") => {
            let action = v.get("action").and_then(Value::as_str).unwrap_or_default();
            if !matches!(action, "apply" | "remove" | "status" | "renew" | "clear_cache" | "access_log" | "error_log") {
                return Err(format!("unsupported site action `{action}`"));
            }

            let spec: forge_catalog::SiteSpec = serde_json::from_value(v.get("spec").cloned().unwrap_or(Value::Null))
                .map_err(|e| format!("invalid site: {e}"))?;
            spec.validate()?;

            if spec.material.is_some() {
                return Err("certificate material is added by Forge, not by the caller".into());
            }

            Ok(Command::SiteAction(SiteAction {
                action: action.to_owned(),
                spec_json: serde_json::to_string(&spec).map_err(|e| e.to_string())?,
            }))
        }
        other => Err(format!("unsupported command {other:?}")),
    }
}

/// Whether a `container` command asks for environment values (`reveal_env: true`).
fn reveals_env(command: &Value) -> bool {
    command.get("reveal_env").and_then(Value::as_bool) == Some(true)
}

/// Whether the payload is a container `inspect` that reveals environment values.
#[must_use]
pub fn reveals_container_env(payload: &Value) -> bool {
    let command = payload.get("command").unwrap_or(&Value::Null);
    command.get("type").and_then(Value::as_str) == Some("container")
        && command.get("op").and_then(Value::as_str) == Some("inspect")
        && reveals_env(command)
}

/// Splits the part of a result that must not be persisted off `output`:
/// the `env` values of a revealing `inspect`. The job table keeps the rest
/// (`env_keys` and all); the caller gets the values from memory once.
fn split_transient(payload: &Value, output: &mut Value) -> Option<Value> {
    if !reveals_container_env(payload) {
        return None;
    }

    let env = output.as_object_mut()?.remove("env")?;

    Some(json!({ "env": env }))
}

/// Top-level payload field set only by the endpoint flow (`api/sites.rs`)
/// after its own checks (approved roots for static directories): the id of
/// the endpoint. Commands sent through `POST /servers/{id}/commands` never
/// carry it.
pub const FROM_ENDPOINT: &str = "from_endpoint";

/// Whether a site spec serves a static directory (default or path rule).
fn has_static(spec: Option<&Value>) -> bool {
    let Some(spec) = spec else { return false };
    let is_static = |v: &Value| v.get("kind").and_then(Value::as_str) == Some("static");

    is_static(spec) || spec.get("rules").and_then(Value::as_array).is_some_and(|r| r.iter().any(is_static))
}

/// Top-level payload field set only by the endpoint flow (`POST /sites/{id}/apply`)
/// after it authorized `certificate.use`: the id of the site being applied.
/// The generic command endpoint cannot set it. Without it an uploaded
/// certificate is never attached; with it the job checks the site again.
pub const ATTACH_CERTIFICATE: &str = "attach_certificate";

/// AAD binding a sealed certificate key to its certificate.
#[must_use]
pub fn certificate_aad(id: forge_core::CertificateId) -> String {
    format!("tls-certificate:{id}")
}

impl AgentCommandJob {
    /// For an uploaded certificate, adds the chain and the unsealed key to the
    /// command sent to the edge. The job payload keeps only the certificate id.
    async fn attach_certificate(
        &self,
        site: &mut SiteAction,
        ws: forge_core::WorkspaceId,
        server: ServerId,
        from_site: Option<&str>,
        job: &forge_core::model::Job,
    ) -> Result<(), StepError> {
        let mut spec: forge_catalog::SiteSpec =
            serde_json::from_str(&site.spec_json).map_err(|e| StepError::Fatal(e.to_string()))?;

        if spec.tls != forge_catalog::sites::Tls::Uploaded || site.action != "apply" {
            return Ok(());
        }

        // The runner authorized `certificate.use` on this edge (see
        // `required_action`); the site id proves the endpoint flow sent it.
        let Some(site_id) = from_site else {
            return Err(StepError::Fatal(
                "uploaded certificates are sent only by applying the endpoint (POST /sites/{id}/apply)".into(),
            ));
        };

        let id: forge_core::CertificateId =
            spec.cert_id.parse().map_err(|_| StepError::Fatal("invalid certificate id".into()))?;
        self.check_site_edge(site_id, ws, server, &spec.cert_id).await?;

        // Re-checked at every attempt: a revoked right stops the key here.
        let req = AccessRequest::new(
            job.principal.clone(),
            Action::CertificateUse,
            EntityRef::new(EntityKind::Workspace, ws),
            job.context,
        );
        self.authz.authorize(&req).await.map_err(|e| StepError::Fatal(e.to_string()))?;
        let cert = self
            .certificates
            .get_certificate(id)
            .await
            .map_err(|e| StepError::Retryable(e.to_string()))?
            .filter(|c| c.workspace_id == ws)
            .ok_or_else(|| StepError::Fatal("the uploaded certificate no longer exists".into()))?;
        let key = self
            .envelope
            .open(&cert.sealed_key, &certificate_aad(cert.id))
            .map_err(|_| StepError::Fatal("cannot unseal the certificate key".into()))?;
        let key_pem = String::from_utf8(key).map_err(|_| StepError::Fatal("certificate key is not PEM".into()))?;
        spec.material = Some(forge_catalog::sites::CertMaterial { chain_pem: cert.chain_pem, key_pem });
        site.spec_json = serde_json::to_string(&spec).map_err(|e| StepError::Fatal(e.to_string()))?;

        Ok(())
    }

    /// The key goes only to an edge of the site being applied, and only for
    /// the certificate that site uses.
    async fn check_site_edge(
        &self,
        site_id: &str,
        ws: forge_core::WorkspaceId,
        server: ServerId,
        cert_id: &str,
    ) -> Result<(), StepError> {
        let site = self
            .sites
            .get_site(site_id)
            .await
            .map_err(|e| StepError::Retryable(e.to_string()))?
            .filter(|s| s.workspace_id == ws)
            .ok_or_else(|| StepError::Fatal("the endpoint no longer exists".into()))?;
        let edge =
            site.config["edges"].as_array().is_some_and(|e| e.iter().any(|v| v.as_str() == Some(&server.to_string())));

        if !edge {
            return Err(StepError::Fatal("this node is not an edge of the endpoint".into()));
        }

        if site.config.pointer("/spec/cert_id").and_then(Value::as_str) != Some(cert_id) {
            return Err(StepError::Fatal("the endpoint does not use this certificate".into()));
        }

        Ok(())
    }

    fn server_id(ctx: &StepContext) -> Result<ServerId, StepError> {
        ctx.payload_str("server_id")
            .and_then(|s| s.parse().ok())
            .ok_or_else(|| StepError::Fatal("payload.server_id missing or invalid".into()))
    }

    async fn dispatch(&self, ctx: &mut StepContext) -> Result<(), StepError> {
        let server_id = Self::server_id(ctx)?;
        let mut command =
            parse_command(ctx.job.payload.get("command").unwrap_or(&Value::Null)).map_err(StepError::Fatal)?;
        let server = self.servers.get_server(server_id).await.map_err(|e| StepError::Fatal(e.to_string()))?;
        if server.status == AgentStatus::Revoked {
            return Err(StepError::Fatal("server is revoked".into()));
        }

        if let Command::SiteAction(site) = &mut command {
            let from_site = ctx.job.payload.get(ATTACH_CERTIFICATE).and_then(Value::as_str);
            self.attach_certificate(site, server.workspace_id, server_id, from_site, &ctx.job).await?;
        }

        let (body, signed) = self.signer.issue(
            &server_id.to_string(),
            &ctx.job.principal.to_string(),
            command,
            self.clock.now(),
            self.ttl,
        );
        self.hub.enqueue(server_id, &body.operation_id, signed).map_err(StepError::Retryable)?;
        ctx.set_state("operation_id", json!(body.operation_id));

        Ok(())
    }
}

/// The action an `agent.command` needs on its node, re-checked by the job
/// runner before every step.
#[must_use]
pub fn required_action(payload: &Value) -> Action {
    match payload.pointer("/command/type").and_then(Value::as_str) {
        Some("ping" | "collect_facts" | "service_status" | "probe") => Action::ServerManage,
        Some("dns") => match payload.pointer("/command/op/op").and_then(Value::as_str) {
            Some("zones" | "records") => Action::ServerManage,
            _ => Action::ProxyManage,
        },
        Some("site") => match payload.pointer("/command/action").and_then(Value::as_str) {
            Some("status" | "access_log" | "error_log") => Action::ServerManage,
            Some("remove" | "clear_cache") => Action::ProxyManage,
            // The uploaded key leaves the vault: `certificate.use` (admins,
            // MFA), which also covers `server.command` and `proxy.manage`.
            Some("apply") if payload.pointer("/command/spec/tls").and_then(Value::as_str) == Some("uploaded") => {
                Action::CertificateUse
            }
            // A custom block runs as root inside nginx.
            _ if has_custom(payload.pointer("/command/spec/custom")) => Action::ServerCommand,
            // A static directory is served by nginx (root): the endpoint flow
            // checked it is inside an approved file root on this edge; any
            // other caller needs `server.command`.
            Some("apply") if has_static(payload.pointer("/command/spec")) && payload.get(FROM_ENDPOINT).is_none() => {
                Action::ServerCommand
            }
            _ => Action::ProxyManage,
        },
        Some("ingress") => match payload.pointer("/command/op/op").and_then(Value::as_str) {
            Some("status" | "network_inspect") => Action::ServerManage,
            Some("network_ensure") => Action::ContainerManage,
            Some("apply") if has_custom(payload.pointer("/command/op/nginx/custom")) => Action::ServerCommand,
            _ => Action::ProxyManage,
        },
        // Host networking: changing it is a privileged command (MFA).
        Some("fabric") => match payload.pointer("/command/op/op").and_then(Value::as_str) {
            Some("key" | "status") => Action::ServerManage,
            _ => Action::ServerCommand,
        },
        Some("container") => match payload.pointer("/command/op").and_then(Value::as_str) {
            // Environment values often hold secrets: a guarded action (MFA).
            Some("inspect") if reveals_container_env(payload) => Action::ContainerInspectEnv,
            Some(op) if CONTAINER_READ.contains(&op) => Action::ServerManage,
            _ => Action::ContainerManage,
        },
        _ => Action::ServerCommand,
    }
}

/// Whether a custom nginx block is present (non-blank).
fn has_custom(v: Option<&Value>) -> bool {
    v.and_then(Value::as_str).is_some_and(|s| !s.trim().is_empty())
}

#[async_trait]
impl JobHandler for AgentCommandJob {
    fn kind(&self) -> &'static str {
        "agent.command"
    }

    fn steps(&self) -> &'static [&'static str] {
        &["dispatch", "await_result"]
    }

    fn required_action(&self, payload: &Value) -> Action {
        required_action(payload)
    }

    async fn run_step(&self, index: usize, ctx: &mut StepContext) -> Result<(), StepError> {
        match index {
            0 => self.dispatch(ctx).await,
            _ => {
                let op = ctx.state_value("operation_id").and_then(Value::as_str).map(str::to_owned);

                // After a control-plane restart the in-memory waiter is gone:
                // re-dispatch (commands are idempotent reads) and wait again.
                let op = match op {
                    Some(op) if self.hub.is_tracked(&op) => op,
                    _ => {
                        self.dispatch(ctx).await?;
                        ctx.state_value("operation_id").and_then(Value::as_str).unwrap_or_default().to_owned()
                    }
                };

                // Image pulls can take a while; everything else is quick.
                let pulling = ctx.job.payload.pointer("/command/op").and_then(Value::as_str) == Some("pull")
                    || ctx.job.payload.pointer("/command/action").and_then(Value::as_str) == Some("apply");
                let wait = if pulling { Duration::from_secs(20 * 60) } else { self.ttl };
                let result = self
                    .hub
                    .wait(&op, wait)
                    .await
                    .ok_or_else(|| StepError::Retryable("agent did not respond in time".into()))?;

                // A coded fabric refusal (old or missing Docker for the
                // container network) is an answer, not a failure: the job
                // engine keeps no state for failed jobs, and the reconciler
                // needs the code to stop retrying (D-172).
                let fabric = ctx.job.payload.pointer("/command/type").and_then(Value::as_str) == Some("fabric");

                if !result.ok && fabric && !result.error_code.is_empty() {
                    ctx.set_state(
                        "output",
                        json!({ "refusal": { "code": result.error_code, "message": result.error } }),
                    );

                    return Ok(());
                }

                if !result.ok {
                    return Err(StepError::Fatal(format!("agent refused or failed: {}", result.error)));
                }

                let mut output: Value = serde_json::from_str(&result.output).unwrap_or(Value::String(result.output));

                // Revealed environment values never reach the job table.
                if let Some(secret) = split_transient(&ctx.job.payload, &mut output) {
                    self.hub.stash_transient(&ctx.job.id.to_string(), secret);
                }

                ctx.set_state("output", output);
                Ok(())
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use forge_core::Action;
    use serde_json::json;

    use super::{required_action, split_transient};

    #[test]
    fn revealing_container_env_is_its_own_action_and_never_persisted() {
        let plain = json!({ "command": { "type": "container", "op": "inspect", "id": "web" } });
        let reveal = json!({ "command": { "type": "container", "op": "inspect", "id": "web", "reveal_env": true } });
        let other = json!({ "command": { "type": "container", "op": "stats", "id": "web", "reveal_env": true } });
        assert_eq!(required_action(&plain), Action::ServerManage);
        assert_eq!(required_action(&reveal), Action::ContainerInspectEnv);
        assert_eq!(required_action(&other), Action::ServerManage, "only inspect carries values");
        let mut out = json!({ "id": "web", "env_keys": ["A"], "env": [{ "key": "A", "value": "1" }] });
        assert!(split_transient(&plain, &mut out.clone()).is_none());
        let secret = split_transient(&reveal, &mut out);
        assert_eq!(secret, Some(json!({ "env": [{ "key": "A", "value": "1" }] })));
        assert_eq!(out, json!({ "id": "web", "env_keys": ["A"] }), "persisted part keeps names only");
    }

    #[test]
    fn site_and_ingress_commands_need_the_right_action() {
        let site = |action: &str, spec: serde_json::Value| {
            required_action(&json!({ "command": { "type": "site", "action": action, "spec": spec } }))
        };
        assert_eq!(site("status", json!({ "tls": "uploaded", "custom": "x" })), Action::ServerManage);
        assert_eq!(site("apply", json!({ "custom": "" })), Action::ProxyManage);
        assert_eq!(site("apply", json!({ "custom": "add_header X 1;" })), Action::ServerCommand);
        assert_eq!(site("renew", json!({ "custom": "add_header X 1;" })), Action::ServerCommand);
        assert_eq!(site("apply", json!({ "tls": "uploaded", "custom": "x" })), Action::CertificateUse);
        assert_eq!(site("remove", json!({ "tls": "uploaded", "custom": "x" })), Action::ProxyManage);

        // Static directories: only the endpoint flow (which checked the
        // approved roots) gets away with proxy.manage.
        let st = json!({ "command": { "type": "site", "action": "apply", "spec": { "kind": "static" } } });
        assert_eq!(required_action(&st), Action::ServerCommand);
        let rule =
            json!({ "command": { "type": "site", "action": "apply", "spec": { "rules": [{ "kind": "static" }] } } });
        assert_eq!(required_action(&rule), Action::ServerCommand);
        let mut flow = st.clone();
        flow[super::FROM_ENDPOINT] = json!("site-1");
        assert_eq!(required_action(&flow), Action::ProxyManage);
        assert_eq!(site("access_log", json!({})), Action::ServerManage);
        assert_eq!(site("clear_cache", json!({})), Action::ProxyManage);
        let ingress = |custom: &str| {
            required_action(
                &json!({ "command": { "type": "ingress", "op": { "op": "apply", "nginx": { "custom": custom } } } }),
            )
        };
        assert_eq!(ingress("  "), Action::ProxyManage);
        assert_eq!(ingress("gzip on;"), Action::ServerCommand);
    }
}
