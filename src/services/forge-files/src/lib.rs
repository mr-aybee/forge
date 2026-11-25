//! File explorer service: the one place that decides what a caller may do
//! with files on a node. The HTTP API and the SFTP gateway both go through
//! it, so they share root resolution, authorization, the protected-path
//! policy, upload ownership and audit.
//!
//! The node enforces root confinement and the protected-path policy again
//! where the files live; this service never relies on that alone.

use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex};

mod grants;

pub use grants::{GrantChanges, IssuedGrant, NewGrant, grant_diff, ip_allowed, narrows, widens};

use forge_core::files::{
    ApprovedRoot, EDITABLE_BYTES, FileOp, FilePath, FileRepository, FileTransport, FileUpload, MAX_CHUNK_BYTES,
    NODE_ROOT, NodeOs, Overwrite, RootKind, RootSpec, protected_reason, protected_within,
};
use forge_core::model::{AgentStatus, AuditEvent, AuditOutcome, Server};
use forge_core::ports::{AuditRecorder, Authorizer, ServerRepository};
use forge_core::{AccessRequest, Action, AuthzContext, Clock, EntityKind, EntityRef, Principal, ServerId, Timestamp};
use serde::Serialize;
use serde_json::{Value, json};

/// How long an upload session may stay unfinished.
pub const UPLOAD_TTL_MS: i64 = 24 * 60 * 60 * 1000;
/// How long a temporary download archive stays readable (nodes remove
/// them after an hour too).
pub const ARCHIVE_TTL_MS: i64 = 60 * 60 * 1000;

/// Who made a temporary archive, where, and until when it may be read.
#[derive(Debug, Clone)]
struct ArchiveOwner {
    actor: String,
    server: ServerId,
    root: String,
    expires_at: i64,
}
/// Largest upload accepted.
pub const MAX_UPLOAD_BYTES: u64 = 50 * 1024 * 1024 * 1024;

/// Who is acting.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Actor {
    /// A signed-in user or API key.
    User { principal: Principal, ctx: AuthzContext },
    /// Temporary SFTP access, acting with its creator's rights (narrowed by
    /// the grant, re-checked on every operation).
    Grant { grant: Box<forge_core::files::FileGrant>, creator: Principal },
}

impl Actor {
    /// A signed-in user or API key acting in `ctx`.
    #[must_use]
    pub fn user(principal: Principal, ctx: AuthzContext) -> Self {
        Self::User { principal, ctx }
    }

    /// Stable key: owner of upload sessions and searches, audit actor.
    #[must_use]
    pub fn key(&self) -> String {
        match self {
            Self::User { principal, .. } => principal.to_string(),
            Self::Grant { grant, .. } => format!("grant:{}", grant.id),
        }
    }

    fn request(&self, action: Action, server: ServerId) -> AccessRequest {
        match self {
            Self::User { principal, ctx } => {
                AccessRequest::new(principal.clone(), action, EntityRef::new(EntityKind::Server, server), *ctx)
            }
            // The grant was issued with a recent MFA check by its creator,
            // who must still hold the rights it uses.
            Self::Grant { creator, .. } => AccessRequest::new(
                creator.clone(),
                action,
                EntityRef::new(EntityKind::Server, server),
                AuthzContext { mfa: true },
            ),
        }
    }
}

/// Why an operation did not happen.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FileError {
    Invalid(String),
    Denied(String),
    /// Needs a session with recent MFA.
    StepUp(String),
    NotFound(String),
    Conflict(String),
    TooLarge(String),
    Busy(String),
    /// The node did not answer (offline, or an old agent).
    Unavailable(String),
    /// The agent does not allow or know the `files` command.
    Outdated,
    /// The node refused for a filesystem reason (link, not a file, ...).
    Node(String),
    Internal(String),
}

impl std::fmt::Display for FileError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Invalid(m)
            | Self::Denied(m)
            | Self::StepUp(m)
            | Self::NotFound(m)
            | Self::Conflict(m)
            | Self::TooLarge(m)
            | Self::Busy(m)
            | Self::Unavailable(m)
            | Self::Node(m)
            | Self::Internal(m) => f.write_str(m),
            Self::Outdated => f.write_str("this node's agent does not support the file explorer; update the agent"),
        }
    }
}

impl std::error::Error for FileError {}

/// Result of a file operation.
pub type FileResult<T> = Result<T, FileError>;

/// Effective permissions on one root, for the console.
#[derive(Debug, Clone, Serialize)]
pub struct Permissions {
    pub read: bool,
    pub write: bool,
    pub delete: bool,
    pub admin: bool,
}

/// A root the caller may open.
#[derive(Debug, Clone, Serialize)]
pub struct RootView {
    pub id: String,
    pub kind: RootKind,
    pub label: String,
    /// Where the root starts, as a public path (`/` for the node root).
    pub base: String,
    pub read_only: bool,
    pub permissions: Permissions,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub container: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub service_id: Option<String>,
}

/// A resolved root.
#[derive(Debug, Clone)]
struct Resolved {
    id: String,
    spec: RootSpec,
    read_only: bool,
}

/// Whether to record an audit event for a call.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Audit {
    Record,
    /// The caller records one event for a series (download chunks).
    Skip,
}

/// The audit action name of an operation.
fn audit_name(op: &FileOp) -> &'static str {
    match op {
        FileOp::List { .. } => "file.list",
        FileOp::Stat { .. } => "file.stat",
        FileOp::Read { .. } => "file.read",
        FileOp::Hash { .. } => "file.hash",
        FileOp::Mkdir { .. } => "file.mkdir",
        FileOp::UploadChunk { .. } | FileOp::UploadStatus { .. } => "file.upload.chunk",
        FileOp::UploadCommit { .. } => "file.upload",
        FileOp::UploadAbort { .. } => "file.upload.abort",
        FileOp::Rename { .. } => "file.move",
        FileOp::Copy { .. } => "file.copy",
        FileOp::Delete { permanent: true, .. } => "file.delete.permanent",
        FileOp::Delete { .. } => "file.delete",
        FileOp::TrashList { .. } => "file.trash.list",
        FileOp::TrashRestore { .. } => "file.trash.restore",
        FileOp::TrashPurge { .. } => "file.trash.purge",
        FileOp::Revisions { .. } | FileOp::RevisionRead { .. } => "file.revision.read",
        FileOp::Search { .. } => "file.search",
        FileOp::SetPermissions { .. } => "file.chmod",
        FileOp::SetOwner { .. } => "file.chown",
        FileOp::Acl { .. } => "file.acl",
        FileOp::Compress { dest: None, .. } => "file.archive",
        FileOp::Compress { .. } => "file.compress",
        FileOp::Extract { .. } => "file.extract",
        FileOp::TempRead { .. } => "file.archive.read",
        FileOp::TempDelete { .. } => "file.archive.delete",
    }
}

/// Cedar actions an operation needs on the server (besides `file.node`
/// for the whole-node root). Copies need read and write; moves need write
/// and delete, as the source disappears.
#[must_use]
pub fn actions_for(op: &FileOp) -> Vec<Action> {
    match op {
        FileOp::List { .. }
        | FileOp::Stat { .. }
        | FileOp::Read { .. }
        | FileOp::Hash { .. }
        | FileOp::Search { .. }
        | FileOp::TrashList { .. }
        | FileOp::Revisions { .. }
        | FileOp::RevisionRead { .. } => vec![Action::FileRead],
        FileOp::Mkdir { .. }
        | FileOp::UploadChunk { .. }
        | FileOp::UploadStatus { .. }
        | FileOp::UploadCommit { .. }
        | FileOp::UploadAbort { .. }
        | FileOp::TrashRestore { .. } => vec![Action::FileRead, Action::FileWrite],
        FileOp::Copy { .. } => vec![Action::FileRead, Action::FileWrite],
        FileOp::Rename { .. } => vec![Action::FileRead, Action::FileWrite, Action::FileDelete],
        FileOp::Delete { .. } | FileOp::TrashPurge { .. } => vec![Action::FileRead, Action::FileDelete],
        // Ownership and permission bits are administration (MFA by guardrail).
        FileOp::SetPermissions { .. } | FileOp::SetOwner { .. } => vec![Action::FileRead, Action::FileAdmin],
        FileOp::Acl { .. } | FileOp::TempRead { .. } | FileOp::TempDelete { .. } => vec![Action::FileRead],
        FileOp::Compress { dest: None, .. } => vec![Action::FileRead],
        // Replacing a folder deletes its old content.
        FileOp::Extract { replace: true, .. } => vec![Action::FileRead, Action::FileWrite, Action::FileDelete],
        FileOp::Compress { .. } | FileOp::Extract { .. } => vec![Action::FileRead, Action::FileWrite],
    }
}

fn b64(data: &[u8]) -> String {
    use base64::Engine as _;
    base64::engine::general_purpose::STANDARD.encode(data)
}
/// A fresh id for an approved root (`fr_` and 24 hex digits).
#[must_use]
pub fn new_root_id() -> String {
    format!("fr_{}", random_hex(12))
}

pub(crate) fn random_hex(bytes: usize) -> String {
    use ring::rand::SecureRandom as _;
    let mut b = vec![0u8; bytes];
    let _ = ring::rand::SystemRandom::new().fill(&mut b);

    b.iter().map(|x| format!("{x:02x}")).collect()
}

/// Maps a node's refusal to an error.
#[must_use]
pub fn node_error(err: &str) -> FileError {
    if let Some(t) = err.strip_prefix("transport:") {
        return FileError::Unavailable(t.trim().to_owned());
    }

    // An older agent refuses fields it does not know (`parents`, `replace`).
    if err.starts_with("invalid file operation") && err.contains("unknown field") {
        return FileError::Invalid("this node's agent does not support this option yet; update the agent".into());
    }

    if err.starts_with("protected path") || err.contains("permission denied") {
        FileError::Denied(err.to_owned())
    } else if err.contains("was not found") || err.contains("does not exist") {
        FileError::NotFound(err.to_owned())
    } else if err.starts_with("source changed")
        || err.contains("already exists")
        || err.contains("in progress")
        || err.contains("created by someone else")
        || err.contains("now exists")
    {
        FileError::Conflict(err.to_owned())
    } else if err.contains("not allowed by this agent") || err.contains("not supported by this agent") {
        FileError::Outdated
    } else if err.contains("too large") || err.contains("disk is full") || err.contains("not enough free space") {
        FileError::TooLarge(err.to_owned())
    } else {
        FileError::Node(err.to_owned())
    }
}

/// File browsing and transfer on nodes, authorized per operation.
#[derive(Debug)]
pub struct FileService {
    repo: Arc<dyn FileRepository>,
    servers: Arc<dyn ServerRepository>,
    authz: Arc<dyn Authorizer>,
    audit: Arc<dyn AuditRecorder>,
    transport: Arc<dyn FileTransport>,
    clock: Arc<dyn Clock>,
    /// Temporary SFTP access grants (`None`: SFTP access disabled).
    grants: Option<Arc<dyn forge_core::files::FileGrantRepository>>,
    /// Longest grant lifetime, in minutes.
    max_grant_minutes: i64,
    /// Whether grants without an expiry may be issued (`[sftp] allow_never_expiry`).
    allow_never_expiry: bool,
    /// Grant ids whose open SFTP sessions must close now (revoked, password
    /// reset or narrowed). The gateway subscribes.
    grant_drops: tokio::sync::broadcast::Sender<String>,
    /// Running searches, one per caller per node.
    searches: Mutex<HashSet<(String, ServerId)>>,
    /// Temporary archives by id: only their creator, through the same root
    /// of the same server, may read or delete them. Kept in memory: an
    /// archive lives an hour, and one made before a restart is made again.
    archives: Mutex<HashMap<String, ArchiveOwner>>,
    /// Puts concurrent chunk requests of one upload in offset order.
    gates: Mutex<HashMap<String, Arc<UploadGate>>>,
    /// Bytes of chunks waiting for earlier ones, across all uploads.
    waiting_bytes: std::sync::atomic::AtomicUsize,
}

/// Upload requests a client may have in flight at once (issue #15): the
/// server writes them to the node in offset order, so the client's sending
/// overlaps with the node's writing. Advertised as `parallel`.
pub const UPLOAD_PARALLEL: usize = 3;

/// How long a chunk waits for the chunks before it.
const GATE_WAIT: std::time::Duration = std::time::Duration::from_secs(120);

/// Chunk bytes that may wait for earlier chunks across all uploads at once;
/// each waiting chunk keeps its body in memory.
const MAX_WAITING_BYTES: usize = 64 * 1024 * 1024;

/// The next offset the node expects for one upload, and who waits for it.
#[derive(Debug, Default)]
struct UploadGate {
    next: tokio::sync::Mutex<Option<u64>>,
    moved: tokio::sync::Notify,
    /// Chunks of this upload waiting for earlier ones.
    waiting: std::sync::atomic::AtomicUsize,
}

/// One chunk's place in the waiting counts; released when dropped.
struct Waiting<'a> {
    gate: &'a UploadGate,
    total: &'a std::sync::atomic::AtomicUsize,
    bytes: usize,
}

impl Drop for Waiting<'_> {
    fn drop(&mut self) {
        use std::sync::atomic::Ordering;
        self.gate.waiting.fetch_sub(1, Ordering::SeqCst);
        self.total.fetch_sub(self.bytes, Ordering::SeqCst);
    }
}

/// Releases a search slot when dropped.
struct SearchSlot<'a> {
    set: &'a Mutex<HashSet<(String, ServerId)>>,
    key: (String, ServerId),
}

impl Drop for SearchSlot<'_> {
    fn drop(&mut self) {
        if let Ok(mut s) = self.set.lock() {
            s.remove(&self.key);
        }
    }
}

impl FileService {
    /// Creates the file service.
    #[must_use]
    pub fn new(
        repo: Arc<dyn FileRepository>,
        servers: Arc<dyn ServerRepository>,
        authz: Arc<dyn Authorizer>,
        audit: Arc<dyn AuditRecorder>,
        transport: Arc<dyn FileTransport>,
        clock: Arc<dyn Clock>,
    ) -> Self {
        Self {
            repo,
            servers,
            authz,
            audit,
            transport,
            clock,
            grants: None,
            max_grant_minutes: 7 * 24 * 60,
            allow_never_expiry: false,
            grant_drops: tokio::sync::broadcast::channel(64).0,
            searches: Mutex::new(HashSet::new()),
            archives: Mutex::new(HashMap::new()),
            gates: Mutex::new(HashMap::new()),
            waiting_bytes: std::sync::atomic::AtomicUsize::new(0),
        }
    }

    /// Whether `actor` made archive `id` on this server and root, and it has
    /// not expired.
    fn owns_archive(&self, actor: &Actor, server: ServerId, root: &str, id: &str) -> bool {
        let now = self.clock.now().0;
        self.archives.lock().is_ok_and(|m| {
            m.get(id)
                .is_some_and(|o| o.actor == actor.key() && o.server == server && o.root == root && now < o.expires_at)
        })
    }

    fn remember_archive(&self, actor: &Actor, server: ServerId, root: &str, id: &str) {
        let now = self.clock.now().0;

        if let Ok(mut m) = self.archives.lock() {
            m.retain(|_, o| now < o.expires_at);
            m.insert(
                id.to_owned(),
                ArchiveOwner { actor: actor.key(), server, root: root.to_owned(), expires_at: now + ARCHIVE_TTL_MS },
            );
        }
    }

    /// Enables temporary SFTP access grants (lifetime capped at `max_hours`).
    #[must_use]
    pub fn with_grants(mut self, repo: Arc<dyn forge_core::files::FileGrantRepository>, max_hours: u32) -> Self {
        self.grants = Some(repo);
        self.max_grant_minutes = i64::from(max_hours.max(1)) * 60;

        self
    }

    /// Allows grants that never expire (`[sftp] allow_never_expiry`).
    #[must_use]
    pub fn with_never_expiry(mut self, allow: bool) -> Self {
        self.allow_never_expiry = allow;
        self
    }

    /// Longest grant lifetime, in minutes.
    #[must_use]
    pub fn max_grant_minutes(&self) -> i64 {
        self.max_grant_minutes
    }

    /// Whether grants without an expiry may be issued at all.
    #[must_use]
    pub fn allow_never_expiry(&self) -> bool {
        self.allow_never_expiry
    }

    /// Grant ids whose open sessions must close at once.
    #[must_use]
    pub fn subscribe_grant_drops(&self) -> tokio::sync::broadcast::Receiver<String> {
        self.grant_drops.subscribe()
    }

    async fn record(&self, actor: &Actor, action: &str, server: &Server, outcome: AuditOutcome, details: Value) {
        let ev = AuditEvent::new(actor.key(), action, EntityRef::new(EntityKind::Server, server.id), outcome)
            .in_org(Some(server.workspace_id))
            .with_details(details);

        if let Err(e) = self.audit.record(ev).await {
            tracing::error!(error = %e, "audit write failed");
        }
    }

    async fn denied(&self, actor: &Actor, action: &str, server: &Server, reason: &str) {
        self.record(actor, action, server, AuditOutcome::Denied, json!({ "reason": reason })).await;
    }

    async fn server(&self, id: ServerId) -> FileResult<Server> {
        let s = self.servers.get_server(id).await.map_err(|_| FileError::NotFound("server not found".into()))?;
        if s.status == AgentStatus::Revoked {
            return Err(FileError::Conflict("the server is revoked".into()));
        }

        Ok(s)
    }

    async fn can(&self, actor: &Actor, action: Action, server: ServerId) -> bool {
        self.authz.is_allowed(&actor.request(action, server)).await
    }

    /// Authorizes `action`; a refusal is audited.
    async fn require(&self, actor: &Actor, action: Action, server: &Server) -> FileResult<()> {
        match self.authz.authorize(&actor.request(action, server.id)).await {
            Ok(()) => Ok(()),
            Err(e) => {
                let msg = e.to_string();
                self.denied(actor, action.as_str(), server, &msg).await;

                if msg.to_lowercase().contains("mfa") || msg.to_lowercase().contains("step-up") {
                    Err(FileError::StepUp(format!("`{action}` needs a recent MFA check")))
                } else {
                    Err(FileError::Denied(format!("not allowed: `{action}` on this server")))
                }
            }
        }
    }

    /// The roots the caller may open on a server, with permissions.
    pub async fn roots(&self, actor: &Actor, server_id: ServerId) -> FileResult<(NodeOs, Vec<RootView>)> {
        let server = self.server(server_id).await?;
        self.require(actor, Action::FileRead, &server).await?;
        let perms = Permissions {
            read: true,
            write: self.can(actor, Action::FileWrite, server_id).await,
            delete: self.can(actor, Action::FileDelete, server_id).await,
            admin: self.can(actor, Action::FileAdmin, server_id).await,
        };
        let os = NodeOs::from_reported(&server.os);
        let mut out = Vec::new();

        if self.can(actor, Action::FileNode, server_id).await {
            out.push(RootView {
                id: NODE_ROOT.into(),
                kind: RootKind::Node,
                label: if os == NodeOs::Windows { "Drives".into() } else { "Node filesystem".into() },
                base: "/".into(),
                read_only: false,
                permissions: perms.clone(),
                container: None,
                service_id: None,
            });
        }

        let approved = self.repo.list_roots(server_id).await.map_err(|e| FileError::Internal(e.to_string()))?;

        // Older servers allowed the same folder to be approved twice (F-06).
        // Exact duplicates are shown once (the oldest); the rows stay, so a
        // root id already in use keeps working until an administrator
        // removes it.
        let approved = dedupe_roots(os, approved);

        for r in approved {
            let (base, container) = match &r.spec {
                RootSpec::Node => ("/".to_owned(), None),
                RootSpec::Dir { base } => (base.to_string(), None),
                RootSpec::Container { container, base } => (base.to_string(), Some(container.clone())),
                RootSpec::App { .. } => (r.spec.base().to_string(), None),
            };

            let p = if r.read_only {
                Permissions { read: true, write: false, delete: false, admin: perms.admin }
            } else {
                perms.clone()
            };

            out.push(RootView {
                id: r.id,
                kind: r.kind,
                label: r.label,
                base,
                read_only: r.read_only,
                permissions: p,
                container,
                service_id: r.service_id,
            });
        }

        Ok((os, out))
    }

    async fn resolve(&self, actor: &Actor, server: &Server, root_id: &str) -> FileResult<Resolved> {
        if root_id == NODE_ROOT {
            self.require(actor, Action::FileNode, server).await?;
            return Ok(Resolved { id: NODE_ROOT.into(), spec: RootSpec::Node, read_only: false });
        }

        let root = self
            .repo
            .get_root(root_id)
            .await
            .map_err(|e| FileError::Internal(e.to_string()))?
            .filter(|r| r.server_id == server.id)
            .ok_or_else(|| FileError::NotFound(format!("unknown root `{root_id}`")))?;

        // Roots approved before D-083 on an app's folder run as the app now;
        // one around or inside app folders is refused until it is approved again.
        let spec = if Self::os_for(server, &root.spec) == NodeOs::Linux {
            root.spec.for_linux_apps().map_err(FileError::Invalid)?
        } else {
            root.spec
        };

        Ok(Resolved { id: root.id, spec, read_only: root.read_only })
    }

    fn os_for(server: &Server, spec: &RootSpec) -> NodeOs {
        if matches!(spec, RootSpec::Container { .. }) { NodeOs::Linux } else { NodeOs::from_reported(&server.os) }
    }

    /// Authorizes, resolves and checks an operation; returns it bound to its root.
    async fn prepare(
        &self,
        actor: &Actor,
        server: &Server,
        root_id: &str,
        op: FileOp,
    ) -> FileResult<(FileOp, Resolved)> {
        let name = audit_name(&op);
        let actions = actions_for(&op);
        if let Err(e) = self.check_grant(actor, root_id, &actions).await {
            self.denied(actor, name, server, &e.to_string()).await;
            return Err(e);
        }

        for a in actions {
            self.require(actor, a, server).await?;
        }

        let root = self.resolve(actor, server, root_id).await?;
        if root.read_only && op.mutates() {
            self.denied(actor, name, server, "the root is read-only").await;
            return Err(FileError::Denied("this root is read-only".into()));
        }

        let op = op.with_root(root.spec.clone());
        op.validate().map_err(FileError::Invalid)?;
        let os = Self::os_for(server, &root.spec);

        for p in op.paths() {
            if let Some(why) = protected_reason(os, &root.spec.full_path(p)) {
                let reason = format!("protected path ({why})");
                self.denied(actor, name, server, &reason).await;

                return Err(FileError::Denied(reason));
            }
        }

        // A moved or removed folder takes everything below it along.
        for p in op.subtrees() {
            if let Some(why) = protected_within(os, &root.spec.full_path(p)) {
                let reason = format!("protected path (it contains protected files: {why})");
                self.denied(actor, name, server, &reason).await;

                return Err(FileError::Denied(reason));
            }
        }

        if let FileOp::TempRead { id, .. } | FileOp::TempDelete { id, .. } = &op
            && !self.owns_archive(actor, server.id, &root.id, id)
        {
            self.denied(actor, name, server, "not the archive's creator or root").await;
            return Err(FileError::NotFound("that archive is gone".into()));
        }

        Ok((op, root))
    }

    async fn call(&self, actor: &Actor, server: &Server, op: &FileOp) -> FileResult<Value> {
        let r = self.transport.call(server.id, &actor.key(), op).await;
        r.map_err(|e| node_error(&e))
    }

    fn details(root: &Resolved, op: &FileOp, out: Option<&Value>) -> Value {
        let mut d = json!({ "root": root.id });

        // An app root (D-083): which app, from our side; the account the
        // node ran it as, as the node reports it.
        if let RootSpec::App { environment, app } = &root.spec {
            d["app"] = json!(format!("{environment}/{app}"));
            if let Some(a) = out.and_then(|o| o.get("as_reported")).and_then(Value::as_str) {
                d["as_reported"] = json!(a.chars().take(64).collect::<String>());
            }
            // Landlock on the node (full, partial, none, error), as reported.
            if let Some(c) = out.and_then(|o| o.get("confined_reported")).and_then(Value::as_str) {
                d["confined_reported"] = json!(c.chars().take(16).collect::<String>());
            }
        }

        match op {
            FileOp::Rename { from, to, .. } | FileOp::Copy { from, to, .. } => {
                d["from"] = json!(from.to_string());
                d["to"] = json!(to.to_string());
            }
            FileOp::TrashRestore { id, .. } => d["trash_id"] = json!(id),
            FileOp::TrashPurge { id, .. } => d["trash_id"] = json!(id),
            FileOp::SetPermissions { path, mode, recursive, dry_run, .. } => {
                d["path"] = json!(path.to_string());
                d["mode"] = json!(format!("{mode:04o}"));
                d["recursive"] = json!(recursive);
                d["dry_run"] = json!(dry_run);
            }
            FileOp::SetOwner { path, owner, group, recursive, dry_run, .. } => {
                d["path"] = json!(path.to_string());
                d["owner"] = json!(owner);
                d["group"] = json!(group);
                d["recursive"] = json!(recursive);
                d["dry_run"] = json!(dry_run);
            }
            FileOp::Compress { sources, dest, format, .. } => {
                d["sources"] = json!(sources.iter().take(50).map(ToString::to_string).collect::<Vec<_>>());
                d["dest"] = json!(dest.as_ref().map(ToString::to_string));
                d["format"] = json!(format);
            }
            FileOp::Extract { archive, dest, replace, .. } => {
                d["archive"] = json!(archive.to_string());
                d["dest"] = json!(dest.to_string());
                d["replace"] = json!(replace);
            }
            FileOp::Search { path, spec, .. } => {
                // Patterns may hold sensitive text: record their shape only.
                d["path"] = json!(path.to_string());
                d["name_pattern"] = json!(!spec.name.is_empty());
                d["content_pattern"] = json!(!spec.content.is_empty());
            }
            _ => d["path"] = json!(op.path().to_string()),
        }

        if let Some(out) = out {
            // Permission changes: counts and the before/after sample.
            for k in ["changed", "unchanged", "skipped", "sample"] {
                if !out[k].is_null() && op.mutates() {
                    d[k] = out[k].clone();
                }
            }

            for k in ["length", "size", "entries", "bytes", "matches", "trash_id", "status", "purged", "offset"] {
                let v = &out[k];
                if !v.is_null() {
                    d[k] = if k == "entries" { json!(v.as_array().map_or(0, Vec::len)) } else { v.clone() };
                }
            }
        }

        d
    }

    /// Records one event for a series of calls the caller ran with
    /// [`Audit::Skip`] (a download made of many reads).
    pub async fn audit(&self, actor: &Actor, server_id: ServerId, action: &str, details: Value) {
        if let Ok(server) = self.server(server_id).await {
            self.record(actor, action, &server, AuditOutcome::Success, details).await;
        }
    }

    /// Runs one operation for `actor` on `root_id` of a server.
    pub async fn run(
        &self,
        actor: &Actor,
        server_id: ServerId,
        root_id: &str,
        op: FileOp,
        audit: Audit,
    ) -> FileResult<Value> {
        let server = self.server(server_id).await?;
        let name = audit_name(&op);
        let (op, root) = self.prepare(actor, &server, root_id, op).await?;
        let _slot = if matches!(op, FileOp::Search { .. }) {
            let key = (actor.key(), server.id);
            let mut set = self.searches.lock().map_err(|_| FileError::Internal("lock poisoned".into()))?;
            if !set.insert(key.clone()) {
                return Err(FileError::Busy("a search of yours is already running on this node".into()));
            }

            Some(SearchSlot { set: &self.searches, key })
        } else {
            None
        };

        let result = self.call(actor, &server, &op).await;
        match (&op, &result) {
            (FileOp::Compress { dest: None, .. }, Ok(v)) => {
                if let Some(id) = v["temp_id"].as_str() {
                    self.remember_archive(actor, server.id, &root.id, id);
                }
            }
            (FileOp::TempDelete { id, .. }, Ok(_)) => {
                if let Ok(mut m) = self.archives.lock() {
                    m.remove(id);
                }
            }
            _ => {}
        }

        if audit == Audit::Record {
            match &result {
                Ok(v) => {
                    self.record(actor, name, &server, AuditOutcome::Success, Self::details(&root, &op, Some(v))).await
                }
                Err(FileError::Denied(m)) => self.denied(actor, name, &server, m).await,
                Err(e) if op.mutates() => {
                    let mut d = Self::details(&root, &op, None);
                    d["error"] = json!(e.to_string());
                    self.record(actor, name, &server, AuditOutcome::Failure, d).await;
                }
                Err(_) => {}
            }
        }

        result
    }

    /// Starts a resumable upload to `path` in a root.
    pub async fn begin_upload(
        &self,
        actor: &Actor,
        server_id: ServerId,
        root_id: &str,
        path: FilePath,
        size: u64,
        overwrite: Overwrite,
    ) -> FileResult<FileUpload> {
        if size > MAX_UPLOAD_BYTES {
            return Err(FileError::TooLarge("uploads are limited to 50 GiB".into()));
        }

        let server = self.server(server_id).await?;
        let id = random_hex(16);
        let probe = FileOp::UploadStatus { root: RootSpec::Node, path: path.clone(), upload: id.clone() };
        let (_, root) = self.prepare(actor, &server, root_id, probe).await?;

        // Refuse before any byte is sent when the target exists and must not
        // be replaced (F-08). The node checks again at commit.
        if overwrite == Overwrite::Never {
            let stat = FileOp::Stat { root: root.spec.clone(), path: path.clone() };
            if self.call(actor, &server, &stat).await.is_ok() {
                return Err(FileError::Conflict(format!(
                    "`{path}` already exists; upload with overwrite to replace it"
                )));
            }
        }

        let now = self.clock.now();
        let up = FileUpload {
            id,
            workspace_id: server.workspace_id,
            server_id,
            root_id: root.id,
            path,
            size,
            received: 0,
            overwrite,
            created_by: actor.key(),
            created_at: now,
            expires_at: Timestamp(now.0 + UPLOAD_TTL_MS),
        };
        self.repo.put_upload(&up).await.map_err(|e| FileError::Internal(e.to_string()))?;

        Ok(up)
    }

    /// The caller's own, unexpired upload.
    async fn upload(&self, actor: &Actor, id: &str) -> FileResult<FileUpload> {
        let up = self
            .repo
            .get_upload(id)
            .await
            .map_err(|e| FileError::Internal(e.to_string()))?
            .filter(|u| u.created_by == actor.key())
            .ok_or_else(|| FileError::NotFound("upload not found".into()))?;

        if up.expires_at.0 < self.clock.now().0 {
            return Err(FileError::NotFound("the upload has expired; start it again".into()));
        }

        Ok(up)
    }

    fn gate(&self, id: &str) -> Option<Arc<UploadGate>> {
        let mut gates = self.gates.lock().ok()?;
        if gates.len() > 4096 {
            gates.retain(|_, g| Arc::strong_count(g) > 1);
        }

        Some(gates.entry(id.to_owned()).or_default().clone())
    }

    fn forget_gate(&self, id: &str) {
        if let Ok(mut gates) = self.gates.lock() {
            gates.remove(id);
        }
    }

    /// Writes one chunk. Permissions are checked again on every chunk.
    /// Chunks of one upload may arrive together (up to [`UPLOAD_PARALLEL`]):
    /// each waits until the node holds every byte before it, then goes out at
    /// once, so they reach the node in order.
    pub async fn upload_chunk(&self, actor: &Actor, id: &str, offset: u64, data: &[u8]) -> FileResult<u64> {
        // Before any waiting: only the upload's owner, only within its size,
        // so strangers and bad offsets fail at once and never hold a slot.
        let up = self.upload(actor, id).await?;

        if data.len() > forge_core::files::API_TRANSFER_BYTES {
            return Err(FileError::TooLarge(format!(
                "chunks are limited to {} bytes",
                forge_core::files::API_TRANSFER_BYTES
            )));
        }

        if offset + data.len() as u64 > up.size {
            return Err(FileError::Invalid("the chunk goes past the announced size".into()));
        }

        let Some(gate) = self.gate(&up.id) else { return self.upload_chunk_now(actor, id, offset, data).await };
        let deadline = tokio::time::Instant::now() + GATE_WAIT;
        let mut place: Option<Waiting<'_>> = None;

        loop {
            let mut next = gate.next.lock().await;
            let expected = match *next {
                Some(n) => n,
                None => self.upload(actor, id).await?.received,
            };

            // In order, or a retry of bytes the node already has (it skips those).
            if offset <= expected {
                drop(place);
                let r = self.upload_chunk_now(actor, id, offset, data).await;
                *next = r.as_ref().ok().copied();
                gate.moved.notify_waiters();

                return r;
            }

            if place.is_none() {
                place = Some(self.wait_slot(&gate, data.len())?);
            }

            let moved = gate.moved.notified();
            tokio::pin!(moved);
            moved.as_mut().enable();
            drop(next);

            if tokio::time::timeout_at(deadline, moved).await.is_err() {
                return Err(FileError::Conflict(format!(
                    "the upload is at byte {expected}, not {offset}; send the earlier chunks first"
                )));
            }
        }
    }

    /// A place among the chunks waiting for earlier ones: at most
    /// [`UPLOAD_PARALLEL`] per upload and [`MAX_WAITING_BYTES`] in all;
    /// beyond that the chunk is refused at once.
    fn wait_slot<'a>(&'a self, gate: &'a UploadGate, bytes: usize) -> FileResult<Waiting<'a>> {
        use std::sync::atomic::Ordering;

        if gate.waiting.fetch_add(1, Ordering::SeqCst) >= UPLOAD_PARALLEL {
            gate.waiting.fetch_sub(1, Ordering::SeqCst);
            return Err(FileError::Busy(format!(
                "at most {UPLOAD_PARALLEL} chunks of an upload may wait for earlier ones; send fewer at once"
            )));
        }

        if self.waiting_bytes.fetch_add(bytes, Ordering::SeqCst) + bytes > MAX_WAITING_BYTES {
            self.waiting_bytes.fetch_sub(bytes, Ordering::SeqCst);
            gate.waiting.fetch_sub(1, Ordering::SeqCst);

            return Err(FileError::Busy("too many upload chunks are waiting on this server; try again".into()));
        }

        Ok(Waiting { gate, total: &self.waiting_bytes, bytes })
    }

    async fn upload_chunk_now(&self, actor: &Actor, id: &str, offset: u64, data: &[u8]) -> FileResult<u64> {
        // Up to `API_TRANSFER_BYTES` from a client: node-sized chunks in one round trip.
        if data.len() > MAX_CHUNK_BYTES && data.len() <= forge_core::files::API_TRANSFER_BYTES {
            let chunks: Vec<&[u8]> = data.chunks(MAX_CHUNK_BYTES).collect();
            return self.upload_chunks(actor, id, offset, &chunks).await;
        }

        if data.len() > MAX_CHUNK_BYTES {
            return Err(FileError::TooLarge(format!(
                "chunks are limited to {} bytes",
                forge_core::files::API_TRANSFER_BYTES
            )));
        }

        let mut up = self.upload(actor, id).await?;
        if offset + data.len() as u64 > up.size {
            return Err(FileError::Invalid("the chunk goes past the announced size".into()));
        }

        let op = FileOp::UploadChunk {
            root: RootSpec::Node,
            path: up.path.clone(),
            upload: up.id.clone(),
            offset,
            data_b64: b64(data),
        };
        let server = self.server(up.server_id).await?;
        let (op, _) = self.prepare(actor, &server, &up.root_id, op).await?;
        let out = self.call(actor, &server, &op).await?;
        up.received = out["received"].as_u64().unwrap_or(offset + data.len() as u64);
        self.repo.put_upload(&up).await.map_err(|e| FileError::Internal(e.to_string()))?;

        Ok(up.received)
    }

    /// Several consecutive chunks of an upload in one round trip to the node
    /// (issue #15): queued in order and written in order there. Returns the
    /// bytes the node holds afterwards.
    pub async fn upload_chunks(&self, actor: &Actor, id: &str, offset: u64, chunks: &[&[u8]]) -> FileResult<u64> {
        if chunks.iter().any(|c| c.len() > MAX_CHUNK_BYTES) {
            return Err(FileError::TooLarge(format!("chunks are limited to {MAX_CHUNK_BYTES} bytes")));
        }

        let mut up = self.upload(actor, id).await?;
        let total: u64 = chunks.iter().map(|c| c.len() as u64).sum();
        if offset + total > up.size {
            return Err(FileError::Invalid("the chunk goes past the announced size".into()));
        }

        let server = self.server(up.server_id).await?;
        let mut ops = Vec::with_capacity(chunks.len());
        let mut at = offset;

        for c in chunks {
            let op = FileOp::UploadChunk {
                root: RootSpec::Node,
                path: up.path.clone(),
                upload: up.id.clone(),
                offset: at,
                data_b64: b64(c),
            };
            ops.push(self.prepare(actor, &server, &up.root_id, op).await?.0);
            at += c.len() as u64;
        }

        let results = self.transport.call_many(server.id, &actor.key(), &ops).await;
        let mut received = offset;

        for r in results {
            let out = r.map_err(|e| node_error(&e))?;
            received = out["received"].as_u64().unwrap_or(received);
        }

        up.received = received;
        self.repo.put_upload(&up).await.map_err(|e| FileError::Internal(e.to_string()))?;

        Ok(up.received)
    }

    /// A read of up to `API_TRANSFER_BYTES` (issue #15): one node read, or
    /// the first through [`Self::run`] (audited as before, its version pins
    /// the rest) and the others together in one round trip. The answer has
    /// the shape of a single read.
    #[allow(clippy::too_many_arguments)]
    pub async fn read_range(
        &self,
        actor: &Actor,
        server_id: ServerId,
        root_id: &str,
        path: FilePath,
        offset: u64,
        length: u32,
        version: Option<String>,
        audit: Audit,
    ) -> FileResult<Value> {
        let piece = forge_core::files::MAX_READ_BYTES;
        let max = u32::try_from(forge_core::files::API_TRANSFER_BYTES).unwrap_or(piece);
        if length == 0 || length > max {
            return Err(FileError::Invalid(format!("length must be between 1 and {max}")));
        }

        let read = |offset: u64, length: u32, version: Option<String>| FileOp::Read {
            root: RootSpec::Node,
            path: path.clone(),
            offset,
            length,
            version,
        };
        let mut first = self.run(actor, server_id, root_id, read(offset, length.min(piece), version), audit).await?;
        if length <= piece || first["eof"] == true {
            return Ok(first);
        }

        let mut data = decode_b64(&first)?;
        if data.len() < piece as usize {
            return Ok(first);
        }

        let version = first["version"].as_str().map(str::to_owned);
        let mut ops = Vec::new();
        let mut at = offset + u64::from(piece);
        let end = offset + u64::from(length);

        while at < end {
            let n = u32::try_from((end - at).min(u64::from(piece))).unwrap_or(piece);
            ops.push(read(at, n, version.clone()));
            at += u64::from(n);
        }

        let mut eof = false;

        for r in self.run_many(actor, server_id, root_id, ops).await {
            let v = r?;
            let more = decode_b64(&v)?;
            let short = more.len() < piece as usize;
            data.extend_from_slice(&more);
            eof = v["eof"] == true;

            if short || eof {
                break;
            }
        }

        first["content_b64"] = json!(b64(&data));
        first["length"] = json!(data.len());
        first["eof"] = json!(eof || first["size"].as_u64().is_some_and(|s| offset + data.len() as u64 >= s));

        Ok(first)
    }

    /// Several operations on one root in one round trip, run in order; each
    /// is authorized like [`Self::run`]. For transfers (reads ahead); not
    /// audited per call.
    pub async fn run_many(
        &self,
        actor: &Actor,
        server_id: ServerId,
        root_id: &str,
        ops: Vec<FileOp>,
    ) -> Vec<FileResult<Value>> {
        let server = match self.server(server_id).await {
            Ok(s) => s,
            Err(e) => return vec![Err(e)],
        };

        let mut prepared = Vec::with_capacity(ops.len());

        for op in ops {
            match self.prepare(actor, &server, root_id, op).await {
                Ok((op, _)) => prepared.push(op),
                Err(e) => return vec![Err(e)],
            }
        }

        self.transport
            .call_many(server.id, &actor.key(), &prepared)
            .await
            .into_iter()
            .map(|r| r.map_err(|e| node_error(&e)))
            .collect()
    }

    /// The upload session and the bytes the node holds (for resuming).
    pub async fn upload_status(&self, actor: &Actor, id: &str) -> FileResult<(FileUpload, u64)> {
        let up = self.upload(actor, id).await?;
        let server = self.server(up.server_id).await?;
        let op = FileOp::UploadStatus { root: RootSpec::Node, path: up.path.clone(), upload: up.id.clone() };
        let (op, _) = self.prepare(actor, &server, &up.root_id, op).await?;
        let out = self.call(actor, &server, &op).await?;
        let received = out["received"].as_u64().unwrap_or(0);

        Ok((up, received))
    }

    /// Verifies and moves a finished upload into place.
    pub async fn commit_upload(&self, actor: &Actor, id: &str, sha256: &str) -> FileResult<Value> {
        let r = self.commit_upload_sized(actor, id, None, sha256).await;
        if r.is_ok() {
            self.forget_gate(id);
        }

        r
    }

    /// [`FileService::commit_upload`] for uploads whose final size is known
    /// only at the end (SFTP): `size` replaces the announced maximum.
    pub async fn commit_upload_sized(
        &self,
        actor: &Actor,
        id: &str,
        size: Option<u64>,
        sha256: &str,
    ) -> FileResult<Value> {
        let mut up = self.upload(actor, id).await?;

        if let Some(size) = size {
            if size > up.size {
                return Err(FileError::Invalid("the upload is larger than announced".into()));
            }

            up.size = size;
        }

        let op = FileOp::UploadCommit {
            root: RootSpec::Node,
            path: up.path.clone(),
            upload: up.id.clone(),
            size: up.size,
            sha256: sha256.to_owned(),
            overwrite: up.overwrite.clone(),
        };

        let out = match self.run(actor, up.server_id, &up.root_id, op, Audit::Record).await {
            Ok(out) => out,
            Err(e) => {
                // A conflict cannot be resumed: the node has removed the
                // partial file, so the session goes too (F-08).
                if matches!(e, FileError::Conflict(_)) {
                    let _ = self.repo.delete_upload(&up.id).await;
                    self.forget_gate(&up.id);
                }

                return Err(e);
            }
        };

        let _ = self.repo.delete_upload(&up.id).await;

        Ok(out)
    }

    /// Aborts an upload and discards what was received.
    pub async fn abort_upload(&self, actor: &Actor, id: &str) -> FileResult<()> {
        let up = self.upload(actor, id).await?;
        self.forget_gate(&up.id);
        let op = FileOp::UploadAbort { root: RootSpec::Node, path: up.path.clone(), upload: up.id.clone() };

        // The partial file is removed best-effort; the session goes either way.
        let _ = self.run(actor, up.server_id, &up.root_id, op, Audit::Record).await;

        self.repo.delete_upload(&up.id).await.map_err(|e| FileError::Internal(e.to_string()))
    }

    /// Removes expired upload sessions and their partial files.
    pub async fn cleanup_uploads(&self) -> usize {
        let Ok(expired) = self.repo.expired_uploads(self.clock.now()).await else { return 0 };
        let mut n = 0;

        for up in expired {
            let spec = match up.root_id.as_str() {
                NODE_ROOT => Some(RootSpec::Node),
                id => self.repo.get_root(id).await.ok().flatten().map(|r| r.spec),
            };

            if let Some(spec) = spec {
                let op = FileOp::UploadAbort { root: spec, path: up.path.clone(), upload: up.id.clone() };

                // Offline nodes keep the partial file until the next sweep.
                if self.transport.call(up.server_id, "system:file-cleanup", &op).await.is_err() {
                    continue;
                }
            }

            if self.repo.delete_upload(&up.id).await.is_ok() {
                n += 1;
            }
        }

        n
    }

    /// Saves editor content: a new file with `version: None`, otherwise only
    /// if the file is still at `version` (conflict otherwise).
    pub async fn save(
        &self,
        actor: &Actor,
        server_id: ServerId,
        root_id: &str,
        path: FilePath,
        content: &[u8],
        version: Option<String>,
    ) -> FileResult<Value> {
        if content.len() as u64 > EDITABLE_BYTES {
            return Err(FileError::TooLarge("the editor saves files up to 2 MB; upload larger files".into()));
        }

        let server = self.server(server_id).await?;
        let id = random_hex(16);
        let mut offset = 0usize;

        loop {
            let end = (offset + MAX_CHUNK_BYTES).min(content.len());
            let op = FileOp::UploadChunk {
                root: RootSpec::Node,
                path: path.clone(),
                upload: id.clone(),
                offset: offset as u64,
                data_b64: b64(&content[offset..end]),
            };
            let (op, _) = self.prepare(actor, &server, root_id, op).await?;
            self.call(actor, &server, &op).await?;
            offset = end;

            if offset >= content.len() {
                break;
            }
        }

        let sha = ring::digest::digest(&ring::digest::SHA256, content);
        let overwrite = version.map_or(Overwrite::Never, |version| Overwrite::IfVersion { version });
        let op = FileOp::UploadCommit {
            root: RootSpec::Node,
            path: path.clone(),
            upload: id.clone(),
            size: content.len() as u64,
            sha256: sha.as_ref().iter().map(|b| format!("{b:02x}")).collect(),
            overwrite,
        };
        let result = self.run(actor, server_id, root_id, op, Audit::Record).await;
        if result.is_err() {
            let abort = FileOp::UploadAbort { root: RootSpec::Node, path, upload: id };
            if let Ok((abort, _)) = self.prepare(actor, &server, root_id, abort).await {
                let _ = self.call(actor, &server, &abort).await;
            }
        }

        result
    }

    /// Approves a directory as a root (`file.admin`, MFA by guardrail).
    #[allow(clippy::too_many_arguments)]
    pub async fn add_root(
        &self,
        actor: &Actor,
        server_id: ServerId,
        kind: RootKind,
        label: String,
        spec: RootSpec,
        read_only: bool,
        service_id: Option<String>,
    ) -> FileResult<ApprovedRoot> {
        let server = self.server(server_id).await?;
        self.require(actor, Action::FileAdmin, &server).await?;
        spec.validate().map_err(FileError::Invalid)?;
        let label = label.trim().to_owned();
        if label.is_empty() || label.len() > 80 {
            return Err(FileError::Invalid("the label must be 1–80 characters".into()));
        }

        let os = Self::os_for(&server, &spec);

        // D-083: on Linux an app's folder becomes an app root (operations
        // run as the app's account); directories around or inside app
        // folders are refused.
        let spec = if os == NodeOs::Linux { spec.for_linux_apps().map_err(FileError::Invalid)? } else { spec };
        let base = match &spec {
            RootSpec::Node => return Err(FileError::Invalid("the whole node is built in".into())),
            RootSpec::Dir { base } => base.clone(),
            RootSpec::Container { base, .. } => {
                if NodeOs::from_reported(&server.os) == NodeOs::Windows {
                    return Err(FileError::Invalid("container roots need a Linux node".into()));
                }

                base.clone()
            }
            RootSpec::App { .. } => {
                if os == NodeOs::Windows {
                    return Err(FileError::Invalid("app roots need a Linux node".into()));
                }

                spec.base()
            }
        };

        let base = &base;

        // One root per node, kind and folder (F-06): a second approval of the
        // same folder is refused and names the root that exists.
        let existing = self.repo.list_roots(server_id).await.map_err(|e| FileError::Internal(e.to_string()))?;
        if let Some(dup) = existing.iter().find(|r| r.kind == kind && same_spec(os, &r.spec, &spec)) {
            return Err(FileError::Conflict(format!(
                "this folder is already a root on this node (`{}`, {}); remove it first to change it",
                dup.id, dup.label
            )));
        }

        if let Some(why) = protected_reason(os, base) {
            return Err(FileError::Denied(format!("protected path ({why}) cannot be a root")));
        }

        // The directory must exist and be a directory (not a link).
        let probe = FileOp::Stat { root: spec.clone(), path: FilePath::root() };
        let out = self.call(actor, &server, &probe).await?;
        if out.pointer("/entry/kind").and_then(Value::as_str) != Some("dir") {
            return Err(FileError::Invalid("the root must be a directory".into()));
        }

        let root = ApprovedRoot {
            id: new_root_id(),
            workspace_id: server.workspace_id,
            server_id,
            kind,
            label,
            spec,
            read_only,
            service_id,
            created_by: actor.key(),
            created_at: self.clock.now(),
        };
        self.repo.put_root(&root).await.map_err(|e| FileError::Internal(e.to_string()))?;
        self.record(
            actor,
            "file.root.create",
            &server,
            AuditOutcome::Success,
            json!({ "root": root.id, "kind": root.kind, "spec": root.spec, "read_only": read_only }),
        )
        .await;

        Ok(root)
    }

    /// Deletes a file root.
    pub async fn delete_root(&self, actor: &Actor, server_id: ServerId, id: &str) -> FileResult<()> {
        let server = self.server(server_id).await?;
        self.require(actor, Action::FileAdmin, &server).await?;
        let root = self
            .repo
            .get_root(id)
            .await
            .map_err(|e| FileError::Internal(e.to_string()))?
            .filter(|r| r.server_id == server_id)
            .ok_or_else(|| FileError::NotFound(format!("unknown root `{id}`")))?;
        self.repo.delete_root(&root.id).await.map_err(|e| FileError::Internal(e.to_string()))?;
        self.record(actor, "file.root.delete", &server, AuditOutcome::Success, json!({ "root": root.id })).await;

        Ok(())
    }
}

/// Whether two root specs cover the same folder (roots approved before
/// D-083 on an app folder count as that app's root).
fn same_spec(os: NodeOs, a: &RootSpec, b: &RootSpec) -> bool {
    let norm = |s: &RootSpec| {
        if os == NodeOs::Linux { s.clone().for_linux_apps().unwrap_or_else(|_| s.clone()) } else { s.clone() }
    };
    norm(a) == norm(b)
}

/// Drops exact duplicates (same kind, folder, service and read-only
/// flag), keeping the oldest of each.
fn dedupe_roots(os: NodeOs, mut roots: Vec<ApprovedRoot>) -> Vec<ApprovedRoot> {
    let mut by_age: Vec<usize> = (0..roots.len()).collect();
    by_age.sort_by_key(|&i| (roots[i].created_at.0, roots[i].id.clone()));
    let mut keep = vec![false; roots.len()];

    for (n, &i) in by_age.iter().enumerate() {
        let r = &roots[i];
        let dup = by_age[..n].iter().any(|&j| {
            let o = &roots[j];
            keep[j]
                && o.kind == r.kind
                && o.service_id == r.service_id
                && o.read_only == r.read_only
                && same_spec(os, &o.spec, &r.spec)
        });
        keep[i] = !dup;
    }

    let mut i = 0;
    roots.retain(|_| {
        i += 1;
        keep[i - 1]
    });

    roots
}

#[cfg(test)]
mod tests;

/// The bytes of a node read answer.
fn decode_b64(v: &Value) -> FileResult<Vec<u8>> {
    use base64::Engine as _;
    base64::engine::general_purpose::STANDARD
        .decode(v["content_b64"].as_str().unwrap_or_default())
        .map_err(|_| FileError::Internal("the node sent invalid content".into()))
}
