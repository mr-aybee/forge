//! Automatic file roots for native apps (F-03 / F-05).
//!
//! When a native app is prepared or deployed on a Linux node, Forge approves
//! a writable `service` root on its folder (`RootSpec::App`, D-083: every
//! operation runs as the app's own account, confined to its folder), so code
//! can be uploaded without an administrator approving the folder first. When
//! the app leaves the node (or is deleted), that root goes too. Both are
//! audited as `file.root.create` / `file.root.delete` with `automatic: true`.
//!
//! Authorization: these run inside an `app.action` job (re-authorized as
//! `deploy` on the environment at every step) or a services API call that
//! has authorized `deploy` (and `server.manage` for placements). The root
//! only covers the app's own folder, which the deployer controls anyway;
//! using it still needs `file.read`/`file.write` on the node.

use forge_catalog::Runtime;
use forge_core::files::{ApprovedRoot, FileRepository, NodeOs, RootKind, RootSpec};
use forge_core::model::{AuditEvent, AuditOutcome, Server, Service};
use forge_core::ports::AuditRecorder;
use forge_core::{DomainResult, EntityKind, EntityRef, ServerId, Timestamp};
use serde_json::json;

use crate::jobs::app::build_spec;

/// The app-root spec of `app`, when it is a native app (service or job).
#[must_use]
pub fn spec_for(app: &Service) -> Option<RootSpec> {
    let spec = build_spec(app, None).ok()?;
    if spec.runtime == Runtime::Container {
        return None;
    }
    let root = RootSpec::App { environment: spec.environment, app: spec.name };
    root.validate().ok().map(|()| root)
}

/// Whether `server` can hold app roots (native apps run on Linux nodes).
#[must_use]
pub fn supported(server: &Server) -> bool {
    NodeOs::from_reported(&server.os) == NodeOs::Linux
}

fn same(a: &RootSpec, b: &RootSpec) -> bool {
    a.clone().for_linux_apps().unwrap_or_else(|_| a.clone()) == *b
}

async fn record(
    audit: &dyn AuditRecorder,
    actor: &str,
    action: &str,
    app: &Service,
    server: ServerId,
    details: serde_json::Value,
) {
    let ev = AuditEvent::new(actor, action, EntityRef::new(EntityKind::Server, server), AuditOutcome::Success)
        .in_org(Some(app.workspace_id))
        .with_details(details);
    if let Err(e) = audit.record(ev).await {
        tracing::error!(error = %e, "audit write failed");
    }
}

/// Approves the app's writable root on `server` unless a root on that folder
/// exists already (of any kind, approved by hand or not). Idempotent.
/// Returns the new root, if one was created.
pub async fn ensure(
    files: &dyn FileRepository,
    audit: &dyn AuditRecorder,
    actor: &str,
    now: Timestamp,
    app: &Service,
    server: &Server,
) -> DomainResult<Option<ApprovedRoot>> {
    let Some(target) = spec_for(app) else { return Ok(None) };
    if !supported(server) {
        return Ok(None);
    }
    if files.list_roots(server.id).await?.iter().any(|r| same(&r.spec, &target)) {
        return Ok(None);
    }
    let root = ApprovedRoot {
        id: forge_files::new_root_id(),
        workspace_id: server.workspace_id,
        server_id: server.id,
        kind: RootKind::Service,
        label: app.name.clone(),
        spec: target,
        read_only: false,
        service_id: Some(app.id.to_string()),
        created_by: actor.to_owned(),
        created_at: now,
    };
    files.put_root(&root).await?;
    let details = json!({
        "root": root.id, "kind": root.kind, "spec": root.spec, "read_only": false,
        "service_id": app.id, "service": app.name, "automatic": true,
    });
    record(audit, actor, "file.root.create", app, server.id, details).await;
    Ok(Some(root))
}

/// Removes the app's roots on `server_id`: roots on the app's folder that
/// belong to this service. Other roots are kept. Idempotent; returns
/// how many were removed.
pub async fn remove(
    files: &dyn FileRepository,
    audit: &dyn AuditRecorder,
    actor: &str,
    app: &Service,
    server_id: ServerId,
) -> DomainResult<usize> {
    let Some(target) = spec_for(app) else { return Ok(0) };
    let service_id = app.id.to_string();
    let mut n = 0;
    for r in files.list_roots(server_id).await? {
        if r.kind == RootKind::Service && r.service_id.as_deref() == Some(service_id.as_str()) && same(&r.spec, &target)
        {
            files.delete_root(&r.id).await?;
            let details = json!({ "root": r.id, "service_id": app.id, "service": app.name, "automatic": true });
            record(audit, actor, "file.root.delete", app, server_id, details).await;
            n += 1;
        }
    }
    Ok(n)
}
