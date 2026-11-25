//! Access profiles and assignments over the API (D-086).

use crate::common::*;
use reqwest::{Method, StatusCode};
use serde_json::json;

async fn admin_mfa(h: &Harness) -> Client {
    let mut admin = Client::login(&h.base, ADMIN_EMAIL, PASSWORD).await;
    step_up(&mut admin).await;

    admin
}

#[tokio::test]
async fn built_in_profiles_mirror_the_roles_and_are_read_only() {
    let h = start().await;
    let admin = admin_mfa(&h).await;
    let (s, list) = admin.get("/api/v1/access/profiles").await;
    assert_eq!(s, StatusCode::OK, "{list}");
    let ops = list.as_array().unwrap().iter().find(|p| p["name"] == "Operator").unwrap().clone();
    assert_eq!(ops["builtin"], true);
    assert!(ops["actions"].as_array().unwrap().iter().any(|a| a == "server.manage"));
    let id = ops["id"].as_str().unwrap();
    let (s, e) = admin
        .call(
            Method::PUT,
            &format!("/api/v1/access/profiles/{id}"),
            Some(json!({ "actions": ["view"], "expected_revision": 1 })),
        )
        .await;
    assert_eq!(s, StatusCode::CONFLICT, "{e}");
    assert_eq!(
        admin.call(Method::DELETE, &format!("/api/v1/access/profiles/{id}"), None).await.0,
        StatusCode::CONFLICT
    );

    // A duplicate is an editable copy.
    let (s, copy) =
        admin.post(&format!("/api/v1/access/profiles/{id}/duplicate"), json!({ "name": "Night operator" })).await;
    assert_eq!(s, StatusCode::CREATED, "{copy}");
    assert_eq!(copy["builtin"], false);
    assert_eq!(copy["actions"], ops["actions"]);

    // The catalog lists every action with MFA and roles.
    let (_, cat) = admin.get("/api/v1/access/catalog").await;
    let term = cat["actions"].as_array().unwrap().iter().find(|a| a["action"] == "terminal.open").unwrap().clone();
    assert_eq!(term["mfa"], true);
    assert!(term["roles"].as_array().unwrap().iter().any(|r| r == "operator"));
}

#[tokio::test]
async fn an_assignment_grants_only_its_actions_and_ends_at_once() {
    let h = start().await;
    let admin = admin_mfa(&h).await;
    let ws = &h.default_org;
    let (s, p) = admin
        .post("/api/v1/access/profiles", json!({ "name": "Cluster editors", "actions": ["view", "workspace.manage"] }))
        .await;
    assert_eq!(s, StatusCode::CREATED, "{p}");
    let pid = p["id"].as_str().unwrap().to_owned();

    // A user with nothing yet.
    h.app.state.identity.create_account(None, Some("v@example.com"), "V", PASSWORD).await.unwrap();
    let v = Client::login(&h.base, "v@example.com", PASSWORD).await;
    assert_eq!(v.post("/api/v1/clusters", json!({ "name": "a" })).await.0, StatusCode::FORBIDDEN);

    let (s, a) = admin
        .post(
            "/api/v1/access/assignments",
            json!({ "login": "v@example.com", "profile_id": pid, "scope": format!("workspace:{ws}") }),
        )
        .await;
    assert_eq!(s, StatusCode::CREATED, "{a}");
    assert_eq!(v.post("/api/v1/clusters", json!({ "name": "b" })).await.0, StatusCode::CREATED, "granted at once");
    assert_eq!(v.get("/api/v1/users").await.0, StatusCode::FORBIDDEN, "nothing more");

    // Publishing a narrower revision moves the assignment at once.
    let url = format!("/api/v1/access/profiles/{pid}");
    let (s, dry) = admin
        .call(Method::PUT, &format!("{url}?dry_run=true"), Some(json!({ "actions": ["view"], "expected_revision": 1 })))
        .await;
    assert_eq!(s, StatusCode::OK, "{dry}");
    assert_eq!((dry["removed"].clone(), dry["assignments"].clone()), (json!(["workspace.manage"]), json!(1)));
    assert_eq!(
        v.post("/api/v1/clusters", json!({ "name": "c" })).await.0,
        StatusCode::CREATED,
        "a dry run changes nothing"
    );
    let (s, b) = admin.call(Method::PUT, &url, Some(json!({ "actions": ["view"], "expected_revision": 1 }))).await;
    assert_eq!(s, StatusCode::OK, "{b}");
    assert_eq!(
        v.post("/api/v1/clusters", json!({ "name": "d" })).await.0,
        StatusCode::FORBIDDEN,
        "revision 2 dropped it"
    );
    let (s, _) = admin.call(Method::PUT, &url, Some(json!({ "actions": ["view"], "expected_revision": 1 }))).await;
    assert_eq!(s, StatusCode::CONFLICT, "stale revision");

    // Archiving ends every assignment at once.
    assert_eq!(v.get("/api/v1/settings").await.0, StatusCode::OK);
    assert_eq!(admin.call(Method::DELETE, &url, None).await.0, StatusCode::OK);
    assert_eq!(v.get("/api/v1/settings").await.0, StatusCode::FORBIDDEN, "archived");
    let (_, audit) = admin.get("/api/v1/audit?limit=100").await;
    let text = audit.to_string();

    for a in ["access.profile.create", "access.assign", "access.profile.publish", "access.profile.archive"] {
        assert!(text.contains(a), "{a} audited");
    }
}

#[tokio::test]
async fn nobody_hands_out_more_than_they_hold() {
    let h = start().await;
    let admin = admin_mfa(&h).await;
    let ws = &h.default_org;
    let (_, p) =
        admin.post("/api/v1/access/profiles", json!({ "name": "Secrets", "actions": ["view", "secret.read"] })).await;
    let pid = p["id"].as_str().unwrap().to_owned();

    // A workspace admin may assign, but `system.update` is platform-admin only.
    let (_, q) =
        admin.post("/api/v1/access/profiles", json!({ "name": "Updater", "actions": ["system.update"] })).await;
    let qid = q["id"].as_str().unwrap().to_owned();
    let mut wsa = user_with_role(&h, &admin, "wsa@example.com", "workspace_admin", &format!("workspace:{ws}")).await;
    step_up(&mut wsa).await;
    h.app.state.identity.create_account(None, Some("t@example.com"), "T", PASSWORD).await.unwrap();
    let (s, e) = wsa
        .post(
            "/api/v1/access/assignments",
            json!({ "login": "t@example.com", "profile_id": qid, "scope": format!("workspace:{ws}") }),
        )
        .await;
    assert_eq!((s, e["title"].as_str()), (StatusCode::FORBIDDEN, Some("escalation")), "{e}");
    let (s, a) = wsa
        .post(
            "/api/v1/access/assignments",
            json!({ "login": "t@example.com", "profile_id": pid, "scope": format!("workspace:{ws}") }),
        )
        .await;
    assert_eq!(s, StatusCode::CREATED, "a workspace admin holds view and secret.read here: {a}");

    // Without MFA, assigning is refused; removing is not.
    let plain = user_with_role(&h, &admin, "wsa2@example.com", "workspace_admin", &format!("workspace:{ws}")).await;
    let (s, e) = plain
        .post(
            "/api/v1/access/assignments",
            json!({ "login": "t@example.com", "profile_id": pid, "scope": format!("workspace:{ws}") }),
        )
        .await;
    assert_eq!((s, e["title"].as_str()), (StatusCode::FORBIDDEN, Some("step_up_required")), "{e}");
    let id = a["id"].as_str().unwrap();
    assert_eq!(
        plain.call(Method::DELETE, &format!("/api/v1/access/assignments/{id}"), None).await.0,
        StatusCode::NO_CONTENT
    );

    // Scopes come from the allow-list.
    let (s, _) = admin
        .post(
            "/api/v1/access/assignments",
            json!({ "login": "t@example.com", "profile_id": pid, "scope": "platform:root" }),
        )
        .await;
    assert_eq!(s, StatusCode::UNPROCESSABLE_ENTITY);
}

#[tokio::test]
async fn service_accounts_use_tokens_only_and_get_access_only_by_assignment() {
    let h = start().await;
    let admin = admin_mfa(&h).await;
    let ws = &h.default_org;
    let (s, sa) =
        admin.post("/api/v1/service-accounts", json!({ "name": "ci-deploy", "description": "GitHub Actions" })).await;
    assert_eq!(s, StatusCode::CREATED, "{sa}");
    assert_eq!(sa["name"], "svc-ci-deploy");
    let id = sa["id"].as_str().unwrap().to_owned();
    let (_, again) = admin.post("/api/v1/service-accounts", json!({ "name": "ci-deploy" })).await;
    assert!(again.to_string().contains("exists"), "{again}");

    // No interactive sign-in: no password, passkey or SSO.
    let (s, _) =
        Client::anonymous(&h.base).post("/api/v1/auth/login", login_body(&h.base, "svc-ci-deploy", "").await).await;
    assert_ne!(s, StatusCode::OK);

    // A token authenticates as the account, which can do nothing yet.
    let (s, t) = admin.post(&format!("/api/v1/service-accounts/{id}/tokens"), json!({ "name": "gha" })).await;
    assert_eq!(s, StatusCode::CREATED, "{t}");
    let days = (t["expires_at"].as_i64().unwrap() - t["created_at"].as_i64().unwrap()) / 86_400_000;
    assert_eq!(days, 90, "tokens expire after 90 days by default");
    let mut ci = Client::anonymous(&h.base);
    ci.bearer = t["secret"].as_str().map(str::to_owned);
    let (s, me) = ci.get("/api/v1/me").await;
    assert_eq!(s, StatusCode::OK);
    assert_eq!(me["user"]["username"], "svc-ci-deploy");
    assert_eq!(ci.post("/api/v1/clusters", json!({ "name": "x" })).await.0, StatusCode::FORBIDDEN);

    // Access comes from an assignment, as for anyone.
    let (_, p) =
        admin.post("/api/v1/access/profiles", json!({ "name": "CI", "actions": ["view", "workspace.manage"] })).await;
    let (s, a) = admin
        .post(
            "/api/v1/access/assignments",
            json!({ "user_id": id, "profile_id": p["id"], "scope": format!("workspace:{ws}") }),
        )
        .await;
    assert_eq!(s, StatusCode::CREATED, "{a}");
    assert_eq!(ci.post("/api/v1/clusters", json!({ "name": "y" })).await.0, StatusCode::CREATED);

    // Tokens never hold MFA: guarded actions stay refused.
    let (s, _) = ci.post("/api/v1/access/profiles", json!({ "name": "z", "actions": ["view"] })).await;
    assert_eq!(s, StatusCode::FORBIDDEN);

    // Disabling revokes its tokens; enabling does not bring them back.
    let url = format!("/api/v1/service-accounts/{id}");
    let (s, b) = admin.call(Method::PATCH, &url, Some(json!({ "disabled": true }))).await;
    assert_eq!((s, b["disabled"].clone(), b["tokens"].clone()), (StatusCode::OK, json!(true), json!(0)), "{b}");
    assert_eq!(ci.get("/api/v1/me").await.0, StatusCode::UNAUTHORIZED);
    let (s, _) = admin.post(&format!("{url}/tokens"), json!({ "name": "x" })).await;
    assert_eq!(s, StatusCode::CONFLICT, "no tokens while disabled");
    admin.call(Method::PATCH, &url, Some(json!({ "disabled": false }))).await;
    assert_eq!(ci.get("/api/v1/me").await.0, StatusCode::UNAUTHORIZED, "the old token stays revoked");
    let (_, t) = admin.post(&format!("{url}/tokens"), json!({ "name": "gha2" })).await;
    ci.bearer = t["secret"].as_str().map(str::to_owned);
    assert_eq!(ci.get("/api/v1/me").await.0, StatusCode::OK, "a new token works");

    // Unknown fields are refused.
    let (s, _) = admin.post(&format!("{url}/tokens"), json!({ "name": "y", "scope": "all" })).await;
    assert_eq!(s, StatusCode::UNPROCESSABLE_ENTITY);

    // A service account's token is a personal access token: it can be
    // restricted like one, and then does only what is listed.
    let me_ws = ci.get("/api/v1/me").await.1["workspace_id"].as_str().map(str::to_owned);
    if let Some(me_ws) = me_ws {
        let body = json!({ "name": "narrow", "restrictions": [{ "scope": format!("workspace:{me_ws}"), "actions": ["audit.read"] }] });
        let (s, r) = admin.post(&format!("{url}/tokens"), body).await;
        assert_eq!(s, StatusCode::CREATED, "{r}");
        assert!(r["restrictions"].is_array(), "{r}");
        let full = ci.bearer.replace(r["secret"].as_str().unwrap_or_default().to_owned());
        assert_eq!(ci.get("/api/v1/me").await.0, StatusCode::OK);
        ci.bearer = full;
        let rid = r["id"].as_str().unwrap_or_default();
        admin.call(Method::DELETE, &format!("{url}/tokens/{rid}"), None).await;
    }

    // Revoking a token needs no MFA and ends it.
    let tid = t["id"].as_str().unwrap();
    assert_eq!(admin.call(Method::DELETE, &format!("{url}/tokens/{tid}"), None).await.0, StatusCode::NO_CONTENT);
    assert_eq!(ci.get("/api/v1/me").await.0, StatusCode::UNAUTHORIZED);

    // Only access.manage creates them; a person can't be disabled through it.
    let dev = user_with_role(&h, &admin, "dev@example.com", "developer", &format!("workspace:{ws}")).await;
    assert_eq!(dev.post("/api/v1/service-accounts", json!({ "name": "evil" })).await.0, StatusCode::FORBIDDEN);
    let (_, me) = admin.get("/api/v1/me").await;
    let person = me["user"]["id"].as_str().unwrap();
    let (s, _) = admin
        .call(Method::PATCH, &format!("/api/v1/service-accounts/{person}"), Some(json!({ "disabled": true })))
        .await;
    assert_eq!(s, StatusCode::NOT_FOUND, "people are not service accounts");

    let (_, audit) = admin.get("/api/v1/audit?limit=100").await;
    let text = audit.to_string();

    for a in [
        "service_account.create",
        "service_account.token.create",
        "service_account.disable",
        "service_account.token.revoke",
    ] {
        assert!(text.contains(a), "{a} audited");
    }

    assert!(!text.contains(t["secret"].as_str().unwrap()), "no token in the audit log");
}

#[tokio::test]
async fn service_account_tokens_are_capped() {
    let h = start().await;
    let admin = admin_mfa(&h).await;
    let (_, sa) = admin.post("/api/v1/service-accounts", json!({ "name": "many" })).await;
    let url = format!("/api/v1/service-accounts/{}/tokens", sa["id"].as_str().unwrap());

    for i in 0..10 {
        let (s, b) = admin.post(&url, json!({ "name": format!("t{i}") })).await;
        assert_eq!(s, StatusCode::CREATED, "{b}");
    }

    let (s, e) = admin.post(&url, json!({ "name": "t10" })).await;
    assert_eq!(s, StatusCode::CONFLICT, "{e}");
    let (_, list) = admin.get(&url).await;
    assert_eq!(list.as_array().unwrap().len(), 10);
    assert!(!list.to_string().contains("secret_hash"));
}

/// Review of #28 (H1/H2): assignments made while the profile is published or
/// archived never stay linked to the old revision or the archived profile.
#[tokio::test]
async fn concurrent_assign_publish_and_archive_never_leave_stale_links() {
    let h = start().await;
    let admin = admin_mfa(&h).await;
    let ws = h.default_org.clone();
    let (_, p) =
        admin.post("/api/v1/access/profiles", json!({ "name": "Racy", "actions": ["view", "workspace.manage"] })).await;
    let pid = p["id"].as_str().unwrap().to_owned();
    let mut people = Vec::new();

    for i in 0..8 {
        let email = format!("r{i}@example.com");
        h.app.state.identity.create_account(None, Some(&email), "R", PASSWORD).await.unwrap();
        people.push(email);
    }

    let assigns = people.iter().map(|email| {
        admin.post(
            "/api/v1/access/assignments",
            json!({ "login": email, "profile_id": pid, "scope": format!("workspace:{ws}") }),
        )
    });
    let profile_url = format!("/api/v1/access/profiles/{pid}");
    let publish = admin.call(Method::PUT, &profile_url, Some(json!({ "actions": ["view"], "expected_revision": 1 })));
    let (_, results) = tokio::join!(publish, futures::future::join_all(assigns));

    // Whatever order they ran in, nobody keeps workspace.manage.
    for (email, (s, _)) in people.iter().zip(&results) {
        let c = Client::login(&h.base, email, PASSWORD).await;
        let made = c.post("/api/v1/clusters", json!({ "name": format!("c-{email}") })).await.0;
        assert_eq!(made, StatusCode::FORBIDDEN, "{email} (assign answered {s}) kept the old revision");
    }

    // Archive racing more assigns: none of them grants afterwards.
    let more: Vec<String> = (0..6).map(|i| format!("a{i}@example.com")).collect();

    for email in &more {
        h.app.state.identity.create_account(None, Some(email), "A", PASSWORD).await.unwrap();
    }

    let assigns = more.iter().map(|email| {
        admin.post(
            "/api/v1/access/assignments",
            json!({ "login": email, "profile_id": pid, "scope": format!("workspace:{ws}") }),
        )
    });
    let archive = admin.call(Method::DELETE, &profile_url, None);
    let _ = tokio::join!(archive, futures::future::join_all(assigns));

    for email in &more {
        let c = Client::login(&h.base, email, PASSWORD).await;
        assert_eq!(c.get("/api/v1/settings").await.0, StatusCode::FORBIDDEN, "{email} granted by an archived profile");
    }
}

/// Review of #28 (M1): an expired assignment stops granting at decision
/// time, before any sweep runs.
#[tokio::test]
async fn an_expired_assignment_stops_granting_without_the_sweep() {
    use forge_core::permissions::PermissionRepository;
    let h = start().await;
    let admin = admin_mfa(&h).await;
    let ws = &h.default_org;
    let (_, p) = admin
        .post("/api/v1/access/profiles", json!({ "name": "Brief", "actions": ["view", "workspace.manage"] }))
        .await;
    h.app.state.identity.create_account(None, Some("b@example.com"), "B", PASSWORD).await.unwrap();
    let (s, a) = admin
        .post(
            "/api/v1/access/assignments",
            json!({ "login": "b@example.com", "profile_id": p["id"], "scope": format!("workspace:{ws}"), "expires_in_minutes": 1 }),
        )
        .await;
    assert_eq!(s, StatusCode::CREATED, "{a}");
    let b = Client::login(&h.base, "b@example.com", PASSWORD).await;
    assert_eq!(b.post("/api/v1/clusters", json!({ "name": "early" })).await.0, StatusCode::CREATED);

    // Relink it with an expiry in the past, as the engine would see it a
    // minute later; the store row stays, so only the engine check can stop it.
    let mut stale: forge_core::permissions::ProfileAssignment = serde_json::from_value(a.clone()).unwrap();
    stale.expires_at = Some(forge_core::Timestamp(1));
    h.app.state.policy.link_assignment(&stale, 1).unwrap();
    assert!(!h.app.state.store.list_assignments().await.unwrap().is_empty(), "no sweep ran");
    assert_eq!(b.post("/api/v1/clusters", json!({ "name": "late" })).await.0, StatusCode::FORBIDDEN);
}
