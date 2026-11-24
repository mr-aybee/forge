//! Maintenance loops.

use std::time::Duration;

use forge_core::model::AgentStatus;
use forge_core::ports::{SecretRepository, ServerRepository};
use tokio::sync::watch;

use crate::events::ServerEvent;
use crate::state::AppState;

/// Marks agents that stopped long-polling as offline.
pub(crate) async fn offline_monitor(state: AppState, mut shutdown: watch::Receiver<bool>) {
    let after = Duration::from_secs(u64::from(state.config.offline_after_secs));
    let mut tick = tokio::time::interval(Duration::from_secs(10).min(after));
    let mut last_upload_sweep = tokio::time::Instant::now();
    loop {
        tokio::select! {
            _ = tick.tick() => {}
            _ = shutdown.changed() => break,
        }
        match state.store.mark_stale_offline(state.clock.now().minus(after)).await {
            Ok(ids) => {
                for id in ids {
                    tracing::info!(server = %id, "agent offline");
                    state.publish(ServerEvent::server(id, AgentStatus::Offline));
                    // Its health checks read unknown now, not after freshness runs out.
                    if let Err(e) = state.health.expire_node(id).await {
                        tracing::warn!(server = %id, error = %e, "marking health checks unknown failed");
                    }
                }
            }
            Err(e) => tracing::warn!(error = %e, "offline check failed"),
        }
        match state.store.purge_expired(state.clock.now()).await {
            Ok(0) => {}
            Ok(n) => tracing::info!(purged = n, "expired credentials purged from the vault"),
            Err(e) => tracing::warn!(error = %e, "secret purge failed"),
        }
        // Observations whose freshness ran out read as unknown (D-085).
        match state.health.sweep_stale().await {
            Ok(t) if !t.is_empty() => tracing::debug!(count = t.len(), "observations went stale"),
            Ok(_) => {}
            Err(e) => tracing::warn!(error = %e, "observation staleness sweep failed"),
        }
        // Abandoned file uploads: their sessions and partial files on the nodes.
        if last_upload_sweep.elapsed() >= Duration::from_secs(300) {
            last_upload_sweep = tokio::time::Instant::now();
            sweep_observations(&state).await;
            let n = state.files.cleanup_uploads().await;
            if n > 0 {
                tracing::info!(removed = n, "expired file uploads removed");
            }
        }
    }
}

/// How often the control-plane key is checked for renewal and rotation.
const AGENT_KEY_TICK: Duration = Duration::from_secs(60);

/// Renews the agent listener certificate with the same key, switches to the
/// next key when the rotation is ready, and picks up a switch made with
/// `forge-server agent-key rotate` (D-167). A failed tick is logged and
/// tried again at the next.
pub(crate) async fn agent_key_upkeep(state: AppState, mut shutdown: watch::Receiver<bool>) {
    let mut tick = tokio::time::interval(AGENT_KEY_TICK);
    tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    loop {
        tokio::select! {
            _ = tick.tick() => {}
            _ = shutdown.changed() => break,
        }
        match state.agent_keys.tick(state.clock.now()).await {
            Ok(out) => {
                if out.renewed {
                    tracing::info!(key = %state.agent_keys.current_fingerprint(), "agent listener certificate renewed (same key)");
                }
                if let Some(r) = out.rotated {
                    state.audit_system(crate::agent_keys::AUDIT_KEY_ROTATE, serde_json::json!(r)).await;
                }
            }
            Err(e) => tracing::warn!(error = %e, "agent key upkeep failed; trying again"),
        }
    }
}

/// How long observation history is kept.
const OBSERVATION_HISTORY_MS: i64 = 7 * 24 * 3_600_000;
/// Transitions kept in the feed (consumers further behind lose the oldest).
const OBSERVATION_TRANSITIONS_KEPT: i64 = 100_000;

async fn sweep_observations(state: &AppState) {
    use forge_core::observation::{ObservationRetention, TransitionFeed};
    let head = match state.store.head().await {
        Ok(h) => h,
        Err(e) => return tracing::warn!(error = %e, "observation retention failed"),
    };
    let before = forge_core::Timestamp(state.clock.now().0 - OBSERVATION_HISTORY_MS);
    match state.store.sweep(before, head - OBSERVATION_TRANSITIONS_KEPT).await {
        Ok(0) => {}
        Ok(n) => tracing::info!(removed = n, "old observations removed"),
        Err(e) => tracing::warn!(error = %e, "observation retention failed"),
    }
    // Current values and history of deleted servers and services.
    match state.store.delete_orphans().await {
        Ok(0) => {}
        Ok(n) => tracing::info!(removed = n, "observations of deleted resources removed"),
        Err(e) => tracing::warn!(error = %e, "observation orphan cleanup failed"),
    }
}
