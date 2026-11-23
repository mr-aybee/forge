//! Enrollment: one-time token + CSR → signed certificate.

use std::time::Duration;

use anyhow::Context;
use forge_pki::AgentCsr;
use forge_proto::messages::{EnrollRequest, EnrollResponse};
use forge_secrets::crypto::b64url;
use prost::Message;

use crate::config::{AgentConfig, AgentIdentity, StatePaths};
use crate::link::{Endpoint, Link};
use crate::tls;

/// Facts sent with the enrollment request.
#[derive(Debug, Clone)]
pub struct HostFacts {
    pub hostname: String,
    pub os: String,
    pub arch: String,
}

impl HostFacts {
    #[must_use]
    pub fn detect() -> Self {
        Self {
            hostname: sysinfo::System::host_name().unwrap_or_else(|| "unknown".into()),
            os: std::env::consts::OS.into(),
            arch: std::env::consts::ARCH.into(),
        }
    }
}

/// Enrolls this machine and writes its state directory.
pub async fn enroll(
    paths: &StatePaths,
    config: AgentConfig,
    token: &str,
    facts: HostFacts,
) -> anyhow::Result<AgentIdentity> {
    enroll_with(paths, config, token, facts, None).await
}

/// [`enroll`], also reporting the terminal account the installer created
/// (D-078), which Forge saves to the node's login list.
pub async fn enroll_with(
    paths: &StatePaths,
    config: AgentConfig,
    token: &str,
    facts: HostFacts,
    terminal_account: Option<&str>,
) -> anyhow::Result<AgentIdentity> {
    let parsed = forge_pki::enrollment::parse(token).context("malformed enrollment token")?;
    let endpoint = Endpoint::parse(&config.server_url)?;
    let timeout = Duration::from_secs(30);

    // 1. Fetch the CA and check it against the fingerprint pinned in the token.
    let mut bootstrap = Link::new(endpoint.clone(), tls::ca_bootstrap()?, timeout);
    let downloaded = String::from_utf8(bootstrap.request(http::Method::GET, forge_proto::paths::CA, vec![]).await?)?;
    // Only the pinned certificate is trusted and saved; extra certificates are refused.
    let ca_pem = tls::pinned_ca_pem(&downloaded, &parsed.ca_fingerprint)?;

    // 2. Generate our key locally and send a CSR over verified TLS.
    let csr = AgentCsr::generate(&facts.hostname)?;
    let mut link = Link::new(endpoint, tls::anonymous(&ca_pem)?, timeout);
    let req = EnrollRequest {
        token_secret: parsed.secret,
        csr_pem: csr.csr_pem,
        hostname: facts.hostname,
        os: facts.os,
        arch: facts.arch,
        agent_version: crate::VERSION.into(),
        terminal_account: terminal_account.unwrap_or_default().into(),
    };
    let resp = EnrollResponse::decode(
        link.request(http::Method::POST, forge_proto::paths::ENROLL, req.encode_to_vec()).await?.as_slice(),
    )?;

    // 3. Persist.
    std::fs::create_dir_all(&paths.dir)?;
    std::fs::write(paths.ca(), &ca_pem)?;
    paths.save_credentials(&csr.key_pem, &resp.cert_pem)?;
    let identity = AgentIdentity {
        server_id: resp.server_id,
        command_public_key: b64url(&resp.command_public_key),
        heartbeat_interval_secs: resp.heartbeat_interval_secs,
    };
    paths.save_identity(&identity)?;
    let mut config = config;
    config.analytics_url = Some(resp.analytics_url).filter(|u| !u.is_empty());
    paths.save_config(&config)?;
    tracing::info!(server_id = %identity.server_id, "enrolled");
    Ok(identity)
}
