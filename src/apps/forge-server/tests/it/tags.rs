//! Tags: set, read, counts, validation, authorization and audit.
#![allow(clippy::unwrap_used)]

use crate::common::{ADMIN_EMAIL, Client, PASSWORD, start, user_with_role};
use reqwest::{Method, StatusCode};
use serde_json::json;

#[tokio::test]
async fn cluster_tags_round_trip_with_validation_authz_and_audit() {
    let h = start().await;
    let admin = Client::login(&h.base, ADMIN_EMAIL, PASSWORD).await;
    let (s, c) = admin.post("/api/v1/clusters", json!({ "name": "prod", "description": "" })).await;
    assert_eq!(s, StatusCode::CREATED, "{c}");
    let id = c["id"].as_str().unwrap().to_owned();

    // Set: normalised, de-duplicated, sorted.
    let (s, b) = admin
        .call(
            Method::PUT,
            &format!("/api/v1/tags/cluster/{id}"),
            Some(json!({ "tags": [" dc : fra1 ", "active", "ACTIVE", "location:karachi"] })),
        )
        .await;
    assert_eq!(s, StatusCode::OK, "{b}");
    assert_eq!(b["tags"], json!(["active", "dc:fra1", "location:karachi"]));

    // Read one item, the kind map and the counts.
    let (_, one) = admin.get(&format!("/api/v1/tags/cluster/{id}")).await;
    assert_eq!(one["tags"], json!(["active", "dc:fra1", "location:karachi"]));
    let (_, map) = admin.get("/api/v1/tags/cluster").await;
    assert_eq!(map[&id], json!(["active", "dc:fra1", "location:karachi"]));
    let (_, counts) = admin.get("/api/v1/tags?kind=cluster").await;
    assert!(counts.as_array().unwrap().iter().any(|r| r["tag"] == "dc:fra1" && r["count"] == 1));

    // Invalid tags are refused.
    for bad in [json!([":x"]), json!(["a,b"]), json!(["x".repeat(65)])] {
        let (s, _) = admin.call(Method::PUT, &format!("/api/v1/tags/cluster/{id}"), Some(json!({ "tags": bad }))).await;
        assert_eq!(s, StatusCode::BAD_REQUEST);
    }
    // Unknown kind and missing item.
    let (s, _) = admin.get("/api/v1/tags/bogus").await;
    assert_eq!(s, StatusCode::BAD_REQUEST);
    let (s, _) = admin
        .call(Method::PUT, "/api/v1/tags/server/01900000-0000-7000-8000-000000000000", Some(json!({ "tags": ["a"] })))
        .await;
    assert_eq!(s, StatusCode::NOT_FOUND);

    // A viewer can read but not change.
    let viewer = user_with_role(&h, &admin, "auditor@example.com", "auditor", "platform:root").await;
    let (s, _) = viewer.get(&format!("/api/v1/tags/cluster/{id}")).await;
    assert_eq!(s, StatusCode::OK);
    let (s, _) =
        viewer.call(Method::PUT, &format!("/api/v1/tags/cluster/{id}"), Some(json!({ "tags": ["mine"] }))).await;
    assert_eq!(s, StatusCode::FORBIDDEN);

    // Clearing works and is audited with before/after.
    let (s, b) = admin.call(Method::PUT, &format!("/api/v1/tags/cluster/{id}"), Some(json!({ "tags": [] }))).await;
    assert_eq!((s, b["tags"].clone()), (StatusCode::OK, json!([])));
    let (_, audit) = admin.get("/api/v1/audit?limit=50").await;
    let text = serde_json::to_string(&audit).unwrap();
    assert!(text.contains("tags.update"));

    // Servers list carries facts and tags (empty here) and accepts ?tag.
    let (s, nodes) = admin.get("/api/v1/servers?tag=dc:fra1").await;
    assert_eq!(s, StatusCode::OK);
    assert!(nodes.as_array().unwrap().is_empty());
    h.app.stop().await;
}
