//! Deploy webhooks (D-159): created with MFA, the secret URL shown once,
//! signatures checked, tags gated by a pattern, every delivery recorded, and
//! a delivery deploys as its creator.
#![allow(clippy::unwrap_used)]

use crate::common::{ADMIN_EMAIL, Client, PASSWORD, start, step_up};
use forge_agent::AgentConfig;
use forge_agent::StatePaths;
use forge_agent::enroll::{HostFacts, enroll};
use reqwest::StatusCode;
use serde_json::{Value, json};

fn hmac(key: &str, body: &[u8]) -> String {
    let k = ring::hmac::Key::new(ring::hmac::HMAC_SHA256, key.as_bytes());
    hex::encode(ring::hmac::sign(&k, body).as_ref())
}

async fn post(base: &str, path: &str, headers: &[(&str, String)], body: &str) -> (StatusCode, Value) {
    let mut r = reqwest::Client::new().post(format!("{base}{path}")).body(body.to_owned());
    for (k, v) in headers {
        r = r.header(*k, v);
    }
    let resp = r.send().await.unwrap();
    let s = resp.status();
    (s, resp.json().await.unwrap_or(Value::Null))
}

#[tokio::test]
async fn webhooks_deploy_as_their_creator_with_checked_signatures() {
    let h = start().await;
    let mut admin = Client::login(&h.base, ADMIN_EMAIL, PASSWORD).await;
    let (_, cl) = admin.post("/api/v1/clusters", json!({"name": "dev"})).await;
    let (_, shop) = admin.post("/api/v1/projects", json!({"name": "shop"})).await;
    let (_, env) = admin
        .post("/api/v1/environments", json!({"name": "prod", "cluster_id": cl["id"], "project_id": shop["id"]}))
        .await;
    let (s, web) = admin
        .post(
            "/api/v1/services",
            json!({"environment_id": env["id"], "name": "web", "runtime": "container", "image": "nginx:1.27"}),
        )
        .await;
    assert_eq!(s, StatusCode::CREATED, "{web}");
    let sid = web["id"].as_str().unwrap().to_owned();
    let (_, tok) = admin.post("/api/v1/enrollment-tokens", json!({"ttl_minutes": 10})).await;
    let dir = tempfile::tempdir().unwrap();
    let facts = HostFacts { hostname: "lab-a".into(), os: "linux".into(), arch: "x86_64".into() };
    let node = enroll(
        &StatePaths::new(dir.path()),
        AgentConfig::new(h.agent_url.clone()),
        tok["token"].as_str().unwrap(),
        facts,
    )
    .await
    .unwrap()
    .server_id;
    let (s, e) = admin.put(&format!("/api/v1/services/{sid}/instances"), json!([{"server_id": node}])).await;
    assert!(s.is_success(), "{e}");

    let body =
        json!({"name": "ci", "target": {"kind": "service", "id": sid}, "signature": "github", "tag_pattern": "v*"});
    let (s, _) = admin.post("/api/v1/webhooks", body.clone()).await;
    assert_eq!(s, StatusCode::FORBIDDEN, "MFA first");
    step_up(&mut admin).await;
    let unsigned =
        json!({"name": "open", "target": {"kind": "service", "id": sid}, "signature": "none", "tag_pattern": "*"});
    let (s, _) = admin.post("/api/v1/webhooks", unsigned).await;
    assert_eq!(s, StatusCode::UNPROCESSABLE_ENTITY, "changing the tag needs a signature");
    let (s, minted) = admin.post("/api/v1/webhooks", body).await;
    assert_eq!(s, StatusCode::CREATED, "{minted}");
    let token = minted["token"].as_str().unwrap().to_owned();
    let secret = minted["secret"].as_str().unwrap().to_owned();
    assert!(token.starts_with("whk_"));
    let (_, list) = admin.get("/api/v1/webhooks").await;
    assert!(!list.to_string().contains(&token) && !list.to_string().contains(&secret), "shown once");

    // Unknown URL, missing or wrong signature, refused tags.
    let (s, _) = post(&h.base, "/hooks/whk_nope", &[], "{}").await;
    assert_eq!(s, StatusCode::NOT_FOUND);
    let path = format!("/hooks/{token}");
    let (s, _) = post(&h.base, &path, &[], "{}").await;
    assert_eq!(s, StatusCode::UNAUTHORIZED);
    let (s, _) =
        post(&h.base, &path, &[("x-hub-signature-256", format!("sha256={}", hmac("wrong", b"{}")))], "{}").await;
    assert_eq!(s, StatusCode::UNAUTHORIZED);
    let tagged = r#"{"tag":"latest"}"#;
    let (s, _) = post(
        &h.base,
        &path,
        &[("x-hub-signature-256", format!("sha256={}", hmac(&secret, tagged.as_bytes())))],
        tagged,
    )
    .await;
    assert_eq!(s, StatusCode::UNPROCESSABLE_ENTITY, "latest does not match v*");

    // A signed delivery with an allowed tag deploys the new tag.
    let good = r#"{"tag":"v2.0.1"}"#;
    let signed = vec![
        ("x-hub-signature-256", format!("sha256={}", hmac(&secret, good.as_bytes()))),
        ("x-github-delivery", "delivery-1".to_owned()),
    ];
    let (s, out) = post(&h.base, &path, &signed, good).await;
    assert_eq!(s, StatusCode::ACCEPTED, "{out}");
    let (s, _) = post(&h.base, &path, &signed, good).await;
    assert_eq!(s, StatusCode::CONFLICT, "a replayed delivery is refused");
    let (_, svc) = admin.get(&format!("/api/v1/services/{sid}")).await;
    assert_eq!(svc["spec"]["image"], "nginx:v2.0.1", "{svc}");
    let status_url = out["status_url"].as_str().unwrap().to_owned();
    let resp = reqwest::get(format!("{}{status_url}", h.base)).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);

    // Every delivery is recorded; rotation kills the old URL.
    let wid = minted["webhook"]["id"].as_str().unwrap();
    let (_, deliveries) = admin.get(&format!("/api/v1/webhooks/{wid}/deliveries")).await;
    let outcomes: Vec<&str> = deliveries.as_array().unwrap().iter().filter_map(|d| d["outcome"].as_str()).collect();
    assert!(outcomes.contains(&"accepted") && outcomes.contains(&"rejected"), "{deliveries}");
    let (s, rotated) = admin.post(&format!("/api/v1/webhooks/{wid}/rotate"), json!({})).await;
    assert_eq!(s, StatusCode::OK, "{rotated}");
    let (s, _) = post(&h.base, &path, &[], "{}").await;
    assert_eq!(s, StatusCode::NOT_FOUND, "the old URL is gone");

    let (_, audit) = admin.get("/api/v1/audit?limit=300").await;
    assert!(audit.to_string().contains("webhook.deliver"), "{audit}");
    assert!(!audit.to_string().contains(&secret));
}
