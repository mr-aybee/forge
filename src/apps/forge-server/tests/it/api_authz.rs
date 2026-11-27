//! API-level enforcement: authentication, CSRF, Cedar authorization, audit.
#![allow(clippy::unwrap_used)]

use reqwest::{Method, StatusCode};
use serde_json::json;

use crate::common::*;

#[tokio::test]
async fn version_and_capabilities_are_public() {
    let h = start().await;
    let c = Client::anonymous(&h.base);
    let (s, v) = c.get("/api/version").await;
    assert_eq!(s, StatusCode::OK);
    assert_eq!(v["product"], "orbyn-forge");
    assert_eq!(v["api_versions"], json!(["v1"]));
    let (s, caps) = c.get("/api/capabilities").await;
    assert_eq!(s, StatusCode::OK);
    let ids: Vec<&str> = caps["capabilities"].as_array().unwrap().iter().filter_map(|c| c["id"].as_str()).collect();
    assert!(ids.contains(&"jobs.durable") && ids.contains(&"plugins.wasm"));
}

#[tokio::test]
async fn unauthenticated_requests_are_rejected() {
    let h = start().await;
    let c = Client::anonymous(&h.base);
    assert_eq!(c.get("/api/v1/servers").await.0, StatusCode::UNAUTHORIZED);
    assert_eq!(c.post("/api/v1/tenants", json!({"name": "x"})).await.0, StatusCode::UNAUTHORIZED);
    let (s, body) = c.post("/api/v1/auth/login", login_body(&h.base, ADMIN_EMAIL, "wrong password!!").await).await;
    assert_eq!(s, StatusCode::UNAUTHORIZED);
    assert_eq!(body["title"], "invalid_credentials");
}

#[tokio::test]
async fn cookie_sessions_require_csrf_for_mutations() {
    let h = start().await;
    let mut admin = Client::login(&h.base, ADMIN_EMAIL, PASSWORD).await;
    let csrf = admin.csrf.take();
    let (s, _) = admin.post("/api/v1/tenants", json!({"name": "No CSRF"})).await;
    assert_eq!(s, StatusCode::FORBIDDEN, "missing CSRF header must be refused");
    admin.csrf = Some("forged".into());
    assert_eq!(admin.post("/api/v1/tenants", json!({"name": "Bad CSRF"})).await.0, StatusCode::FORBIDDEN);
    admin.csrf = csrf;
    assert_eq!(admin.post("/api/v1/tenants", json!({"name": "Good CSRF"})).await.0, StatusCode::CREATED);

    // Safe methods need no CSRF token.
    admin.csrf = None;
    assert_eq!(admin.get("/api/v1/workspace").await.0, StatusCode::OK);
}

#[tokio::test]
async fn roles_are_enforced_at_the_api() {
    let h = start().await;
    let admin = Client::login(&h.base, ADMIN_EMAIL, PASSWORD).await;
    let dev = user_with_role(&h, &admin, "dev@example.com", "developer", &format!("workspace:{}", h.default_org)).await;

    // Allowed: view the deployment's workspace.
    let (s, ws) = dev.get("/api/v1/workspace").await;
    assert_eq!(s, StatusCode::OK);
    assert_eq!(ws["name"], "Default");

    // Denied: admin actions.
    assert_eq!(dev.post("/api/v1/enrollment-tokens", json!({})).await.0, StatusCode::FORBIDDEN);
    assert_eq!(dev.get("/api/v1/audit").await.0, StatusCode::FORBIDDEN);
    assert_eq!(dev.post("/api/v1/tenants", json!({"name": "t"})).await.0, StatusCode::FORBIDDEN);
    assert_eq!(dev.get("/api/v1/plugins").await.0, StatusCode::FORBIDDEN);

    // Denials are audited.
    let (_, entries) = admin.get("/api/v1/audit?limit=200").await;
    let denied = entries.as_array().unwrap().iter().filter(|e| e["outcome"] == "denied").count();
    assert!(denied >= 4, "expected audited denials, got {denied}");
}

#[tokio::test]
async fn org_admin_cannot_escalate() {
    let h = start().await;
    let admin = Client::login(&h.base, ADMIN_EMAIL, PASSWORD).await;
    let mut oa =
        user_with_role(&h, &admin, "oa@example.com", "workspace_admin", &format!("workspace:{}", h.default_org)).await;
    h.app.state.identity.create_account(None, Some("victim@example.com"), "v", PASSWORD).await.unwrap();

    let (s, _) = oa
        .post(
            "/api/v1/role-bindings",
            json!({"email": "victim@example.com", "role": "platform_admin", "scope": "platform:root"}),
        )
        .await;
    assert_eq!(s, StatusCode::FORBIDDEN);
    let (s, _) = oa
        .post(
            "/api/v1/role-bindings",
            json!({"email": "victim@example.com", "role": "platform_admin", "scope": format!("workspace:{}", h.default_org)}),
        )
        .await;
    assert_eq!(s, StatusCode::BAD_REQUEST);

    // Granting needs an MFA-verified session (#13 inventory) …
    let grant =
        json!({"email": "victim@example.com", "role": "operator", "scope": format!("workspace:{}", h.default_org)});
    let (s, e) = oa.post("/api/v1/role-bindings", grant.clone()).await;
    assert_eq!((s, e["title"].as_str()), (StatusCode::FORBIDDEN, Some("step_up_required")), "{e}");
    let (s, e) =
        admin.post("/api/v1/users", json!({"email": "x@example.com", "display_name": "x", "password": PASSWORD})).await;
    assert_eq!((s, e["title"].as_str()), (StatusCode::FORBIDDEN, Some("step_up_required")), "{e}");
    step_up(&mut oa).await;

    // … then granting inside their own org is fine.
    let (s, _) = oa
        .post(
            "/api/v1/role-bindings",
            json!({"email": "victim@example.com", "role": "operator", "scope": format!("workspace:{}", h.default_org)}),
        )
        .await;
    assert_eq!(s, StatusCode::CREATED);
}

#[tokio::test]
async fn revoking_a_role_takes_effect_immediately() {
    let h = start().await;
    let admin = Client::login(&h.base, ADMIN_EMAIL, PASSWORD).await;
    let auditor = user_with_role(&h, &admin, "aud@example.com", "auditor", "platform:root").await;
    assert_eq!(auditor.get("/api/v1/audit").await.0, StatusCode::OK);
    let (_, bindings) = admin.get("/api/v1/role-bindings").await;
    let me = auditor.get("/api/v1/me").await.1;
    let id = bindings.as_array().unwrap().iter().find(|b| b["user_id"] == me["user"]["id"]).unwrap()["id"]
        .as_str()
        .unwrap()
        .to_owned();
    assert_eq!(
        admin.call(Method::DELETE, &format!("/api/v1/role-bindings/{id}"), None).await.0,
        StatusCode::NO_CONTENT
    );
    assert_eq!(auditor.get("/api/v1/audit").await.0, StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn mutating_actions_are_audited_and_chain_verifies() {
    let h = start().await;
    let admin = Client::login(&h.base, ADMIN_EMAIL, PASSWORD).await;
    admin.post("/api/v1/tenants", json!({"name": "Audited"})).await;
    let (_, entries) = admin.get("/api/v1/audit").await;
    let actions: Vec<&str> = entries.as_array().unwrap().iter().filter_map(|e| e["action"].as_str()).collect();

    for expected in ["platform.init", "auth.login", "tenant.create"] {
        assert!(actions.contains(&expected), "missing {expected} in {actions:?}");
    }

    let (s, report) = admin.get("/api/v1/audit/verify").await;
    assert_eq!(s, StatusCode::OK);
    assert_eq!(report["ok"], true, "{report}");
}

#[tokio::test]
async fn tampered_audit_entry_is_detected_via_api() {
    let h = start().await;
    let admin = Client::login(&h.base, ADMIN_EMAIL, PASSWORD).await;
    admin.post("/api/v1/tenants", json!({"name": "Evidence"})).await;
    sqlx::query("UPDATE audit_log SET actor = 'user:someone-else' WHERE action = 'tenant.create'")
        .execute(h.app.state.store.pool())
        .await
        .unwrap();
    let (_, report) = admin.get("/api/v1/audit/verify").await;
    assert_eq!(report["ok"], false);
    assert!(report["first_invalid"][1].as_str().unwrap().contains("modified"));
}

#[tokio::test]
async fn oauth_pkce_gives_apps_a_bearer_token() {
    let h = start().await;
    let admin = Client::login(&h.base, ADMIN_EMAIL, PASSWORD).await;
    let verifier = "dBjftJeZ4CVP-mB92K27uhbUJU1p1r_wW1gFWFOEjXk";
    let challenge = forge_identity::pkce::s256_challenge(verifier);
    let redirect = "http://127.0.0.1:53682/callback";
    let url = format!(
        "{}/oauth/authorize?response_type=code&client_id=forge-app&redirect_uri={}&code_challenge={challenge}&code_challenge_method=S256&state=xyz",
        h.base,
        url_encode(redirect)
    );
    let resp = admin.http.get(url).send().await.unwrap();
    assert_eq!(resp.status(), StatusCode::SEE_OTHER);
    let location = url::Url::parse(resp.headers()["location"].to_str().unwrap()).unwrap();
    let params: std::collections::HashMap<_, _> = location.query_pairs().into_owned().collect();
    assert_eq!(params["state"], "xyz");

    let app = Client::anonymous(&h.base);
    let tokens: serde_json::Value = app
        .http
        .post(format!("{}/oauth/token", h.base))
        .form(&[
            ("grant_type", "authorization_code"),
            ("code", params["code"].as_str()),
            ("code_verifier", verifier),
            ("client_id", "forge-app"),
            ("redirect_uri", redirect),
        ])
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let mut bearer = Client::anonymous(&h.base);
    bearer.bearer = tokens["access_token"].as_str().map(str::to_owned);
    let (s, me) = bearer.get("/api/v1/me").await;
    assert_eq!(s, StatusCode::OK);
    assert_eq!(me["session_kind"], "bearer");

    // Bearer clients need no CSRF token.
    assert_eq!(bearer.post("/api/v1/tenants", json!({"name": "From App"})).await.0, StatusCode::CREATED);
}

#[tokio::test]
async fn unauthenticated_oauth_authorize_redirects_to_login() {
    let h = start().await;
    let c = Client::anonymous(&h.base);
    let resp = c
        .http
        .get(format!("{}/oauth/authorize?response_type=code&client_id=forge-app&redirect_uri=x&code_challenge=c&code_challenge_method=S256", h.base))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::SEE_OTHER);
    assert!(resp.headers()["location"].to_str().unwrap().starts_with("/login?next="));
}

#[tokio::test]
async fn security_headers_are_set() {
    let h = start().await;
    let resp = reqwest::get(format!("{}/api/version", h.base)).await.unwrap();
    let hd = resp.headers();
    assert_eq!(hd["x-content-type-options"], "nosniff");
    assert_eq!(hd["x-frame-options"], "DENY");
    assert!(hd.contains_key("content-security-policy"));
}

fn url_encode(s: &str) -> String {
    url::form_urlencoded::byte_serialize(s.as_bytes()).collect()
}

#[tokio::test]
async fn clusters_crud_and_scoping() {
    let h = start().await;
    let admin = Client::login(&h.base, ADMIN_EMAIL, PASSWORD).await;
    let ws = &h.default_org;
    let (s, c) = admin.post("/api/v1/clusters", json!({"name": "edge-eu", "description": "EU edge"})).await;
    assert_eq!(s, StatusCode::CREATED, "{c}");
    let id = c["id"].as_str().unwrap().to_owned();
    let (_, list) = admin.get("/api/v1/clusters").await;
    assert_eq!(list[0]["name"], "edge-eu");
    assert_eq!(list[0]["node_count"], 0);

    let viewer = user_with_role(&h, &admin, "v@example.com", "customer", &format!("workspace:{ws}")).await;
    assert_eq!(viewer.get(&format!("/api/v1/clusters/{id}")).await.0, StatusCode::OK);
    assert_eq!(viewer.post("/api/v1/clusters", json!({"name": "x"})).await.0, StatusCode::FORBIDDEN);
    assert_eq!(viewer.call(Method::DELETE, &format!("/api/v1/clusters/{id}"), None).await.0, StatusCode::FORBIDDEN);
    assert_eq!(admin.post("/api/v1/clusters", json!({"name": "bad/name"})).await.0, StatusCode::BAD_REQUEST);
    assert_eq!(admin.call(Method::DELETE, &format!("/api/v1/clusters/{id}"), None).await.0, StatusCode::NO_CONTENT);
}

#[tokio::test]
async fn node_onboarding_requires_an_mfa_session() {
    let h = start().await;
    let admin = Client::login(&h.base, ADMIN_EMAIL, PASSWORD).await; // password-only session
    let (s, _) = admin.post("/api/v1/nodes/probe", json!({"host": "127.0.0.1", "port": 22})).await;
    assert_eq!(s, StatusCode::FORBIDDEN, "probe must require step-up MFA");
    let (s, _) = admin
        .post(
            "/api/v1/nodes/onboard",
            json!({"host": "127.0.0.1", "username": "ops", "host_key_fingerprint": "SHA256:x",
                   "auth": {"type": "password", "password": "secret-value"}}),
        )
        .await;
    assert_eq!(s, StatusCode::FORBIDDEN);

    // Nothing was written to the vault.
    let (_, secrets) = admin.get("/api/v1/secrets").await;
    assert_eq!(secrets, json!([]));
}

#[tokio::test]
async fn installer_is_public_and_contains_no_secrets() {
    let h = start().await;
    let c = Client::anonymous(&h.base);
    let (s, body) = c.get("/install.sh").await;
    assert_eq!(s, StatusCode::OK);
    let script = body.as_str().unwrap();
    assert!(script.starts_with("#!/bin/sh"));
    assert!(script.contains("FORGE_ENROLL_TOKEN:?"), "manual installer must not embed a token");

    // An empty `?hostname=` is no override (it used to become `--hostname ''`).
    let (_, empty) = c.get("/install.sh?hostname=").await;
    assert!(!empty.as_str().unwrap().contains("--hostname"), "an empty hostname adds no flag");
    let (_, named) = c.get("/install.sh?hostname=web-1").await;
    assert!(named.as_str().unwrap().contains("--hostname 'web-1'"));
    assert_eq!(c.get("/agent/download/linux/x86_64/../../etc/passwd").await.0, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn terminal_requires_mfa_and_a_valid_login() {
    let h = start().await;
    let admin = Client::login(&h.base, ADMIN_EMAIL, PASSWORD).await;
    let upgrade = |path: &str| {
        admin
            .http
            .get(format!("{}{path}", h.base))
            .header("Connection", "Upgrade")
            .header("Upgrade", "websocket")
            .header("Sec-WebSocket-Version", "13")
            .header("Sec-WebSocket-Key", "dGhlIHNhbXBsZSBub25jZQ==")
    };
    let id = forge_core::ServerId::new();
    let server = forge_core::model::Server {
        id,
        workspace_id: h.default_org.parse().unwrap(),
        environment_id: None,
        cluster_id: None,
        role: forge_core::model::NodeRole::Worker,
        display_name: None,
        hostname: "web-1".into(),
        os: "linux".into(),
        arch: "x86_64".into(),
        agent_version: "0".into(),
        status: forge_core::model::AgentStatus::Online,
        last_seen_at: None,
        cert_serial: None,
        cert_not_after: None,
        drained: false,
        created_at: forge_core::Timestamp(0),
    };
    forge_core::ports::ServerRepository::create_server(&h.app.state.store, &server).await.unwrap();
    let s = upgrade(&format!("/api/v1/servers/{id}/terminal?login=forgeops")).send().await.unwrap().status();
    assert_eq!(s, StatusCode::FORBIDDEN, "terminal.open is MFA-guarded");
    let (s, e) = admin.get(&format!("/api/v1/servers/{id}/terminal/check?login=forgeops")).await;
    assert_eq!(s, StatusCode::FORBIDDEN, "the preflight runs the same checks: {e}");
    assert_eq!(e["title"], "step_up_required");
    let s = upgrade(&format!("/api/v1/servers/{id}/terminal?login=-oProxyCommand")).send().await.unwrap().status();
    assert_eq!(s, StatusCode::BAD_REQUEST, "option-like logins are rejected before anything else");
    let s = upgrade(&format!("/api/v1/servers/{id}/terminal?login=forgeops"))
        .header("Origin", "https://evil.example")
        .send()
        .await
        .unwrap()
        .status();
    assert_eq!(s, StatusCode::FORBIDDEN, "foreign origins are refused");

    let (s, ca) = admin.get("/api/v1/ssh/ca").await;
    assert_eq!(s, StatusCode::OK);
    assert!(ca["public_key"].as_str().unwrap().starts_with("ssh-ed25519 "));
}

#[tokio::test]
async fn app_store_lists_catalog_and_guards_installs() {
    let h = start().await;
    let admin = Client::login(&h.base, ADMIN_EMAIL, PASSWORD).await;
    let (s, cat) = admin.get("/api/v1/catalog").await;
    assert_eq!(s, StatusCode::OK);
    let ids: Vec<&str> = cat["packages"].as_array().unwrap().iter().filter_map(|p| p["id"].as_str()).collect();

    for id in ["php", "postgresql", "redis", "docker", "rustfs", "nginx", "powerdns", "dotnet"] {
        assert!(ids.contains(&id), "{id} missing from catalog");
    }

    assert!(cat["packages"][0].get("recipe").is_none(), "recipes are never sent to clients");

    let id = forge_core::ServerId::new();
    let server = forge_core::model::Server {
        id,
        workspace_id: h.default_org.parse().unwrap(),
        environment_id: None,
        cluster_id: None,
        role: forge_core::model::NodeRole::Worker,
        display_name: None,
        hostname: "db-1".into(),
        os: "linux".into(),
        arch: "x86_64".into(),
        agent_version: "0".into(),
        status: forge_core::model::AgentStatus::Online,
        last_seen_at: None,
        cert_serial: None,
        cert_not_after: None,
        drained: false,
        created_at: forge_core::Timestamp(0),
    };
    forge_core::ports::ServerRepository::create_server(&h.app.state.store, &server).await.unwrap();
    let req = |params: serde_json::Value| json!({"package": "redis", "version": "distro", "params": params, "server_ids": [id]});
    let (s, _) = admin.post("/api/v1/packages/install", req(json!({"port": "1;id"}))).await;
    assert_eq!(s, StatusCode::BAD_REQUEST, "parameters are validated against the catalog");
    let (s, _) = admin.post("/api/v1/packages/install", req(json!({}))).await;
    assert_eq!(s, StatusCode::FORBIDDEN, "package.manage needs an MFA session");
    let (s, list) = admin.get("/api/v1/installations").await;
    assert_eq!(s, StatusCode::OK);
    assert_eq!(list.as_array().unwrap().len(), 0, "nothing was recorded for refused requests");
}

#[tokio::test]
async fn services_seal_secrets_and_validate_env() {
    let h = start().await;
    let admin = Client::login(&h.base, ADMIN_EMAIL, PASSWORD).await;
    let (_, cl) = admin.post("/api/v1/clusters", json!({"name": "shop"})).await;
    let (_, ag1) = admin.post("/api/v1/projects", json!({"name": "grp1"})).await;
    let (s, ns) = admin
        .post("/api/v1/environments", json!({"name": "UAT", "cluster_id": cl["id"], "project_id": ag1["id"]}))
        .await;
    assert_eq!(s, StatusCode::CREATED, "{ns}");
    let body = |env: serde_json::Value| {
        json!({
            "environment_id": ns["id"], "name": "orders-api", "runtime": "systemd",
            "command": "/srv/forge/apps/uat/orders-api/venv/bin/gunicorn app:app",
            "env_files": ["postgresql"], "env": env
        })
    };
    let (s, r) = admin.post("/api/v1/services", body(json!([{"key": "BAD", "value": "a\nb"}]))).await;
    assert_eq!(s, StatusCode::BAD_REQUEST, "line breaks are rejected: {r}");
    let (s, r) = admin.post("/api/v1/services", body(json!([{"key": "1X", "value": "a"}]))).await;
    assert_eq!(s, StatusCode::BAD_REQUEST, "{r}");
    let (s, app) = admin
        .post(
            "/api/v1/services",
            body(
                json!([{"key": "MODE", "value": "uat"}, {"key": "API_TOKEN", "value": "s3cr3t-value", "secret": true}]),
            ),
        )
        .await;
    assert_eq!(s, StatusCode::CREATED, "{app}");
    assert_eq!(app["spec"]["environment"], "uat", "environment slug");
    let id = app["id"].as_str().unwrap();
    let (_, got) = admin.get(&format!("/api/v1/services/{id}")).await;
    assert!(!got.to_string().contains("s3cr3t-value"), "secret values never leave the server: {got}");
    assert!(got["env"][1]["sealed"].is_null() && got["env"][1]["secret"] == true);
    let raw: String = sqlx::query_scalar("SELECT env FROM services WHERE id = $1")
        .bind(id)
        .fetch_one(h.app.state.store.pool())
        .await
        .unwrap();
    assert!(!raw.contains("s3cr3t-value"), "sealed at rest");

    // Updating without a value keeps the stored secret.
    let mut upd = body(json!([{"key": "MODE", "value": "uat"}, {"key": "API_TOKEN", "value": null, "secret": true}]));
    upd.as_object_mut().unwrap().remove("name");
    let (s, r) = admin
        .http
        .put(format!("{}/api/v1/services/{id}", h.base))
        .header("X-CSRF-Token", admin.csrf.clone().unwrap())
        .json(&upd)
        .send()
        .await
        .map(|r| (r.status(), ()))
        .unwrap();
    assert_eq!(s, StatusCode::OK, "{r:?}");
    let (s, _) = admin.post(&format!("/api/v1/services/{id}/deploy"), json!({})).await;
    assert_eq!(s, StatusCode::CONFLICT, "no nodes chosen yet");
}

/// #13 inventory: keys expire by default, and admins can revoke anyone's.
#[tokio::test]
async fn api_keys_expire_by_default_and_admins_can_revoke_them() {
    let h = start().await;
    let admin = Client::login(&h.base, ADMIN_EMAIL, PASSWORD).await;
    let dev = user_with_role(&h, &admin, "dev@example.com", "developer", &format!("workspace:{}", h.default_org)).await;
    let (_, k) = dev.post("/api/v1/api-keys", json!({"name": "ci"})).await;
    let days = (k["expires_at"].as_i64().unwrap() - k["created_at"].as_i64().unwrap()) / 86_400_000;
    assert_eq!(days, 90, "default expiry: {k}");
    let (_, never) = dev.post("/api/v1/api-keys", json!({"name": "cron", "never_expires": true})).await;
    assert!(never["expires_at"].is_null(), "{never}");
    let dev_id = dev.get("/api/v1/me").await.1["user"]["id"].as_str().unwrap().to_owned();

    // Only platform admins see or revoke another user's keys.
    let other = user_with_role(&h, &admin, "op@example.com", "operator", &format!("workspace:{}", h.default_org)).await;
    assert_eq!(other.get(&format!("/api/v1/users/{dev_id}/api-keys")).await.0, StatusCode::FORBIDDEN);
    assert_eq!(
        other.call(Method::DELETE, &format!("/api/v1/users/{dev_id}/api-keys"), None).await.0,
        StatusCode::FORBIDDEN
    );
    let (s, list) = admin.get(&format!("/api/v1/users/{dev_id}/api-keys")).await;
    assert_eq!(s, StatusCode::OK);
    assert_eq!(list.as_array().unwrap().len(), 2);
    assert!(!list.to_string().contains("secret_hash"), "no hashes in lists");
    let one = k["id"].as_str().unwrap();

    // No MFA needed to revoke.
    let (s, _) = admin.call(Method::DELETE, &format!("/api/v1/users/{dev_id}/api-keys/{one}"), None).await;
    assert_eq!(s, StatusCode::NO_CONTENT);
    let (s, r) = admin.call(Method::DELETE, &format!("/api/v1/users/{dev_id}/api-keys"), None).await;
    assert_eq!((s, r["revoked"].as_u64()), (StatusCode::OK, Some(1)), "{r}");
    let mut ci = Client::anonymous(&h.base);
    ci.bearer = never["secret"].as_str().map(str::to_owned);
    assert_eq!(ci.get("/api/v1/servers").await.0, StatusCode::UNAUTHORIZED, "revoked keys stop working");
}

#[tokio::test]
async fn api_keys_authenticate_without_mfa_powers_and_can_be_revoked() {
    let h = start().await;
    let admin = Client::login(&h.base, ADMIN_EMAIL, PASSWORD).await;
    let (s, created) = admin.post("/api/v1/api-keys", json!({"name": "ci", "expires_days": 30})).await;
    assert_eq!(s, StatusCode::CREATED, "{created}");
    let secret = created["secret"].as_str().unwrap().to_owned();
    assert!(secret.starts_with("fk_"));
    let (_, list) = admin.get("/api/v1/api-keys").await;
    assert!(!list.to_string().contains(&secret), "the secret is shown only once");

    let mut ci = Client::anonymous(&h.base);
    ci.bearer = Some(secret.clone());
    assert_eq!(ci.get("/api/v1/servers").await.0, StatusCode::OK);
    assert_eq!(
        ci.post("/api/v1/tenants", json!({"name": "from-ci"})).await.0,
        StatusCode::CREATED,
        "acts as its owner, no CSRF"
    );
    assert_eq!(
        ci.post("/api/v1/api-keys", json!({"name": "more"})).await.0,
        StatusCode::FORBIDDEN,
        "keys cannot mint keys"
    );
    let (_, entries) = admin.get("/api/v1/audit?limit=50").await;
    assert!(entries.to_string().contains("apikey.create"));

    // Settings: managed by admins, readable.
    let (s, _) = ci.call(reqwest::Method::PUT, "/api/v1/settings/company.name", Some(json!({"value": "Acme"}))).await;
    assert_eq!(s, StatusCode::NO_CONTENT);
    let (_, settings) = ci.get("/api/v1/settings").await;
    assert_eq!(settings["values"][0]["value"], "Acme");

    let id = created["id"].as_str().unwrap();
    let (s, _) = admin.call(reqwest::Method::DELETE, &format!("/api/v1/api-keys/{id}"), None).await;
    assert_eq!(s, StatusCode::NO_CONTENT);
    assert_eq!(ci.get("/api/v1/servers").await.0, StatusCode::UNAUTHORIZED, "revoked keys stop working");
}

#[tokio::test]
async fn sites_validate_preview_and_need_mfa_to_apply() {
    let h = start().await;
    let admin = Client::login(&h.base, ADMIN_EMAIL, PASSWORD).await;
    let edge = forge_core::ServerId::new();
    let server = forge_core::model::Server {
        id: edge,
        workspace_id: h.default_org.parse().unwrap(),
        environment_id: None,
        cluster_id: None,
        role: forge_core::model::NodeRole::Worker,
        display_name: None,
        hostname: "edge-1".into(),
        os: "linux".into(),
        arch: "x86_64".into(),
        agent_version: "0".into(),
        status: forge_core::model::AgentStatus::Online,
        last_seen_at: None,
        cert_serial: None,
        cert_not_after: None,
        drained: false,
        created_at: forge_core::Timestamp(0),
    };
    forge_core::ports::ServerRepository::create_server(&h.app.state.store, &server).await.unwrap();

    // No custom block: saving one needs server.command (MFA), see edge_security.rs.
    let site = json!({"name": "shop", "domains": ["shop.example.com"], "upstreams": ["10.0.0.5:8000"], "edges": [edge],
                      "custom": ""});
    let mut spec = site.clone();
    spec.as_object_mut().unwrap().remove("edges");
    let (s, p) = admin.post("/api/v1/sites/preview", spec).await;
    assert_eq!(s, StatusCode::OK);
    assert!(p["http"].as_str().unwrap().contains("server 10.0.0.5:8000"));
    let mut bad = site.clone();
    bad["custom"] = json!("load_module /tmp/evil.so;");
    assert_eq!(
        admin.post("/api/v1/sites", bad).await.0,
        StatusCode::BAD_REQUEST,
        "code-loading directives are refused"
    );
    let mut bad = site.clone();
    bad["dns"] = json!({"server_id": edge, "zone": "other.org.", "addresses": ["203.0.113.1"]});
    assert_eq!(
        admin.post("/api/v1/sites", bad).await.0,
        StatusCode::BAD_REQUEST,
        "domains must be inside the DNS zone"
    );
    let (s, created) = admin.post("/api/v1/sites", site).await;
    assert_eq!(s, StatusCode::CREATED, "{created}");
    let (s, _) = admin.post(&format!("/api/v1/sites/{}/apply", created["id"].as_str().unwrap()), json!({})).await;
    assert_eq!(s, StatusCode::FORBIDDEN, "proxy.manage needs an MFA session");
}

fn test_node(
    h: &Harness,
    hostname: &str,
    role: forge_core::model::NodeRole,
    status: forge_core::model::AgentStatus,
) -> forge_core::model::Server {
    forge_core::model::Server {
        id: forge_core::ServerId::new(),
        workspace_id: h.default_org.parse().unwrap(),
        environment_id: None,
        cluster_id: None,
        role,
        display_name: None,
        hostname: hostname.into(),
        os: "linux".into(),
        arch: "x86_64".into(),
        agent_version: "0".into(),
        status,
        last_seen_at: None,
        cert_serial: None,
        cert_not_after: None,
        drained: false,
        created_at: forge_core::Timestamp(0),
    }
}

#[tokio::test]
async fn node_metrics_and_connectivity() {
    use forge_core::model::{AgentStatus, NodeRole};
    let h = start().await;
    let admin = Client::login(&h.base, ADMIN_EMAIL, PASSWORD).await;
    let control = test_node(&h, "cp", NodeRole::Control, AgentStatus::Online);
    let worker = test_node(&h, "w1", NodeRole::Worker, AgentStatus::Offline);

    for s in [&control, &worker] {
        forge_core::ports::ServerRepository::create_server(&h.app.state.store, s).await.unwrap();
    }

    let m = forge_proto::messages::NodeMetrics {
        cpu_percent: 12.5,
        mem_total_bytes: 100,
        mem_used_bytes: 40,
        ..Default::default()
    };
    h.app.state.telemetry.record(worker.id, forge_server::metrics::Sample::from_wire(forge_core::Timestamp(1_000), &m));

    let (s, r) = admin.get(&format!("/api/v1/servers/{}/metrics?since=0", worker.id)).await;
    assert_eq!(s, StatusCode::OK, "{r}");
    assert_eq!(r["samples"][0]["cpu"], json!(12.5));
    assert!(h.app.state.telemetry.is_watched(worker.id), "viewing metrics speeds up the agent");
    let (_, r) = admin.get(&format!("/api/v1/servers/{}/metrics?since=1000", worker.id)).await;
    assert_eq!(r["samples"], json!([]), "incremental polling");
    let (_, latest) = admin.get("/api/v1/metrics/latest").await;
    assert_eq!(latest[worker.id.to_string()]["mem_used"], json!(40));

    // The control node is a peer of every node.
    let (s, c) = admin.get(&format!("/api/v1/servers/{}/connectivity", worker.id)).await;
    assert_eq!(s, StatusCode::OK);
    assert_eq!(c["peers"][0]["hostname"], "cp");
    let (s, _) = admin.post(&format!("/api/v1/servers/{}/connectivity", worker.id), json!({"count": 2})).await;
    assert_eq!(s, StatusCode::BAD_REQUEST, "offline agents cannot run checks");

    let anon = reqwest::Client::new();
    let s = anon.get(format!("{}/api/v1/servers/{}/metrics", h.base, worker.id)).send().await.unwrap().status();
    assert_eq!(s, StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn revoking_needs_the_hostname_and_renaming_keeps_it() {
    use forge_core::model::{AgentStatus, NodeRole};
    let h = start().await;
    let admin = Client::login(&h.base, ADMIN_EMAIL, PASSWORD).await;
    let n = test_node(&h, "db-1", NodeRole::Worker, AgentStatus::Online);
    forge_core::ports::ServerRepository::create_server(&h.app.state.store, &n).await.unwrap();

    let (s, r) = admin.put(&format!("/api/v1/servers/{}/name", n.id), json!({"name": "Primary database"})).await;
    assert_eq!(s, StatusCode::OK, "{r}");
    assert_eq!((r["display_name"].as_str(), r["hostname"].as_str()), (Some("Primary database"), Some("db-1")));
    let (s, _) = admin.put(&format!("/api/v1/servers/{}/name", n.id), json!({"name": "x\u{7}"})).await;
    assert_eq!(s, StatusCode::BAD_REQUEST);

    let path = format!("/api/v1/servers/{}/revoke", n.id);
    assert_eq!(admin.post(&path, json!({})).await.0, StatusCode::BAD_REQUEST, "confirmation required");
    assert_eq!(
        admin.post(&path, json!({"confirm_hostname": "Primary database"})).await.0,
        StatusCode::BAD_REQUEST,
        "the machine hostname, not the label"
    );
    assert_eq!(admin.post(&path, json!({"confirm_hostname": "db-1"})).await.0, StatusCode::NO_CONTENT);
    let (_, audit) = admin.get("/api/v1/audit?limit=50").await;
    let actions: Vec<(&str, &str)> = audit
        .as_array()
        .unwrap()
        .iter()
        .map(|e| (e["action"].as_str().unwrap(), e["outcome"].as_str().unwrap()))
        .collect();
    assert!(
        actions.contains(&("server.revoke", "denied"))
            && actions.contains(&("server.revoke", "success"))
            && actions.contains(&("server.rename", "success")),
        "{actions:?}"
    );
}

#[tokio::test]
async fn step_up_turns_a_session_into_an_mfa_session() {
    use forge_identity::totp;
    let h = start().await;
    let mut admin = Client::login(&h.base, ADMIN_EMAIL, PASSWORD).await;
    let (s, r) = admin.post("/api/v1/auth/step-up", json!({"code": "123456"})).await;
    assert_eq!(s, StatusCode::BAD_REQUEST, "no authenticator enrolled yet: {r}");

    let (_, t) = admin.post("/api/v1/auth/totp", json!({})).await;
    let secret = totp::base32_decode(t["secret"].as_str().unwrap()).unwrap();
    let now = || {
        i64::try_from(std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_secs()).unwrap()
    };
    let code = || format!("{:06}", totp::code_at(&secret, now()));
    let (s, _) = admin
        .post("/api/v1/auth/totp/confirm", json!({"password": PASSWORD, "secret": t["secret"], "code": code()}))
        .await;
    assert_eq!(s, StatusCode::NO_CONTENT);

    // A guarded action names the missing step-up instead of a bare denial.
    let (s, e) = admin
        .post("/api/v1/nodes/probe", json!({"host": "10.0.0.1", "port": 22, "username": "x", "password": "y"}))
        .await;
    assert_eq!(s, StatusCode::FORBIDDEN);
    assert_eq!(e["title"], "step_up_required", "{e}");

    let (s, _) = admin.post("/api/v1/auth/step-up", json!({"code": "000000"})).await;
    assert_eq!(s, StatusCode::UNAUTHORIZED);
    let (s, r) = admin.post("/api/v1/auth/step-up", json!({"code": code()})).await;
    assert_eq!(s, StatusCode::OK, "{r}");
    admin.csrf = r["csrf_token"].as_str().map(str::to_owned);
    let (_, me) = admin.get("/api/v1/me").await;
    assert_eq!(me["mfa"], true);
}

/// Security review (#13 inventory): a session or token alone must not add
/// or replace an authenticator, or a stolen non-MFA token could set a
/// secret it knows and step itself up to MFA.
#[tokio::test]
async fn an_authenticator_needs_the_password_first_and_mfa_to_replace() {
    use forge_identity::totp;
    let h = start().await;
    let mut admin = Client::login(&h.base, ADMIN_EMAIL, PASSWORD).await;
    let now = || {
        i64::try_from(std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_secs()).unwrap()
    };
    let (_, t) = admin.post("/api/v1/auth/totp", json!({})).await;
    let secret = totp::base32_decode(t["secret"].as_str().unwrap()).unwrap();
    let code = |s: &[u8]| format!("{:06}", totp::code_at(s, now()));

    // API keys never set up an authenticator.
    let (_, created) = admin.post("/api/v1/api-keys", json!({"name": "ci", "expires_days": 1})).await;
    let mut key = Client::anonymous(&h.base);
    key.bearer = created["secret"].as_str().map(str::to_owned);
    let body = json!({"password": PASSWORD, "secret": t["secret"], "code": code(&secret)});
    assert_eq!(key.post("/api/v1/auth/totp/confirm", body.clone()).await.0, StatusCode::FORBIDDEN);

    // First factor: the current password is required.
    let (s, e) = admin.post("/api/v1/auth/totp/confirm", json!({"secret": t["secret"], "code": code(&secret)})).await;
    assert_eq!(s, StatusCode::UNAUTHORIZED, "{e}");
    assert_eq!(e["title"], "password_required", "{e}");
    let wrong = json!({"password": "not it", "secret": t["secret"], "code": code(&secret)});
    assert_eq!(admin.post("/api/v1/auth/totp/confirm", wrong).await.0, StatusCode::UNAUTHORIZED);
    assert_eq!(admin.post("/api/v1/auth/totp/confirm", body).await.0, StatusCode::NO_CONTENT);

    // Replacing it: the password is not enough, the session must be MFA-verified.
    let (_, t2) = admin.post("/api/v1/auth/totp", json!({})).await;
    let secret2 = totp::base32_decode(t2["secret"].as_str().unwrap()).unwrap();
    let replace = json!({"password": PASSWORD, "secret": t2["secret"], "code": code(&secret2)});
    let (s, e) = admin.post("/api/v1/auth/totp/confirm", replace).await;
    assert_eq!(s, StatusCode::FORBIDDEN, "{e}");
    assert_eq!(e["title"], "step_up_required", "{e}");
    let (s, r) = admin.post("/api/v1/auth/step-up", json!({"code": code(&secret)})).await;
    assert_eq!(s, StatusCode::OK, "{r}");
    admin.csrf = r["csrf_token"].as_str().map(str::to_owned);
    let replace = json!({"secret": t2["secret"], "code": code(&secret2)});
    assert_eq!(admin.post("/api/v1/auth/totp/confirm", replace).await.0, StatusCode::NO_CONTENT);
    let (_, entries) = admin.get("/api/v1/audit?limit=50").await;
    assert!(entries.to_string().contains("auth.totp.enroll"), "{entries}");
}

/// Review of the fix above: with MFA turned off for the server every session
/// counts as MFA-verified for authorization, but changing a factor still
/// needs a session that verified one itself.
#[tokio::test]
async fn with_mfa_off_a_stolen_session_still_cannot_replace_a_factor() {
    use forge_identity::totp;
    let h = start_configured(|c| c.security.require_mfa = false).await;
    let mut admin = Client::login(&h.base, ADMIN_EMAIL, PASSWORD).await;
    let now = || {
        i64::try_from(std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_secs()).unwrap()
    };
    let code = |s: &[u8], at: i64| format!("{:06}", totp::code_at(s, at));
    let (_, t) = admin.post("/api/v1/auth/totp", json!({})).await;
    let secret = totp::base32_decode(t["secret"].as_str().unwrap()).unwrap();
    let first = json!({"password": PASSWORD, "secret": t["secret"], "code": code(&secret, now())});
    assert_eq!(admin.post("/api/v1/auth/totp/confirm", first).await.0, StatusCode::NO_CONTENT);
    let (_, me) = admin.get("/api/v1/me").await;
    assert_eq!(me["mfa"], true, "MFA off: the session counts as verified for authorization");

    // The same (stolen) session cannot swap the authenticator or strip a passkey.
    let (_, t2) = admin.post("/api/v1/auth/totp", json!({})).await;
    let secret2 = totp::base32_decode(t2["secret"].as_str().unwrap()).unwrap();
    let replace = json!({"password": PASSWORD, "secret": t2["secret"], "code": code(&secret2, now())});
    let (s, e) = admin.post("/api/v1/auth/totp/confirm", replace).await;
    assert_eq!(s, StatusCode::FORBIDDEN, "{e}");
    assert_eq!(e["title"], "step_up_required", "{e}");
    let (s, e) = admin.call(Method::DELETE, "/api/v1/me/passkeys/any", None).await;
    assert_eq!(s, StatusCode::FORBIDDEN, "{e}");

    // With proof of the current factor (step-up), it works.
    let (s, r) = admin.post("/api/v1/auth/step-up", json!({"code": code(&secret, now())})).await;
    assert_eq!(s, StatusCode::OK, "{r}");
    admin.csrf = r["csrf_token"].as_str().map(str::to_owned);
    let replace = json!({"secret": t2["secret"], "code": code(&secret2, now())});
    assert_eq!(admin.post("/api/v1/auth/totp/confirm", replace).await.0, StatusCode::NO_CONTENT);
}

#[tokio::test]
async fn cluster_profiles_validate_scope_and_feed_ingress_endpoints() {
    use forge_core::model::{AgentStatus, NodeRole};
    let h = start().await;
    let admin = Client::login(&h.base, ADMIN_EMAIL, PASSWORD).await;
    let ws = &h.default_org;
    let (_, c) = admin.post("/api/v1/clusters", json!({"name": "lab"})).await;
    let cid = c["id"].as_str().unwrap().to_owned();
    let mut a = test_node(&h, "edge-a", NodeRole::Worker, AgentStatus::Online);
    let mut b = test_node(&h, "edge-b", NodeRole::Worker, AgentStatus::Online);
    let outsider = test_node(&h, "other", NodeRole::Worker, AgentStatus::Online);
    a.cluster_id = Some(cid.parse().unwrap());
    b.cluster_id = Some(cid.parse().unwrap());

    for s in [&a, &b, &outsider] {
        forge_core::ports::ServerRepository::create_server(&h.app.state.store, s).await.unwrap();
    }

    let profile = |edges: serde_json::Value, vips: serde_json::Value| {
        json!({
            "orchestrator": "swarm",
            "network": {"name": "forge-lab", "subnet": "10.42.1.0/24", "encrypted": true},
            "ingress": {"mode": "active_standby", "edges": edges, "failover": "vrrp",
                        "vrrp": {"vips": vips, "interface": "eth0", "router_id": 51}}
        })
    };
    let url = format!("/api/v1/clusters/{cid}/profile");
    let good = profile(
        json!([{"server_id": a.id, "role": "primary"}, {"server_id": b.id, "role": "backup"}]),
        json!(["10.0.0.100"]),
    );

    // Validation: two VIPs for active-standby, and a node outside the cluster.
    let two = profile(
        json!([{"server_id": a.id, "role": "primary"}, {"server_id": b.id, "role": "backup"}]),
        json!(["10.0.0.100", "10.0.0.101"]),
    );
    assert_eq!(admin.call(Method::PUT, &url, Some(two)).await.0, StatusCode::BAD_REQUEST);
    let foreign = profile(
        json!([{"server_id": a.id, "role": "primary"}, {"server_id": outsider.id, "role": "backup"}]),
        json!(["10.0.0.100"]),
    );
    assert_eq!(admin.call(Method::PUT, &url, Some(foreign)).await.0, StatusCode::BAD_REQUEST);

    let viewer = user_with_role(&h, &admin, "v@example.com", "customer", &format!("workspace:{ws}")).await;
    assert_eq!(viewer.call(Method::PUT, &url, Some(good.clone())).await.0, StatusCode::FORBIDDEN);
    let (s, body) = admin.call(Method::PUT, &url, Some(good)).await;
    assert_eq!(s, StatusCode::OK, "{body}");

    let (_, got) = admin.get(&url).await;
    assert_eq!(got["profile"]["ingress"]["mode"], "active_standby");
    assert!(got["status"].get("vrrp_auth").is_none(), "VRRP auth stays on the server");
    assert_eq!(got["members"].as_array().unwrap().len(), 2);

    // Applying touches edges: proxy.manage needs an MFA session.
    assert_eq!(admin.post(&format!("{url}/apply"), json!({})).await.0, StatusCode::FORBIDDEN);

    // A Forge service placed on edge-b (active) and edge-a (standby).
    let store = &h.app.state.store;
    let cluster_id: forge_core::ClusterId = cid.parse().unwrap();
    let wsid: forge_core::WorkspaceId = ws.parse().unwrap();
    let now = forge_core::Timestamp(1);
    let ns = forge_core::model::Environment {
        id: forge_core::EnvironmentId::new(),
        workspace_id: wsid,
        cluster_id: Some(cluster_id),
        project_id: None,
        name: "payments".into(),
        protected: false,
        created_at: now,
    };
    forge_core::ports::TenancyRepository::create_environment(store, &ns).await.unwrap();
    let app = forge_core::model::Service {
        id: forge_core::ServiceId::new(),
        workspace_id: wsid,
        environment_id: ns.id,
        name: "api".into(),
        runtime: "service".into(),
        spec: json!({}),
        env: vec![],
        created_by: forge_core::Principal::User(forge_core::UserId::new()),
        created_at: now,
        updated_at: now,
        version: 0,
    };
    forge_core::ports::ServiceRepository::put_service(store, &app).await.unwrap();

    for (node, role) in [(&b, "active"), (&a, "standby")] {
        forge_core::ports::ServiceRepository::put_instance(
            store,
            &forge_core::model::ServiceInstance {
                service_id: app.id,
                server_id: node.id,
                role: role.into(),
                status: "deployed".into(),
                state: Some("active".into()),
                last_job_id: None,
                last_error: None,
                updated_at: now,
                desired: forge_core::model::InstanceDesired::Running,
                deployed_release: None,
                exit_code: None,
            },
        )
        .await
        .unwrap();
    }

    // An endpoint on the cluster takes its edges from the profile and resolves
    // typed targets; a service expands to its placements.
    let (s, site) = admin
        .post(
            "/api/v1/sites",
            json!({"name": "shop", "domains": ["shop.example.com"], "upstreams": [], "cluster_id": cid,
                   "targets": [{"kind": "node", "server_id": a.id, "port": 8080}, {"kind": "app", "service_id": app.id, "port": 9000}]}),
        )
        .await;
    assert_eq!(s, StatusCode::CREATED, "{site}");
    assert_eq!(site["config"]["edges"].as_array().unwrap().len(), 2);
    assert_eq!(site["config"]["spec"]["upstreams"], json!(["edge-a:8080", "edge-b:9000"]));
    assert_eq!(site["config"]["spec"]["standby"], json!(["edge-a:9000"]));

    // With a cluster network, a node is reached on its mesh address only when
    // every edge is up in the same fabric (#18); else on its own address.
    let spec = json!({"spec": {"subnet": "10.64.0.0/16", "mode": "mesh", "domain": "lab.forge.internal"}});
    assert_eq!(admin.call(Method::PUT, &format!("/api/v1/clusters/{cid}/fabric"), Some(spec)).await.0, StatusCode::OK);
    let mut rec = forge_core::ports::FabricRepository::get_fabric(store, cluster_id).await.unwrap().unwrap();
    rec.status = json!({ "nodes": { b.id.to_string(): { "state": "ok" } } });
    forge_core::ports::FabricRepository::put_fabric(store, &rec).await.unwrap();
    let (_, routing) = admin.get(&format!("/api/v1/clusters/{cid}/routing")).await;
    let ep = &routing["endpoints"][0];
    assert_eq!(ep["domains"], json!(["shop.example.com"]), "{routing}");
    let svc = ep["targets"].as_array().unwrap().iter().find(|t| t["target"]["kind"] == "app").unwrap();
    assert_eq!(svc["internal_name"], "api.payments.lab.forge.internal");
    let ups = svc["upstreams"].as_array().unwrap();
    assert_eq!(ups.len(), 2);
    let addr_of = |id: &str| ups.iter().find(|u| u["server_id"] == id).unwrap()["address"].as_str().unwrap().to_owned();

    // edge-a's mesh is not up: it cannot reach edge-b's mesh address.
    assert_eq!(addr_of(&b.id.to_string()), "edge-b:9000", "{routing}");
    assert_eq!(addr_of(&a.id.to_string()), "edge-a:9000", "mesh not up on edge-a: its own address");

    // Both edges up in the fabric: both nodes on their mesh addresses.
    rec.status = json!({ "nodes": { a.id.to_string(): { "state": "ok" }, b.id.to_string(): { "state": "ok" } } });
    forge_core::ports::FabricRepository::put_fabric(store, &rec).await.unwrap();
    let (_, routing) = admin.get(&format!("/api/v1/clusters/{cid}/routing")).await;
    let svc = routing["endpoints"][0]["targets"]
        .as_array()
        .unwrap()
        .iter()
        .find(|t| t["target"]["kind"] == "app")
        .unwrap()
        .clone();

    for u in svc["upstreams"].as_array().unwrap() {
        assert!(u["address"].as_str().unwrap().starts_with("10.64."), "{routing}");
    }

    // Issue #26: preview takes the endpoint body and shows the upstreams each
    // target resolves to, with the same address rule; nothing is stored.
    let body = json!({"name": "shop-preview", "domains": ["preview.example.com"], "upstreams": [], "cluster_id": cid,
                      "edges": [], "targets": [{"kind": "app", "service_id": app.id, "port": 9000}]});
    let (s, p) = admin.post("/api/v1/sites/preview", body.clone()).await;
    assert_eq!(s, StatusCode::OK, "{p}");
    assert_eq!(p["edges"].as_array().unwrap().len(), 2, "edges from the profile: {p}");
    let ups = p["targets"][0]["upstreams"].as_array().unwrap();
    assert_eq!(ups.len(), 2, "{p}");
    assert!(ups.iter().all(|u| u["address"].as_str().unwrap().starts_with("10.64.")), "{p}");
    assert!(p["http"].as_str().unwrap().contains("10.64."), "{p}");
    let sites = forge_core::ports::SiteRepository::list_sites(store).await.unwrap();
    assert!(sites.iter().all(|s| s.name != "shop-preview"), "preview stores nothing");
    let mut foreign = body.clone();
    foreign["targets"] = json!([{"kind": "app", "service_id": forge_core::ServiceId::new(), "port": 9000}]);
    assert_eq!(admin.post("/api/v1/sites/preview", foreign).await.0, StatusCode::BAD_REQUEST, "unknown service");

    let (_, audit) = admin.get("/api/v1/audit?limit=50").await;
    assert!(audit.to_string().contains("cluster.profile.update"));
}

#[tokio::test]
async fn fabric_suggests_allocates_pins_and_rejects_overlap() {
    use forge_core::model::{AgentStatus, NodeRole};
    let h = start().await;
    let admin = Client::login(&h.base, ADMIN_EMAIL, PASSWORD).await;
    let ws = &h.default_org;
    let (_, c) = admin.post("/api/v1/clusters", json!({"name": "lab"})).await;
    let cid = c["id"].as_str().unwrap().to_owned();
    let (_, c2) = admin.post("/api/v1/clusters", json!({"name": "uat"})).await;
    let cid2 = c2["id"].as_str().unwrap().to_owned();
    let mut a = test_node(&h, "n-a", NodeRole::Worker, AgentStatus::Online);
    a.cluster_id = Some(cid.parse().unwrap());
    forge_core::ports::ServerRepository::create_server(&h.app.state.store, &a).await.unwrap();

    let (s, sug) = admin.post(&format!("/api/v1/clusters/{cid}/fabric/suggest"), json!({"prefix": 16})).await;
    assert_eq!(s, StatusCode::OK, "{sug}");
    assert_eq!(sug["subnet"], "10.64.0.0/16");
    assert_eq!(sug["domain"], "lab.local");

    let url = format!("/api/v1/clusters/{cid}/fabric");
    let spec = json!({"subnet": "10.64.0.0/16", "mode": "mesh", "domain": "lab.forge.internal"});
    let viewer = user_with_role(&h, &admin, "v@example.com", "customer", &format!("workspace:{ws}")).await;
    assert_eq!(viewer.call(Method::PUT, &url, Some(json!({"spec": spec}))).await.0, StatusCode::FORBIDDEN);

    // Routed mode is retired: refused on save; a stored one shows as mesh.
    let routed = json!({"spec": {"subnet": "10.64.0.0/16", "mode": "routed", "domain": "lab.forge.internal"}});
    assert_eq!(admin.call(Method::PUT, &url, Some(routed)).await.0, StatusCode::BAD_REQUEST);
    let (s, body) = admin.call(Method::PUT, &url, Some(json!({"spec": spec}))).await;
    assert_eq!(s, StatusCode::OK, "{body}");
    let store = &h.app.state.store;
    let mut rec = forge_core::ports::FabricRepository::get_fabric(store, cid.parse().unwrap()).await.unwrap().unwrap();
    rec.spec["mode"] = json!("routed");
    forge_core::ports::FabricRepository::put_fabric(store, &rec).await.unwrap();
    assert_eq!(admin.get(&url).await.1["spec"]["mode"], "mesh");

    let (_, got) = admin.get(&url).await;
    let leases = got["leases"].as_array().unwrap();
    assert!(leases.iter().any(|l| l["kind"] == "node" && l["cidr"] == "10.64.0.16/32"), "{got}");
    assert!(leases.iter().any(|l| l["kind"] == "block" && l["cidr"] == "10.64.10.0/24"), "{got}");

    // A pin moves the node and is kept; one outside its range is refused.
    let pin = json!({"spec": spec, "pins": [{"kind": "node", "owner": a.id, "cidr": "10.64.0.200/32"}]});
    assert_eq!(admin.call(Method::PUT, &url, Some(pin)).await.0, StatusCode::OK);
    let (_, got) = admin.get(&url).await;
    assert!(got["leases"].as_array().unwrap().iter().any(|l| l["cidr"] == "10.64.0.200/32" && l["pinned"] == true));
    let bad = json!({"spec": spec, "pins": [{"kind": "node", "owner": a.id, "cidr": "10.64.1.5/32"}]});
    assert_eq!(admin.call(Method::PUT, &url, Some(bad)).await.0, StatusCode::BAD_REQUEST);

    // Another cluster cannot take an overlapping subnet; the next suggestion skips it.
    let url2 = format!("/api/v1/clusters/{cid2}/fabric");
    let clash = json!({"spec": {"subnet": "10.64.5.0/24", "mode": "mesh", "domain": "uat.forge.internal"}});
    assert_eq!(admin.call(Method::PUT, &url2, Some(clash)).await.0, StatusCode::CONFLICT);
    let (_, sug) = admin.post("/api/v1/fabric/suggest", json!({"prefix": 16, "name": "uat"})).await;
    assert_eq!(sug["subnet"], "10.65.0.0/16");

    // Applying changes host networking on the members: server.command needs MFA.
    assert_eq!(admin.post(&format!("{url}/apply"), json!({})).await.0, StatusCode::FORBIDDEN);
    assert_eq!(viewer.post(&format!("{url}/apply"), json!({})).await.0, StatusCode::FORBIDDEN);

    // A plan that was never applied can be deleted without touching nodes.
    let (s, body) = admin.call(Method::DELETE, &url, None).await;
    assert_eq!(s, StatusCode::OK, "{body}");
    assert_eq!(admin.get(&url).await.1["spec"], serde_json::Value::Null);
}

/// Issue #24: moving a node between clusters makes every member of the old
/// and the new fabric due at once, so their next desired config includes (or
/// drops) the node without waiting for the periodic recheck.
#[tokio::test]
async fn moving_a_node_resyncs_every_member_of_both_fabrics() {
    use forge_core::model::{AgentStatus, NodeRole};
    use forge_core::ports::{FabricRepository, ServerRepository};
    let h = start().await;
    let admin = Client::login(&h.base, ADMIN_EMAIL, PASSWORD).await;
    let store = &h.app.state.store;
    let mut ids = Vec::new();

    for (name, subnet) in [("lab", "10.64.0.0/16"), ("uat", "10.65.0.0/16")] {
        let (_, c) = admin.post("/api/v1/clusters", json!({"name": name})).await;
        let cid: forge_core::ClusterId = c["id"].as_str().unwrap().parse().unwrap();
        let spec = json!({"spec": {"subnet": subnet, "mode": "mesh", "domain": format!("{name}.forge.internal")}});
        let (s, b) = admin.call(Method::PUT, &format!("/api/v1/clusters/{cid}/fabric"), Some(spec)).await;
        assert_eq!(s, StatusCode::OK, "{b}");
        ids.push(cid);
    }

    let (lab, uat) = (ids[0], ids[1]);

    // Offline members: the background pass leaves their schedules alone.
    let mut nodes = Vec::new();

    for (name, c) in [("a", lab), ("b", lab), ("c", uat), ("d", uat)] {
        let mut n = test_node(&h, name, NodeRole::Worker, AgentStatus::Offline);
        n.cluster_id = Some(c);
        store.create_server(&n).await.unwrap();
        nodes.push(n);
    }

    for c in [lab, uat] {
        let mut rec = store.get_fabric(c).await.unwrap().unwrap();
        let mut st = serde_json::Map::new();

        for n in nodes.iter().filter(|n| n.cluster_id == Some(c)) {
            st.insert(n.id.to_string(), json!({"state": "ok", "next_at": i64::MAX / 2, "public_key": "K"}));
        }

        rec.status = json!({ "nodes": st });
        rec.leases = json!([]);
        store.put_fabric(&rec).await.unwrap();
    }

    let moved = &nodes[2];
    let (s, b) =
        admin.call(Method::PATCH, &format!("/api/v1/servers/{}", moved.id), Some(json!({"cluster_id": lab}))).await;
    assert_eq!(s, StatusCode::OK, "{b}");

    for (c, members) in [(lab, vec![&nodes[0], &nodes[1]]), (uat, vec![&nodes[3]])] {
        let rec = store.get_fabric(c).await.unwrap().unwrap();

        for n in members {
            assert_eq!(
                rec.status["nodes"][n.id.to_string()]["next_at"],
                0,
                "{} due now in {c}: {}",
                n.hostname,
                rec.status
            );
        }
    }

    let leases = store.get_fabric(lab).await.unwrap().unwrap().leases.to_string();
    assert!(leases.contains(&moved.id.to_string()), "the new member has an address in lab: {leases}");
    let leases = store.get_fabric(uat).await.unwrap().unwrap().leases.to_string();
    assert!(!leases.contains(&moved.id.to_string()), "and none left in uat: {leases}");
}

#[tokio::test]
async fn vault_is_platform_admin_only_and_reveal_needs_step_up() {
    let h = start().await;
    let mut admin = Client::login(&h.base, ADMIN_EMAIL, PASSWORD).await;
    let ws = &h.default_org;
    let (_, c) = admin.post("/api/v1/clusters", json!({"name": "lab"})).await;
    let cid = c["id"].as_str().unwrap().to_owned();

    let item = json!({"name": "db root", "username": "root", "password": "s3cr3t-P@ss", "url": "ssh://db1",
                      "tags": ["Prod", "db"], "links": [{"kind": "cluster", "id": cid}]});

    // Writing to the vault goes through Cedar (`secret.manage`): MFA (#13 inventory).
    let other_admin = user_with_role(&h, &admin, "pa2@example.com", "platform_admin", "platform:root").await;
    let (s, e) = other_admin.post("/api/v1/vault/items", item.clone()).await;
    assert_eq!((s, e["title"].as_str()), (StatusCode::FORBIDDEN, Some("step_up_required")), "{e}");
    step_up(&mut admin).await;
    let (s, created) = admin.post("/api/v1/vault/items", item).await;
    assert_eq!(s, StatusCode::CREATED, "{created}");
    let id = created["id"].as_str().unwrap().to_owned();
    assert_eq!(created["tags"], json!(["prod", "db"]));
    assert_eq!(created["links"][0]["name"], "lab");
    assert!(created.get("password").is_none() && created.get("sealed").is_none());
    let (_, list) = admin.get("/api/v1/vault/items").await;
    assert!(!list.to_string().contains("s3cr3t-P@ss"), "list never carries the password");

    // Bad input.
    let bad = json!({"name": "x", "password": "p", "url": "javascript:alert(1)"});
    assert_eq!(admin.post("/api/v1/vault/items", bad).await.0, StatusCode::BAD_REQUEST);
    let bad = json!({"name": "x", "password": "p", "links": [{"kind": "node", "id": "nope"}]});
    assert_eq!(admin.post("/api/v1/vault/items", bad).await.0, StatusCode::BAD_REQUEST);

    // A workspace admin is not a platform admin: no access at all.
    let wsa = user_with_role(&h, &admin, "wsa@example.com", "workspace_admin", &format!("workspace:{ws}")).await;
    assert_eq!(wsa.get("/api/v1/vault/items").await.0, StatusCode::FORBIDDEN);
    assert_eq!(wsa.post(&format!("/api/v1/vault/items/{id}/reveal"), json!({})).await.0, StatusCode::FORBIDDEN);

    // A platform admin without step-up MFA cannot reveal, nor change or delete.
    let reveal = format!("/api/v1/vault/items/{id}/reveal");
    let (s, e) = other_admin.post(&reveal, json!({})).await;
    assert_eq!(s, StatusCode::FORBIDDEN);
    assert_eq!(e["title"], "step_up_required", "{e}");
    let item_url = format!("/api/v1/vault/items/{id}");
    assert_eq!(other_admin.call(Method::DELETE, &item_url, None).await.0, StatusCode::FORBIDDEN);
    assert_eq!(other_admin.get("/api/v1/vault/items").await.0, StatusCode::OK, "listing needs no MFA");

    let (s, r) = admin.post(&reveal, json!({})).await;
    assert_eq!(s, StatusCode::OK, "{r}");
    assert_eq!(r["password"], "s3cr3t-P@ss");

    // Rotate, then reveal the new one; editing without a password keeps it.
    let upd = json!({"name": "db root", "username": "root", "password": "n3w-P@ss!", "tags": ["db"]});
    assert_eq!(admin.call(Method::PUT, &format!("/api/v1/vault/items/{id}"), Some(upd)).await.0, StatusCode::OK);
    let keep = json!({"name": "db root (renamed)", "username": "root", "tags": ["db"]});
    assert_eq!(admin.call(Method::PUT, &format!("/api/v1/vault/items/{id}"), Some(keep)).await.0, StatusCode::OK);
    assert_eq!(admin.post(&reveal, json!({})).await.1["password"], "n3w-P@ss!");

    // Every reveal is audited, the value never is.
    let (_, audit) = admin.get("/api/v1/audit?limit=200").await;
    let text = audit.to_string();
    assert!(text.contains("vault.reveal"));

    // Secrets with `-`, `@` and `!` cannot occur by chance in the base64
    // signatures and hex hashes the audit JSON carries.
    assert!(!text.contains("n3w-P@ss") && !text.contains("s3cr3t-P@ss"), "no secret in the audit log");

    assert_eq!(admin.call(Method::DELETE, &format!("/api/v1/vault/items/{id}"), None).await.0, StatusCode::NO_CONTENT);
}

#[tokio::test]
async fn uploaded_certificates_are_validated_sealed_and_never_returned() {
    let h = start().await;
    let admin = Client::login(&h.base, ADMIN_EMAIL, PASSWORD).await;
    let ws = &h.default_org;
    let pair = rcgen::generate_simple_self_signed(vec!["shop.example.com".to_owned()]).unwrap();
    let (crt, key) = (pair.cert.pem(), pair.key_pair.serialize_pem());
    let other = rcgen::generate_simple_self_signed(vec!["x.example.com".to_owned()]).unwrap();

    // Key from another certificate: refused.
    let (s, body) = admin
        .post(
            "/api/v1/certificates",
            json!({"name": "bad", "chain_pem": crt, "key_pem": other.key_pair.serialize_pem()}),
        )
        .await;
    assert_eq!(s, StatusCode::BAD_REQUEST, "{body}");

    let viewer = user_with_role(&h, &admin, "v@example.com", "customer", &format!("workspace:{ws}")).await;
    let upload = json!({"name": "shop", "chain_pem": crt, "key_pem": key});
    assert_eq!(viewer.post("/api/v1/certificates", upload.clone()).await.0, StatusCode::FORBIDDEN);
    let (s, cert) = admin.post("/api/v1/certificates", upload).await;
    assert_eq!(s, StatusCode::CREATED, "{cert}");
    assert_eq!(cert["domains"], json!(["shop.example.com"]));
    assert!(cert["days_left"].as_i64().unwrap() > 0);
    let id = cert["id"].as_str().unwrap().to_owned();

    let (_, list) = admin.get("/api/v1/certificates").await;
    assert!(!list.to_string().contains("PRIVATE KEY"), "the key never leaves the server");

    // Endpoints must be covered by the certificate.
    let site = |domain: &str| json!({"name": "shop", "domains": [domain], "upstreams": ["10.0.0.5:80"], "edges": [], "tls": "uploaded", "cert_id": id});
    let (s, body) = admin.post("/api/v1/sites", site("other.example.com")).await;
    assert_eq!(s, StatusCode::BAD_REQUEST, "{body}");
    let mut node = test_node(&h, "edge", forge_core::model::NodeRole::Worker, forge_core::model::AgentStatus::Online);
    node.cluster_id = None;
    forge_core::ports::ServerRepository::create_server(&h.app.state.store, &node).await.unwrap();
    let mut ok = site("shop.example.com");
    ok["edges"] = json!([node.id]);
    let (s, created) = admin.post("/api/v1/sites", ok).await;
    assert_eq!(s, StatusCode::CREATED, "{created}");
    assert!(!created.to_string().contains("PRIVATE KEY"));

    // In use: cannot be deleted.
    let url = format!("/api/v1/certificates/{id}");
    assert_eq!(admin.call(Method::DELETE, &url, None).await.0, StatusCode::CONFLICT);
    let (_, audit) = admin.get("/api/v1/audit?limit=50").await;
    assert!(audit.to_string().contains("certificate.upload"));
    assert!(!audit.to_string().contains("PRIVATE KEY"));
}
