//! `forge.stack.yaml` through `POST /stacks/plan` and `POST /stacks/apply`
//! (`plans/native-mesh.md` 3, D-174): field-path errors, the plan's
//! findings for a cluster without a network, the rollout that stops after
//! a failed node, the `?stack=` names filter, and the roles that may use it.
//! Running Compose itself needs a Linux node with Docker (lab test): here
//! every node's deploy fails, which is what the rollout test needs.
#![allow(clippy::unwrap_used)] // reason: tests

use std::time::Duration;

use forge_agent::enroll::{HostFacts, enroll};
use forge_agent::{Agent, AgentConfig, StatePaths};
use reqwest::StatusCode;
use serde_json::{Value, json};

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

const COMPOSE: &str = "services:\n  web:\n    image: nginx:1.27\n    ports: [\"8080:80\"]\n    x-forge: { node: lab-b }\n  worker:\n    image: ghcr.io/example/worker:1.0\n    x-forge: { node: lab-c }\n";

fn document(extra: &str) -> String {
    format!(
        "apiVersion: forge/v1\nkind: Stack\nmetadata:\n  name: orders\n  environment: web\nspec:\n  compose:\n    file: compose.yaml\n  placement:\n    nodes: [lab-b, lab-c]\n{extra}"
    )
}

/// An environment `Web` (slug `web`) in a cluster without a network.
async fn environment(admin: &Client) -> String {
    let (_, cl) = admin.post("/api/v1/clusters", json!({"name": "lab"})).await;
    let (_, pr) = admin.post("/api/v1/projects", json!({"name": "shop"})).await;
    let (s, ns) = admin
        .post("/api/v1/environments", json!({"name": "Web", "cluster_id": cl["id"], "project_id": pr["id"]}))
        .await;
    assert_eq!(s, StatusCode::CREATED, "{ns}");

    ns["id"].as_str().unwrap().to_owned()
}

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
async fn stack_documents_plan_apply_and_stop_after_a_failed_node() {
    let h = start().await;
    let admin = Client::login(&h.base, ADMIN_EMAIL, PASSWORD).await;
    let ns_id = environment(&admin).await;
    let b = enroll_and_run(&h, &admin, "lab-b").await;
    let c = enroll_and_run(&h, &admin, "lab-c").await;
    let body = |doc: String, compose: &str| json!({ "document": doc, "compose": compose });

    // Refusals name the field.
    let (s, e) = admin.post("/api/v1/stacks/plan", body(document("  speed: 2\n"), COMPOSE)).await;
    assert_eq!(s, StatusCode::BAD_REQUEST, "{e}");
    assert!(e.to_string().contains("spec.speed"), "{e}");
    let plain = "services:\n  web:\n    image: nginx\n";
    let (s, e) = admin.post("/api/v1/stacks/plan", body(document(""), plain)).await;
    assert_eq!(s, StatusCode::BAD_REQUEST, "{e}");
    assert!(e.to_string().contains("placement.nodes lists several nodes"), "{e}");

    // The plan: a new stack, both nodes in order, and the network finding.
    let (s, p) = admin.post("/api/v1/stacks/plan", body(document(""), COMPOSE)).await;
    assert_eq!(s, StatusCode::OK, "{p}");
    assert_eq!(p["action"], "create");
    assert_eq!(p["revision_next"], 1);
    assert_eq!(p["environment_id"], ns_id.as_str());
    let order: Vec<&Value> = p["nodes"].as_array().unwrap().iter().map(|n| &n["server_id"]).collect();
    assert_eq!(order, [&json!(b.server_id), &json!(c.server_id)]);
    assert_eq!(p["mesh"]["enabled"], true);
    assert_eq!(p["mesh"]["nodes_in_sync"], json!([]));
    assert_eq!(p["mesh"]["nodes_not_in_sync"], json!(["lab-b", "lab-c"]));
    let no_fabric = p["findings"]
        .as_array()
        .unwrap()
        .iter()
        .find(|f| f["message"].as_str().is_some_and(|m| m.starts_with("services are reachable from other nodes only")))
        .cloned();
    assert_eq!(no_fabric.unwrap()["severity"], "info", "{p}");
    let (_, list) = admin.get("/api/v1/stacks").await;
    assert_eq!(list, json!([]), "a plan saves nothing");

    // Apply: the stack is created and the rollout stops at the first node.
    let (s, a) = admin.post("/api/v1/stacks/apply", body(document(""), COMPOSE)).await;
    assert_eq!(s, StatusCode::ACCEPTED, "{a}");
    assert_eq!(a["revision"], 1);
    let id = a["stack_id"].as_str().unwrap().to_owned();
    let done = wait_job(&admin, a["job_id"].as_str().unwrap(), Duration::from_secs(120)).await;
    assert_eq!(done["status"], "failed", "{done}");
    let why = done["last_error"].as_str().unwrap_or_default();
    assert!(why.contains("stopped at node 1 of 2") && why.contains("1 later node(s) were not deployed"), "{done}");
    let (_, got) = admin.get(&format!("/api/v1/stacks/{id}")).await;
    let nodes = got["nodes"].as_array().unwrap();
    assert_eq!(nodes.len(), 1, "lab-c was never started: {got}");
    assert_eq!(nodes[0]["server_id"], b.server_id.as_str());
    assert_eq!(nodes[0]["status"], "failed");
    let (_, revs) = admin.get(&format!("/api/v1/stacks/{id}/revisions")).await;
    assert!(revs[0]["note"].as_str().unwrap().starts_with("apply "), "{revs}");

    // A second plan updates; the content is unchanged so the revision stays.
    let (s, p) = admin.post("/api/v1/stacks/plan", body(document(""), COMPOSE)).await;
    assert_eq!(s, StatusCode::OK, "{p}");
    assert_eq!(p["action"], "update");
    assert_eq!(p["revision_next"], 1);

    // Names of the stack: none without the cluster network.
    let (s, n) = admin.get(&format!("/api/v1/names?stack={id}")).await;
    assert_eq!(s, StatusCode::OK, "{n}");
    assert_eq!(n["names"], json!([]));

    let actions = audit_actions(&admin).await;

    for x in ["stack.plan", "stack.create", "stack.deploy"] {
        assert!(actions.iter().any(|a| a == x), "{x} missing in {actions:?}");
    }
}

/// `stack.manage` (with `server.manage` on the nodes, as the stack editor
/// needs) is enough: an operator plans and applies without
/// `workspace.manage`; an auditor cannot.
#[tokio::test]
async fn stack_documents_need_stack_manage_only() {
    let h = start().await;
    let admin = Client::login(&h.base, ADMIN_EMAIL, PASSWORD).await;
    environment(&admin).await;
    let _b = enroll_and_run(&h, &admin, "lab-b").await;
    let _c = enroll_and_run(&h, &admin, "lab-c").await;
    let scope = format!("workspace:{}", h.default_org);
    let doc = document("  rollout:\n    order: all-at-once\n    healthTimeout: 30s\n");
    let body = json!({ "document": doc, "compose": COMPOSE });

    let auditor = user_with_role(&h, &admin, "aud@example.com", "auditor", &scope).await;
    let (s, e) = auditor.post("/api/v1/stacks/plan", body.clone()).await;
    assert_eq!(s, StatusCode::FORBIDDEN, "{e}");
    let (s, _) = auditor.post("/api/v1/stacks/apply", body.clone()).await;
    assert_eq!(s, StatusCode::FORBIDDEN);

    let operator = user_with_role(&h, &admin, "ops@example.com", "operator", &scope).await;
    let (s, p) = operator.post("/api/v1/stacks/plan", body.clone()).await;
    assert_eq!(s, StatusCode::OK, "{p}");
    assert_eq!(p["order"], "all-at-once");
    assert_eq!(p["health_timeout_s"], 30);
    let (s, a) = operator.post("/api/v1/stacks/apply", body).await;
    assert_eq!(s, StatusCode::ACCEPTED, "{a}");
    let done = wait_job(&operator, a["job_id"].as_str().unwrap(), Duration::from_secs(120)).await;
    assert_eq!(done["status"], "failed", "no Docker on the test nodes: {done}");
    assert_eq!(done["kind"], "stack.rollout");
}
