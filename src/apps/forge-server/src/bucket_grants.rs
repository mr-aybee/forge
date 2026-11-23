//! Bucket grants (D-108): one person's access to one bucket of a store.
//!
//! Someone who may manage a store (`storage.manage`) lets one person list
//! and download (`read`), or also upload and delete (`readwrite`), the
//! objects of one bucket, without a role on the store's node, workspace or
//! environment. The access is through Forge only (console, API, CLI): a
//! grant never issues an S3 key, and never reaches the store's other
//! buckets, its keys, its lifecycle or its grants.
//!
//! [`BucketGrants`] holds the rules: who can be named, when a grant counts
//! and what the lists show. The API authorizes the administrator, checks
//! that the bucket exists, audits and asks here before it serves a caller
//! whom no role lets in. A grant counts for one request only while it is
//! not revoked, not expired by the [`Clock`] and its holder is not disabled,
//! so a revocation or an expiry takes effect with the next request. It is
//! checked when a request starts: a download already running finishes.
//!
//! A grant names its bucket by name, so the grants of a bucket end with the
//! bucket: deleting it through Forge revokes them, and creating a bucket
//! revokes what is left for that name from a bucket deleted elsewhere
//! ([`BucketGrants::revoke_bucket`]).

use std::collections::HashMap;
use std::sync::Arc;

use forge_core::model::User;
use forge_core::ports::UserRepository;
use forge_core::storage::{
    BucketAccess, BucketGrant, BucketGrantRepository, GrantStatus, ObjectStore, ObjectStoreRepository,
};
use forge_core::{
    BucketGrantId, Clock, DomainError, DomainResult, NEVER_EXPIRES, ObjectStoreId, Principal, Timestamp, UserId,
    WorkspaceId,
};
use serde::Serialize;

/// Audit action of a new grant.
pub const AUDIT_CREATE: &str = "storage.grant.create";
/// Audit action of a revocation.
pub const AUDIT_REVOKE: &str = "storage.grant.revoke";
/// Audit reason of a revocation that followed the deletion of the bucket.
pub const REASON_BUCKET_DELETED: &str = "bucket deleted";
/// Audit reason of a revocation made when a bucket of that name is created.
pub const REASON_BUCKET_RECREATED: &str = "bucket recreated";
/// The one answer for a person who cannot be given a grant: it does not say
/// whether the account exists.
pub const UNKNOWN_USER: &str = "unknown or disabled user";
/// Longest note kept with a grant, in characters.
pub const MAX_NOTE: usize = 200;
/// Rows a store's list returns when no limit is given, and at most.
pub const LIST_DEFAULT: u32 = 200;
pub const LIST_MAX: u32 = 500;
/// Longest lifetime a grant with an end can be given: ten years.
const MAX_LIFETIME_MINUTES: u32 = 10 * 366 * 24 * 60;

/// When a new grant ends.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Expiry {
    Never,
    /// At this moment, which must lie ahead.
    At(Timestamp),
    /// This many minutes after it is issued.
    In {
        minutes: u32,
    },
}

/// What an administrator asks for.
#[derive(Debug, Clone)]
pub struct NewBucketGrant {
    /// The person: a user id, a username or an email address.
    pub user: String,
    /// A bucket of the store. The caller has checked that it exists.
    pub bucket: String,
    pub access: BucketAccess,
    pub expiry: Expiry,
    pub note: String,
    pub created_by: Principal,
}

/// One grant as the lists show it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct GrantView {
    pub id: BucketGrantId,
    pub store_id: ObjectStoreId,
    /// The store's name.
    pub store: String,
    pub user_id: UserId,
    /// The name the person signs in with.
    pub user: String,
    pub bucket: String,
    pub access: BucketAccess,
    /// Principal of the person who issued it (`user:<id>`).
    pub created_by: String,
    /// The name that person signs in with, when the account is known.
    pub created_by_name: Option<String>,
    pub created_at: Timestamp,
    /// `null` when the grant never expires.
    pub expires_at: Option<Timestamp>,
    pub never_expires: bool,
    pub revoked_at: Option<Timestamp>,
    pub note: String,
    pub status: GrantStatus,
}

/// A grant as its holder sees it: what they may open and until when, not
/// who gave it or why.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct MyGrantView {
    pub id: BucketGrantId,
    pub store_id: ObjectStoreId,
    /// The store's name.
    pub store: String,
    pub bucket: String,
    pub access: BucketAccess,
    /// `null` when the grant never expires.
    pub expires_at: Option<Timestamp>,
    pub status: GrantStatus,
}

/// The use cases around bucket grants. Depends only on ports.
#[derive(Debug, Clone)]
pub struct BucketGrants {
    grants: Arc<dyn BucketGrantRepository>,
    stores: Arc<dyn ObjectStoreRepository>,
    users: Arc<dyn UserRepository>,
    clock: Arc<dyn Clock>,
}

fn login(u: &User) -> String {
    u.username.clone().or_else(|| u.email.clone()).unwrap_or_else(|| u.id.to_string())
}

impl BucketGrants {
    #[must_use]
    pub fn new(
        grants: Arc<dyn BucketGrantRepository>,
        stores: Arc<dyn ObjectStoreRepository>,
        users: Arc<dyn UserRepository>,
        clock: Arc<dyn Clock>,
    ) -> Self {
        Self { grants, stores, users, clock }
    }

    /// The enabled account `name` stands for: a user id, else a username,
    /// else an email address. An account that does not exist and one that is
    /// disabled get the same answer ([`UNKNOWN_USER`]).
    async fn person(&self, name: &str) -> DomainResult<User> {
        let name = name.trim();
        if name.is_empty() {
            return Err(DomainError::Invalid("user: a user id, username or email".into()));
        }
        let found = if let Ok(id) = name.parse::<UserId>() {
            match self.users.get_user(id).await {
                Ok(u) => Some(u),
                Err(DomainError::NotFound(_)) => None,
                Err(e) => return Err(e),
            }
        } else {
            let lower = name.to_lowercase();
            match self.users.find_user_by_username(&lower).await? {
                Some(u) => Some(u),
                None => self.users.find_user_by_email(&lower).await?,
            }
        };
        found.filter(|u| !u.disabled).ok_or_else(|| DomainError::NotFound(UNKNOWN_USER.into()))
    }

    fn expires_at(&self, expiry: Expiry, now: Timestamp) -> DomainResult<Timestamp> {
        match expiry {
            Expiry::Never => Ok(NEVER_EXPIRES),
            Expiry::At(t) if t > now && t < NEVER_EXPIRES => Ok(t),
            Expiry::At(_) => Err(DomainError::Invalid("expires_at must be a time in the future".into())),
            Expiry::In { minutes } if (1..=MAX_LIFETIME_MINUTES).contains(&minutes) => {
                Ok(Timestamp(now.0.saturating_add(i64::from(minutes) * 60_000)))
            }
            Expiry::In { .. } => Err(DomainError::Invalid("the lifetime must be between 1 minute and 10 years".into())),
        }
    }

    /// The sign-in names of the people `grants` mention: their holders and
    /// their issuers, and nobody else.
    async fn names(&self, grants: &[BucketGrant]) -> DomainResult<HashMap<UserId, String>> {
        let mut ids: Vec<UserId> = grants.iter().map(|g| g.user_id).collect();
        ids.extend(grants.iter().filter_map(|g| match g.created_by.parse::<Principal>() {
            Ok(Principal::User(id)) => Some(id),
            _ => None,
        }));
        ids.sort_unstable();
        ids.dedup();
        Ok(self.users.get_users(&ids).await?.iter().map(|u| (u.id, login(u))).collect())
    }

    fn view(&self, g: &BucketGrant, store: &str, names: &HashMap<UserId, String>, now: Timestamp) -> GrantView {
        let created_by_name = match g.created_by.parse::<Principal>() {
            Ok(Principal::User(id)) => names.get(&id).cloned(),
            _ => None,
        };
        GrantView {
            id: g.id,
            store_id: g.store_id,
            store: store.to_owned(),
            user_id: g.user_id,
            // An account that is gone has no name left to show.
            user: names.get(&g.user_id).cloned().unwrap_or_else(|| g.user_id.to_string()),
            bucket: g.bucket.clone(),
            access: g.access,
            created_by: g.created_by.clone(),
            created_by_name,
            created_at: g.created_at,
            expires_at: (!g.never_expires()).then_some(g.expires_at),
            never_expires: g.never_expires(),
            revoked_at: g.revoked_at,
            note: g.note.clone(),
            status: g.status(now),
        }
    }

    /// Issues a grant on `store`. The person must exist and be enabled, and
    /// must not hold a live grant for that bucket already (revoke it first to
    /// change the access or the end).
    pub async fn issue(&self, store: &ObjectStore, new: NewBucketGrant) -> DomainResult<GrantView> {
        if new.bucket.is_empty() {
            return Err(DomainError::Invalid("bucket: the name of a bucket of this store".into()));
        }
        let person = self.person(&new.user).await?;
        let now = self.clock.now();
        let expires_at = self.expires_at(new.expiry, now)?;
        // Checked, then written: two requests at the same moment can both
        // pass and leave two grants. Each is listed and revoked on its own,
        // and neither gives more than was asked for.
        let held = self.grants.list_live_bucket_grants(person.id, now).await?;
        if let Some(g) = held.iter().find(|g| g.store_id == store.id && g.bucket == new.bucket) {
            return Err(DomainError::Conflict(format!(
                "{} already has {} access to bucket `{}` (grant {}); revoke that grant first",
                login(&person),
                g.access,
                g.bucket,
                g.id
            )));
        }
        let grant = BucketGrant {
            id: BucketGrantId::new(),
            workspace_id: store.workspace_id,
            user_id: person.id,
            store_id: store.id,
            bucket: new.bucket,
            access: new.access,
            created_by: new.created_by.to_string(),
            created_at: now,
            expires_at,
            revoked_at: None,
            note: new.note.chars().filter(|c| !c.is_control()).take(MAX_NOTE).collect(),
        };
        self.grants.put_bucket_grant(&grant).await?;
        let names = self.names(std::slice::from_ref(&grant)).await?;
        Ok(self.view(&grant, &store.name, &names, now))
    }

    /// Revokes every live grant for `bucket` of `store`, and returns them as
    /// revoked. Called when the bucket is deleted, and when a bucket of that
    /// name is created: a grant names its bucket by name, and must not carry
    /// over to another bucket that later gets the name.
    pub async fn revoke_bucket(&self, store: &ObjectStore, bucket: &str) -> DomainResult<Vec<GrantView>> {
        let now = self.clock.now();
        let mut grants = self.grants.list_live_grants_of_bucket(store.id, bucket, now).await?;
        // One write per grant. A failure in between leaves the rest live;
        // the caller reports it, and the next call revokes what remains.
        for g in &mut grants {
            g.revoked_at = Some(now);
            self.grants.put_bucket_grant(g).await?;
        }
        let names = self.names(&grants).await?;
        Ok(grants.iter().map(|g| self.view(g, &store.name, &names, now)).collect())
    }

    /// Revokes a grant of `store`. Revoking twice keeps the first time.
    /// `NotFound` for a grant of another store.
    pub async fn revoke(&self, store: &ObjectStore, id: BucketGrantId) -> DomainResult<GrantView> {
        let mut grant = self
            .grants
            .get_bucket_grant(id)
            .await?
            .filter(|g| g.store_id == store.id)
            .ok_or_else(|| DomainError::NotFound("grant not found".into()))?;
        let now = self.clock.now();
        if grant.revoked_at.is_none() {
            grant.revoked_at = Some(now);
            self.grants.put_bucket_grant(&grant).await?;
        }
        let names = self.names(std::slice::from_ref(&grant)).await?;
        Ok(self.view(&grant, &store.name, &names, now))
    }

    /// The grants on `store`, newest first: the live ones, or with
    /// `all` the expired and revoked ones too. At most `limit` of them.
    pub async fn on_store(&self, store: &ObjectStore, all: bool, limit: Option<u32>) -> DomainResult<Vec<GrantView>> {
        let limit = limit.unwrap_or(LIST_DEFAULT).clamp(1, LIST_MAX);
        let now = self.clock.now();
        let rows = self.grants.list_store_bucket_grants(store.id, (!all).then_some(now), limit).await?;
        let names = self.names(&rows).await?;
        Ok(rows.iter().filter(|g| all || g.is_active(now)).map(|g| self.view(g, &store.name, &names, now)).collect())
    }

    /// The live grants `user` holds: none for an account that is disabled or
    /// gone.
    pub async fn held(&self, user: UserId) -> DomainResult<Vec<BucketGrant>> {
        match self.users.get_user(user).await {
            Ok(u) if !u.disabled => {}
            Ok(_) | Err(DomainError::NotFound(_)) => return Ok(Vec::new()),
            Err(e) => return Err(e),
        }
        let now = self.clock.now();
        let mut grants = self.grants.list_live_bucket_grants(user, now).await?;
        // The store filters by time already; the rule itself decides.
        grants.retain(|g| g.user_id == user && g.is_active(now));
        Ok(grants)
    }

    /// The live grants `user` holds on one store.
    pub async fn held_on(&self, user: UserId, store: ObjectStoreId) -> DomainResult<Vec<BucketGrant>> {
        let mut grants = self.held(user).await?;
        grants.retain(|g| g.store_id == store);
        Ok(grants)
    }

    /// The grant that lets `user` do what needs `need` in `bucket` of
    /// `store` right now, if one does.
    pub async fn allowance(
        &self,
        user: UserId,
        store: ObjectStoreId,
        bucket: &str,
        need: BucketAccess,
    ) -> DomainResult<Option<BucketGrant>> {
        let now = self.clock.now();
        Ok(self.held(user).await?.into_iter().find(|g| g.covers(user, store, bucket, need, now)))
    }

    /// The live grants of `user` in `workspace` as their holder sees them,
    /// for "what may I open". A grant whose store is gone is left out.
    pub async fn mine(&self, user: UserId, workspace: WorkspaceId) -> DomainResult<Vec<MyGrantView>> {
        let grants = self.held(user).await?;
        if grants.is_empty() {
            return Ok(Vec::new());
        }
        let stores: HashMap<ObjectStoreId, String> =
            self.stores.list_object_stores(Some(workspace)).await?.into_iter().map(|s| (s.id, s.name)).collect();
        let now = self.clock.now();
        Ok(grants
            .iter()
            .filter_map(|g| {
                stores.get(&g.store_id).map(|store| MyGrantView {
                    id: g.id,
                    store_id: g.store_id,
                    store: store.clone(),
                    bucket: g.bucket.clone(),
                    access: g.access,
                    expires_at: (!g.never_expires()).then_some(g.expires_at),
                    status: g.status(now),
                })
            })
            .collect())
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)] // tests
mod tests {
    use std::time::Duration;

    use forge_core::ManualClock;
    use forge_store::Store;

    use super::*;

    struct Fixture {
        grants: BucketGrants,
        store: Store,
        clock: Arc<ManualClock>,
        media: ObjectStore,
        ada: User,
        admin: Principal,
        _dir: tempfile::TempDir,
    }

    fn user(name: &str, email: Option<&str>) -> User {
        User {
            id: UserId::new(),
            username: Some(name.into()),
            email: email.map(str::to_owned),
            display_name: name.into(),
            password_hash: None,
            totp_secret_sealed: None,
            oidc_subject: None,
            disabled: false,
            created_at: Timestamp(1),
        }
    }

    fn object_store(ws: WorkspaceId, name: &str) -> ObjectStore {
        ObjectStore {
            id: ObjectStoreId::new(),
            workspace_id: ws,
            environment_id: None,
            name: name.into(),
            slug: name.to_lowercase(),
            provider: "rustfs".into(),
            mode: "managed".into(),
            region: "us-east-1".into(),
            path_style: true,
            version: "latest".into(),
            deployed_version: None,
            runtime: "native".into(),
            topology: "standalone".into(),
            s3_port: 9000,
            console_port: None,
            endpoint: "http://node:9000".into(),
            credential_id: forge_core::VaultItemId::new(),
            status: "running".into(),
            last_error: None,
            last_job_id: None,
            created_by: Principal::User(UserId::new()),
            created_at: Timestamp(1),
            updated_at: Timestamp(1),
        }
    }

    async fn fixture() -> Fixture {
        use forge_core::ports::TenancyRepository;
        let dir = tempfile::tempdir().unwrap();
        let store = Store::open_sqlite(&dir.path().join("forge.db")).await.unwrap();
        let ws = forge_core::model::Workspace {
            id: WorkspaceId::new(),
            name: "Acme".into(),
            slug: "acme".into(),
            created_at: Timestamp(1),
        };
        store.create_workspace(&ws).await.unwrap();
        let (ada, root) = (user("ada", Some("ada@example.com")), user("root", None));
        store.create_user(&ada).await.unwrap();
        store.create_user(&root).await.unwrap();
        let media = object_store(ws.id, "Media");
        store.put_object_store(&media).await.unwrap();
        let clock = Arc::new(ManualClock::new(Timestamp(1_000_000)));
        let shared = Arc::new(store.clone());
        let grants = BucketGrants::new(shared.clone(), shared.clone(), shared, clock.clone());
        Fixture { grants, store, clock, media, ada, admin: Principal::User(root.id), _dir: dir }
    }

    impl Fixture {
        fn ask(&self, user: &str, bucket: &str, access: BucketAccess, expiry: Expiry) -> NewBucketGrant {
            NewBucketGrant {
                user: user.into(),
                bucket: bucket.into(),
                access,
                expiry,
                note: String::new(),
                created_by: self.admin.clone(),
            }
        }

        async fn may(&self, bucket: &str, need: BucketAccess) -> Option<BucketGrantId> {
            self.grants.allowance(self.ada.id, self.media.id, bucket, need).await.unwrap().map(|g| g.id)
        }
    }

    const READ: BucketAccess = BucketAccess::Read;
    const WRITE: BucketAccess = BucketAccess::ReadWrite;

    #[tokio::test]
    async fn a_read_grant_reads_its_bucket_and_nothing_else() {
        let f = fixture().await;
        assert_eq!(f.may("photos", READ).await, None, "no grant, no access");
        let g = f.grants.issue(&f.media, f.ask("ada", "photos", READ, Expiry::Never)).await.unwrap();
        assert_eq!((g.user.as_str(), g.store.as_str(), g.status), ("ada", "Media", GrantStatus::Active));
        assert_eq!((g.expires_at, g.never_expires), (None, true));
        assert_eq!(g.created_by_name.as_deref(), Some("root"));
        assert_eq!(f.may("photos", READ).await, Some(g.id));
        assert_eq!(f.may("photos", WRITE).await, None, "read does not upload or delete");
        assert_eq!(f.may("videos", READ).await, None, "another bucket");
        assert_eq!(f.may("Photos", READ).await, None, "bucket names are exact");
        // Another store with a bucket of the same name, and another person.
        let logs = object_store(f.media.workspace_id, "Logs");
        f.store.put_object_store(&logs).await.unwrap();
        assert!(f.grants.allowance(f.ada.id, logs.id, "photos", READ).await.unwrap().is_none());
        assert!(f.grants.allowance(UserId::new(), f.media.id, "photos", READ).await.unwrap().is_none());
        assert!(f.grants.held_on(f.ada.id, logs.id).await.unwrap().is_empty());
        assert_eq!(f.grants.held_on(f.ada.id, f.media.id).await.unwrap().len(), 1);
    }

    #[tokio::test]
    async fn readwrite_also_writes() {
        let f = fixture().await;
        let g = f.grants.issue(&f.media, f.ask("ada@example.com", "photos", WRITE, Expiry::Never)).await.unwrap();
        assert_eq!(g.user_id, f.ada.id, "an email names the person too");
        assert_eq!(f.may("photos", READ).await, Some(g.id));
        assert_eq!(f.may("photos", WRITE).await, Some(g.id));
    }

    #[tokio::test]
    async fn a_grant_ends_at_its_expiry_by_the_clock() {
        let f = fixture().await;
        let g = f.grants.issue(&f.media, f.ask("ada", "photos", WRITE, Expiry::In { minutes: 30 })).await.unwrap();
        assert_eq!(g.expires_at, Some(Timestamp(1_000_000 + 30 * 60_000)));
        f.clock.advance(Duration::from_secs(30 * 60) - Duration::from_millis(1));
        assert_eq!(f.may("photos", WRITE).await, Some(g.id), "the last millisecond still counts");
        f.clock.advance(Duration::from_millis(1));
        assert_eq!(f.may("photos", READ).await, None, "expired at its end");
        assert!(f.grants.held(f.ada.id).await.unwrap().is_empty());
        assert!(f.grants.mine(f.ada.id, f.media.workspace_id).await.unwrap().is_empty());
        // The store's list keeps it for the record, as expired.
        assert!(f.grants.on_store(&f.media, false, None).await.unwrap().is_empty());
        let all = f.grants.on_store(&f.media, true, None).await.unwrap();
        assert_eq!(all.iter().map(|v| (v.id, v.status)).collect::<Vec<_>>(), [(g.id, GrantStatus::Expired)]);
        // An expired grant no longer stands in the way of a new one.
        f.grants.issue(&f.media, f.ask("ada", "photos", READ, Expiry::Never)).await.unwrap();
    }

    #[tokio::test]
    async fn a_revoked_grant_stops_at_once_and_stays_revoked() {
        let f = fixture().await;
        let g = f.grants.issue(&f.media, f.ask("ada", "photos", WRITE, Expiry::Never)).await.unwrap();
        let other = object_store(f.media.workspace_id, "Logs");
        let wrong = f.grants.revoke(&other, g.id).await;
        assert!(matches!(wrong, Err(DomainError::NotFound(_))), "a grant is revoked on its own store: {wrong:?}");
        assert_eq!(f.may("photos", READ).await, Some(g.id));
        let first = f.grants.revoke(&f.media, g.id).await.unwrap();
        assert_eq!((first.status, first.revoked_at), (GrantStatus::Revoked, Some(Timestamp(1_000_000))));
        assert_eq!(f.may("photos", READ).await, None);
        f.clock.advance(Duration::from_secs(5));
        let again = f.grants.revoke(&f.media, g.id).await.unwrap();
        assert_eq!(again.revoked_at, first.revoked_at, "the first revocation is kept");
        let missing = f.grants.revoke(&f.media, BucketGrantId::new()).await;
        assert!(matches!(missing, Err(DomainError::NotFound(_))), "{missing:?}");
    }

    #[tokio::test]
    async fn a_disabled_account_holds_no_grant() {
        let f = fixture().await;
        let g = f.grants.issue(&f.media, f.ask("ada", "photos", READ, Expiry::Never)).await.unwrap();
        assert_eq!(f.may("photos", READ).await, Some(g.id));
        // Written directly: only service accounts have a switch for it.
        sqlx::query("UPDATE users SET disabled = 1 WHERE id = $1")
            .bind(f.ada.id.to_string())
            .execute(f.store.pool())
            .await
            .unwrap();
        assert_eq!(f.may("photos", READ).await, None, "the grant is still stored, and counts for nothing");
        assert!(f.grants.held(f.ada.id).await.unwrap().is_empty());
        assert!(f.grants.mine(f.ada.id, f.media.workspace_id).await.unwrap().is_empty());
        assert_eq!(f.grants.on_store(&f.media, false, None).await.unwrap().len(), 1, "administrators still see it");
        // And nobody hands a new grant to a disabled account. The answer is
        // the one an unknown account gets.
        let refused = f.grants.issue(&f.media, f.ask("ada", "videos", READ, Expiry::Never)).await;
        let unknown = f.grants.issue(&f.media, f.ask("nobody", "videos", READ, Expiry::Never)).await;
        assert!(matches!(&refused, Err(DomainError::NotFound(m)) if m == UNKNOWN_USER), "{refused:?}");
        assert_eq!(format!("{refused:?}"), format!("{unknown:?}"));
    }

    #[tokio::test]
    async fn requests_are_validated() {
        let f = fixture().await;
        let issue = |ask: NewBucketGrant| async { f.grants.issue(&f.media, ask).await };
        let unknown = issue(f.ask("nobody", "photos", READ, Expiry::Never)).await;
        assert!(matches!(unknown, Err(DomainError::NotFound(_))), "{unknown:?}");
        let unknown = issue(f.ask(&UserId::new().to_string(), "photos", READ, Expiry::Never)).await;
        assert!(matches!(unknown, Err(DomainError::NotFound(_))), "{unknown:?}");
        for bad in [Expiry::At(Timestamp(1_000_000)), Expiry::At(NEVER_EXPIRES), Expiry::In { minutes: 0 }] {
            let r = issue(f.ask("ada", "photos", READ, bad)).await;
            assert!(matches!(r, Err(DomainError::Invalid(_))), "{bad:?}: {r:?}");
        }
        let r = issue(f.ask("ada", "", READ, Expiry::Never)).await;
        assert!(matches!(r, Err(DomainError::Invalid(_))), "{r:?}");
        let r = issue(f.ask(" ", "photos", READ, Expiry::Never)).await;
        assert!(matches!(r, Err(DomainError::Invalid(_))), "{r:?}");

        let mut ask = f.ask(&f.ada.id.to_string(), "photos", READ, Expiry::At(Timestamp(2_000_000)));
        ask.note = format!("line one\nline two{}", "x".repeat(300));
        let g = issue(ask).await.unwrap();
        assert_eq!(g.expires_at, Some(Timestamp(2_000_000)));
        assert_eq!(g.note.chars().count(), MAX_NOTE);
        assert!(!g.note.contains('\n'), "control characters are dropped");
        // One live grant per person and bucket.
        let twice = issue(f.ask("ada", "photos", WRITE, Expiry::Never)).await;
        assert!(matches!(twice, Err(DomainError::Conflict(_))), "{twice:?}");
        f.clock.advance(Duration::from_secs(1));
        issue(f.ask("ada", "videos", WRITE, Expiry::Never)).await.unwrap();
        let mine = f.grants.mine(f.ada.id, f.media.workspace_id).await.unwrap();
        assert_eq!(mine.iter().map(|v| v.bucket.as_str()).collect::<Vec<_>>(), ["videos", "photos"]);
        assert!(f.grants.mine(f.ada.id, WorkspaceId::new()).await.unwrap().is_empty(), "another workspace");
        assert_eq!(f.grants.on_store(&f.media, false, Some(1)).await.unwrap().len(), 1, "the list is bounded");
    }

    #[tokio::test]
    async fn the_grants_of_a_bucket_end_with_the_bucket() {
        let f = fixture().await;
        let bob = user("bob", None);
        f.store.create_user(&bob).await.unwrap();
        let photos = f.grants.issue(&f.media, f.ask("ada", "photos", WRITE, Expiry::Never)).await.unwrap();
        let videos = f.grants.issue(&f.media, f.ask("ada", "videos", READ, Expiry::Never)).await.unwrap();
        let other = f.grants.issue(&f.media, f.ask("bob", "photos", READ, Expiry::In { minutes: 5 })).await.unwrap();
        let old = f.grants.issue(&f.media, f.ask("bob", "videos", READ, Expiry::Never)).await.unwrap();
        f.grants.revoke(&f.media, old.id).await.unwrap();
        f.clock.advance(Duration::from_secs(1));

        let revoked = f.grants.revoke_bucket(&f.media, "photos").await.unwrap();
        let mut ids: Vec<BucketGrantId> = revoked.iter().map(|v| v.id).collect();
        ids.sort();
        let mut want = vec![photos.id, other.id];
        want.sort();
        assert_eq!(ids, want, "every holder of that bucket, and no other bucket");
        assert!(revoked.iter().all(|v| v.status == GrantStatus::Revoked && v.revoked_at == Some(f.clock.now())));
        assert!(revoked.iter().any(|v| v.user == "bob"), "the views name the holders");
        assert_eq!(f.may("photos", READ).await, None);
        assert_eq!(f.may("videos", READ).await, Some(videos.id), "another bucket keeps its grants");
        // Nothing live is left: a second call, as when the bucket is created
        // again, finds nothing, and a new grant can be issued.
        assert!(f.grants.revoke_bucket(&f.media, "photos").await.unwrap().is_empty());
        assert_eq!(f.may("photos", READ).await, None, "a bucket with the old name is not open to the old holder");
        f.grants.issue(&f.media, f.ask("ada", "photos", READ, Expiry::Never)).await.unwrap();
    }

    #[tokio::test]
    async fn a_holder_sees_what_they_may_open_and_nothing_about_who_gave_it() {
        let f = fixture().await;
        let mut ask = f.ask("ada", "photos", READ, Expiry::In { minutes: 30 });
        ask.note = "for the audit".into();
        let g = f.grants.issue(&f.media, ask).await.unwrap();
        let mine = f.grants.mine(f.ada.id, f.media.workspace_id).await.unwrap();
        let json = serde_json::to_value(&mine).unwrap();
        let mut keys: Vec<&str> = json[0].as_object().unwrap().keys().map(String::as_str).collect();
        keys.sort_unstable();
        assert_eq!(keys, ["access", "bucket", "expires_at", "id", "status", "store", "store_id"]);
        assert_eq!(mine[0].id, g.id);
        assert_eq!((mine[0].store.as_str(), mine[0].expires_at), ("Media", g.expires_at));
        assert!(!json.to_string().contains("for the audit") && !json.to_string().contains("root"));
    }
}
