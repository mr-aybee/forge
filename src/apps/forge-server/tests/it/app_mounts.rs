//! Service containers: host bind mounts outside the app's own folder and
//! the node's approved file roots need `stack.privileged` (administrators,
//! MFA), like compose stacks. A plain deploy cannot mount `/var/spool/cron`,
//! `/home` or another environment's app folder (security regression).
#![allow(clippy::unwrap_used)]

use crate::common::*;
use reqwest::{Method, StatusCode};
use serde_json::{Value, json};

async fn step_up(c: &mut Client) {
    use forge_identity::totp;
    let (_, t) = c.post("/api/v1/auth/totp", json!({})).await;
    let secret = totp::base32_decode(t["secret"].as_str().unwrap()).unwrap();
    let code = || {
        let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_secs();
        format!("{:06}", totp::code_at(&secret, i64::try_from(now).unwrap()))
    };
    let (s, _) =
        c.post("/api/v1/auth/totp/confirm", json!({"password": PASSWORD, "secret": t["secret"], "code": code()})).await;
    assert_eq!(s, StatusCode::NO_CONTENT);
    let (s, r) = c.post("/api/v1/auth/step-up", json!({"code": code()})).await;
    assert_eq!(s, StatusCode::OK, "{r}");
    c.csrf = r["csrf_token"].as_str().map(str::to_owned);
}

async fn node(h: &Harness) -> forge_core::ServerId {
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
        created_at: forge_core::Timestamp(0),
    };
    forge_core::ports::ServerRepository::create_server(&h.app.state.store, &server).await.unwrap();
    id
}

/// A container app named `name` with `volume`, placed on `server`.
async fn app(c: &Client, ns: &Value, name: &str, volume: &str, server: forge_core::ServerId) -> String {
    let (s, app) = c
        .post(
            "/api/v1/services",
            json!({
                "environment_id": ns["id"], "name": name, "runtime": "container",
                "image": "nginx:1.27", "volumes": [volume],
            }),
        )
        .await;
    assert_eq!(s, StatusCode::CREATED, "{app}");
    let id = app["id"].as_str().unwrap().to_owned();
    let (s, r) = c.put(&format!("/api/v1/services/{id}/instances"), json!([{"server_id": server}])).await;
    assert_eq!(s, StatusCode::OK, "{r}");
    id
}

async fn job_payload(h: &Harness, job: &str) -> Value {
    let raw: String = sqlx::query_scalar("SELECT payload FROM jobs WHERE id = $1")
        .bind(job)
        .fetch_one(h.app.state.store.pool())
        .await
        .unwrap();
    serde_json::from_str(&raw).unwrap()
}

#[tokio::test]
async fn mounts_outside_the_app_folder_need_stack_privileged_with_mfa() {
    let h = start().await;
    let mut admin = Client::login(&h.base, ADMIN_EMAIL, PASSWORD).await;
    let (_, cl) = admin.post("/api/v1/clusters", json!({"name": "shop"})).await;
    let (_, ag1) = admin.post("/api/v1/projects", json!({"name": "grp1"})).await;
    let (s, ns) = admin
        .post("/api/v1/environments", json!({"name": "uat", "cluster_id": cl["id"], "project_id": ag1["id"]}))
        .await;
    assert_eq!(s, StatusCode::CREATED, "{ns}");
    let server = node(&h).await;

    // Paths the system-path deny-list does not cover, but that lead to root
    // on the node or to another tenant's data.
    let escalations =
        ["/var/spool/cron:/c", "/var/lib/dpkg:/d", "/home:/h", "/srv/forge/apps/prod/db/data:/d", "/opt/vendor:/o"];
    for (n, v) in escalations.iter().enumerate() {
        let id = app(&admin, &ns, &format!("esc-{n}"), v, server).await;
        // Deploy to every placement, and the per-node deploy action.
        let (s, r) = admin.post(&format!("/api/v1/services/{id}/deploy"), json!({})).await;
        assert_eq!(s, StatusCode::FORBIDDEN, "{v}: a deploy without MFA may not mount it: {r}");
        let (s, r) =
            admin.post(&format!("/api/v1/services/{id}/instances/{server}/actions"), json!({"action": "deploy"})).await;
        assert_eq!(s, StatusCode::FORBIDDEN, "{v}: {r}");
    }
    // System paths stay refused for everyone, at save time.
    let (s, r) = admin
        .post(
            "/api/v1/services",
            json!({"environment_id": ns["id"], "name": "etc", "runtime": "container", "image": "nginx:1.27", "volumes": ["/etc:/e"]}),
        )
        .await;
    assert_eq!(s, StatusCode::BAD_REQUEST, "{r}");

    // The app's own folder is a plain deploy: no MFA, no privileged mounts.
    let own = app(&admin, &ns, "web", "/srv/forge/apps/uat/web/data:/data", server).await;
    let (s, jobs) = admin.post(&format!("/api/v1/services/{own}/deploy"), json!({})).await;
    assert_eq!(s, StatusCode::ACCEPTED, "{jobs}");
    let p = job_payload(&h, jobs[0]["id"].as_str().unwrap()).await;
    assert_eq!(p["privileged_mounts"], json!([]), "{p}");

    // An administrator with MFA may: the job carries the mounts, so it
    // re-authorizes stack.privileged at every step, and the audit names them.
    step_up(&mut admin).await;
    let id = app(&admin, &ns, "cron", "/var/spool/cron:/c", server).await;
    let (s, jobs) = admin.post(&format!("/api/v1/services/{id}/deploy"), json!({})).await;
    assert_eq!(s, StatusCode::ACCEPTED, "{jobs}");
    let p = job_payload(&h, jobs[0]["id"].as_str().unwrap()).await;
    assert_eq!(p["privileged_mounts"], json!(["/var/spool/cron:/c"]), "{p}");
    let (_, audit) = admin.call(Method::GET, "/api/v1/audit?action=service.deploy", None).await;
    assert!(audit.to_string().contains("/var/spool/cron:/c"), "{audit}");
}
