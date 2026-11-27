//! D-177, the service reconciler against nodes played by the test: an
//! exited container is started again, a movable service leaves a node that
//! is offline past the grace period, a service with a volume stays and is
//! stranded, and a drained node takes no new placements.
#![allow(clippy::unwrap_used)]

use std::time::Duration;

use forge_core::model::{AgentStatus, ServiceInstance};
use forge_core::ports::{JobRepository, ServerRepository, ServiceRepository};
use forge_core::tags::FactsRepository;
use forge_core::{ServerId, ServiceId};
use reqwest::StatusCode;
use serde_json::json;

use crate::app_deploy::{container, fake_node, node};
use crate::common::*;

async fn instances(h: &Harness, app: &str) -> Vec<ServiceInstance> {
    h.app.state.store.list_instances(app.parse().unwrap()).await.unwrap()
}

/// Deploys `app` through the API and waits for its jobs.
async fn deploy(admin: &Client, app: &str) {
    let (s, jobs) = admin.post(&format!("/api/v1/services/{app}/deploy"), json!({})).await;
    assert_eq!(s, StatusCode::ACCEPTED, "{jobs}");

    for j in jobs.as_array().unwrap() {
        let done = wait_job(admin, j["id"].as_str().unwrap(), Duration::from_secs(60)).await;
        assert_eq!(done["status"], "succeeded", "{done}");
    }
}

/// Runs reconciler passes until `done` holds, or fails after 30 s.
async fn reconcile_until<F, Fut>(h: &Harness, what: &str, mut done: F)
where
    F: FnMut() -> Fut,
    Fut: std::future::Future<Output = bool>,
{
    let deadline = tokio::time::Instant::now() + Duration::from_secs(30);

    loop {
        h.app.state.reconciler.tick().await.unwrap();

        if done().await {
            return;
        }

        assert!(tokio::time::Instant::now() < deadline, "timed out waiting for {what}");
        tokio::task::yield_now().await;
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

/// Marks `server` offline, last seen long ago.
async fn lose(h: &Harness, server: ServerId) {
    h.app.state.store.set_status(server, AgentStatus::Offline).await.unwrap();
}

#[tokio::test]
async fn an_exited_container_is_started_again() {
    let h = start().await;
    let admin = Client::login(&h.base, ADMIN_EMAIL, PASSWORD).await;
    let server = node(&h, "lab-b").await;
    let app = container(&admin, "web", server).await;
    let _fake = fake_node(&h, server, true);
    h.app.state.store.put_facts(server, &json!({ "app_states": true }), h.app.state.clock.now()).await.unwrap();
    deploy(&admin, &app).await;

    // The node reports the container exited with 137.
    let svc = h.app.state.store.get_service(app.parse::<ServiceId>().unwrap()).await.unwrap().unwrap();
    let label = format!("{}/web", svc.spec["environment"].as_str().unwrap());
    let rows = json!([{ "app": label, "state": "exited", "exit_code": 137, "container_id": "c1" }]);
    h.app.state.reconciler.observe_node(server, &rows.to_string()).await;
    let i = instances(&h, &app).await.remove(0);
    assert_eq!((i.state.as_deref(), i.exit_code, i.status.as_str()), (Some("exited"), Some(137), "deployed"));
    let deployed_job = i.last_job_id;

    reconcile_until(&h, "a start job", || async {
        let i = instances(&h, &app).await.remove(0);

        match i.last_job_id.filter(|j| Some(*j) != deployed_job) {
            Some(j) => h.app.state.store.get_job(j).await.unwrap().payload["action"] == "start",
            None => false,
        }
    })
    .await;

    let (_, audit) = admin.get("/api/v1/audit?action=service.reconcile.restart").await;
    assert!(audit.to_string().contains("system:reconciler"), "{audit}");
}

#[tokio::test]
async fn a_movable_service_leaves_a_lost_node_and_the_old_instance_is_stranded() {
    let h = start().await;
    let admin = Client::login(&h.base, ADMIN_EMAIL, PASSWORD).await;
    let old = node(&h, "lab-b").await;
    let new = node(&h, "lab-c").await;
    let app = container(&admin, "api", old).await;
    let _a = fake_node(&h, old, true);
    let _b = fake_node(&h, new, true);
    deploy(&admin, &app).await;

    lose(&h, old).await;

    reconcile_until(&h, "a deployed instance on the other node", || async {
        instances(&h, &app).await.iter().any(|i| i.server_id == new && i.status == "deployed")
    })
    .await;

    let all = instances(&h, &app).await;
    let stranded = all.iter().find(|i| i.server_id == old).unwrap();
    assert_eq!(stranded.status, "stranded", "{all:?}");
    let (_, audit) = admin.get("/api/v1/audit?action=service.reconcile.move").await;
    assert!(audit.to_string().contains("system:reconciler"), "{audit}");
}

#[tokio::test]
async fn a_service_with_a_volume_is_stranded_and_not_moved() {
    let h = start().await;
    let admin = Client::login(&h.base, ADMIN_EMAIL, PASSWORD).await;
    let old = node(&h, "lab-b").await;
    let other = node(&h, "lab-c").await;
    let app = container(&admin, "db", old).await;
    let id: ServiceId = app.parse().unwrap();

    // A volume makes it unmovable; recorded as deployed without a node.
    let mut svc = h.app.state.store.get_service(id).await.unwrap().unwrap();
    svc.spec["volumes"] = json!(["/srv/db:/var/lib/data"]);
    h.app.state.store.put_service(&svc).await.unwrap();
    let mut i = instances(&h, &app).await.remove(0);
    i.status = "deployed".into();
    h.app.state.store.put_instance(&i).await.unwrap();
    lose(&h, old).await;

    reconcile_until(&h, "a stranded instance", || async {
        instances(&h, &app).await.iter().any(|i| i.status == "stranded")
    })
    .await;

    let all = instances(&h, &app).await;
    assert_eq!(all.len(), 1, "not moved: {all:?}");
    assert!(all.iter().all(|i| i.server_id != other));
    let (_, audit) = admin.get("/api/v1/audit?action=service.reconcile.stranded").await;
    assert!(audit.to_string().contains("system:reconciler"), "{audit}");
}

#[tokio::test]
async fn a_drained_node_is_audited_and_takes_no_new_placements() {
    let h = start().await;
    let admin = Client::login(&h.base, ADMIN_EMAIL, PASSWORD).await;
    let first = node(&h, "lab-b").await;
    let drained = node(&h, "lab-c").await;
    let app = container(&admin, "site", first).await;

    let (s, r) = admin.post(&format!("/api/v1/servers/{drained}/drain"), json!({})).await;
    assert_eq!(s, StatusCode::OK, "{r}");
    assert_eq!(r["drained"], true);
    assert!(h.app.state.store.get_server(drained).await.unwrap().drained);
    let (_, audit) = admin.get("/api/v1/audit?action=server.drain").await;
    assert!(audit.to_string().contains(&drained.to_string()), "{audit}");

    let placements = json!([{ "server_id": first }, { "server_id": drained }]);
    let (s, r) = admin.put(&format!("/api/v1/services/{app}/instances"), placements.clone()).await;
    assert_eq!(s, StatusCode::CONFLICT, "{r}");
    assert_eq!(instances(&h, &app).await.len(), 1, "nothing changed");

    let (s, r) = admin.post(&format!("/api/v1/servers/{drained}/undrain"), json!({})).await;
    assert_eq!((s, r["drained"].clone()), (StatusCode::OK, json!(false)), "{r}");
    let (s, r) = admin.put(&format!("/api/v1/services/{app}/instances"), placements).await;
    assert_eq!(s, StatusCode::OK, "{r}");
}
