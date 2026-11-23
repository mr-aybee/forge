//! Server settings: the runtime public address (D-120).
#![allow(clippy::unwrap_used)]

use crate::common::*;
use reqwest::StatusCode;
use serde_json::json;

/// Platform admins set it with MFA; it is validated, audited with the old
/// and new value, and the generic settings editor cannot bypass it. Others
/// cannot read or change it. Clearing goes back to `public_url`.
#[tokio::test]
async fn the_public_address_is_a_validated_audited_platform_setting() {
    let h = start().await;
    let mut admin = Client::login(&h.base, ADMIN_EMAIL, PASSWORD).await;

    let (s, v) = admin.get("/api/v1/server/settings").await;
    assert_eq!(s, StatusCode::OK, "{v}");
    let configured = v["config"]["public_url"].as_str().unwrap().trim_end_matches('/').to_owned();
    assert_eq!(v["public_address"]["source"], "config", "{v}");
    assert_eq!(v["public_address"]["effective"].as_str(), Some(configured.as_str()), "{v}");
    assert!(v["config"]["trusted_proxies"].is_array(), "{v}");

    // Without MFA: refused and audited.
    let (s, e) = admin.put("/api/v1/server/public-address", json!({ "value": "https://forge.example.com" })).await;
    assert_eq!((s, e["title"].as_str()), (StatusCode::FORBIDDEN, Some("step_up_required")), "{e}");
    // The generic editor refuses `server.` keys.
    let (s, _) = admin.put("/api/v1/settings/server.public_address", json!({ "value": "https://evil.test" })).await;
    assert_eq!(s, StatusCode::BAD_REQUEST);

    step_up(&mut admin).await;
    for bad in [
        "forge.example.com",
        "ftp://forge.example.com",
        "https://forge.example.com/forge",
        "https://u:p@forge.example.com",
        "https://forge.example.com/?x=1",
        "javascript:alert(1)",
    ] {
        let (s, body) = admin.put("/api/v1/server/public-address", json!({ "value": bad })).await;
        assert_eq!(s, StatusCode::BAD_REQUEST, "{bad:?}: {body}");
    }
    let (s, v) = admin.put("/api/v1/server/public-address", json!({ "value": "https://Forge.Example.com/" })).await;
    assert_eq!(s, StatusCode::OK, "{v}");
    assert_eq!((v["effective"].as_str(), v["source"].as_str()), (Some("https://forge.example.com"), Some("setting")));
    assert_eq!(v["https"], true);
    let origins: Vec<&str> = v["allowed_origins"].as_array().unwrap().iter().filter_map(|o| o.as_str()).collect();
    assert!(origins.contains(&"https://forge.example.com"), "{v}");
    assert!(origins.contains(&configured.as_str()), "the direct address stays allowed: {v}");

    let (_, log) = admin.get("/api/v1/audit?action=server.public_address.update").await;
    let log = log.as_array().unwrap();
    let ok = log.iter().find(|e| e["outcome"] == "success").unwrap();
    assert_eq!(ok["details"]["to"], "https://forge.example.com", "{ok}");
    assert!(log.iter().any(|e| e["outcome"] == "denied"), "{log:?}");

    // A workspace operator can neither read nor change it.
    let mut op =
        user_with_role(&h, &admin, "op@example.com", "operator", &format!("workspace:{}", h.default_org)).await;
    let (s, _) = op.get("/api/v1/server/settings").await;
    assert_eq!(s, StatusCode::FORBIDDEN);
    step_up(&mut op).await;
    let (s, _) = op.put("/api/v1/server/public-address", json!({ "value": "https://evil.test" })).await;
    assert_eq!(s, StatusCode::FORBIDDEN);

    // Clearing goes back to public_url.
    let (s, v) = admin.call(reqwest::Method::DELETE, "/api/v1/server/public-address", None).await;
    assert_eq!((s, v["source"].as_str()), (StatusCode::OK, Some("config")), "{v}");
    assert_eq!(v["effective"].as_str(), Some(configured.as_str()));
    h.app.stop().await;
}
