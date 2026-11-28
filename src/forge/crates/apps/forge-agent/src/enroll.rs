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

/// Writes the pin set of a new enrollment (D-167): the key from the token
/// and the keys the server announced, checked against its command key.
/// Run as the agent user for the default state directory (the installer),
/// the set is staged for `forge-agent-helper pin` to adopt as root.
/// Nothing when neither is known (an older server): the agent then checks
/// the CA and host name, and pins once the server announces its keys.
fn save_pins(paths: &StatePaths, token_key: Option<&str>, resp: &EnrollResponse) -> anyhow::Result<()> {
    let now = forge_core::Clock::now(&forge_core::SystemClock);
    let mut set = crate::pins::PinSet::default();

    if let Some(key) = token_key {
        set.add(key, crate::pins::LABEL_ENROLLMENT, now.0)?;
    }

    if let Some(signed) = &resp.server_keys {
        let keys = forge_proto::verify_server_keys(&resp.command_public_key, signed, &resp.server_id, now)
            .map_err(|e| anyhow::anyhow!("the server's key announcement is invalid: {e}"))?;
        set.apply_verified(&keys, now.0)?;
    }

    if set.is_empty() {
        return Ok(());
    }

    let path = crate::pins::enrollment_target(&paths.dir, running_as_root());

    crate::pins::save(&path, &set).with_context(|| format!("writing {}", path.display()))
}

/// Whether this process runs as root (always false off Unix, where the pins
/// never live in the root-owned folder).
fn running_as_root() -> bool {
    #[cfg(unix)]
    {
        rustix::process::geteuid().is_root()
    }
    #[cfg(not(unix))]
    {
        false
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

    // 2. Generate our key locally and send a CSR over verified TLS: by the
    // control-plane key the token pins (D-167), so any address of the
    // server works; with an older token, by the CA and the host name.
    let csr = AgentCsr::generate(&facts.hostname)?;

    let enroll_tls = match &parsed.server_key_sha256 {
        Some(key) => tls::anonymous_pinned(std::slice::from_ref(key), &endpoint.authority())?,
        None => tls::anonymous(&ca_pem)?,
    };

    let mut link = Link::new(endpoint.clone(), enroll_tls, timeout);
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
    save_pins(paths, parsed.server_key_sha256.as_deref(), &resp)?;
    let identity = AgentIdentity {
        server_id: resp.server_id,
        command_public_key: b64url(&resp.command_public_key),
        heartbeat_interval_secs: resp.heartbeat_interval_secs,
    };
    paths.save_identity(&identity)?;
    let mut config = config;
    let analytics = crate::link::analytics_url(&endpoint, &resp.analytics_url, resp.analytics_port);
    config.analytics_url = Some(analytics).filter(|u| !u.is_empty());
    paths.save_config(&config)?;
    tracing::info!(server_id = %identity.server_id, "enrolled");

    Ok(identity)
}
