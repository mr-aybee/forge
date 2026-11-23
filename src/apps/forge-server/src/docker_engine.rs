//! Nodes' Docker Engine API over mutual TLS (D-125): the control plane's
//! side.
//!
//! Each node with the endpoint has its own Docker CA, created when the
//! endpoint is enabled. Its key is sealed with the master key (associated
//! data `docker-ca:<server id>`, so a sealed key moved to another row does
//! not open). The CA signs dockerd's certificate (for the key the node
//! made) and this server's client certificates, which live in memory for
//! [`CLIENT_CERT_VALIDITY`] and are renewed after two thirds of it.
//! Disabling the endpoint deletes the row and with it the CA: no
//! certificate it signed is trusted again.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use forge_core::model::Server;
use forge_core::model::{DockerEndpoint, DockerEndpointState};
use forge_core::ports::{DockerEndpointRepository, ServerRepository};
use forge_core::{Clock, DomainError, DomainResult, ServerId, Timestamp};
use forge_docker::Target;
use forge_pki::CertificateAuthority;
use forge_secrets::Envelope;

/// Lifetime of the control plane's client certificates.
pub const CLIENT_CERT_VALIDITY: Duration = Duration::from_secs(24 * 3600);
/// Lifetime of dockerd's certificate; re-enabling issues a new one.
pub const DAEMON_CERT_VALIDITY: Duration = Duration::from_secs(825 * 24 * 3600);
/// Common name of the control plane's client certificates.
pub const CLIENT_NAME: &str = "forge-server";

/// Associated data of a node's sealed Docker CA key.
#[must_use]
pub fn ca_aad(server: ServerId) -> String {
    format!("docker-ca:{server}")
}

struct Cached {
    target: Target,
    renew_at: Timestamp,
    /// The CA the target was built for; a re-enabled node has a new one.
    ca_pem: String,
    address: String,
}

/// Builds and caches TLS targets for nodes' Docker endpoints.
pub struct DockerEngines {
    repo: Arc<dyn DockerEndpointRepository>,
    servers: Arc<dyn ServerRepository>,
    envelope: Envelope,
    clock: Arc<dyn Clock>,
    cache: Mutex<HashMap<ServerId, Cached>>,
}

impl std::fmt::Debug for DockerEngines {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DockerEngines").finish_non_exhaustive()
    }
}

impl DockerEngines {
    #[must_use]
    pub fn new(
        repo: Arc<dyn DockerEndpointRepository>,
        servers: Arc<dyn ServerRepository>,
        envelope: Envelope,
        clock: Arc<dyn Clock>,
    ) -> Self {
        Self { repo, servers, envelope, clock, cache: Mutex::new(HashMap::new()) }
    }

    /// The node itself (for its workspace and status).
    pub async fn server(&self, server: ServerId) -> DomainResult<Server> {
        self.servers.get_server(server).await
    }

    /// The node's endpoint row, if any.
    pub async fn endpoint(&self, server: ServerId) -> DomainResult<Option<DockerEndpoint>> {
        self.repo.get_docker_endpoint(server).await
    }

    /// Opens the node's sealed Docker CA.
    pub fn node_ca(&self, ep: &DockerEndpoint) -> DomainResult<CertificateAuthority> {
        let key = self.envelope.open(&ep.ca_key_sealed, &ca_aad(ep.server_id)).map_err(DomainError::internal)?;
        let key = String::from_utf8(key).map_err(DomainError::internal)?;
        CertificateAuthority::from_pem(&ep.ca_cert_pem, &key).map_err(DomainError::internal)
    }

    /// A new CA for `server`, with its key sealed for storage.
    pub fn new_node_ca(&self, server: ServerId) -> DomainResult<(CertificateAuthority, String)> {
        let ca = CertificateAuthority::generate(&format!("Orbyn Forge Docker CA {server}"), self.clock.now())
            .map_err(DomainError::internal)?;
        let sealed = self.envelope.seal(ca.key_pem().as_bytes(), &ca_aad(server)).map_err(DomainError::internal)?;
        Ok((ca, sealed))
    }

    /// A target for the node's daemon: its endpoint must exist and be
    /// enabled, or be in any state with `any_state` (the enable job and
    /// checks, which may move it to enabled).
    pub async fn target(&self, server: ServerId, any_state: bool) -> DomainResult<(DockerEndpoint, Target)> {
        let ep = self
            .repo
            .get_docker_endpoint(server)
            .await?
            .ok_or_else(|| DomainError::Conflict("this node's Docker API endpoint is not enabled".into()))?;
        if ep.state != DockerEndpointState::Enabled && !any_state {
            return Err(DomainError::Conflict(format!(
                "this node's Docker API endpoint is {}{}",
                ep.state,
                ep.last_error.as_deref().map(|e| format!(": {e}")).unwrap_or_default()
            )));
        }
        let address = format!("{}:{}", bracket(&ep.host), ep.port);
        let now = self.clock.now();
        if let Ok(cache) = self.cache.lock()
            && let Some(c) = cache.get(&server)
            && c.renew_at > now
            && c.ca_pem == ep.ca_cert_pem
            && c.address == address
        {
            return Ok((ep, c.target.clone()));
        }
        let ca = self.node_ca(&ep)?;
        let client =
            ca.issue_client_certificate(CLIENT_NAME, now, CLIENT_CERT_VALIDITY).map_err(DomainError::internal)?;
        let target = Target::new(
            address.clone(),
            forge_pki::docker_dns_name(&server.to_string()),
            &ep.ca_cert_pem,
            &client.cert_pem,
            &client.key_pem,
        )
        .map_err(DomainError::internal)?;
        let renew_at = now.plus(CLIENT_CERT_VALIDITY * 2 / 3);
        if let Ok(mut cache) = self.cache.lock() {
            cache.insert(server, Cached { target: target.clone(), renew_at, ca_pem: ep.ca_cert_pem.clone(), address });
        }
        Ok((ep, target))
    }

    /// Forgets the node's cached target (the endpoint changed or is gone).
    pub fn forget(&self, server: ServerId) {
        if let Ok(mut cache) = self.cache.lock() {
            cache.remove(&server);
        }
    }

    /// Asks the daemon `/version` and records the answer (or the failure)
    /// on the endpoint. Pending endpoints become enabled when it answers.
    pub async fn check(&self, server: ServerId) -> DomainResult<DockerEndpoint> {
        // Any state: checking a failed endpoint again is how it recovers.
        let (mut ep, target) = self.target(server, true).await?;
        let now = self.clock.now();
        match target.get_json("/version", Duration::from_secs(15)).await {
            Ok((200, v)) => {
                ep.state = DockerEndpointState::Enabled;
                ep.engine_version = v["Version"].as_str().map(str::to_owned);
                ep.api_version = v["ApiVersion"].as_str().map(str::to_owned);
                ep.last_error = None;
            }
            Ok((status, _)) => {
                ep.last_error = Some(format!("the daemon answered HTTP {status}"));
            }
            Err(e) => {
                ep.last_error = Some(e.to_string());
            }
        }
        ep.updated_at = now;
        self.repo.put_docker_endpoint(&ep).await?;
        Ok(ep)
    }

    /// Removes the endpoint row (and with it the CA).
    pub async fn delete(&self, server: ServerId) -> DomainResult<()> {
        self.forget(server);
        self.repo.delete_docker_endpoint(server).await
    }

    pub async fn put(&self, ep: &DockerEndpoint) -> DomainResult<()> {
        self.forget(ep.server_id);
        self.repo.put_docker_endpoint(ep).await
    }

    #[must_use]
    pub fn now(&self) -> Timestamp {
        self.clock.now()
    }
}

/// `[v6]` for IPv6 literals, so `host:port` parses.
fn bracket(host: &str) -> String {
    if host.contains(':') && !host.starts_with('[') { format!("[{host}]") } else { host.to_owned() }
}
