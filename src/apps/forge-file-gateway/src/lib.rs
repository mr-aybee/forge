//! Forge-managed SFTP gateway.
//!
//! Forge terminates SSH itself and offers only the `sftp` subsystem: no
//! shell, command execution, port forwarding or agent forwarding. Users log
//! in with a temporary access grant (user name = grant id, password = its
//! token). Every SFTP request becomes a file-service operation run as the
//! grant, so it gets the same root resolution, Cedar checks (as the grant's
//! creator), protected-path policy, node confinement and audit as the
//! console. Sessions close when their grant expires, is revoked, has its
//! password reset or loses access they may be using.

mod sftp;

use std::collections::HashMap;
use std::net::{IpAddr, SocketAddr};
use std::path::Path;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use forge_core::model::AuditOutcome;
use forge_files::{Actor, FileService};
use russh::keys::ssh_key::private::{Ed25519Keypair, KeypairData};
use russh::keys::ssh_key::{HashAlg, LineEnding, PrivateKey};
use russh::server::{Auth, Handle, Msg, Session};
use russh::{Channel, ChannelId, MethodKind, MethodSet};
use serde_json::json;
use tokio::net::TcpListener;
use tokio::sync::watch;

/// Failed logins from one address before it is refused for a while.
const MAX_FAILURES: u32 = 10;
const FAILURE_WINDOW: Duration = Duration::from_secs(5 * 60);
/// How often open sessions are checked against their grant.
const SWEEP: Duration = Duration::from_secs(5);

/// The gateway's SSH host key. Clients verify its fingerprint.
pub struct HostKey {
    key: PrivateKey,
}

impl std::fmt::Debug for HostKey {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("HostKey").field("fingerprint", &self.fingerprint()).finish()
    }
}

impl HostKey {
    /// Loads the Ed25519 host key at `path`, creating it (mode 0600) on first
    /// use. Delete the file and restart to rotate it.
    pub fn load_or_create(path: &Path) -> Result<Self, String> {
        load_or_create_host_key(path).map(|key| Self { key })
    }

    /// `SHA256:…` fingerprint for clients to verify.
    #[must_use]
    pub fn fingerprint(&self) -> String {
        fingerprint(&self.key)
    }
}

fn load_or_create_host_key(path: &Path) -> Result<PrivateKey, String> {
    if let Ok(pem) = std::fs::read_to_string(path) {
        return PrivateKey::from_openssh(pem.as_bytes()).map_err(|e| format!("invalid SFTP host key: {e}"));
    }
    let pair = Ed25519Keypair::from_seed(&forge_secrets::crypto::random_bytes::<32>());
    let key = PrivateKey::new(KeypairData::Ed25519(pair), "orbyn-forge-sftp").map_err(|e| e.to_string())?;
    let pem = key.to_openssh(LineEnding::LF).map_err(|e| e.to_string())?;
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    }
    write_private(path, pem.as_bytes())?;
    Ok(key)
}

fn write_private(path: &Path, data: &[u8]) -> Result<(), String> {
    #[cfg(unix)]
    {
        use std::io::Write;
        use std::os::unix::fs::OpenOptionsExt;
        let mut f = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(path)
            .map_err(|e| e.to_string())?;
        f.write_all(data).map_err(|e| e.to_string())
    }
    #[cfg(not(unix))]
    {
        std::fs::write(path, data).map_err(|e| e.to_string())
    }
}

fn fingerprint(key: &PrivateKey) -> String {
    key.public_key().fingerprint(HashAlg::Sha256).to_string()
}

/// Failed-login counts per client address.
#[derive(Debug, Default)]
struct Throttle(Mutex<HashMap<IpAddr, (u32, Instant)>>);

impl Throttle {
    fn blocked(&self, ip: IpAddr) -> bool {
        let Ok(mut m) = self.0.lock() else { return true };
        m.retain(|_, (_, at)| at.elapsed() < FAILURE_WINDOW);
        m.get(&ip).is_some_and(|(n, _)| *n >= MAX_FAILURES)
    }

    fn fail(&self, ip: IpAddr) {
        if let Ok(mut m) = self.0.lock() {
            let e = m.entry(ip).or_insert((0, Instant::now()));
            e.0 += 1;
            e.1 = Instant::now();
        }
    }
}

/// One logged-in connection.
struct Conn {
    grant: String,
    /// Set once the login completes; used to close the connection.
    handle: Option<Handle>,
    /// Session channels open on it.
    channels: usize,
}

impl Conn {
    /// Slots it uses towards the grant's `max_sessions`: one from login,
    /// one per session channel after that.
    fn slots(&self) -> usize {
        self.channels.max(1)
    }
}

/// Open sessions per grant, for limits and closing. A slot is reserved at
/// login (checked and taken under one lock, so parallel logins cannot both
/// pass), and every further session channel on a connection takes another.
#[derive(Default)]
struct Sessions(Mutex<HashMap<u64, Conn>>);

impl std::fmt::Debug for Sessions {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Sessions")
    }
}

impl Sessions {
    fn used(m: &HashMap<u64, Conn>, grant: &str) -> usize {
        m.values().filter(|c| c.grant == grant).map(Conn::slots).sum()
    }

    /// Reserves a slot for connection `id` at login; `false` at the limit.
    fn reserve(&self, id: u64, grant: &str, max: usize) -> bool {
        let Ok(mut m) = self.0.lock() else { return false };
        if Self::used(&m, grant) >= max {
            return false;
        }
        m.insert(id, Conn { grant: grant.to_owned(), handle: None, channels: 0 });
        true
    }

    fn set_handle(&self, id: u64, handle: Handle) {
        if let Ok(mut m) = self.0.lock()
            && let Some(c) = m.get_mut(&id)
        {
            c.handle = Some(handle);
        }
    }

    /// Takes a slot for a new session channel; the first one uses the slot
    /// reserved at login. `false` at the limit or for an unknown connection.
    fn open_channel(&self, id: u64, max: usize) -> bool {
        let Ok(mut m) = self.0.lock() else { return false };
        let Some(grant) = m.get(&id).map(|c| c.grant.clone()) else { return false };
        let extra = m.get(&id).is_some_and(|c| c.channels >= 1);
        if extra && Self::used(&m, &grant) >= max {
            return false;
        }
        if let Some(c) = m.get_mut(&id) {
            c.channels += 1;
        }
        true
    }

    fn close_channel(&self, id: u64) {
        if let Ok(mut m) = self.0.lock()
            && let Some(c) = m.get_mut(&id)
        {
            c.channels = c.channels.saturating_sub(1);
        }
    }

    /// Removes every connection of `grant` and returns their handles.
    fn take_grant(&self, grant: &str) -> Vec<Handle> {
        let Ok(mut m) = self.0.lock() else { return Vec::new() };
        let ids: Vec<u64> = m.iter().filter(|(_, c)| c.grant == grant).map(|(id, _)| *id).collect();
        ids.into_iter().filter_map(|id| m.remove(&id).and_then(|c| c.handle)).collect()
    }

    fn remove(&self, id: u64) {
        if let Ok(mut m) = self.0.lock() {
            m.remove(&id);
        }
    }
}

#[derive(Debug)]
struct Shared {
    files: Arc<FileService>,
    throttle: Throttle,
    sessions: Sessions,
}

/// Serves SFTP on `listener` until `shutdown` turns true.
pub async fn serve(
    listener: TcpListener,
    host_key: Arc<HostKey>,
    files: Arc<FileService>,
    mut shutdown: watch::Receiver<bool>,
) {
    let config = Arc::new(russh::server::Config {
        keys: vec![host_key.key.clone()],
        methods: MethodSet::from(&[MethodKind::Password][..]),
        auth_rejection_time: Duration::from_secs(1),
        auth_rejection_time_initial: Some(Duration::from_millis(0)),
        max_auth_attempts: 3,
        inactivity_timeout: Some(Duration::from_secs(30 * 60)),
        keepalive_interval: Some(Duration::from_secs(30)),
        ..Default::default()
    });
    let shared = Arc::new(Shared { files, throttle: Throttle::default(), sessions: Sessions::default() });
    let sweeper = tokio::spawn(sweep(shared.clone()));
    let dropper = tokio::spawn(drop_on_change(shared.clone()));
    let mut next_id = 0u64;
    loop {
        let (stream, peer) = tokio::select! {
            r = listener.accept() => match r {
                Ok(c) => c,
                Err(e) => {
                    tracing::warn!(error = %e, "SFTP accept failed");
                    continue;
                }
            },
            _ = shutdown.changed() => break,
        };
        if shared.throttle.blocked(peer.ip()) {
            continue;
        }
        let _ = stream.set_nodelay(true);
        next_id += 1;
        let handler = SshSession {
            id: next_id,
            peer,
            shared: shared.clone(),
            actor: None,
            channels: HashMap::new(),
            counted: std::collections::HashSet::new(),
        };
        let config = config.clone();
        tokio::spawn(async move {
            match russh::server::run_stream(config, stream, handler).await {
                Ok(session) => {
                    let _ = session.await;
                }
                Err(e) => tracing::debug!(error = %e, "SFTP session ended during setup"),
            }
        });
    }
    sweeper.abort();
    dropper.abort();
}

/// Closes a grant's open sessions as soon as the file service says so
/// (revoked, password reset, or narrowed access).
async fn drop_on_change(shared: Arc<Shared>) {
    let mut rx = shared.files.subscribe_grant_drops();
    loop {
        let grant = match rx.recv().await {
            Ok(g) => g,
            Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => {
                // Missed some: check every open session now.
                sweep_once(&shared).await;
                continue;
            }
            Err(tokio::sync::broadcast::error::RecvError::Closed) => return,
        };
        for handle in shared.sessions.take_grant(&grant) {
            let _ = handle
                .disconnect(
                    russh::Disconnect::ByApplication,
                    "this access was changed or revoked; reconnect".into(),
                    String::new(),
                )
                .await;
        }
    }
}

/// Closes sessions whose grant expired, was revoked, or lost its creator's rights.
async fn sweep(shared: Arc<Shared>) {
    let mut tick = tokio::time::interval(SWEEP);
    loop {
        tick.tick().await;
        sweep_once(&shared).await;
    }
}

async fn sweep_once(shared: &Shared) {
    {
        let open: Vec<(u64, String, Handle)> = shared
            .sessions
            .0
            .lock()
            .map(|m| m.iter().filter_map(|(id, c)| Some((*id, c.grant.clone(), c.handle.clone()?))).collect())
            .unwrap_or_default();
        let mut checked: HashMap<String, bool> = HashMap::new();
        for (id, grant, handle) in open {
            let active = match checked.get(&grant) {
                Some(a) => *a,
                None => {
                    let a = shared.files.grant_is_active(&grant).await;
                    checked.insert(grant.clone(), a);
                    a
                }
            };
            if !active {
                let _ = handle
                    .disconnect(
                        russh::Disconnect::ByApplication,
                        "this access has expired or was revoked".into(),
                        String::new(),
                    )
                    .await;
                shared.sessions.remove(id);
            }
        }
    }
}

struct SshSession {
    id: u64,
    peer: SocketAddr,
    shared: Arc<Shared>,
    actor: Option<Actor>,
    channels: HashMap<ChannelId, Channel<Msg>>,
    /// Session channels counted against the grant's limit.
    counted: std::collections::HashSet<ChannelId>,
}

impl SshSession {
    /// Frees a session channel's slot (once, however it closed).
    fn release(&mut self, channel: ChannelId) {
        self.channels.remove(&channel);
        if self.counted.remove(&channel) {
            self.shared.sessions.close_channel(self.id);
        }
    }
}

impl Drop for SshSession {
    fn drop(&mut self) {
        self.shared.sessions.remove(self.id);
        if let Some(actor) = self.actor.take() {
            let files = self.shared.files.clone();
            let peer = self.peer.ip().to_string();
            tokio::spawn(async move {
                files.grant_event(&actor, "file.sftp.disconnect", AuditOutcome::Success, json!({ "peer": peer })).await;
            });
        }
    }
}

impl russh::server::Handler for SshSession {
    type Error = russh::Error;

    async fn auth_password(&mut self, user: &str, password: &str) -> Result<Auth, Self::Error> {
        let ip = self.peer.ip();
        if self.shared.throttle.blocked(ip) {
            return Ok(Auth::reject());
        }
        match self.shared.files.authenticate(user, password, Some(ip)).await {
            Ok(actor) => {
                let (grant_id, max) = match &actor {
                    Actor::Grant { grant, .. } => (grant.id.clone(), grant.max_sessions as usize),
                    Actor::User { .. } => return Ok(Auth::reject()),
                };
                if !self.shared.sessions.reserve(self.id, &grant_id, max) {
                    self.shared
                        .files
                        .grant_event(
                            &actor,
                            "file.sftp.connect",
                            AuditOutcome::Denied,
                            json!({ "peer": ip.to_string(), "reason": "session limit" }),
                        )
                        .await;
                    return Ok(Auth::reject());
                }
                self.shared
                    .files
                    .grant_event(&actor, "file.sftp.connect", AuditOutcome::Success, json!({ "peer": ip.to_string() }))
                    .await;
                self.actor = Some(actor);
                Ok(Auth::Accept)
            }
            Err(_) => {
                self.shared.throttle.fail(ip);
                Ok(Auth::reject())
            }
        }
    }

    async fn auth_succeeded(&mut self, session: &mut Session) -> Result<(), Self::Error> {
        self.shared.sessions.set_handle(self.id, session.handle());
        Ok(())
    }

    async fn channel_open_session(
        &mut self,
        channel: Channel<Msg>,
        reply: russh::server::ChannelOpenHandle,
        _session: &mut Session,
    ) -> Result<(), Self::Error> {
        let Some(Actor::Grant { grant, .. }) = &self.actor else { return Ok(()) };
        // Each session channel is a session: dropping `reply` refuses it.
        if !self.shared.sessions.open_channel(self.id, grant.max_sessions as usize) {
            let actor = self.actor.clone();
            if let Some(actor) = actor {
                let reason = json!({ "peer": self.peer.ip().to_string(), "reason": "session limit" });
                self.shared.files.grant_event(&actor, "file.sftp.connect", AuditOutcome::Denied, reason).await;
            }
            return Ok(());
        }
        self.counted.insert(channel.id());
        self.channels.insert(channel.id(), channel);
        reply.accept().await;
        Ok(())
    }

    async fn channel_close(&mut self, channel: ChannelId, _session: &mut Session) -> Result<(), Self::Error> {
        self.release(channel);
        Ok(())
    }

    async fn subsystem_request(
        &mut self,
        channel: ChannelId,
        name: &str,
        session: &mut Session,
    ) -> Result<(), Self::Error> {
        let (Some(actor), true) = (self.actor.clone(), name == "sftp") else {
            session.channel_failure(channel)?;
            return Ok(());
        };
        let Some(ch) = self.channels.remove(&channel) else {
            session.channel_failure(channel)?;
            return Ok(());
        };
        let Some(handler) = sftp::SftpSession::new(self.shared.files.clone(), actor).await else {
            session.channel_failure(channel)?;
            return Ok(());
        };
        session.channel_success(channel)?;
        russh_sftp::server::run(ch.into_stream(), handler).await;
        Ok(())
    }

    async fn exec_request(
        &mut self,
        channel: ChannelId,
        _data: &[u8],
        session: &mut Session,
    ) -> Result<(), Self::Error> {
        session.channel_failure(channel)?;
        session.close(channel)?;
        // Closed by us: its slot is free now, whenever the client answers.
        self.release(channel);
        Ok(())
    }

    async fn shell_request(&mut self, channel: ChannelId, session: &mut Session) -> Result<(), Self::Error> {
        session.channel_failure(channel)?;
        session.close(channel)?;
        self.release(channel);
        Ok(())
    }

    #[allow(clippy::too_many_arguments)]
    async fn pty_request(
        &mut self,
        channel: ChannelId,
        _term: &str,
        _col_width: u32,
        _row_height: u32,
        _pix_width: u32,
        _pix_height: u32,
        _modes: &[(russh::Pty, u32)],
        session: &mut Session,
    ) -> Result<(), Self::Error> {
        session.channel_failure(channel)?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::Sessions;

    #[test]
    fn session_slots_are_reserved_at_login_and_counted_per_channel() {
        let s = Sessions::default();
        // Limit 2: two logins fit, a third is refused before any channel opens.
        assert!(s.reserve(1, "g", 2));
        assert!(s.reserve(2, "g", 2));
        assert!(!s.reserve(3, "g", 2));
        // Another grant has its own limit.
        assert!(s.reserve(4, "other", 1));
        // The first channel uses the login's slot; a second one on the same
        // connection needs a free slot.
        assert!(s.open_channel(1, 2));
        assert!(!s.open_channel(1, 2));
        s.remove(2);
        assert!(s.open_channel(1, 2));
        assert!(!s.reserve(5, "g", 2));
        // Closing a channel frees its slot.
        s.close_channel(1);
        assert!(s.reserve(5, "g", 2));
        // Unknown connections never get a channel.
        assert!(!s.open_channel(99, 10));
    }

    #[test]
    fn take_grant_removes_only_that_grants_connections() {
        let s = Sessions::default();
        assert!(s.reserve(1, "g", 5));
        assert!(s.reserve(2, "g", 5));
        assert!(s.reserve(3, "other", 5));
        // No handles yet (login not finished): removed, nothing to close.
        assert!(s.take_grant("g").is_empty());
        assert!(s.reserve(4, "g", 2) && s.reserve(5, "g", 2));
        assert!(!s.reserve(6, "other", 1));
    }
}
