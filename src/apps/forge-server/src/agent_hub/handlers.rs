use std::time::Duration;

use axum::body::Bytes;
use axum::extract::{Extension, State};
use axum::http::{StatusCode, header};
use axum::response::{IntoResponse, Response};
use forge_core::model::{AgentStatus, AuditEvent, AuditOutcome, NodeRole, Server};
use forge_core::ports::{EnrollmentRepository, ServerRepository};
use forge_core::{EntityKind, EntityRef, ServerId};
use forge_proto::messages::{EnrollRequest, EnrollResponse, Heartbeat, HeartbeatResponse, RenewRequest, RenewResponse};
use prost::Message;

use super::{AgentPeer, PeerAddr};
use crate::events::ServerEvent;
use crate::state::AppState;

const MAX_COMMANDS_PER_RESPONSE: usize = 16;

fn proto(msg: &impl Message) -> Response {
    ([(header::CONTENT_TYPE, forge_proto::CONTENT_TYPE)], msg.encode_to_vec()).into_response()
}

fn reject(status: StatusCode, msg: &'static str) -> Response {
    (status, msg).into_response()
}

pub(super) async fn ca(State(state): State<AppState>) -> Response {
    ([(header::CONTENT_TYPE, "application/x-pem-file")], state.ca.cert_pem().to_owned()).into_response()
}

/// Enrollment attempts per client address per minute.
const ENROLL_PER_MINUTE: usize = 20;

pub(super) async fn enroll(
    State(state): State<AppState>,
    Extension(PeerAddr(addr)): Extension<PeerAddr>,
    body: Bytes,
) -> Response {
    // Before any lookup or audit row: an address gets a few tries a minute.
    if !state.enroll_attempts.allow(
        &crate::rate_limit::client_key(addr.ip()),
        ENROLL_PER_MINUTE,
        std::time::Duration::from_secs(60),
    ) {
        return reject(StatusCode::TOO_MANY_REQUESTS, "too many enrollment attempts; try again in a minute");
    }

    let Ok(req) = EnrollRequest::decode(body) else { return reject(StatusCode::BAD_REQUEST, "malformed request") };
    if req.hostname.is_empty() || req.hostname.len() > 253 || req.os.len() > 32 || req.arch.len() > 32 {
        return reject(StatusCode::BAD_REQUEST, "invalid host facts");
    }

    let now = state.clock.now();
    let server_id = ServerId::new();
    let hash = forge_pki::enrollment::hash_secret(&req.token_secret);
    let token = match state.store.consume_token(&hash, now, server_id).await {
        Ok(Some(t)) => t,
        Ok(None) => {
            audit(
                &state,
                "agent",
                "server.enroll",
                "server:unknown",
                AuditOutcome::Denied,
                None,
                serde_json::json!({"hostname": req.hostname}),
            )
            .await;
            return reject(StatusCode::FORBIDDEN, "invalid, expired or used enrollment token");
        }
        Err(e) => {
            tracing::error!(error = %e, "token lookup failed");
            return reject(StatusCode::INTERNAL_SERVER_ERROR, "internal error");
        }
    };

    let issued = match state.ca.issue_agent_certificate(
        &req.csr_pem,
        &server_id.to_string(),
        now,
        forge_pki::AGENT_CERT_VALIDITY,
    ) {
        Ok(c) => c,
        Err(e) => {
            tracing::warn!(error = %e, "CSR rejected");
            return reject(StatusCode::BAD_REQUEST, "invalid CSR");
        }
    };

    // The agent running on the Forge host itself becomes the control node
    // (D-016). Loopback cannot be spoofed by remote agents.
    let role =
        if addr.ip().is_loopback() && !has_control_node(&state).await { NodeRole::Control } else { NodeRole::Worker };

    // The cluster chosen when the token was created (manual installs).
    let cluster_id = match state.store.get_enrollment_target(&token.id).await {
        Ok(Some((cluster, _))) => cluster,
        _ => None,
    };

    let server = Server {
        id: server_id,
        role,
        display_name: None,
        workspace_id: token.workspace_id,
        environment_id: token.environment_id,
        cluster_id,
        hostname: req.hostname.clone(),
        os: req.os,
        arch: req.arch,
        agent_version: req.agent_version,
        status: AgentStatus::Pending,
        last_seen_at: None,
        cert_serial: Some(issued.serial_hex.clone()),
        cert_not_after: Some(issued.not_after),
        created_at: now,
    };

    if let Err(e) = state.store.create_server(&server).await {
        tracing::error!(error = %e, "could not register server");
        return reject(StatusCode::INTERNAL_SERVER_ERROR, "internal error");
    }

    // Give the new member a fabric address in its cluster.
    if let Some(c) = cluster_id
        && let Err(e) = crate::api::fabric::reallocate(&state, c).await
    {
        tracing::warn!(error = ?e, cluster = %c, "could not plan the fabric address of a new node");
    }

    // Existing members add the new node as soon as it reports its key (#24).
    if let Some(c) = cluster_id {
        crate::api::fabric_reconcile::membership_changed(&state, &[c]).await;
    }

    audit(
        &state,
        &token.created_by.to_string(),
        "server.enroll",
        &EntityRef::new(EntityKind::Server, server_id).to_string(),
        AuditOutcome::Success,
        Some(token.workspace_id),
        serde_json::json!({ "hostname": req.hostname, "token_id": token.id, "cert_serial": issued.serial_hex }),
    )
    .await;

    // The terminal account the installer created (D-078) is saved for the
    // node's terminal, as the person who created the token.
    let account = req.terminal_account.trim();
    if !account.is_empty()
        && let Ok(Some(before)) =
            crate::api::logins::save_login(&state.store, server_id, account, state.clock.now()).await
    {
        let mut after = before.clone();
        after.push(account.to_owned());
        audit(
            &state,
            &token.created_by.to_string(),
            "server.logins.update",
            &EntityRef::new(EntityKind::Server, server_id).to_string(),
            AuditOutcome::Success,
            Some(token.workspace_id),
            serde_json::json!({ "from": before, "to": after, "source": "installer" }),
        )
        .await;
    }

    state.publish(ServerEvent::server(server_id, AgentStatus::Pending));

    proto(&EnrollResponse {
        server_id: server_id.to_string(),
        cert_pem: issued.cert_pem,
        ca_pem: state.ca.cert_pem().to_owned(),
        command_public_key: state.signer.public_key(),
        heartbeat_interval_secs: state.config.heartbeat_hold_secs,
        analytics_url: if state.analytics_enabled { state.config.worker_agent_url() } else { String::new() },
        server_keys: Some(state.agent_keys.announcement(&server_id.to_string(), now)),
        analytics_port: analytics_port(&state),
    })
}

/// Whether the certificate `serial` (as presented) is revoked. Serials are
/// revoked in DER form since D-145; one revoked before may be recorded
/// with leading zero bytes, so those forms are looked up too.
async fn is_revoked(state: &AppState, serial: &str) -> forge_core::DomainResult<bool> {
    let mut s = forge_pki::canonical_serial(serial);

    loop {
        if state.store.is_certificate_revoked(&s).await? {
            return Ok(true);
        }

        if s.len() >= 32 {
            return Ok(false);
        }

        s.insert_str(0, "00");
    }
}

/// Validates the client certificate and returns the server it belongs to.
pub(crate) async fn authenticated_server(state: &AppState, peer: &AgentPeer) -> Result<Server, Box<Response>> {
    let Some(id) = &peer.0 else {
        return Err(Box::new(reject(StatusCode::UNAUTHORIZED, "client certificate required")));
    };

    let Ok(server_id) = id.common_name.parse::<ServerId>() else {
        return Err(Box::new(reject(StatusCode::FORBIDDEN, "not an agent certificate")));
    };

    match is_revoked(state, &id.serial_hex).await {
        Ok(false) => {}
        Ok(true) => return Err(Box::new(reject(StatusCode::FORBIDDEN, "certificate revoked"))),
        Err(_) => return Err(Box::new(reject(StatusCode::INTERNAL_SERVER_ERROR, "internal error"))),
    }

    let server = state
        .store
        .get_server(server_id)
        .await
        .map_err(|_| Box::new(reject(StatusCode::FORBIDDEN, "unknown agent")))?;

    if server.status == AgentStatus::Revoked {
        return Err(Box::new(reject(StatusCode::FORBIDDEN, "agent revoked")));
    }

    Ok(server)
}

pub(super) async fn heartbeat(
    State(state): State<AppState>,
    Extension(peer): Extension<AgentPeer>,
    Extension(peer_addr): Extension<super::PeerAddr>,
    body: Bytes,
) -> Response {
    let server = match authenticated_server(&state, &peer).await {
        Ok(s) => s,
        Err(r) => return *r,
    };

    let Ok(hb) = Heartbeat::decode(body) else { return reject(StatusCode::BAD_REQUEST, "malformed heartbeat") };
    let now = state.clock.now();
    if let Err(e) = state.store.record_heartbeat(server.id, now, &hb.agent_version).await {
        tracing::error!(error = %e, "heartbeat write failed");
    }

    if server.status != AgentStatus::Online {
        state.publish(ServerEvent::server(server.id, AgentStatus::Online));
    }

    crate::facts::note_address(server.id, peer_addr.0.ip());

    // The control-plane keys this agent pins: its acknowledgement of the
    // next key (D-167). Never fails the heartbeat.
    if let Err(e) = state.agent_keys.record_ack(&server.id.to_string(), &hb.pinned_server_keys, now).await {
        tracing::warn!(server = %server.id, error = %e, "recording the agent's pinned keys failed");
    }

    // OS, kernel and CPU facts: collected on connect, daily and right after
    // the agent changes version (facts.rs).
    crate::facts::maybe_collect(&state, server.id, server.agent_version != hb.agent_version);

    // Compose containers that changed since the last push: stored rows and
    // names follow them between deploys (D-172).
    if let Some(st) = &hb.stack_states {
        let (state, id, json) = (state.clone(), server.id, st.states_json.clone());
        tokio::spawn(async move { crate::stack_states::ingest(&state, id, &json).await });
    }

    // The fleet's names (D-084): on change, on reconnect and every few minutes.
    crate::names::on_heartbeat(&state, server.id, server.status != AgentStatus::Online);

    if let Some(m) = &hb.metrics {
        state.telemetry.record(server.id, crate::metrics::Sample::from_wire(now, m));
        state.telemetry.set_volumes(server.id, crate::metrics::volumes_from_wire(m));
        state.telemetry.set_traffic(server.id, now.0, m.traffic.clone());

        // CPU, memory and root disk as observations for alert rules (#11, D-085
        // amendment). In the background: a heartbeat stays cheap, and a
        // recorder error never fails it.
        if crate::measures::due(server.id, now) {
            let fresh_ms = (u64::from(state.config.heartbeat_hold_secs) * 3_000).clamp(1_000, 3_600_000);
            let obs = crate::measures::from_metrics(server.id, m, now, fresh_ms);
            let (health, id) = (state.health.clone(), server.id);
            tokio::spawn(async move {
                if let Err(e) = health.record_from_agent(id, obs).await {
                    tracing::warn!(server = %id, error = %e, "recording node measurements failed");
                }
            });
        }
    }

    // Agents with a spool upload node history and container data to the
    // analytics worker themselves (D-142). Older agents still send them
    // here; they are forwarded to the worker as received.
    let container_logs_ack = if hb.spool.is_some() { 0 } else { forward_telemetry(&state, server.id, now, &hb).await };

    // Health check results of the apps on this node (D-085).
    crate::health_ingest::ingest(&state, server.id, hb.health);

    for p in &hb.progress {
        state.hub.append_progress(server.id, &p.operation_id, &p.text);
    }

    for r in hb.results {
        let op = r.operation_id.clone();

        if !state.hub.resolve(server.id, r) {
            tracing::debug!(server = %server.id, operation = %op, "result for an operation not sent to this node dropped");
        }
    }

    // An agent that restarted after taking commands will never answer them.
    let lost = state.hub.agent_uptime(server.id, Duration::from_secs(hb.uptime_secs));
    if !lost.is_empty() {
        tracing::warn!(server = %server.id, operations = ?lost, "the agent restarted after taking these commands; their jobs retry");
    }

    // Long-poll: hold until a command arrives or the hold time elapses.
    let mut commands = state.hub.take(server.id, MAX_COMMANDS_PER_RESPONSE);
    if commands.is_empty() {
        let notify = state.hub.notifier(server.id);
        let hold = Duration::from_secs(u64::from(state.config.heartbeat_hold_secs));

        // Someone is watching this node's live charts: answer sooner.
        let hold = if state.telemetry.is_watched(server.id) { hold.min(crate::metrics::LIVE_HOLD) } else { hold };

        // A command is streaming output: come back within a second.
        let hold = if hb.streaming { hold.min(Duration::from_secs(1)) } else { hold };
        let _ = tokio::time::timeout(hold, notify.notified()).await;
        commands = state.hub.take(server.id, MAX_COMMANDS_PER_RESPONSE);
    }

    let renew = peer.0.as_ref().is_some_and(|p| forge_pki::renewal_due(p.not_before, p.not_after, now));
    let container_telemetry = Some(state.container_telemetry.config_for(server.id).await);

    proto(&HeartbeatResponse {
        commands,
        next_interval_secs: 0,
        renew_certificate: renew,
        container_telemetry,
        container_logs_ack,
        analytics: analytics_config(&state, server.id),
        server_keys: Some(state.agent_keys.announcement(&server.id.to_string(), state.clock.now())),
    })
}

/// What the node needs for its spool and log listener (D-142); `None`
/// while analytics is off.
fn analytics_config(state: &AppState, server: ServerId) -> Option<forge_proto::messages::AnalyticsAgentConfig> {
    if !state.analytics_enabled {
        return None;
    }

    Some(forge_proto::messages::AnalyticsAgentConfig {
        url: state.config.worker_agent_url(),
        spool: Some(state.config.analytics.spool.wire()),
        keys: state.analytics_maps.keys_for(server).as_ref().clone(),
        listener: Some(state.config.analytics.listener.wire()),
        port: analytics_port(state),
    })
}

/// The worker's upload port for agents that dial it on their own server
/// URL's host (D-167); 0 when analytics is off or the URL is configured.
fn analytics_port(state: &AppState) -> u32 {
    if !state.analytics_enabled {
        return 0;
    }

    state.config.worker_agent_port().map_or(0, u32::from)
}

/// Forwards a heartbeat's metrics and container lines to the analytics
/// worker (D-141), re-encoded as the node sent them; forge-server keeps
/// none of it. Returns the container batch `seq` to acknowledge: the
/// batch's once stored, else 0 so the node sends it again. Never fails the
/// heartbeat; the worker answers within its own deadline.
async fn forward_telemetry(state: &AppState, server: ServerId, now: forge_core::Timestamp, hb: &Heartbeat) -> u64 {
    if !state.analytics_enabled {
        return 0;
    }

    let metrics = hb.metrics.as_ref().map(Message::encode_to_vec).unwrap_or_default();
    let logs = hb.container_logs.as_ref().map(Message::encode_to_vec).unwrap_or_default();
    if metrics.is_empty() && logs.is_empty() {
        return 0;
    }

    let t = forge_core::analytics::NodeTelemetry { server, now, metrics: &metrics, container_logs: &logs };
    match state.telemetry_sink.forward_telemetry(t).await {
        Ok(ack) => ack,
        Err(e) => {
            tracing::debug!(server = %server, error = %e, "heartbeat telemetry not stored; the node sends it again");
            0
        }
    }
}

/// Vault-backed header values of one HTTP health check, for the node that
/// is about to run it (D-111). The node names the check; what it gets, and
/// whether, is decided by `HealthSecrets::resolve`, which also audits.
pub(super) async fn health_secrets(
    State(state): State<AppState>,
    Extension(peer): Extension<AgentPeer>,
    body: Bytes,
) -> Response {
    let server = match authenticated_server(&state, &peer).await {
        Ok(s) => s,
        Err(r) => return *r,
    };

    let Ok(req) = forge_proto::messages::HealthSecretsRequest::decode(body) else {
        return reject(StatusCode::BAD_REQUEST, "malformed request");
    };

    let mut resp = proto(&state.health_secrets.resolve(server.id, &req).await);
    resp.headers_mut().insert(header::CACHE_CONTROL, axum::http::HeaderValue::from_static("no-store"));

    resp
}

pub(super) async fn renew(
    State(state): State<AppState>,
    Extension(peer): Extension<AgentPeer>,
    body: Bytes,
) -> Response {
    let server = match authenticated_server(&state, &peer).await {
        Ok(s) => s,
        Err(r) => return *r,
    };

    let Ok(req) = RenewRequest::decode(body) else { return reject(StatusCode::BAD_REQUEST, "malformed request") };
    let now = state.clock.now();

    // Only the node's current certificate renews, and only once renewal is
    // due: an older (or copied) certificate of the node cannot mint new ones.
    let Some(presented) = peer.0.as_ref() else {
        return reject(StatusCode::UNAUTHORIZED, "client certificate required");
    };

    // Compared in DER form: a serial recorded before D-145 may carry a
    // leading zero byte the presented certificate does not.
    let current = server
        .cert_serial
        .as_deref()
        .is_some_and(|s| forge_pki::canonical_serial(s) == forge_pki::canonical_serial(&presented.serial_hex));
    let due = forge_pki::renewal_due(presented.not_before, presented.not_after, now);
    if !current || !due {
        let reason = if current { "renewal not due yet" } else { "not the node's current certificate" };
        audit(
            &state,
            &format!("agent:{}", server.id),
            "server.cert.renew",
            &EntityRef::new(EntityKind::Server, server.id).to_string(),
            AuditOutcome::Denied,
            Some(server.workspace_id),
            serde_json::json!({ "serial": presented.serial_hex, "reason": reason }),
        )
        .await;

        return reject(StatusCode::FORBIDDEN, reason);
    }

    let previous = presented.serial_hex.clone();

    let issued = match state.ca.issue_agent_certificate(
        &req.csr_pem,
        &server.id.to_string(),
        now,
        forge_pki::AGENT_CERT_VALIDITY,
    ) {
        Ok(c) => c,
        Err(_) => return reject(StatusCode::BAD_REQUEST, "invalid CSR"),
    };

    if let Err(e) = state.store.set_certificate(server.id, &issued.serial_hex, issued.not_after).await {
        tracing::error!(error = %e, "could not store renewed certificate");
        return reject(StatusCode::INTERNAL_SERVER_ERROR, "internal error");
    }

    // The replaced certificate stops working now rather than at its expiry.
    let revoked = match state.store.revoke_certificate(&forge_pki::canonical_serial(&previous), server.id, now).await {
        Ok(()) => true,
        Err(e) => {
            tracing::error!(error = %e, "could not revoke the replaced agent certificate");
            false
        }
    };

    // The analytics worker learns the new serial now, not at the next
    // periodic push, so uploads with the new certificate are not refused.
    state.analytics_maps.nudge();
    audit(
        &state,
        &format!("agent:{}", server.id),
        "server.cert.renew",
        &EntityRef::new(EntityKind::Server, server.id).to_string(),
        AuditOutcome::Success,
        Some(server.workspace_id),
        serde_json::json!({ "serial": issued.serial_hex, "previous": previous, "previous_revoked": revoked }),
    )
    .await;

    proto(&RenewResponse { cert_pem: issued.cert_pem })
}

async fn audit(
    state: &AppState,
    actor: &str,
    action: &str,
    resource: &str,
    outcome: AuditOutcome,
    org: Option<forge_core::WorkspaceId>,
    details: serde_json::Value,
) {
    let ev = AuditEvent::new(actor, action, resource, outcome).in_org(org).with_details(details);
    if let Err(e) = state.audit.append(ev).await {
        tracing::error!(error = %e, "audit write failed");
    }
}

async fn has_control_node(state: &AppState) -> bool {
    state.store.list_servers(None).await.map(|all| all.iter().any(|s| s.role == NodeRole::Control)).unwrap_or(true)
}
