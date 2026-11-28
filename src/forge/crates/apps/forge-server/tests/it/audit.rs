//! Audit API: server-side filters, cursor pages, distinct actions and authorization.
#![allow(clippy::unwrap_used)]

use reqwest::StatusCode;
use serde_json::json;

use crate::common::{ADMIN_EMAIL, Client, PASSWORD, start, user_with_role};

#[tokio::test]
async fn audit_pages_filter_and_list_actions() {
    let h = start().await;
    let admin = Client::login(&h.base, ADMIN_EMAIL, PASSWORD).await;

    // Create some mutations so the log has a few kinds of action.
    for i in 0..5 {
        let (s, c) = admin.post("/api/v1/clusters", json!({ "name": format!("c{i}"), "description": "" })).await;
        assert_eq!(s, StatusCode::CREATED, "{c}");
    }

    // Distinct actions with counts.
    let (s, actions) = admin.get("/api/v1/audit/actions").await;
    assert_eq!(s, StatusCode::OK, "{actions}");
    let list = actions.as_array().unwrap();
    assert!(!list.is_empty());
    assert!(list.iter().all(|a| a["action"].is_string() && a["count"].as_i64().unwrap() > 0));
    let names: Vec<&str> = list.iter().map(|a| a["action"].as_str().unwrap()).collect();
    let mut sorted = names.clone();
    sorted.sort_unstable();
    assert_eq!(names, sorted);
    let cluster_action = names.iter().find(|a| a.starts_with("cluster.")).copied().unwrap();

    // Page through everything two at a time; no entry appears twice.
    let (_, first) = admin.get("/api/v1/audit/page?limit=2&total=true").await;
    let total = first["total"].as_i64().unwrap();
    assert!(total >= 5);
    let mut seen: Vec<i64> = Vec::new();
    let mut page = first;

    loop {
        seen.extend(page["entries"].as_array().unwrap().iter().map(|e| e["seq"].as_i64().unwrap()));
        let Some(c) = page["next_cursor"].as_i64() else { break };
        let (s, p) = admin.get(&format!("/api/v1/audit/page?limit=2&before={c}")).await;
        assert_eq!(s, StatusCode::OK);
        assert!(p.get("total").is_none(), "total only on request");
        page = p;
    }

    assert!(seen.windows(2).all(|w| w[0] > w[1]), "strictly newest first: {seen:?}");
    assert!(seen.len() as i64 >= total);

    // Multiple action prefixes, comma and repeated forms.
    let (_, a) = admin.get(&format!("/api/v1/audit/page?action={cluster_action},auth.&limit=200")).await;
    let (_, b) = admin.get(&format!("/api/v1/audit/page?action={cluster_action}&action=auth.&limit=200")).await;
    assert_eq!(a["entries"], b["entries"]);
    let entries = a["entries"].as_array().unwrap();
    assert!(entries.iter().filter(|e| e["action"] == cluster_action).count() >= 5);
    assert!(entries.iter().all(|e| {
        let act = e["action"].as_str().unwrap();
        act.starts_with(cluster_action) || act.starts_with("auth.")
    }));

    // Date range: an empty window in the far past, and until must follow since.
    let (_, old) = admin.get("/api/v1/audit/page?since=1&until=2").await;
    assert_eq!(old["entries"], json!([]));
    assert_eq!(old["next_cursor"], json!(null));
    let (s, _) = admin.get("/api/v1/audit/page?since=5&until=5").await;
    assert_eq!(s, StatusCode::BAD_REQUEST);

    // Free text over action, actor and resource.
    let (_, hits) =
        admin.get(&format!("/api/v1/audit/page?q={}&limit=200&total=true", cluster_action.to_uppercase())).await;
    assert!(hits["total"].as_i64().unwrap() >= 5);
    assert!(hits["entries"].as_array().unwrap().iter().all(|e| {
        let row = format!("{} {} {} {}", e["action"], e["actor"], e["resource"], e["details"]).to_lowercase();
        row.contains(cluster_action)
    }));

    // Histogram: same filters, every bucket present, counts add up to the total.
    let (s, hist) = admin.get(&format!("/api/v1/audit/histogram?action={cluster_action}&buckets=500")).await;
    assert_eq!(s, StatusCode::OK, "{hist}");
    let buckets = hist["buckets"].as_array().unwrap();
    assert!(!buckets.is_empty() && buckets.len() <= 201, "clamped: {}", buckets.len());
    let counted: i64 = buckets
        .iter()
        .map(|b| b["success"].as_i64().unwrap() + b["denied"].as_i64().unwrap() + b["failure"].as_i64().unwrap())
        .sum();
    let (_, matching) = admin.get(&format!("/api/v1/audit/page?action={cluster_action}&limit=1&total=true")).await;
    assert_eq!(counted, matching["total"].as_i64().unwrap());
    assert!(hist["step_ms"].as_i64().unwrap() >= 1_000);
    let (_, none) = admin.get("/api/v1/audit/histogram?outcome=bogus").await;
    assert!(
        none["buckets"].as_array().unwrap().iter().all(|b| b["success"] == 0 && b["denied"] == 0 && b["failure"] == 0)
    );
    let (s, _) = admin.get("/api/v1/audit/histogram?since=5&until=5").await;
    assert_eq!(s, StatusCode::BAD_REQUEST);

    // Outcome aliases, and the legacy array endpoint keeps its shape.
    let (_, ok) = admin.get("/api/v1/audit?outcome=allow&limit=500").await;
    assert!(ok.as_array().unwrap().iter().all(|e| e["outcome"] == "success"));
    let (_, none) = admin.get("/api/v1/audit?outcome=bogus").await;
    assert_eq!(none, json!([]));
    let (_, arr) = admin.get("/api/v1/audit?limit=3").await;
    assert_eq!(arr.as_array().unwrap().len(), 3);

    // Someone without audit access is refused on every endpoint.
    let op = user_with_role(&h, &admin, "op@example.com", "operator", "platform:root").await;

    for path in ["/api/v1/audit", "/api/v1/audit/page", "/api/v1/audit/actions", "/api/v1/audit/histogram"] {
        let (s, _) = op.get(path).await;
        assert_eq!(s, StatusCode::FORBIDDEN, "{path}");
    }

    let auditor = user_with_role(&h, &admin, "aud@example.com", "auditor", "platform:root").await;
    let (s, _) = auditor.get("/api/v1/audit/actions").await;
    assert_eq!(s, StatusCode::OK);
}
