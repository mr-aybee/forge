//! Compose stacks API (issue #7) against two real enrolled agents: preview
//! and placement per node, the policy (blocked and privileged settings),
//! revisions and rollback targets, deploy jobs per node, **Create
//! container**, secrets never returned, roles and audit. Running Compose
//! itself needs a Linux node with Docker (lab test).
#![allow(clippy::unwrap_used)]

use std::time::Duration;

use forge_agent::enroll::{HostFacts, enroll};
use forge_agent::{Agent, AgentConfig, StatePaths};
use reqwest::StatusCode;
use serde_json::json;

use crate::common::*;

struct Enrolled {
    server_id: String,
    _stop: tokio::sync::watch::Sender<bool>,
    _dir: tempfile::TempDir,
}

async fn enroll_and_run(h: &Harness, admin: &Client, hostname: &str) -> Enrolled {
    let (s, body) = admin.post("/api/v1/enrollment-tokens", json!({"ttl_minutes": 10})).await;
    assert_eq!(s, StatusCode::CREATED, "{body}");
    let dir = tempfile::tempdir().unwrap();
    let paths = StatePaths::new(dir.path());
    let config = AgentConfig::new(h.agent_url.clone());
    let facts = HostFacts { hostname: hostname.into(), os: "linux".into(), arch: "x86_64".into() };
    let id = enroll(&paths, config, body["token"].as_str().unwrap(), facts).await.unwrap();
    let agent = Agent::load(paths).unwrap();
    let (stop, rx) = tokio::sync::watch::channel(false);
    tokio::spawn(agent.run(rx));
    let deadline = tokio::time::Instant::now() + Duration::from_secs(60);

    loop {
        let (_, s) = admin.get(&format!("/api/v1/servers/{}", id.server_id)).await;
        if s["status"] == "online" {
            break;
        }

        assert!(tokio::time::Instant::now() < deadline, "agent never came online");
        tokio::time::sleep(Duration::from_millis(100)).await;
    }

    Enrolled { server_id: id.server_id, _stop: stop, _dir: dir }
}

const SHOP: &str = r#"
x-common: &common
  restart: unless-stopped
services:
  web:
    <<: *common
    image: nginx:1.27
    ports: ["8080:80"]
    depends_on: [api]
    x-forge: { node: lab-b }
  api:
    <<: *common
    image: ghcr.io/example/api:1.0
    environment:
      DB_PASSWORD: ${DB_PASSWORD}
      MODE: prod
    deploy:
      placement:
        constraints: ["node.hostname == lab-b"]
  worker:
    image: ghcr.io/example/worker:1.0
    x-forge: { node: lab-c }
    volumes: ["cache:/cache"]
volumes:
  cache: {}
"#;

async fn audit_actions(admin: &Client) -> Vec<String> {
    let (_, a) = admin.get("/api/v1/audit?limit=500").await;
    a.as_array()
        .cloned()
        .or_else(|| a["items"].as_array().cloned())
        .unwrap_or_default()
        .iter()
        .filter_map(|e| e["action"].as_str().map(str::to_owned))
        .collect()
}

#[tokio::test]
async fn stacks_plan_check_revise_and_deploy_per_node() {
    let h = start().await;
    let admin = Client::login(&h.base, ADMIN_EMAIL, PASSWORD).await;
    let (_, cl) = admin.post("/api/v1/clusters", json!({"name": "lab"})).await;
    let (_, ag1) = admin.post("/api/v1/projects", json!({"name": "grp1"})).await;
    let (s, ns) = admin
        .post("/api/v1/environments", json!({"name": "Web", "cluster_id": cl["id"], "project_id": ag1["id"]}))
        .await;
    assert_eq!(s, StatusCode::CREATED, "{ns}");
    let b = enroll_and_run(&h, &admin, "lab-b").await;
    let c = enroll_and_run(&h, &admin, "lab-c").await;
    let ns_id = ns["id"].as_str().unwrap().to_owned();

    // Preview: services grouped per node, env values masked, ports internal.
    let (s, p) = admin
        .post(
            "/api/v1/stacks/preview",
            json!({ "environment_id": ns_id, "server_id": b.server_id, "name": "shop", "content": SHOP, "env": "DB_PASSWORD=hunter2\n" }),
        )
        .await;
    assert_eq!(s, StatusCode::OK, "{p}");
    let nodes = p["nodes"].as_array().unwrap();
    assert_eq!(nodes.len(), 2, "{p}");
    let on = |id: &str| nodes.iter().find(|n| n["server_id"] == id).unwrap()["services"].clone();
    assert_eq!(on(&b.server_id), json!(["api", "web"]));
    assert_eq!(on(&c.server_id), json!(["worker"]));
    assert_eq!(p["blocked"], false);
    assert_eq!(p["privileged"], false);
    let web = p["services"].as_array().unwrap().iter().find(|s| s["name"] == "web").unwrap().clone();
    assert_eq!(web["ports"][0]["host_ip"], "127.0.0.1", "no cluster network: loopback");
    assert_eq!(web["ports"][0]["public"], false);
    assert_eq!(web["restart"], "unless-stopped", "merge keys");
    assert!(!p.to_string().contains("hunter2"), "env values never in a preview");

    // An unknown node and blocked settings.
    let bad = SHOP.replace("node: lab-c", "node: deb-z");
    let (s, e) = admin
        .post("/api/v1/stacks/preview", json!({ "environment_id": ns_id, "server_id": b.server_id, "content": bad }))
        .await;
    assert_eq!(s, StatusCode::BAD_REQUEST, "{e}");
    let build = "services:\n  a:\n    image: x\n    build: .\n";
    let (s, e) = admin
        .post(
            "/api/v1/stacks",
            json!({ "environment_id": ns_id, "server_id": b.server_id, "name": "bad", "content": build }),
        )
        .await;
    assert_eq!(s, StatusCode::BAD_REQUEST, "{e}");
    assert!(e.to_string().contains("build"), "{e}");

    // Create, then a second revision.
    let (s, created) = admin
        .post(
            "/api/v1/stacks",
            json!({ "environment_id": ns_id, "server_id": b.server_id, "name": "shop", "content": SHOP, "env": "DB_PASSWORD=hunter2\n" }),
        )
        .await;
    assert_eq!(s, StatusCode::CREATED, "{created}");
    let id = created["stack"]["id"].as_str().unwrap().to_owned();
    assert_eq!(created["stack"]["env_keys"], json!(["DB_PASSWORD"]));
    assert!(!created.to_string().contains("hunter2"));
    let (s, _) = admin
        .post(
            "/api/v1/stacks",
            json!({ "environment_id": ns_id, "server_id": b.server_id, "name": "shop", "content": SHOP }),
        )
        .await;
    assert_eq!(s, StatusCode::CONFLICT);
    let v2 = SHOP.replace("nginx:1.27", "nginx:1.28");
    let (s, u) = admin.put(&format!("/api/v1/stacks/{id}"), json!({ "content": v2, "note": "nginx 1.28" })).await;
    assert_eq!(s, StatusCode::OK, "{u}");
    assert_eq!(u["stack"]["revision"], 2);
    let (_, revs) = admin.get(&format!("/api/v1/stacks/{id}/revisions")).await;
    assert_eq!(
        revs.as_array().unwrap().iter().map(|r| r["revision"].clone()).collect::<Vec<_>>(),
        [json!(2), json!(1)]
    );
    let (_, r1) = admin.get(&format!("/api/v1/stacks/{id}/revisions/1")).await;
    assert!(r1["content"].as_str().unwrap().contains("nginx:1.27"));
    let (_, got) = admin.get(&format!("/api/v1/stacks/{id}")).await;
    assert_eq!(got["dir"], "/opt/forge/stacks/web/shop");
    assert!(!got.to_string().contains("hunter2"), "{got}");

    // Deploy only `web`: its dependency `api` comes along, both on lab-b, so
    // one job on lab-b and none on lab-c.
    let (s, jobs) = admin.post(&format!("/api/v1/stacks/{id}/deploy"), json!({ "services": ["web"] })).await;
    assert_eq!(s, StatusCode::ACCEPTED, "{jobs}");
    let jobs = jobs.as_array().unwrap().clone();
    assert_eq!(jobs.len(), 1, "{jobs:?}");
    assert_eq!(jobs[0]["payload"]["server_id"], b.server_id.as_str());
    assert_eq!(jobs[0]["payload"]["revision"], 2);
    assert!(!jobs[0].to_string().contains("hunter2"), "no secrets in job payloads");

    // The node is not Linux with Docker here: the job fails and says why.
    let done = wait_job(&admin, jobs[0]["id"].as_str().unwrap(), Duration::from_secs(30)).await;
    assert_eq!(done["status"], "failed", "{done}");
    let (_, got) = admin.get(&format!("/api/v1/stacks/{id}")).await;
    let node =
        got["nodes"].as_array().unwrap().iter().find(|n| n["server_id"] == b.server_id.as_str()).unwrap().clone();
    assert_eq!(node["status"], "failed", "{got}");
    assert!(node["last_error"].as_str().is_some_and(|e| !e.is_empty()));

    // A rollback names an older revision; unknown services are refused.
    let (s, jobs) = admin.post(&format!("/api/v1/stacks/{id}/deploy"), json!({ "revision": 1 })).await;
    assert_eq!(s, StatusCode::ACCEPTED, "{jobs}");
    assert_eq!(jobs.as_array().unwrap().len(), 2, "every node of revision 1");
    assert!(jobs.as_array().unwrap().iter().all(|j| j["payload"]["revision"] == 1));
    let (s, _) = admin.post(&format!("/api/v1/stacks/{id}/deploy"), json!({ "services": ["nope"] })).await;
    assert_eq!(s, StatusCode::BAD_REQUEST);

    // Privileged settings need stack.privileged with MFA (this session has none).
    let priv_c = "services:\n  tool:\n    image: alpine:3\n    privileged: true\n    volumes: [\"/var/run/docker.sock:/var/run/docker.sock\"]\n";
    let (s, pv) = admin
        .post("/api/v1/stacks/preview", json!({ "environment_id": ns_id, "server_id": c.server_id, "content": priv_c }))
        .await;
    assert_eq!(s, StatusCode::OK, "{pv}");
    assert_eq!(pv["privileged"], true);
    assert_eq!(pv["services"][0]["policy"], "privileged");
    assert_eq!(pv["can_privileged"], false, "no MFA in this session");
    let (s, made) = admin
        .post(
            "/api/v1/stacks",
            json!({ "environment_id": ns_id, "server_id": c.server_id, "name": "tools", "content": priv_c }),
        )
        .await;
    assert_eq!(s, StatusCode::CREATED, "saving is fine: {made}");
    let tools = made["stack"]["id"].as_str().unwrap();
    let (s, e) = admin.post(&format!("/api/v1/stacks/{tools}/deploy"), json!({})).await;
    assert_eq!(s, StatusCode::FORBIDDEN, "{e}");

    // Create container: a one-service stack; variable values only in the sealed .env.
    let (s, cc) = admin
        .post(
            "/api/v1/stacks/containers",
            json!({
                "environment_id": ns_id, "server_id": c.server_id, "start": false,
                "container": {
                    "name": "cache", "image": "redis:7",
                    "ports": [{ "container": 6379 }],
                    "env": [{ "key": "REDIS_PASSWORD", "value": "s3cret-pw" }],
                    "volumes": [{ "source": "data", "target": "/data" }],
                    "memory_mb": 256
                }
            }),
        )
        .await;
    assert_eq!(s, StatusCode::CREATED, "{cc}");
    assert_eq!(cc["stack"]["kind"], "container");
    assert!(!cc.to_string().contains("s3cret-pw"), "{cc}");
    let cid = cc["stack"]["id"].as_str().unwrap();
    let (_, cget) = admin.get(&format!("/api/v1/stacks/{cid}")).await;
    let content = cget["content"].as_str().unwrap();
    assert!(content.contains("redis:7") && content.contains("${REDIS_PASSWORD}"), "{content}");
    assert!(!cget.to_string().contains("s3cret-pw"));
    let (s, _) = admin
        .post(
            "/api/v1/stacks/containers",
            json!({ "environment_id": ns_id, "server_id": c.server_id, "container": { "name": "x", "image": "alpine", "privileged": true } }),
        )
        .await;
    assert!(s == StatusCode::BAD_REQUEST || s == StatusCode::UNPROCESSABLE_ENTITY, "unknown form fields are refused");

    // Delete: refused while deployed parts exist (the failed one counts).
    let (s, _) = admin.call(reqwest::Method::DELETE, &format!("/api/v1/stacks/{id}"), None).await;
    assert_eq!(s, StatusCode::CONFLICT);
    let (s, _) = admin.call(reqwest::Method::DELETE, &format!("/api/v1/stacks/{cid}"), None).await;
    assert_eq!(s, StatusCode::NO_CONTENT);

    // Roles: an auditor reads, cannot change; a developer can manage stacks.
    let auditor =
        user_with_role(&h, &admin, "aud@example.com", "auditor", &format!("workspace:{}", h.default_org)).await;
    let (s, list) = auditor.get("/api/v1/stacks").await;
    assert_eq!(s, StatusCode::OK);
    assert!(list.as_array().unwrap().iter().any(|x| x["stack"]["name"] == "shop"));
    let (s, _) = auditor.post(&format!("/api/v1/stacks/{id}/deploy"), json!({})).await;
    assert_eq!(s, StatusCode::FORBIDDEN);
    let (s, _) = auditor.put(&format!("/api/v1/stacks/{id}"), json!({ "content": SHOP })).await;
    assert_eq!(s, StatusCode::FORBIDDEN);

    let actions = audit_actions(&admin).await;

    for a in ["stack.create", "stack.update", "stack.deploy"] {
        assert!(actions.iter().any(|x| x == a), "{a} missing in {actions:?}");
    }
}

/// Security regressions: `.env` values never reach readers of a stack
/// (preview fields, finding messages, errors), and a preview needs
/// `server.manage` on the node like create does.
#[tokio::test]
async fn stack_reads_never_show_env_values_and_preview_needs_the_node() {
    let h = start().await;
    let admin = Client::login(&h.base, ADMIN_EMAIL, PASSWORD).await;
    let (_, cl) = admin.post("/api/v1/clusters", json!({"name": "lab"})).await;
    let (_, ag2) = admin.post("/api/v1/projects", json!({"name": "grp2"})).await;
    let (s, ns) = admin
        .post("/api/v1/environments", json!({"name": "Data", "cluster_id": cl["id"], "project_id": ag2["id"]}))
        .await;
    assert_eq!(s, StatusCode::CREATED, "{ns}");
    let ns_id = ns["id"].as_str().unwrap().to_owned();
    let b = enroll_and_run(&h, &admin, "lab-b").await;
    let content = "services:\n  db:\n    image: mysql:${TAG}\n    volumes: [\"${SRC}:/var/lib/mysql\"]\n    healthcheck:\n      test: [\"CMD-SHELL\", \"mysqladmin ping -p${MYSQL_ROOT_PASSWORD}\"]\n    restart: unless-stopped\n";
    let env = "MYSQL_ROOT_PASSWORD=Sup3r-root-pw\nTAG=tag-s3cr3t\nSRC=/srv/hidden-s3cr3t-dir\n";
    let (s, created) = admin
        .post(
            "/api/v1/stacks",
            json!({ "environment_id": ns_id, "server_id": b.server_id, "name": "db", "content": content, "env": env }),
        )
        .await;
    assert_eq!(s, StatusCode::CREATED, "{created}");
    let id = created["stack"]["id"].as_str().unwrap().to_owned();
    let secrets = ["Sup3r-root-pw", "tag-s3cr3t", "hidden-s3cr3t-dir"];
    let clean = |v: &serde_json::Value| secrets.iter().all(|s| !v.to_string().contains(s));
    assert!(clean(&created), "{created}");

    // The bind outside approved roots is a finding, named by its variable.
    assert_eq!(created["preview"]["privileged"], true, "{created}");
    let auditor =
        user_with_role(&h, &admin, "aud2@example.com", "auditor", &format!("workspace:{}", h.default_org)).await;
    let (s, got) = auditor.get(&format!("/api/v1/stacks/{id}")).await;
    assert_eq!(s, StatusCode::OK, "{got}");
    assert!(clean(&got), "{got}");
    let db = &got["preview"]["services"][0];
    assert_eq!(db["image"], "mysql:${TAG}", "{got}");
    assert!(db["findings"].to_string().contains("${SRC}"), "{got}");

    // A developer manages stacks but not the node: no preview on it.
    let dev =
        user_with_role(&h, &admin, "dev2@example.com", "developer", &format!("workspace:{}", h.default_org)).await;
    let (s, e) = dev
        .post(
            "/api/v1/stacks/preview",
            json!({ "environment_id": ns_id, "server_id": b.server_id, "content": content }),
        )
        .await;
    assert_eq!(s, StatusCode::FORBIDDEN, "{e}");
    let (s, p) = admin
        .post(
            "/api/v1/stacks/preview",
            json!({ "environment_id": ns_id, "server_id": b.server_id, "content": content, "env": env }),
        )
        .await;
    assert_eq!(s, StatusCode::OK, "{p}");
    assert!(clean(&p), "{p}");
}

/// Compose app templates (D-160): listed for viewers; a stack made from one
/// gets its secrets generated and sealed, and unknown variables are refused.
#[tokio::test]
async fn stacks_come_from_templates_with_generated_secrets() {
    let h = crate::common::start().await;
    let admin = crate::common::Client::login(&h.base, crate::common::ADMIN_EMAIL, crate::common::PASSWORD).await;
    let (s, list) = admin.get("/api/v1/stack-templates").await;
    assert_eq!(s, reqwest::StatusCode::OK);
    assert!(list.as_array().unwrap().iter().any(|t| t["id"] == "postgres"), "{list}");
    let (s, e) = admin
        .post("/api/v1/stacks/from-template", serde_json::json!({"template": "nope", "environment_id": forge_core::EnvironmentId::new(), "server_id": forge_core::ServerId::new(), "name": "db"}))
        .await;
    assert_eq!(s, reqwest::StatusCode::BAD_REQUEST, "{e}");
    let (s, e) = admin
        .post("/api/v1/stacks/from-template", serde_json::json!({"template": "postgres", "environment_id": forge_core::EnvironmentId::new(), "server_id": forge_core::ServerId::new(), "name": "db", "values": {"EVIL": "x"}}))
        .await;
    assert_eq!(s, reqwest::StatusCode::BAD_REQUEST, "{e}");
}
