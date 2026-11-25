//! Regression tests for the endpoint (site), ingress, certificate, SSH
//! endpoint and node-move authorization fixes.
#![allow(clippy::unwrap_used)]

use crate::common::{ADMIN_EMAIL, Client, Harness, PASSWORD, start, user_with_role};
use forge_core::model::{AgentStatus, NodeRole, Server};
use reqwest::{Method, StatusCode};
use serde_json::{Value, json};

fn node(h: &Harness, hostname: &str, status: AgentStatus) -> Server {
    Server {
        id: forge_core::ServerId::new(),
        workspace_id: h.default_org.parse().unwrap(),
        environment_id: None,
        cluster_id: None,
        role: NodeRole::Worker,
        display_name: None,
        hostname: hostname.into(),
        os: "linux".into(),
        arch: "x86_64".into(),
        agent_version: "0".into(),
        status,
        last_seen_at: None,
        cert_serial: None,
        cert_not_after: None,
        created_at: forge_core::Timestamp(0),
    }
}

async fn add_node(h: &Harness, hostname: &str, status: AgentStatus) -> Server {
    let n = node(h, hostname, status);
    forge_core::ports::ServerRepository::create_server(&h.app.state.store, &n).await.unwrap();
    n
}

/// Enrolls TOTP for `c` and steps its session up to MFA.
async fn step_up(c: &mut Client) {
    use forge_identity::totp;
    let (_, t) = c.post("/api/v1/auth/totp", json!({})).await;
    let secret = totp::base32_decode(t["secret"].as_str().unwrap()).unwrap();
    let now =
        i64::try_from(std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_secs()).unwrap();
    let code = format!("{:06}", totp::code_at(&secret, now));
    let (s, b) =
        c.post("/api/v1/auth/totp/confirm", json!({"password": PASSWORD, "secret": t["secret"], "code": code})).await;
    assert_eq!(s, StatusCode::NO_CONTENT, "{b}");
    let (s, r) = c.post("/api/v1/auth/step-up", json!({"code": code})).await;
    assert_eq!(s, StatusCode::OK, "{r}");
    c.csrf = r["csrf_token"].as_str().map(str::to_owned);
}

fn site(name: &str, edge: &Server, custom: &str) -> Value {
    json!({"name": name, "domains": [format!("{name}.example.com")], "upstreams": ["10.0.0.5:8000"],
           "edges": [edge.id], "custom": custom})
}

#[tokio::test]
async fn sites_are_scoped_to_their_workspace() {
    use forge_core::ports::{SiteRepository, TenancyRepository};
    let h = start().await;
    let admin = Client::login(&h.base, ADMIN_EMAIL, PASSWORD).await;
    let edge = add_node(&h, "edge-1", AgentStatus::Online).await;
    let store = &h.app.state.store;
    let other = forge_core::model::Workspace {
        id: forge_core::WorkspaceId::new(),
        name: "Other".into(),
        slug: "other".into(),
        created_at: forge_core::Timestamp(i64::MAX / 2),
    };
    store.create_workspace(&other).await.unwrap();
    let foreign = forge_core::model::Site {
        id: "foreign-site".into(),
        workspace_id: other.id,
        name: "foreign".into(),
        config: json!({"spec": {"domains": ["f.example.com"], "upstreams": ["10.0.0.9:80"]}, "edges": [edge.id]}),
        edge_status: json!({}),
        created_at: forge_core::Timestamp(0),
        updated_at: forge_core::Timestamp(0),
    };
    store.put_site(&foreign).await.unwrap();

    // One deployment is one workspace (D-016): with a second one in the
    // store, every workspace route fails closed (503) instead of serving or
    // touching another workspace's site.
    let (s, list) = admin.get("/api/v1/sites").await;
    assert_eq!(s, StatusCode::SERVICE_UNAVAILABLE, "{list}");
    assert!(!list.to_string().contains("foreign"), "{list}");
    let path = "/api/v1/sites/foreign-site";
    assert_eq!(admin.get(path).await.0, StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(admin.put(path, site("foreign", &edge, "")).await.0, StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(admin.call(Method::DELETE, path, None).await.0, StatusCode::SERVICE_UNAVAILABLE);
    for action in ["apply", "remove", "renew"] {
        let (s, _) = admin.post(&format!("{path}/{action}"), json!({})).await;
        assert!(matches!(s, StatusCode::SERVICE_UNAVAILABLE | StatusCode::NOT_FOUND), "{action}: {s}");
    }
    assert!(store.get_site("foreign-site").await.unwrap().is_some(), "untouched");
}

#[tokio::test]
async fn custom_blocks_need_server_command_and_the_allowlist() {
    let h = start().await;
    let mut admin = Client::login(&h.base, ADMIN_EMAIL, PASSWORD).await;
    let edge = add_node(&h, "edge-1", AgentStatus::Online).await;

    // Without a custom block: workspace.manage is enough to save.
    let (s, b) = admin.post("/api/v1/sites", site("plain", &edge, "")).await;
    assert_eq!(s, StatusCode::CREATED, "{b}");
    // A non-empty custom block needs server.command (MFA) on the edge.
    let (s, e) = admin.post("/api/v1/sites", site("custom", &edge, "add_header X-A 1;")).await;
    assert_eq!(s, StatusCode::FORBIDDEN, "{e}");

    step_up(&mut admin).await;
    let (s, b) = admin.post("/api/v1/sites", site("custom", &edge, "add_header X-A 1;")).await;
    assert_eq!(s, StatusCode::CREATED, "{b}");
    for bad in [
        "log_format x '$http_x';\naccess_log /etc/cron.d/x x;",
        "location /a { alias /etc/; }",
        "\"root\" /;",
        "set_by_lua $x 'os.execute(1)';",
        "location / { proxy_pass http://unix:/var/run/docker.sock:; }",
    ] {
        let (s, e) = admin.post("/api/v1/sites", site("evil", &edge, bad)).await;
        assert_eq!(s, StatusCode::BAD_REQUEST, "{bad}: {e}");
    }

    // The same for the cluster profile's global block.
    let (_, c) = admin.post("/api/v1/clusters", json!({"name": "lab"})).await;
    let cid = c["id"].as_str().unwrap().to_owned();
    let (s, b) =
        admin.call(Method::PATCH, &format!("/api/v1/servers/{}", edge.id), Some(json!({"cluster_id": cid}))).await;
    assert_eq!(s, StatusCode::OK, "{b}");
    let profile = |custom: &str| json!({"ingress": {"mode": "standalone", "edges": [{"server_id": edge.id}], "nginx": {"custom": custom}}});
    let path = format!("/api/v1/clusters/{cid}/profile");
    let (s, e) = admin.put(&path, profile("access_log /etc/cron.d/x;")).await;
    assert_eq!(s, StatusCode::BAD_REQUEST, "{e}");
    let (s, b) = admin.put(&path, profile("client_body_buffer_size 1m;")).await;
    assert_eq!(s, StatusCode::OK, "{b}");

    // Another administrator, without MFA, cannot save a global block.
    let second =
        user_with_role(&h, &admin, "wa@example.com", "workspace_admin", &format!("workspace:{}", h.default_org)).await;
    let (s, e) = second.put(&path, profile("client_body_buffer_size 2m;")).await;
    assert_eq!(s, StatusCode::FORBIDDEN, "{e}");
    let (s, b) = second.put(&path, profile("")).await;
    assert_eq!(s, StatusCode::OK, "no custom block, no server.command: {b}");
}

#[tokio::test]
async fn uploaded_certificates_are_not_sent_by_generic_commands() {
    let h = start().await;
    let mut admin = Client::login(&h.base, ADMIN_EMAIL, PASSWORD).await;
    step_up(&mut admin).await;
    let edge = add_node(&h, "edge-1", AgentStatus::Online).await;
    let cmd = json!({"command": {"type": "site", "action": "apply", "spec": {
        "name": "x", "domains": ["x.example.com"], "upstreams": ["10.0.0.5:80"], "tls": "uploaded",
        "cert_id": forge_core::CertificateId::new()}}});
    let (s, e) = admin.post(&format!("/api/v1/servers/{}/commands", edge.id), cmd).await;
    assert_eq!(s, StatusCode::BAD_REQUEST, "{e}");
}

#[tokio::test]
async fn ssh_endpoints_need_mfa_and_cannot_point_at_another_node() {
    let h = start().await;
    let mut admin = Client::login(&h.base, ADMIN_EMAIL, PASSWORD).await;
    let a = add_node(&h, "node-a", AgentStatus::Offline).await;
    let b = add_node(&h, "node-b", AgentStatus::Offline).await;
    let put = |id: forge_core::ServerId, host: &str, port: u16, fp: &str| {
        (format!("/api/v1/servers/{id}/ssh"), json!({"host": host, "port": port, "host_key_fingerprint": fp}))
    };
    let (p, body) = put(b.id, "10.0.0.20", 22, "SHA256:bbbb");
    let (s, e) = admin.put(&p, body.clone()).await;
    assert_eq!(s, StatusCode::FORBIDDEN, "server.command needs MFA: {e}");

    step_up(&mut admin).await;
    let (s, e) = admin.put(&p, body).await;
    assert_eq!(s, StatusCode::OK, "{e}");
    // Node A cannot borrow node B's host key or endpoint.
    let (p, body) = put(a.id, "10.0.0.99", 22, "SHA256:bbbb");
    assert_eq!(admin.put(&p, body).await.0, StatusCode::BAD_REQUEST);
    let (p, body) = put(a.id, "10.0.0.20", 22, "SHA256:aaaa");
    assert_eq!(admin.put(&p, body).await.0, StatusCode::BAD_REQUEST);
    let (p, body) = put(a.id, "10.0.0.10", 22, "SHA256:aaaa");
    let (s, e) = admin.put(&p, body).await;
    assert_eq!(s, StatusCode::OK, "{e}");

    let (_, audit) = admin.get("/api/v1/audit?limit=200").await;
    assert!(audit.to_string().contains("server.ssh_endpoint"), "{audit}");
}

#[tokio::test]
async fn moving_nodes_needs_enroll_rights_on_both_sides() {
    use forge_core::ports::ServerRepository;
    let h = start().await;
    let admin = Client::login(&h.base, ADMIN_EMAIL, PASSWORD).await;
    let (_, x) = admin.post("/api/v1/clusters", json!({"name": "x"})).await;
    let (_, y) = admin.post("/api/v1/clusters", json!({"name": "y"})).await;
    let (_, ag1) = admin.post("/api/v1/projects", json!({"name": "grp1"})).await;
    let (_, nx) = admin
        .post("/api/v1/environments", json!({"name": "web", "cluster_id": x["id"], "project_id": ag1["id"]}))
        .await;
    let (_, ag2) = admin.post("/api/v1/projects", json!({"name": "grp2"})).await;
    let (_, ny) = admin
        .post("/api/v1/environments", json!({"name": "api", "cluster_id": y["id"], "project_id": ag2["id"]}))
        .await;
    let n = add_node(&h, "worker-1", AgentStatus::Online).await;
    let path = format!("/api/v1/servers/{}", n.id);
    let (s, b) =
        admin.call(Method::PATCH, &path, Some(json!({"cluster_id": x["id"], "environment_id": nx["id"]}))).await;
    assert_eq!(s, StatusCode::OK, "{b}");

    // An operator of cluster X manages the node but not environment Y, nor the
    // environment the node is in.
    let op =
        user_with_role(&h, &admin, "op@example.com", "operator", &format!("cluster:{}", x["id"].as_str().unwrap()))
            .await;
    let (s, e) = op.call(Method::PATCH, &path, Some(json!({"cluster_id": x["id"], "environment_id": ny["id"]}))).await;
    assert_eq!(s, StatusCode::FORBIDDEN, "{e}");
    let (s, e) = op.call(Method::PATCH, &path, Some(json!({"cluster_id": x["id"]}))).await;
    assert_eq!(s, StatusCode::FORBIDDEN, "leaving environment web needs server.enroll there: {e}");
    let still = h.app.state.store.get_server(n.id).await.unwrap();
    assert_eq!(still.environment_id.map(|i| i.to_string()).as_deref(), nx["id"].as_str());

    // An operator of environment web cannot take the node out of cluster X.
    let nop = user_with_role(
        &h,
        &admin,
        "nop@example.com",
        "operator",
        &format!("environment:{}", nx["id"].as_str().unwrap()),
    )
    .await;
    let (s, e) = nop.call(Method::PATCH, &path, Some(json!({"cluster_id": null, "environment_id": nx["id"]}))).await;
    assert!(s == StatusCode::FORBIDDEN, "{s} {e}");
    let still = h.app.state.store.get_server(n.id).await.unwrap();
    assert_eq!(still.cluster_id.map(|i| i.to_string()).as_deref(), x["id"].as_str());
}

#[tokio::test]
async fn apply_and_remove_without_edges_change_nothing() {
    use forge_core::ports::SiteRepository;
    let h = start().await;
    let admin = Client::login(&h.base, ADMIN_EMAIL, PASSWORD).await;
    let viewer =
        user_with_role(&h, &admin, "aud@example.com", "auditor", &format!("workspace:{}", h.default_org)).await;
    let store = &h.app.state.store;
    let empty = forge_core::model::Site {
        id: "no-edges".into(),
        workspace_id: h.default_org.parse().unwrap(),
        name: "no-edges".into(),
        config: json!({"spec": {"domains": ["e.example.com"], "upstreams": ["10.0.0.9:80"]}, "edges": []}),
        edge_status: json!({}),
        created_at: forge_core::Timestamp(0),
        updated_at: forge_core::Timestamp(0),
    };
    store.put_site(&empty).await.unwrap();
    for action in ["apply", "remove"] {
        let (s, e) = viewer.post(&format!("/api/v1/sites/no-edges/{action}"), json!({})).await;
        assert_eq!(s, StatusCode::BAD_REQUEST, "{action}: {e}");
    }
    let after = store.get_site("no-edges").await.unwrap().unwrap();
    assert_eq!(after.edge_status, json!({}), "no _applied_by written");

    // A profile without ingress: applying it is refused, its status untouched.
    let (_, c) = admin.post("/api/v1/clusters", json!({"name": "lab"})).await;
    let cid = c["id"].as_str().unwrap();
    let (s, b) = admin.put(&format!("/api/v1/clusters/{cid}/profile"), json!({"ingress": null})).await;
    assert_eq!(s, StatusCode::OK, "{b}");
    for action in ["apply", "remove"] {
        let (s, e) = viewer.post(&format!("/api/v1/clusters/{cid}/profile/{action}"), json!({})).await;
        assert_eq!(s, StatusCode::BAD_REQUEST, "{action}: {e}");
    }
    let (_, p) = admin.get(&format!("/api/v1/clusters/{cid}/profile")).await;
    assert!(p["status"].get("applied_at").is_none(), "{p}");
}

/// The job itself re-checks an uploaded key's destination: only an edge of
/// the site being applied, and only for that site's certificate.
#[tokio::test]
async fn uploaded_keys_go_only_to_the_sites_own_edges() {
    use forge_core::model::{JobStatus, Site};
    use forge_core::ports::{JobRepository, SiteRepository, UserRepository};
    use forge_core::{Principal, Timestamp};
    let h = start().await;
    let store = &h.app.state.store;
    let edge = add_node(&h, "edge-1", AgentStatus::Online).await;
    let other = add_node(&h, "edge-2", AgentStatus::Online).await;
    let cert = forge_core::CertificateId::new().to_string();
    let site_id = "site-cert-test".to_owned();
    store
        .put_site(&Site {
            id: site_id.clone(),
            workspace_id: h.default_org.parse().unwrap(),
            name: "x".into(),
            config: json!({"spec": {"domains": ["x.example.com"], "upstreams": ["10.0.0.5:80"], "tls": "uploaded", "cert_id": cert},
                           "edges": [edge.id]}),
            edge_status: json!({}),
            created_at: Timestamp(0),
            updated_at: Timestamp(0),
        })
        .await
        .unwrap();
    let admin = store.find_user_by_email(ADMIN_EMAIL).await.unwrap().unwrap();
    let wait = |id| async move {
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(20);
        loop {
            let j = store.get_job(id).await.unwrap();
            if matches!(j.status, JobStatus::Failed | JobStatus::Succeeded | JobStatus::Cancelled) {
                return j;
            }
            assert!(std::time::Instant::now() < deadline, "job did not finish");
            tokio::time::sleep(std::time::Duration::from_millis(100)).await;
        }
    };
    let j = wait(attach_job(&h, Principal::User(admin.id), &site_id, other.id, &cert).await).await;
    assert_eq!(j.status, JobStatus::Failed);
    assert!(j.last_error.unwrap_or_default().contains("not an edge"), "a node outside the site is refused");
    let j = wait(
        attach_job(&h, Principal::User(admin.id), &site_id, edge.id, &forge_core::CertificateId::new().to_string())
            .await,
    )
    .await;
    assert_eq!(j.status, JobStatus::Failed);
    assert!(j.last_error.unwrap_or_default().contains("does not use this certificate"));
}

/// Submits a site apply job as if the endpoint flow had sent it.
async fn attach_job(
    h: &Harness,
    principal: forge_core::Principal,
    site_id: &str,
    server: forge_core::ServerId,
    cert_id: &str,
) -> forge_core::JobId {
    use forge_core::{AuthzContext, EntityKind, EntityRef};
    let job = h
        .app
        .state
        .jobs
        .submit(forge_core::model::NewJob {
            kind: "agent.command".into(),
            workspace_id: Some(h.default_org.parse().unwrap()),
            resource: EntityRef::new(EntityKind::Server, server),
            principal,
            context: AuthzContext { mfa: true },
            idempotency_key: None,
            payload: json!({"server_id": server, "attach_certificate": site_id, "command": {"type": "site", "action": "apply",
                "spec": {"name": "x", "domains": ["x.example.com"], "upstreams": ["10.0.0.5:80"], "tls": "uploaded", "cert_id": cert_id}}}),
            max_attempts: 1,
        })
        .await
        .unwrap();
    job.id
}

/// Issue #25: removing the profile releases the edge role of every edge it
/// was taken off; an edge whose removal did not succeed keeps the role (and
/// still blocks the move) so the removal can be retried.
#[tokio::test]
async fn removing_the_profile_releases_the_edge_role() {
    let h = start().await;
    let mut admin = Client::login(&h.base, ADMIN_EMAIL, PASSWORD).await;
    step_up(&mut admin).await;
    let (_, c) = admin.post("/api/v1/clusters", json!({"name": "edge-lab"})).await;
    let cid = c["id"].as_str().unwrap().to_owned();
    let (_, other) = admin.post("/api/v1/clusters", json!({"name": "edge-uat"})).await;
    let mut edge = node(&h, "edge-1", AgentStatus::Offline);
    edge.cluster_id = Some(cid.parse().unwrap());
    forge_core::ports::ServerRepository::create_server(&h.app.state.store, &edge).await.unwrap();
    let profile = json!({"ingress": {"mode": "standalone", "edges": [{"server_id": edge.id}]}});
    let (s, b) = admin.put(&format!("/api/v1/clusters/{cid}/profile"), profile).await;
    assert_eq!(s, StatusCode::OK, "{b}");
    let mv = json!({"cluster_id": other["id"]});
    let path = format!("/api/v1/servers/{}", edge.id);
    assert_eq!(admin.call(Method::PATCH, &path, Some(mv.clone())).await.0, StatusCode::CONFLICT, "still the edge");

    // The edge is offline: its removal does not succeed and it keeps the
    // role. The answer comes at once (not after the 120 s an online edge
    // gets), with the job pending until the edge reconnects.
    let asked = std::time::Instant::now();
    let (s, r) = admin.post(&format!("/api/v1/clusters/{cid}/profile/remove"), json!({})).await;
    assert!(asked.elapsed() < std::time::Duration::from_secs(10), "{:?}", asked.elapsed());
    assert_eq!(s, StatusCode::OK, "{r}");
    let answer = &r["edges"][edge.id.to_string()];
    assert_eq!(answer["status"], "pending", "{r}");
    let job = answer["job_id"].as_str().unwrap();
    let (_, j) = admin.get(&format!("/api/v1/jobs/{job}")).await;
    assert!(matches!(j["status"].as_str(), Some("queued" | "running")), "the job waits for the edge: {j}");
    let (_, p) = admin.get(&format!("/api/v1/clusters/{cid}/profile")).await;
    assert_eq!(p["profile"]["ingress"]["edges"][0]["server_id"], edge.id.to_string(), "{p}");
    assert_eq!(admin.call(Method::PATCH, &path, Some(mv.clone())).await.0, StatusCode::CONFLICT);

    // A successful removal needs the agent's privileged helper, which tests do
    // not have: store what the handler leaves behind (the settings with an
    // empty edge list). An empty list is no edge, so the move goes through.
    let store = &h.app.state.store;
    let mut rec = forge_core::ports::ClusterProfileRepository::get_cluster_profile(store, cid.parse().unwrap())
        .await
        .unwrap()
        .unwrap();
    rec.profile["ingress"]["edges"] = json!([]);
    forge_core::ports::ClusterProfileRepository::put_cluster_profile(store, &rec).await.unwrap();
    let (_, p) = admin.get(&format!("/api/v1/clusters/{cid}/profile")).await;
    assert_eq!(p["profile"]["ingress"]["mode"], "standalone", "settings kept: {p}");
    assert_eq!(p["profile"]["ingress"]["edges"], json!([]), "{p}");
    let (s, b) = admin.call(Method::PATCH, &path, Some(mv)).await;
    assert_eq!(s, StatusCode::OK, "released edge moves: {b}");
}
