//! F-03 / F-05: placing a native app on a Linux node prepares it (account,
//! folder) through an `app.action` job, and the app gets a writable file
//! root on its folder that goes away with the app.
#![allow(clippy::unwrap_used)]

use forge_core::files::{FileRepository, RootKind, RootSpec};
use reqwest::{Method, StatusCode};
use serde_json::{Value, json};

use crate::common::*;

async fn node(h: &Harness, os: &str) -> forge_core::model::Server {
    let server = forge_core::model::Server {
        id: forge_core::ServerId::new(),
        workspace_id: h.default_org.parse().unwrap(),
        environment_id: None,
        cluster_id: None,
        role: forge_core::model::NodeRole::Worker,
        display_name: None,
        hostname: format!("{os}-1"),
        os: os.into(),
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

    server
}

async fn app_jobs(h: &Harness) -> Vec<Value> {
    let rows: Vec<String> = sqlx::query_scalar("SELECT payload FROM jobs WHERE kind = 'app.action'")
        .fetch_all(h.app.state.store.pool())
        .await
        .unwrap();
    rows.iter().map(|r| serde_json::from_str(r).unwrap()).collect()
}

#[tokio::test]
async fn placing_a_native_app_prepares_it_and_its_root_follows_the_app() {
    let h = start().await;
    let admin = Client::login(&h.base, ADMIN_EMAIL, PASSWORD).await;
    let (_, cl) = admin.post("/api/v1/clusters", json!({"name": "lab"})).await;
    let (_, ag1) = admin.post("/api/v1/projects", json!({"name": "grp1"})).await;
    let (s, ns) = admin
        .post("/api/v1/environments", json!({"name": "lab-test", "cluster_id": cl["id"], "project_id": ag1["id"]}))
        .await;
    assert_eq!(s, StatusCode::CREATED, "{ns}");
    let linux = node(&h, "linux").await;
    let windows = node(&h, "windows").await;
    let (s, app) = admin
        .post(
            "/api/v1/services",
            json!({ "environment_id": ns["id"], "name": "xcweb", "runtime": "systemd", "command": "python3 app.py" }),
        )
        .await;
    assert_eq!(s, StatusCode::CREATED, "{app}");
    let id = app["id"].as_str().unwrap().to_owned();

    // Placing on a Linux node submits a `prepare` job for it (not for Windows).
    let (s, r) = admin
        .put(&format!("/api/v1/services/{id}/instances"), json!([{"server_id": linux.id}, {"server_id": windows.id}]))
        .await;
    assert_eq!(s, StatusCode::OK, "{r}");
    let jobs = app_jobs(&h).await;
    assert_eq!(jobs.len(), 1, "{jobs:?}");
    assert_eq!(jobs[0]["action"], "prepare");
    assert_eq!(jobs[0]["server_id"], json!(linux.id));

    // Placing again does not prepare again.
    let (s, _) = admin.put(&format!("/api/v1/services/{id}/instances"), json!([{"server_id": linux.id}])).await;
    assert_eq!(s, StatusCode::OK);
    assert_eq!(app_jobs(&h).await.len(), 1);

    // What the job does once the node prepared the app: one writable app root, idempotent.
    let state = &h.app.state;
    let model =
        forge_core::ports::ServiceRepository::get_service(&state.store, id.parse().unwrap()).await.unwrap().unwrap();
    let now = forge_core::Timestamp(1);
    let made = forge_server::app_roots::ensure(&state.store, state.audit.as_ref(), "user:x", now, &model, &linux)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(made.kind, RootKind::Service);
    assert!(!made.read_only);
    assert_eq!(made.service_id.as_deref(), Some(id.as_str()));
    assert_eq!(made.spec, RootSpec::App { environment: "lab-test".into(), app: "xcweb".into() });
    let again = forge_server::app_roots::ensure(&state.store, state.audit.as_ref(), "user:x", now, &model, &linux)
        .await
        .unwrap();
    assert!(again.is_none());
    assert!(
        forge_server::app_roots::ensure(&state.store, state.audit.as_ref(), "user:x", now, &model, &windows)
            .await
            .unwrap()
            .is_none(),
        "no app roots on Windows nodes"
    );
    assert_eq!(state.store.list_roots(linux.id).await.unwrap().len(), 1);

    // Deleting the (never deployed) app removes its root; both are audited.
    let (s, r) = admin.call(Method::DELETE, &format!("/api/v1/services/{id}"), None).await;
    assert_eq!(s, StatusCode::NO_CONTENT, "{r}");
    assert!(state.store.list_roots(linux.id).await.unwrap().is_empty());
    let (_, audit) = admin.call(Method::GET, "/api/v1/audit?action=file.root.delete", None).await;
    assert!(audit.to_string().contains(&made.id), "{audit}");
    let (_, audit) = admin.call(Method::GET, "/api/v1/audit?action=file.root.create", None).await;
    assert!(audit.to_string().contains("\"automatic\":true"), "{audit}");
}

#[tokio::test]
async fn container_apps_are_not_prepared() {
    let h = start().await;
    let admin = Client::login(&h.base, ADMIN_EMAIL, PASSWORD).await;
    let (_, cl) = admin.post("/api/v1/clusters", json!({"name": "lab"})).await;
    let (_, ag2) = admin.post("/api/v1/projects", json!({"name": "grp2"})).await;
    let (_, ns) = admin
        .post("/api/v1/environments", json!({"name": "uat", "cluster_id": cl["id"], "project_id": ag2["id"]}))
        .await;
    let linux = node(&h, "linux").await;
    let (_, app) = admin
        .post(
            "/api/v1/services",
            json!({ "environment_id": ns["id"], "name": "web", "runtime": "container", "image": "nginx:1.27" }),
        )
        .await;
    let id = app["id"].as_str().unwrap();
    let (s, _) = admin.put(&format!("/api/v1/services/{id}/instances"), json!([{"server_id": linux.id}])).await;
    assert_eq!(s, StatusCode::OK);
    assert!(app_jobs(&h).await.is_empty());
}
