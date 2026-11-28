//! File explorer persistence: approved roots and upload sessions.
#![allow(clippy::unwrap_used)]

use forge_core::files::{ApprovedRoot, FilePath, FileRepository, FileUpload, Overwrite, RootKind, RootSpec};
use forge_core::model::*;
use forge_core::ports::*;
use forge_core::*;
use forge_store::Store;

async fn seed() -> (Store, tempfile::TempDir, WorkspaceId, ServerId) {
    let dir = tempfile::tempdir().unwrap();
    let s = Store::open_sqlite(&dir.path().join("forge.db")).await.unwrap();
    let org = Workspace { id: WorkspaceId::new(), name: "Acme".into(), slug: "acme".into(), created_at: Timestamp(1) };
    s.create_workspace(&org).await.unwrap();
    let server = Server {
        role: NodeRole::Worker,
        display_name: None,
        id: ServerId::new(),
        workspace_id: org.id,
        environment_id: None,
        cluster_id: None,
        hostname: "n".into(),
        os: "linux".into(),
        arch: "x86_64".into(),
        agent_version: "0".into(),
        status: AgentStatus::Online,
        last_seen_at: None,
        cert_serial: None,
        cert_not_after: None,
        drained: false,
        created_at: Timestamp(1),
    };
    s.create_server(&server).await.unwrap();

    (s, dir, org.id, server.id)
}

#[tokio::test]
async fn roots_and_uploads_round_trip() {
    let (store, _d, ws, server) = seed().await;
    let root = ApprovedRoot {
        id: "fr_0123456789abcdef01234567".into(),
        workspace_id: ws,
        server_id: server,
        kind: RootKind::Service,
        label: "web app".into(),
        spec: RootSpec::Dir { base: FilePath::parse("/srv/web").unwrap() },
        read_only: true,
        service_id: None,
        created_by: "user:x".into(),
        created_at: Timestamp(1),
    };
    store.put_root(&root).await.unwrap();
    assert_eq!(store.get_root(&root.id).await.unwrap(), Some(root.clone()));
    assert_eq!(store.list_roots(server).await.unwrap(), vec![root.clone()]);
    store.delete_root(&root.id).await.unwrap();
    assert!(store.get_root(&root.id).await.unwrap().is_none());

    let up = FileUpload {
        id: "0123456789abcdef0123456789abcdef".into(),
        workspace_id: ws,
        server_id: server,
        root_id: "node".into(),
        path: FilePath::parse("/tmp/a.bin").unwrap(),
        size: 10,
        received: 4,
        overwrite: Overwrite::IfVersion { version: "v1".into() },
        created_by: "user:x".into(),
        created_at: Timestamp(1),
        expires_at: Timestamp(100),
    };
    store.put_upload(&up).await.unwrap();
    assert_eq!(store.get_upload(&up.id).await.unwrap(), Some(up.clone()));
    assert!(store.expired_uploads(Timestamp(50)).await.unwrap().is_empty());
    assert_eq!(store.expired_uploads(Timestamp(101)).await.unwrap().len(), 1);
    store.delete_upload(&up.id).await.unwrap();
    assert!(store.get_upload(&up.id).await.unwrap().is_none());
}

#[tokio::test]
async fn grants_round_trip_and_count_usage() {
    use forge_core::files::{FileGrant, FileGrantRepository};
    let (store, _d, ws, server) = seed().await;
    let g = FileGrant {
        id: "fg0123456789abcdef".into(),
        workspace_id: ws,
        server_id: server,
        name: "vendor upload".into(),
        roots: vec!["node".into(), "fr_x".into()],
        write: true,
        delete: false,
        token_hash: "ab".repeat(32),
        created_by: "user:x".into(),
        owner: "user:x".into(),
        created_at: Timestamp(1),
        expires_at: Timestamp(1000),
        revoked_at: None,
        allowed_ips: vec!["203.0.113.0/24".into()],
        max_sessions: 2,
        bandwidth_kbps: Some(512),
        transfer_limit: Some(10_000),
        bytes_transferred: 0,
        last_used_at: None,
    };
    store.put_grant(&g).await.unwrap();
    assert_eq!(store.get_grant(&g.id).await.unwrap(), Some(g.clone()));
    store.add_grant_usage(&g.id, 300, Timestamp(5)).await.unwrap();
    store.add_grant_usage(&g.id, 200, Timestamp(6)).await.unwrap();
    let got = store.get_grant(&g.id).await.unwrap().unwrap();
    assert_eq!((got.bytes_transferred, got.last_used_at), (500, Some(Timestamp(6))));

    // Re-put updates the settings and revocation, never the usage counters
    // (the gateway adds to them concurrently) or the creation fields.
    let mut revoked = got.clone();
    revoked.revoked_at = Some(Timestamp(7));
    revoked.name = "changed".into();
    revoked.write = false;
    revoked.allowed_ips = vec![];
    revoked.transfer_limit = None;
    revoked.bytes_transferred = 0;
    revoked.created_by = "user:someone-else".into();
    revoked.owner = "user:new-owner".into();
    store.put_grant(&revoked).await.unwrap();
    let after = store.get_grant(&g.id).await.unwrap().unwrap();
    assert_eq!(after.revoked_at, Some(Timestamp(7)));
    assert_eq!(after.name, "changed");
    assert!(!after.write && after.allowed_ips.is_empty() && after.transfer_limit.is_none());
    assert_eq!(after.bytes_transferred, 500);
    assert_eq!(after.created_by, g.created_by);
    assert_eq!(after.owner, "user:new-owner");
    assert!(!after.is_active(Timestamp(8)));
    assert_eq!(store.list_grants(server).await.unwrap().len(), 1);
    assert_eq!(store.list_all_grants().await.unwrap().len(), 1);
}
