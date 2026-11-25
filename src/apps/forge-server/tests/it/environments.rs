//! Environments live in clusters, names unique per cluster (D-056), and are
//! the environments of a project (D-102).
#![allow(clippy::unwrap_used)]

use crate::common::{ADMIN_EMAIL, Client, PASSWORD, start, user_with_role};
use reqwest::{Method, StatusCode};
use serde_json::json;

fn id(v: &serde_json::Value) -> &str {
    v["id"].as_str().unwrap()
}

#[tokio::test]
async fn environments_are_created_and_moved_inside_clusters() {
    let h = start().await;
    let admin = Client::login(&h.base, ADMIN_EMAIL, PASSWORD).await;
    let (_, a) = admin.post("/api/v1/clusters", json!({"name": "dev"})).await;
    let (_, b) = admin.post("/api/v1/clusters", json!({"name": "prod"})).await;
    let (s, shop) = admin.post("/api/v1/projects", json!({"name": "shop"})).await;
    assert_eq!(s, StatusCode::CREATED, "{shop}");

    // A cluster and a project are required.
    let (s, _) = admin.post("/api/v1/environments", json!({"name": "web", "project_id": shop["id"]})).await;
    assert_eq!(s, StatusCode::BAD_REQUEST);
    let (s, e) = admin.post("/api/v1/environments", json!({"name": "web", "cluster_id": a["id"]})).await;
    assert_eq!(s, StatusCode::BAD_REQUEST, "{e}");
    assert!(e.to_string().contains("project_id"));
    let unknown = forge_core::ProjectId::new();
    let (s, _) =
        admin.post("/api/v1/environments", json!({"name": "web", "cluster_id": a["id"], "project_id": unknown})).await;
    assert_eq!(s, StatusCode::BAD_REQUEST, "unknown project");

    let ns = |name: &str, cluster: &serde_json::Value| json!({"name": name, "cluster_id": cluster["id"], "project_id": shop["id"]});
    let (s, web_a) = admin.post("/api/v1/environments", ns("web", &a)).await;
    assert_eq!(s, StatusCode::CREATED, "{web_a}");
    assert_eq!(web_a["project_id"], shop["id"]);
    let (s, _) = admin.post("/api/v1/environments", ns("web", &a)).await;
    assert_eq!(s, StatusCode::CONFLICT, "unique inside a cluster");
    let (s, web_b) = admin.post("/api/v1/environments", ns("web", &b)).await;
    assert_eq!(s, StatusCode::CREATED, "the same name in another cluster");
    let (_, api_b) = admin.post("/api/v1/environments", ns("api", &b)).await;

    let (_, all) = admin.get("/api/v1/environments").await;
    assert_eq!(all.as_array().unwrap().len(), 3);
    let (_, in_b) = admin.get(&format!("/api/v1/environments?cluster_id={}", id(&b))).await;
    let names: Vec<_> = in_b.as_array().unwrap().iter().map(|n| n["name"].as_str().unwrap()).collect();
    assert_eq!(names, ["api", "web"]);

    // Moves keep names unique and need a target.
    let mv = |id: &serde_json::Value| format!("/api/v1/environments/{}/cluster", id.as_str().unwrap());
    let (s, _) = admin.put(&mv(&web_b["id"]), json!({"cluster_id": a["id"]})).await;
    assert_eq!(s, StatusCode::CONFLICT);
    let (s, _) = admin.put(&mv(&api_b["id"]), json!({})).await;
    assert_eq!(s, StatusCode::BAD_REQUEST);
    let (s, moved) = admin.put(&mv(&api_b["id"]), json!({"cluster_id": a["id"]})).await;
    assert_eq!(s, StatusCode::OK, "{moved}");
    assert_eq!(moved["cluster_id"], a["id"]);

    // A cluster with environments cannot be deleted.
    let del = format!("/api/v1/clusters/{}", id(&b));
    let (s, e) = admin.call(Method::DELETE, &del, None).await;
    assert_eq!(s, StatusCode::CONFLICT, "{e}");
    assert!(e.to_string().contains("web"));

    // A binding on a cluster does not reach the environments placed in it.
    let ca = user_with_role(&h, &admin, "ca@example.com", "operator", &format!("cluster:{}", id(&a))).await;
    let (_, seen) = ca.get("/api/v1/environments").await;
    assert!(seen.as_array().unwrap().is_empty(), "{seen}");
    let (s, _) = ca.put(&mv(&web_a["id"]), json!({"cluster_id": a["id"]})).await;
    assert_eq!(s, StatusCode::FORBIDDEN);

    let (_, audit) = admin.get("/api/v1/audit?limit=200").await;
    let text = audit.to_string();
    assert!(text.contains("environment.create") && text.contains("environment.move"));
}

/// Projects (D-102): CRUD, the environments listed with a group, rename
/// and delete of environments, attaching legacy ones, and a binding on a
/// group reaching its environments only.
#[tokio::test]
async fn projects_group_environments() {
    let h = start().await;
    let admin = Client::login(&h.base, ADMIN_EMAIL, PASSWORD).await;
    let (_, cl) = admin.post("/api/v1/clusters", json!({"name": "lab"})).await;

    // Names are lower-case slugs, unique per workspace.
    for bad in ["Shop", "shop_1", "", "a b"] {
        let (s, _) = admin.post("/api/v1/projects", json!({"name": bad})).await;
        assert_eq!(s, StatusCode::BAD_REQUEST, "{bad:?}");
    }

    let (s, shop) = admin.post("/api/v1/projects", json!({"name": "shop", "description": "Web shop"})).await;
    assert_eq!(s, StatusCode::CREATED, "{shop}");
    assert_eq!(shop["description"], "Web shop");
    assert_eq!(shop["service_count"], 0);
    assert!(shop["environments"].as_array().unwrap().is_empty());
    let (s, _) = admin.post("/api/v1/projects", json!({"name": "shop"})).await;
    assert_eq!(s, StatusCode::CONFLICT);
    let (_, crm) = admin.post("/api/v1/projects", json!({"name": "crm"})).await;

    let ns = |name: &str, group: &serde_json::Value| json!({"name": name, "cluster_id": cl["id"], "project_id": group["id"]});
    let (_, shop_test) = admin.post("/api/v1/environments", ns("test", &shop)).await;
    let (_, shop_uat) = admin.post("/api/v1/environments", ns("uat", &shop)).await;
    let (_, crm_test) = admin.post("/api/v1/environments", ns("crm-test", &crm)).await;

    // The group lists its environments; the environment list filters by group.
    let (s, got) = admin.get(&format!("/api/v1/projects/{}", id(&shop))).await;
    assert_eq!(s, StatusCode::OK, "{got}");
    let envs: Vec<_> = got["environments"].as_array().unwrap().iter().map(|e| e["name"].as_str().unwrap()).collect();
    assert_eq!(envs, ["test", "uat"]);
    assert_eq!(got["environments"][0]["cluster_id"], cl["id"]);
    assert_eq!(got["environments"][0]["protected"], false);
    let (_, all) = admin.get("/api/v1/projects").await;
    assert_eq!(all.as_array().unwrap().len(), 2);
    let (_, filtered) = admin.get(&format!("/api/v1/environments?project_id={}", id(&crm))).await;
    assert_eq!(filtered.as_array().unwrap().len(), 1);
    assert_eq!(filtered[0]["id"], crm_test["id"]);

    // Update: name and description, separately; duplicates refused.
    let put = format!("/api/v1/projects/{}", id(&shop));
    let (s, upd) = admin.put(&put, json!({"description": "Shop front"})).await;
    assert_eq!(s, StatusCode::OK, "{upd}");
    assert_eq!(upd["name"], "shop");
    assert_eq!(upd["description"], "Shop front");
    let (s, _) = admin.put(&put, json!({"name": "crm"})).await;
    assert_eq!(s, StatusCode::CONFLICT);
    let (s, upd) = admin.put(&put, json!({"name": "shop-v2"})).await;
    assert_eq!(s, StatusCode::OK, "{upd}");
    assert_eq!(upd["name"], "shop-v2");

    // Rename an environment (unique in its cluster), delete an empty one.
    let (s, renamed) = admin.put(&format!("/api/v1/environments/{}", id(&shop_uat)), json!({"name": "staging"})).await;
    assert_eq!(s, StatusCode::OK, "{renamed}");
    assert_eq!(renamed["name"], "staging");
    let (s, _) = admin.put(&format!("/api/v1/environments/{}", id(&shop_uat)), json!({"name": "test"})).await;
    assert_eq!(s, StatusCode::CONFLICT);

    // An environment with a service cannot be deleted; nor can its group.
    let (s, app) = admin
        .post(
            "/api/v1/services",
            json!({"environment_id": shop_test["id"], "name": "web", "runtime": "systemd", "command": "python3 app.py"}),
        )
        .await;
    assert_eq!(s, StatusCode::CREATED, "{app}");
    let (_, got) = admin.get(&format!("/api/v1/projects/{}", id(&shop))).await;
    assert_eq!(got["service_count"], 1, "{got}");
    let (s, e) = admin.call(Method::DELETE, &format!("/api/v1/environments/{}", id(&shop_test)), None).await;
    assert_eq!(s, StatusCode::CONFLICT, "{e}");
    let (s, e) = admin.call(Method::DELETE, &put, None).await;
    assert_eq!(s, StatusCode::CONFLICT, "{e}");
    assert!(e.to_string().contains("staging"));

    // Delete the empty environment, move the other to crm, then the group goes.
    let (s, _) = admin.call(Method::DELETE, &format!("/api/v1/environments/{}", id(&shop_uat)), None).await;
    assert_eq!(s, StatusCode::NO_CONTENT);
    let (s, attached) =
        admin.put(&format!("/api/v1/environments/{}/project", id(&shop_test)), json!({"project_id": crm["id"]})).await;
    assert_eq!(s, StatusCode::OK, "{attached}");
    assert_eq!(attached["project_id"], crm["id"]);
    let unknown = forge_core::ProjectId::new();
    let (s, _) =
        admin.put(&format!("/api/v1/environments/{}/project", id(&shop_test)), json!({"project_id": unknown})).await;
    assert_eq!(s, StatusCode::BAD_REQUEST);
    let (s, _) = admin.call(Method::DELETE, &put, None).await;
    assert_eq!(s, StatusCode::NO_CONTENT);
    let (s, _) = admin.get(&put).await;
    assert_eq!(s, StatusCode::NOT_FOUND);

    // A developer bound to the crm group sees its environments only.
    let dev = user_with_role(&h, &admin, "dev@example.com", "developer", &format!("project:{}", id(&crm))).await;
    let (_, seen) = dev.get("/api/v1/environments").await;
    let mut names: Vec<_> = seen.as_array().unwrap().iter().map(|n| n["name"].as_str().unwrap()).collect();
    names.sort_unstable();
    assert_eq!(names, ["crm-test", "test"], "{seen}");
    let (_, groups) = dev.get("/api/v1/projects").await;
    assert_eq!(groups.as_array().unwrap().len(), 1, "{groups}");
    assert_eq!(groups[0]["name"], "crm");
    let (s, _) = dev.post("/api/v1/projects", json!({"name": "mine"})).await;
    assert_eq!(s, StatusCode::FORBIDDEN);
    let (s, _) = dev.put(&format!("/api/v1/environments/{}", id(&crm_test)), json!({"name": "x"})).await;
    assert_eq!(s, StatusCode::FORBIDDEN);

    let (_, audit) = admin.get("/api/v1/audit?limit=300").await;
    let text = audit.to_string();

    for a in [
        "project.create",
        "project.update",
        "project.delete",
        "environment.rename",
        "environment.delete",
        "environment.set_project",
    ] {
        assert!(text.contains(a), "{a} audited");
    }
}
