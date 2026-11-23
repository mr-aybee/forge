//! Package updates: what is installed, which newer build the node's package
//! repositories offer, and what the vendor has released (DECISIONS.md D-035).
//!
//! * `GET  /api/v1/package-updates` — one row per installation that came
//!   from a package repository, with the installed, candidate and running
//!   builds, the update kind and the vendor's release line (LTS, latest, EOL).
//! * `GET  /api/v1/package-releases` — vendor release lines (LTS, latest, EOL).
//! * `POST /api/v1/package-updates/check` — ask every node for fresh facts.
//! * `POST /api/v1/installations/{id}/update` — `{build}`: upgrade to exactly
//!   that build (it must be the node's current candidate).
//! * `PUT  /api/v1/installations/{id}/policy` — `{update_policy}`: `notify` or
//!   `auto_patch` (patch and revision updates applied automatically).
//!
//! A background loop refreshes vendor release lines daily (endoflife.date)
//! and asks nodes for their builds every six hours, as the principal of each
//! installation's last job (the job engine re-authorizes every step).

use std::collections::BTreeMap;
use std::time::Duration;

use axum::Json;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use forge_catalog::versions::{UpdateKind, classify, compare, numbers};
use forge_core::model::{AuditEvent, AuditOutcome, Installation, Job, PackageRelease};
use forge_core::ports::{InstallationRepository, JobRepository, SettingsRepository};
use forge_core::{Action, InstallationId, ServerId, Timestamp};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use tokio::sync::watch;

use super::packages::{server_ref, submit_as};
use crate::auth::Caller;
use crate::error::{ApiError, ApiResult};
use crate::state::AppState;

/// How often nodes are asked for their builds.
const CHECK_EVERY: Duration = Duration::from_secs(6 * 3600);
/// How old vendor release data may get before it is fetched again.
const RELEASES_MAX_AGE_MS: i64 = 20 * 3600 * 1000;
const RELEASES_URL: &str = "https://endoflife.date/api";

/// The vendor release line an installation belongs to.
#[derive(Debug, Clone, Serialize)]
pub struct Upstream {
    pub line: String,
    pub latest: String,
    pub lts: bool,
    pub eol: Option<String>,
    pub support_until: Option<String>,
    /// The newest line the vendor ships, and whether it is LTS.
    pub newest_line: Option<String>,
    /// The vendor's latest is newer than anything this node's repositories offer.
    pub ahead_of_node: bool,
}

#[derive(Debug, Clone, Serialize)]
pub struct UpdateRow {
    pub installation_id: InstallationId,
    pub server_id: ServerId,
    pub package: String,
    pub name: &'static str,
    /// The release line chosen at install (`17`, `distro`, …).
    pub line: String,
    pub status: String,
    pub state: Option<String>,
    pub manager: String,
    pub os_package: String,
    pub installed: String,
    pub candidate: String,
    pub running: String,
    pub checked_at: Option<Timestamp>,
    /// Size of the update the node offers (`None`: up to date).
    pub kind: Option<UpdateKind>,
    pub update_policy: String,
    pub release_notes: &'static str,
    pub upstream: Option<Upstream>,
}

/// Finds the vendor line for an installation: its catalog line (`17`,
/// `8.3`), else the line of the installed build (`7.2` or `7`).
fn line_for<'a>(releases: &'a [PackageRelease], catalog_line: &str, installed: &str) -> Option<&'a PackageRelease> {
    if let Some(r) = releases.iter().find(|r| r.line == catalog_line) {
        return Some(r);
    }
    let n = numbers(installed);
    let two = n.iter().take(2).map(u64::to_string).collect::<Vec<_>>().join(".");
    let one = n.first().map(u64::to_string).unwrap_or_default();
    releases.iter().find(|r| r.line == two).or_else(|| releases.iter().find(|r| r.line == one))
}

fn upstream_for(releases: &[PackageRelease], i: &Installation) -> Option<Upstream> {
    let v = i.versions.as_ref()?;
    let r = line_for(releases, &i.version, &v.installed)?;
    // endoflife.date lists the newest line first.
    let newest = releases.first().filter(|n| n.line != r.line).map(|n| n.line.clone());
    let best_node = if v.candidate.is_empty() { &v.installed } else { &v.candidate };
    let ahead = !best_node.is_empty() && {
        let (vendor, node) = (numbers(&r.latest), numbers(best_node));
        !vendor.is_empty() && vendor > node[..node.len().min(vendor.len())].to_vec()
    };
    Some(Upstream {
        line: r.line.clone(),
        latest: r.latest.clone(),
        lts: r.lts,
        eol: r.eol.clone(),
        support_until: r.support_until.clone(),
        newest_line: newest,
        ahead_of_node: ahead,
    })
}

fn row(i: &Installation, releases: &BTreeMap<String, Vec<PackageRelease>>) -> Option<UpdateRow> {
    let pkg = forge_catalog::find(&i.package)?;
    if !forge_catalog::updatable(pkg) {
        return None;
    }
    let v = i.versions.clone().unwrap_or_else(|| forge_core::model::PackageVersions {
        manager: String::new(),
        package: String::new(),
        installed: String::new(),
        candidate: String::new(),
        running: String::new(),
        checked_at: Timestamp(0),
    });
    let kind =
        (!v.installed.is_empty() && !v.candidate.is_empty()).then(|| classify(&v.installed, &v.candidate)).flatten();
    let upstream = releases.get(pkg.upstream).and_then(|r| upstream_for(r, i));
    Some(UpdateRow {
        installation_id: i.id,
        server_id: i.server_id,
        package: i.package.clone(),
        name: pkg.name,
        line: i.version.clone(),
        status: i.status.clone(),
        state: i.state.clone(),
        manager: v.manager,
        os_package: v.package,
        installed: v.installed,
        candidate: v.candidate,
        running: v.running,
        checked_at: (v.checked_at.0 > 0).then_some(v.checked_at),
        kind,
        update_policy: i.update_policy.clone(),
        release_notes: pkg.release_notes,
        upstream,
    })
}

async fn releases_by_product(state: &AppState) -> BTreeMap<String, Vec<PackageRelease>> {
    let mut by: BTreeMap<String, Vec<PackageRelease>> = BTreeMap::new();
    for r in state.store.list_releases().await.unwrap_or_default() {
        by.entry(r.product.clone()).or_default().push(r);
    }
    // Newest line first (the store orders by text).
    for list in by.values_mut() {
        list.sort_by(|a, b| compare(&b.line, &a.line));
    }
    by
}

/// Vendor release lines per catalog product (public vendor data).
pub async fn releases(State(state): State<AppState>, _caller: Caller) -> ApiResult<Json<Vec<PackageRelease>>> {
    Ok(Json(state.store.list_releases().await?))
}

pub async fn list(State(state): State<AppState>, caller: Caller) -> ApiResult<Json<Vec<UpdateRow>>> {
    let all = state.store.list_installations(None).await?;
    let allowed = caller.can_many(Action::View, all.iter().map(|i| server_ref(i.server_id)).collect()).await;
    let releases = releases_by_product(&state).await;
    Ok(Json(all.iter().zip(allowed).filter(|(_, ok)| *ok).filter_map(|(i, _)| row(i, &releases)).collect()))
}

fn checkable(i: &Installation) -> bool {
    i.status == "installed" && forge_catalog::find(&i.package).is_some_and(forge_catalog::updatable)
}

/// Asks every node the caller manages for fresh build facts.
pub async fn check(State(state): State<AppState>, caller: Caller) -> ApiResult<(StatusCode, Json<Value>)> {
    let all: Vec<Installation> = state.store.list_installations(None).await?.into_iter().filter(checkable).collect();
    let allowed = caller.can_many(Action::ServerManage, all.iter().map(|i| server_ref(i.server_id)).collect()).await;
    let mut jobs = 0;
    for (i, _) in all.iter().zip(allowed).filter(|(_, ok)| *ok) {
        submit_as(&state, caller.principal(), caller.ctx(), i, "versions", None).await?;
        jobs += 1;
    }
    tokio::spawn(refresh_releases(state.clone(), false));
    Ok((StatusCode::ACCEPTED, Json(json!({ "checks": jobs }))))
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct UpdateRequest {
    /// The exact build to install: the candidate the node reported.
    pub build: String,
}

pub async fn update(
    State(state): State<AppState>,
    caller: Caller,
    Path(id): Path<InstallationId>,
    Json(req): Json<UpdateRequest>,
) -> ApiResult<(StatusCode, Json<Job>)> {
    let inst = state.store.get_installation(id).await?.ok_or_else(|| ApiError::not_found("installation not found"))?;
    caller.authorize(Action::PackageManage, server_ref(inst.server_id)).await?;
    let job = start_update(&state, caller.principal(), caller.ctx(), &inst, &req.build).await?;
    caller
        .audit(
            "package.update",
            &server_ref(inst.server_id),
            Some(inst.workspace_id),
            json!({
                "package": inst.package,
                "version": inst.version,
                "from": inst.versions.as_ref().map(|v| v.installed.clone()),
                "to": req.build,
                "job_id": job.id,
            }),
        )
        .await;
    Ok((StatusCode::ACCEPTED, Json(job)))
}

/// Pins `build` (which must be the node's reported candidate) and queues
/// the upgrade; a retry installs the same build.
async fn start_update(
    state: &AppState,
    principal: forge_core::Principal,
    context: forge_core::AuthzContext,
    inst: &Installation,
    build: &str,
) -> ApiResult<Job> {
    forge_catalog::validate(&inst.package, &inst.version, "upgrade", &BTreeMap::new())
        .map_err(ApiError::bad_request)?;
    if !forge_catalog::versions::valid_build(build) {
        return Err(ApiError::bad_request("invalid build"));
    }
    let Some(v) = inst.versions.as_ref() else {
        return Err(ApiError::conflict("check for updates first"));
    };
    if v.candidate != build {
        return Err(ApiError::conflict(format!(
            "{} offers {} on this node, not {build}; check for updates again",
            v.package,
            if v.candidate.is_empty() { "nothing" } else { &v.candidate }
        )));
    }
    if classify(&v.installed, build).is_none() {
        return Err(ApiError::conflict(format!("{build} is not newer than the installed {}", v.installed)));
    }
    if inst.status.ends_with("ing") {
        return Err(ApiError::conflict(format!("{} is {}; wait for it to finish", inst.package, inst.status)));
    }
    let mut busy = inst.clone();
    busy.status = "updating".into();
    busy.updated_at = state.clock.now();
    state.store.put_installation(&busy).await?;
    submit_as(state, principal, context, inst, "upgrade", Some(build)).await
}

/// Setting that records who turned `auto_patch` on for an installation:
/// automatic updates run as that person (with their current roles), never
/// as whoever happened to run the installation's last job (#13 inventory).
fn auto_patch_key(id: InstallationId) -> String {
    format!("packages.auto_patch_by.{id}")
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct PolicyRequest {
    pub update_policy: String,
}

pub async fn set_policy(
    State(state): State<AppState>,
    caller: Caller,
    Path(id): Path<InstallationId>,
    Json(req): Json<PolicyRequest>,
) -> ApiResult<Json<Installation>> {
    if !matches!(req.update_policy.as_str(), "notify" | "auto_patch") {
        return Err(ApiError::bad_request("update_policy must be `notify` or `auto_patch`"));
    }
    let mut inst =
        state.store.get_installation(id).await?.ok_or_else(|| ApiError::not_found("installation not found"))?;
    caller.authorize(Action::PackageManage, server_ref(inst.server_id)).await?;
    if req.update_policy == "auto_patch" {
        let by = super::fabric_reconcile::AppliedBy {
            principal: caller.principal(),
            context: caller.ctx(),
            at: state.clock.now(),
        };
        let v = serde_json::to_string(&by).map_err(forge_core::DomainError::internal)?;
        state.store.put_setting(&auto_patch_key(inst.id), &v, state.clock.now()).await?;
    } else {
        state.store.delete_setting(&auto_patch_key(inst.id)).await?;
    }
    inst.update_policy = req.update_policy;
    inst.updated_at = state.clock.now();
    state.store.put_installation(&inst).await?;
    caller
        .audit(
            "package.update_policy",
            &server_ref(inst.server_id),
            Some(inst.workspace_id),
            json!({ "package": inst.package, "version": inst.version, "update_policy": inst.update_policy }),
        )
        .await;
    Ok(Json(inst))
}

// ------------------------------------------------------------------ vendor data

/// A string, or a boolean rendered as `true`/`false`.
fn text_or_bool(v: &Value) -> Option<String> {
    match v {
        Value::String(s) => Some(s.clone()),
        Value::Bool(b) => Some(b.to_string()),
        _ => None,
    }
}

/// Parses an endoflife.date product document.
#[must_use]
pub fn parse_releases(product: &str, doc: &Value, now: Timestamp) -> Vec<PackageRelease> {
    doc.as_array()
        .map(|cycles| {
            cycles
                .iter()
                .filter_map(|c| {
                    let line = match &c["cycle"] {
                        Value::String(s) => s.clone(),
                        Value::Number(n) => n.to_string(),
                        _ => return None,
                    };
                    Some(PackageRelease {
                        product: product.to_owned(),
                        line,
                        latest: text_or_bool(&c["latest"]).unwrap_or_default(),
                        // `lts` is `true`, `false` or the date the line became LTS.
                        lts: matches!(&c["lts"], Value::Bool(true) | Value::String(_)),
                        released_at: text_or_bool(&c["releaseDate"]),
                        latest_at: text_or_bool(&c["latestReleaseDate"]),
                        eol: text_or_bool(&c["eol"]),
                        support_until: text_or_bool(&c["support"]),
                        fetched_at: now,
                    })
                })
                .take(64)
                .collect()
        })
        .unwrap_or_default()
}

/// Fetches vendor release lines for every catalog product (skipping fresh ones
/// unless `force`). Failures keep the previous data.
pub(crate) async fn refresh_releases(state: AppState, force: bool) {
    let now = state.clock.now();
    let stored = releases_by_product(&state).await;
    let Ok(client) = reqwest::Client::builder()
        .timeout(Duration::from_secs(20))
        .user_agent(concat!("orbyn-forge/", env!("CARGO_PKG_VERSION")))
        .build()
    else {
        return;
    };
    let mut products: Vec<&str> = forge_catalog::CATALOG.iter().map(|p| p.upstream).filter(|u| !u.is_empty()).collect();
    products.sort_unstable();
    products.dedup();
    for product in products {
        let fresh =
            stored.get(product).and_then(|r| r.first()).is_some_and(|r| now.0 - r.fetched_at.0 < RELEASES_MAX_AGE_MS);
        if fresh && !force {
            continue;
        }
        let doc = match client.get(format!("{RELEASES_URL}/{product}.json")).send().await {
            Ok(r) if r.status().is_success() => r.json::<Value>().await.ok(),
            Ok(r) => {
                tracing::warn!(product, status = %r.status(), "release data unavailable");
                None
            }
            Err(e) => {
                // reqwest's Display hides the cause (DNS, TLS roots, …).
                let mut chain = e.to_string();
                let mut src = std::error::Error::source(&e);
                while let Some(s) = src {
                    chain.push_str(": ");
                    chain.push_str(&s.to_string());
                    src = s.source();
                }
                tracing::warn!(product, error = %chain, "release data unavailable (outbound HTTPS needs the system CA bundle, e.g. ca-certificates)");
                None
            }
        };
        let releases = doc.map(|d| parse_releases(product, &d, now)).unwrap_or_default();
        if !releases.is_empty()
            && let Err(e) = state.store.put_releases(product, &releases).await
        {
            tracing::warn!(product, error = %e, "cannot store release data");
        }
    }
}

// ------------------------------------------------------------------ loop

/// One pass: refresh vendor data, apply due automatic patches, then ask every
/// node for fresh facts (picked up by the next pass).
async fn pass(state: &AppState) {
    refresh_releases(state.clone(), false).await;
    let Ok(all) = state.store.list_installations(None).await else { return };
    let enabled_by: BTreeMap<String, String> =
        state.store.all_settings().await.unwrap_or_default().into_iter().collect();
    for i in all.iter().filter(|i| checkable(i)) {
        let Some(last) = i.last_job_id else { continue };
        let Ok(job) = state.store.get_job(last).await else { continue };
        let (principal, context) = (job.principal.clone(), job.context);
        let auto = i.update_policy == "auto_patch"
            && i.versions
                .as_ref()
                .and_then(|v| classify(&v.installed, &v.candidate))
                .is_some_and(|k| matches!(k, UpdateKind::Patch | UpdateKind::Revision));
        let owner = enabled_by
            .get(&auto_patch_key(i.id))
            .and_then(|v| serde_json::from_str::<super::fabric_reconcile::AppliedBy>(v).ok());
        if auto && owner.is_none() {
            tracing::warn!(installation = %i.id, "auto_patch has no recorded owner: set the update policy again");
        }
        // The person who enabled it must still exist and be enabled.
        let owner = match owner {
            Some(o) if auto => {
                let forge_core::Principal::User(uid) = &o.principal;
                match forge_core::ports::UserRepository::get_user(&state.store, *uid).await {
                    Ok(u) if !u.disabled => Some(o),
                    found => {
                        let why = if found.is_ok() { "disabled" } else { "removed" };
                        let ev = AuditEvent::new(
                            "system:package-updates",
                            "package.update",
                            server_ref(i.server_id),
                            AuditOutcome::Denied,
                        )
                        .in_org(Some(i.workspace_id))
                        .with_details(json!({
                            "package": i.package,
                            "policy": "auto_patch",
                            "on_behalf_of": o.principal.to_string(),
                            "reason": format!("the person who enabled auto_patch was {why}: set the policy again"),
                        }));
                        if let Err(e) = state.audit.append(ev).await {
                            tracing::error!(error = %e, "audit write failed");
                        }
                        None
                    }
                }
            }
            o => o,
        };
        if auto
            && let Some(owner) = owner
            && let Some(build) = i.versions.as_ref().map(|v| v.candidate.clone())
        {
            let principal = owner.principal;
            let outcome = start_update(state, principal.clone(), owner.context, i, &build).await;
            let ev = AuditEvent::new(
                "system:package-updates",
                "package.update",
                server_ref(i.server_id),
                if outcome.is_ok() { AuditOutcome::Success } else { AuditOutcome::Failure },
            )
            .in_org(Some(i.workspace_id))
            .with_details(json!({
                "package": i.package,
                "version": i.version,
                "to": build,
                "policy": "auto_patch",
                "on_behalf_of": principal.to_string(),
                "error": outcome.as_ref().err().map(|e| e.message.clone()),
            }));
            if let Err(e) = state.audit.append(ev).await {
                tracing::error!(error = %e, "audit write failed");
            }
            continue;
        }
        if let Err(e) = submit_as(state, principal, context, i, "versions", None).await {
            tracing::warn!(installation = %i.id, error = %e.message, "cannot queue a version check");
        }
    }
}

pub(crate) async fn run(state: AppState, mut shutdown: watch::Receiver<bool>) {
    let mut wait = Duration::from_secs(90);
    loop {
        tokio::select! {
            () = tokio::time::sleep(wait) => {}
            _ = shutdown.changed() => break,
        }
        pass(&state).await;
        wait = CHECK_EVERY;
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    #[test]
    fn parses_endoflife_documents() {
        let doc = json!([
            {"cycle": "17", "releaseDate": "2024-09-26", "eol": "2029-11-08", "latest": "17.7",
             "latestReleaseDate": "2025-11-13", "lts": false},
            {"cycle": "22", "lts": "2024-10-29", "eol": "2027-04-30", "latest": "22.21.1", "support": "2025-10-21"},
            {"cycle": 8, "eol": true, "latest": "8.0.4"}
        ]);
        let r = parse_releases("x", &doc, Timestamp(1));
        assert_eq!(r.len(), 3);
        assert_eq!(r[0].latest, "17.7");
        assert!(!r[0].lts);
        assert!(r[1].lts);
        assert_eq!(r[1].support_until.as_deref(), Some("2025-10-21"));
        assert_eq!(r[2].line, "8");
        assert_eq!(r[2].eol.as_deref(), Some("true"));
    }

    fn rel(line: &str, latest: &str) -> PackageRelease {
        PackageRelease {
            product: "p".into(),
            line: line.into(),
            latest: latest.into(),
            lts: false,
            released_at: None,
            latest_at: None,
            eol: None,
            support_until: None,
            fetched_at: Timestamp(0),
        }
    }

    #[test]
    fn matches_release_lines() {
        let rs = vec![rel("8.0", "8.0.2"), rel("7.4", "7.4.6"), rel("7.2", "7.2.11"), rel("17", "17.7")];
        assert_eq!(line_for(&rs, "distro", "5:7.2.4-1").unwrap().line, "7.2");
        assert_eq!(line_for(&rs, "17", "17.6-1").unwrap().line, "17");
        assert!(line_for(&rs, "distro", "6.0.1").is_none());
    }
}
