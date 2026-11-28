//! Plugin lifecycle endpoints. Authorization and audit happen inside the
//! plugin host, so every caller (API, CLI, future catalog sync) is covered.

use axum::Json;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use base64::Engine;
use base64::engine::general_purpose::STANDARD;
use forge_core::model::PluginRecord;
use forge_core::{Action, EntityRef};
use forge_plugin_host::{InvokeOutcome, PluginPackage};
use serde::Deserialize;

use crate::auth::Caller;
use crate::error::{ApiError, ApiResult};
use crate::state::AppState;

pub async fn list(State(state): State<AppState>, caller: Caller) -> ApiResult<Json<Vec<PluginRecord>>> {
    caller.authorize(Action::View, EntityRef::platform()).await?;
    Ok(Json(state.plugins.list().await?))
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct PackageUpload {
    pub manifest_toml: String,
    /// The component (binary `.wasm` or `.wat` text), base64.
    pub artifact_base64: String,
    pub signature: String,
    /// Permissions the administrator approves (subset of requested).
    #[serde(default)]
    pub approved_permissions: Vec<String>,
}

impl PackageUpload {
    fn into_parts(self) -> ApiResult<(PluginPackage, Vec<String>)> {
        let artifact = STANDARD
            .decode(self.artifact_base64.trim())
            .map_err(|_| ApiError::bad_request("artifact_base64 is not base64"))?;
        Ok((
            PluginPackage { manifest_toml: self.manifest_toml, artifact, signature: self.signature },
            self.approved_permissions,
        ))
    }
}

pub async fn install(
    State(state): State<AppState>,
    caller: Caller,
    Json(req): Json<PackageUpload>,
) -> ApiResult<(StatusCode, Json<PluginRecord>)> {
    let (pkg, approved) = req.into_parts()?;
    let r = state.plugins.install(&caller.principal(), caller.ctx(), pkg, approved).await?;

    Ok((StatusCode::CREATED, Json(r)))
}

pub async fn upgrade(
    State(state): State<AppState>,
    caller: Caller,
    Json(req): Json<PackageUpload>,
) -> ApiResult<Json<PluginRecord>> {
    let (pkg, approved) = req.into_parts()?;
    Ok(Json(state.plugins.upgrade(&caller.principal(), caller.ctx(), pkg, approved).await?))
}

pub async fn enable(
    State(state): State<AppState>,
    caller: Caller,
    Path(id): Path<String>,
) -> ApiResult<Json<PluginRecord>> {
    Ok(Json(state.plugins.enable(&caller.principal(), caller.ctx(), &id).await?))
}

pub async fn disable(
    State(state): State<AppState>,
    caller: Caller,
    Path(id): Path<String>,
) -> ApiResult<Json<PluginRecord>> {
    Ok(Json(state.plugins.disable(&caller.principal(), caller.ctx(), &id).await?))
}

pub async fn pin(
    State(state): State<AppState>,
    caller: Caller,
    Path(id): Path<String>,
) -> ApiResult<Json<PluginRecord>> {
    Ok(Json(state.plugins.set_pinned(&caller.principal(), caller.ctx(), &id, true).await?))
}

pub async fn unpin(
    State(state): State<AppState>,
    caller: Caller,
    Path(id): Path<String>,
) -> ApiResult<Json<PluginRecord>> {
    Ok(Json(state.plugins.set_pinned(&caller.principal(), caller.ctx(), &id, false).await?))
}

pub async fn uninstall(State(state): State<AppState>, caller: Caller, Path(id): Path<String>) -> ApiResult<StatusCode> {
    state.plugins.uninstall(&caller.principal(), caller.ctx(), &id).await?;
    Ok(StatusCode::NO_CONTENT)
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct InvokeRequest {
    /// Scope the invocation acts in, e.g. `workspace:<id>` (default: platform).
    #[serde(default)]
    pub scope: Option<String>,
    #[serde(default)]
    pub input: u32,
}

pub async fn invoke(
    State(state): State<AppState>,
    caller: Caller,
    Path(id): Path<String>,
    Json(req): Json<InvokeRequest>,
) -> ApiResult<Json<InvokeOutcome>> {
    let scope = match req.scope {
        Some(s) => s.parse().map_err(ApiError::bad_request)?,
        None => EntityRef::platform(),
    };

    Ok(Json(state.plugins.invoke(&caller.principal(), caller.ctx(), &id, scope, req.input).await?))
}
