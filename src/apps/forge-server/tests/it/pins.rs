//! Pins: add, list, order, remove, the cap, visibility and isolation.
#![allow(clippy::unwrap_used)] // test code

use reqwest::{Method, StatusCode};
use serde_json::{Value, json};

use crate::common::{ADMIN_EMAIL, Client, PASSWORD, start, user_with_role};

async fn cluster(admin: &Client, name: &str) -> String {
    let (s, c) = admin.post("/api/v1/clusters", json!({ "name": name, "description": "" })).await;
    assert_eq!(s, StatusCode::CREATED, "{c}");

    c["id"].as_str().unwrap().to_owned()
}

async fn pin(c: &Client, kind: &str, id: &str) -> (StatusCode, Value) {
    c.call(Method::PUT, &format!("/api/v1/me/pins/{kind}/{id}"), None).await
}

async fn pinned(c: &Client) -> Vec<String> {
    let (s, b) = c.get("/api/v1/me/pins").await;
    assert_eq!(s, StatusCode::OK, "{b}");

    b["items"].as_array().unwrap().iter().map(|i| i["id"].as_str().unwrap().to_owned()).collect()
}

#[tokio::test]
async fn pins_add_list_order_remove_and_stay_private() {
    let h = start().await;
    let admin = Client::login(&h.base, ADMIN_EMAIL, PASSWORD).await;
    let a = cluster(&admin, "alpha").await;
    let b = cluster(&admin, "beta").await;
    let c = cluster(&admin, "gamma").await;

    for id in [&a, &b, &c] {
        assert_eq!(pin(&admin, "cluster", id).await.0, StatusCode::NO_CONTENT);
    }

    // Idempotent: pinning again changes nothing.
    assert_eq!(pin(&admin, "cluster", &a).await.0, StatusCode::NO_CONTENT);
    assert_eq!(pinned(&admin).await, vec![a.clone(), b.clone(), c.clone()]);
    let (_, body) = admin.get("/api/v1/me/pins").await;
    let first = &body["items"][0];
    assert_eq!((first["kind"].as_str(), first["label"].as_str()), (Some("cluster"), Some("alpha")));
    assert!(first["created_at"].as_i64().unwrap() > 0);

    // Reorder: listed first, unknown ignored, the rest keep their order.
    let (s, _) = admin
        .call(
            Method::PUT,
            "/api/v1/me/pins/order",
            Some(json!({ "items": [
                { "kind": "cluster", "id": c },
                { "kind": "project", "id": "01900000-0000-7000-8000-000000000000" },
            ] })),
        )
        .await;
    assert_eq!(s, StatusCode::NO_CONTENT);
    assert_eq!(pinned(&admin).await, vec![c.clone(), a.clone(), b.clone()]);

    // Remove, twice (idempotent).
    for _ in 0..2 {
        let (s, _) = admin.call(Method::DELETE, &format!("/api/v1/me/pins/cluster/{a}"), None).await;
        assert_eq!(s, StatusCode::NO_CONTENT);
    }

    assert_eq!(pinned(&admin).await, vec![c.clone(), b.clone()]);

    // Errors: unknown kind, bad id, missing entity.
    assert_eq!(pin(&admin, "server", &a).await.0, StatusCode::BAD_REQUEST);
    assert_eq!(pin(&admin, "cluster", "nope").await.0, StatusCode::BAD_REQUEST);
    assert_eq!(pin(&admin, "node", "01900000-0000-7000-8000-000000000000").await.0, StatusCode::NOT_FOUND);

    // Another user never sees these pins.
    let other = user_with_role(&h, &admin, "other@example.com", "auditor", "platform:root").await;
    assert!(pinned(&other).await.is_empty());
    assert_eq!(pin(&other, "cluster", &a).await.0, StatusCode::NO_CONTENT);
    assert_eq!(pinned(&other).await, vec![a.clone()]);
    assert_eq!(pinned(&admin).await, vec![c.clone(), b.clone()]);

    // Audited.
    let (_, audit) = admin.get("/api/v1/audit?limit=100").await;
    let text = serde_json::to_string(&audit).unwrap();

    for action in ["user.pin.add", "user.pin.remove", "user.pin.reorder"] {
        assert!(text.contains(action), "missing {action}");
    }

    h.app.stop().await;
}

#[tokio::test]
async fn pins_are_capped_at_fifty() {
    let h = start().await;
    let admin = Client::login(&h.base, ADMIN_EMAIL, PASSWORD).await;
    let mut ids = Vec::new();

    for i in 0..51 {
        ids.push(cluster(&admin, &format!("c{i}")).await);
    }

    for id in &ids[..50] {
        assert_eq!(pin(&admin, "cluster", id).await.0, StatusCode::NO_CONTENT);
    }

    let (s, e) = pin(&admin, "cluster", &ids[50]).await;
    assert_eq!(s, StatusCode::CONFLICT, "{e}");
    assert_eq!(e["title"], "pin_limit", "{e}");

    // A pin that exists is still accepted at the cap.
    assert_eq!(pin(&admin, "cluster", &ids[0]).await.0, StatusCode::NO_CONTENT);
    assert_eq!(pinned(&admin).await.len(), 50);
    h.app.stop().await;
}

#[tokio::test]
async fn pins_hide_deleted_entities_and_lost_access() {
    let h = start().await;
    let admin = Client::login(&h.base, ADMIN_EMAIL, PASSWORD).await;
    let keep = cluster(&admin, "keep").await;
    let gone = cluster(&admin, "gone").await;
    let other = cluster(&admin, "other").await;

    // An operator on one cluster cannot pin another (404, not 403).
    let op = user_with_role(&h, &admin, "op@example.com", "operator", &format!("cluster:{keep}")).await;
    assert_eq!(pin(&op, "cluster", &keep).await.0, StatusCode::NO_CONTENT);
    assert_eq!(pin(&op, "cluster", &other).await.0, StatusCode::NOT_FOUND);
    assert_eq!(pinned(&op).await, vec![keep.clone()]);

    // Losing view hides the pin; it is kept and returns with access.
    let (_, bindings) = admin.get("/api/v1/role-bindings").await;
    let me = op.get("/api/v1/me").await.1;
    let binding = bindings.as_array().unwrap().iter().find(|b| b["user_id"] == me["user"]["id"]).unwrap()["id"]
        .as_str()
        .unwrap()
        .to_owned();
    let (s, _) = admin.call(Method::DELETE, &format!("/api/v1/role-bindings/{binding}"), None).await;
    assert_eq!(s, StatusCode::NO_CONTENT);
    assert!(pinned(&op).await.is_empty());
    let (s, _) = admin
        .post(
            "/api/v1/role-bindings",
            json!({ "user_id": me["user"]["id"], "role": "operator", "scope": format!("cluster:{keep}") }),
        )
        .await;

    if s == StatusCode::CREATED {
        assert_eq!(pinned(&op).await, vec![keep.clone()]);
    }

    // A deleted entity is hidden.
    assert_eq!(pin(&admin, "cluster", &keep).await.0, StatusCode::NO_CONTENT);
    assert_eq!(pin(&admin, "cluster", &gone).await.0, StatusCode::NO_CONTENT);
    let (s, _) = admin.call(Method::DELETE, &format!("/api/v1/clusters/{gone}"), None).await;
    assert_eq!(s, StatusCode::NO_CONTENT);
    assert_eq!(pinned(&admin).await, vec![keep.clone()]);
    h.app.stop().await;
}
