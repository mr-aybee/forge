//! Updating Forge itself from the console (DECISIONS.md D-037, D-098).
//!
//! * `GET  /api/v1/system/update` — current and latest version, state.
//! * `POST /api/v1/system/update/check` — ask the release feed now.
//! * `GET  /api/v1/system/update/releases` — releases newer than or equal to
//!   this version, newest first (older ones are never offered).
//! * `POST /api/v1/system/update/apply` — `{version, agents, batch}`:
//!   download, verify, back up, install and restart into any listed version
//!   strictly newer than this one (upgrades only), then optionally roll the
//!   agents to it. `system.update` on the platform (platform administrators,
//!   MFA step-up), audited.

use std::path::PathBuf;
use std::time::Duration;

use axum::Json;
use axum::extract::State;
use axum::http::StatusCode;
use forge_core::{Action, EntityRef};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use tokio::sync::watch;

use crate::auth::Caller;
use crate::error::{ApiError, ApiResult};
use crate::state::AppState;
use crate::update;

const CHECK_EVERY: Duration = Duration::from_secs(6 * 3600);

#[derive(Debug, Serialize)]
pub struct View {
    #[serde(flatten)]
    pub status: update::Status,
    /// Whether this caller may install it.
    pub can_update: bool,
    pub target: &'static str,
    /// The release feed this server checks.
    pub feed_url: String,
}

fn public_key(state: &AppState) -> String {
    state.config.update.public_key.clone().unwrap_or_else(|| update::RELEASE_PUBLIC_KEY.to_owned())
}

pub async fn get(State(state): State<AppState>, caller: Caller) -> ApiResult<Json<View>> {
    let mut status = update::status();
    status.enabled = state.config.update.enabled;
    let can_update = caller.can(Action::SystemUpdate, EntityRef::platform()).await;
    Ok(Json(View { status, can_update, target: update::target(), feed_url: state.config.update.feed_url.clone() }))
}

pub async fn check(State(state): State<AppState>, caller: Caller) -> ApiResult<Json<View>> {
    caller.authorize(Action::SystemUpdate, EntityRef::platform()).await?;
    let status = update::check(&state.config.update.feed_url, state.clock.now().0)
        .await
        .map_err(|e| ApiError::bad_request(format!("cannot reach the release feed: {e}")))?;
    Ok(Json(View {
        status,
        can_update: true,
        target: update::target(),
        feed_url: state.config.update.feed_url.clone(),
    }))
}

fn list_url(state: &AppState) -> String {
    update::releases_url(&state.config.update.feed_url, state.config.update.releases_url.as_deref())
}

/// One release offered for installation.
#[derive(Debug, Serialize)]
pub struct ReleaseView {
    pub version: String,
    pub published_at: Option<String>,
    pub notes: String,
    pub url: String,
    /// This control plane runs it.
    pub current: bool,
}

fn release_views(list: Vec<update::Release>) -> Vec<ReleaseView> {
    list.into_iter()
        .map(|r| ReleaseView {
            current: r.version == crate::VERSION,
            version: r.version,
            published_at: r.published_at,
            notes: r.notes,
            url: r.url,
        })
        .collect()
}

/// `GET /system/update/releases`: `view` on the platform. Releases newer
/// than or equal to this version, newest first.
pub async fn releases(State(state): State<AppState>, caller: Caller) -> ApiResult<Json<Value>> {
    caller.authorize(Action::View, EntityRef::platform()).await?;
    let list = update::list_releases(&list_url(&state))
        .await
        .map_err(|e| ApiError::bad_request(format!("cannot reach the release feed: {e}")))?;
    Ok(Json(json!({ "current": crate::VERSION, "releases": release_views(list) })))
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ApplyRequest {
    /// The version to install: any listed release strictly newer than this one.
    pub version: String,
    /// `rollout` (default): after the new server serves, update outdated
    /// Linux agents to it in canary batches; `none`: leave the agents.
    #[serde(default)]
    pub agents: Option<String>,
    /// Nodes per batch for the agent rollout (1-100, default 1).
    #[serde(default)]
    pub batch: Option<usize>,
}

/// Whether `version` may be installed over `current`: upgrades only
/// ([`update::compare_release`]: `1.0.0` is newer than `1.0.0-rc-01`).
fn upgrade_allowed(version: &str, current: &str) -> Result<(), String> {
    if update::compare_release(version, current) != std::cmp::Ordering::Greater {
        return Err(format!("{version} is not newer than {current}: Forge only upgrades"));
    }
    Ok(())
}

/// SQLite: a consistent copy with `VACUUM INTO`; other databases are left to
/// their own backups.
async fn backup(state: &AppState, from: &str) -> Result<Option<PathBuf>, String> {
    let url = state.config.database_url();
    if !url.starts_with("sqlite:") {
        return Ok(None);
    }
    let dir = state.config.data_dir.join("backups");
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let file = dir.join(format!("forge-{from}-{}.db", state.clock.now().0));
    state.store.backup_sqlite(&url, &file).await.map_err(|e| format!("database backup failed: {e}"))?;
    Ok(Some(file))
}

async fn refuse<T>(caller: &Caller, msg: String) -> ApiResult<T> {
    caller.audit_denied("system.update", &EntityRef::platform(), None, &msg).await;
    Err(ApiError::conflict(msg))
}

pub async fn apply(
    State(state): State<AppState>,
    caller: Caller,
    Json(req): Json<ApplyRequest>,
) -> ApiResult<(StatusCode, Json<Value>)> {
    let platform = EntityRef::platform();
    caller.authorize(Action::SystemUpdate, platform.clone()).await?;
    let version = req.version.trim().trim_start_matches('v').to_owned();
    let agents = req.agents.as_deref().unwrap_or("rollout").to_owned();
    let batch = req.batch.unwrap_or(1);
    if !matches!(agents.as_str(), "rollout" | "none") {
        return Err(ApiError::bad_request("agents: `rollout` or `none`"));
    }
    if batch == 0 || batch > crate::jobs::agent_rollout::MAX_BATCH {
        return Err(ApiError::bad_request("batch: 1 to 100 nodes"));
    }
    if let Err(e) = upgrade_allowed(&version, crate::VERSION) {
        return refuse(&caller, e).await;
    }
    if matches!(update::status().state.as_str(), "downloading" | "verifying" | "installing" | "restarting") {
        return refuse(&caller, "an update is already in progress".into()).await;
    }
    // The release comes from the feed by tag and must be one the list offers.
    let url = list_url(&state);
    let rel = update::release_by_tag(&url, &version)
        .await
        .map_err(|e| ApiError::bad_request(format!("cannot fetch release {version}: {e}")))?;
    let offered = update::list_releases(&url)
        .await
        .map_err(|e| ApiError::bad_request(format!("cannot reach the release feed: {e}")))?;
    if !offered.iter().any(|r| r.version == rel.version) {
        return refuse(&caller, format!("{version} is not offered by the release feed")).await;
    }
    let from = crate::VERSION;
    let follow = (agents == "rollout").then(|| update::AgentFollowUp {
        mode: agents.clone(),
        batch,
        principal: caller.principal(),
        context: caller.ctx(),
    });
    caller
        .audit(
            "system.update",
            &platform,
            None,
            json!({ "from": from, "to": rel.version, "agents": agents, "batch": batch, "target": update::target() }),
        )
        .await;
    // Runs in the background; the console polls the status, then waits for
    // the new version to answer.
    let st = state.clone();
    let key = public_key(&state);
    tokio::spawn(async move {
        let data_dir = st.config.data_dir.clone();
        let dist = st.config.agent_dist_dir();
        let cfg = st.config.config_path.clone();
        let r =
            update::apply(&rel, &key, &data_dir, &dist, cfg.as_deref(), backup(&st, from), follow, st.clock.now().0)
                .await;
        match r {
            Ok(a) => {
                tracing::info!(from = %a.from, to = %a.to, "update installed; restarting");
                // Let the console read "restarting" before the listener goes.
                tokio::time::sleep(Duration::from_secs(2)).await;
                update::restart_signal().notify_one();
            }
            Err(e) => {
                tracing::warn!(error = %e, "update failed; nothing was changed");
                update::fail(e);
            }
        }
    });
    Ok((StatusCode::ACCEPTED, Json(json!({ "version": version, "agents": agents, "state": "downloading" }))))
}

/// Starts the agent follow-up recorded with a completed update (call after
/// [`update::mark_serving`]). Runs in the background.
pub fn start_agent_follow_up(state: &AppState, applied: &update::Applied) {
    if let Some(f) = applied.agents.clone() {
        tokio::spawn(super::agent_updates::update_follow_up(state.clone(), f));
    }
}

/// Checks the feed every six hours (first check a minute after start).
pub(crate) async fn run(state: AppState, mut shutdown: watch::Receiver<bool>) {
    if !state.config.update.enabled {
        return;
    }
    let mut wait = Duration::from_secs(60);
    loop {
        tokio::select! {
            () = tokio::time::sleep(wait) => {}
            _ = shutdown.changed() => break,
        }
        if let Err(e) = update::check(&state.config.update.feed_url, state.clock.now().0).await {
            tracing::debug!(error = %e, "update check failed");
        }
        wait = CHECK_EVERY;
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    #[test]
    fn only_strictly_newer_versions_are_installed() {
        assert!(upgrade_allowed("1.2.0", "1.1.0").is_ok());
        assert!(upgrade_allowed("1.1.1", "1.1.0").is_ok(), "a newer release that is not the latest");
        assert!(upgrade_allowed("1.1.0", "1.1.0").is_err(), "the running version");
        assert!(upgrade_allowed("1.0.9", "1.1.0").unwrap_err().contains("only upgrades"), "never a downgrade");
        assert!(upgrade_allowed("1.0.0", "1.0.0-rc-01").is_ok(), "the final release replaces its candidate");
        assert!(upgrade_allowed("1.0.0-rc-01", "1.0.0").is_err(), "a candidate never replaces its final");
    }

    #[test]
    fn the_running_release_is_marked_current() {
        let rel = |v: &str| update::Release {
            version: v.into(),
            notes: String::new(),
            url: String::new(),
            published_at: None,
            assets: std::collections::BTreeMap::new(),
        };
        let v = release_views(vec![rel("99.0.0"), rel(crate::VERSION)]);
        assert_eq!(v.iter().map(|r| r.current).collect::<Vec<_>>(), [false, true]);
    }
}
