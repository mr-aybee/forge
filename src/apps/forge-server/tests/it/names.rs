//! Local names (D-084): labels, renames, the names list and the signed
//! `names` command reaching every node. The agents here refuse that command
//! (it is left out of their allow-list), so this machine's hosts file is
//! never written; the refusal proves the command was sent and signed.
#![allow(clippy::unwrap_used)]

use std::time::Duration;

use crate::common::*;
use forge_agent::enroll::{HostFacts, enroll};
use forge_agent::{Agent, AgentConfig, StatePaths};
use reqwest::StatusCode;
use serde_json::{Value, json};

struct Node {
    id: String,
    _stop: tokio::sync::watch::Sender<bool>,
    _dir: tempfile::TempDir,
}

async fn node(h: &Harness, admin: &Client, hostname: &str) -> Node {
    let (s, body) = admin.post("/api/v1/enrollment-tokens", json!({"ttl_minutes": 10})).await;
    assert_eq!(s, StatusCode::CREATED, "{body}");
    let dir = tempfile::tempdir().unwrap();
    let paths = StatePaths::new(dir.path());
    let mut config = AgentConfig::new(h.agent_url.clone());
    // Everything except `names`: never touch this machine's hosts file.
    config.allowed_commands.retain(|c| c != "names");
    let facts = HostFacts { hostname: hostname.into(), os: "linux".into(), arch: "x86_64".into() };
    let id = enroll(&paths, config, body["token"].as_str().unwrap(), facts).await.unwrap();
    let agent = Agent::load(paths).unwrap();
    let (stop, rx) = tokio::sync::watch::channel(false);
    tokio::spawn(agent.run(rx));
    Node { id: id.server_id, _stop: stop, _dir: dir }
}

async fn names(c: &Client) -> Value {
    let (s, v) = c.get("/api/v1/names").await;
    assert_eq!(s, StatusCode::OK, "{v}");
    v
}

fn node_row<'a>(v: &'a Value, id: &str) -> &'a Value {
    v["nodes"].as_array().unwrap().iter().find(|n| n["server_id"] == id).unwrap()
}

#[tokio::test]
async fn names_follow_renames_and_reach_every_node() {
    let h = start_with(|c| c.names.enabled = true).await;
    let admin = Client::login(&h.base, ADMIN_EMAIL, PASSWORD).await;
    let a = node(&h, &admin, "web-01.example.com").await;
    let b = node(&h, &admin, "WEB-01").await;

    // Derived labels never collide; names end in the suffix.
    let v = names(&admin).await;
    assert_eq!(v["suffix"], "local");
    let (la, lb) = (node_row(&v, &a.id)["label"].clone(), node_row(&v, &b.id)["label"].clone());
    assert_ne!(la, lb, "{v}");
    assert!([&la, &lb].contains(&&json!("web-01")), "{v}");
    assert!(v["names"].as_array().unwrap().iter().all(|n| n["name"].as_str().unwrap().ends_with(".local")));

    // The signed command reaches both nodes (and they refuse it here).
    for n in [&a, &b] {
        let mut last = Value::Null;
        for _ in 0..150 {
            last = node_row(&names(&admin).await, &n.id)["sync"].clone();
            if last["error"].as_str().is_some_and(|e| e.contains("`names` is not allowed")) {
                break;
            }
            tokio::time::sleep(Duration::from_millis(200)).await;
        }
        assert!(last["error"].as_str().unwrap_or_default().contains("`names` is not allowed"), "{last}");
    }

    // Rename b: validated, unique in the workspace, audited.
    let url = |n: &Node| format!("/api/v1/servers/{}/dns-name", n.id);
    for bad in ["Bad_Name", "localhost", "-x", "a.b", "a b", "x\n10.0.0.1 evil", &"a".repeat(64)] {
        let (s, body) = admin.put(&url(&b), json!({ "label": bad })).await;
        assert_eq!(s, StatusCode::BAD_REQUEST, "{bad:?}: {body}");
    }
    // Review of D-084: b cannot take the name a derives.
    let first = if la == "web-01" { &b } else { &a };
    let (s, body) = admin.put(&url(first), json!({ "label": "web-01" })).await;
    assert_eq!(s, StatusCode::CONFLICT, "{body}");
    assert!(body.to_string().contains("in use"), "{body}");
    let (s, body) = admin.put(&url(&b), json!({ "label": "db" })).await;
    assert_eq!(s, StatusCode::OK, "{body}");
    assert_eq!(body["name"], "db.local");
    let (s, body) = admin.put(&url(&a), json!({ "label": "db" })).await;
    assert_eq!(s, StatusCode::CONFLICT, "{body}");
    let v = names(&admin).await;
    assert_eq!(node_row(&v, &b.id)["label_set"], true);
    assert!(v["names"].as_array().unwrap().iter().any(|n| n["name"] == "db.local" && n["server_id"] == b.id));
    let (_, node) = admin.get(&format!("/api/v1/servers/{}", b.id)).await;
    assert_eq!(node["dns"]["name"], "db.local", "{node}");

    // Review of D-084: labels are sticky. A console rename keeps a's label.
    let (s, _) = admin.put(&format!("/api/v1/servers/{}/name", a.id), json!({ "name": "Front Door" })).await;
    assert_eq!(s, StatusCode::OK);
    assert_eq!(node_row(&names(&admin).await, &a.id)["label"], la);
    // Renaming a away leaves b alone, and a's old name stays free.
    let (s, body) = admin.put(&url(&a), json!({ "label": "front" })).await;
    assert_eq!(s, StatusCode::OK, "{body}");
    let v = names(&admin).await;
    assert_eq!(node_row(&v, &b.id)["label"], "db");
    assert!(v["nodes"].as_array().unwrap().iter().all(|n| n["label"] != la), "{v}");

    // Clearing goes back to the derived label.
    let (s, body) = admin.put(&url(&b), json!({ "label": "" })).await;
    assert_eq!(s, StatusCode::OK, "{body}");
    assert_eq!(body["label_set"], false);

    // A revoked node has no name any more.
    let (s, body) = admin
        .post(&format!("/api/v1/servers/{}/revoke", a.id), json!({ "confirm_hostname": "web-01.example.com" }))
        .await;
    assert!(s.is_success(), "{s} {body}");
    let v = names(&admin).await;
    assert!(v["names"].as_array().unwrap().iter().all(|n| n["server_id"] != a.id), "{v}");
    assert!(v["names"].as_array().unwrap().iter().any(|n| n["server_id"] == b.id));

    let (_, audit) = admin.get("/api/v1/audit?limit=100").await;
    let text = serde_json::to_string(&audit).unwrap();
    assert!(text.contains("server.dns_rename") && text.contains("\"to\":\"db\""), "renames are audited");
    h.app.stop().await;
}

/// The suffix is a runtime setting: platform admins change it (with MFA),
/// it is validated, audited with the old and new value, and every name
/// follows. Others are refused.
#[tokio::test]
async fn the_suffix_is_a_validated_audited_runtime_setting() {
    let h = start_with(|c| c.names.enabled = true).await;
    let mut admin = Client::login(&h.base, ADMIN_EMAIL, PASSWORD).await;
    let a = node(&h, &admin, "lab-b").await;

    let (s, v) = admin.get("/api/v1/names/suffix").await;
    assert_eq!(s, StatusCode::OK, "{v}");
    assert_eq!((v["suffix"].as_str(), v["source"].as_str()), (Some("local"), Some("default")), "{v}");

    // Without MFA: refused and audited.
    let (s, e) = admin.put("/api/v1/names/suffix", json!({ "suffix": "home.arpa" })).await;
    assert_eq!((s, e["title"].as_str()), (StatusCode::FORBIDDEN, Some("step_up_required")), "{e}");
    // The generic settings editor cannot be used around it.
    let (s, _) = admin.put("/api/v1/settings/names.suffix", json!({ "value": "com" })).await;
    assert_eq!(s, StatusCode::BAD_REQUEST);

    step_up(&mut admin).await;
    for bad in ["com", "net", "org", "Home.Arpa", "a..b", "x.localhost", "-a.internal", "lab_1.internal"] {
        let (s, body) = admin.put("/api/v1/names/suffix", json!({ "suffix": bad })).await;
        assert_eq!(s, StatusCode::BAD_REQUEST, "{bad:?}: {body}");
    }
    let (s, v) = admin.put("/api/v1/names/suffix", json!({ "suffix": "home.arpa" })).await;
    assert_eq!(s, StatusCode::OK, "{v}");
    assert_eq!((v["suffix"].as_str(), v["source"].as_str()), (Some("home.arpa"), Some("setting")), "{v}");

    // Every name follows.
    let v = names(&admin).await;
    assert_eq!((v["suffix"].as_str(), v["suffix_source"].as_str()), (Some("home.arpa"), Some("setting")), "{v}");
    assert_eq!(node_row(&v, &a.id)["name"], "lab-b.home.arpa", "{v}");
    assert!(v["names"].as_array().unwrap().iter().all(|n| n["name"].as_str().unwrap().ends_with(".home.arpa")));

    // Audited with the old and new value (and the refusals).
    let (_, log) = admin.get("/api/v1/audit?action=names.suffix.update").await;
    let log = log.as_array().unwrap();
    let ok = log.iter().find(|e| e["outcome"] == "success").unwrap();
    assert_eq!(
        (ok["details"]["from"].as_str(), ok["details"]["to"].as_str()),
        (Some("local"), Some("home.arpa")),
        "{ok}"
    );
    assert!(log.iter().any(|e| e["outcome"] == "denied"), "{log:?}");

    // A workspace operator may read it but not change it.
    let mut op =
        user_with_role(&h, &admin, "op@example.com", "operator", &format!("workspace:{}", h.default_org)).await;
    let (s, v) = op.get("/api/v1/names/suffix").await;
    assert_eq!((s, v["suffix"].as_str()), (StatusCode::OK, Some("home.arpa")), "{v}");
    step_up(&mut op).await;
    let (s, e) = op.put("/api/v1/names/suffix", json!({ "suffix": "corp.internal" })).await;
    assert_eq!(s, StatusCode::FORBIDDEN, "{e}");

    // Empty clears the setting: back to the config file's value.
    let (s, v) = admin.put("/api/v1/names/suffix", json!({ "suffix": "" })).await;
    assert_eq!(
        (s, v["suffix"].as_str(), v["source"].as_str()),
        (StatusCode::OK, Some("local"), Some("default")),
        "{v}"
    );
}
