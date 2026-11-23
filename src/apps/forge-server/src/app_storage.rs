//! Object storage attached to a service (D-106).
//!
//! An attachment gives one service a key that is limited to one bucket
//! of a managed store. The key is a vault item like every Forge-issued
//! store key; what makes it an attachment are two more tags on that item,
//! the service and the environment-variable prefix. Nothing is copied
//! into the service: at deploy time [`deploy_env`] reads the vault items
//! and adds the endpoint, region, bucket and credential to the environment
//! sent to the node. Rotating the key or detaching it therefore takes effect
//! with the next deploy, and the secret exists in one place only.

use forge_core::model::{Service, VaultItem};
use forge_core::ports::VaultRepository;
use forge_core::storage::ObjectStoreRepository;
use forge_core::{DomainError, DomainResult, ObjectStoreId, ServiceId, VaultItemId, WorkspaceId};
use forge_secrets::Envelope;
use serde::Serialize;

/// Tag every store key carries: `objstore:<store id>`.
pub const TAG_STORE: &str = "objstore:";
/// Tag of the bucket a key is limited to: `bucket:<name>`.
pub const TAG_BUCKET: &str = "bucket:";
/// Tag of the key's access: `access:read` or `access:readwrite`.
pub const TAG_ACCESS: &str = "access:";
/// Tag that attaches a key to a service: `app:<service id>`.
pub const TAG_APP: &str = "app:";
/// Tag with the prefix of the injected variables: `env-prefix:<PREFIX>`.
pub const TAG_PREFIX: &str = "env-prefix:";
/// Prefix used when none is given.
pub const DEFAULT_PREFIX: &str = "S3";

/// What a key may do in its bucket: the access of a bucket grant too, so it
/// is defined once, in the domain.
pub use forge_core::storage::BucketAccess as Access;

/// The variables an attachment adds to the service's environment.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct EnvNames {
    pub endpoint: String,
    pub region: String,
    pub bucket: String,
    pub access_key: String,
    pub secret_key: String,
}

impl EnvNames {
    /// `<PREFIX>_ENDPOINT`, `_REGION`, `_BUCKET`, `_ACCESS_KEY_ID` and
    /// `_SECRET_ACCESS_KEY`.
    #[must_use]
    pub fn of(prefix: &str) -> Self {
        Self {
            endpoint: format!("{prefix}_ENDPOINT"),
            region: format!("{prefix}_REGION"),
            bucket: format!("{prefix}_BUCKET"),
            access_key: format!("{prefix}_ACCESS_KEY_ID"),
            secret_key: format!("{prefix}_SECRET_ACCESS_KEY"),
        }
    }

    #[must_use]
    pub fn all(&self) -> [&str; 5] {
        [&self.endpoint, &self.region, &self.bucket, &self.access_key, &self.secret_key]
    }
}

/// A prefix is an environment-variable name of its own: an uppercase letter,
/// then uppercase letters, digits and underscores, at most 24 characters.
#[must_use]
pub fn valid_prefix(p: &str) -> bool {
    let mut bytes = p.bytes();
    bytes.next().is_some_and(|b| b.is_ascii_uppercase())
        && p.len() <= 24
        && !p.ends_with('_')
        && bytes.all(|b| b.is_ascii_uppercase() || b.is_ascii_digit() || b == b'_')
}

/// One bucket attached to a service.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Attachment {
    pub access_key: String,
    pub store_id: ObjectStoreId,
    pub bucket: String,
    pub access: Access,
    pub prefix: String,
    pub vault_item_id: VaultItemId,
    /// The variables the service gets at deploy.
    pub env: EnvNames,
}

fn tag<'a>(item: &'a VaultItem, prefix: &str) -> Option<&'a str> {
    item.tags.iter().find_map(|t| t.strip_prefix(prefix))
}

/// The tags that turn a store key's vault item into an attachment.
#[must_use]
pub fn attachment_tags(app: ServiceId, prefix: &str) -> [String; 2] {
    [format!("{TAG_APP}{app}"), format!("{TAG_PREFIX}{prefix}")]
}

/// Reads an attachment of `app` from a vault item; `None` for any other item.
#[must_use]
pub fn attachment(item: &VaultItem, app: ServiceId) -> Option<Attachment> {
    if tag(item, TAG_APP)? != app.to_string() {
        return None;
    }
    let prefix = tag(item, TAG_PREFIX)?.to_owned();
    Some(Attachment {
        access_key: item.username.clone(),
        store_id: tag(item, TAG_STORE)?.parse().ok()?,
        bucket: tag(item, TAG_BUCKET)?.to_owned(),
        access: Access::parse(tag(item, TAG_ACCESS)?)?,
        env: EnvNames::of(&prefix),
        prefix,
        vault_item_id: item.id,
    })
}

/// The service a vault item attaches a key to; `None` for any other item.
#[must_use]
pub fn attached_service(item: &VaultItem) -> Option<ServiceId> {
    tag(item, TAG_APP)?.parse().ok()
}

/// Every attachment on `store`, with the service it belongs to.
pub async fn store_attachments(
    vault: &dyn VaultRepository,
    workspace: WorkspaceId,
    store: ObjectStoreId,
) -> DomainResult<Vec<(ServiceId, Attachment)>> {
    Ok(vault
        .list_vault_items(workspace)
        .await?
        .iter()
        .filter_map(|i| {
            let app = attached_service(i)?;
            attachment(i, app).filter(|a| a.store_id == store).map(|a| (app, a))
        })
        .collect())
}

/// The attachments of `app`, each with its vault item, ordered by prefix.
pub async fn attachments(vault: &dyn VaultRepository, app: &Service) -> DomainResult<Vec<(Attachment, VaultItem)>> {
    let mut out: Vec<(Attachment, VaultItem)> = vault
        .list_vault_items(app.workspace_id)
        .await?
        .into_iter()
        .filter_map(|i| attachment(&i, app.id).map(|a| (a, i)))
        .collect();
    out.sort_by(|a, b| a.0.prefix.cmp(&b.0.prefix));
    Ok(out)
}

/// The environment the attachments of `app` add at deploy, with the secret
/// unsealed. Fails when an attached store no longer exists, so a deploy
/// never starts a service without the storage it was given.
pub async fn deploy_env(
    vault: &dyn VaultRepository,
    stores: &dyn ObjectStoreRepository,
    envelope: &Envelope,
    app: &Service,
) -> DomainResult<Vec<(String, String)>> {
    let mut out = Vec::new();
    for (a, item) in attachments(vault, app).await? {
        let store = stores.get_object_store(a.store_id).await?.ok_or_else(|| {
            DomainError::Conflict(format!(
                "the object store of bucket `{}` no longer exists; detach it from the service",
                a.bucket
            ))
        })?;
        let secret = envelope
            .open(&item.sealed, &crate::jobs::storage::vault_aad(item.id))
            .ok()
            .and_then(|b| String::from_utf8(b).ok())
            .ok_or_else(|| DomainError::internal(format!("cannot unseal the key of bucket `{}`", a.bucket)))?;
        out.push((a.env.endpoint, store.endpoint));
        out.push((a.env.region, store.region));
        out.push((a.env.bucket, a.bucket));
        out.push((a.env.access_key, a.access_key));
        out.push((a.env.secret_key, secret));
    }
    Ok(out)
}

/// Adds `extra` to `env`; a variable the service defines itself wins.
pub fn merge_env(env: &mut Vec<(String, String)>, extra: Vec<(String, String)>) {
    for (k, v) in extra {
        if !env.iter().any(|(have, _)| *have == k) {
            env.push((k, v));
        }
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)] // tests
mod tests {
    use super::*;

    #[test]
    fn prefixes_are_variable_names() {
        for ok in ["S3", "MEDIA", "BACKUP_2", "A"] {
            assert!(valid_prefix(ok), "{ok}");
        }
        for bad in ["", "s3", "3S", "S3_", "A-B", "A B", "ABCDEFGHIJKLMNOPQRSTUVWXYZ"] {
            assert!(!valid_prefix(bad), "{bad}");
        }
        let n = EnvNames::of("MEDIA");
        assert_eq!(
            n.all(),
            ["MEDIA_ENDPOINT", "MEDIA_REGION", "MEDIA_BUCKET", "MEDIA_ACCESS_KEY_ID", "MEDIA_SECRET_ACCESS_KEY"]
        );
    }

    #[test]
    fn an_service_variable_wins_over_an_attachment() {
        let mut env = vec![("S3_BUCKET".to_owned(), "mine".to_owned())];
        merge_env(&mut env, vec![("S3_BUCKET".into(), "attached".into()), ("S3_REGION".into(), "us-east-1".into())]);
        assert_eq!(env, [("S3_BUCKET".into(), "mine".into()), ("S3_REGION".into(), "us-east-1".into())]);
    }

    #[test]
    fn access_names_round_trip() {
        assert_eq!(Access::parse("readwrite"), Some(Access::ReadWrite));
        assert_eq!(Access::parse(Access::Read.as_str()), Some(Access::Read));
        assert_eq!(Access::parse("admin"), None);
        assert_eq!(serde_json::to_string(&Access::ReadWrite).unwrap(), "\"readwrite\"");
    }
}
