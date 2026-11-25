//! Access profiles, revisions, assignments and service accounts (migration
//! 0019, D-086).

use forge_core::model::User;
use forge_core::permissions::*;
use forge_core::ports::UserRepository;
use forge_core::*;
use forge_store::Store;

async fn store() -> (Store, tempfile::TempDir) {
    let dir = tempfile::tempdir().unwrap();
    let s = Store::open_sqlite(&dir.path().join("forge.db")).await.unwrap();

    (s, dir)
}

async fn user(s: &Store, name: &str) -> UserId {
    let u = User {
        id: UserId::new(),
        username: Some(name.into()),
        email: None,
        display_name: name.into(),
        password_hash: None,
        totp_secret_sealed: None,
        oidc_subject: None,
        disabled: false,
        created_at: Timestamp(1),
    };
    s.create_user(&u).await.unwrap();

    u.id
}

fn profile(name: &str, builtin: bool) -> (PermissionProfile, ProfileRevision) {
    let id = PermissionProfileId::new();
    (
        PermissionProfile {
            id,
            name: name.into(),
            description: "d".into(),
            builtin,
            revision: 1,
            archived_at: None,
            created_by: "user:x".into(),
            created_at: Timestamp(1),
            updated_at: Timestamp(1),
        },
        ProfileRevision {
            profile_id: id,
            revision: 1,
            actions: vec![Action::View],
            created_by: "user:x".into(),
            created_at: Timestamp(1),
        },
    )
}

#[tokio::test]
async fn revisions_publish_in_order_and_built_in_profiles_are_read_only() {
    let (s, _d) = store().await;
    let (p, r1) = profile("UAT Support", false);
    s.create_profile(&p, &r1).await.unwrap();
    let r2 = ProfileRevision { revision: 2, actions: vec![Action::ServerManage, Action::View], ..r1.clone() };
    s.publish_revision(&r2, Timestamp(5)).await.unwrap();

    // A stale publish (revision 2 again) conflicts instead of overwriting.
    assert!(matches!(s.publish_revision(&r2, Timestamp(6)).await, Err(DomainError::Conflict(_))));
    let got = s.get_profile(p.id).await.unwrap().unwrap();
    assert_eq!((got.revision, got.updated_at), (2, Timestamp(5)));
    assert_eq!(s.get_revision(p.id, 2).await.unwrap().unwrap().actions, vec![Action::ServerManage, Action::View]);
    assert_eq!(s.list_revisions(p.id).await.unwrap().len(), 2, "history is kept");

    // Names are unique.
    let (dup, dup_r) = profile("UAT Support", false);
    assert!(s.create_profile(&dup, &dup_r).await.is_err());

    // Built-in: no publish, no archive.
    let (b, b1) = profile("Operator", true);
    s.create_profile(&b, &b1).await.unwrap();
    let b2 = ProfileRevision { revision: 2, ..b1 };
    assert!(matches!(s.publish_revision(&b2, Timestamp(7)).await, Err(DomainError::Conflict(_))));
    assert!(matches!(s.archive_profile(b.id, Timestamp(7)).await, Err(DomainError::Conflict(_))));

    // Archived: no more publishing; archiving twice conflicts.
    s.archive_profile(p.id, Timestamp(8)).await.unwrap();
    let r3 = ProfileRevision { revision: 3, ..r2 };
    assert!(s.publish_revision(&r3, Timestamp(9)).await.is_err());
    assert!(s.archive_profile(p.id, Timestamp(9)).await.is_err());
}

#[tokio::test]
async fn assignments_and_service_accounts_round_trip() {
    let (s, _d) = store().await;
    let (p, r1) = profile("Readers", false);
    s.create_profile(&p, &r1).await.unwrap();
    let alice = user(&s, "alice").await;
    let a = ProfileAssignment {
        id: ProfileAssignmentId::new(),
        user_id: Some(alice),
        group_id: None,
        profile_id: p.id,
        scope: EntityRef::new(EntityKind::Environment, EnvironmentId::new()),
        expires_at: Some(Timestamp(100)),
        created_by: "user:x".into(),
        created_at: Timestamp(1),
    };
    s.create_assignment(&a, 1).await.unwrap();
    assert_eq!(s.list_assignments().await.unwrap(), vec![a.clone()]);
    assert!(a.expired(Timestamp(100)) && !a.expired(Timestamp(99)));

    // Fail closed: a stale revision or an archived profile stores nothing.
    let stale = ProfileAssignment { id: ProfileAssignmentId::new(), ..a.clone() };
    assert!(matches!(s.create_assignment(&stale, 2).await, Err(DomainError::Conflict(_))));
    assert!(s.delete_assignment(a.id).await.unwrap());
    assert!(!s.delete_assignment(a.id).await.unwrap(), "idempotent");

    let robot_user = User {
        id: UserId::new(),
        username: Some("svc-ci-deploy".into()),
        email: None,
        display_name: "svc-ci-deploy".into(),
        password_hash: None,
        totp_secret_sealed: None,
        oidc_subject: None,
        disabled: false,
        created_at: Timestamp(2),
    };
    let robot = robot_user.id;
    let sa = ServiceAccount {
        user_id: robot,
        owner_id: alice,
        description: "deploys from CI".into(),
        created_by: format!("user:{alice}"),
        created_at: Timestamp(2),
    };
    s.create_service_account(&robot_user, &sa).await.unwrap();
    assert_eq!(s.get_service_account(robot).await.unwrap(), Some(sa.clone()));
    assert_eq!(s.list_service_accounts().await.unwrap(), vec![sa]);
    assert_eq!(s.get_service_account(alice).await.unwrap(), None);
    s.archive_profile(p.id, Timestamp(9)).await.unwrap();
    let late = ProfileAssignment { id: ProfileAssignmentId::new(), ..a };
    assert!(matches!(s.create_assignment(&late, 1).await, Err(DomainError::Conflict(_))), "archived");
}

#[tokio::test]
async fn a_hand_edited_unknown_action_never_loads() {
    let (s, _d) = store().await;
    let (p, r1) = profile("Edited", false);
    s.create_profile(&p, &r1).await.unwrap();
    sqlx::query("UPDATE permission_profile_revisions SET actions = '[\"view\",\"everything\"]'")
        .execute(s.pool())
        .await
        .unwrap();
    assert!(s.get_revision(p.id, 1).await.is_err());
}

#[tokio::test]
async fn groups_hold_members_and_assignments_and_delete_atomically() {
    use forge_core::permissions::Group;
    let (s, _d) = store().await;
    let (p, r1) = profile("Support", false);
    s.create_profile(&p, &r1).await.unwrap();
    let (alice, bob) = (user(&s, "alice").await, user(&s, "bob").await);
    let g = Group {
        id: GroupId::new(),
        name: "UAT support".into(),
        description: String::new(),
        created_by: "user:x".into(),
        created_at: Timestamp(1),
    };
    s.create_group(&g).await.unwrap();
    let dup = Group { id: GroupId::new(), name: "uat SUPPORT".into(), ..g.clone() };
    assert!(matches!(s.create_group(&dup).await, Err(DomainError::Conflict(_))), "names are case-insensitive");
    assert!(s.add_member(g.id, alice, Timestamp(2)).await.unwrap());
    assert!(!s.add_member(g.id, alice, Timestamp(2)).await.unwrap(), "idempotent");
    assert!(s.add_member(g.id, bob, Timestamp(3)).await.unwrap());
    assert!(!s.add_member(GroupId::new(), bob, Timestamp(3)).await.unwrap(), "unknown group");
    assert_eq!(s.list_memberships().await.unwrap(), vec![(g.id, alice), (g.id, bob)]);
    assert!(s.remove_member(g.id, bob).await.unwrap());
    assert!(!s.remove_member(g.id, bob).await.unwrap());

    let a = ProfileAssignment {
        id: ProfileAssignmentId::new(),
        user_id: None,
        group_id: Some(g.id),
        profile_id: p.id,
        scope: EntityRef::new(EntityKind::Cluster, ClusterId::new()),
        expires_at: None,
        created_by: "user:x".into(),
        created_at: Timestamp(4),
    };
    s.create_assignment(&a, 1).await.unwrap();
    assert_eq!(s.list_assignments().await.unwrap(), vec![a.clone()]);
    let neither = ProfileAssignment { id: ProfileAssignmentId::new(), group_id: None, ..a.clone() };
    assert!(s.create_assignment(&neither, 1).await.is_err(), "needs a user or a group");
    let orphan = ProfileAssignment { id: ProfileAssignmentId::new(), group_id: Some(GroupId::new()), ..a.clone() };
    assert!(matches!(s.create_assignment(&orphan, 1).await, Err(DomainError::Conflict(_))), "group must exist");

    assert_eq!(s.delete_group(g.id).await.unwrap(), vec![a.id]);
    assert!(s.list_assignments().await.unwrap().is_empty());
    assert!(s.list_memberships().await.unwrap().is_empty());
    assert!(s.get_group(g.id).await.unwrap().is_none());
}
