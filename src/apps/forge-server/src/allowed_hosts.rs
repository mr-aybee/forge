//! The `server.allowed_hosts` runtime setting (D-167): load at start and
//! save from the API and CLI. Matching lives in
//! `forge_core::allowed_hosts`; the 421 guard in `crate::request_origin`.

use forge_core::allowed_hosts::{AllowedHosts, SETTING_KEY, parse_list};
use forge_core::ports::SettingsRepository;
use forge_core::{DomainError, DomainResult, Timestamp};

/// Reads the setting at start. A stored value that no longer parses is
/// ignored with a warning (any host is allowed, the default).
pub async fn load(settings: &dyn SettingsRepository) -> anyhow::Result<AllowedHosts> {
    let hosts = AllowedHosts::default();
    let stored = settings.all_settings().await?.into_iter().find(|(k, _)| k == SETTING_KEY).map(|(_, v)| v);
    if let Some(v) = stored {
        let parsed = serde_json::from_str::<Vec<String>>(&v)
            .map_err(|e| e.to_string())
            .and_then(|l| parse_list(&l).map_err(|e| e.to_string()));
        match parsed {
            Ok(p) => hosts.set(Some(p)),
            Err(e) => tracing::warn!(error = %e, "ignoring the stored allowed hosts; any host is allowed"),
        }
    }
    Ok(hosts)
}

/// Stores (or with `None` clears back to `*`) the list and applies it.
/// Store first, then memory: a failed write leaves both unchanged.
pub async fn save(
    settings: &dyn SettingsRepository,
    hosts: &AllowedHosts,
    value: Option<&[String]>,
    now: Timestamp,
) -> DomainResult<()> {
    match value {
        Some(list) => {
            let patterns = parse_list(list).map_err(|e| DomainError::Invalid(e.to_string()))?;
            let text: Vec<String> = patterns.iter().map(ToString::to_string).collect();
            let json = serde_json::to_string(&text).map_err(|e| DomainError::Internal(e.to_string()))?;
            settings.put_setting(SETTING_KEY, &json, now).await?;
            hosts.set(Some(patterns));
        }
        None => {
            settings.delete_setting(SETTING_KEY).await?;
            hosts.set(None);
        }
    }
    Ok(())
}
