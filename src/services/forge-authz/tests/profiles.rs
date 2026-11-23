//! Access profiles (D-086): built-in profiles decide exactly like the roles
//! they mirror, custom profiles grant only their listed actions, and no
//! user input reaches policy text.
#![allow(clippy::unwrap_used)]

use forge_authz::PolicyEngine;
use forge_core::model::RoleBinding;
use forge_core::permissions::ProfileAssignment;
use forge_core::*;

fn ws() -> EntityRef {
    EntityRef::new(EntityKind::Workspace, "01900000-0000-7000-8000-0000000000aa")
}

/// One resource of each kind, placed under the workspace.
fn resources() -> Vec<ResourceRef> {
    let ns = EntityRef::new(EntityKind::Environment, "01900000-0000-7000-8000-0000000000b1");
    let cl = EntityRef::new(EntityKind::Cluster, "01900000-0000-7000-8000-0000000000c1");
    let srv = EntityRef::new(EntityKind::Server, "01900000-0000-7000-8000-0000000000d1");
    let app = EntityRef::new(EntityKind::Service, "01900000-0000-7000-8000-0000000000e1");
    let job = EntityRef::new(EntityKind::Job, "01900000-0000-7000-8000-0000000000f1");
    vec![
        ResourceRef::new(EntityRef::platform(), vec![]),
        ResourceRef::new(ws(), vec![]),
        ResourceRef::new(ns.clone(), vec![ws()]),
        ResourceRef::new(cl.clone(), vec![ws()]),
        ResourceRef::new(srv, vec![ns.clone(), ws()]),
        ResourceRef::new(app, vec![ns.clone(), ws()]),
        ResourceRef::new(job, vec![ns, ws()]),
    ]
}

fn allowed(e: &PolicyEngine, user: UserId, action: Action, r: &ResourceRef, mfa: bool) -> bool {
    e.evaluate(&Principal::User(user), action, r, AuthzContext { mfa }).is_ok_and(|d| d.allowed)
}

fn assign(
    e: &PolicyEngine,
    user: UserId,
    profile: PermissionProfileId,
    revision: u32,
    scope: EntityRef,
) -> ProfileAssignment {
    let a = ProfileAssignment {
        id: ProfileAssignmentId::new(),
        user_id: Some(user),
        group_id: None,
        profile_id: profile,
        scope,
        expires_at: None,
        created_by: "test".into(),
        created_at: Timestamp(0),
    };
    e.link_assignment(&a, revision).unwrap();
    a
}

/// For every role, a role binding and an assignment of the built-in profile
/// made from that role give identical decisions for every action, resource
/// kind and MFA state, at platform and at workspace scope.
#[test]
fn builtin_profiles_decide_exactly_like_their_roles() {
    let e = PolicyEngine::new().unwrap();
    for role in Role::ALL {
        let actions = e.role_actions(role).unwrap();
        assert!(!actions.is_empty(), "{role:?} grants something");
        let profile = PermissionProfileId::new();
        e.add_profile_revision(profile, 1, &actions).unwrap();
        let scopes = [
            EntityRef::platform(),
            ws(),
            EntityRef::new(EntityKind::Environment, "01900000-0000-7000-8000-0000000000b1"),
            EntityRef::new(EntityKind::Cluster, "01900000-0000-7000-8000-0000000000c1"),
            EntityRef::new(EntityKind::Server, "01900000-0000-7000-8000-0000000000d1"),
            EntityRef::new(EntityKind::Service, "01900000-0000-7000-8000-0000000000e1"),
        ];
        for scope in scopes {
            let (by_role, by_profile) = (UserId::new(), UserId::new());
            let b = RoleBinding {
                id: RoleBindingId::new(),
                user_id: by_role,
                role,
                scope: scope.clone(),
                created_at: Timestamp(0),
            };
            e.link_binding(&b).unwrap();
            assign(&e, by_profile, profile, 1, scope.clone());
            for action in Action::ALL {
                for r in &resources() {
                    for mfa in [false, true] {
                        assert_eq!(
                            allowed(&e, by_role, action, r, mfa),
                            allowed(&e, by_profile, action, r, mfa),
                            "{role:?} at {scope}: {action} on {} (mfa {mfa})",
                            r.entity
                        );
                    }
                }
            }
        }
    }
}

#[test]
fn a_custom_profile_grants_only_its_actions_at_its_scope_and_mfa_still_applies() {
    let e = PolicyEngine::new().unwrap();
    let profile = PermissionProfileId::new();
    e.add_profile_revision(profile, 1, &[Action::View, Action::TerminalOpen]).unwrap();
    let user = UserId::new();
    let ns = EntityRef::new(EntityKind::Environment, "01900000-0000-7000-8000-0000000000b1");
    let a = assign(&e, user, profile, 1, ns.clone());
    let srv =
        ResourceRef::new(EntityRef::new(EntityKind::Server, "01900000-0000-7000-8000-0000000000d1"), vec![ns, ws()]);
    let other =
        ResourceRef::new(EntityRef::new(EntityKind::Server, "01900000-0000-7000-8000-0000000000d2"), vec![ws()]);
    assert!(allowed(&e, user, Action::View, &srv, false));
    assert!(!allowed(&e, user, Action::ServerManage, &srv, true), "not listed");
    assert!(!allowed(&e, user, Action::View, &other, true), "outside the scope");
    assert!(!allowed(&e, user, Action::TerminalOpen, &srv, false), "the MFA guardrail still applies");
    assert!(allowed(&e, user, Action::TerminalOpen, &srv, true));

    // Publishing revision 2 moves the assignment; unlinking removes it.
    e.add_profile_revision(profile, 2, &[Action::View]).unwrap();
    e.link_assignment(&a, 2).unwrap();
    assert!(!allowed(&e, user, Action::TerminalOpen, &srv, true), "revision 2 dropped it");
    assert!(allowed(&e, user, Action::View, &srv, false));
    e.unlink_assignment(a.id).unwrap();
    assert!(!allowed(&e, user, Action::View, &srv, true), "unlinked");
    e.unlink_assignment(a.id).unwrap();
}

/// Lead review: crafted names, actions or scopes never become policy text.
/// Actions are typed (`Action`), scopes are parsed from an allow-list with
/// real ids, and the scope and principal are template slots, not text.
#[test]
fn crafted_input_cannot_inject_policy_text() {
    use forge_core::permissions::{parse_actions, parse_scope};
    for bad in [
        "view\"]; permit(principal, action, resource); //",
        "view, Forge::Action::\"secret.read\"",
        "role:platform_admin",
        "view\n); permit(principal, action, resource);",
        "view\r\nforbid",
    ] {
        assert!(parse_actions(&[bad]).is_err(), "{bad}");
    }
    for bad in [
        "environment:x\" || true",
        "environment:01900000-0000-7000-8000-0000000000b1\"); permit(principal,action,resource);//",
        "platform:root",
        "environment:01900000-0000-7000-8000-0000000000b1\n); permit(principal, action, resource);",
    ] {
        assert!(parse_scope(bad).is_err(), "{bad}");
    }
    // Even a scope id with quotes (if one reached the engine) is an entity id
    // in a slot: it matches nothing and grants nothing beyond itself.
    let e = PolicyEngine::new().unwrap();
    let profile = PermissionProfileId::new();
    e.add_profile_revision(profile, 1, &[Action::View]).unwrap();
    let user = UserId::new();
    assign(&e, user, profile, 1, EntityRef::new(EntityKind::Environment, "x\") || true || (\""));
    for r in &resources() {
        assert!(!allowed(&e, user, Action::View, r, true), "{}", r.entity);
    }
}

/// Expiry is judged by the engine's injected clock at every decision, with
/// no sweep involved.
#[test]
fn an_assignment_stops_granting_when_the_clock_passes_its_expiry() {
    let clock = std::sync::Arc::new(ManualClock::new(Timestamp(1_000)));
    let e = PolicyEngine::with_clock(clock.clone()).unwrap();
    let profile = PermissionProfileId::new();
    e.add_profile_revision(profile, 1, &[Action::View]).unwrap();
    let user = UserId::new();
    let a = ProfileAssignment {
        id: ProfileAssignmentId::new(),
        user_id: Some(user),
        group_id: None,
        profile_id: profile,
        scope: ws(),
        expires_at: Some(Timestamp(2_000)),
        created_by: "test".into(),
        created_at: Timestamp(0),
    };
    e.link_assignment(&a, 1).unwrap();
    let r = ResourceRef::new(ws(), vec![]);
    assert!(allowed(&e, user, Action::View, &r, false), "before expiry");
    clock.set(Timestamp(2_000));
    assert!(!allowed(&e, user, Action::View, &r, false), "at expiry");
    // An assignment already expired is never linked.
    e.link_assignment(&a, 1).unwrap();
    assert!(!allowed(&e, user, Action::View, &r, false));
}

fn assign_group(e: &PolicyEngine, group: GroupId, profile: PermissionProfileId, scope: EntityRef) -> ProfileAssignment {
    let a = ProfileAssignment {
        id: ProfileAssignmentId::new(),
        user_id: None,
        group_id: Some(group),
        profile_id: profile,
        scope,
        expires_at: None,
        created_by: "test".into(),
        created_at: Timestamp(0),
    };
    e.link_assignment(&a, 1).unwrap();
    a
}

/// #13: a profile assigned to a group grants every member at that scope
/// only; a user in two groups gets the union; leaving a group or deleting
/// it ends the grant at the next decision.
#[test]
fn group_assignments_grant_members_only_at_their_scope() {
    let e = PolicyEngine::new().unwrap();
    let ns_a = EntityRef::new(EntityKind::Environment, "01900000-0000-7000-8000-0000000000b1");
    let ns_b = EntityRef::new(EntityKind::Environment, "01900000-0000-7000-8000-0000000000b2");
    let srv_a = ResourceRef::new(
        EntityRef::new(EntityKind::Server, "01900000-0000-7000-8000-0000000000d1"),
        vec![ns_a.clone(), ws()],
    );
    let srv_b = ResourceRef::new(
        EntityRef::new(EntityKind::Server, "01900000-0000-7000-8000-0000000000d2"),
        vec![ns_b.clone(), ws()],
    );
    let viewer = PermissionProfileId::new();
    e.add_profile_revision(viewer, 1, &[Action::View]).unwrap();
    let restarter = PermissionProfileId::new();
    e.add_profile_revision(restarter, 1, &[Action::ContainerManage]).unwrap();

    let (support, ops) = (GroupId::new(), GroupId::new());
    let (alice, bob, carol) = (UserId::new(), UserId::new(), UserId::new());
    assign_group(&e, support, viewer, ns_a.clone());
    assign_group(&e, ops, restarter, ns_b.clone());
    e.set_memberships(&[(support, alice), (support, bob), (ops, bob)]);

    // Members get the group's actions at its scope, nowhere else.
    assert!(allowed(&e, alice, Action::View, &srv_a, false));
    assert!(!allowed(&e, alice, Action::View, &srv_b, false));
    assert!(!allowed(&e, alice, Action::ContainerManage, &srv_a, false));
    // A user in two groups gets the union.
    assert!(allowed(&e, bob, Action::View, &srv_a, false));
    assert!(allowed(&e, bob, Action::ContainerManage, &srv_b, false));
    assert!(!allowed(&e, bob, Action::ContainerManage, &srv_a, false));
    // Not a member: nothing.
    assert!(!allowed(&e, carol, Action::View, &srv_a, false));
    // Batched decisions agree.
    let many = e
        .evaluate_many(
            &Principal::User(bob),
            Action::View,
            &[srv_a.clone(), srv_b.clone()],
            AuthzContext { mfa: false },
        )
        .unwrap();
    assert_eq!(many.iter().map(|d| d.allowed).collect::<Vec<_>>(), vec![true, false]);

    // Joining and leaving take effect at the next decision.
    e.add_member(support, carol);
    assert!(allowed(&e, carol, Action::View, &srv_a, false));
    e.remove_member(support, carol);
    assert!(!allowed(&e, carol, Action::View, &srv_a, false));
    // Deleting a group ends every grant through it.
    e.drop_group(ops);
    assert!(!allowed(&e, bob, Action::ContainerManage, &srv_b, false));
    assert!(allowed(&e, bob, Action::View, &srv_a, false), "other groups still grant");
}

/// Role templates use `principal in ?principal` now; a role binding still
/// grants only the bound user, never someone sharing a group with them.
#[test]
fn role_bindings_still_grant_only_their_user() {
    let e = PolicyEngine::new().unwrap();
    let (alice, bob, g) = (UserId::new(), UserId::new(), GroupId::new());
    e.set_memberships(&[(g, alice), (g, bob)]);
    let b = RoleBinding {
        id: RoleBindingId::new(),
        user_id: alice,
        role: Role::Operator,
        scope: ws(),
        created_at: Timestamp(0),
    };
    e.link_binding(&b).unwrap();
    let r = ResourceRef::new(ws(), vec![]);
    assert!(allowed(&e, alice, Action::View, &r, false));
    assert!(!allowed(&e, bob, Action::View, &r, false));
}

/// #13: grant checks ask whether an action is held *over* a scope, so a
/// node-only action granted at an environment (or above) counts there, while a
/// grant on a sibling environment or a node below does not.
#[test]
fn holds_over_a_scope_covers_actions_of_other_resource_types() {
    let e = PolicyEngine::new().unwrap();
    let ns_a = EntityRef::new(EntityKind::Environment, "01900000-0000-7000-8000-0000000000b1");
    let ns_b = EntityRef::new(EntityKind::Environment, "01900000-0000-7000-8000-0000000000b2");
    let srv_a = EntityRef::new(EntityKind::Server, "01900000-0000-7000-8000-0000000000d1");
    let over = |u, a, r: &EntityRef, parents: Vec<EntityRef>| {
        e.holds_over(&Principal::User(u), a, &ResourceRef::new(r.clone(), parents), AuthzContext { mfa: true })
            .is_ok_and(|d| d.allowed)
    };
    let p = PermissionProfileId::new();
    e.add_profile_revision(p, 1, &[Action::ContainerManage]).unwrap();
    let (admin, local, node_only) = (UserId::new(), UserId::new(), UserId::new());
    assign(&e, admin, p, 1, ws());
    assign(&e, local, p, 1, ns_a.clone());
    assign(&e, node_only, p, 1, srv_a.clone());

    assert!(over(admin, Action::ContainerManage, &ns_a, vec![ws()]), "held at the workspace covers ns A");
    assert!(over(local, Action::ContainerManage, &ns_a, vec![ws()]));
    assert!(!over(local, Action::ContainerManage, &ns_b, vec![ws()]), "sibling environment");
    assert!(!over(node_only, Action::ContainerManage, &ns_a, vec![ws()]), "a node grant is not over its environment");
    assert!(!over(local, Action::View, &ns_a, vec![ws()]), "only listed actions");
    // Real requests still follow the schema: the action is refused on an environment.
    assert!(!allowed(&e, admin, Action::ContainerManage, &ResourceRef::new(ns_a, vec![ws()]), true));
}
