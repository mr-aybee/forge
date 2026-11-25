//! `node.bootstrap`: turn SSH access to a fresh machine into an enrolled node.
//!
//! The credential is used only here, read from the vault per step, and never
//! persisted in job state. Every step opens its own SSH session with the
//! pinned host key, so the job resumes cleanly after a control-plane restart.
//!
//! Steps: `connect` (preflight) → `install` (installer with an embedded
//! one-time token, run with sudo) → `await_online` → `cleanup` (purge the
//! credential unless the user chose to keep it for repairs).

use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use forge_core::model::{AgentStatus, EnrollmentToken};
use forge_core::model::{AuditEvent, AuditOutcome};
use forge_core::ports::{ClusterRepository, EnrollmentRepository, SecretRepository, ServerRepository};
use forge_core::remote::{RemoteAuth, RemoteConnector, RemoteSession, RemoteTarget};
use forge_core::*;
use forge_jobs::{JobHandler, StepContext, StepError};
use forge_secrets::Envelope;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::install::{AgentDist, InstallParams, render_script};

/// Sealed content of an `ssh.bootstrap` secret.
#[derive(Serialize, Deserialize)]
pub struct BootstrapCredential {
    pub auth: RemoteAuth,
    /// Password for `sudo` when it differs from the login password.
    pub sudo_password: Option<String>,
}

impl std::fmt::Debug for BootstrapCredential {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("BootstrapCredential").field("auth", &self.auth).field("sudo_password", &"<redacted>").finish()
    }
}

/// Job payload (no secrets: only the vault id).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BootstrapPayload {
    pub workspace_id: WorkspaceId,
    pub target: RemoteTarget,
    pub secret_id: SecretId,
    pub cluster_id: Option<ClusterId>,
    pub environment_id: Option<EnvironmentId>,
    pub keep_credential: bool,
    pub hostname: Option<String>,
    /// Terminal account the installer creates (D-078).
    #[serde(default)]
    pub terminal_account: Option<String>,
    #[serde(default)]
    pub terminal_account_sudo: bool,
    /// Where the node downloads from and links to: the address of the
    /// request that started the job (D-167). `None` in jobs from before it,
    /// which use the configured addresses.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub urls: Option<BootstrapUrls>,
}

/// The console and agent URLs given to the node.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BootstrapUrls {
    pub forge_url: String,
    pub agent_url: String,
}

impl From<crate::request_origin::NodeUrls> for BootstrapUrls {
    fn from(u: crate::request_origin::NodeUrls) -> Self {
        Self { forge_url: u.forge_url, agent_url: u.agent_url }
    }
}

#[derive(Debug)]
pub struct NodeBootstrapJob {
    pub connector: Arc<dyn RemoteConnector>,
    pub secrets: Arc<dyn SecretRepository>,
    pub servers: Arc<dyn ServerRepository>,
    pub enrollment: Arc<dyn EnrollmentRepository>,
    pub clusters: Arc<dyn ClusterRepository>,
    pub ssh_endpoints: Arc<dyn forge_core::ports::SshEndpointRepository>,
    /// The SSH user onboarding logged in as is saved for the terminal.
    pub logins: Arc<dyn forge_core::tags::NodeLoginRepository>,
    pub audit: Arc<dyn forge_core::ports::AuditRecorder>,
    pub envelope: Envelope,
    pub dist: Arc<AgentDist>,
    pub ca_fingerprint: String,
    /// The control-plane key new nodes pin (D-167).
    pub agent_keys: Arc<crate::agent_keys::AgentKeys>,
    /// OpenSSH public key of the SSH user CA (installed into sshd).
    pub ssh_user_ca: String,
    pub node_forge_url: String,
    pub node_agent_url: String,
    pub clock: Arc<dyn Clock>,
    pub online_timeout: Duration,
}

/// Saves the SSH user an onboarding logged in as to the node's terminal
/// logins (D-078), audited as `principal` (who onboarded it). Returns whether
/// it was added; a retried step finds it saved and adds nothing.
pub async fn save_onboarding_login(
    logins: &dyn forge_core::tags::NodeLoginRepository,
    audit: &dyn forge_core::ports::AuditRecorder,
    principal: &Principal,
    workspace: WorkspaceId,
    server: ServerId,
    login: &str,
    now: Timestamp,
) -> DomainResult<bool> {
    let Some(before) = crate::api::logins::save_login(logins, server, login, now).await? else {
        return Ok(false);
    };
    let mut after = before.clone();
    after.push(login.to_owned());
    let ev = AuditEvent::new(
        principal,
        "server.logins.update",
        EntityRef::new(EntityKind::Server, server),
        AuditOutcome::Success,
    )
    .in_org(Some(workspace))
    .with_details(json!({ "from": before, "to": after, "source": "onboarding" }));
    if let Err(e) = audit.record(ev).await {
        tracing::error!(error = %e, "audit write failed");
    }
    Ok(true)
}

pub fn secret_aad(id: SecretId) -> String {
    format!("secret:{id}:ssh.bootstrap")
}

/// Longest console log kept in the job state (lines).
const LOG_LINES: usize = 400;

/// Appends lines to the job's console log (`state.log`: `[{t, step, line}]`),
/// shown as a terminal view while onboarding. `secret` is masked first.
fn log(ctx: &mut StepContext, step: &str, text: &str, secret: Option<&str>) {
    let t = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| u64::try_from(d.as_millis()).unwrap_or(0));
    let mut lines: Vec<Value> = ctx.state_value("log").and_then(Value::as_array).cloned().unwrap_or_default();
    let mut live = Vec::new();
    for line in text.lines().map(str::trim_end).filter(|l| !l.trim().is_empty()) {
        let line = match secret.filter(|s| !s.is_empty()) {
            Some(s) => line.replace(s, "********"),
            None => line.to_owned(),
        };
        let line: String = line.chars().take(400).collect();
        lines.push(json!({ "t": t, "step": step, "line": line }));
        live.push(line);
    }
    // Also to the live console, so the browser sees it while the step runs.
    crate::api::onboarding::push(&crate::api::onboarding::job_key(ctx.job.id), step, &live);
    let excess = lines.len().saturating_sub(LOG_LINES);
    lines.drain(..excess);
    ctx.set_state("log", Value::Array(lines));
}

fn fatal(e: impl std::fmt::Display) -> StepError {
    StepError::Fatal(e.to_string())
}

/// Host-key and credential errors are permanent; network trouble is retried.
fn classify(e: DomainError) -> StepError {
    match e {
        DomainError::Denied(m) | DomainError::NotFound(m) => StepError::Fatal(m),
        other => StepError::Retryable(other.to_string()),
    }
}

impl NodeBootstrapJob {
    fn payload(ctx: &StepContext) -> Result<BootstrapPayload, StepError> {
        serde_json::from_value(ctx.job.payload.clone()).map_err(|e| fatal(format!("bad payload: {e}")))
    }

    /// Saves the SSH user onboarding logged in as to the node's terminal
    /// logins (D-078), audited as the person who onboarded it. Idempotent: a
    /// retried step finds it saved.
    async fn save_ssh_login(&self, ctx: &mut StepContext, p: &BootstrapPayload, id: ServerId) -> Result<(), StepError> {
        let login = p.target.username.as_str();
        let principal = ctx.job.principal.clone();
        let saved = save_onboarding_login(
            self.logins.as_ref(),
            self.audit.as_ref(),
            &principal,
            p.workspace_id,
            id,
            login,
            self.clock.now(),
        )
        .await
        .map_err(classify)?;
        if saved {
            log(ctx, "await_online", &format!("saved `{login}` as a terminal login for this node"), None);
        }
        Ok(())
    }

    async fn credential(&self, p: &BootstrapPayload) -> Result<BootstrapCredential, StepError> {
        let rec = self
            .secrets
            .get_secret(p.secret_id)
            .await
            .map_err(classify)?
            .ok_or_else(|| fatal("the SSH credential has expired or was deleted; start onboarding again"))?;
        let plain = self.envelope.open(&rec.sealed, &secret_aad(p.secret_id)).map_err(fatal)?;
        serde_json::from_slice(&plain).map_err(fatal)
    }

    async fn session(
        &self,
        p: &BootstrapPayload,
        cred: &BootstrapCredential,
    ) -> Result<Box<dyn RemoteSession>, StepError> {
        self.connector.connect(&p.target, &cred.auth).await.map_err(classify)
    }

    /// Wraps a command so it runs as root: directly, with passwordless sudo,
    /// or with `sudo -S` fed the password on stdin.
    fn as_root(ctx: &StepContext, cred: &BootstrapCredential, cmd: &str) -> (String, Option<Vec<u8>>) {
        let root = ctx.state_value("root").and_then(Value::as_bool).unwrap_or(false);
        let nopass = ctx.state_value("sudo_nopass").and_then(Value::as_bool).unwrap_or(false);
        if root {
            return (cmd.to_owned(), None);
        }
        if nopass {
            return (format!("sudo -n {cmd}"), None);
        }
        let pw = cred.sudo_password.clone().or_else(|| match &cred.auth {
            RemoteAuth::Password { password } => Some(password.clone()),
            RemoteAuth::PrivateKey { .. } | RemoteAuth::Certificate { .. } => None,
        });
        (format!("sudo -S -p '' {cmd}"), pw.map(|p| format!("{p}\n").into_bytes()))
    }

    /// Follows a background installer in `dir`: new output lines go to the
    /// console as they appear (token masked); returns all output and the
    /// exit code (`None` if the node stopped answering or it ran too long).
    async fn follow(
        &self,
        ctx: &mut StepContext,
        s: &dyn RemoteSession,
        dir: &str,
        token: &str,
    ) -> (String, Option<i32>) {
        const MARK: &str = "@@forge-output@@";
        let deadline = tokio::time::Instant::now() + Duration::from_secs(20 * 60);
        let (mut offset, mut all, mut partial) = (0usize, String::new(), String::new());
        let mut failures = 0;
        loop {
            let cmd =
                format!("cat {dir}/rc 2>/dev/null; echo {MARK}; tail -c +{} {dir}/out.log 2>/dev/null", offset + 1);
            match s.exec(&cmd, None).await {
                Ok(r) => {
                    failures = 0;
                    let (head, body) = r.stdout.split_once(&format!("{MARK}\n")).unwrap_or((r.stdout.as_str(), ""));
                    offset += body.len();
                    partial.push_str(body);
                    // Complete lines only; keep the rest for the next read.
                    if let Some(cut) = partial.rfind('\n') {
                        let done: String = partial.drain(..=cut).collect();
                        log(ctx, "install", &done, Some(token));
                        all.push_str(&done);
                    }
                    if let Ok(rc) = head.trim().parse::<i32>() {
                        if !partial.is_empty() {
                            log(ctx, "install", &partial, Some(token));
                            all.push_str(&partial);
                        }
                        return (all, Some(rc));
                    }
                }
                Err(_) => {
                    failures += 1;
                    if failures >= 5 {
                        return (all, None);
                    }
                }
            }
            if tokio::time::Instant::now() >= deadline {
                log(ctx, "install", "the installer is still running after 20 minutes; giving up", None);
                return (all, None);
            }
            tokio::time::sleep(Duration::from_millis(1500)).await;
        }
    }

    async fn connect_step(&self, ctx: &mut StepContext) -> Result<(), StepError> {
        let p = Self::payload(ctx)?;
        let cred = self.credential(&p).await?;
        let s = self.session(&p, &cred).await?;
        let probe = "uname -s; uname -m; id -u; \
                     if sudo -n true 2>/dev/null; then echo SUDO_NOPASS; else echo SUDO_PASS; fi; \
                     if [ -d /run/systemd/system ]; then echo SYSTEMD; else echo NOSYSTEMD; fi; \
                     (command -v curl || command -v wget) >/dev/null 2>&1 && echo FETCH_OK || echo FETCH_MISSING";
        let out = s.exec(probe, None).await.map_err(classify)?;
        s.close().await;
        let lines: Vec<&str> = out.stdout.lines().map(str::trim).collect();
        let [os, arch, uid, sudo, init, fetch, ..] = lines.as_slice() else {
            return Err(fatal(format!("unexpected preflight output: {}", out.stdout)));
        };
        if *os != "Linux" {
            return Err(fatal(format!("{os} is not supported by assisted onboarding yet; use the manual installer")));
        }
        if *fetch == "FETCH_MISSING" {
            return Err(fatal("the node has neither curl nor wget; install one and retry"));
        }
        let root = *uid == "0";
        if !root
            && *sudo == "SUDO_PASS"
            && cred.sudo_password.is_none()
            && matches!(cred.auth, RemoteAuth::PrivateKey { .. })
        {
            return Err(fatal("sudo needs a password: provide the sudo password"));
        }
        log(
            ctx,
            "connect",
            &format!("connected to {}:{} as {}", p.target.host, p.target.port, p.target.username),
            None,
        );
        log(
            ctx,
            "connect",
            &format!(
                "detected {os} {arch} · {} · {} · {}",
                if root {
                    "root".to_owned()
                } else if *sudo == "SUDO_NOPASS" {
                    "sudo (no password)".to_owned()
                } else {
                    "sudo (password)".to_owned()
                },
                if *init == "SYSTEMD" { "systemd" } else { "no systemd" },
                "curl/wget available"
            ),
            None,
        );
        ctx.set_state("os", json!(os));
        ctx.set_state("arch", json!(arch));
        ctx.set_state("root", json!(root));
        ctx.set_state("sudo_nopass", json!(*sudo == "SUDO_NOPASS"));
        ctx.set_state("systemd", json!(*init == "SYSTEMD"));
        Ok(())
    }

    async fn install_step(&self, ctx: &mut StepContext) -> Result<(), StepError> {
        if ctx.state_value("server_id").is_some() {
            return Ok(()); // already enrolled on a previous attempt
        }
        let p = Self::payload(ctx)?;
        if !self.dist.available() {
            return Err(fatal("no agent binaries are configured on the control plane (agent_dist_dir)"));
        }
        let cred = self.credential(&p).await?;

        // A fresh single-use token for this attempt; only its id is kept.
        let key = self.agent_keys.current_fingerprint();
        let generated = forge_pki::enrollment::generate(&self.ca_fingerprint, Some(&key));
        let token_id = forge_secrets::crypto::random_token::<9>();
        self.enrollment
            .create_token(&EnrollmentToken {
                id: token_id.clone(),
                workspace_id: p.workspace_id,
                environment_id: p.environment_id,
                token_hash: generated.secret_hash,
                created_by: ctx.job.principal.clone(),
                expires_at: self.clock.now().plus(Duration::from_secs(1800)),
                used_at: None,
                server_id: None,
            })
            .await
            .map_err(classify)?;
        ctx.set_state("token_id", json!(token_id));

        let script = render_script(
            &self.dist,
            &InstallParams {
                forge_url: p.urls.as_ref().map_or(&self.node_forge_url, |u| &u.forge_url),
                agent_url: p.urls.as_ref().map_or(&self.node_agent_url, |u| &u.agent_url),
                token: Some(&generated.display),
                hostname: p.hostname.as_deref(),
                terminal_account: p.terminal_account.as_deref(),
                terminal_account_sudo: p.terminal_account_sudo,
                ssh_user_ca: Some(&self.ssh_user_ca),
            },
        );
        let s = self.session(&p, &cred).await?;
        // A private directory (0700, the login user's) for the installer, its
        // runner, its output and its exit code.
        log(ctx, "install", "$ mktemp -d /tmp/forge-install.XXXXXX  # upload the installer", None);
        let up = s
            .exec(
                "umask 077; d=$(mktemp -d /tmp/forge-install.XXXXXX) && cat > \"$d/install.sh\" && \
                 printf '%s\\n' 'sh \"$1/install.sh\" > \"$1/out.log\" 2>&1; echo $? > \"$1/rc\"' > \"$d/run.sh\" && \
                 chmod 0755 \"$d\" && echo \"$d\"",
                Some(script.into_bytes()),
            )
            .await
            .map_err(classify)?;
        let dir = up.stdout.trim().to_owned();
        if !up.ok()
            || !dir.starts_with("/tmp/forge-install.")
            || !dir.bytes().all(|b| b.is_ascii_alphanumeric() || b"/._-".contains(&b))
        {
            s.close().await;
            return Err(StepError::Retryable(format!("could not upload installer: {}", up.stderr.trim())));
        }
        // Run it as root in the background, detached from this SSH channel,
        // and follow its output while it runs.
        let root = ctx.state_value("root").and_then(Value::as_bool).unwrap_or(false);
        log(ctx, "install", &format!("$ {}sh {dir}/install.sh", if root { "" } else { "sudo " }), None);
        let (cmd, stdin) = Self::as_root(
            ctx,
            &cred,
            &format!("sh -c 'setsid nohup sh {dir}/run.sh {dir} </dev/null >/dev/null 2>&1 &'"),
        );
        let start = s.exec(&cmd, stdin).await.map_err(classify)?;
        if !start.ok() {
            s.close().await;
            let e = start.stderr.trim();
            let msg = if e.contains("incorrect password") || e.contains("Sorry, try again") {
                "sudo rejected the password".to_owned()
            } else {
                format!("could not start the installer: {e}")
            };
            return Err(fatal(msg));
        }
        let token = generated.display.clone();
        let (output, rc) = self.follow(ctx, &*s, &dir, &token).await;
        let (cleanup, cstdin) = Self::as_root(ctx, &cred, &format!("rm -rf {dir}"));
        let _ = s.exec(&cleanup, cstdin).await;
        s.close().await;
        let rc = rc.ok_or_else(|| StepError::Retryable("lost track of the installer; it is retried".into()))?;
        if rc != 0 {
            log(ctx, "install", &format!("installer exited with status {rc}"), None);
            let tail: String = output
                .lines()
                .filter(|l| !l.trim().is_empty())
                .rev()
                .take(5)
                .collect::<Vec<_>>()
                .into_iter()
                .rev()
                .collect::<Vec<_>>()
                .join(" | ")
                .replace(&token, "********");
            return Err(fatal(format!("installer failed (exit {rc}): {tail}")));
        }
        let run_stdout = output;
        ctx.set_state("install_log", json!(run_stdout.lines().last().unwrap_or_default()));
        // A machine that was already enrolled keeps its identity: the
        // installer reports it, and onboarding continues with that server
        // (updated in place and placed in the chosen cluster) instead of
        // waiting for an enrollment that will never come.
        let reported = run_stdout
            .lines()
            .find_map(|l| l.trim().strip_prefix("forge-install: server "))
            .and_then(|s| s.trim().parse::<ServerId>().ok());
        if let Some(id) = reported
            && ctx.state_value("server_id").is_none()
            && let Ok(server) = self.servers.get_server(id).await
            && server.workspace_id == p.workspace_id
        {
            ctx.set_state("server_id", json!(id));
            log(
                ctx,
                "install",
                &format!("this machine is already enrolled as {}; it is updated in place", server.hostname),
                None,
            );
        }
        log(ctx, "install", "waiting for the agent to enroll and connect", None);
        Ok(())
    }

    async fn await_online_step(&self, ctx: &mut StepContext) -> Result<(), StepError> {
        let p = Self::payload(ctx)?;
        let token_id = ctx.state_value("token_id").and_then(Value::as_str).unwrap_or_default().to_owned();
        let deadline = tokio::time::Instant::now() + self.online_timeout;
        loop {
            let server_id = match ctx.state_value("server_id").and_then(Value::as_str) {
                Some(s) => s.parse::<ServerId>().ok(),
                None => self.enrollment.get_token(&token_id).await.map_err(classify)?.and_then(|t| t.server_id),
            };
            if let Some(id) = server_id {
                ctx.set_state("server_id", json!(id));
                let server = self.servers.get_server(id).await.map_err(classify)?;
                // Place the node in its cluster as soon as it has enrolled,
                // not only once it is online: a slow first connection or a
                // retried step must never leave it unassigned.
                if (p.cluster_id.is_some() && server.cluster_id != p.cluster_id)
                    || (p.environment_id.is_some() && server.environment_id != p.environment_id)
                {
                    self.clusters
                        .assign_server(
                            id,
                            p.cluster_id.or(server.cluster_id),
                            p.environment_id.or(server.environment_id),
                        )
                        .await
                        .map_err(classify)?;
                    log(
                        ctx,
                        "await_online",
                        &format!("enrolled as {}; placed in the selected cluster", server.hostname),
                        None,
                    );
                }
                if server.status == AgentStatus::Online {
                    // The browser terminal reuses the address and the host key
                    // the user confirmed for onboarding.
                    self.ssh_endpoints
                        .put_ssh_endpoint(&forge_core::model::SshEndpoint {
                            server_id: id,
                            host: p.target.host.clone(),
                            port: p.target.port,
                            host_key_fingerprint: p.target.host_key_fingerprint.clone(),
                            updated_at: self.clock.now(),
                        })
                        .await
                        .map_err(classify)?;
                    self.save_ssh_login(ctx, &p, id).await?;
                    log(
                        ctx,
                        "await_online",
                        &format!("agent enrolled as {} and online (agent {})", server.hostname, server.agent_version),
                        None,
                    );
                    return Ok(());
                }
            }
            if tokio::time::Instant::now() >= deadline {
                return Err(StepError::Retryable("the agent was installed but has not connected yet".into()));
            }
            tokio::time::sleep(Duration::from_secs(2)).await;
        }
    }

    async fn cleanup_step(&self, ctx: &mut StepContext) -> Result<(), StepError> {
        let p = Self::payload(ctx)?;
        if !p.keep_credential {
            self.secrets.delete_secret(p.secret_id).await.map_err(classify)?;
            ctx.set_state("credential", json!("purged"));
            log(ctx, "cleanup", "the SSH credential was deleted from the vault", None);
        } else {
            ctx.set_state("credential", json!("kept for repairs"));
            log(ctx, "cleanup", "the SSH credential is kept, sealed, for repairs", None);
        }
        log(ctx, "cleanup", "onboarding complete", None);
        Ok(())
    }
}

#[async_trait]
impl JobHandler for NodeBootstrapJob {
    fn kind(&self) -> &'static str {
        "node.bootstrap"
    }

    fn steps(&self) -> &'static [&'static str] {
        &["connect", "install", "await_online", "cleanup"]
    }

    fn required_action(&self, _payload: &Value) -> Action {
        Action::NodeOnboard
    }

    async fn run_step(&self, index: usize, ctx: &mut StepContext) -> Result<(), StepError> {
        match index {
            0 => self.connect_step(ctx).await,
            1 => self.install_step(ctx).await,
            2 => self.await_online_step(ctx).await,
            _ => self.cleanup_step(ctx).await,
        }
    }
}
