//! `GET /api/v1/events` — one WebSocket per UI session carrying live job and
//! server status. Each event is authorized for the subscriber before sending.
//!
//! The socket closes when the sign-in behind it ends (logout, password
//! change, revoked API key, expiry): checked every [`RECHECK`], and never
//! later than [`MAX_LIFETIME`] after it opened (#13 inventory).

use axum::extract::State;
use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::http::HeaderMap;
use axum::response::Response;
use forge_core::{Action, EntityKind, EntityRef};
use tokio::sync::broadcast::error::RecvError;

use crate::auth::Caller;
use crate::error::{ApiError, ApiResult};
use crate::events::ServerEvent;
use crate::state::AppState;

/// How often the sign-in behind an open socket is checked.
const RECHECK: std::time::Duration = std::time::Duration::from_secs(30);
/// Longest a socket stays open; the console reconnects.
const MAX_LIFETIME: std::time::Duration = std::time::Duration::from_secs(12 * 3600);

pub async fn subscribe(
    State(state): State<AppState>,
    caller: Caller,
    origin: crate::request_origin::RequestOrigin,
    headers: HeaderMap,
    ws: WebSocketUpgrade,
) -> ApiResult<Response> {
    // Cross-site WebSocket hijacking guard: browsers always send Origin.
    if !crate::request_origin::origin_header_ok(&state, &origin, &headers) {
        return Err(ApiError::forbidden("origin not allowed"));
    }

    Ok(ws.max_message_size(64 * 1024).on_upgrade(move |socket| stream(state, caller, socket)))
}

async fn stream(state: AppState, caller: Caller, mut socket: WebSocket) {
    let mut jobs = state.jobs.subscribe();
    let mut servers = state.events.subscribe();
    let mut signed_out = state.signed_out.subscribe();
    let me = caller.session.user.id;
    let deadline = tokio::time::Instant::now() + MAX_LIFETIME;
    let mut recheck = tokio::time::interval(RECHECK);
    recheck.tick().await;

    loop {
        let event = tokio::select! {
            () = tokio::time::sleep_until(deadline) => break,
            u = signed_out.recv() => match u {
                Ok(u) if u == me => break,
                _ => continue,
            },
            _ = recheck.tick() => {
                if crate::auth::still_signed_in(&state, &caller).await {
                    continue;
                }
                break;
            }
            r = jobs.recv() => r.map(ServerEvent::Job),
            r = servers.recv() => r,
            msg = socket.recv() => match msg {
                Some(Ok(Message::Close(_)) | Err(_)) | None => break,
                _ => continue,
            },
        };

        let event = match event {
            Ok(e) => e,
            Err(RecvError::Lagged(n)) => {
                tracing::debug!(skipped = n, "slow event subscriber");
                continue;
            }
            Err(RecvError::Closed) => break,
        };

        let resource = match &event {
            ServerEvent::Job(j) => EntityRef::new(EntityKind::Job, j.job_id),
            ServerEvent::Server { server_id, .. } => EntityRef::new(EntityKind::Server, server_id),
        };

        if !caller.can(Action::View, resource).await {
            continue;
        }

        let Ok(text) = serde_json::to_string(&event) else { continue };
        if socket.send(Message::Text(text.into())).await.is_err() {
            break;
        }
    }

    let _ = socket.send(Message::Close(None)).await;
}
