//! Device sign-in (RFC 8628) for the CLI on machines without a browser:
//! `forge auth login --device` gets a short code, a person signed in to the
//! console approves it at `/device`, and the CLI polls the token endpoint.
//! Requests live in memory for ten minutes; nothing is issued without the
//! person's approval in their own browser session. The console shows where a
//! request came from (address, client), so a person can tell a code someone
//! sent them (phishing) from their own; issued tokens never carry MFA — the
//! CLI steps up for MFA-protected actions.

use std::collections::HashMap;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use axum::Json;
use axum::body::{Body, Bytes};
use axum::extract::{ConnectInfo, FromRequest, Path, State};
use axum::http::{HeaderMap, HeaderValue, Request, StatusCode, header};
use axum::response::{IntoResponse, Response};
use forge_core::model::SessionKind;
use forge_core::{EntityRef, UserId};
use serde::Deserialize;
use serde_json::{Value, json};

use crate::auth::Caller;
use crate::error::{ApiError, ApiResult};
use crate::state::AppState;

pub const DEVICE_GRANT: &str = "urn:ietf:params:oauth:grant-type:device_code";
/// How long a person has to approve.
const TTL: Duration = Duration::from_secs(600);
/// Seconds between polls the client must keep.
const INTERVAL: Duration = Duration::from_secs(5);
const MAX_PENDING: usize = 1000;
/// Pending requests one client address may hold (not counted for loopback,
/// which is an unconfigured local reverse proxy shared by everyone).
const MAX_PENDING_PER_ADDRESS: usize = 5;
/// Unambiguous letters (no vowels, so no words; no I/O/L/U lookalikes).
const ALPHABET: &[u8] = b"BCDFGHJKMNPQRSTVWXZ";
/// Wrong codes a signed-in person may enter in 15 minutes.
const MAX_WRONG_CODES: u32 = 10;

#[derive(Debug, Clone)]
enum Decision {
    Pending,
    Approved { user: UserId },
    Denied,
}

#[derive(Debug)]
struct Entry {
    device_hash: String,
    client_id: String,
    at: Instant,
    last_poll: Option<Instant>,
    decision: Decision,
    /// Where the request came from, shown before approving.
    requested_from: std::net::IpAddr,
    user_agent: String,
}

/// Pending device requests by user code.
#[derive(Debug, Default)]
pub struct DeviceRequests(Mutex<HashMap<String, Entry>>);

fn user_code() -> String {
    let bytes = forge_secrets::crypto::random_bytes::<8>();
    let s: String = bytes.iter().map(|b| char::from(ALPHABET[usize::from(*b) % ALPHABET.len()])).collect();
    format!("{}-{}", &s[..4], &s[4..])
}

/// `abcd efgh`, `ABCD-EFGH` → `ABCD-EFGH`.
fn normalise(code: &str) -> Option<String> {
    let s: String = code.chars().filter(char::is_ascii_alphanumeric).map(|c| c.to_ascii_uppercase()).collect();
    (s.len() == 8 && s.bytes().all(|b| ALPHABET.contains(&b))).then(|| format!("{}-{}", &s[..4], &s[4..]))
}

fn oauth_error(error: &str, description: &str) -> Response {
    let mut r =
        (StatusCode::BAD_REQUEST, Json(json!({ "error": error, "error_description": description }))).into_response();
    r.headers_mut().insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    r
}

fn form(body: &Bytes) -> HashMap<String, String> {
    url::form_urlencoded::parse(body).into_owned().collect()
}

/// Whether one more pending request fits: the global cap, and a cap per
/// client address (loopback is an unconfigured local proxy everyone shares).
fn may_add(total: usize, from_this_address: usize, from: std::net::IpAddr) -> bool {
    total < MAX_PENDING && (from.to_canonical().is_loopback() || from_this_address < MAX_PENDING_PER_ADDRESS)
}

/// A short, printable `User-Agent` for the console.
fn user_agent(headers: &HeaderMap) -> String {
    headers
        .get(header::USER_AGENT)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("unknown")
        .chars()
        .filter(|c| !c.is_control())
        .take(120)
        .collect()
}

/// `POST /oauth/device_authorization` (RFC 8628 §3.1), first-party clients only.
pub async fn authorize(
    State(state): State<AppState>,
    ConnectInfo(peer): ConnectInfo<std::net::SocketAddr>,
    origin: crate::request_origin::RequestOrigin,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    let f = form(&body);
    let client_id = f.get("client_id").cloned().unwrap_or_default();
    if !state.identity.is_first_party(&client_id) {
        return oauth_error("invalid_client", "device sign-in is for Forge's own apps");
    }
    let from = crate::throttle::client_ip(&headers, peer, &state.config.trusted_proxies());
    let device_code = forge_secrets::crypto::random_token::<32>();
    let Ok(mut m) = state.device_requests.0.lock() else {
        return oauth_error("server_error", "try again");
    };
    m.retain(|_, e| e.at.elapsed() < TTL);
    // One address cannot fill the table for everyone.
    let mine = m.values().filter(|e| e.requested_from == from).count();
    if !may_add(m.len(), mine, from) {
        return oauth_error("slow_down", "too many sign-in requests; try again later");
    }
    let mut code = user_code();
    while m.contains_key(&code) {
        code = user_code();
    }
    m.insert(
        code.clone(),
        Entry {
            device_hash: forge_identity::device_code_hash(&device_code),
            client_id,
            at: Instant::now(),
            last_poll: None,
            decision: Decision::Pending,
            requested_from: from,
            user_agent: user_agent(&headers),
        },
    );
    drop(m);
    // The address the client used (D-167): the person opens it on the same
    // network.
    let base = origin.origin();
    let mut r = Json(json!({
        "device_code": device_code,
        "user_code": code,
        "verification_uri": format!("{base}/device"),
        "verification_uri_complete": format!("{base}/device?code={code}"),
        "expires_in": TTL.as_secs(),
        "interval": INTERVAL.as_secs(),
    }))
    .into_response();
    r.headers_mut().insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    r
}

/// `POST /oauth/token`: the device grant here; every other grant goes to
/// [`super::session::oauth_token`] unchanged.
pub async fn token(State(state): State<AppState>, body: Bytes) -> Response {
    let f = form(&body);
    if f.get("grant_type").map(String::as_str) != Some(DEVICE_GRANT) {
        let req = Request::builder()
            .method(axum::http::Method::POST)
            .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded")
            .body(Body::from(body))
            .unwrap_or_default();
        return match axum::Form::<super::session::TokenRequest>::from_request(req, &state).await {
            Ok(form) => super::session::oauth_token(State(state), form).await.into_response(),
            Err(e) => e.into_response(),
        };
    }
    let (Some(device_code), Some(client_id)) = (f.get("device_code"), f.get("client_id")) else {
        return oauth_error("invalid_request", "device_code and client_id are required");
    };
    let hash = forge_identity::device_code_hash(device_code);
    let decided = {
        let Ok(mut m) = state.device_requests.0.lock() else { return oauth_error("server_error", "try again") };
        let Some((code, e)) = m.iter_mut().find(|(_, e)| e.device_hash == hash) else {
            return oauth_error("expired_token", "the device code expired or is unknown; start again");
        };
        let code = code.clone();
        if e.at.elapsed() >= TTL {
            m.remove(&code);
            return oauth_error("expired_token", "the device code expired; start again");
        }
        if e.client_id != *client_id {
            return oauth_error("invalid_grant", "client mismatch");
        }
        match e.decision.clone() {
            Decision::Pending => {
                let too_fast = e.last_poll.is_some_and(|p| p.elapsed() < INTERVAL);
                e.last_poll = Some(Instant::now());
                return if too_fast {
                    oauth_error("slow_down", "poll less often")
                } else {
                    oauth_error("authorization_pending", "waiting for approval in the console")
                };
            }
            d => {
                // Single use: approved or denied, the request is done.
                m.remove(&code);
                d
            }
        }
    };
    match decided {
        Decision::Approved { user } => match state.identity.issue_device_tokens(user).await {
            Ok(t) => {
                let ev = forge_core::model::AuditEvent::new(
                    format!("user:{user}"),
                    "oauth.device.token",
                    EntityRef::platform(),
                    forge_core::model::AuditOutcome::Success,
                )
                .with_details(json!({ "client_id": client_id }));
                if let Err(e) = state.audit.append(ev).await {
                    tracing::error!(error = %e, "audit write failed");
                }
                let mut r = Json(t).into_response();
                r.headers_mut().insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
                r
            }
            Err(e) => ApiError::from(e).into_response(),
        },
        _ => oauth_error("access_denied", "the sign-in was denied"),
    }
}

/// Only a person's console session (cookie with CSRF) approves.
fn console_caller(caller: &Caller) -> ApiResult<()> {
    if caller.session.session.kind != SessionKind::Web || caller.via_api_key() {
        return Err(ApiError::forbidden("approve device sign-ins in the Forge console"));
    }
    Ok(())
}

/// Looks up a code for the console, counting (and auditing) wrong codes per person.
async fn find(state: &AppState, caller: &Caller, code: &str) -> ApiResult<String> {
    let key = format!("device:{}", caller.session.user.id);
    if !state.login_throttle.allowed_up_to(&key, MAX_WRONG_CODES) {
        caller.audit_denied("oauth.device.lookup", &EntityRef::platform(), None, "too many wrong codes").await;
        return Err(ApiError::new(StatusCode::TOO_MANY_REQUESTS, "throttled", "too many wrong codes; wait a while"));
    }
    let found =
        normalise(code).filter(|c| {
            state.device_requests.0.lock().is_ok_and(|m| {
                m.get(c).is_some_and(|e| e.at.elapsed() < TTL && matches!(e.decision, Decision::Pending))
            })
        });
    match found {
        Some(c) => Ok(c),
        None => {
            state.login_throttle.record_failure(&key);
            caller.audit_denied("oauth.device.lookup", &EntityRef::platform(), None, "unknown or expired code").await;
            Err(ApiError::not_found("no pending sign-in with that code; check it, or start again on the device"))
        }
    }
}

/// `GET /api/v1/oauth/device/{code}`: what the console shows before approving.
pub async fn get(State(state): State<AppState>, caller: Caller, Path(code): Path<String>) -> ApiResult<Json<Value>> {
    console_caller(&caller)?;
    let code = find(&state, &caller, &code).await?;
    let (client_id, age, from, agent) = state
        .device_requests
        .0
        .lock()
        .ok()
        .and_then(|m| {
            m.get(&code)
                .map(|e| (e.client_id.clone(), e.at.elapsed(), e.requested_from.to_string(), e.user_agent.clone()))
        })
        .unwrap_or_default();
    Ok(Json(json!({
        "user_code": code,
        "client_id": client_id,
        "client_name": if client_id == "forge-app" { "Orbyn Forge CLI and apps" } else { client_id.as_str() },
        "requested_seconds_ago": age.as_secs(),
        "expires_in": TTL.saturating_sub(age).as_secs(),
        "user": caller.session.user.login(),
        // Device tokens never carry MFA; the CLI asks for a code when needed.
        "mfa": false,
        "requested_from": from,
        "user_agent": agent,
    })))
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Decide {
    pub approve: bool,
}

/// `POST /api/v1/oauth/device/{code}`: approve or deny.
pub async fn decide(
    State(state): State<AppState>,
    caller: Caller,
    Path(code): Path<String>,
    Json(d): Json<Decide>,
) -> ApiResult<Json<Value>> {
    console_caller(&caller)?;
    let code = find(&state, &caller, &code).await?;
    let (client_id, from, agent) = {
        let mut m = state
            .device_requests
            .0
            .lock()
            .map_err(|_| ApiError::new(StatusCode::SERVICE_UNAVAILABLE, "busy", "try again"))?;
        // Decided once, under the same lock as the check: two answers racing
        // cannot overwrite each other.
        let e = m
            .get_mut(&code)
            .filter(|e| matches!(e.decision, Decision::Pending) && e.at.elapsed() < TTL)
            .ok_or_else(|| ApiError::not_found("no pending sign-in with that code"))?;
        e.decision = if d.approve { Decision::Approved { user: caller.session.user.id } } else { Decision::Denied };
        (e.client_id.clone(), e.requested_from.to_string(), e.user_agent.clone())
    };
    let resource = EntityRef::platform();
    if d.approve {
        caller
            .audit(
                "oauth.device.approve",
                &resource,
                None,
                json!({ "client_id": client_id, "user_code": code, "requested_from": from, "user_agent": agent }),
            )
            .await;
    } else {
        caller.audit_denied("oauth.device.approve", &resource, None, "the person denied the device sign-in").await;
    }
    Ok(Json(json!({ "approved": d.approve })))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Security review: one address could fill the table and block device
    /// sign-in for everyone.
    #[test]
    fn one_address_cannot_fill_the_table() {
        let a: std::net::IpAddr = "203.0.113.9".parse().unwrap_or(std::net::IpAddr::from([0, 0, 0, 0]));
        assert!(may_add(10, 4, a));
        assert!(!may_add(10, MAX_PENDING_PER_ADDRESS, a));
        assert!(!may_add(MAX_PENDING, 0, a));
        assert!(may_add(10, 50, std::net::IpAddr::from([127, 0, 0, 1])), "a local proxy is shared");
    }

    #[test]
    fn codes_are_unambiguous_and_normalised() {
        for _ in 0..50 {
            let c = user_code();
            assert_eq!(c.len(), 9);
            assert_eq!(normalise(&c).as_deref(), Some(c.as_str()));
        }
        assert_eq!(normalise("bcdf ghjk").as_deref(), Some("BCDF-GHJK"));
        assert_eq!(normalise("BCDF-GHJ"), None, "too short");
        assert_eq!(normalise("ABCD-EFGH"), None, "vowels are never issued");
    }
}
