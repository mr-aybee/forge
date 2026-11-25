//! Ingress endpoints (D-169): placement, path rules, static roots, basic
//! auth and the custom configuration check.
#![allow(clippy::unwrap_used)]

use crate::common::{ADMIN_EMAIL, Client, Harness, PASSWORD, start};
use forge_core::model::{AgentStatus, NodeRole, Server};
use reqwest::StatusCode;
use serde_json::{Value, json};

async fn add_node(h: &Harness, hostname: &str) -> Server {
    let n = Server {
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
        status: AgentStatus::Online,
        last_seen_at: None,
        cert_serial: None,
        cert_not_after: None,
        created_at: forge_core::Timestamp(0),
    };
    forge_core::ports::ServerRepository::create_server(&h.app.state.store, &n).await.unwrap();
    n
}

fn on_node(name: &str, edge: &Server, extra: Value) -> Value {
    let mut v = json!({ "name": name, "domains": [format!("{name}.example.com")], "upstreams": ["10.0.0.5:8000"],
                        "placement": { "kind": "nodes", "server_ids": [edge.id] } });
    for (k, x) in extra.as_object().unwrap() {
        v[k] = x.clone();
    }
    v
}

#[tokio::test]
async fn placement_is_explicit_and_errors_say_what_is_missing() {
    let h = start().await;
    let admin = Client::login(&h.base, ADMIN_EMAIL, PASSWORD).await;
    let edge = add_node(&h, "edge-1").await;
    let (s, b) = admin.post("/api/v1/sites", on_node("one", &edge, json!({}))).await;
    assert_eq!(s, StatusCode::CREATED, "{b}");
    assert_eq!(b["config"]["placement"], json!({ "kind": "nodes", "server_ids": [edge.id] }));
    assert_eq!(b["config"]["edges"], json!([edge.id]), "older readers still see the edges");
    // No node chosen.
    let (s, e) = admin
        .post("/api/v1/sites", on_node("none", &edge, json!({ "placement": { "kind": "nodes", "server_ids": [] } })))
        .await;
    assert_eq!(s, StatusCode::BAD_REQUEST);
    assert!(e.to_string().contains("choose the node"), "{e}");
    // A cluster without an ingress profile.
    let (_, c) = admin.post("/api/v1/clusters", json!({ "name": "lab" })).await;
    let (s, e) = admin
        .post(
            "/api/v1/sites",
            on_node("cl", &edge, json!({ "placement": { "kind": "cluster", "cluster_id": c["id"] } })),
        )
        .await;
    assert_eq!(s, StatusCode::BAD_REQUEST);
    assert!(e.to_string().contains("ingress profile"), "{e}");
    // Older clients sending `edges` still work, and get a placement.
    let (s, b) = admin
        .post("/api/v1/sites", json!({ "name": "legacy", "domains": ["legacy.example.com"], "upstreams": ["10.0.0.5:80"], "edges": [edge.id] }))
        .await;
    assert_eq!(s, StatusCode::CREATED, "{b}");
    assert_eq!(b["config"]["placement"]["kind"], "nodes");
}

#[tokio::test]
async fn old_rows_still_read_and_preview() {
    use forge_core::ports::SiteRepository;
    let h = start().await;
    let admin = Client::login(&h.base, ADMIN_EMAIL, PASSWORD).await;
    let edge = add_node(&h, "edge-1").await;
    let old = forge_core::model::Site {
        id: "old-site".into(),
        workspace_id: h.default_org.parse().unwrap(),
        name: "old".into(),
        config: json!({ "spec": { "name": "old", "domains": ["old.example.com"], "upstreams": ["10.0.0.9:80"], "custom": "" },
                        "edges": [edge.id], "cluster_id": null, "targets": [], "standby_targets": [], "dns": null }),
        edge_status: json!({}),
        created_at: forge_core::Timestamp(0),
        updated_at: forge_core::Timestamp(0),
    };
    h.app.state.store.put_site(&old).await.unwrap();
    let (s, b) = admin.get("/api/v1/sites/old-site").await;
    assert_eq!(s, StatusCode::OK, "{b}");
    let mut body = b["config"]["spec"].clone();
    body["edges"] = json!([edge.id]);
    let (s, p) = admin.post("/api/v1/sites/preview", body.clone()).await;
    assert_eq!(s, StatusCode::OK, "{p}");
    assert!(p["config"].as_str().unwrap().contains("upstream forge_old {"));
    let (s, b) = admin.put("/api/v1/sites/old-site", body).await;
    assert_eq!(s, StatusCode::OK, "{b}");
}

#[tokio::test]
async fn preview_renders_each_type_and_reports_custom_errors_by_line() {
    let h = start().await;
    let admin = Client::login(&h.base, ADMIN_EMAIL, PASSWORD).await;
    let edge = add_node(&h, "edge-1").await;
    let redirect = on_node(
        "r",
        &edge,
        json!({ "upstreams": [], "kind": "redirect", "redirect": { "to": "https://example.com", "code": 308 } }),
    );
    let (s, p) = admin.post("/api/v1/sites/preview", redirect).await;
    assert_eq!(s, StatusCode::OK, "{p}");
    assert!(p["config"].as_str().unwrap().contains("return 308 https://example.com$request_uri;"));
    let lb = on_node(
        "lb",
        &edge,
        json!({ "kind": "balancer", "upstreams": ["10.0.0.5:80", "10.0.0.6:80"],
        "rules": [{ "path": "/api/", "targets": [{ "kind": "external", "address": "10.0.0.7:9000" }], "upstreams": [] }] }),
    );
    let (s, p) = admin.post("/api/v1/sites/preview", lb).await;
    assert_eq!(s, StatusCode::OK, "{p}");
    assert!(p["config"].as_str().unwrap().contains("upstream forge_lb_r1 {\n    server 10.0.0.7:9000"), "{p}");
    let (s, p) = admin
        .post("/api/v1/sites/preview", on_node("c", &edge, json!({ "custom": "add_header X 1;\nroot /etc;" })))
        .await;
    assert_eq!(s, StatusCode::OK, "{p}");
    assert_eq!(p["custom_error"]["line"], 2);
    assert_eq!(p["custom_error"]["directive"], "root");
    // A bad redirect URL is refused outright.
    let bad = on_node(
        "b",
        &edge,
        json!({ "upstreams": [], "kind": "redirect", "redirect": { "to": "https://x.com/$host" } }),
    );
    assert_eq!(admin.post("/api/v1/sites/preview", bad).await.0, StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn static_directories_must_be_inside_an_approved_root_on_every_edge() {
    use forge_core::files::{ApprovedRoot, FilePath, FileRepository, RootKind, RootSpec};
    let h = start().await;
    let admin = Client::login(&h.base, ADMIN_EMAIL, PASSWORD).await;
    let edge = add_node(&h, "edge-1").await;
    let site = |root: &str| {
        on_node("st", &edge, json!({ "upstreams": [], "kind": "static", "static_root": { "root": root } }))
    };
    let (s, e) = admin.post("/api/v1/sites", site("/srv/www/shop")).await;
    assert_eq!(s, StatusCode::UNPROCESSABLE_ENTITY, "{e}");
    assert!(e.to_string().contains("approved file root"), "{e}");
    h.app
        .state
        .store
        .put_root(&ApprovedRoot {
            id: "fr_000000000000000000000001".into(),
            workspace_id: h.default_org.parse().unwrap(),
            server_id: edge.id,
            kind: RootKind::Service,
            label: "www".into(),
            spec: RootSpec::Dir { base: FilePath::parse("/srv/www").unwrap() },
            read_only: true,
            service_id: None,
            created_by: "test".into(),
            created_at: forge_core::Timestamp(0),
        })
        .await
        .unwrap();
    let (s, b) = admin.post("/api/v1/sites", site("/srv/www/shop")).await;
    assert_eq!(s, StatusCode::CREATED, "{b}");
    // A sibling that only shares a name prefix, and /etc, are still refused.
    for bad in ["/srv/wwwx", "/etc"] {
        let mut v = site(bad);
        v["name"] = json!("st2");
        assert!(admin.post("/api/v1/sites", v).await.0.is_client_error(), "{bad}");
    }
}

#[tokio::test]
async fn basic_auth_stores_only_hashes_and_never_returns_them() {
    use forge_core::ports::SiteRepository;
    let h = start().await;
    let admin = Client::login(&h.base, ADMIN_EMAIL, PASSWORD).await;
    let edge = add_node(&h, "edge-1").await;
    let auth = |users: Value| on_node("ba", &edge, json!({ "access": { "basic_auth": { "users": users } } }));
    let (s, e) = admin.post("/api/v1/sites", auth(json!([{ "user": "ops" }]))).await;
    assert_eq!(s, StatusCode::BAD_REQUEST, "a new user needs a password: {e}");
    let (s, b) = admin.post("/api/v1/sites", auth(json!([{ "user": "ops", "password": "s3cret-pw" }]))).await;
    assert_eq!(s, StatusCode::CREATED, "{b}");
    let id = b["id"].as_str().unwrap().to_owned();
    assert!(!b.to_string().contains("$6$") && !b.to_string().contains("s3cret"), "{b}");
    let stored = h.app.state.store.get_site(&id).await.unwrap().unwrap();
    let hash = stored.config["spec"]["access"]["basic_auth"]["users"][0]["hash"].as_str().unwrap().to_owned();
    assert!(hash.starts_with("$6$"));
    assert!(!stored.config.to_string().contains("s3cret"));
    // Saving the user without a password keeps the stored hash.
    let (s, b) = admin.put(&format!("/api/v1/sites/{id}"), auth(json!([{ "user": "ops" }]))).await;
    assert_eq!(s, StatusCode::OK, "{b}");
    let again = h.app.state.store.get_site(&id).await.unwrap().unwrap();
    assert_eq!(again.config["spec"]["access"]["basic_auth"]["users"][0]["hash"], hash);
    let (_, list) = admin.get("/api/v1/sites").await;
    assert!(!list.to_string().contains("$6$"));
    // Logs are read only from the endpoint's own edges.
    let other = add_node(&h, "edge-2").await;
    let (s, _) = admin.get(&format!("/api/v1/sites/{id}/logs?server_id={}&log=access", other.id)).await;
    assert_eq!(s, StatusCode::BAD_REQUEST);
}
