//! First-run initialisation: the first platform administrator and a default org.

use forge_core::model::{AuditEvent, AuditOutcome, RoleBinding, Workspace};
use forge_core::ports::{SettingsRepository, TenancyRepository, UserRepository};
use forge_core::*;
use serde_json::json;

use crate::state::AppState;

/// File under `data_dir` where the installer keeps a generated administrator
/// password (readable by root/Administrators only). Forge deletes it when that
/// administrator changes their password.
pub const INITIAL_PASSWORD_FILE: &str = "initial-admin-password";
/// Setting naming the user whose password is in [`INITIAL_PASSWORD_FILE`].
pub const INITIAL_PASSWORD_USER: &str = "bootstrap.initial_password_user";

#[derive(Debug, Clone)]
pub struct InitResult {
    pub user_id: UserId,
    pub workspace_id: WorkspaceId,
}

/// Creates the first administrator. Refuses if any user already exists, so
/// it cannot be used to add a backdoor admin to a running installation.
/// `generated_password` records that the installer saved the password to
/// [`INITIAL_PASSWORD_FILE`], so it is removed once the admin changes it.
pub async fn init_admin(
    state: &AppState,
    username: Option<&str>,
    email: Option<&str>,
    display_name: &str,
    password: &str,
    generated_password: bool,
) -> anyhow::Result<InitResult> {
    if !state.store.list_users().await?.is_empty() {
        anyhow::bail!("already initialised: users exist");
    }
    let user = state.identity.create_account(username, email, display_name, password).await?;
    if generated_password {
        state.store.put_setting(INITIAL_PASSWORD_USER, &user.id.to_string(), state.clock.now()).await?;
    }
    let binding = RoleBinding {
        id: RoleBindingId::new(),
        user_id: user.id,
        role: Role::PlatformAdmin,
        scope: EntityRef::platform(),
        created_at: state.clock.now(),
    };
    state.grant(&binding).await?;
    let org = Workspace {
        id: WorkspaceId::new(),
        name: "Default".into(),
        slug: "default".into(),
        created_at: state.clock.now(),
    };
    state.store.create_workspace(&org).await?;
    state
        .audit
        .append(
            AuditEvent::new(format!("user:{}", user.id), "platform.init", EntityRef::platform(), AuditOutcome::Success)
                .with_details(json!({ "admin": user.login(), "default_org": org.id })),
        )
        .await?;
    Ok(InitResult { user_id: user.id, workspace_id: org.id })
}

/// Deletes the installer's saved password once `user` (the administrator it
/// belongs to) has chosen a new one.
pub async fn forget_initial_password(state: &AppState, user: UserId) {
    let Ok(settings) = state.store.all_settings().await else { return };
    let ours = settings.iter().any(|(k, v)| k == INITIAL_PASSWORD_USER && *v == user.to_string());
    if !ours {
        return;
    }
    let path = state.config.data_dir.join(INITIAL_PASSWORD_FILE);
    match std::fs::remove_file(&path) {
        Ok(()) => tracing::info!(path = %path.display(), "removed the installer's initial administrator password"),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(e) => tracing::warn!(path = %path.display(), error = %e, "could not remove the initial password file"),
    }
    let _ = state.store.delete_setting(INITIAL_PASSWORD_USER).await;
}

/// `forge-server enroll-token` (D-099): an enrollment token made on the
/// server host by local root, for the installer's "agent on this host"
/// option. Creates the cluster `cluster` if the workspace has none of that
/// name. The token is filed under the first platform administrator (tokens
/// name a person) and audited as `system:forge`. Returns the token secret.
pub async fn local_enroll_token(
    state: &AppState,
    cluster: &str,
    hostname: &str,
    ttl_minutes: u32,
) -> anyhow::Result<String> {
    use forge_core::model::{Cluster, EnrollmentToken};
    use forge_core::ports::{ClusterRepository, EnrollmentRepository, RoleBindingRepository};
    let name = cluster.trim();
    if name.is_empty()
        || name.len() > 63
        || !name.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.' | ' '))
    {
        anyhow::bail!("cluster name: 1-63 letters, digits, space, - _ .");
    }
    let hostname = hostname.trim();
    if hostname.is_empty() || hostname.len() > 253 || hostname.chars().any(char::is_control) {
        anyhow::bail!("hostname: 1-253 characters");
    }
    let ttl = ttl_minutes.clamp(1, 1440);
    let ws = match state.store.list_workspaces().await?.as_slice() {
        [only] => only.id,
        [] => anyhow::bail!("no workspace: run `forge-server init` first"),
        _ => anyhow::bail!("more than one workspace in the store (D-016 allows one)"),
    };
    let mut admins: Vec<RoleBinding> = state
        .store
        .list_bindings()
        .await?
        .into_iter()
        .filter(|b| b.role == Role::PlatformAdmin && b.scope == EntityRef::platform())
        .collect();
    admins.sort_by_key(|b| b.created_at.0);
    let mut owner = None;
    for b in admins {
        if state.store.get_user(b.user_id).await.is_ok_and(|u| !u.disabled) {
            owner = Some(b.user_id);
            break;
        }
    }
    let owner =
        owner.ok_or_else(|| anyhow::anyhow!("no enabled platform administrator: run `forge-server init` first"))?;
    let existing = state.store.list_clusters(ws).await?.into_iter().find(|c| c.name.eq_ignore_ascii_case(name));
    let (cluster_id, created) = match existing {
        Some(c) => (c.id, false),
        None => {
            let c = Cluster {
                id: ClusterId::new(),
                workspace_id: ws,
                name: name.to_owned(),
                description: String::new(),
                environment_id: None,
                abbr: String::new(),
                icon: None,
                mode: Default::default(),
                created_at: state.clock.now(),
            };
            state.store.create_cluster(&c).await?;
            state
                .audit_system(
                    "cluster.create",
                    json!({ "cluster_id": c.id, "name": c.name, "via": "forge-server enroll-token" }),
                )
                .await;
            (c.id, true)
        }
    };
    let generated = forge_pki::enrollment::generate(state.ca.fingerprint());
    let token = EnrollmentToken {
        id: forge_secrets::crypto::random_token::<9>(),
        workspace_id: ws,
        environment_id: None,
        token_hash: generated.secret_hash,
        created_by: Principal::User(owner),
        expires_at: state.clock.now().plus(std::time::Duration::from_secs(u64::from(ttl) * 60)),
        used_at: None,
        server_id: None,
    };
    state.store.create_token(&token).await?;
    state.store.put_enrollment_target(&token.id, Some(cluster_id), Some(hostname), state.clock.now()).await?;
    state
        .audit_system(
            "server.enrollment_token.create",
            json!({
                "token_id": token.id, "ttl_minutes": ttl, "cluster_id": cluster_id, "cluster_created": created,
                "hostname": hostname, "filed_under": owner, "via": "forge-server enroll-token",
            }),
        )
        .await;
    Ok(generated.display)
}
