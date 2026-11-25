use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use forge_core::model::{AuditEvent, AuditOutcome, PluginRecord, PluginState};
use forge_core::ports::{AuditRecorder, Authorizer, PluginRepository};
use forge_core::{
    AccessRequest, Action, AuthzContext, Clock, DomainError, DomainResult, EntityKind, EntityRef, Principal,
};
use forge_sdk::manifest::Manifest;
use forge_sdk::runner_protocol::{FailureKind, RunRequest, RunnerMessage};
use forge_sdk::signing::TrustStore;
use serde::Serialize;
use tokio::sync::Semaphore;

use crate::functions::{ForgeHostFunctions, Invocation};
use crate::runner::PluginRunner;

/// An uploaded plugin package.
#[derive(Debug, Clone)]
pub struct PluginPackage {
    pub manifest_toml: String,
    pub artifact: Vec<u8>,
    /// Publisher signature (base64url) over manifest + artifact.
    pub signature: String,
}

/// Settings for the plugin host.
#[derive(Debug, Clone)]
pub struct PluginHostConfig {
    pub artifacts_dir: PathBuf,
    /// Plugin API version this host implements.
    pub api_version: String,
    /// Non-budget faults tolerated before automatic quarantine.
    pub fault_threshold: u32,
}

/// The result of one plugin invocation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct InvokeOutcome {
    pub output: u32,
    pub fuel_used: u64,
}

/// Plugin lifecycle and invocation service.
#[derive(Debug)]
pub struct PluginHost {
    repo: Arc<dyn PluginRepository>,
    authz: Arc<dyn Authorizer>,
    audit: Arc<dyn AuditRecorder>,
    runner: Arc<dyn PluginRunner>,
    functions: ForgeHostFunctions,
    trust: TrustStore,
    clock: Arc<dyn Clock>,
    config: PluginHostConfig,
    concurrency: Mutex<HashMap<String, Arc<Semaphore>>>,
}

fn plugin_ref(id: &str) -> EntityRef {
    EntityRef::new(EntityKind::Plugin, id)
}

fn invalid(msg: impl Into<String>) -> DomainError {
    DomainError::Invalid(msg.into())
}

impl PluginHost {
    /// Creates the plugin host.
    #[allow(clippy::too_many_arguments)]
    #[must_use]
    pub fn new(
        repo: Arc<dyn PluginRepository>,
        authz: Arc<dyn Authorizer>,
        audit: Arc<dyn AuditRecorder>,
        runner: Arc<dyn PluginRunner>,
        functions: ForgeHostFunctions,
        trust: TrustStore,
        clock: Arc<dyn Clock>,
        config: PluginHostConfig,
    ) -> Self {
        Self { repo, authz, audit, runner, functions, trust, clock, config, concurrency: Mutex::new(HashMap::new()) }
    }

    /// Lists installed plugins.
    pub async fn list(&self) -> DomainResult<Vec<PluginRecord>> {
        self.repo.list_plugins().await
    }

    /// One installed plugin by id.
    pub async fn get(&self, id: &str) -> DomainResult<PluginRecord> {
        self.repo.get_plugin(id).await?.ok_or_else(|| DomainError::NotFound(format!("plugin {id}")))
    }

    // ------------------------------------------------------------ lifecycle

    /// Verifies and installs a package. `approved` must be a subset of the
    /// manifest's requested permissions; anything not approved stays denied.
    pub async fn install(
        &self,
        principal: &Principal,
        ctx: AuthzContext,
        package: PluginPackage,
        approved: Vec<String>,
    ) -> DomainResult<PluginRecord> {
        self.require_manage(principal, EntityRef::platform(), ctx).await?;
        let manifest = self.verify_package(&package, &approved).await?;
        if self.repo.get_plugin(&manifest.plugin.id).await?.is_some() {
            return Err(DomainError::Conflict(format!("{} is already installed; use upgrade", manifest.plugin.id)));
        }

        let sha = self.store_artifact(&manifest, &package.artifact).await?;
        let now = self.clock.now();
        let record = PluginRecord {
            plugin_id: manifest.plugin.id.clone(),
            version: manifest.plugin.version.clone(),
            publisher: manifest.plugin.publisher.clone(),
            manifest_toml: package.manifest_toml,
            artifact_sha256: sha,
            state: PluginState::Installed,
            pinned: false,
            approved_permissions: approved,
            fault_count: 0,
            quarantine_reason: None,
            installed_at: now,
            updated_at: now,
        };
        self.repo.upsert_plugin(&record).await?;
        self.audit_ok(principal, "plugin.install", &record).await;

        Ok(record)
    }

    /// Enables a plugin.
    pub async fn enable(&self, principal: &Principal, ctx: AuthzContext, id: &str) -> DomainResult<PluginRecord> {
        self.transition(principal, ctx, id, "plugin.enable", |r| {
            if r.state == PluginState::Enabled {
                return Err(DomainError::Conflict("already enabled".into()));
            }

            r.state = PluginState::Enabled;
            r.fault_count = 0;
            r.quarantine_reason = None;
            Ok(())
        })
        .await
    }

    /// Disables a plugin.
    pub async fn disable(&self, principal: &Principal, ctx: AuthzContext, id: &str) -> DomainResult<PluginRecord> {
        self.transition(principal, ctx, id, "plugin.disable", |r| {
            if r.state != PluginState::Enabled {
                return Err(DomainError::Conflict("plugin is not enabled".into()));
            }

            r.state = PluginState::Disabled;
            Ok(())
        })
        .await
    }

    /// Pins or unpins a plugin.
    pub async fn set_pinned(
        &self,
        principal: &Principal,
        ctx: AuthzContext,
        id: &str,
        pinned: bool,
    ) -> DomainResult<PluginRecord> {
        let action = if pinned { "plugin.pin" } else { "plugin.unpin" };
        self.transition(principal, ctx, id, action, |r| {
            r.pinned = pinned;
            Ok(())
        })
        .await
    }

    /// Upgrades to a newer version of the same plugin. Pinned plugins refuse.
    pub async fn upgrade(
        &self,
        principal: &Principal,
        ctx: AuthzContext,
        package: PluginPackage,
        approved: Vec<String>,
    ) -> DomainResult<PluginRecord> {
        let manifest = Manifest::parse(&package.manifest_toml).map_err(|e| invalid(e.to_string()))?;
        let id = manifest.plugin.id.clone();
        self.require_manage(principal, plugin_ref(&id), ctx).await?;
        let mut current = self.get(&id).await?;
        if current.pinned {
            return Err(DomainError::Conflict(format!("{id} is pinned at {}", current.version)));
        }

        if manifest.plugin.publisher != current.publisher {
            return Err(DomainError::Denied(format!(
                "{id} is published by {}; an upgrade must come from the same publisher",
                current.publisher
            )));
        }

        let new_v = manifest.version().map_err(|e| invalid(e.to_string()))?;
        let old_v = semver::Version::parse(&current.version).map_err(DomainError::internal)?;
        if new_v <= old_v {
            return Err(invalid(format!("upgrade must increase version ({old_v} → {new_v})")));
        }

        let manifest = self.verify_package(&package, &approved).await?;
        let sha = self.store_artifact(&manifest, &package.artifact).await?;
        let old_version = std::mem::replace(&mut current.version, manifest.plugin.version.clone());
        current.manifest_toml = package.manifest_toml;
        current.artifact_sha256 = sha;
        current.approved_permissions = approved;
        current.updated_at = self.clock.now();
        self.repo.upsert_plugin(&current).await?;
        let _ = tokio::fs::remove_file(self.artifact_path(&id, &old_version)).await;
        self.audit_ok(principal, "plugin.upgrade", &current).await;

        Ok(current)
    }

    /// Removes the integration. Never touches infrastructure the plugin managed.
    pub async fn uninstall(&self, principal: &Principal, ctx: AuthzContext, id: &str) -> DomainResult<()> {
        self.require_manage(principal, plugin_ref(id), ctx).await?;
        let record = self.get(id).await?;
        if record.state == PluginState::Enabled {
            return Err(DomainError::Conflict("disable the plugin before uninstalling".into()));
        }

        let dependents: Vec<String> = self
            .repo
            .list_plugins()
            .await?
            .into_iter()
            .filter(|p| p.plugin_id != id)
            .filter(|p| Manifest::parse(&p.manifest_toml).is_ok_and(|m| m.dependencies.contains_key(id)))
            .map(|p| p.plugin_id)
            .collect();

        if !dependents.is_empty() {
            return Err(DomainError::Conflict(format!("required by: {}", dependents.join(", "))));
        }

        self.repo.delete_plugin(id).await?;
        let _ = tokio::fs::remove_dir_all(self.config.artifacts_dir.join(id)).await;
        self.audit_ok(principal, "plugin.uninstall", &record).await;

        Ok(())
    }

    // ------------------------------------------------------------ invocation

    /// Invokes an enabled plugin on behalf of `principal`, scoped to `scope`.
    pub async fn invoke(
        &self,
        principal: &Principal,
        ctx: AuthzContext,
        id: &str,
        scope: EntityRef,
        input: u32,
    ) -> DomainResult<InvokeOutcome> {
        self.authorize_audited(principal, Action::PluginInvoke, scope, ctx, "plugin.invoke").await?;
        let record = self.get(id).await?;
        if record.state != PluginState::Enabled {
            return Err(DomainError::Conflict(format!("plugin is {}", record.state.as_str())));
        }

        let manifest = Manifest::parse(&record.manifest_toml).map_err(DomainError::internal)?;
        let _permit = self.acquire_slot(id, manifest.budgets.concurrency)?;

        let path = self.artifact_path(id, &record.version);
        let bytes = tokio::fs::read(&path).await.map_err(DomainError::internal)?;
        if forge_secrets::crypto::sha256_hex(&bytes) != record.artifact_sha256 {
            self.quarantine(&record, "artifact on disk does not match installed digest").await?;
            return Err(DomainError::Denied("plugin artifact integrity check failed".into()));
        }

        let b = manifest.budgets;
        let req = RunRequest {
            artifact_path: path.display().to_string(),
            export: manifest.runtime.export.clone(),
            input,
            fuel: b.fuel,
            memory_bytes: u64::from(b.memory_mb) * 1024 * 1024,
            timeout_ms: b.timeout_ms,
            max_output_bytes: b.max_output_bytes,
            artifact_sha256: record.artifact_sha256.clone(),
        };
        let host = self.functions.bind(Invocation {
            plugin_id: id.to_owned(),
            principal: principal.clone(),
            context: ctx,
            approved: record.approved_permissions.iter().cloned().collect::<HashSet<_>>(),
        });

        match self.runner.run(req, &host).await? {
            RunnerMessage::Done { output, fuel_used } => {
                self.audit_ok(principal, "plugin.invoke", &record).await;
                Ok(InvokeOutcome { output, fuel_used })
            }
            RunnerMessage::Failed { kind, message } => {
                self.record_fault(&record, kind, &message).await?;
                Err(match kind {
                    FailureKind::Denied => DomainError::Denied(message),
                    k if k.is_budget_violation() => {
                        DomainError::Invalid(format!("plugin exceeded budget ({k:?}); quarantined"))
                    }
                    _ => DomainError::Invalid(format!("plugin failed: {message}")),
                })
            }
            RunnerMessage::HostCall { .. } => Err(DomainError::internal("runner protocol violation")),
        }
    }

    // ------------------------------------------------------------ internals

    async fn verify_package(&self, package: &PluginPackage, approved: &[String]) -> DomainResult<Manifest> {
        let manifest = Manifest::parse(&package.manifest_toml).map_err(|e| invalid(e.to_string()))?;
        if !manifest.supports_api(&self.config.api_version).map_err(|e| invalid(e.to_string()))? {
            return Err(invalid(format!(
                "plugin requires API {}, host provides {}",
                manifest.plugin.forge_api, self.config.api_version
            )));
        }

        self.trust
            .verify(&manifest.plugin.publisher, package.manifest_toml.as_bytes(), &package.artifact, &package.signature)
            .map_err(|e| DomainError::Denied(e.to_string()))?;
        let requested: HashSet<&String> = manifest.permissions.requested.iter().collect();
        if let Some(extra) = approved.iter().find(|p| !requested.contains(p)) {
            return Err(invalid(format!("cannot approve `{extra}`: not requested by the plugin")));
        }

        for (dep, req) in &manifest.dependencies {
            let installed =
                self.repo.get_plugin(dep).await?.ok_or_else(|| invalid(format!("missing dependency {dep}")))?;
            let req = semver::VersionReq::parse(req).map_err(|e| invalid(e.to_string()))?;
            let v = semver::Version::parse(&installed.version).map_err(DomainError::internal)?;
            if !req.matches(&v) {
                return Err(invalid(format!("dependency {dep} {v} does not satisfy {req}")));
            }
        }

        Ok(manifest)
    }

    fn artifact_path(&self, id: &str, version: &str) -> PathBuf {
        self.config.artifacts_dir.join(id).join(format!("{version}.wasm"))
    }

    async fn store_artifact(&self, manifest: &Manifest, bytes: &[u8]) -> DomainResult<String> {
        let path = self.artifact_path(&manifest.plugin.id, &manifest.plugin.version);
        if let Some(dir) = path.parent() {
            tokio::fs::create_dir_all(dir).await.map_err(DomainError::internal)?;
        }

        tokio::fs::write(&path, bytes).await.map_err(DomainError::internal)?;

        Ok(forge_secrets::crypto::sha256_hex(bytes))
    }

    async fn require_manage(&self, principal: &Principal, resource: EntityRef, ctx: AuthzContext) -> DomainResult<()> {
        self.authorize_audited(principal, Action::PluginManage, resource, ctx, "plugin.manage").await
    }

    /// Authorizes and records a denial in the audit log (CLAUDE.md rule 3).
    async fn authorize_audited(
        &self,
        principal: &Principal,
        action: Action,
        resource: EntityRef,
        ctx: AuthzContext,
        audit_action: &str,
    ) -> DomainResult<()> {
        let result = self.authz.authorize(&AccessRequest::new(principal.clone(), action, resource.clone(), ctx)).await;
        if let Err(e) = &result {
            let ev = AuditEvent::new(principal, audit_action, resource, AuditOutcome::Denied)
                .with_details(serde_json::json!({ "reason": e.to_string() }));

            if let Err(e) = self.audit.record(ev).await {
                tracing::error!(error = %e, "audit write failed");
            }
        }

        result
    }

    async fn transition(
        &self,
        principal: &Principal,
        ctx: AuthzContext,
        id: &str,
        action: &str,
        apply: impl FnOnce(&mut PluginRecord) -> DomainResult<()>,
    ) -> DomainResult<PluginRecord> {
        self.require_manage(principal, plugin_ref(id), ctx).await?;
        let mut record = self.get(id).await?;
        apply(&mut record)?;
        record.updated_at = self.clock.now();
        self.repo.upsert_plugin(&record).await?;
        self.audit_ok(principal, action, &record).await;

        Ok(record)
    }

    fn acquire_slot(&self, id: &str, limit: u32) -> DomainResult<tokio::sync::OwnedSemaphorePermit> {
        let sem = {
            let mut map = self.concurrency.lock().map_err(|_| DomainError::internal("lock poisoned"))?;
            map.entry(id.to_owned())
                .or_insert_with(|| Arc::new(Semaphore::new(usize::try_from(limit.max(1)).unwrap_or(1))))
                .clone()
        };

        sem.try_acquire_owned().map_err(|_| DomainError::Conflict("plugin concurrency limit reached".into()))
    }

    async fn record_fault(&self, record: &PluginRecord, kind: FailureKind, message: &str) -> DomainResult<()> {
        if kind == FailureKind::Denied {
            // A refused host call is the sandbox working, not a plugin fault.
            return Ok(());
        }

        let mut r = self.get(&record.plugin_id).await?;
        r.fault_count += 1;

        if kind.is_budget_violation() || r.fault_count >= self.config.fault_threshold {
            return self.quarantine(&r, &format!("{kind:?}: {message}")).await;
        }

        r.updated_at = self.clock.now();

        self.repo.upsert_plugin(&r).await
    }

    /// Stops the plugin from running until an administrator re-enables it.
    async fn quarantine(&self, record: &PluginRecord, reason: &str) -> DomainResult<()> {
        let mut r = record.clone();
        r.state = PluginState::Quarantined;
        r.quarantine_reason = Some(reason.chars().take(500).collect());
        r.updated_at = self.clock.now();
        self.repo.upsert_plugin(&r).await?;
        tracing::warn!(plugin = %r.plugin_id, reason, "plugin quarantined");
        let ev = AuditEvent::new("system", "plugin.quarantine", plugin_ref(&r.plugin_id), AuditOutcome::Success)
            .with_details(serde_json::json!({ "reason": r.quarantine_reason, "version": r.version }));

        if let Err(e) = self.audit.record(ev).await {
            tracing::error!(error = %e, "audit write failed");
        }

        Ok(())
    }

    async fn audit_ok(&self, principal: &Principal, action: &str, r: &PluginRecord) {
        let ev = AuditEvent::new(principal, action, plugin_ref(&r.plugin_id), AuditOutcome::Success).with_details(
            serde_json::json!({ "version": r.version, "state": r.state.as_str(), "approved": r.approved_permissions }),
        );

        if let Err(e) = self.audit.record(ev).await {
            tracing::error!(error = %e, "audit write failed");
        }
    }
}
