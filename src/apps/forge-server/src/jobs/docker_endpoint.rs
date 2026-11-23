//! `docker.endpoint.enable` / `docker.endpoint.disable`: a node's Docker
//! Engine API over mutual TLS (D-125).
//!
//! Enable payload: `{"server_id", "host", "port", "listen"?, "acknowledge_restart"}`.
//! Steps:
//! 1. `prepare`: the node makes dockerd's key (it never leaves the node)
//!    and returns a CSR, its addresses and whether a daemon restart stops
//!    containers.
//! 2. `sign`: a new CA for the node signs dockerd's certificate; the row is
//!    saved as `pending` with the CA key sealed. Refused when the restart
//!    would stop running containers and the caller did not acknowledge it.
//! 3. `apply`: the node installs the certificates and the listener, restarts
//!    dockerd and rolls back on failure.
//! 4. `verify`: the control plane calls `/version` over mutual TLS; the
//!    endpoint becomes `enabled`, or `failed` with the reason.
//!
//! Disable payload: `{"server_id", "force"}`. Steps: `remove` (the node
//! takes the listener away; skipped with `force` when the node is gone),
//! then `forget` (the row and the CA are deleted).
//!
//! Every step can run twice: the node keeps its key, a signed certificate is
//! kept in the job state, applying the same files is a no-op, and removing
//! twice finds nothing to remove. Both kinds need `server.command`.

use std::net::IpAddr;
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use forge_catalog::docker_tls::{DockerTlsOp, Prepared};
use forge_core::model::{AgentStatus, DockerEndpoint, DockerEndpointState};
use forge_core::ports::ServerRepository;
use forge_core::{Action, Clock, ServerId};
use forge_jobs::{JobHandler, StepContext, StepError};
use forge_proto::messages::DockerTlsAction;
use forge_proto::{Command, CommandSigner};
use serde::Deserialize;
use serde_json::{Value, json};

use crate::agent_hub::CommandQueue;
use crate::docker_engine::{DAEMON_CERT_VALIDITY, DockerEngines};

pub const ENABLE_KIND: &str = "docker.endpoint.enable";
pub const DISABLE_KIND: &str = "docker.endpoint.disable";
const COMMAND_TTL: Duration = Duration::from_secs(120);
/// Restarting dockerd waits up to two minutes, then a rollback may restart it again.
const APPLY_WAIT: Duration = Duration::from_secs(6 * 60);
const QUICK_WAIT: Duration = Duration::from_secs(90);

#[derive(Debug, Clone, Deserialize)]
struct EnablePayload {
    server_id: ServerId,
    host: String,
    port: u16,
    #[serde(default)]
    listen: Option<IpAddr>,
    #[serde(default)]
    acknowledge_restart: bool,
}

#[derive(Debug, Clone, Deserialize)]
struct DisablePayload {
    server_id: ServerId,
    #[serde(default)]
    force: bool,
}

fn retry(e: impl std::fmt::Display) -> StepError {
    StepError::Retryable(e.to_string())
}

fn fatal(e: impl std::fmt::Display) -> StepError {
    StepError::Fatal(e.to_string())
}

fn parse<T: serde::de::DeserializeOwned>(ctx: &StepContext) -> Result<T, StepError> {
    serde_json::from_value(ctx.job.payload.clone()).map_err(|e| fatal(format!("bad payload: {e}")))
}

/// What both kinds need to reach a node.
#[derive(Debug)]
pub struct NodeLink {
    pub hub: Arc<CommandQueue>,
    pub signer: Arc<CommandSigner>,
    pub servers: Arc<dyn ServerRepository>,
    pub engines: Arc<DockerEngines>,
    pub clock: Arc<dyn Clock>,
}

impl NodeLink {
    /// Sends `op` to the node's helper and waits for its answer. The
    /// operation id is kept under `key`, so a resumed step waits for the
    /// same operation, or sends it again after a control-plane restart.
    async fn run(
        &self,
        ctx: &mut StepContext,
        server: ServerId,
        key: &str,
        op: &DockerTlsOp,
        wait: Duration,
    ) -> Result<Value, StepError> {
        let s = self.servers.get_server(server).await.map_err(retry)?;
        if s.status == AgentStatus::Revoked {
            return Err(fatal("server is revoked"));
        }
        let tracked =
            ctx.state_value(key).and_then(Value::as_str).filter(|op| self.hub.is_tracked(op)).map(str::to_owned);
        let op_id = match tracked {
            Some(op_id) => op_id,
            None => {
                let op_json = serde_json::to_string(op).map_err(fatal)?;
                let (body, signed) = self.signer.issue(
                    &server.to_string(),
                    &ctx.job.principal.to_string(),
                    Command::DockerTlsAction(DockerTlsAction { op_json }),
                    self.clock.now(),
                    COMMAND_TTL,
                );
                self.hub.enqueue(server, &body.operation_id, signed).map_err(StepError::Retryable)?;
                ctx.set_state(key, json!(body.operation_id));
                body.operation_id
            }
        };
        let result = self.hub.wait(&op_id, wait).await.ok_or_else(|| retry("the node did not answer in time"))?;
        if !result.ok {
            return Err(fatal(result.error));
        }
        serde_json::from_str(&result.output).map_err(|e| fatal(format!("the node's answer is not JSON: {e}")))
    }
}

/// Turns on dockerd's TLS listener on one node.
#[derive(Debug)]
pub struct DockerEndpointEnableJob {
    pub link: NodeLink,
}

/// Turns it off again and forgets the node's Docker CA.
#[derive(Debug)]
pub struct DockerEndpointDisableJob {
    pub link: NodeLink,
}

/// The address dockerd should listen on: the one asked for, else the
/// dialled host when it is one of the node's own addresses.
fn listen_address(p: &EnablePayload, prepared: &Prepared) -> Result<IpAddr, String> {
    if let Some(l) = p.listen {
        return Ok(l);
    }
    match p.host.parse::<IpAddr>() {
        Ok(ip) if prepared.addresses.contains(&ip) => Ok(ip),
        _ => Err(format!(
            "`{}` is not one of the node's addresses ({}); pass `listen` with the address dockerd should listen on",
            p.host,
            prepared.addresses.iter().map(ToString::to_string).collect::<Vec<_>>().join(", ")
        )),
    }
}

impl DockerEndpointEnableJob {
    async fn prepare(&self, ctx: &mut StepContext) -> Result<(), StepError> {
        let p: EnablePayload = parse(ctx)?;
        let out = self.link.run(ctx, p.server_id, "prepare_op", &DockerTlsOp::Prepare {}, QUICK_WAIT).await?;
        let prepared: Prepared = serde_json::from_value(out.clone()).map_err(fatal)?;
        if !prepared.live_restore && prepared.running_containers > 0 && !p.acknowledge_restart {
            return Err(fatal(format!(
                "enabling restarts dockerd and {} running container(s) stop (live-restore is off); \
                 containers with a restart policy start again. Enable it again with `acknowledge_restart`",
                prepared.running_containers
            )));
        }
        ctx.set_state("prepared", out);
        Ok(())
    }

    async fn sign(&self, ctx: &mut StepContext) -> Result<(), StepError> {
        if ctx.state_value("cert_pem").is_some() {
            return Ok(());
        }
        let p: EnablePayload = parse(ctx)?;
        let prepared: Prepared =
            serde_json::from_value(ctx.state_value("prepared").cloned().unwrap_or(Value::Null)).map_err(fatal)?;
        let listen = listen_address(&p, &prepared).map_err(fatal)?;
        let engines = &self.link.engines;
        let (ca, sealed) = engines.new_node_ca(p.server_id).map_err(fatal)?;
        let mut sans: Vec<IpAddr> = Vec::new();
        if !listen.is_unspecified() {
            sans.push(listen);
        }
        if let Ok(ip) = p.host.parse::<IpAddr>()
            && !sans.contains(&ip)
        {
            sans.push(ip);
        }
        let now = engines.now();
        let cert = ca
            .issue_docker_daemon_certificate(
                &prepared.csr_pem,
                &p.server_id.to_string(),
                &sans,
                now,
                DAEMON_CERT_VALIDITY,
            )
            .map_err(fatal)?;
        let created_at = engines.endpoint(p.server_id).await.map_err(retry)?.map_or(now, |e| e.created_at);
        engines
            .put(&DockerEndpoint {
                server_id: p.server_id,
                host: p.host.clone(),
                port: p.port,
                state: DockerEndpointState::Pending,
                ca_cert_pem: ca.cert_pem().to_owned(),
                ca_key_sealed: sealed,
                daemon_cert_not_after: cert.not_after,
                engine_version: None,
                api_version: None,
                last_error: None,
                created_at,
                updated_at: now,
            })
            .await
            .map_err(retry)?;
        ctx.set_state("ca_pem", json!(ca.cert_pem()));
        ctx.set_state("cert_pem", json!(cert.cert_pem));
        ctx.set_state("listen", json!(listen));
        Ok(())
    }

    async fn apply(&self, ctx: &mut StepContext) -> Result<(), StepError> {
        let p: EnablePayload = parse(ctx)?;
        let get = |k: &str| ctx.state_value(k).and_then(Value::as_str).map(str::to_owned);
        let (Some(ca_pem), Some(cert_pem), Some(listen)) = (get("ca_pem"), get("cert_pem"), get("listen")) else {
            return Err(fatal("the certificates were not issued"));
        };
        let op = DockerTlsOp::Apply { ca_pem, cert_pem, listen: listen.parse().map_err(fatal)?, port: p.port };
        op.validate().map_err(fatal)?;
        match self.link.run(ctx, p.server_id, "apply_op", &op, APPLY_WAIT).await {
            Ok(out) => {
                ctx.set_state("applied", out);
                Ok(())
            }
            Err(e) => {
                self.mark_failed(p.server_id, &e).await;
                Err(e)
            }
        }
    }

    async fn verify(&self, ctx: &mut StepContext) -> Result<(), StepError> {
        let p: EnablePayload = parse(ctx)?;
        let deadline = tokio::time::Instant::now() + Duration::from_secs(45);
        loop {
            let ep = self.link.engines.check(p.server_id).await.map_err(retry)?;
            if ep.state == DockerEndpointState::Enabled {
                ctx.set_state("engine_version", json!(ep.engine_version));
                ctx.set_state("api_version", json!(ep.api_version));
                return Ok(());
            }
            if tokio::time::Instant::now() >= deadline {
                let why = ep.last_error.unwrap_or_else(|| "no answer".into());
                let e = fatal(format!("dockerd listens, but the control plane cannot reach it: {why}"));
                self.mark_failed(p.server_id, &e).await;
                return Err(e);
            }
            tokio::time::sleep(Duration::from_secs(2)).await;
        }
    }

    /// Records a failure on the row, so the node's page says why.
    async fn mark_failed(&self, server: ServerId, e: &StepError) {
        let msg = match e {
            StepError::Fatal(m) | StepError::Retryable(m) => m.clone(),
        };
        let engines = &self.link.engines;
        match engines.endpoint(server).await {
            Ok(Some(mut ep)) => {
                ep.state = DockerEndpointState::Failed;
                ep.last_error = Some(msg);
                ep.updated_at = engines.now();
                if let Err(err) = engines.put(&ep).await {
                    tracing::warn!(%server, error = %err, "could not record the Docker endpoint failure");
                }
            }
            Ok(None) => {}
            Err(err) => tracing::warn!(%server, error = %err, "could not load the Docker endpoint"),
        }
    }
}

#[async_trait]
impl JobHandler for DockerEndpointEnableJob {
    fn kind(&self) -> &'static str {
        ENABLE_KIND
    }

    fn steps(&self) -> &'static [&'static str] {
        &["prepare", "sign", "apply", "verify"]
    }

    fn required_action(&self, _payload: &Value) -> Action {
        Action::ServerCommand
    }

    async fn run_step(&self, index: usize, ctx: &mut StepContext) -> Result<(), StepError> {
        match index {
            0 => self.prepare(ctx).await,
            1 => self.sign(ctx).await,
            2 => self.apply(ctx).await,
            _ => self.verify(ctx).await,
        }
    }
}

#[async_trait]
impl JobHandler for DockerEndpointDisableJob {
    fn kind(&self) -> &'static str {
        DISABLE_KIND
    }

    fn steps(&self) -> &'static [&'static str] {
        &["remove", "forget"]
    }

    fn required_action(&self, _payload: &Value) -> Action {
        Action::ServerCommand
    }

    async fn run_step(&self, index: usize, ctx: &mut StepContext) -> Result<(), StepError> {
        let p: DisablePayload = parse(ctx)?;
        match index {
            0 if p.force => Ok(()),
            0 => {
                let out = self.link.run(ctx, p.server_id, "remove_op", &DockerTlsOp::Remove {}, APPLY_WAIT).await?;
                ctx.set_state("removed", out);
                Ok(())
            }
            _ => self.link.engines.delete(p.server_id).await.map_err(retry),
        }
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    fn payload(host: &str, listen: Option<&str>) -> EnablePayload {
        EnablePayload {
            server_id: ServerId::new(),
            host: host.into(),
            port: 2376,
            listen: listen.map(|l| l.parse().unwrap()),
            acknowledge_restart: false,
        }
    }

    #[test]
    fn dockerd_listens_on_the_dialled_address_or_the_one_asked_for() {
        let prepared = Prepared { addresses: vec!["10.0.0.5".parse().unwrap()], ..Prepared::default() };
        assert_eq!(listen_address(&payload("10.0.0.5", None), &prepared).unwrap().to_string(), "10.0.0.5");
        assert!(listen_address(&payload("node.example", None), &prepared).is_err());
        assert_eq!(
            listen_address(&payload("203.0.113.9", Some("10.0.0.5")), &prepared).unwrap().to_string(),
            "10.0.0.5"
        );
    }
}
