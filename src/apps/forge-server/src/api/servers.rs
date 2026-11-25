//! Servers, enrollment tokens, revocation and agent commands.

use std::time::Duration;

use axum::Json;
use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use forge_core::model::{AgentStatus, EnrollmentToken, Job, NewJob, Server};
use forge_core::ports::{ClusterRepository, EnrollmentRepository, ServerRepository, TenancyRepository};
use forge_core::*;
use serde::Deserialize;
use serde_json::{Value, json};

use crate::auth::Caller;
use crate::auth::Ws;
use crate::error::{ApiError, ApiResult};
use crate::events::ServerEvent;
use crate::jobs::agent_command::parse_command;
use crate::state::AppState;

fn server_ref(id: ServerId) -> EntityRef {
    EntityRef::new(EntityKind::Server, id)
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct ServerQuery {
    #[serde(default)]
    #[schemars(with = "Option<String>")]
    pub workspace: Option<WorkspaceId>,
}

/// Nodes you can see, each with `facts` (OS, kernel, CPUs, RAM, disk) and
/// `tags`. `?tag=dc:fra1` (repeatable, or comma-separated) keeps nodes carrying every tag.
pub async fn list(
    State(state): State<AppState>,
    caller: Caller,
    Query(q): Query<ServerQuery>,
    axum::extract::RawQuery(raw): axum::extract::RawQuery,
) -> ApiResult<Json<Vec<serde_json::Value>>> {
    use forge_core::tags::{FactsRepository, TagKind, TagRepository, has_all};
    let wanted: Vec<String> = url::form_urlencoded::parse(raw.unwrap_or_default().as_bytes())
        .filter(|(k, _)| k == "tag")
        // `?tag=a,b` and `?tag=a&tag=b` mean the same (tags never contain commas).
        .flat_map(|(_, v)| v.split(',').map(str::trim).filter(|t| !t.is_empty()).map(str::to_owned).collect::<Vec<_>>())
        .collect();
    let all = state.store.list_servers(q.workspace).await?;
    let allowed = caller.can_many(Action::View, all.iter().map(|s| server_ref(s.id)).collect()).await;
    let facts = state.store.all_facts().await.unwrap_or_default();
    let tags = state.store.tags_of_kind(TagKind::Server).await.unwrap_or_default();
    let mut out = Vec::new();

    for (s, ok) in all.into_iter().zip(allowed) {
        let t = tags.get(&s.id.to_string()).cloned().unwrap_or_default();
        if !ok || !has_all(&t, &wanted) {
            continue;
        }

        let f = crate::facts::with_live(&state, s.id, facts.get(&s.id));
        let mut v = serde_json::to_value(&s).unwrap_or_default();
        v["facts"] = f;
        v["tags"] = serde_json::json!(t);
        out.push(v);
    }

    Ok(Json(out))
}

/// One node, with `facts` and `tags`.
pub async fn get(
    State(state): State<AppState>,
    caller: Caller,
    Path(id): Path<ServerId>,
) -> ApiResult<Json<serde_json::Value>> {
    use forge_core::tags::{FactsRepository, TagKind, TagRepository};
    caller.authorize(Action::View, server_ref(id)).await?;
    let s = state.store.get_server(id).await?;
    let facts = state.store.all_facts().await.unwrap_or_default();
    let mut v = serde_json::to_value(&s).unwrap_or_default();
    v["facts"] = crate::facts::with_live(&state, id, facts.get(&id));
    v["tags"] = serde_json::json!(state.store.tags_of(TagKind::Server, &id.to_string()).await.unwrap_or_default());
    v["dns"] = super::names::of_node(&state, &s).await;

    Ok(Json(v))
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct CreateToken {
    #[serde(default)]
    #[schemars(with = "Option<String>")]
    pub environment_id: Option<EnvironmentId>,
    /// Validity in minutes (1–1440, default 60).
    #[serde(default)]
    pub ttl_minutes: Option<u32>,
    /// Cluster the node joins when it enrolls with this token.
    #[serde(default)]
    #[schemars(with = "Option<String>")]
    pub cluster_id: Option<forge_core::ClusterId>,
    /// Expected host name, shown in the onboarding list until it enrolls.
    #[serde(default)]
    pub hostname: Option<String>,
}

pub async fn create_enrollment_token(
    State(state): State<AppState>,
    caller: Caller,
    origin: crate::request_origin::RequestOrigin,
    Ws(org): Ws,
    Json(req): Json<CreateToken>,
) -> ApiResult<(StatusCode, Json<Value>)> {
    let scope = match req.environment_id {
        Some(env) => {
            if state.store.get_environment(env).await?.workspace_id != org {
                return Err(ApiError::bad_request("environment belongs to another org"));
            }

            EntityRef::new(EntityKind::Environment, env)
        }
        None => EntityRef::new(EntityKind::Workspace, org),
    };

    caller.authorize(Action::ServerEnroll, scope.clone()).await?;
    let ttl = req.ttl_minutes.unwrap_or(60).clamp(1, 1440);
    let generated =
        forge_pki::enrollment::generate(state.ca.fingerprint(), Some(&state.agent_keys.current_fingerprint()));
    let token = EnrollmentToken {
        id: forge_secrets::crypto::random_token::<9>(),
        workspace_id: org,
        environment_id: req.environment_id,
        token_hash: generated.secret_hash,
        created_by: caller.principal(),
        expires_at: state.clock.now().plus(Duration::from_secs(u64::from(ttl) * 60)),
        used_at: None,
        server_id: None,
    };

    if let Some(c) = req.cluster_id {
        if state.store.get_cluster(c).await?.workspace_id != org {
            return Err(ApiError::bad_request("cluster belongs to another workspace"));
        }

        caller.authorize(Action::ServerEnroll, EntityRef::new(EntityKind::Cluster, c)).await?;
    }

    let hostname = req.hostname.as_deref().map(str::trim).filter(|h| !h.is_empty() && h.len() <= 253);
    state.store.create_token(&token).await?;

    if req.cluster_id.is_some() || hostname.is_some() {
        state.store.put_enrollment_target(&token.id, req.cluster_id, hostname, state.clock.now()).await?;
    }

    caller
        .audit(
            "server.enrollment_token.create",
            &scope,
            Some(org),
            json!({ "token_id": token.id, "ttl_minutes": ttl, "cluster_id": req.cluster_id }),
        )
        .await;

    // From the address this person is on (D-167).
    let urls = crate::request_origin::NodeUrls::from_state(&state, &origin);
    let (server, forge_url) = (&urls.agent_url, &urls.forge_url);

    Ok((
        StatusCode::CREATED,
        Json(json!({
            "id": token.id,
            "token": generated.display,
            "expires_at": token.expires_at,
            // The commands prompt for the token, so it never lands in argv,
            // shell history or the screen.
            "install": {
                "linux": format!("{LINUX_TOKEN_PROMPT}forge-agent enroll --server {server}; unset FORGE_ENROLL_TOKEN"),
                // install.ps1 downloads and verifies the agent, enrolls with the
                // token from the environment and installs the ForgeAgent service
                // (D-079); it removes the variable when it is done.
                "windows": format!("{WINDOWS_TOKEN_PROMPT}irm {forge_url}/install.ps1 | iex"),
            }
        })),
    ))
}

/// POSIX shell: reads the enrollment token without echo into the environment.
const LINUX_TOKEN_PROMPT: &str =
    "printf 'Enrollment token: '; stty -echo; read -r FORGE_ENROLL_TOKEN; stty echo; echo; export FORGE_ENROLL_TOKEN; ";
/// PowerShell 5.1 and 7: reads the token masked into the environment.
const WINDOWS_TOKEN_PROMPT: &str = "$t = Read-Host 'Enrollment token' -AsSecureString; $env:FORGE_ENROLL_TOKEN = [Net.NetworkCredential]::new('', $t).Password; Remove-Variable t; ";

/// Revoking cuts a node off for good, so it is confirmed twice: the exact
/// hostname, and a fresh authenticator code when the user has MFA enrolled.
#[derive(Debug, Default, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct RevokeRequest {
    #[serde(default)]
    pub confirm_hostname: String,
    #[serde(default)]
    pub totp_code: Option<String>,
}

pub async fn revoke(
    State(state): State<AppState>,
    caller: Caller,
    Path(id): Path<ServerId>,
    body: Option<Json<RevokeRequest>>,
) -> ApiResult<StatusCode> {
    caller.authorize(Action::ServerManage, server_ref(id)).await?;
    let server = state.store.get_server(id).await?;
    confirm(&state, &caller, &server, "server.revoke", body.map(|b| b.0).unwrap_or_default()).await?;

    if let Some(serial) = &server.cert_serial {
        state.store.revoke_certificate(&forge_pki::canonical_serial(serial), id, state.clock.now()).await?;
    }

    state.store.set_status(id, AgentStatus::Revoked).await?;
    state.publish(ServerEvent::server(id, AgentStatus::Revoked));

    // The analytics worker stops taking its uploads now (D-145).
    state.analytics_maps.nudge();

    // The other members drop the revoked node from their peers now (#24).
    if let Some(c) = server.cluster_id {
        super::fabric_reconcile::membership_changed(&state, &[c]).await;
    }

    caller
        .audit(
            "server.revoke",
            &server_ref(id),
            Some(server.workspace_id),
            json!({ "cert_serial": server.cert_serial }),
        )
        .await;

    Ok(StatusCode::NO_CONTENT)
}

/// `DELETE /servers/{id}`: removes a node for good. Its certificate is
/// revoked, then the node and everything recorded for it are deleted (see
/// `ServerRepository::delete_server`), so the machine can enroll again
/// under the same name as a fresh node. Confirmed like a revoke.
pub async fn remove(
    State(state): State<AppState>,
    caller: Caller,
    Path(id): Path<ServerId>,
    body: Option<Json<RevokeRequest>>,
) -> ApiResult<StatusCode> {
    caller.authorize(Action::ServerManage, server_ref(id)).await?;
    let server = state.store.get_server(id).await?;
    if server.role == forge_core::model::NodeRole::Control {
        return Err(ApiError::conflict("the control node cannot be removed"));
    }

    confirm(&state, &caller, &server, "server.remove", body.map(|b| b.0).unwrap_or_default()).await?;
    let now = state.clock.now();

    // Revoked first: the serial stays on the revocation list after the
    // node row is gone, so the old certificate can never reconnect.
    if let Some(serial) = &server.cert_serial {
        state.store.revoke_certificate(&forge_pki::canonical_serial(serial), id, now).await?;
    }

    state.store.delete_server(id, now).await?;
    state.publish(ServerEvent::server(id, AgentStatus::Revoked));
    state.analytics_maps.nudge();

    if let Some(c) = server.cluster_id {
        if let Err(e) = super::fabric::reallocate(&state, c).await {
            tracing::warn!(error = ?e, cluster = %c, "could not replan the fabric after removing a node");
        }

        super::fabric_reconcile::membership_changed(&state, &[c]).await;
    }

    crate::names::poke();
    caller
        .audit(
            "server.remove",
            &server_ref(id),
            Some(server.workspace_id),
            json!({
                "hostname": server.hostname,
                "display_name": server.display_name,
                "cluster_id": server.cluster_id,
                "cert_serial": server.cert_serial,
            }),
        )
        .await;

    Ok(StatusCode::NO_CONTENT)
}

/// Checks a revoke or remove confirmation: the exact hostname, and an
/// authenticator code when MFA is required. Failures are audited as `action`.
async fn confirm(
    state: &AppState,
    caller: &Caller,
    server: &forge_core::model::Server,
    action: &str,
    req: RevokeRequest,
) -> ApiResult<()> {
    let id = server.id;

    if req.confirm_hostname.trim() != server.hostname {
        caller.audit_denied(action, &server_ref(id), Some(server.workspace_id), "hostname confirmation mismatch").await;
        return Err(ApiError::bad_request("type the node's hostname exactly to confirm"));
    }

    if caller.session.user.mfa_enabled() && state.config.mfa_required() {
        let code = req.totp_code.as_deref().map(str::trim).unwrap_or_default();

        // Same throttle as step-up: an inline code is no side door for guessing.
        let key = format!("step-up:{}", caller.session.user.id);
        if !state.login_throttle.allowed(&key) {
            return Err(ApiError::new(
                axum::http::StatusCode::TOO_MANY_REQUESTS,
                "throttled",
                "too many attempts; try again in 15 minutes",
            ));
        }

        if code.is_empty() || state.identity.verify_step_up(&caller.session.user, code).is_err() {
            state.login_throttle.record_failure(&key);
            caller.audit_denied(action, &server_ref(id), Some(server.workspace_id), "invalid MFA code").await;

            return Err(ApiError::forbidden("a valid authenticator code is required to revoke or remove a node"));
        }

        state.login_throttle.reset(&key);
    }

    Ok(())
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct CommandRequest {
    /// `{"type": "ping"}`, `{"type": "collect_facts"}`, `{"type": "service_status", "name": "nginx"}`
    pub command: Value,
    #[serde(default)]
    pub idempotency_key: Option<String>,
}

pub async fn command(
    State(state): State<AppState>,
    caller: Caller,
    Path(id): Path<ServerId>,
    Json(req): Json<CommandRequest>,
) -> ApiResult<(StatusCode, Json<Job>)> {
    parse_command(&req.command).map_err(ApiError::bad_request)?;

    // Only applying an endpoint (POST /sites/{id}/apply) sends an uploaded
    // certificate's key, to that endpoint's edges.
    if req.command["type"] == "site" && req.command.pointer("/spec/tls").and_then(Value::as_str) == Some("uploaded") {
        return Err(ApiError::bad_request("uploaded certificates are sent by applying the endpoint, not as a command"));
    }

    // An endpoint's logs and cache are reached through the endpoint.
    if req.command["type"] == "site"
        && matches!(req.command["action"].as_str(), Some("access_log" | "error_log" | "clear_cache"))
    {
        return Err(ApiError::bad_request("use the endpoint's Logs and Clear cache instead of a command"));
    }

    let server = state.store.get_server(id).await.map_err(|_| ApiError::not_found("server not found"))?;

    // The job engine authorizes job.create and the command's action, and
    // re-authorizes before every step.
    let job = state
        .jobs
        .submit(NewJob {
            kind: "agent.command".into(),
            workspace_id: Some(server.workspace_id),
            resource: server_ref(id),
            principal: caller.principal(),
            context: caller.ctx(),
            idempotency_key: req.idempotency_key,
            payload: json!({ "server_id": id, "command": req.command }),
            max_attempts: 3,
        })
        .await?;

    Ok((StatusCode::ACCEPTED, Json(job)))
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct RenameRequest {
    /// New console name; empty restores the machine hostname.
    pub name: String,
}

/// `PUT /api/v1/servers/{id}/name`: renames a node in the console. The
/// machine's hostname is untouched.
pub async fn rename(
    State(state): State<AppState>,
    caller: Caller,
    Path(id): Path<ServerId>,
    Json(req): Json<RenameRequest>,
) -> ApiResult<Json<Server>> {
    caller.authorize(Action::ServerRename, server_ref(id)).await?;
    let server = state.store.get_server(id).await?;
    let name = req.name.trim();
    if name.chars().count() > 63 || name.chars().any(char::is_control) {
        return Err(ApiError::bad_request("node names are at most 63 characters, without control characters"));
    }

    let new = (!name.is_empty() && name != server.hostname).then_some(name);
    state.store.rename_server(id, new).await?;
    caller
        .audit(
            "server.rename",
            &server_ref(id),
            Some(server.workspace_id),
            json!({ "from": server.display_name, "to": new, "hostname": server.hostname }),
        )
        .await;

    // A node that has no local name yet takes one from this name (D-084).
    crate::names::poke();

    Ok(Json(state.store.get_server(id).await?))
}
