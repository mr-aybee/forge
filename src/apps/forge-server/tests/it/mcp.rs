//! MCP connectors end to end (GitHub issue #6): OAuth discovery, dynamic
//! client registration, consent in the console, audience-bound tokens,
//! `/mcp` tools that run the REST handlers, server-side modes, revocation.
#![allow(clippy::unwrap_used)]

use crate::common::*;
use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use forge_server::App;
use forge_server::state::Services;
use reqwest::StatusCode;
use serde_json::{Value, json};
use tokio::net::TcpListener;

async fn start_mcp(max_mode: &str) -> Harness {
    start_mcp_pinned(max_mode, true).await
}

async fn start_mcp_pinned(max_mode: &str, pinned: bool) -> Harness {
    let dir = tempfile::tempdir().unwrap();
    let mut config = test_config(dir.path());
    let api = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let agent = TcpListener::bind("127.0.0.1:0").await.unwrap();
    config.public_url = pinned.then(|| format!("http://{}", api.local_addr().unwrap()));
    config.mcp.enabled = true;
    config.mcp.max_mode = forge_core::model::AgentMode::parse(max_mode).unwrap();
    // The development toggle must never give an agent MFA.
    config.dev_skip_mfa = false;
    let state = Services::build(config).await.unwrap();
    let init =
        forge_server::bootstrap::init_admin(&state, None, Some(ADMIN_EMAIL), "Admin", PASSWORD, false).await.unwrap();
    let app = App::start_with(state, api, agent).unwrap();
    Harness {
        base: format!("http://{}", app.api_addr),
        agent_url: format!("https://127.0.0.1:{}", app.agent_addr.port()),
        default_org: init.workspace_id.to_string(),
        app,
        dir,
    }
}

const REDIRECT: &str = "http://127.0.0.1:47123/callback";
const VERIFIER: &str = "an-agent-verifier-with-enough-entropy-0123456789";

fn challenge() -> String {
    URL_SAFE_NO_PAD.encode(ring::digest::digest(&ring::digest::SHA256, VERIFIER.as_bytes()).as_ref())
}

async fn register(h: &Harness, uris: Value) -> (StatusCode, Value) {
    Client::anonymous(&h.base)
        .post("/oauth/register", json!({ "client_name": "Test agent", "redirect_uris": uris }))
        .await
}

/// Runs authorize → consent → token for `client_id`; returns the token response.
async fn connect(h: &Harness, web: &Client, client_id: &str, mode: &str) -> (Value, String) {
    let res = format!("{}/mcp", h.base);
    let resp = web
        .http
        .get(format!("{}/oauth/authorize", h.base))
        .query(&[
            ("response_type", "code"),
            ("client_id", client_id),
            ("redirect_uri", REDIRECT),
            ("code_challenge", challenge().as_str()),
            ("code_challenge_method", "S256"),
            ("state", "st-1"),
            ("resource", res.as_str()),
        ])
        .send()
        .await
        .unwrap();
    assert!(resp.status().is_redirection(), "{}", resp.status());
    let loc = resp.headers()["location"].to_str().unwrap().to_owned();
    assert!(loc.starts_with("/connect?request="), "consent page, not a code: {loc}");
    let id = loc.trim_start_matches("/connect?request=").to_owned();

    let (s, page) = web.get(&format!("/api/v1/oauth/consent/{id}")).await;
    assert_eq!(s, StatusCode::OK, "{page}");
    assert_eq!(page["client"]["name"], "Test agent");
    assert_eq!(page["redirect_host"], "127.0.0.1:47123");
    assert_eq!(page["redirect_uri"], REDIRECT);

    let (s, d) = web.post(&format!("/api/v1/oauth/consent/{id}"), json!({ "approve": true, "mode": mode })).await;
    assert_eq!(s, StatusCode::OK, "{d}");
    let target = url::Url::parse(d["redirect"].as_str().unwrap()).unwrap();
    assert!(target.as_str().starts_with(REDIRECT));
    let get = |k: &str| target.query_pairs().find(|(n, _)| n == k).map(|(_, v)| v.into_owned());
    assert_eq!(get("state").as_deref(), Some("st-1"));
    assert_eq!(get("iss").as_deref(), Some(h.base.as_str()));
    let code = get("code").unwrap();
    let tokens = token(
        h,
        &[
            ("grant_type", "authorization_code"),
            ("code", code.as_str()),
            ("code_verifier", VERIFIER),
            ("client_id", client_id),
            ("redirect_uri", REDIRECT),
            ("resource", res.as_str()),
        ],
    )
    .await;
    assert_eq!(tokens.0, StatusCode::OK, "{}", tokens.1);
    (tokens.1, id)
}

async fn token(h: &Harness, form: &[(&str, &str)]) -> (StatusCode, Value) {
    let r = reqwest::Client::new().post(format!("{}/oauth/token", h.base)).form(form).send().await.unwrap();
    let s = r.status();
    (s, r.json().await.unwrap_or(Value::Null))
}

async fn rpc(h: &Harness, bearer: Option<&str>, body: Value) -> reqwest::Response {
    let mut r = reqwest::Client::new().post(format!("{}/mcp", h.base)).json(&body);
    if let Some(b) = bearer {
        r = r.bearer_auth(b);
    }
    r.send().await.unwrap()
}

async fn rpc_ok(h: &Harness, bearer: &str, method: &str, params: Value) -> Value {
    let r = rpc(h, Some(bearer), json!({ "jsonrpc": "2.0", "id": 1, "method": method, "params": params })).await;
    assert_eq!(r.status(), StatusCode::OK);
    r.json().await.unwrap()
}

#[tokio::test]
async fn discovery_registration_and_consent_bind_tokens_to_mcp() {
    let h = start_mcp("auto").await;
    let web = Client::login(&h.base, ADMIN_EMAIL, PASSWORD).await;
    let anon = Client::anonymous(&h.base);

    // Discovery (RFC 8414, RFC 9728) and the 401 that points to it.
    let (s, asm) = anon.get("/.well-known/oauth-authorization-server").await;
    assert_eq!(s, StatusCode::OK);
    assert_eq!(asm["registration_endpoint"], format!("{}/oauth/register", h.base));
    assert_eq!(asm["code_challenge_methods_supported"], json!(["S256"]));
    let (s, prm) = anon.get("/.well-known/oauth-protected-resource/mcp").await;
    assert_eq!(s, StatusCode::OK);
    assert_eq!(prm["resource"], format!("{}/mcp", h.base));
    let r = rpc(&h, None, json!({ "jsonrpc": "2.0", "id": 1, "method": "initialize" })).await;
    assert_eq!(r.status(), StatusCode::UNAUTHORIZED);
    let wa = r.headers()["www-authenticate"].to_str().unwrap().to_owned();
    assert!(wa.contains("resource_metadata=") && wa.contains("/.well-known/oauth-protected-resource/mcp"), "{wa}");

    // Registration (RFC 7591): strict redirect URIs.
    for bad in [
        json!(["https://*.example.com/cb"]),
        json!(["https://example.com/cb#x"]),
        json!(["http://example.com/cb"]),
        json!([]),
    ] {
        let (s, e) = register(&h, bad.clone()).await;
        assert_eq!(s, StatusCode::BAD_REQUEST, "{bad}");
        assert!(e["error"] == "invalid_redirect_uri" || e["error"] == "invalid_client_metadata", "{e}");
    }
    let (s, reg) = register(&h, json!([REDIRECT])).await;
    assert_eq!(s, StatusCode::CREATED, "{reg}");
    let client_id = reg["client_id"].as_str().unwrap().to_owned();
    assert_eq!(reg["token_endpoint_auth_method"], "none");

    // A redirect URI that was not registered (other port) is refused, not followed.
    let r = web
        .http
        .get(format!("{}/oauth/authorize", h.base))
        .query(&[
            ("response_type", "code"),
            ("client_id", client_id.as_str()),
            ("redirect_uri", "http://127.0.0.1:47124/callback"),
            ("code_challenge", challenge().as_str()),
            ("code_challenge_method", "S256"),
        ])
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), StatusCode::BAD_REQUEST);

    // Consent: only the browser session (with CSRF) of the person who started it.
    let r = web
        .http
        .get(format!("{}/oauth/authorize", h.base))
        .query(&[
            ("response_type", "code"),
            ("client_id", client_id.as_str()),
            ("redirect_uri", REDIRECT),
            ("code_challenge", challenge().as_str()),
            ("code_challenge_method", "S256"),
        ])
        .send()
        .await
        .unwrap();
    let id = r.headers()["location"].to_str().unwrap().trim_start_matches("/connect?request=").to_owned();
    let mut no_csrf = Client::login(&h.base, ADMIN_EMAIL, PASSWORD).await;
    no_csrf.csrf = None;
    let (s, _) = no_csrf.post(&format!("/api/v1/oauth/consent/{id}"), json!({ "approve": true })).await;
    assert_eq!(s, StatusCode::FORBIDDEN, "consent needs the CSRF token");
    let other = user_with_role(&h, &web, "dev@example.com", "developer", &format!("workspace:{}", h.default_org)).await;
    let (s, _) = other.get(&format!("/api/v1/oauth/consent/{id}")).await;
    assert_eq!(s, StatusCode::NOT_FOUND, "another person cannot see or approve it");
    let (s, d) = web.post(&format!("/api/v1/oauth/consent/{id}"), json!({ "approve": false })).await;
    assert_eq!(s, StatusCode::OK);
    assert!(d["redirect"].as_str().unwrap().contains("error=access_denied"));
    let (s, _) = web.post(&format!("/api/v1/oauth/consent/{id}"), json!({ "approve": true })).await;
    assert_eq!(s, StatusCode::NOT_FOUND, "a request is decided once");

    // The full flow, read-only.
    let (tokens, _) = connect(&h, &web, &client_id, "read_only").await;
    let access = tokens["access_token"].as_str().unwrap().to_owned();
    let refresh = tokens["refresh_token"].as_str().unwrap().to_owned();

    // The connector's token is refused on the REST API, even with a header
    // that looks like the in-process marker.
    let mut agent_rest = Client::anonymous(&h.base);
    agent_rest.bearer = Some(access.clone());
    let (s, e) = agent_rest.get("/api/v1/me").await;
    assert_eq!(s, StatusCode::UNAUTHORIZED, "{e}");
    let r = reqwest::Client::new()
        .get(format!("{}/api/v1/servers", h.base))
        .bearer_auth(&access)
        .header("mcp-dispatch", "1")
        .header("x-mcp-dispatch", "1")
        .header("x-forge-mcp-connection", "anything")
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), StatusCode::UNAUTHORIZED);

    // Other tokens are refused at /mcp: an API key, the console session, a CLI token.
    let (s, key) = web.post("/api/v1/api-keys", json!({ "name": "k" })).await;
    assert!(s.is_success(), "{key}");
    let r =
        rpc(&h, Some(key["secret"].as_str().unwrap()), json!({ "jsonrpc": "2.0", "id": 1, "method": "ping" })).await;
    assert_eq!(r.status(), StatusCode::UNAUTHORIZED);
    let r = web.http.post(format!("{}/mcp", h.base)).json(&json!({ "jsonrpc": "2.0", "id": 1, "method": "ping" }));
    assert_eq!(r.send().await.unwrap().status(), StatusCode::UNAUTHORIZED);
    let cli_challenge = challenge();
    let r = web
        .http
        .get(format!("{}/oauth/authorize", h.base))
        .query(&[
            ("response_type", "code"),
            ("client_id", "forge-app"),
            ("redirect_uri", "http://127.0.0.1:53682/callback"),
            ("code_challenge", cli_challenge.as_str()),
            ("code_challenge_method", "S256"),
        ])
        .send()
        .await
        .unwrap();
    let url = url::Url::parse(r.headers()["location"].to_str().unwrap()).unwrap();
    let code = url.query_pairs().find(|(k, _)| k == "code").unwrap().1.into_owned();
    let (_, cli) = token(
        &h,
        &[
            ("grant_type", "authorization_code"),
            ("code", code.as_str()),
            ("code_verifier", VERIFIER),
            ("client_id", "forge-app"),
            ("redirect_uri", "http://127.0.0.1:53682/callback"),
        ],
    )
    .await;
    let r = rpc(&h, cli["access_token"].as_str(), json!({ "jsonrpc": "2.0", "id": 1, "method": "ping" })).await;
    assert_eq!(r.status(), StatusCode::UNAUTHORIZED, "a CLI token is not for /mcp");

    // MCP over Streamable HTTP.
    let init = rpc_ok(&h, &access, "initialize", json!({ "protocolVersion": "2025-06-18", "capabilities": {} })).await;
    assert_eq!(init["result"]["protocolVersion"], "2025-06-18");
    let r = rpc(&h, Some(&access), json!({ "jsonrpc": "2.0", "method": "notifications/initialized" })).await;
    assert_eq!(r.status(), StatusCode::ACCEPTED);
    let list = rpc_ok(&h, &access, "tools/list", json!({})).await;
    let tools = list["result"]["tools"].as_array().unwrap();
    assert!(tools.iter().any(|t| t["name"] == "forge_list_nodes"));
    assert!(tools.iter().all(|t| t["annotations"]["readOnlyHint"] == true), "read-only lists only read tools");
    let nodes = rpc_ok(&h, &access, "tools/call", json!({ "name": "forge_list_nodes", "arguments": {} })).await;
    assert!(nodes["result"]["isError"].is_null(), "{nodes}");
    let refused =
        rpc_ok(&h, &access, "tools/call", json!({ "name": "forge_cancel_job", "arguments": { "job": "x" } })).await;
    assert_eq!(refused["result"]["isError"], true, "read-only cannot mutate: {refused}");
    let r = reqwest::Client::new().get(format!("{}/mcp", h.base)).bearer_auth(&access).send().await.unwrap();
    assert_eq!(r.status(), StatusCode::METHOD_NOT_ALLOWED);
    let r = reqwest::Client::new()
        .post(format!("{}/mcp", h.base))
        .bearer_auth(&access)
        .header("origin", "https://evil.example")
        .json(&json!({ "jsonrpc": "2.0", "id": 1, "method": "ping" }))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), StatusCode::FORBIDDEN, "foreign Origin refused");

    // Refresh keeps the connection and its audience; a wrong resource is refused.
    let (s, _) = token(
        &h,
        &[("grant_type", "refresh_token"), ("refresh_token", refresh.as_str()), ("resource", "https://other/mcp")],
    )
    .await;
    assert_eq!(s, StatusCode::BAD_REQUEST);
    let (_, t2) = token(&h, &[("grant_type", "authorization_code"), ("code", "nope")]).await;
    assert!(t2.get("access_token").is_none());

    // Connected apps: listed, then revoked; the token stops at once.
    let (s, apps) = web.get("/api/v1/connected-apps").await;
    assert_eq!(s, StatusCode::OK);
    let conn = apps["connections"].as_array().unwrap().iter().find(|c| c["revoked_at"].is_null()).unwrap().clone();
    assert_eq!(conn["client_name"], "Test agent");
    assert_eq!(conn["mode"], "read_only");
    let (s, _) = web.post(&format!("/api/v1/connected-apps/{}/revoke", conn["id"].as_str().unwrap()), json!({})).await;
    assert_eq!(s, StatusCode::OK);
    let r = rpc(&h, Some(&access), json!({ "jsonrpc": "2.0", "id": 1, "method": "ping" })).await;
    assert_eq!(r.status(), StatusCode::UNAUTHORIZED);

    // Audit names the connector.
    let (_, log) = web.get("/api/v1/audit?limit=200").await;
    let items = log.as_array().cloned().or_else(|| log["items"].as_array().cloned()).unwrap_or_default();
    for a in ["oauth.client.register", "connector.approve", "connector.revoke"] {
        assert!(items.iter().any(|e| e["action"] == a), "missing audit {a}");
    }
}

#[tokio::test]
async fn auto_mode_runs_changes_through_the_rest_handlers_and_never_mfa() {
    let h = start_mcp("auto").await;
    let web = Client::login(&h.base, ADMIN_EMAIL, PASSWORD).await;
    let (_, reg) = register(&h, json!([REDIRECT])).await;
    let client_id = reg["client_id"].as_str().unwrap().to_owned();
    let (tokens, _) = connect(&h, &web, &client_id, "auto").await;
    let access = tokens["access_token"].as_str().unwrap().to_owned();

    let list = rpc_ok(&h, &access, "tools/list", json!({})).await;
    let names: Vec<&str> =
        list["result"]["tools"].as_array().unwrap().iter().filter_map(|t| t["name"].as_str()).collect();
    assert!(names.contains(&"forge_stack_service_action"), "{names:?}");
    assert!(names.contains(&"forge_deploy_stack"), "destructive tools are listed and wait for approval");
    // A change goes through the REST handler: unknown stack → its 404.
    let r = rpc_ok(
        &h,
        &access,
        "tools/call",
        json!({
            "name": "forge_stack_service_action",
            "arguments": {
                "stack": "01a1d3c8-8cd8-7489-8be5-e69b2042ef24",
                "server_id": "01a1d3c4-8250-754a-988c-8474d92c6a86",
                "action": "restart",
            },
        }),
    )
    .await;
    assert_eq!(r["result"]["isError"], true);
    assert!(r["result"]["content"][0]["text"].as_str().unwrap().contains("not found"), "{r}");
    // A malformed id is the handler's own rejection, passed on as text.
    let r = rpc_ok(
        &h,
        &access,
        "tools/call",
        json!({
            "name": "forge_stack_service_action",
            "arguments": { "stack": "not-a-uuid", "server_id": "x", "action": "restart" },
        }),
    )
    .await;
    assert_eq!(r["result"]["isError"], true);
    let text = r["result"]["content"][0]["text"].as_str().unwrap();
    assert!(!text.ends_with(": "), "the reason is passed on: {r}");
    // Arguments are validated before anything runs.
    let r =
        rpc_ok(&h, &access, "tools/call", json!({ "name": "forge_get_node", "arguments": { "node": "../me" } })).await;
    assert_eq!(r["result"]["isError"], true);
    let r = rpc_ok(&h, &access, "tools/call", json!({ "name": "forge_deploy_stack", "arguments": {} })).await;
    assert_eq!(r["result"]["isError"], true, "arguments are checked before an approval is queued: {r}");

    // The mode cap: a Forge capped at read_only offers no Auto.
    let capped = start_mcp("read_only").await;
    let web2 = Client::login(&capped.base, ADMIN_EMAIL, PASSWORD).await;
    let (_, reg2) = register(&capped, json!([REDIRECT])).await;
    let cid = reg2["client_id"].as_str().unwrap();
    let r = web2
        .http
        .get(format!("{}/oauth/authorize", capped.base))
        .query(&[
            ("response_type", "code"),
            ("client_id", cid),
            ("redirect_uri", REDIRECT),
            ("code_challenge", challenge().as_str()),
            ("code_challenge_method", "S256"),
        ])
        .send()
        .await
        .unwrap();
    let id = r.headers()["location"].to_str().unwrap().trim_start_matches("/connect?request=").to_owned();
    let (_, page) = web2.get(&format!("/api/v1/oauth/consent/{id}")).await;
    assert_eq!(page["modes"], json!(["read_only"]));
    let (s, _) = web2.post(&format!("/api/v1/oauth/consent/{id}"), json!({ "approve": true, "mode": "auto" })).await;
    assert_eq!(s, StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn ask_mode_waits_for_the_owner_who_runs_it_in_the_console() {
    let h = start_mcp("auto").await;
    let web = Client::login(&h.base, ADMIN_EMAIL, PASSWORD).await;
    let (_, reg) = register(&h, json!([REDIRECT])).await;
    let client_id = reg["client_id"].as_str().unwrap().to_owned();
    let (tokens, _) = connect(&h, &web, &client_id, "ask").await;
    let access = tokens["access_token"].as_str().unwrap().to_owned();

    // Changes are listed, marked as waiting for approval.
    let list = rpc_ok(&h, &access, "tools/list", json!({})).await;
    let tools = list["result"]["tools"].as_array().unwrap();
    let refresh = tools.iter().find(|t| t["name"] == "forge_refresh_stack").unwrap();
    assert!(refresh["description"].as_str().unwrap().contains("Waits for approval"), "{refresh}");
    assert!(tools.iter().any(|t| t["name"] == "forge_approval_status"));

    // A change becomes a pending approval instead of running.
    let stack = "01a1d3c8-8cd8-7489-8be5-e69b2042ef24";
    let r =
        rpc_ok(&h, &access, "tools/call", json!({ "name": "forge_refresh_stack", "arguments": { "stack": stack } }))
            .await;
    assert!(r["result"]["isError"].is_null(), "{r}");
    let id = r["result"]["structuredContent"]["approval_id"].as_str().unwrap().to_owned();
    assert_eq!(r["result"]["structuredContent"]["status"], "pending");
    let st =
        rpc_ok(&h, &access, "tools/call", json!({ "name": "forge_approval_status", "arguments": { "approval": id } }))
            .await;
    assert_eq!(st["result"]["structuredContent"]["status"], "pending", "{st}");

    // The agent cannot approve its own request.
    let r = reqwest::Client::new()
        .post(format!("{}/api/v1/approvals/{id}/approve", h.base))
        .bearer_auth(&access)
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), StatusCode::UNAUTHORIZED);
    // Nor can another person.
    let other = user_with_role(&h, &web, "ops@example.com", "operator", &format!("workspace:{}", h.default_org)).await;
    let (s, _) = other.post(&format!("/api/v1/approvals/{id}/approve"), json!({})).await;
    assert!(s == StatusCode::NOT_FOUND || s == StatusCode::FORBIDDEN, "{s}");

    // The owner sees it and approves: it runs once, as them, through the REST handler.
    let (_, mine) = web.get("/api/v1/approvals?status=pending").await;
    let item = mine["approvals"].as_array().unwrap().iter().find(|a| a["id"] == id.as_str()).unwrap().clone();
    assert_eq!(item["action"], "forge_refresh_stack");
    assert_eq!(item["requester"], "Test agent");
    let (s, done) = web.post(&format!("/api/v1/approvals/{id}/approve"), json!({})).await;
    assert_eq!(s, StatusCode::OK, "{done}");
    assert_eq!(done["status"], "failed", "the stack does not exist: {done}");
    assert!(done["error"].as_str().unwrap().contains("not found"), "{done}");
    let (s, _) = web.post(&format!("/api/v1/approvals/{id}/approve"), json!({})).await;
    assert_eq!(s, StatusCode::CONFLICT, "single use");
    let st =
        rpc_ok(&h, &access, "tools/call", json!({ "name": "forge_approval_status", "arguments": { "approval": id } }))
            .await;
    assert_eq!(st["result"]["structuredContent"]["status"], "failed", "{st}");

    // Denied.
    let r =
        rpc_ok(&h, &access, "tools/call", json!({ "name": "forge_refresh_stack", "arguments": { "stack": stack } }))
            .await;
    let id2 = r["result"]["structuredContent"]["approval_id"].as_str().unwrap().to_owned();
    let (s, d) = web.post(&format!("/api/v1/approvals/{id2}/deny"), json!({})).await;
    assert_eq!((s, d["status"].as_str()), (StatusCode::OK, Some("denied")));

    // Outside the console (the CLI, scripts) the owner may deny but never approve.
    let (_, key) = web.post("/api/v1/api-keys", json!({ "name": "cli" })).await;
    let key = key["secret"].as_str().unwrap().to_owned();
    let r =
        rpc_ok(&h, &access, "tools/call", json!({ "name": "forge_refresh_stack", "arguments": { "stack": stack } }))
            .await;
    let id3 = r["result"]["structuredContent"]["approval_id"].as_str().unwrap().to_owned();
    let cli = reqwest::Client::new();
    let r = cli.post(format!("{}/api/v1/approvals/{id3}/approve", h.base)).bearer_auth(&key).send().await.unwrap();
    assert_eq!(r.status(), StatusCode::FORBIDDEN);
    let r = cli.get(format!("{}/api/v1/approvals?status=pending", h.base)).bearer_auth(&key).send().await.unwrap();
    let listed: serde_json::Value = r.json().await.unwrap();
    assert!(listed["approvals"].as_array().unwrap().iter().any(|a| a["id"] == id3.as_str()), "{listed}");
    let r = cli.post(format!("{}/api/v1/approvals/{id3}/deny", h.base)).bearer_auth(&key).send().await.unwrap();
    assert_eq!(r.status(), StatusCode::OK);
    let d: serde_json::Value = r.json().await.unwrap();
    assert_eq!(d["status"], "denied");

    // Auto mode: destructive tools wait for approval too.
    let (tokens, _) = connect(&h, &web, &client_id, "auto").await;
    let auto = tokens["access_token"].as_str().unwrap().to_owned();
    let r =
        rpc_ok(&h, &auto, "tools/call", json!({ "name": "forge_deploy_stack", "arguments": { "stack": stack } })).await;
    assert_eq!(r["result"]["structuredContent"]["status"], "pending", "{r}");

    let (_, log) = web.get("/api/v1/audit?action=mcp.approval").await;
    let acts: Vec<&str> = log.as_array().unwrap().iter().filter_map(|e| e["action"].as_str()).collect();
    assert!(acts.contains(&"mcp.approval.request") && acts.contains(&"mcp.approval.decide"), "{acts:?}");
}

const STACK: &str = "01a1d3c8-8cd8-7489-8be5-e69b2042ef24";

/// Queues `tool` on `stack` and returns the approval id.
async fn ask(h: &Harness, access: &str, tool: &str) -> String {
    let r = rpc_ok(h, access, "tools/call", json!({ "name": tool, "arguments": { "stack": STACK } })).await;
    r["result"]["structuredContent"]["approval_id"].as_str().unwrap_or_else(|| panic!("not queued: {r}")).to_owned()
}

async fn approval(web: &Client, id: &str) -> Value {
    let (s, a) = web.get(&format!("/api/v1/approvals/{id}")).await;
    assert_eq!(s, StatusCode::OK, "{a}");
    a
}

/// Denied `mcp.approval.decide` entries that name `id`.
async fn refusals(web: &Client, id: &str) -> Vec<String> {
    let (_, log) = web.get("/api/v1/audit?action=mcp.approval.decide&limit=200").await;
    log.as_array()
        .unwrap()
        .iter()
        .filter(|e| e["outcome"] == "denied")
        .map(Value::to_string)
        .filter(|e| e.contains(id))
        .collect()
}

#[tokio::test]
async fn revoking_a_connection_or_client_cancels_its_approvals_and_refusals_are_audited() {
    let h = start_mcp("auto").await;
    let web = Client::login(&h.base, ADMIN_EMAIL, PASSWORD).await;
    let (_, reg) = register(&h, json!([REDIRECT])).await;
    let client_id = reg["client_id"].as_str().unwrap().to_owned();
    let (tokens, _) = connect(&h, &web, &client_id, "ask").await;
    let access = tokens["access_token"].as_str().unwrap().to_owned();

    // A destructive tool needs an MFA-verified session to approve, even when
    // its route alone is not under the MFA guardrail.
    let deploy = ask(&h, &access, "forge_deploy_stack").await;
    assert_eq!(approval(&web, &deploy).await["mfa_required"], true);
    let (s, e) = web.post(&format!("/api/v1/approvals/{deploy}/approve"), json!({})).await;
    assert_eq!(s, StatusCode::FORBIDDEN, "{e}");
    assert!(e.to_string().contains("step_up_required"), "{e}");
    assert_eq!(refusals(&web, &deploy).await.len(), 1, "the step-up refusal is audited");

    // An API key cannot approve; that refusal is audited too.
    let refresh = ask(&h, &access, "forge_refresh_stack").await;
    let (_, key) = web.post("/api/v1/api-keys", json!({ "name": "cli" })).await;
    let r = reqwest::Client::new()
        .post(format!("{}/api/v1/approvals/{refresh}/approve", h.base))
        .bearer_auth(key["secret"].as_str().unwrap())
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), StatusCode::FORBIDDEN);
    assert_eq!(refusals(&web, &refresh).await.len(), 1, "{:?}", refusals(&web, &refresh).await);

    // Revoking the connection cancels what it was waiting for.
    let (_, apps) = web.get("/api/v1/connected-apps").await;
    let conn = apps["connections"].as_array().unwrap().iter().find(|c| c["revoked_at"].is_null()).unwrap().clone();
    let (s, d) = web.post(&format!("/api/v1/connected-apps/{}/revoke", conn["id"].as_str().unwrap()), json!({})).await;
    assert_eq!(s, StatusCode::OK, "{d}");
    assert_eq!(d["cancelled_approvals"], 2, "{d}");
    for id in [&deploy, &refresh] {
        assert_eq!(approval(&web, id).await["status"], "cancelled");
        let (s, _) = web.post(&format!("/api/v1/approvals/{id}/approve"), json!({})).await;
        assert_eq!(s, StatusCode::CONFLICT, "a cancelled request never runs");
    }
    let (s, _) = web.post(&format!("/api/v1/approvals/{refresh}/deny"), json!({})).await;
    assert_eq!(s, StatusCode::CONFLICT);
    assert_eq!(refusals(&web, &refresh).await.len(), 3, "both later refusals are audited");
    let (_, log) = web.get("/api/v1/audit?action=mcp.approval.cancel").await;
    assert_eq!(log.as_array().unwrap().len(), 2, "{log}");

    // Revoking the client cancels the requests of every connection made with it.
    let (tokens, _) = connect(&h, &web, &client_id, "ask").await;
    let access = tokens["access_token"].as_str().unwrap().to_owned();
    let id = ask(&h, &access, "forge_refresh_stack").await;
    let (s, d) = web.post(&format!("/api/v1/oauth/clients/{client_id}/revoke"), json!({})).await;
    assert_eq!(s, StatusCode::OK, "{d}");
    assert_eq!(d["cancelled_approvals"], 1, "{d}");
    assert_eq!(approval(&web, &id).await["status"], "cancelled");
}

/// Another control plane on the same database, with different `[mcp]`
/// settings, as after an operator changed the configuration and restarted.
async fn restart_with(h: &Harness, enabled: bool, max_mode: &str) -> (forge_server::App, String) {
    let mut config = test_config(h.dir.path());
    let api = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let agent = TcpListener::bind("127.0.0.1:0").await.unwrap();
    config.public_url = Some(format!("http://{}", api.local_addr().unwrap()));
    config.mcp.enabled = enabled;
    config.mcp.max_mode = forge_core::model::AgentMode::parse(max_mode).unwrap();
    config.dev_skip_mfa = false;
    let state = Services::build(config).await.unwrap();
    let app = App::start_with(state, api, agent).unwrap();
    let base = format!("http://{}", app.api_addr);
    (app, base)
}

#[tokio::test]
async fn approving_rechecks_that_connectors_are_on_and_the_mode_still_allows_it() {
    let h = start_mcp("auto").await;
    let web = Client::login(&h.base, ADMIN_EMAIL, PASSWORD).await;
    let (_, reg) = register(&h, json!([REDIRECT])).await;
    let client_id = reg["client_id"].as_str().unwrap().to_owned();
    let (tokens, _) = connect(&h, &web, &client_id, "ask").await;
    let access = tokens["access_token"].as_str().unwrap().to_owned();
    let id = ask(&h, &access, "forge_refresh_stack").await;

    // The server cap was lowered to read-only: the change can no longer run.
    let (_app, base) = restart_with(&h, true, "read_only").await;
    let capped = Client::login(&base, ADMIN_EMAIL, PASSWORD).await;
    let (s, e) = capped.post(&format!("/api/v1/approvals/{id}/approve"), json!({})).await;
    assert_eq!(s, StatusCode::CONFLICT, "{e}");
    assert!(e.to_string().contains("mode"), "{e}");

    // Connectors were turned off.
    let (_app2, base2) = restart_with(&h, false, "auto").await;
    let off = Client::login(&base2, ADMIN_EMAIL, PASSWORD).await;
    let (s, e) = off.post(&format!("/api/v1/approvals/{id}/approve"), json!({})).await;
    assert_eq!(s, StatusCode::CONFLICT, "{e}");
    assert_eq!(refusals(&web, &id).await.len(), 2, "each refusal is audited");

    // Nothing ran; with the original settings the owner can still decide it.
    assert_eq!(approval(&web, &id).await["status"], "pending");
    let (s, d) = web.post(&format!("/api/v1/approvals/{id}/approve"), json!({})).await;
    assert_eq!((s, d["status"].as_str()), (StatusCode::OK, Some("failed")), "{d}");
}

#[tokio::test]
async fn expired_requests_do_not_block_new_ones() {
    use forge_core::ports::ApprovalRepository;
    let h = start_mcp("auto").await;
    let web = Client::login(&h.base, ADMIN_EMAIL, PASSWORD).await;
    let (_, reg) = register(&h, json!([REDIRECT])).await;
    let client_id = reg["client_id"].as_str().unwrap().to_owned();
    let (tokens, _) = connect(&h, &web, &client_id, "ask").await;
    let access = tokens["access_token"].as_str().unwrap().to_owned();
    let first = ask(&h, &access, "forge_refresh_stack").await;

    // Ten requests that expired without a decision (the per-connection limit).
    let store = &h.app.state.store;
    let template = store.get_approval(&first).await.unwrap().unwrap();
    let now = i64::try_from(std::time::SystemTime::UNIX_EPOCH.elapsed().unwrap().as_millis()).unwrap();
    let mut old = Vec::new();
    for i in 0..10 {
        let mut a = template.clone();
        a.id = format!("ap_expired_{i}");
        a.created_at = forge_core::Timestamp(now - 20 * 60 * 1000);
        a.expires_at = forge_core::Timestamp(now - 5 * 60 * 1000);
        store.create_approval(&a).await.unwrap();
        old.push(a.id);
    }
    // A decided request from long ago, past retention.
    let mut stale = template.clone();
    stale.id = "ap_stale".into();
    stale.status = "denied".into();
    stale.created_at = forge_core::Timestamp(1_000);
    store.create_approval(&stale).await.unwrap();

    let next = ask(&h, &access, "forge_refresh_stack").await;
    assert_ne!(next, first);
    for id in &old {
        assert_eq!(store.get_approval(id).await.unwrap().unwrap().status, "expired");
    }
    assert!(store.get_approval("ap_stale").await.unwrap().is_none(), "old decided requests are dropped");
    assert_eq!(store.get_approval(&first).await.unwrap().unwrap().status, "pending", "open requests stay");
}

#[tokio::test]
async fn connector_endpoints_need_a_pinned_address() {
    let h = start_mcp_pinned("auto", false).await;
    // An administrator signs in (records a last-admin address), which OAuth
    // must still not use.
    let _admin = Client::login(&h.base, ADMIN_EMAIL, PASSWORD).await;
    let anon = Client::anonymous(&h.base);
    for path in ["/.well-known/oauth-authorization-server", "/.well-known/oauth-protected-resource/mcp"] {
        let (s, v) = anon.get(path).await;
        assert_eq!(s, StatusCode::CONFLICT, "{path}: {v}");
        assert!(v.to_string().contains("no_public_address"), "{v}");
    }
    let (s, v) = register(&h, json!([REDIRECT])).await;
    assert_eq!(s, StatusCode::CONFLICT, "{v}");
    let r = rpc(&h, None, json!({ "jsonrpc": "2.0", "id": 1, "method": "ping" })).await;
    assert_eq!(r.status(), StatusCode::CONFLICT);
    h.app.stop().await;
}
