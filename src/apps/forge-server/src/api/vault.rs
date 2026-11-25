//! Credential vault: accounts and passwords kept for people, tagged and
//! linked to nodes, services and clusters.
//!
//! * Only **platform administrators** can use it: list, create, edit, delete.
//!   Creating, changing and deleting items also go through Cedar
//!   (`secret.manage`, MFA-guarded), so guardrails and connector limits apply.
//! * Revealing a password additionally needs `secret.read` (MFA-guarded) and
//!   a browser session (not an API key). Every reveal is audited, and so is
//!   every refused attempt. Responses are `Cache-Control: no-store` (all API
//!   responses are).
//! * Only the password is sealed (envelope encryption, AAD bound to the item
//!   id, so a sealed value cannot be moved to another item). Name, username,
//!   URL, notes, tags and links are metadata.
//!
//! * `GET    /api/v1/vault/items`
//! * `POST   /api/v1/vault/items`
//! * `PUT    /api/v1/vault/items/{id}` — `password` present = rotate
//! * `DELETE /api/v1/vault/items/{id}`
//! * `POST   /api/v1/vault/items/{id}/reveal`

use std::collections::HashMap;

use axum::Json;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use forge_core::access::Role;
use forge_core::model::{VaultItem, VaultLink};
use forge_core::ports::{
    ClusterRepository, RoleBindingRepository, ServerRepository, ServiceRepository, TenancyRepository, VaultRepository,
};
use forge_core::{Action, DomainError, EntityKind, EntityRef, Principal, VaultItemId, WorkspaceId};
use serde::Deserialize;
use serde_json::{Value, json};

use crate::auth::{Caller, Ws};
use crate::error::{ApiError, ApiResult};
use crate::state::AppState;

const MAX_PASSWORD: usize = 4096;

fn aad(id: VaultItemId) -> String {
    format!("vault:{id}")
}

/// The vault is for platform administrators only.
async fn require_platform_admin(state: &AppState, caller: &Caller, ws: WorkspaceId, what: &str) -> ApiResult<()> {
    let Principal::User(me) = caller.principal();
    let ok = state.store.list_bindings_for_user(me).await?.iter().any(|b| b.role == Role::PlatformAdmin);
    if !ok {
        caller
            .audit_denied(
                what,
                &EntityRef::new(EntityKind::Workspace, ws),
                Some(ws),
                "the credential vault is for platform administrators",
            )
            .await;
        return Err(ApiError::forbidden("the credential vault is for platform administrators"));
    }

    Ok(())
}

fn bad(m: impl Into<String>) -> ApiError {
    ApiError::bad_request(m)
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct ItemInput {
    pub name: String,
    #[serde(default)]
    pub username: String,
    /// Required on create; on update, present = rotate.
    #[serde(default)]
    pub password: Option<String>,
    #[serde(default)]
    pub url: String,
    #[serde(default)]
    pub notes: String,
    #[serde(default)]
    pub tags: Vec<String>,
    #[serde(default)]
    #[schemars(with = "Vec<serde_json::Value>")]
    pub links: Vec<VaultLink>,
}

/// Names of the nodes, services and clusters of a workspace, for validating
/// and displaying links.
async fn link_targets(state: &AppState, ws: WorkspaceId) -> ApiResult<HashMap<(String, String), String>> {
    let mut m = HashMap::new();

    for s in state.store.list_servers(Some(ws)).await? {
        m.insert(("node".into(), s.id.to_string()), s.display_name.clone().unwrap_or(s.hostname.clone()));
    }

    for c in state.store.list_clusters(ws).await? {
        m.insert(("cluster".into(), c.id.to_string()), c.name.clone());
    }

    for ns in state.store.list_environments(ws, None).await? {
        for a in state.store.list_services(Some(ns.id)).await? {
            m.insert(("service".into(), a.id.to_string()), format!("{}.{}", a.name, ns.name));
        }
    }

    Ok(m)
}

fn clean(
    req: &ItemInput,
    targets: &HashMap<(String, String), String>,
) -> ApiResult<(String, Vec<String>, Vec<VaultLink>)> {
    let name = req.name.trim().to_owned();
    if name.is_empty() || name.chars().count() > 100 {
        return Err(bad("name: 1 to 100 characters"));
    }

    if req.username.chars().count() > 200 || req.notes.chars().count() > 4000 {
        return Err(bad("username or notes too long"));
    }

    let url = req.url.trim();
    let lower = url.to_ascii_lowercase();

    if url.len() > 500
        || !(url.is_empty() || ["https://", "http://", "ssh://", "rdp://"].iter().any(|p| lower.starts_with(p)))
    {
        return Err(bad("url: http(s)://, ssh:// or rdp://"));
    }

    let mut tags: Vec<String> = Vec::new();

    for t in &req.tags {
        let t = t.trim().to_lowercase();
        if t.is_empty() || t.len() > 40 || !t.chars().all(|c| c.is_alphanumeric() || "-_.:".contains(c)) {
            return Err(bad(format!("tag {t:?}: letters, digits and - _ . : (up to 40)")));
        }

        if !tags.contains(&t) {
            tags.push(t);
        }
    }

    if tags.len() > 20 || req.links.len() > 20 {
        return Err(bad("at most 20 tags and 20 links"));
    }

    let mut links = Vec::new();

    for l in &req.links {
        if !targets.contains_key(&(l.kind.clone(), l.id.clone())) {
            return Err(bad(format!("unknown {} {}", l.kind, l.id)));
        }

        if !links.contains(l) {
            links.push(l.clone());
        }
    }

    Ok((name, tags, links))
}

fn view(i: &VaultItem, targets: &HashMap<(String, String), String>) -> Value {
    let links: Vec<Value> = i
        .links
        .iter()
        .map(|l| json!({ "kind": l.kind, "id": l.id, "name": targets.get(&(l.kind.clone(), l.id.clone())).cloned().unwrap_or_else(|| "(removed)".into()) }))
        .collect();
    json!({
        "id": i.id,
        "name": i.name,
        "username": i.username,
        "url": i.url,
        "notes": i.notes,
        "tags": i.tags,
        "links": links,
        "created_by": i.created_by,
        "created_at": i.created_at,
        "updated_at": i.updated_at,
        "rotated_at": i.rotated_at,
    })
}

async fn load(state: &AppState, ws: WorkspaceId, id: VaultItemId) -> ApiResult<VaultItem> {
    match state.store.get_vault_item(id).await? {
        Some(i) if i.workspace_id == ws => Ok(i),
        _ => Err(ApiError::not_found("credential not found")),
    }
}

/// The password of item `id`, for another feature that already checked
/// [`authorize_item_use`] (e.g. a Swarm secret made from it, D-171).
pub(super) async fn open_item(state: &AppState, ws: WorkspaceId, id: VaultItemId) -> ApiResult<Vec<u8>> {
    let item = load(state, ws, id).await?;
    Ok(state.envelope.open(&item.sealed, &aad(id)).map_err(DomainError::internal)?)
}

fn seal(state: &AppState, id: VaultItemId, password: &str) -> ApiResult<String> {
    if password.is_empty() || password.len() > MAX_PASSWORD {
        return Err(bad("password: 1 to 4096 bytes"));
    }

    Ok(state.envelope.seal(password.as_bytes(), &aad(id)).map_err(DomainError::internal)?)
}

pub async fn list(State(state): State<AppState>, caller: Caller, Ws(ws): Ws) -> ApiResult<Json<Vec<Value>>> {
    require_platform_admin(&state, &caller, ws, "vault.list").await?;
    let targets = link_targets(&state, ws).await?;

    Ok(Json(state.store.list_vault_items(ws).await?.iter().map(|i| view(i, &targets)).collect()))
}

pub async fn create(
    State(state): State<AppState>,
    caller: Caller,
    Ws(ws): Ws,
    Json(req): Json<ItemInput>,
) -> ApiResult<(StatusCode, Json<Value>)> {
    require_platform_admin(&state, &caller, ws, "vault.create").await?;

    // Through Cedar too, so the MFA guardrail and connector limits apply.
    caller.authorize(Action::SecretManage, EntityRef::platform()).await?;
    let targets = link_targets(&state, ws).await?;
    let (name, tags, links) = clean(&req, &targets)?;
    let id = VaultItemId::new();
    let sealed = seal(&state, id, req.password.as_deref().unwrap_or(""))?;
    let now = state.clock.now();
    let item = VaultItem {
        id,
        workspace_id: ws,
        name,
        username: req.username.trim().to_owned(),
        url: req.url.trim().to_owned(),
        notes: req.notes,
        tags,
        links,
        sealed,
        created_by: caller.principal(),
        created_at: now,
        updated_at: now,
        rotated_at: now,
    };
    state.store.put_vault_item(&item).await?;
    caller
        .audit(
            "vault.create",
            &EntityRef::new(EntityKind::Workspace, ws),
            Some(ws),
            json!({ "item": id, "name": item.name, "tags": item.tags, "links": item.links }),
        )
        .await;

    Ok((StatusCode::CREATED, Json(view(&item, &targets))))
}

pub async fn update(
    State(state): State<AppState>,
    caller: Caller,
    Ws(ws): Ws,
    Path(id): Path<VaultItemId>,
    Json(req): Json<ItemInput>,
) -> ApiResult<Json<Value>> {
    require_platform_admin(&state, &caller, ws, "vault.update").await?;

    // Through Cedar too, so the MFA guardrail and connector limits apply.
    caller.authorize(Action::SecretManage, EntityRef::platform()).await?;
    let mut item = load(&state, ws, id).await?;
    let targets = link_targets(&state, ws).await?;
    let (name, tags, links) = clean(&req, &targets)?;
    let now = state.clock.now();
    let rotated = req.password.as_deref().is_some_and(|p| !p.is_empty());

    if let Some(p) = req.password.as_deref().filter(|p| !p.is_empty()) {
        item.sealed = seal(&state, id, p)?;
        item.rotated_at = now;
    }

    item.name = name;
    item.username = req.username.trim().to_owned();
    item.url = req.url.trim().to_owned();
    item.notes = req.notes;
    item.tags = tags;
    item.links = links;
    item.updated_at = now;
    state.store.put_vault_item(&item).await?;
    caller
        .audit(
            "vault.update",
            &EntityRef::new(EntityKind::Workspace, ws),
            Some(ws),
            json!({ "item": id, "name": item.name, "rotated": rotated }),
        )
        .await;

    Ok(Json(view(&item, &targets)))
}

pub async fn delete(
    State(state): State<AppState>,
    caller: Caller,
    Ws(ws): Ws,
    Path(id): Path<VaultItemId>,
) -> ApiResult<StatusCode> {
    require_platform_admin(&state, &caller, ws, "vault.delete").await?;

    // Through Cedar too, so the MFA guardrail and connector limits apply.
    caller.authorize(Action::SecretManage, EntityRef::platform()).await?;
    let item = load(&state, ws, id).await?;
    state.store.delete_vault_item(id).await?;
    caller
        .audit(
            "vault.delete",
            &EntityRef::new(EntityKind::Workspace, ws),
            Some(ws),
            json!({ "item": id, "name": item.name }),
        )
        .await;

    Ok(StatusCode::NO_CONTENT)
}

/// Returns the password once. Platform admin + `secret.read` (MFA) + a session.
/// Checks that `caller` may use vault item `id` of workspace `ws` from
/// another feature (e.g. a health-check header, D-085): the same rules as
/// revealing it (platform administrators, no API keys, `secret.read` with
/// its MFA guardrail), and the item must exist. Denials are audited as
/// `what`.
pub(super) async fn authorize_item_use(
    state: &AppState,
    caller: &Caller,
    ws: WorkspaceId,
    id: VaultItemId,
    what: &str,
) -> ApiResult<()> {
    require_platform_admin(state, caller, ws, what).await?;
    let target = EntityRef::new(EntityKind::Workspace, ws);

    if caller.via_api_key() {
        caller.audit_denied(what, &target, Some(ws), "vault items cannot be used with an API key").await;
        return Err(ApiError::forbidden("vault items cannot be used with an API key"));
    }

    caller.authorize(Action::SecretRead, target).await?;
    load(state, ws, id).await?;

    Ok(())
}

pub async fn reveal(
    State(state): State<AppState>,
    caller: Caller,
    Ws(ws): Ws,
    Path(id): Path<VaultItemId>,
) -> ApiResult<Json<Value>> {
    require_platform_admin(&state, &caller, ws, "vault.reveal").await?;
    let target = EntityRef::new(EntityKind::Workspace, ws);

    if caller.via_api_key() {
        caller.audit_denied("vault.reveal", &target, Some(ws), "passwords cannot be revealed with an API key").await;
        return Err(ApiError::forbidden("passwords cannot be revealed with an API key"));
    }

    caller.authorize(Action::SecretRead, target.clone()).await?;
    let item = load(&state, ws, id).await?;
    let plain = state.envelope.open(&item.sealed, &aad(id)).map_err(DomainError::internal)?;
    let password = String::from_utf8(plain).map_err(|_| ApiError::conflict("stored password is not text"))?;
    caller.audit("vault.reveal", &target, Some(ws), json!({ "item": id, "name": item.name })).await;

    Ok(Json(json!({ "password": password })))
}
