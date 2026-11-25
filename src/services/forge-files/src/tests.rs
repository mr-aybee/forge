#![allow(clippy::unwrap_used)]

use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use forge_core::model::*;
use forge_core::*;
use serde_json::{Value, json};

use super::*;

#[derive(Debug, Default)]
struct Mem {
    roots: Mutex<HashMap<String, ApprovedRoot>>,
    uploads: Mutex<HashMap<String, FileUpload>>,
}

#[async_trait]
impl FileRepository for Mem {
    async fn put_root(&self, r: &ApprovedRoot) -> DomainResult<()> {
        self.roots.lock().unwrap().insert(r.id.clone(), r.clone());
        Ok(())
    }
    async fn get_root(&self, id: &str) -> DomainResult<Option<ApprovedRoot>> {
        Ok(self.roots.lock().unwrap().get(id).cloned())
    }
    async fn list_roots(&self, s: ServerId) -> DomainResult<Vec<ApprovedRoot>> {
        Ok(self.roots.lock().unwrap().values().filter(|r| r.server_id == s).cloned().collect())
    }
    async fn delete_root(&self, id: &str) -> DomainResult<()> {
        self.roots.lock().unwrap().remove(id);
        Ok(())
    }
    async fn put_upload(&self, u: &FileUpload) -> DomainResult<()> {
        self.uploads.lock().unwrap().insert(u.id.clone(), u.clone());
        Ok(())
    }
    async fn get_upload(&self, id: &str) -> DomainResult<Option<FileUpload>> {
        Ok(self.uploads.lock().unwrap().get(id).cloned())
    }
    async fn delete_upload(&self, id: &str) -> DomainResult<()> {
        self.uploads.lock().unwrap().remove(id);
        Ok(())
    }
    async fn expired_uploads(&self, now: Timestamp) -> DomainResult<Vec<FileUpload>> {
        Ok(self.uploads.lock().unwrap().values().filter(|u| u.expires_at.0 < now.0).cloned().collect())
    }
}

#[derive(Debug)]
struct Servers(Server);

#[async_trait]
impl ServerRepository for Servers {
    async fn create_server(&self, _: &Server) -> DomainResult<()> {
        Ok(())
    }
    async fn get_server(&self, _: ServerId) -> DomainResult<Server> {
        Ok(self.0.clone())
    }
    async fn rename_server(&self, _: ServerId, _: Option<&str>) -> DomainResult<()> {
        Ok(())
    }
    async fn set_node_label(&self, _: ServerId, _: WorkspaceId, _: &str, _: bool, _: Timestamp) -> DomainResult<()> {
        Ok(())
    }
    async fn claim_node_label(&self, _: ServerId, _: WorkspaceId, _: &str, _: Timestamp) -> DomainResult<bool> {
        Ok(true)
    }
    async fn list_node_labels(&self, _: Option<WorkspaceId>) -> DomainResult<Vec<(ServerId, String, bool)>> {
        Ok(Vec::new())
    }
    async fn list_servers(&self, _: Option<WorkspaceId>) -> DomainResult<Vec<Server>> {
        Ok(vec![self.0.clone()])
    }
    async fn find_server_by_hostname(&self, _: WorkspaceId, _: &str) -> DomainResult<Option<Server>> {
        Ok(None)
    }
    async fn record_heartbeat(&self, _: ServerId, _: Timestamp, _: &str) -> DomainResult<()> {
        Ok(())
    }
    async fn set_status(&self, _: ServerId, _: AgentStatus) -> DomainResult<()> {
        Ok(())
    }
    async fn set_certificate(&self, _: ServerId, _: &str, _: Timestamp) -> DomainResult<()> {
        Ok(())
    }
    async fn mark_stale_offline(&self, _: Timestamp) -> DomainResult<Vec<ServerId>> {
        Ok(vec![])
    }
}

/// Allows the listed (principal, action) pairs; MFA-only actions need `mfa`.
#[derive(Debug, Default)]
struct Allow(Mutex<HashSet<(String, Action)>>);

#[async_trait]
impl Authorizer for Allow {
    async fn authorize(&self, r: &AccessRequest) -> DomainResult<()> {
        let mfa_only = matches!(r.action, Action::FileWrite | Action::FileDelete | Action::FileAdmin);
        if mfa_only && !r.context.mfa {
            return Err(DomainError::Denied("step-up MFA required".into()));
        }

        if self.0.lock().unwrap().contains(&(r.principal.to_string(), r.action)) {
            Ok(())
        } else {
            Err(DomainError::Denied("no".into()))
        }
    }
}

#[derive(Debug, Default)]
struct Audits(Mutex<Vec<AuditEvent>>);

#[async_trait]
impl AuditRecorder for Audits {
    async fn record(&self, e: AuditEvent) -> DomainResult<AuditEntry> {
        self.0.lock().unwrap().push(e);
        Err(DomainError::Internal("not stored in tests".into()))
    }
}

/// Records operations and answers with a canned reply.
#[derive(Debug, Default)]
struct Node {
    ops: Mutex<Vec<FileOp>>,
    reply: Mutex<Option<Result<Value, String>>>,
    /// Paths a `stat` finds (besides a root's own base, `/`).
    existing: Mutex<HashSet<String>>,
}

#[async_trait]
impl FileTransport for Node {
    async fn call(&self, _: ServerId, _: &str, op: &FileOp) -> Result<Value, String> {
        self.ops.lock().unwrap().push(op.clone());
        if let FileOp::UploadChunk { offset, data_b64, .. } = op {
            let len = data_b64.len() / 4 * 3;
            return Ok(json!({ "received": *offset as usize + len }));
        }
        if let FileOp::Stat { path, .. } = op
            && !path.is_root()
            && !self.existing.lock().unwrap().contains(&path.to_string())
        {
            return Err(format!("`{}` was not found", path.name().unwrap_or("/")));
        }

        self.reply.lock().unwrap().clone().unwrap_or_else(|| Ok(json!({ "entry": { "kind": "dir" }, "entries": [] })))
    }
}

struct Env {
    svc: FileService,
    allow: Arc<Allow>,
    node: Arc<Node>,
    audits: Arc<Audits>,
    repo: Arc<Mem>,
    server: ServerId,
}

fn env(os: &str) -> Env {
    let server = Server {
        role: NodeRole::Worker,
        display_name: None,
        id: ServerId::new(),
        workspace_id: WorkspaceId::new(),
        environment_id: None,
        cluster_id: None,
        hostname: "n".into(),
        os: os.into(),
        arch: "x86_64".into(),
        agent_version: "0".into(),
        status: AgentStatus::Online,
        last_seen_at: None,
        cert_serial: None,
        cert_not_after: None,
        created_at: Timestamp(1),
    };
    let (allow, node, audits, repo) =
        (Arc::new(Allow::default()), Arc::new(Node::default()), Arc::new(Audits::default()), Arc::new(Mem::default()));
    let svc = FileService::new(
        repo.clone(),
        Arc::new(Servers(server.clone())),
        allow.clone(),
        audits.clone(),
        node.clone(),
        Arc::new(ManualClock::new(Timestamp(1000))),
    );

    Env { svc, allow, node, audits, repo, server: server.id }
}

fn user(e: &Env, actions: &[Action], mfa: bool) -> Actor {
    let p = Principal::User(UserId::new());

    for a in actions {
        e.allow.0.lock().unwrap().insert((p.to_string(), *a));
    }

    Actor::user(p, AuthzContext { mfa })
}

fn p(s: &str) -> FilePath {
    FilePath::parse(s).unwrap()
}

fn list(path: &str) -> FileOp {
    FileOp::List { root: RootSpec::Node, path: p(path), offset: 0, limit: 10, hidden: false }
}

fn approve(e: &Env, read_only: bool) -> String {
    let id = format!("fr_{}", if read_only { "ro" } else { "rw" });
    let root = ApprovedRoot {
        id: id.clone(),
        workspace_id: WorkspaceId::new(),
        server_id: e.server,
        kind: RootKind::Service,
        label: "app".into(),
        spec: RootSpec::Dir { base: p("/srv/app") },
        read_only,
        service_id: None,
        created_by: "x".into(),
        created_at: Timestamp(1),
    };
    e.repo.roots.lock().unwrap().insert(id.clone(), root);

    id
}

#[tokio::test]
async fn node_root_needs_file_node() {
    let e = env("linux");
    let op = user(&e, &[Action::FileRead], false);
    assert!(matches!(e.svc.run(&op, e.server, "node", list("/etc"), Audit::Record).await, Err(FileError::Denied(_))));
    let (_, roots) = e.svc.roots(&op, e.server).await.unwrap();
    assert!(roots.is_empty());
    let admin = user(&e, &[Action::FileRead, Action::FileNode], false);
    e.svc.run(&admin, e.server, "node", list("/etc"), Audit::Record).await.unwrap();
    let (_, roots) = e.svc.roots(&admin, e.server).await.unwrap();
    assert_eq!(roots[0].id, "node");

    // The node received the op bound to the resolved root.
    assert!(matches!(&e.node.ops.lock().unwrap()[0], FileOp::List { root: RootSpec::Node, .. }));

    // Successes and denials are both audited.
    let audits = e.audits.0.lock().unwrap();
    assert!(audits.iter().any(|a| a.action == "file.node" && a.outcome == AuditOutcome::Denied));
    assert!(audits.iter().any(|a| a.action == "file.list" && a.outcome == AuditOutcome::Success));
}

#[tokio::test]
async fn approved_roots_bind_the_spec_and_enforce_read_only() {
    let e = env("linux");
    let ro = approve(&e, true);
    let rw = approve(&e, false);
    let op = user(&e, &[Action::FileRead, Action::FileWrite, Action::FileDelete], true);
    e.svc.run(&op, e.server, &ro, list("/conf"), Audit::Record).await.unwrap();

    match &e.node.ops.lock().unwrap()[0] {
        FileOp::List { root: RootSpec::Dir { base }, .. } => assert_eq!(base.to_string(), "/srv/app"),
        other => panic!("{other:?}"),
    }

    let mkdir = FileOp::Mkdir { root: RootSpec::Node, path: p("/x"), parents: false };
    assert!(matches!(e.svc.run(&op, e.server, &ro, mkdir.clone(), Audit::Record).await, Err(FileError::Denied(_))));
    e.svc.run(&op, e.server, &rw, mkdir, Audit::Record).await.unwrap();

    // Unknown roots and roots of other servers are not found.
    assert!(matches!(e.svc.run(&op, e.server, "fr_nope", list("/"), Audit::Record).await, Err(FileError::NotFound(_))));
    let (_, roots) = e.svc.roots(&op, e.server).await.unwrap();
    assert_eq!(roots.len(), 2);
    assert!(!roots.iter().find(|r| r.id == ro).unwrap().permissions.write);
}

#[tokio::test]
async fn moves_need_write_and_delete_and_mfa() {
    let e = env("linux");
    let rw = approve(&e, false);
    let mv = FileOp::Rename { root: RootSpec::Node, from: p("/a"), to: p("/b"), overwrite: false };
    let writer = user(&e, &[Action::FileRead, Action::FileWrite], true);
    assert!(matches!(e.svc.run(&writer, e.server, &rw, mv.clone(), Audit::Record).await, Err(FileError::Denied(_))));
    let no_mfa = user(&e, &[Action::FileRead, Action::FileWrite, Action::FileDelete], false);
    assert!(matches!(e.svc.run(&no_mfa, e.server, &rw, mv.clone(), Audit::Record).await, Err(FileError::StepUp(_))));
    let full = user(&e, &[Action::FileRead, Action::FileWrite, Action::FileDelete], true);
    e.svc.run(&full, e.server, &rw, mv, Audit::Record).await.unwrap();
}

#[tokio::test]
async fn protected_paths_never_reach_the_node() {
    let e = env("linux");
    let admin = user(&e, &[Action::FileRead, Action::FileNode, Action::FileWrite], true);

    for path in ["/etc/shadow", "/etc/forge/pkgs/x.env", "/proc/1/environ"] {
        let op = FileOp::Read { root: RootSpec::Node, path: p(path), offset: 0, length: 10, version: None };
        assert!(matches!(e.svc.run(&admin, e.server, "node", op, Audit::Record).await, Err(FileError::Denied(_))));
    }

    // An approved root's base counts: /srv/app + /x is checked as a whole.
    let up = e.svc.begin_upload(&admin, e.server, "node", p("/etc/forge/x"), 1, Overwrite::Never).await;
    assert!(matches!(up, Err(FileError::Denied(_))));
    assert!(e.node.ops.lock().unwrap().is_empty());

    // Windows policy on Windows nodes.
    let w = env("windows");
    let a = user(&w, &[Action::FileRead, Action::FileNode], false);
    let op = FileOp::Read {
        root: RootSpec::Node,
        path: p("/C:/ProgramData/Forge/agent.toml"),
        offset: 0,
        length: 1,
        version: None,
    };
    assert!(matches!(w.svc.run(&a, w.server, "node", op, Audit::Record).await, Err(FileError::Denied(_))));
}

#[tokio::test]
async fn uploads_belong_to_their_creator() {
    let e = env("linux");
    let rw = approve(&e, false);
    let alice = user(&e, &[Action::FileRead, Action::FileWrite], true);
    let bob = user(&e, &[Action::FileRead, Action::FileWrite], true);
    let up = e.svc.begin_upload(&alice, e.server, &rw, p("/f.bin"), 6, Overwrite::Never).await.unwrap();
    assert_eq!(up.id.len(), 32);
    assert_eq!(e.svc.upload_chunk(&alice, &up.id, 0, b"abc").await.unwrap(), 3);
    assert!(matches!(e.svc.upload_chunk(&bob, &up.id, 3, b"def").await, Err(FileError::NotFound(_))));
    assert!(matches!(e.svc.upload_chunk(&alice, &up.id, 3, b"defg").await, Err(FileError::Invalid(_))));
    *e.node.reply.lock().unwrap() = Some(Ok(json!({ "path": "/f.bin", "size": 6 })));
    e.svc.commit_upload(&alice, &up.id, &"0".repeat(64)).await.unwrap();
    assert!(e.repo.uploads.lock().unwrap().is_empty());

    match e.node.ops.lock().unwrap().last().unwrap() {
        FileOp::UploadCommit { root: RootSpec::Dir { .. }, size: 6, .. } => {}
        other => panic!("{other:?}"),
    }
}

/// Review of #15: a stranger never waits on someone's upload, and chunks
/// waiting for earlier ones are capped per upload.
#[tokio::test]
async fn upload_gate_checks_the_owner_first_and_caps_waiters() {
    let e = env("linux");
    let rw = approve(&e, false);
    let alice = user(&e, &[Action::FileRead, Action::FileWrite], true);
    let bob = user(&e, &[Action::FileRead, Action::FileWrite], true);
    let up = e.svc.begin_upload(&alice, e.server, &rw, p("/f.bin"), 100, Overwrite::Never).await.unwrap();

    // Far ahead, from someone else: refused at once, not after the wait.
    let t = std::time::Instant::now();
    assert!(matches!(e.svc.upload_chunk(&bob, &up.id, 50, b"x").await, Err(FileError::NotFound(_))));
    assert!(matches!(e.svc.upload_chunk(&alice, &up.id, 99, b"xy").await, Err(FileError::Invalid(_))), "past the size");
    assert!(t.elapsed() < std::time::Duration::from_secs(5));

    // Three chunks may wait for the first; a fourth is refused at once.
    let svc = Arc::new(e.svc);
    let waiting: Vec<_> = (1..=3u64)
        .map(|i| {
            let (svc, a, id) = (svc.clone(), alice.clone(), up.id.clone());
            tokio::spawn(async move { svc.upload_chunk(&a, &id, i * 12, &[b'x'; 12]).await })
        })
        .collect();
    tokio::time::sleep(std::time::Duration::from_millis(200)).await;
    assert!(matches!(svc.upload_chunk(&alice, &up.id, 48, &[b'x'; 12]).await, Err(FileError::Busy(_))));

    // The first chunk arrives: the waiting ones follow in order.
    assert_eq!(svc.upload_chunk(&alice, &up.id, 0, &[b'x'; 12]).await.unwrap(), 12);

    for (i, w) in waiting.into_iter().enumerate() {
        assert_eq!(w.await.unwrap().unwrap(), 12 * (i as u64 + 2));
    }

    // The slots are free again.
    assert_eq!(svc.upload_chunk(&alice, &up.id, 48, &[b'x'; 12]).await.unwrap(), 60);
}

#[tokio::test]
async fn save_chunks_and_commits_with_the_version() {
    let e = env("linux");
    let rw = approve(&e, false);
    let u = user(&e, &[Action::FileRead, Action::FileWrite], true);
    let content = vec![b'x'; MAX_CHUNK_BYTES + 10];
    *e.node.reply.lock().unwrap() = Some(Ok(json!({ "version": "v2" })));
    e.svc.save(&u, e.server, &rw, p("/big.txt"), &content, Some("v1".into())).await.unwrap();

    {
        let ops = e.node.ops.lock().unwrap();
        assert_eq!(ops.iter().filter(|o| matches!(o, FileOp::UploadChunk { .. })).count(), 2);

        match ops.last().unwrap() {
            FileOp::UploadCommit { overwrite: Overwrite::IfVersion { version }, .. } => assert_eq!(version, "v1"),
            other => panic!("{other:?}"),
        }
    }

    let too_big = vec![0u8; usize::try_from(EDITABLE_BYTES).unwrap() + 1];
    assert!(matches!(e.svc.save(&u, e.server, &rw, p("/x"), &too_big, None).await, Err(FileError::TooLarge(_))));
}

#[tokio::test]
async fn moving_or_deleting_a_parent_of_protected_paths_is_denied() {
    let e = env("linux");
    let admin = user(&e, &[Action::FileRead, Action::FileNode, Action::FileWrite, Action::FileDelete], true);

    for (from, to) in [("/home/alice", "/tmp/a"), ("/var", "/v"), ("/root", "/r"), ("/tmp/x", "/home/bob")] {
        let mv = FileOp::Rename { root: RootSpec::Node, from: p(from), to: p(to), overwrite: false };
        assert!(matches!(e.svc.run(&admin, e.server, "node", mv, Audit::Record).await, Err(FileError::Denied(_))));
    }

    for permanent in [false, true] {
        let del = FileOp::Delete { root: RootSpec::Node, path: p("/var/lib"), permanent };
        assert!(matches!(e.svc.run(&admin, e.server, "node", del, Audit::Record).await, Err(FileError::Denied(_))));
    }

    assert!(e.node.ops.lock().unwrap().is_empty());
    assert!(e.audits.0.lock().unwrap().iter().any(|a| a.action == "file.move" && a.outcome == AuditOutcome::Denied));
    let ok =
        FileOp::Rename { root: RootSpec::Node, from: p("/home/alice/a"), to: p("/home/alice/b"), overwrite: false };
    e.svc.run(&admin, e.server, "node", ok, Audit::Record).await.unwrap();

    // Windows: 8.3 short names and parents of protected folders.
    let w = env("windows");
    let a = user(&w, &[Action::FileRead, Action::FileNode, Action::FileDelete], true);

    for path in ["/C:/PROGRA~3/Forge/agent.toml", "/C:/ProgramData", "/C:/Users/bob"] {
        let del = FileOp::Delete { root: RootSpec::Node, path: p(path), permanent: true };
        assert!(matches!(w.svc.run(&a, w.server, "node", del, Audit::Record).await, Err(FileError::Denied(_))));
    }

    assert!(w.node.ops.lock().unwrap().is_empty());
}

#[tokio::test]
async fn temporary_archives_belong_to_their_creator_and_root() {
    let e = env("linux");
    let rw = approve(&e, false);
    let alice = user(&e, &[Action::FileRead, Action::FileNode], false);
    let bob = user(&e, &[Action::FileRead, Action::FileNode], false);
    *e.node.reply.lock().unwrap() = Some(Ok(json!({ "temp_id": "abc-123", "size": 10 })));
    let make = FileOp::Compress {
        root: RootSpec::Node,
        sources: vec![p("/srv/x")],
        dest: None,
        format: forge_core::files::ArchiveFormat::Zip,
    };
    e.svc.run(&alice, e.server, "node", make, Audit::Record).await.unwrap();
    let read = |root: &str| {
        (root.to_owned(), FileOp::TempRead { root: RootSpec::Node, id: "abc-123".into(), offset: 0, length: 10 })
    };

    // Someone else, another root, or an unknown id: not found, and nothing reaches the node.
    let sent = e.node.ops.lock().unwrap().len();

    for (who, (root, op)) in [(&bob, read("node")), (&alice, read(&rw))] {
        assert!(matches!(e.svc.run(who, e.server, &root, op, Audit::Record).await, Err(FileError::NotFound(_))));
    }

    let unknown = FileOp::TempDelete { root: RootSpec::Node, id: "nope".into() };
    assert!(matches!(e.svc.run(&alice, e.server, "node", unknown, Audit::Record).await, Err(FileError::NotFound(_))));
    assert_eq!(e.node.ops.lock().unwrap().len(), sent);

    // The creator reads it through the same root, then deletes it.
    let (root, op) = read("node");
    e.svc.run(&alice, e.server, &root, op, Audit::Skip).await.unwrap();
    let del = FileOp::TempDelete { root: RootSpec::Node, id: "abc-123".into() };
    e.svc.run(&alice, e.server, "node", del.clone(), Audit::Record).await.unwrap();
    assert!(matches!(e.svc.run(&alice, e.server, "node", del, Audit::Record).await, Err(FileError::NotFound(_))));
}

#[test]
fn node_errors_map() {
    assert!(matches!(node_error("`x` was not found"), FileError::NotFound(_)));
    assert!(matches!(node_error("source changed: x"), FileError::Conflict(_)));
    assert!(matches!(node_error("protected path (x)"), FileError::Denied(_)));
    assert!(matches!(node_error("command `files` is not allowed by this agent's local policy"), FileError::Outdated));
    assert!(matches!(node_error("transport: the node did not answer"), FileError::Unavailable(_)));
    assert!(matches!(node_error("`x` is a link; Forge does not follow links"), FileError::Node(_)));
}
#[derive(Debug, Default)]
struct Grants(Mutex<HashMap<String, forge_core::files::FileGrant>>);

#[async_trait]
impl forge_core::files::FileGrantRepository for Grants {
    async fn put_grant(&self, g: &forge_core::files::FileGrant) -> DomainResult<()> {
        self.0.lock().unwrap().insert(g.id.clone(), g.clone());
        Ok(())
    }
    async fn get_grant(&self, id: &str) -> DomainResult<Option<forge_core::files::FileGrant>> {
        Ok(self.0.lock().unwrap().get(id).cloned())
    }
    async fn list_grants(&self, s: ServerId) -> DomainResult<Vec<forge_core::files::FileGrant>> {
        Ok(self.0.lock().unwrap().values().filter(|g| g.server_id == s).cloned().collect())
    }
    async fn list_all_grants(&self) -> DomainResult<Vec<forge_core::files::FileGrant>> {
        Ok(self.0.lock().unwrap().values().cloned().collect())
    }
    async fn add_grant_usage(&self, _: &str, _: u64, _: Timestamp) -> DomainResult<()> {
        Ok(())
    }
}

fn delete(path: &str) -> FileOp {
    FileOp::Delete { root: RootSpec::Node, path: p(path), permanent: false }
}

#[tokio::test]
async fn grants_act_with_their_owners_current_rights_and_widening_moves_ownership() {
    let mut e = env("linux");
    e.svc = e.svc.with_grants(Arc::new(Grants::default()), 24);
    let rw = approve(&e, false);
    let full = [Action::FileRead, Action::FileWrite, Action::FileDelete];
    let alice = user(&e, &full, true);
    let req = NewGrant {
        name: "vendor".into(),
        roots: vec![rw.clone()],
        write: true,
        delete: true,
        expires_in_minutes: 60,
        never_expires: false,
        allowed_ips: vec![],
        max_sessions: 1,
        bandwidth_kbps: None,
        transfer_limit: None,
    };
    let issued = e.svc.create_grant(&alice, e.server, req).await.unwrap();
    let id = issued.grant.id.clone();
    assert_eq!(issued.grant.owner, alice.key());
    let g = e.svc.authenticate(&id, &issued.token, None).await.unwrap();
    e.svc.run(&g, e.server, &rw, delete("/a"), Audit::Record).await.unwrap();

    // The owner loses `file.delete`: deleting through the grant is refused,
    // reading still works.
    e.allow.0.lock().unwrap().remove(&(alice.key(), Action::FileDelete));

    match e.svc.run(&g, e.server, &rw, delete("/b"), Audit::Record).await {
        Err(FileError::Denied(m)) => assert!(m.contains("owner"), "{m}"),
        other => panic!("{other:?}"),
    }

    e.svc.run(&g, e.server, &rw, list("/"), Audit::Record).await.unwrap();

    // An admin narrows it: the owner stays.
    let bob = user(&e, &[Action::FileRead, Action::FileWrite, Action::FileDelete, Action::FileAdmin], true);
    let narrow = GrantChanges { delete: Some(false), ..GrantChanges::default() };
    let after = e.svc.update_grant(&bob, e.server, &id, narrow).await.unwrap();
    assert_eq!(after.owner, alice.key());

    // The admin widens it: the admin becomes the owner, and deleting works
    // again with the admin's rights.
    let widen = GrantChanges { delete: Some(true), ..GrantChanges::default() };
    let after = e.svc.update_grant(&bob, e.server, &id, widen).await.unwrap();
    assert_eq!(after.owner, bob.key());
    assert_eq!(after.created_by, alice.key(), "the creator is kept");
    let g = e.svc.authenticate(&id, &issued.token, None).await.unwrap();
    e.svc.run(&g, e.server, &rw, delete("/c"), Audit::Record).await.unwrap();

    // The ownership change is audited with the diff.
    let audits = e.audits.0.lock().unwrap();
    let moved =
        audits.iter().find(|a| a.action == "file.grant.update" && a.details["changes"]["owner"].is_object()).unwrap();
    assert_eq!(moved.details["changes"]["owner"]["before"], json!(alice.key()));
    assert_eq!(moved.details["changes"]["owner"]["after"], json!(bob.key()));
    assert_eq!(moved.details["sessions_closed"], true);
}

/// F-06: the same folder is approved once per node and kind; exact
/// duplicates stored by older servers are listed once.
#[tokio::test]
async fn duplicate_roots_are_refused_and_old_duplicates_listed_once() {
    let e = env("linux");
    let admin = user(&e, &[Action::FileRead, Action::FileAdmin], true);
    let dir = || RootSpec::Dir { base: p("/srv/web") };
    let first = e.svc.add_root(&admin, e.server, RootKind::Service, "web".into(), dir(), false, None).await.unwrap();
    let again = e.svc.add_root(&admin, e.server, RootKind::Service, "web2".into(), dir(), true, None).await;
    match again {
        Err(FileError::Conflict(m)) => assert!(m.contains(&first.id), "{m}"),
        other => panic!("{other:?}"),
    }

    // An app folder approved as a plain directory is the same root as the app root.
    let app = || RootSpec::App { environment: "lab".into(), app: "web".into() };
    let a = e.svc.add_root(&admin, e.server, RootKind::Service, "a".into(), app(), false, None).await.unwrap();
    let as_dir = RootSpec::Dir { base: p("/srv/forge/apps/lab/web") };
    let dup = e.svc.add_root(&admin, e.server, RootKind::Service, "b".into(), as_dir, false, None).await;
    assert!(matches!(dup, Err(FileError::Conflict(_))), "{dup:?}");

    // A duplicate row written by an older server: hidden from the list, still usable by id.
    let mut old = a.clone();
    old.id = "fr_dup".into();
    old.created_at = Timestamp(a.created_at.0 + 1);
    e.repo.roots.lock().unwrap().insert(old.id.clone(), old);
    let (_, roots) = e.svc.roots(&admin, e.server).await.unwrap();
    assert_eq!(roots.iter().filter(|r| r.base == "/srv/forge/apps/lab/web").count(), 1);
    assert!(roots.iter().any(|r| r.id == a.id));
    e.svc.run(&admin, e.server, "fr_dup", list("/"), Audit::Record).await.unwrap();
}

/// F-08: an upload that may not replace an existing file is refused before
/// any byte is sent; a refused commit drops the session.
#[tokio::test]
async fn uploads_onto_existing_files_are_refused_before_streaming() {
    let e = env("linux");
    let rw = approve(&e, false);
    let alice = user(&e, &[Action::FileRead, Action::FileWrite], true);
    e.node.existing.lock().unwrap().insert("/site.zip".into());
    let refused = e.svc.begin_upload(&alice, e.server, &rw, p("/site.zip"), 6, Overwrite::Never).await;
    assert!(matches!(refused, Err(FileError::Conflict(_))), "{refused:?}");
    assert!(e.repo.uploads.lock().unwrap().is_empty());
    assert!(!e.node.ops.lock().unwrap().iter().any(|o| matches!(o, FileOp::UploadChunk { .. })));

    // With overwrite the upload starts; a conflict at commit ends the session.
    let up = e.svc.begin_upload(&alice, e.server, &rw, p("/site.zip"), 3, Overwrite::Always).await.unwrap();
    e.svc.upload_chunk(&alice, &up.id, 0, b"abc").await.unwrap();
    *e.node.reply.lock().unwrap() = Some(Err("the destination was created by someone else meanwhile".into()));
    assert!(matches!(e.svc.commit_upload(&alice, &up.id, &"0".repeat(64)).await, Err(FileError::Conflict(_))));
    assert!(e.repo.uploads.lock().unwrap().is_empty());
}

/// F-07: replacing a folder by extraction deletes its old content, so it
/// needs `file.delete` too; old agents' refusal of new fields is explained.
#[tokio::test]
async fn extract_replace_needs_delete() {
    let e = env("linux");
    let rw = approve(&e, false);
    let writer = user(&e, &[Action::FileRead, Action::FileWrite], true);
    let ex = |replace| FileOp::Extract { root: RootSpec::Node, archive: p("/a.zip"), dest: p("/site"), replace };
    e.svc.run(&writer, e.server, &rw, ex(false), Audit::Record).await.unwrap();
    assert!(matches!(e.svc.run(&writer, e.server, &rw, ex(true), Audit::Record).await, Err(FileError::Denied(_))));
    let full = user(&e, &[Action::FileRead, Action::FileWrite, Action::FileDelete], true);
    e.svc.run(&full, e.server, &rw, ex(true), Audit::Record).await.unwrap();
    let audits = e.audits.0.lock().unwrap();
    assert!(audits.iter().any(|a| a.action == "file.extract" && a.details["replace"] == json!(true)));
    let old = node_error("invalid file operation: unknown field `parents`, expected one of `root`, `path`");
    assert!(matches!(old, FileError::Invalid(m) if m.contains("update the agent")));
}
