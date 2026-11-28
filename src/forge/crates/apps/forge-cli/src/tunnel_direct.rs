//! `forge tunnel open --direct` (#35 phase 2): one accepted local connection
//! goes straight to the node's SSH endpoint.
//!
//! For each connection the CLI asks the server for a certificate
//! (`POST /api/v1/tunnels/{id}/ssh-cert`). The server re-checks access,
//! then returns the node's SSH endpoint, its pinned host key and a
//! one-minute certificate with its own key. The CLI refuses any other host
//! key, logs in as the tunnel account and opens one `direct-tcpip` channel
//! to the tunnel's destination. The node's sshd allows that destination
//! only. The key lives in memory for this connection and is never written.

use std::sync::Arc;
use std::time::Duration;

use russh::client::{self, AuthResult};
use russh::keys::{Certificate, HashAlg, PrivateKey, PublicKeyOrCertificate};
use serde::Deserialize;
use serde_json::json;
use tokio::net::TcpStream;
use tokio::sync::Mutex;

use crate::api::Api;
use crate::error::{CliError, Exit, Result};

const CONNECT_TIMEOUT: Duration = Duration::from_secs(20);

/// The server's answer (`DirectCert`).
#[derive(Deserialize)]
struct DirectCert {
    host: String,
    port: u16,
    host_key_fingerprint: String,
    user: String,
    dial_ip: String,
    dial_port: u16,
    key_openssh: String,
    cert_openssh: String,
}

/// Accepts only the host key Forge pinned for the node.
struct PinnedHost {
    fingerprint: String,
    seen: Option<String>,
}

impl client::Handler for PinnedHost {
    type Error = russh::Error;

    async fn check_server_key(&mut self, key: &PublicKeyOrCertificate) -> std::result::Result<bool, Self::Error> {
        let PublicKeyOrCertificate::PublicKey { key, .. } = key else {
            return Ok(false);
        };

        let fp = key.fingerprint(HashAlg::Sha256).to_string();
        let ok = fp == self.fingerprint;
        self.seen = Some(fp);

        Ok(ok)
    }
}

fn fail(msg: impl Into<String>) -> CliError {
    CliError::new(Exit::General, msg.into())
}

/// Serves one accepted connection until either side closes it.
pub async fn connect(api: &Arc<Mutex<Api>>, tunnel_id: &str, mut sock: TcpStream) -> Result<()> {
    let _ = sock.set_nodelay(true);
    let path = format!("/api/v1/tunnels/{}/ssh-cert", crate::admincmd::enc(tunnel_id));
    let v = api.lock().await.post(&path, &json!({})).await?;
    let c: DirectCert =
        serde_json::from_value(v).map_err(|e| fail(format!("unexpected answer from the server: {e}")))?;
    let config = Arc::new(client::Config { keepalive_interval: Some(Duration::from_secs(30)), ..Default::default() });
    let handler = PinnedHost { fingerprint: c.host_key_fingerprint.clone(), seen: None };
    let addr = (c.host.as_str(), c.port);
    let mut handle = match tokio::time::timeout(CONNECT_TIMEOUT, client::connect(config, addr, handler)).await {
        Err(_) => return Err(fail(format!("SSH to {}:{} timed out; is it reachable from here?", c.host, c.port))),
        Ok(Err(russh::Error::UnknownKey)) => {
            return Err(fail(format!(
                "the node's SSH host key is not the pinned {} (possible man-in-the-middle); not connecting",
                c.host_key_fingerprint
            )));
        }
        Ok(Err(e)) => return Err(fail(format!("SSH to {}:{}: {e}", c.host, c.port))),
        Ok(Ok(h)) => h,
    };

    let key = PrivateKey::from_openssh(c.key_openssh.as_bytes()).map_err(|e| fail(format!("bad key: {e}")))?;
    let cert = Certificate::from_openssh(&c.cert_openssh).map_err(|e| fail(format!("bad certificate: {e}")))?;
    let auth = handle
        .authenticate_openssh_cert(&c.user, Arc::new(key), cert)
        .await
        .map_err(|e| fail(format!("SSH authentication: {e}")))?;

    if !matches!(auth, AuthResult::Success) {
        return Err(fail("the node refused the tunnel certificate (is its agent current?)"));
    }

    let channel = handle
        .channel_open_direct_tcpip(c.dial_ip.as_str(), u32::from(c.dial_port), "127.0.0.1", 0)
        .await
        .map_err(|e| fail(format!("the node refused to forward to {}:{}: {e}", c.dial_ip, c.dial_port)))?;
    let mut stream = channel.into_stream();
    let copied = tokio::io::copy_bidirectional(&mut sock, &mut stream).await;
    let _ = handle.disconnect(russh::Disconnect::ByApplication, "", "en").await;

    copied.map(|_| ()).map_err(|e| fail(e.to_string()))
}
