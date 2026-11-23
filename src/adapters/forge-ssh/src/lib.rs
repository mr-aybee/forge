//! SSH adapter (russh, ring crypto) implementing [`RemoteConnector`].
//!
//! Security properties:
//! * `probe` never sends credentials; it only records the host key.
//! * `connect` refuses every host key except the pinned fingerprint.
//! * Output is size-capped and every operation has a timeout.
//! * Terminal sessions authenticate with short-lived certificates from [`SshUserCa`].

mod ca;

pub use ca::{IssuedCert, SshUserCa};

use std::sync::{Arc, Mutex};
use std::time::Duration;

use async_trait::async_trait;
use forge_core::remote::{
    ExecEvent, ExecReader, ExecResult, HostKeyInfo, RemoteAuth, RemoteConnector, RemoteSession, RemoteTarget,
    ShellReader, ShellWriter, TermSize,
};
use forge_core::{DomainError, DomainResult};
use russh::ChannelMsg;
use russh::client::{self, AuthResult, Handle};
use russh::keys::{Certificate, HashAlg, PrivateKey, PrivateKeyWithHashAlg, PublicKeyOrCertificate};
use russh::{ChannelReadHalf, ChannelWriteHalf};

const MAX_OUTPUT: usize = 1024 * 1024;

/// Host-key policy for one connection.
struct Pin {
    expected: Option<String>,
    seen: Arc<Mutex<Option<HostKeyInfo>>>,
}

impl client::Handler for Pin {
    type Error = russh::Error;

    async fn check_server_key(&mut self, key: &PublicKeyOrCertificate) -> Result<bool, Self::Error> {
        let PublicKeyOrCertificate::PublicKey { key, .. } = key else {
            return Ok(false); // SSH certificates are not supported for bootstrap
        };
        let info = HostKeyInfo {
            algorithm: key.algorithm().as_str().to_owned(),
            fingerprint: key.fingerprint(HashAlg::Sha256).to_string(),
        };
        let accept = self.expected.as_deref().is_none_or(|e| e == info.fingerprint);
        if let Ok(mut s) = self.seen.lock() {
            *s = Some(info);
        }
        Ok(accept)
    }
}

/// russh-backed connector.
#[derive(Debug, Clone)]
pub struct SshConnector {
    timeout: Duration,
}

impl Default for SshConnector {
    fn default() -> Self {
        Self { timeout: Duration::from_secs(20) }
    }
}

impl SshConnector {
    #[must_use]
    pub fn new(timeout: Duration) -> Self {
        Self { timeout }
    }

    fn config(&self) -> Arc<client::Config> {
        Arc::new(client::Config { inactivity_timeout: Some(Duration::from_secs(300)), ..Default::default() })
    }

    async fn open(
        &self,
        host: &str,
        port: u16,
        expected: Option<String>,
    ) -> DomainResult<(Handle<Pin>, Option<HostKeyInfo>)> {
        let seen = Arc::new(Mutex::new(None));
        let pin = Pin { expected: expected.clone(), seen: seen.clone() };
        let res = tokio::time::timeout(self.timeout, client::connect(self.config(), (host, port), pin)).await;
        let info = seen.lock().ok().and_then(|s| s.clone());
        match res {
            Err(_) => Err(DomainError::Invalid(format!("SSH connection to {host}:{port} timed out"))),
            Ok(Ok(h)) => Ok((h, info)),
            Ok(Err(e)) => {
                if expected.is_some() && info.as_ref().is_some_and(|i| Some(&i.fingerprint) != expected.as_ref()) {
                    return Err(DomainError::Denied(format!(
                        "host key changed: expected {}, got {} — possible man-in-the-middle",
                        expected.unwrap_or_default(),
                        info.map(|i| i.fingerprint).unwrap_or_default()
                    )));
                }
                Err(DomainError::Invalid(format!("SSH connection failed: {e}")))
            }
        }
    }
}

#[async_trait]
impl RemoteConnector for SshConnector {
    async fn probe(&self, host: &str, port: u16) -> DomainResult<HostKeyInfo> {
        let (handle, info) = self.open(host, port, None).await?;
        let _ = handle.disconnect(russh::Disconnect::ByApplication, "probe", "en").await;
        info.ok_or_else(|| DomainError::Invalid("server sent no host key".into()))
    }

    async fn connect(&self, target: &RemoteTarget, auth: &RemoteAuth) -> DomainResult<Box<dyn RemoteSession>> {
        let (mut handle, _) = self.open(&target.host, target.port, Some(target.host_key_fingerprint.clone())).await?;
        let result = match auth {
            RemoteAuth::Password { password } => handle.authenticate_password(&target.username, password).await,
            RemoteAuth::PrivateKey { pem, passphrase } => {
                let key = PrivateKey::from_openssh(pem.as_bytes())
                    .map_err(|_| DomainError::Invalid("private key is not in OpenSSH format".into()))?;
                let key = match (key.is_encrypted(), passphrase) {
                    (true, Some(p)) => {
                        key.decrypt(p.as_bytes()).map_err(|_| DomainError::Invalid("wrong key passphrase".into()))?
                    }
                    (true, None) => return Err(DomainError::Invalid("private key needs a passphrase".into())),
                    (false, _) => key,
                };
                let hash = handle.best_supported_rsa_hash().await.ok().flatten().flatten();
                handle.authenticate_publickey(&target.username, PrivateKeyWithHashAlg::new(Arc::new(key), hash)).await
            }
            RemoteAuth::Certificate { key_openssh, cert_openssh } => {
                let key = PrivateKey::from_openssh(key_openssh.as_bytes()).map_err(DomainError::internal)?;
                let cert = Certificate::from_openssh(cert_openssh).map_err(DomainError::internal)?;
                handle.authenticate_openssh_cert(&target.username, Arc::new(key), cert).await
            }
        }
        .map_err(|e| DomainError::Invalid(format!("SSH authentication error: {e}")))?;
        match result {
            AuthResult::Success => Ok(Box::new(SshSession { handle, timeout: Duration::from_secs(600) })),
            AuthResult::Failure { .. } => {
                Err(DomainError::Denied("SSH authentication failed (wrong username or credential)".into()))
            }
        }
    }
}

struct SshSession {
    handle: Handle<Pin>,
    timeout: Duration,
}

#[async_trait]
impl RemoteSession for SshSession {
    async fn exec(&self, command: &str, stdin: Option<Vec<u8>>) -> DomainResult<ExecResult> {
        let run = async {
            let mut channel = self.handle.channel_open_session().await.map_err(DomainError::internal)?;
            channel.exec(true, command).await.map_err(DomainError::internal)?;
            if let Some(input) = stdin {
                channel.data(&input[..]).await.map_err(DomainError::internal)?;
                channel.eof().await.map_err(DomainError::internal)?;
            }
            let (mut out, mut err, mut status) = (Vec::new(), Vec::new(), None);
            while let Some(msg) = channel.wait().await {
                match msg {
                    ChannelMsg::Data { data } if out.len() < MAX_OUTPUT => out.extend_from_slice(&data),
                    ChannelMsg::ExtendedData { data, .. } if err.len() < MAX_OUTPUT => err.extend_from_slice(&data),
                    ChannelMsg::ExitStatus { exit_status } => status = Some(exit_status),
                    ChannelMsg::Close => break,
                    _ => {}
                }
            }
            Ok(ExecResult {
                status: status.unwrap_or(255),
                stdout: String::from_utf8_lossy(&out).into_owned(),
                stderr: String::from_utf8_lossy(&err).into_owned(),
            })
        };
        tokio::time::timeout(self.timeout, run)
            .await
            .map_err(|_| DomainError::Invalid("remote command timed out".into()))?
    }

    async fn shell(&self, term: &str, size: TermSize) -> DomainResult<(Box<dyn ShellReader>, Box<dyn ShellWriter>)> {
        let size = size.clamped();
        let channel = self.handle.channel_open_session().await.map_err(DomainError::internal)?;
        channel.request_pty(true, term, size.cols, size.rows, 0, 0, &[]).await.map_err(DomainError::internal)?;
        channel.request_shell(true).await.map_err(DomainError::internal)?;
        let (read, write) = channel.split();
        Ok((Box::new(SshShellReader { read }), Box::new(SshShellWriter { write })))
    }

    async fn shell_command(
        &self,
        term: &str,
        size: TermSize,
        command: &str,
    ) -> DomainResult<(Box<dyn ShellReader>, Box<dyn ShellWriter>)> {
        let size = size.clamped();
        let channel = self.handle.channel_open_session().await.map_err(DomainError::internal)?;
        channel.request_pty(true, term, size.cols, size.rows, 0, 0, &[]).await.map_err(DomainError::internal)?;
        channel.exec(true, command).await.map_err(DomainError::internal)?;
        let (read, write) = channel.split();
        Ok((Box::new(SshShellReader { read }), Box::new(SshShellWriter { write })))
    }

    async fn exec_stream(&self, command: &str) -> DomainResult<(Box<dyn ExecReader>, Box<dyn ShellWriter>)> {
        let channel = self.handle.channel_open_session().await.map_err(DomainError::internal)?;
        channel.exec(true, command).await.map_err(DomainError::internal)?;
        let (read, write) = channel.split();
        Ok((Box::new(SshExecReader { read, done: false }), Box::new(SshShellWriter { write })))
    }

    async fn close(self: Box<Self>) {
        let _ = self.handle.disconnect(russh::Disconnect::ByApplication, "done", "en").await;
    }
}

/// Exit status of a command killed by a signal: 128 + the signal number, as shells report it.
fn signal_status(name: &str) -> u32 {
    let n = match name.trim_start_matches("SIG").to_ascii_uppercase().as_str() {
        "HUP" => 1,
        "INT" => 2,
        "QUIT" => 3,
        "ILL" => 4,
        "ABRT" => 6,
        "FPE" => 8,
        "KILL" => 9,
        "USR1" => 10,
        "SEGV" => 11,
        "USR2" => 12,
        "PIPE" => 13,
        "ALRM" => 14,
        "TERM" => 15,
        _ => 0,
    };
    128 + n
}

struct SshExecReader {
    read: ChannelReadHalf,
    done: bool,
}

#[async_trait]
impl ExecReader for SshExecReader {
    async fn recv(&mut self) -> Option<ExecEvent> {
        if self.done {
            return None;
        }
        loop {
            match self.read.wait().await {
                Some(ChannelMsg::Data { data }) => return Some(ExecEvent::Stdout(data.to_vec())),
                Some(ChannelMsg::ExtendedData { data, .. }) => return Some(ExecEvent::Stderr(data.to_vec())),
                Some(ChannelMsg::ExitStatus { exit_status }) => {
                    self.done = true;
                    return Some(ExecEvent::Exit(exit_status));
                }
                Some(ChannelMsg::ExitSignal { signal_name, .. }) => {
                    self.done = true;
                    return Some(ExecEvent::Exit(signal_status(&format!("{signal_name:?}"))));
                }
                Some(ChannelMsg::Close) | None => {
                    self.done = true;
                    return None;
                }
                Some(_) => {}
            }
        }
    }
}

struct SshShellReader {
    read: ChannelReadHalf,
}

#[async_trait]
impl ShellReader for SshShellReader {
    async fn recv(&mut self) -> Option<Vec<u8>> {
        loop {
            match self.read.wait().await? {
                ChannelMsg::Data { data } | ChannelMsg::ExtendedData { data, .. } => return Some(data.to_vec()),
                ChannelMsg::Eof | ChannelMsg::Close | ChannelMsg::ExitStatus { .. } => return None,
                _ => {}
            }
        }
    }
}

struct SshShellWriter {
    write: ChannelWriteHalf<client::Msg>,
}

#[async_trait]
impl ShellWriter for SshShellWriter {
    async fn write(&self, data: &[u8]) -> DomainResult<()> {
        self.write.data_bytes(data.to_vec()).await.map_err(DomainError::internal)
    }

    async fn resize(&self, size: TermSize) -> DomainResult<()> {
        let size = size.clamped();
        self.write.window_change(size.cols, size.rows, 0, 0).await.map_err(DomainError::internal)
    }

    async fn eof(&self) -> DomainResult<()> {
        self.write.eof().await.map_err(DomainError::internal)
    }

    async fn close(&self) {
        let _ = self.write.eof().await;
        let _ = self.write.close().await;
    }
}
