//! Onboarding console: what a node install is doing, shell style, live.
//!
//! * Assisted (SSH) onboarding: the `node.bootstrap` job writes each command
//!   it runs (`$ …`) and the installer's output here while it runs, and keeps
//!   a copy in its state (`state.log`) for history.
//! * Manual installs: the installer reports its steps to
//!   `POST /api/v1/enroll/progress`, authenticated by its enrollment token.
//!
//! * `GET /api/v1/onboarding/nodes` — nodes being onboarded (installing, failed,
//!   done in the last day), with their cluster.
//! * `GET /api/v1/onboarding/console?job_id=|token_id=` — the console lines.
//!
//! Lines live in memory (bounded) while an install runs; a job's copy in its
//! state survives restarts. Secrets are masked before lines are stored.

use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

use axum::Json;
use axum::Router;
use axum::body::Bytes;
use axum::extract::{Query, State};
use axum::http::{HeaderMap, StatusCode, header};
use axum::routing::{get, post};
use forge_core::model::{JobFilter, JobStatus};
use forge_core::ports::{EnrollmentRepository, JobRepository};
use forge_core::{Action, EntityKind, EntityRef, JobId, Timestamp};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::auth::{Caller, Ws};
use crate::error::{ApiError, ApiResult};
use crate::state::AppState;

const MAX_LINES: usize = 2000;
const MAX_CONSOLES: usize = 500;
const KEEP: Duration = Duration::from_secs(24 * 3600);
const MAX_REPORT: usize = 2048;

/// One console line.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Line {
    /// Unix milliseconds.
    pub t: u64,
    /// Job step (`connect`, `install`…) or `installer` for manual reports.
    pub step: String,
    pub line: String,
}

#[derive(Debug, Default)]
struct Console {
    lines: Vec<Line>,
    touched: Option<Instant>,
}

fn consoles() -> &'static Mutex<HashMap<String, Console>> {
    static C: OnceLock<Mutex<HashMap<String, Console>>> = OnceLock::new();
    C.get_or_init(|| Mutex::new(HashMap::new()))
}

fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| u64::try_from(d.as_millis()).unwrap_or(0))
}

/// Console key of a job.
#[must_use]
pub fn job_key(id: JobId) -> String {
    format!("job:{id}")
}

fn token_key(id: &str) -> String {
    format!("token:{id}")
}

/// Appends lines (already masked) to a console.
pub fn push(key: &str, step: &str, lines: &[String]) {
    let Ok(mut all) = consoles().lock() else { return };
    if all.len() >= MAX_CONSOLES && !all.contains_key(key) {
        // Drop the oldest consoles.
        all.retain(|_, c| c.touched.is_some_and(|t| t.elapsed() < KEEP));

        if all.len() >= MAX_CONSOLES
            && let Some(oldest) = all.iter().min_by_key(|(_, c)| c.touched).map(|(k, _)| k.clone())
        {
            all.remove(&oldest);
        }
    }

    let c = all.entry(key.to_owned()).or_default();
    let t = now_ms();

    for l in lines {
        c.lines.push(Line { t, step: step.to_owned(), line: l.chars().take(400).collect() });
    }

    let excess = c.lines.len().saturating_sub(MAX_LINES);
    c.lines.drain(..excess);
    c.touched = Some(Instant::now());
}

fn lines(key: &str) -> Option<Vec<Line>> {
    consoles().lock().ok()?.get(key).map(|c| c.lines.clone())
}

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/onboarding/nodes", get(list))
        .route("/onboarding/console", get(console))
        .route("/enroll/progress", post(progress))
}

/// `POST /api/v1/enroll/progress`: one line from an installer, authenticated
/// by its enrollment token (`Authorization: Bearer <token>`; used or not,
/// until an hour after it expires).
pub async fn progress(State(state): State<AppState>, headers: HeaderMap, body: Bytes) -> StatusCode {
    let Some(token) = headers
        .get(header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "))
        .and_then(forge_pki::enrollment::parse)
    else {
        return StatusCode::UNAUTHORIZED;
    };

    if body.len() > MAX_REPORT {
        return StatusCode::PAYLOAD_TOO_LARGE;
    }

    let hash = forge_pki::enrollment::hash_secret(&token.secret);
    let Ok(Some(t)) = state.store.find_token_by_hash(&hash).await else { return StatusCode::UNAUTHORIZED };
    if state.clock.now().0 > t.expires_at.0 + 3_600_000 {
        return StatusCode::UNAUTHORIZED;
    }

    let text = String::from_utf8_lossy(&body).replace(&token.secret, "********");
    let clean: Vec<String> = text
        .lines()
        .map(|l| l.chars().filter(|c| !c.is_control()).collect::<String>())
        .filter(|l| !l.trim().is_empty())
        .collect();
    let key = token_key(&t.id);
    if lines(&key).is_some_and(|l| l.len() >= MAX_LINES) {
        return StatusCode::TOO_MANY_REQUESTS;
    }

    push(&key, "installer", &clean);

    StatusCode::NO_CONTENT
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct ConsoleQuery {
    #[serde(default)]
    #[schemars(with = "Option<String>")]
    pub job_id: Option<JobId>,
    #[serde(default)]
    pub token_id: Option<String>,
}

/// The console of an onboarding job or a manual install's token.
pub async fn console(
    State(state): State<AppState>,
    caller: Caller,
    Ws(ws): Ws,
    Query(q): Query<ConsoleQuery>,
) -> ApiResult<Json<Value>> {
    if let Some(id) = q.job_id {
        caller.authorize(Action::View, EntityRef::new(EntityKind::Job, id)).await?;
        let job = state.store.get_job(id).await?;

        // Live lines while running; the job's own copy otherwise.
        let live = lines(&job_key(id));
        let stored: Vec<Line> =
            job.state.get("log").cloned().and_then(|v| serde_json::from_value(v).ok()).unwrap_or_default();

        let out = match live {
            Some(l) if l.len() >= stored.len() => l,
            _ => stored,
        };

        return Ok(Json(json!({ "status": job.status, "error": job.last_error, "lines": out })));
    }

    let tid = q.token_id.ok_or_else(|| ApiError::bad_request("job_id or token_id"))?;
    let t = state
        .store
        .get_token(&tid)
        .await?
        .filter(|t| t.workspace_id == ws)
        .ok_or_else(|| ApiError::not_found("token not found"))?;
    caller.authorize(Action::ServerEnroll, EntityRef::new(EntityKind::Workspace, ws)).await?;
    let l = lines(&token_key(&t.id)).unwrap_or_default();
    let failed = l.iter().any(|x| x.line.starts_with("installer failed") || x.line.starts_with("error: "));
    let done = l.iter().any(|x| x.line == "installer finished");

    let status = if failed {
        "failed"
    } else if done || t.server_id.is_some() {
        "succeeded"
    } else {
        "running"
    };

    Ok(Json(json!({ "status": status, "server_id": t.server_id, "lines": l })))
}

/// Nodes being onboarded: assisted jobs and manual installs that reported.
pub async fn list(State(state): State<AppState>, caller: Caller, Ws(ws): Ws) -> ApiResult<Json<Vec<Value>>> {
    let since = state.clock.now().0 - 24 * 3_600_000;
    let jobs = state.store.list_jobs(&JobFilter { workspace_id: Some(ws), status: None, limit: 500 }).await?;
    let mut out = Vec::new();
    let bootstrap: Vec<_> =
        jobs.into_iter().filter(|j| j.kind == "node.bootstrap" && j.created_at.0 >= since).collect();
    let refs = bootstrap.iter().map(|j| EntityRef::new(EntityKind::Job, j.id)).collect();
    let visible = caller.can_many(Action::View, refs).await;

    for (j, ok) in bootstrap.iter().zip(visible) {
        if !ok {
            continue;
        }

        let p = &j.payload;
        out.push(json!({
            "kind": "assisted",
            "job_id": j.id,
            "status": j.status,
            "error": j.last_error,
            "host": p.pointer("/target/host"),
            "hostname": p.get("hostname").cloned().filter(|h| !h.is_null()).or_else(|| p.pointer("/target/host").cloned()),
            "cluster_id": p.get("cluster_id"),
            "server_id": j.state.get("server_id"),
            "created_at": j.created_at,
            "updated_at": j.updated_at,
            "active": matches!(j.status, JobStatus::Queued | JobStatus::Running),
        }));
    }

    if caller.can(Action::ServerEnroll, EntityRef::new(EntityKind::Workspace, ws)).await {
        let keys: Vec<(String, u64)> = consoles()
            .lock()
            .map(|c| {
                c.iter()
                    .filter_map(|(k, v)| {
                        k.strip_prefix("token:").map(|t| (t.to_owned(), v.lines.last().map_or(0, |l| l.t)))
                    })
                    .collect()
            })
            .unwrap_or_default();

        for (tid, last) in keys {
            let Ok(Some(t)) = state.store.get_token(&tid).await else { continue };
            if t.workspace_id != ws {
                continue;
            }

            let target = state.store.get_enrollment_target(&tid).await.ok().flatten();
            let l = lines(&token_key(&tid)).unwrap_or_default();
            let failed = l.iter().any(|x| x.line.starts_with("installer failed") || x.line.starts_with("error: "));
            let done = l.iter().any(|x| x.line == "installer finished");
            out.push(json!({
                "kind": "manual",
                "token_id": tid,
                "status": if failed { "failed" } else if done || t.server_id.is_some() { "succeeded" } else { "running" },
                "hostname": target.as_ref().and_then(|(_, h)| h.clone()),
                "cluster_id": target.and_then(|(c, _)| c),
                "server_id": t.server_id,
                "updated_at": Timestamp(i64::try_from(last).unwrap_or(0)),
                "active": !failed && !done && t.server_id.is_none(),
            }));
        }
    }

    Ok(Json(out))
}
