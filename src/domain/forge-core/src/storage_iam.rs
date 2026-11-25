//! Access keys of a RustFS store, kept in RustFS's own IAM (D-170).
//!
//! RustFS is the source of truth: every key is a RustFS service account whose
//! inline policy carries its grants. Forge writes keys and policies there,
//! reads them back to show them, and keeps a [`StoreKey`] row per key only as
//! a cache (shown read-only when the store is unreachable) and an audit trail
//! with ids. Comparing the two shows drift: keys or policies changed directly
//! in RustFS.
//!
//! A grant opens one bucket, or one prefix of it, to read or read-write:
//!
//! * objects: `s3:GetObject` (+ put, delete, multipart for read-write) on
//!   `arn:aws:s3:::bucket/prefix*`;
//! * listing: `s3:ListBucket` on `arn:aws:s3:::bucket`, limited by a
//!   `StringLike s3:prefix` condition when the grant has a prefix.

use std::collections::BTreeMap;

use async_trait::async_trait;
use serde::{Deserialize, Deserializer, Serialize};

use crate::storage::BucketAccess;
use crate::{DomainResult, ObjectStoreId, StoreKeyId, Timestamp, VaultItemId};

/// Most grants one key may hold.
pub const MAX_GRANTS: usize = 32;

const READ_OBJECT: &[&str] = &["s3:GetObject"];
const WRITE_OBJECT: &[&str] =
    &["s3:GetObject", "s3:PutObject", "s3:DeleteObject", "s3:AbortMultipartUpload", "s3:ListMultipartUploadParts"];
const BUCKET_ACTIONS: &[&str] = &["s3:ListBucket"];
const LOCATION_ACTIONS: &[&str] = &["s3:GetBucketLocation", "s3:ListBucketMultipartUploads"];

/// One bucket, or one prefix of it, opened to a key.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct KeyGrant {
    pub bucket: String,
    /// Empty for the whole bucket; otherwise ends with `/` (`uploads/`).
    #[serde(default)]
    pub prefix: String,
    pub access: BucketAccess,
}

/// Bucket names (S3 rules, lowercase).
#[must_use]
pub fn valid_bucket(name: &str) -> bool {
    (3..=63).contains(&name.len())
        && name.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-' || b == b'.')
        && name.as_bytes()[0].is_ascii_alphanumeric()
        && name.as_bytes()[name.len() - 1].is_ascii_alphanumeric()
}

/// Prefixes: empty, or `[A-Za-z0-9!_.*'()/-]` path segments ending with `/`,
/// no leading `/`, no `..`, no `*` (a wildcard would widen the grant).
#[must_use]
pub fn valid_prefix(p: &str) -> bool {
    p.is_empty()
        || (p.len() <= 512
            && p.ends_with('/')
            && !p.starts_with('/')
            && !p.contains("//")
            && !p.split('/').any(|s| s == ".." || s == ".")
            && p.bytes()
                .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'/' | b'_' | b'.' | b'-' | b'!' | b'(' | b')')))
}

/// Why grants were refused.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("{0}")]
pub struct GrantError(pub String);

/// Checks a key's grants and puts them in a stable order without duplicates.
pub fn normalize_grants(grants: &[KeyGrant]) -> Result<Vec<KeyGrant>, GrantError> {
    if grants.is_empty() || grants.len() > MAX_GRANTS {
        return Err(GrantError(format!("a key needs 1 to {MAX_GRANTS} grants")));
    }

    for g in grants {
        if !valid_bucket(&g.bucket) {
            return Err(GrantError(format!("bucket name `{}`", g.bucket)));
        }

        if !valid_prefix(&g.prefix) {
            return Err(GrantError(format!(
                "prefix `{}`: path segments ending with /, such as uploads/ (no wildcards, no leading /)",
                g.prefix
            )));
        }
    }

    let mut out: Vec<KeyGrant> = grants.to_vec();
    out.sort();
    out.dedup();

    Ok(out)
}

/// An IAM statement as RustFS stores it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Statement {
    #[serde(rename = "Effect")]
    pub effect: String,
    #[serde(rename = "Action", deserialize_with = "one_or_many")]
    pub action: Vec<String>,
    #[serde(rename = "Resource", deserialize_with = "one_or_many")]
    pub resource: Vec<String>,
    /// `{operator: {key: [values]}}`.
    #[serde(rename = "Condition", default, skip_serializing_if = "BTreeMap::is_empty")]
    pub condition: BTreeMap<String, BTreeMap<String, Values>>,
}

/// An IAM policy document.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PolicyDocument {
    #[serde(rename = "Version")]
    pub version: String,
    #[serde(rename = "Statement")]
    pub statement: Vec<Statement>,
}

/// Condition values: RustFS returns a single value as a string.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(transparent)]
pub struct Values(pub Vec<String>);

impl<'de> Deserialize<'de> for Values {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        one_or_many(d).map(Values)
    }
}

fn one_or_many<'de, D: Deserializer<'de>>(d: D) -> Result<Vec<String>, D::Error> {
    #[derive(Deserialize)]
    #[serde(untagged)]
    enum OneOrMany {
        One(String),
        Many(Vec<String>),
    }
    Ok(match OneOrMany::deserialize(d)? {
        OneOrMany::One(s) => vec![s],
        OneOrMany::Many(v) => v,
    })
}

fn strings(list: &[&str]) -> Vec<String> {
    list.iter().map(|s| (*s).to_owned()).collect()
}

fn allow(action: &[&str], resource: Vec<String>) -> Statement {
    Statement { effect: "Allow".into(), action: strings(action), resource, condition: BTreeMap::new() }
}

/// The policy that gives exactly `grants` (already normalized).
#[must_use]
pub fn policy_for(grants: &[KeyGrant]) -> PolicyDocument {
    let mut statement = Vec::new();
    let buckets: std::collections::BTreeSet<&str> = grants.iter().map(|g| g.bucket.as_str()).collect();

    for b in &buckets {
        statement.push(allow(LOCATION_ACTIONS, vec![format!("arn:aws:s3:::{b}")]));
    }

    for g in grants {
        let mut list = allow(BUCKET_ACTIONS, vec![format!("arn:aws:s3:::{}", g.bucket)]);

        if !g.prefix.is_empty() {
            list.condition = BTreeMap::from([(
                "StringLike".to_owned(),
                BTreeMap::from([("s3:prefix".to_owned(), Values(vec![format!("{}*", g.prefix)]))]),
            )]);
        }

        statement.push(list);
        let object = if g.access.writes() { WRITE_OBJECT } else { READ_OBJECT };
        statement.push(allow(object, vec![format!("arn:aws:s3:::{}/{}*", g.bucket, g.prefix)]));
    }

    PolicyDocument { version: "2012-10-17".into(), statement }
}

fn same_set(a: &[String], b: &[&str]) -> bool {
    let a: std::collections::BTreeSet<&str> = a.iter().map(String::as_str).collect();
    let b: std::collections::BTreeSet<&str> = b.iter().copied().collect();

    a == b
}

/// The grants a policy gives, when it has exactly the shape [`policy_for`]
/// writes; `None` for any other policy (shown as a custom policy).
#[must_use]
pub fn grants_of(policy: &PolicyDocument) -> Option<Vec<KeyGrant>> {
    let mut grants = Vec::new();

    for s in &policy.statement {
        if s.effect != "Allow" || s.resource.len() != 1 {
            return None;
        }

        let arn = s.resource[0].strip_prefix("arn:aws:s3:::")?;

        if same_set(&s.action, LOCATION_ACTIONS) || same_set(&s.action, BUCKET_ACTIONS) {
            // Listing and location follow from the object statements;
            // checked below for prefix consistency.
            if arn.contains('/') {
                return None;
            }

            continue;
        }

        let access = if same_set(&s.action, READ_OBJECT) {
            BucketAccess::Read
        } else if same_set(&s.action, WRITE_OBJECT) {
            BucketAccess::ReadWrite
        } else {
            return None;
        };

        if !s.condition.is_empty() {
            return None;
        }

        let (bucket, rest) = arn.split_once('/')?;
        let prefix = rest.strip_suffix('*')?;
        grants.push(KeyGrant { bucket: bucket.to_owned(), prefix: prefix.to_owned(), access });
    }

    let grants = normalize_grants(&grants).ok()?;

    // Only a policy equal to the one Forge would write counts as grants: a
    // wider listing statement is a custom policy.
    (policy_for(&grants).statement.len() == policy.statement.len()
        && policy_for(&grants).statement.iter().all(|s| policy.statement.iter().any(|p| same_statement(p, s))))
    .then_some(grants)
}

fn same_statement(a: &Statement, b: &Statement) -> bool {
    a.effect == b.effect
        && same_set(&a.action, &b.action.iter().map(String::as_str).collect::<Vec<_>>())
        && a.resource == b.resource
        && a.condition == b.condition
}

/// One access key as RustFS reports it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct IamKey {
    pub access_key: String,
    pub name: String,
    pub description: String,
    /// `on` or `off`.
    pub status: Option<String>,
}

/// Why a call to the store's IAM failed.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum IamError {
    /// The store did not answer (network, timeout, 5xx).
    #[error("the store is unreachable: {0}")]
    Unreachable(String),
    /// The store answered and refused (bad key, denied, invalid policy).
    #[error("the store refused: {0}")]
    Rejected(String),
    #[error("not found: {0}")]
    NotFound(String),
}

/// The IAM of one store (RustFS admin API), signed with its root credential.
#[async_trait]
pub trait StoreIam: Send + Sync {
    async fn list_keys(&self) -> Result<Vec<IamKey>, IamError>;
    /// The key's policy as RustFS stores it; `None` when it has none (the
    /// parent user's rights).
    async fn key_policy(&self, access_key: &str) -> Result<Option<PolicyDocument>, IamError>;
    /// Creates a key; returns `(access_key, secret_key)`.
    async fn create_key(
        &self,
        name: &str,
        description: &str,
        policy: &PolicyDocument,
    ) -> Result<(String, String), IamError>;
    async fn set_policy(&self, access_key: &str, policy: &PolicyDocument) -> Result<(), IamError>;
    async fn delete_key(&self, access_key: &str) -> Result<(), IamError>;
}

/// Forge's record of a key it manages: cache and audit trail, never the
/// source of truth.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StoreKey {
    pub id: StoreKeyId,
    pub store_id: ObjectStoreId,
    pub access_key: String,
    pub name: String,
    /// The grants last written to or read from RustFS; `None` when RustFS
    /// holds a policy Forge cannot express as grants.
    pub grants: Option<Vec<KeyGrant>>,
    pub vault_item_id: Option<VaultItemId>,
    pub created_by: String,
    pub created_at: Timestamp,
    pub updated_at: Timestamp,
    /// When the row last matched RustFS.
    pub synced_at: Option<Timestamp>,
}

/// How Forge's record of a key compares with RustFS.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Drift {
    InSync,
    /// RustFS holds another policy than Forge last wrote.
    PolicyChanged,
    /// Forge has a record, RustFS no longer has the key.
    MissingInRustfs,
    /// RustFS has a key Forge has no record of.
    NotInForge,
    /// RustFS is unreachable; the cache is shown.
    Unknown,
}

/// Compares a record with what RustFS reports for the same key.
#[must_use]
pub fn drift(record: Option<&StoreKey>, live: Option<&Option<Vec<KeyGrant>>>) -> Drift {
    match (record, live) {
        (Some(_), None) => Drift::MissingInRustfs,
        (None, Some(_)) => Drift::NotInForge,
        (Some(r), Some(g)) if r.grants == *g => Drift::InSync,
        (Some(_), Some(_)) => Drift::PolicyChanged,
        (None, None) => Drift::Unknown,
    }
}

/// Forge's key records.
#[async_trait]
pub trait StoreKeyRepository: Send + Sync + std::fmt::Debug {
    async fn put_store_key(&self, key: &StoreKey) -> DomainResult<()>;
    async fn list_store_keys(&self, store: ObjectStoreId) -> DomainResult<Vec<StoreKey>>;
    async fn delete_store_key(&self, id: StoreKeyId) -> DomainResult<()>;
}

#[cfg(test)]
mod tests {
    use super::*;

    fn g(bucket: &str, prefix: &str, access: BucketAccess) -> KeyGrant {
        KeyGrant { bucket: bucket.into(), prefix: prefix.into(), access }
    }

    #[test]
    fn prefix_grants_limit_objects_and_listing() {
        let grants = normalize_grants(&[g("media", "uploads/", BucketAccess::ReadWrite)]).unwrap_or_default();
        let p = serde_json::to_value(policy_for(&grants)).unwrap_or_default();
        let text = p.to_string();
        assert!(text.contains("arn:aws:s3:::media/uploads/*"), "{text}");
        assert!(text.contains(r#""StringLike":{"s3:prefix":["uploads/*"]}"#), "{text}");
        assert!(text.contains("s3:PutObject") && text.contains("s3:DeleteObject"));
        assert!(!text.contains("arn:aws:s3:::media/*"), "no whole-bucket object access: {text}");
        let read = policy_for(&[g("media", "", BucketAccess::Read)]);
        let text = serde_json::to_string(&read).unwrap_or_default();
        assert!(text.contains("arn:aws:s3:::media/*") && !text.contains("PutObject") && !text.contains("Condition"));
    }

    #[test]
    fn policies_round_trip_to_grants() {
        let grants = normalize_grants(&[
            g("media", "a/", BucketAccess::Read),
            g("media", "b/c/", BucketAccess::ReadWrite),
            g("logs", "", BucketAccess::Read),
        ])
        .unwrap_or_default();
        let doc = policy_for(&grants);

        // RustFS returns the policy as pretty JSON text.
        let back: PolicyDocument =
            serde_json::from_str(&serde_json::to_string_pretty(&doc).unwrap_or_default()).unwrap_or(doc.clone());
        assert_eq!(grants_of(&back), Some(grants));
        let custom: PolicyDocument = serde_json::from_str(
            r#"{"Version":"2012-10-17","Statement":[{"Effect":"Allow","Action":"s3:*","Resource":"arn:aws:s3:::*"}]}"#,
        )
        .unwrap_or(doc);
        assert_eq!(grants_of(&custom), None);
    }

    #[test]
    fn rejects_bad_grants() {
        for p in ["/a/", "a", "a/*/", "../", "a//b/", "a b/"] {
            assert!(normalize_grants(&[g("media", p, BucketAccess::Read)]).is_err(), "{p}");
        }

        assert!(normalize_grants(&[g("Media", "", BucketAccess::Read)]).is_err());
        assert!(normalize_grants(&[]).is_err());
    }

    #[test]
    fn drift_compares_records_with_rustfs() {
        let rec = StoreKey {
            id: StoreKeyId::new(),
            store_id: ObjectStoreId::new(),
            access_key: "AK".into(),
            name: "app".into(),
            grants: Some(vec![g("media", "", BucketAccess::Read)]),
            vault_item_id: None,
            created_by: "user:x".into(),
            created_at: Timestamp(0),
            updated_at: Timestamp(0),
            synced_at: None,
        };
        assert_eq!(drift(Some(&rec), Some(&rec.grants.clone())), Drift::InSync);
        assert_eq!(drift(Some(&rec), Some(&None)), Drift::PolicyChanged);
        assert_eq!(drift(Some(&rec), None), Drift::MissingInRustfs);
        assert_eq!(drift(None, Some(&None)), Drift::NotInForge);
    }
}
