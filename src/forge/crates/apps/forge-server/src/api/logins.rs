//! SSH accounts offered for a node's terminal.
//!
//! `GET /servers/{id}/logins` (view) lists the saved accounts and the ones the
//! node reports in its facts (`login_accounts`, D-078), so the console can
//! offer a choice on **Connect terminal**; `PUT` (server.manage, audited)
//! replaces the saved ones. `POST /servers/{id}/terminal-account`
//! (server.command, MFA, audited) has the node's root helper create a
//! terminal account and saves it. Opening a terminal still authorizes the
//! chosen login as before (`terminal.open`, MFA); this list is a convenience,
//! not a grant.

use std::time::Duration;

use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::routing::{get, post};
use axum::{Json, Router};
use forge_core::ports::ServerRepository;
use forge_core::tags::FactsRepository;
use forge_core::tags::{NodeLoginRepository, valid_account_name, valid_login};
use forge_core::{Action, DomainResult, EntityKind, EntityRef, ServerId, Timestamp};
use forge_proto::Command;
use forge_proto::messages::TerminalAccount;
use serde::Deserialize;
use serde_json::{Value, json};

use crate::auth::Caller;
use crate::error::{ApiError, ApiResult};
use crate::state::AppState;

/// Most saved accounts per node.
const MAX_LOGINS: usize = 32;

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/servers/{id}/logins", get(list).put(set))
        .route("/servers/{id}/terminal-account", post(create_account))
}

fn server_ref(id: ServerId) -> EntityRef {
    EntityRef::new(EntityKind::Server, id)
}

/// Adds `login` to the end of a node's saved list. Returns the list before
/// the change, or `None` when nothing changed (already saved, invalid, or
/// the list is full).
pub async fn save_login(
    repo: &dyn NodeLoginRepository,
    server: ServerId,
    login: &str,
    now: Timestamp,
) -> DomainResult<Option<Vec<String>>> {
    if !valid_login(login) {
        return Ok(None);
    }

    let before = repo.logins_of(server).await?;
    if before.iter().any(|l| l == login) || before.len() >= MAX_LOGINS {
        return Ok(None);
    }

    let mut after = before.clone();
    after.push(login.to_owned());
    repo.set_logins(server, &after, now).await?;

    Ok(Some(before))
}

/// Accounts a node reported in its facts that are not saved yet.
fn discovered(facts: Option<&Value>, saved: &[String]) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();

    for l in facts.and_then(|f| f.get("login_accounts")).and_then(Value::as_array).into_iter().flatten() {
        if let Some(l) = l.as_str().filter(|l| valid_login(l))
            && !saved.iter().any(|s| s == l)
            && !out.iter().any(|o| o == l)
        {
            out.push(l.to_owned());
        }
    }

    out
}

/// `{saved, discovered, logins}`: `logins` is `saved`, kept for older
/// clients.
pub async fn list(State(state): State<AppState>, caller: Caller, Path(id): Path<ServerId>) -> ApiResult<Json<Value>> {
    caller.authorize(Action::View, server_ref(id)).await?;
    state.store.get_server(id).await.map_err(|_| ApiError::not_found("node not found"))?;
    let saved = state.store.logins_of(id).await?;
    let facts = state.store.all_facts().await?;
    let found = discovered(facts.get(&id).map(|(f, _)| f), &saved);

    Ok(Json(json!({ "logins": saved, "saved": saved, "discovered": found })))
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct SetLogins {
    pub logins: Vec<String>,
}

pub async fn set(
    State(state): State<AppState>,
    caller: Caller,
    Path(id): Path<ServerId>,
    Json(req): Json<SetLogins>,
) -> ApiResult<Json<Value>> {
    caller.authorize(Action::ServerManage, server_ref(id)).await?;
    state.store.get_server(id).await.map_err(|_| ApiError::not_found("node not found"))?;
    let mut logins: Vec<String> = Vec::new();

    for l in req.logins.iter().map(|l| l.trim()).filter(|l| !l.is_empty()) {
        if !valid_login(l) {
            return Err(ApiError::bad_request(format!(
                "`{l}` is not a valid login name (letters, digits, '_', '.', '-')"
            )));
        }

        if !logins.iter().any(|x| x == l) {
            logins.push(l.to_owned());
        }
    }

    if logins.len() > MAX_LOGINS {
        return Err(ApiError::bad_request(format!("at most {MAX_LOGINS} accounts per node")));
    }

    let before = state.store.logins_of(id).await?;
    state.store.set_logins(id, &logins, state.clock.now()).await?;
    caller.audit("server.logins.update", &server_ref(id), None, json!({ "from": before, "to": logins })).await;

    Ok(Json(json!({ "logins": logins })))
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct CreateAccount {
    /// Account name: 1–32 of `a-z`, `0-9`, `_`, `-`, starting with a letter
    /// or `_`; not a system name.
    pub name: String,
    /// Passwordless sudo through `/etc/sudoers.d/forge-<name>`.
    #[serde(default)]
    pub sudo: bool,
}

/// `POST /servers/{id}/terminal-account`: the node's root helper creates (or
/// keeps) the account, sets its sudo rule as asked, and Forge saves it to the
/// node's login list.
pub async fn create_account(
    State(state): State<AppState>,
    caller: Caller,
    Path(id): Path<ServerId>,
    Json(req): Json<CreateAccount>,
) -> ApiResult<Json<Value>> {
    caller.authorize(Action::ServerCommand, server_ref(id)).await?;
    let node = state.store.get_server(id).await.map_err(|_| ApiError::not_found("node not found"))?;
    let name = req.name.trim().to_owned();
    if !valid_account_name(&name) {
        return Err(ApiError::bad_request(format!(
            "`{name}` is not allowed: use 1–32 of a-z, 0-9, `_` and `-`, starting with a letter or `_`, and not a system name"
        )));
    }

    if node.os != "linux" {
        return Err(ApiError::conflict("terminal accounts need a Linux node"));
    }

    let (body, signed) = state.signer.issue(
        &id.to_string(),
        &caller.principal().to_string(),
        Command::TerminalAccount(TerminalAccount { name: name.clone(), sudo: req.sudo }),
        state.clock.now(),
        Duration::from_secs(120),
    );
    state.hub.enqueue(id, &body.operation_id, signed).map_err(|e| ApiError::conflict(e.to_string()))?;
    let r = state
        .hub
        .wait(&body.operation_id, Duration::from_secs(60))
        .await
        .ok_or_else(|| ApiError::new(StatusCode::GATEWAY_TIMEOUT, "timeout", "the node did not answer in time"))?;
    let target = server_ref(id);
    let details = json!({ "name": name, "sudo": req.sudo });

    if !r.ok {
        caller
            .audit_failed("server.terminal_account.create", &target, Some(node.workspace_id), details, &r.error)
            .await;
        return Err(ApiError::conflict(format!("the node refused: {}", r.error)));
    }

    let created = serde_json::from_str::<Value>(&r.output).ok().and_then(|v| v["created"].as_bool()).unwrap_or(false);
    caller
        .audit(
            "server.terminal_account.create",
            &target,
            Some(node.workspace_id),
            json!({ "name": name, "sudo": req.sudo, "created": created }),
        )
        .await;

    if let Some(before) = save_login(&state.store, id, &name, state.clock.now()).await? {
        let mut after = before.clone();
        after.push(name.clone());
        caller
            .audit("server.logins.update", &target, Some(node.workspace_id), json!({ "from": before, "to": after }))
            .await;
    }

    Ok(Json(json!({ "name": name, "sudo": req.sudo, "created": created, "logins": state.store.logins_of(id).await? })))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn discovered_skips_saved_invalid_and_duplicate_accounts() {
        let facts = json!({ "login_accounts": ["root", "alice", "-x", "alice", "bob", 7] });
        assert_eq!(discovered(Some(&facts), &["alice".to_owned()]), ["root", "bob"]);
        assert!(discovered(None, &[]).is_empty());
        assert!(discovered(Some(&json!({})), &[]).is_empty());
    }
}
