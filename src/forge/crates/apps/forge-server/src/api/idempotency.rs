//! `Idempotency-Key` for mutating requests (issue #5, D-074; IETF httpapi
//! draft "The Idempotency-Key HTTP Header Field").
//!
//! A POST, PUT, PATCH or DELETE with the header runs at most once per
//! principal and key for 24 hours. A retry with the same key and the same
//! request gets the stored response again (`Idempotent-Replayed: true`); a
//! retry while the first is still running gets `409`; the same key with a
//! different request gets `422`. Only successful (2xx) JSON or empty
//! responses are stored, sealed with the vault envelope because some carry a
//! one-time secret. Any other outcome releases the key, so a retry runs
//! again (a step-up, an expired token or a transient error never sticks).

use axum::body::{Body, Bytes, HttpBody};
use axum::extract::{FromRequestParts, Request, State};
use axum::http::{HeaderValue, Method, StatusCode, header};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use forge_core::Timestamp;
use forge_core::model::{IdempotencyBegin, IdempotencyRecord};
use forge_core::ports::IdempotencyRepository;

use crate::auth::Caller;
use crate::error::ApiError;
use crate::state::AppState;

pub const HEADER: &str = "idempotency-key";
pub const REPLAYED: &str = "idempotent-replayed";
/// How long a key and its stored response are kept.
const TTL_MS: i64 = 24 * 60 * 60 * 1000;
/// A pending key whose request died (crash, restart) is free again after this.
const STALE_PENDING_MS: i64 = 10 * 60 * 1000;
/// Largest response body that is stored for replay; larger answers are not
/// replayable (a retry runs again).
const MAX_STORED: u64 = 256 * 1024;
/// Unexpired keys one person (per credential kind) may hold; beyond it
/// requests run without a stored outcome, so nobody can fill the database.
const MAX_KEYS_PER_PRINCIPAL: u64 = 1000;

/// Whose stored outcomes a request may see: the person AND the kind of
/// credential, so a weaker credential (an API key, a session without MFA)
/// never receives what a stronger one was allowed to see. Tokens the CLI
/// refreshes keep their kind, so its retries still find their keys.
fn scope(caller: &Caller) -> String {
    let kind = if caller.via_api_key() {
        "api-key"
    } else if caller.ctx().mfa {
        "mfa"
    } else {
        "session"
    };

    format!("{}#{kind}", caller.principal())
}

/// Responses that can carry a credential or a revealed secret are never
/// stored for replay (the same paths are never compressed).
fn storable_path(path: &str) -> bool {
    !super::NO_COMPRESSION.iter().any(|p| path.contains(p))
}

fn valid_key(k: &str) -> bool {
    (1..=255).contains(&k.len()) && k.bytes().all(|b| b.is_ascii_graphic())
}

fn aad(principal: &str, key: &str) -> String {
    format!("idempotency:{principal}:{key}")
}

fn fingerprint(method: &Method, path_and_query: &str, body: &[u8]) -> String {
    let mut ctx = ring::digest::Context::new(&ring::digest::SHA256);
    ctx.update(method.as_str().as_bytes());
    ctx.update(b"\n");
    ctx.update(path_and_query.as_bytes());
    ctx.update(b"\n");
    ctx.update(body);

    hex::encode(ctx.finish().as_ref())
}

fn in_progress() -> Response {
    let mut r = ApiError::new(
        StatusCode::CONFLICT,
        "idempotency_in_progress",
        "a request with this Idempotency-Key is still running; retry shortly",
    )
    .into_response();
    r.headers_mut().insert(header::RETRY_AFTER, HeaderValue::from_static("1"));

    r
}

/// The stored outcome of an earlier request with the same key.
fn replay(state: &AppState, rec: &IdempotencyRecord, request_hash: &str) -> Response {
    if rec.request_hash != request_hash {
        return ApiError::new(
            StatusCode::UNPROCESSABLE_ENTITY,
            "idempotency_key_reused",
            "this Idempotency-Key was used for a different request; use a new key",
        )
        .into_response();
    }

    if !rec.done {
        return in_progress();
    }

    let body = match rec.body.as_deref().map(|b| state.envelope.open(b, &aad(&rec.principal, &rec.key))).transpose() {
        Ok(b) => b.unwrap_or_default(),
        Err(e) => {
            tracing::error!(error = %e, "stored idempotent response cannot be opened");
            return ApiError::new(StatusCode::INTERNAL_SERVER_ERROR, "internal", "internal error").into_response();
        }
    };

    let status = rec.status.and_then(|s| StatusCode::from_u16(s).ok()).unwrap_or(StatusCode::OK);
    let mut r = Response::new(Body::from(body));
    *r.status_mut() = status;

    if let Some(ct) = rec.content_type.as_deref().and_then(|c| HeaderValue::from_str(c).ok()) {
        r.headers_mut().insert(header::CONTENT_TYPE, ct);
    }

    r.headers_mut().insert(REPLAYED, HeaderValue::from_static("true"));

    r
}

/// Middleware on `/api/v1`.
pub async fn guard(State(state): State<AppState>, req: Request, next: Next) -> Response {
    if !matches!(*req.method(), Method::POST | Method::PUT | Method::PATCH | Method::DELETE) {
        return next.run(req).await;
    }

    let Some(raw) = req.headers().get(HEADER) else {
        return next.run(req).await;
    };

    let Some(key) = raw.to_str().ok().filter(|k| valid_key(k)).map(str::to_owned) else {
        return ApiError::bad_request("Idempotency-Key must be 1 to 255 visible ASCII characters").into_response();
    };

    if !storable_path(req.uri().path()) {
        return next.run(req).await;
    }

    let (mut parts, body) = req.into_parts();

    // Unauthenticated requests go on and fail in the handler as usual.
    let Ok(caller) = Caller::from_request_parts(&mut parts, &state).await else {
        return next.run(Request::from_parts(parts, body)).await;
    };

    // Scoped to the person and credential kind: never replayed to anyone
    // else, nor to a weaker credential of the same person.
    let principal = scope(&caller);
    let now = state.clock.now();
    if state.store.count_idempotent(&principal, now).await.unwrap_or(u64::MAX) >= MAX_KEYS_PER_PRINCIPAL {
        return next.run(Request::from_parts(parts, body)).await;
    }

    let bytes: Bytes = match axum::body::to_bytes(body, super::MAX_BODY).await {
        Ok(b) => b,
        Err(_) => {
            return ApiError::new(StatusCode::PAYLOAD_TOO_LARGE, "too_large", "request body too large").into_response();
        }
    };

    let pq = parts.uri.path_and_query().map_or_else(|| parts.uri.path().to_owned(), ToString::to_string);
    let hash = fingerprint(&parts.method, &pq, &bytes);
    let begun = state
        .store
        .begin_idempotent(&principal, &key, &hash, now, Timestamp(now.0 + TTL_MS), Timestamp(now.0 - STALE_PENDING_MS))
        .await;

    match begun {
        Ok(IdempotencyBegin::Started) => {}
        Ok(IdempotencyBegin::Existing(rec)) => {
            let resp = replay(&state, &rec, &hash);
            if resp.headers().contains_key(REPLAYED) {
                audit_replay(&state, &caller, parts.uri.path()).await;
            }

            return resp;
        }
        Err(forge_core::DomainError::Conflict(_)) => return in_progress(),
        Err(e) => return ApiError::from(e).into_response(),
    }

    let resp = next.run(Request::from_parts(parts, Body::from(bytes))).await;
    let claim_hash = hash.clone();
    let release = |state: AppState, principal: String, key: String| async move {
        if let Err(e) = state.store.release_idempotent(&principal, &key, (&claim_hash, now)).await {
            tracing::warn!(error = %e, "releasing an Idempotency-Key failed");
        }
    };

    if !resp.status().is_success() {
        release(state, principal, key).await;
        return resp;
    }

    let content_type = resp.headers().get(header::CONTENT_TYPE).and_then(|v| v.to_str().ok()).map(str::to_owned);
    let json = content_type.as_deref().is_some_and(|c| c.starts_with("application/json"));
    let size = resp.body().size_hint().exact();
    let storable = (json && size.is_some_and(|s| s <= MAX_STORED)) || size == Some(0);
    if !storable {
        // A streamed or large response is not kept: the key is released and
        // a retry runs again (documented).
        release(state, principal, key).await;
        return resp;
    }

    let (parts, body) = resp.into_parts();

    let Ok(bytes) = axum::body::to_bytes(body, usize::try_from(MAX_STORED).unwrap_or(usize::MAX)).await else {
        release(state, principal, key).await;
        return ApiError::new(StatusCode::INTERNAL_SERVER_ERROR, "internal", "internal error").into_response();
    };

    let sealed =
        if bytes.is_empty() { Ok(None) } else { state.envelope.seal(&bytes, &aad(&principal, &key)).map(Some) };

    match sealed {
        Ok(sealed) => {
            if let Err(e) = state
                .store
                .finish_idempotent(
                    &principal,
                    &key,
                    (&hash, now),
                    parts.status.as_u16(),
                    content_type.as_deref(),
                    sealed.as_deref(),
                )
                .await
            {
                tracing::warn!(error = %e, "storing an idempotent response failed");
                release(state, principal, key).await;
            }
        }
        Err(e) => {
            tracing::warn!(error = %e, "sealing an idempotent response failed");
            release(state, principal, key).await;
        }
    }

    Response::from_parts(parts, Body::from(bytes))
}

/// A replay runs no handler, so it leaves its own trace (path only).
async fn audit_replay(state: &AppState, caller: &Caller, path: &str) {
    let ev = forge_core::model::AuditEvent::new(
        caller.principal().to_string(),
        "idempotency.replay",
        forge_core::EntityRef::platform(),
        forge_core::model::AuditOutcome::Success,
    )
    .with_details(serde_json::json!({ "path": path }));

    if let Err(e) = state.audit.append(ev).await {
        tracing::error!(error = %e, "audit write failed");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Security review: a reveal or a token is never stored for replay.
    #[test]
    fn secret_bearing_paths_are_not_stored() {
        for p in ["/api/v1/vault/items/x/reveal", "/api/v1/auth/totp", "/api/v1/me/password", "/api/v1/api-keys/token"]
        {
            assert!(!storable_path(p), "{p}");
        }

        assert!(storable_path("/api/v1/clusters"));
    }

    #[test]
    fn keys_are_visible_ascii() {
        assert!(valid_key("0193a1b2-retry-1"));
        assert!(!valid_key(""));
        assert!(!valid_key("has space"));
        assert!(!valid_key(&"k".repeat(256)));
        assert!(!valid_key("ünicode"));
    }

    #[test]
    fn fingerprint_covers_method_path_and_body() {
        let a = fingerprint(&Method::POST, "/api/v1/x?y=1", b"{}");
        assert_eq!(a, fingerprint(&Method::POST, "/api/v1/x?y=1", b"{}"));
        assert_ne!(a, fingerprint(&Method::PUT, "/api/v1/x?y=1", b"{}"));
        assert_ne!(a, fingerprint(&Method::POST, "/api/v1/x?y=2", b"{}"));
        assert_ne!(a, fingerprint(&Method::POST, "/api/v1/x?y=1", b"{\"a\":1}"));
    }
}
