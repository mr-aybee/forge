//! End-to-end plugin tests: real runner process, real store and Cedar.
#![allow(clippy::unwrap_used)]

use std::path::PathBuf;
use std::sync::Arc;

use async_trait::async_trait;
use forge_authz::{CedarAuthorizer, PolicyEngine};
use forge_core::model::*;
use forge_core::ports::{AuditRecorder, ServerRepository, TenancyRepository};
use forge_core::*;
use forge_plugin_host::*;
use forge_sdk::signing::{PublisherKey, TrustStore};
use forge_store::Store;

const SAMPLE_MANIFEST: &str = include_str!("../../../../plugins/uptime-probe/forge-plugin.toml");
const SAMPLE_WAT: &str = include_str!("../../../../plugins/uptime-probe/uptime-probe.wat");

const LOOP_WAT: &str = r#"(component
  (core module $M (func (export "run") (param i32) (result i32) (loop $l (br $l)) (i32.const 0)))
  (core instance $i (instantiate $M))
  (func (export "run") (param "input" u32) (result u32) (canon lift (core func $i "run"))))"#;

const MEMORY_HOG_WAT: &str = r#"(component
  (core module $M
    (memory 1)
    (func (export "run") (param i32) (result i32) (drop (memory.grow (i32.const 2000))) (i32.const 0)))
  (core instance $i (instantiate $M))
  (func (export "run") (param "input" u32) (result u32) (canon lift (core func $i "run"))))"#;

/// Two memories of 40 pages (2.5 MiB) each: within a 4 MiB budget one by
/// one, over it together.
const TWO_MEMORIES_WAT: &str = r#"(component
  (core module $A (memory (export "m") 40))
  (core module $B
    (memory 40)
    (func (export "run") (param i32) (result i32) (i32.const 0)))
  (core instance $a (instantiate $A))
  (core instance $b (instantiate $B))
  (func (export "run") (param "input" u32) (result u32) (canon lift (core func $b "run"))))"#;

const WASI_WAT: &str = r#"(component
  (import "wasi:cli/environment@0.2.0" (instance (export "get-arguments" (func (result (list string))))))
  (core module $M (func (export "run") (param i32) (result i32) (i32.const 0)))
  (core instance $i (instantiate $M))
  (func (export "run") (param "input" u32) (result u32) (canon lift (core func $i "run"))))"#;

#[derive(Debug, Default)]
struct MemAudit(std::sync::Mutex<Vec<String>>);

#[async_trait]
impl AuditRecorder for MemAudit {
    async fn record(&self, e: AuditEvent) -> DomainResult<AuditEntry> {
        let label =
            if matches!(e.outcome, AuditOutcome::Denied) { format!("denied:{}", e.action) } else { e.action.clone() };
        self.0.lock().unwrap().push(label);

        Ok(AuditEntry {
            seq: 0,
            id: String::new(),
            at: Timestamp(0),
            actor: e.actor,
            action: e.action,
            resource: e.resource,
            workspace_id: e.workspace_id,
            outcome: e.outcome,
            details: e.details,
            prev_hash: String::new(),
            hash: String::new(),
            signature: String::new(),
        })
    }
}

struct Fx {
    host: PluginHost,
    key: PublisherKey,
    admin: Principal,
    operator: Principal,
    stranger: Principal,
    /// Developer bound to a second org that contains no servers.
    outsider: Principal,
    org: EntityRef,
    org2: EntityRef,
    audit: Arc<MemAudit>,
    _dir: tempfile::TempDir,
}

const MFA: AuthzContext = AuthzContext { mfa: true };

fn runner_bin() -> PathBuf {
    PathBuf::from(env!("CARGO_BIN_EXE_forge-plugin-runner"))
}

async fn fx() -> Fx {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::open_sqlite(&dir.path().join("p.db")).await.unwrap();
    let org = Workspace { id: WorkspaceId::new(), name: "o".into(), slug: "o".into(), created_at: Timestamp(0) };
    store.create_workspace(&org).await.unwrap();
    let org2 = Workspace { id: WorkspaceId::new(), name: "o2".into(), slug: "o2".into(), created_at: Timestamp(0) };
    store.create_workspace(&org2).await.unwrap();

    for (hostname, status) in [("web-01", AgentStatus::Online), ("db-01", AgentStatus::Offline)] {
        store
            .create_server(&Server {
                role: forge_core::model::NodeRole::Worker,
                display_name: None,
                id: ServerId::new(),
                workspace_id: org.id,
                environment_id: None,
                cluster_id: None,
                hostname: hostname.into(),
                os: "linux".into(),
                arch: "x86_64".into(),
                agent_version: "0".into(),
                status,
                last_seen_at: None,
                cert_serial: None,
                cert_not_after: None,
                drained: false,
                created_at: Timestamp(0),
            })
            .await
            .unwrap();
    }

    let engine = Arc::new(PolicyEngine::new().unwrap());
    let bind = |role, scope: EntityRef| {
        let u = UserId::new();
        engine
            .link_binding(&RoleBinding { id: RoleBindingId::new(), user_id: u, role, scope, created_at: Timestamp(0) })
            .unwrap();
        Principal::User(u)
    };
    let workspace_ref = EntityRef::new(EntityKind::Workspace, org.id);
    let admin = bind(Role::PlatformAdmin, EntityRef::platform());
    let operator = bind(Role::Operator, workspace_ref.clone());
    let stranger = Principal::User(UserId::new());
    let org2_ref = EntityRef::new(EntityKind::Workspace, org2.id);
    let outsider = bind(Role::Developer, org2_ref.clone());

    let authz = Arc::new(CedarAuthorizer::new(engine, Arc::new(store.clone())));
    let (key, _) = PublisherKey::generate().unwrap();
    let mut trust = TrustStore::new();
    trust.trust("orbyn", &key.public_key_b64()).unwrap();
    let audit = Arc::new(MemAudit::default());
    let host = PluginHost::new(
        Arc::new(store.clone()),
        authz.clone(),
        audit.clone(),
        Arc::new(ProcessRunner::new(runner_bin())),
        ForgeHostFunctions::new(Arc::new(store), authz),
        trust,
        Arc::new(SystemClock),
        PluginHostConfig { artifacts_dir: dir.path().join("plugins"), api_version: "0.1.0".into(), fault_threshold: 3 },
    );

    Fx { host, key, admin, operator, stranger, outsider, org: workspace_ref, org2: org2_ref, audit, _dir: dir }
}

fn package(key: &PublisherKey, manifest: &str, wat: &str) -> PluginPackage {
    PluginPackage {
        manifest_toml: manifest.to_owned(),
        artifact: wat.as_bytes().to_vec(),
        signature: key.sign(manifest.as_bytes(), wat.as_bytes()),
    }
}

fn all_requested() -> Vec<String> {
    vec!["host:log".into(), "host:server-online".into()]
}

async fn install_enabled(f: &Fx, manifest: &str, wat: &str, approved: Vec<String>) -> String {
    let r = f.host.install(&f.admin, MFA, package(&f.key, manifest, wat), approved).await.unwrap();
    f.host.enable(&f.admin, MFA, &r.plugin_id).await.unwrap();

    r.plugin_id
}

fn variant(id: &str) -> String {
    SAMPLE_MANIFEST.replace("orbyn.uptime-probe", id)
}

#[tokio::test]
async fn sample_signed_plugin_calls_typed_host_function() {
    let f = fx().await;
    let id = install_enabled(&f, SAMPLE_MANIFEST, SAMPLE_WAT, all_requested()).await;
    let out = f.host.invoke(&f.operator, AuthzContext::default(), &id, f.org.clone(), 0).await.unwrap();
    assert_eq!(out.output, 1, "web-01 is online and visible to the operator");
    assert!(out.fuel_used > 0);
    let actions = f.audit.0.lock().unwrap().clone();
    assert!(actions.contains(&"plugin.install".to_owned()) && actions.contains(&"plugin.enable".to_owned()));
    assert!(actions.contains(&"plugin.invoke".to_owned()), "invocations are audited: {actions:?}");
}

#[tokio::test]
async fn tampered_or_unsigned_package_is_rejected() {
    let f = fx().await;
    let mut pkg = package(&f.key, SAMPLE_MANIFEST, SAMPLE_WAT);
    pkg.artifact.extend_from_slice(b";; injected");
    assert!(matches!(f.host.install(&f.admin, MFA, pkg, all_requested()).await, Err(DomainError::Denied(_))));

    let (rogue, _) = PublisherKey::generate().unwrap();
    let pkg = package(&rogue, SAMPLE_MANIFEST, SAMPLE_WAT);
    assert!(matches!(f.host.install(&f.admin, MFA, pkg, all_requested()).await, Err(DomainError::Denied(_))));
}

#[tokio::test]
async fn only_platform_admin_with_mfa_manages_plugins() {
    let f = fx().await;
    let pkg = || package(&f.key, SAMPLE_MANIFEST, SAMPLE_WAT);
    assert!(matches!(f.host.install(&f.operator, MFA, pkg(), all_requested()).await, Err(DomainError::Denied(_))));
    assert!(matches!(
        f.host.install(&f.admin, AuthzContext::default(), pkg(), all_requested()).await,
        Err(DomainError::Denied(_))
    ));
    assert!(f.host.install(&f.admin, MFA, pkg(), all_requested()).await.is_ok());

    // Security review: denials are audited too (CLAUDE.md rule 3).
    let denied = f.audit.0.lock().unwrap().iter().filter(|a| *a == "denied:plugin.manage").count();
    assert_eq!(denied, 2, "both refused installs are in the audit log");
}

#[tokio::test]
async fn cannot_approve_unrequested_permission() {
    let f = fx().await;
    let pkg = package(&f.key, SAMPLE_MANIFEST, SAMPLE_WAT);
    let err = f.host.install(&f.admin, MFA, pkg, vec!["host:exec".into()]).await.unwrap_err();
    assert!(matches!(err, DomainError::Invalid(_)));
}

#[tokio::test]
async fn effective_permission_is_user_intersect_plugin() {
    let f = fx().await;

    // Plugin approved for log only: server-online is refused even for an admin.
    let id = install_enabled(&f, &variant("orbyn.narrow"), SAMPLE_WAT, vec!["host:log".into()]).await;
    let err = f.host.invoke(&f.admin, MFA, &id, EntityRef::platform(), 0).await.unwrap_err();
    assert!(matches!(err, DomainError::Denied(_)), "{err:?}");

    // A refused host call is not a plugin fault.
    assert_eq!(f.host.get(&id).await.unwrap().state, PluginState::Enabled);

    // Fully approved plugin, but the user may not invoke in this scope.
    let id2 = install_enabled(&f, &variant("orbyn.full"), SAMPLE_WAT, all_requested()).await;
    assert!(matches!(f.host.invoke(&f.stranger, MFA, &id2, f.org.clone(), 0).await, Err(DomainError::Denied(_))));
}

#[tokio::test]
async fn plugin_sees_only_what_the_user_can_see() {
    let f = fx().await;
    let id = install_enabled(&f, SAMPLE_MANIFEST, SAMPLE_WAT, all_requested()).await;

    // A developer of another org may invoke the plugin in their org, but the
    // plugin cannot use them to observe web-01 in the first org.
    let out = f.host.invoke(&f.outsider, AuthzContext::default(), &id, f.org2.clone(), 0).await.unwrap();
    assert_eq!(out.output, 0, "web-01 is invisible to the outsider");
    let out = f.host.invoke(&f.admin, MFA, &id, EntityRef::platform(), 0).await.unwrap();
    assert_eq!(out.output, 1, "admin sees web-01");
}

#[tokio::test]
async fn plugin_exceeding_fuel_is_stopped_and_quarantined() {
    let f = fx().await;
    let manifest = variant("orbyn.looper").replace("fuel = 5000000", "fuel = 100000");
    let id = install_enabled(&f, &manifest, LOOP_WAT, vec![]).await;
    let err = f.host.invoke(&f.admin, MFA, &id, EntityRef::platform(), 0).await.unwrap_err();
    assert!(err.to_string().contains("FuelExhausted"), "{err}");
    let rec = f.host.get(&id).await.unwrap();
    assert_eq!(rec.state, PluginState::Quarantined);
    assert!(rec.quarantine_reason.unwrap().contains("FuelExhausted"));
    assert!(f.audit.0.lock().unwrap().contains(&"plugin.quarantine".to_owned()));

    // Quarantined plugins cannot run until re-enabled.
    assert!(matches!(f.host.invoke(&f.admin, MFA, &id, EntityRef::platform(), 0).await, Err(DomainError::Conflict(_))));
}

#[tokio::test]
async fn plugin_exceeding_wall_clock_is_quarantined() {
    let f = fx().await;
    let manifest = variant("orbyn.slow")
        .replace("fuel = 5000000", "fuel = 10000000000")
        .replace("timeout_ms = 1000", "timeout_ms = 200");
    let id = install_enabled(&f, &manifest, LOOP_WAT, vec![]).await;
    let err = f.host.invoke(&f.admin, MFA, &id, EntityRef::platform(), 0).await.unwrap_err();
    assert!(err.to_string().contains("Timeout"), "{err}");
    assert_eq!(f.host.get(&id).await.unwrap().state, PluginState::Quarantined);
}

#[tokio::test]
async fn plugin_exceeding_memory_is_quarantined() {
    let f = fx().await;
    let id = install_enabled(&f, &variant("orbyn.hog"), MEMORY_HOG_WAT, vec![]).await;
    let err = f.host.invoke(&f.admin, MFA, &id, EntityRef::platform(), 0).await.unwrap_err();
    assert!(err.to_string().contains("MemoryExceeded"), "{err}");
    assert_eq!(f.host.get(&id).await.unwrap().state, PluginState::Quarantined);
}

/// Security review: the memory budget was applied to each memory on its
/// own, so several memories could use a multiple of it.
#[tokio::test]
async fn the_memory_budget_is_a_total_across_memories() {
    let f = fx().await;
    let id = install_enabled(&f, &variant("orbyn.twomem"), TWO_MEMORIES_WAT, vec![]).await;
    let err = f.host.invoke(&f.admin, MFA, &id, EntityRef::platform(), 0).await.unwrap_err();
    assert!(err.to_string().contains("MemoryExceeded"), "{err}");
}

#[tokio::test]
async fn components_importing_wasi_cannot_instantiate() {
    let f = fx().await;
    let id = install_enabled(&f, &variant("orbyn.wasi"), WASI_WAT, vec![]).await;
    let err = f.host.invoke(&f.admin, MFA, &id, EntityRef::platform(), 0).await.unwrap_err();
    assert!(err.to_string().contains("plugin failed"), "{err}");
    assert_eq!(f.host.get(&id).await.unwrap().fault_count, 1);
}

#[tokio::test]
async fn lifecycle_disable_upgrade_pin_uninstall() {
    let f = fx().await;
    let id = install_enabled(&f, SAMPLE_MANIFEST, SAMPLE_WAT, all_requested()).await;

    f.host.disable(&f.admin, MFA, &id).await.unwrap();
    assert!(matches!(f.host.invoke(&f.admin, MFA, &id, EntityRef::platform(), 0).await, Err(DomainError::Conflict(_))));

    let v2 = SAMPLE_MANIFEST.replace("version = \"0.1.0\"", "version = \"0.2.0\"");
    let up = f.host.upgrade(&f.admin, MFA, package(&f.key, &v2, SAMPLE_WAT), all_requested()).await.unwrap();
    assert_eq!(up.version, "0.2.0");

    // Downgrade refused.
    assert!(
        f.host.upgrade(&f.admin, MFA, package(&f.key, SAMPLE_MANIFEST, SAMPLE_WAT), all_requested()).await.is_err()
    );

    // Pinned plugins refuse upgrades.
    f.host.set_pinned(&f.admin, MFA, &id, true).await.unwrap();
    let v3 = SAMPLE_MANIFEST.replace("version = \"0.1.0\"", "version = \"0.3.0\"");
    assert!(matches!(
        f.host.upgrade(&f.admin, MFA, package(&f.key, &v3, SAMPLE_WAT), all_requested()).await,
        Err(DomainError::Conflict(_))
    ));
    f.host.set_pinned(&f.admin, MFA, &id, false).await.unwrap();

    // Enabled plugins must be disabled before uninstall.
    f.host.enable(&f.admin, MFA, &id).await.unwrap();
    assert!(f.host.uninstall(&f.admin, MFA, &id).await.is_err());
    f.host.disable(&f.admin, MFA, &id).await.unwrap();
    f.host.uninstall(&f.admin, MFA, &id).await.unwrap();
    assert!(f.host.get(&id).await.is_err());
}

#[tokio::test]
async fn incompatible_api_version_is_rejected() {
    let f = fx().await;
    let m = SAMPLE_MANIFEST.replace(">=0.1.0, <0.2.0", ">=1.0.0");
    let err = f.host.install(&f.admin, MFA, package(&f.key, &m, SAMPLE_WAT), all_requested()).await.unwrap_err();
    assert!(err.to_string().contains("requires API"));
}
