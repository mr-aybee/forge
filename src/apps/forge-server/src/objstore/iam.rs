//! Access keys kept in RustFS's IAM (D-170). RustFS is the source of
//! truth; Forge's [`StoreKey`] rows are a cache and audit trail. Every
//! change is written to RustFS first and recorded after it succeeded.

use std::collections::BTreeMap;
use std::sync::Arc;

use forge_core::storage::ObjectStore;
use forge_core::storage_iam::{
    Drift, IamError, KeyGrant, StoreIam, StoreKey, StoreKeyRepository, drift, grants_of, normalize_grants, policy_for,
};
use forge_core::{Clock, DomainError, StoreKeyId, VaultItemId};
use serde::Serialize;

/// Where the list came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Source {
    Rustfs,
    Cache,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct KeyView {
    pub access_key: String,
    pub name: String,
    pub description: String,
    pub status: Option<String>,
    pub grants: Vec<KeyGrant>,
    /// RustFS holds a policy Forge cannot show as grants.
    pub custom_policy: bool,
    pub managed_by_forge: bool,
    pub record_id: Option<StoreKeyId>,
    pub vault_item_id: Option<VaultItemId>,
    pub drift: Drift,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct IamView {
    pub reachable: bool,
    pub error: Option<String>,
    pub source: Source,
    pub keys: Vec<KeyView>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct ReconcileReport {
    pub imported: u32,
    pub updated: u32,
    pub removed: u32,
}

/// Why an IAM change failed.
#[derive(Debug, thiserror::Error)]
pub enum KeyError {
    #[error("{0}")]
    Invalid(String),
    #[error(transparent)]
    Iam(#[from] IamError),
    #[error(transparent)]
    Domain(#[from] DomainError),
}

#[derive(Debug, Clone)]
pub struct StoreIamService {
    pub keys: Arc<dyn StoreKeyRepository>,
    pub clock: Arc<dyn Clock>,
}

/// One live key: what RustFS reports, with its grants when Forge can read them.
struct Live {
    name: String,
    description: String,
    status: Option<String>,
    grants: Option<Vec<KeyGrant>>,
}

fn clean_name(n: &str) -> Result<String, KeyError> {
    let n = n.trim();
    if n.is_empty() || n.len() > 32 || !n.bytes().all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.'))
    {
        return Err(KeyError::Invalid("key name: 1 to 32 letters, digits, - _ .".into()));
    }

    Ok(n.to_owned())
}

impl StoreIamService {
    /// Every key of the store (except `root`, the store's own credential).
    async fn live(&self, iam: &dyn StoreIam, root: &str) -> Result<BTreeMap<String, Live>, IamError> {
        let mut out = BTreeMap::new();

        for k in iam.list_keys().await? {
            if k.access_key == root {
                continue;
            }

            let grants = iam.key_policy(&k.access_key).await?.and_then(|p| grants_of(&p));
            out.insert(
                k.access_key.clone(),
                Live { name: k.name, description: k.description, status: k.status, grants },
            );
        }

        Ok(out)
    }

    /// Keys as RustFS holds them, compared with Forge's records. When
    /// RustFS cannot be reached, the records are shown (`source: cache`).
    pub async fn view(&self, store: &ObjectStore, iam: &dyn StoreIam, root: &str) -> Result<IamView, KeyError> {
        let records = self.keys.list_store_keys(store.id).await?;

        let live = match self.live(iam, root).await {
            Ok(l) => l,
            Err(e @ IamError::Unreachable(_)) => {
                let keys = records
                    .into_iter()
                    .map(|r| KeyView {
                        access_key: r.access_key,
                        name: r.name,
                        description: String::new(),
                        status: None,
                        custom_policy: r.grants.is_none(),
                        grants: r.grants.unwrap_or_default(),
                        managed_by_forge: true,
                        record_id: Some(r.id),
                        vault_item_id: r.vault_item_id,
                        drift: Drift::Unknown,
                    })
                    .collect();
                return Ok(IamView { reachable: false, error: Some(e.to_string()), source: Source::Cache, keys });
            }
            Err(e) => return Err(e.into()),
        };

        let by_key: BTreeMap<&str, &StoreKey> = records.iter().map(|r| (r.access_key.as_str(), r)).collect();
        let mut keys: Vec<KeyView> = live
            .iter()
            .map(|(ak, l)| {
                let rec = by_key.get(ak.as_str()).copied();
                KeyView {
                    access_key: ak.clone(),
                    name: if l.name.is_empty() {
                        rec.map(|r| r.name.clone()).unwrap_or_default()
                    } else {
                        l.name.clone()
                    },
                    description: l.description.clone(),
                    status: l.status.clone(),
                    grants: l.grants.clone().unwrap_or_default(),
                    custom_policy: l.grants.is_none(),
                    managed_by_forge: rec.is_some(),
                    record_id: rec.map(|r| r.id),
                    vault_item_id: rec.and_then(|r| r.vault_item_id),
                    drift: drift(rec, Some(&l.grants)),
                }
            })
            .collect();
        keys.extend(records.iter().filter(|r| !live.contains_key(&r.access_key)).map(|r| KeyView {
            access_key: r.access_key.clone(),
            name: r.name.clone(),
            description: String::new(),
            status: None,
            grants: r.grants.clone().unwrap_or_default(),
            custom_policy: r.grants.is_none(),
            managed_by_forge: true,
            record_id: Some(r.id),
            vault_item_id: r.vault_item_id,
            drift: Drift::MissingInRustfs,
        }));

        Ok(IamView { reachable: true, error: None, source: Source::Rustfs, keys })
    }

    /// Creates a key in RustFS with exactly `grants`, then records it.
    /// Returns the record and the secret (for the vault only).
    pub async fn create(
        &self,
        store: &ObjectStore,
        iam: &dyn StoreIam,
        name: &str,
        grants: &[KeyGrant],
        created_by: &str,
    ) -> Result<(StoreKey, String), KeyError> {
        let name = clean_name(name)?;
        let grants = normalize_grants(grants).map_err(|e| KeyError::Invalid(e.to_string()))?;
        let buckets: Vec<&str> = grants.iter().map(|g| g.bucket.as_str()).collect();
        let (ak, sk) =
            iam.create_key(&name, &format!("Forge key for {}", buckets.join(", ")), &policy_for(&grants)).await?;
        let now = self.clock.now();
        let record = StoreKey {
            id: StoreKeyId::new(),
            store_id: store.id,
            access_key: ak,
            name,
            grants: Some(grants),
            vault_item_id: None,
            created_by: created_by.to_owned(),
            created_at: now,
            updated_at: now,
            synced_at: Some(now),
        };

        // RustFS holds the key now; a failed record shows as drift
        // (`not_in_forge`) and a reconcile imports it.
        self.keys.put_store_key(&record).await?;

        Ok((record, sk))
    }

    /// Links the vault item holding the key's secret.
    pub async fn attach_vault(&self, mut record: StoreKey, item: VaultItemId) -> Result<StoreKey, KeyError> {
        record.vault_item_id = Some(item);
        record.updated_at = self.clock.now();
        self.keys.put_store_key(&record).await?;

        Ok(record)
    }

    /// Replaces the key's policy in RustFS with exactly `grants` (any key,
    /// also one Forge did not create), then records it.
    pub async fn set_grants(
        &self,
        store: &ObjectStore,
        iam: &dyn StoreIam,
        access_key: &str,
        grants: &[KeyGrant],
        by: &str,
    ) -> Result<Vec<KeyGrant>, KeyError> {
        let grants = normalize_grants(grants).map_err(|e| KeyError::Invalid(e.to_string()))?;
        let known = iam.list_keys().await?;

        let Some(live) = known.iter().find(|k| k.access_key == access_key) else {
            return Err(IamError::NotFound(format!("RustFS has no key {access_key}")).into());
        };

        iam.set_policy(access_key, &policy_for(&grants)).await?;
        let now = self.clock.now();
        let records = self.keys.list_store_keys(store.id).await?;
        let record = match records.into_iter().find(|r| r.access_key == access_key) {
            Some(mut r) => {
                r.grants = Some(grants.clone());
                r.updated_at = now;
                r.synced_at = Some(now);
                r
            }
            None => StoreKey {
                id: StoreKeyId::new(),
                store_id: store.id,
                access_key: access_key.to_owned(),
                name: if live.name.is_empty() { "imported".into() } else { live.name.clone() },
                grants: Some(grants.clone()),
                vault_item_id: None,
                created_by: by.to_owned(),
                created_at: now,
                updated_at: now,
                synced_at: Some(now),
            },
        };

        self.keys.put_store_key(&record).await?;

        Ok(grants)
    }

    /// Forgets a key RustFS no longer has (after a revoke).
    pub async fn forget(&self, store: &ObjectStore, access_key: &str) -> Result<(), KeyError> {
        for r in self.keys.list_store_keys(store.id).await? {
            if r.access_key == access_key {
                self.keys.delete_store_key(r.id).await?;
            }
        }

        Ok(())
    }

    /// Records a rotated key under its new access key.
    pub async fn rotated(&self, store: &ObjectStore, old: &str, new: &str) -> Result<(), KeyError> {
        let now = self.clock.now();

        for mut r in self.keys.list_store_keys(store.id).await? {
            if r.access_key == old {
                new.clone_into(&mut r.access_key);
                r.updated_at = now;
                r.synced_at = Some(now);
                self.keys.put_store_key(&r).await?;
            }
        }

        Ok(())
    }

    /// Accepts what RustFS holds: imports keys Forge has no record of,
    /// updates records whose policy changed in RustFS, and removes records of
    /// keys RustFS no longer has. Fails (changes nothing) when RustFS is
    /// unreachable.
    pub async fn reconcile(
        &self,
        store: &ObjectStore,
        iam: &dyn StoreIam,
        root: &str,
        by: &str,
    ) -> Result<ReconcileReport, KeyError> {
        let live = self.live(iam, root).await?;
        let records = self.keys.list_store_keys(store.id).await?;
        let now = self.clock.now();
        let mut report = ReconcileReport::default();

        for r in &records {
            match live.get(&r.access_key) {
                None => {
                    self.keys.delete_store_key(r.id).await?;
                    report.removed += 1;
                }
                Some(l) if l.grants != r.grants => {
                    let mut r = r.clone();
                    r.grants.clone_from(&l.grants);
                    r.updated_at = now;
                    r.synced_at = Some(now);
                    self.keys.put_store_key(&r).await?;
                    report.updated += 1;
                }
                Some(_) => {}
            }
        }

        for (ak, l) in &live {
            if records.iter().any(|r| &r.access_key == ak) {
                continue;
            }

            self.keys
                .put_store_key(&StoreKey {
                    id: StoreKeyId::new(),
                    store_id: store.id,
                    access_key: ak.clone(),
                    name: if l.name.is_empty() { "imported".into() } else { l.name.clone() },
                    grants: l.grants.clone(),
                    vault_item_id: None,
                    created_by: by.to_owned(),
                    created_at: now,
                    updated_at: now,
                    synced_at: Some(now),
                })
                .await?;
            report.imported += 1;
        }

        Ok(report)
    }
}
