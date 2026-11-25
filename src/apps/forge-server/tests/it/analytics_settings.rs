//! Analytics settings at runtime (D-136).
#![allow(clippy::unwrap_used)]

use crate::common::*;
use reqwest::{Method, StatusCode};
use serde_json::{Value, json};

fn setting<'a>(v: &'a Value, name: &str) -> &'a Value {
    v["settings"].as_array().unwrap().iter().find(|s| s["setting"] == name).unwrap()
}

fn result<'a>(report: &'a Value, name: &str) -> &'a Value {
    report["results"].as_array().unwrap().iter().find(|r| r["setting"] == name).unwrap()
}

/// Platform admins change settings with MFA; values are validated,
/// shortening retention needs confirmation, each setting reports its own
/// outcome, the change is audited with old and new values, and the generic
/// settings editor cannot bypass it. Others can neither change nor (without
/// a platform role) read them.
#[tokio::test]
async fn analytics_settings_are_validated_confirmed_audited_and_reported() {
    let h = start().await;
    let mut admin = Client::login(&h.base, ADMIN_EMAIL, PASSWORD).await;

    let (s, v) = admin.get("/api/v1/analytics/settings").await;
    assert_eq!(s, StatusCode::OK, "{v}");
    assert_eq!((v["engine"].as_str(), v["managed"].as_bool()), (Some("off"), Some(false)), "{v}");
    let logs = setting(&v, "app_logs_days");
    assert_eq!((logs["configured"].as_f64(), logs["effective"].as_f64()), (Some(30.0), Some(30.0)), "{v}");
    assert!(logs["override_value"].is_null());
    assert_eq!(setting(&v, "memory_budget_mb")["effective"].as_f64(), Some(1024.0));
    assert!(v["settings"].as_array().unwrap().iter().all(|s| s["setting"] != "memory_limit_mb"), "removed");
    assert_eq!(setting(&v, "cpu_quota_vcpu")["applies"], false, "not Forge's to apply for a non-managed engine");
    assert!(v["queue"]["queued_bytes"].is_number(), "{v}");
    assert_eq!(v["worker"]["status"], "off", "no worker runs while analytics is off: {v}");

    // Without MFA: refused.
    let (s, e) = admin.put("/api/v1/analytics/settings", json!({ "settings": { "search_threads": 2 } })).await;
    assert_eq!((s, e["title"].as_str()), (StatusCode::FORBIDDEN, Some("step_up_required")), "{e}");
    step_up(&mut admin).await;

    // Validation: out of range, unknown names, nothing to change.
    for bad in [
        json!({ "settings": { "app_logs_days": 0 } }),
        json!({ "settings": { "cpu_quota_vcpu": 0.05 } }),
        json!({ "settings": { "memory_budget_mb": 100 } }),
        json!({ "settings": { "no_such_setting": 1 } }),
        json!({ "settings": {} }),
    ] {
        let (s, body) = admin.put("/api/v1/analytics/settings", bad.clone()).await;
        assert!(s == StatusCode::BAD_REQUEST || s == StatusCode::UNPROCESSABLE_ENTITY, "{bad}: {s} {body}");
    }

    // Shortening retention needs confirmation.
    // CPU and memory belong to an engine Forge runs: refused here, with a
    // typed error and an audit row; removing an override is allowed.
    for foreign in [json!({ "cpu_quota_vcpu": 1.5 }), json!({ "memory_budget_mb": 2048 })] {
        let (s, e) = admin.put("/api/v1/analytics/settings", json!({ "settings": foreign })).await;
        assert_eq!((s, e["title"].as_str()), (StatusCode::CONFLICT, Some("managed_externally")), "{e}");
    }

    let (s, e) = admin.put("/api/v1/analytics/settings", json!({ "settings": { "cpu_quota_vcpu": null } })).await;
    assert_eq!(s, StatusCode::OK, "{e}");

    let shorten = json!({ "settings": { "app_logs_days": 7, "search_max_concurrent": 3 } });
    let (s, e) = admin.put("/api/v1/analytics/settings", shorten.clone()).await;
    assert_eq!((s, e["title"].as_str()), (StatusCode::CONFLICT, Some("confirmation_required")), "{e}");
    assert_eq!(e["confirm"][0]["confirm"], "shorten_retention", "{e}");
    let (_, v) = admin.get("/api/v1/analytics/settings").await;
    assert_eq!(setting(&v, "app_logs_days")["effective"].as_f64(), Some(30.0), "nothing changed");

    let mut confirmed = shorten;
    confirmed["confirm"] = json!({ "shorten_retention": true });
    let (s, report) = admin.put("/api/v1/analytics/settings", confirmed).await;
    assert_eq!(s, StatusCode::OK, "{report}");

    // Analytics is off here: retention waits, Forge's own limits apply at
    // once.
    assert_eq!(result(&report, "app_logs_days")["status"], "pending", "{report}");
    assert_eq!(result(&report, "search_max_concurrent")["status"], "applied", "{report}");

    let (_, v) = admin.get("/api/v1/analytics/settings").await;
    let logs = setting(&v, "app_logs_days");
    assert_eq!((logs["override_value"].as_f64(), logs["effective"].as_f64()), (Some(7.0), Some(7.0)), "{v}");
    assert_eq!(v["last_apply"]["results"].as_array().map(Vec::len), Some(2), "{v}");

    // Lengthening needs no confirmation; `null` returns to forge.toml.
    let (s, report) = admin.put("/api/v1/analytics/settings", json!({ "settings": { "app_logs_days": null } })).await;
    assert_eq!(s, StatusCode::OK, "{report}");
    let (_, v) = admin.get("/api/v1/analytics/settings").await;
    let logs = setting(&v, "app_logs_days");
    assert_eq!((logs["override_value"].is_null(), logs["effective"].as_f64()), (true, Some(30.0)), "{v}");
    assert_eq!(setting(&v, "search_max_concurrent")["override_value"].as_f64(), Some(3.0), "left alone");

    // Audited with old and new values; refusals too.
    let (_, log) = admin.get("/api/v1/audit?action=analytics.settings.update").await;
    let log = log.as_array().unwrap();
    let first = log.iter().filter(|e| e["outcome"] == "success").find(|e| {
        e["details"]["changes"]
            .as_array()
            .is_some_and(|c| c.iter().any(|c| c["setting"] == "app_logs_days" && c["new"] == 7.0))
    });
    let first = first.unwrap_or_else(|| panic!("{log:?}"));
    let change =
        first["details"]["changes"].as_array().unwrap().iter().find(|c| c["setting"] == "app_logs_days").unwrap();
    assert_eq!((change["old"].as_f64(), change["new"].as_f64()), (Some(30.0), Some(7.0)), "{first}");
    assert!(log.iter().any(|e| e["outcome"] == "denied"), "invalid values are audited: {log:?}");

    // The generic settings editor cannot write the overrides.
    let (s, _) = admin.put("/api/v1/settings/analytics.overrides", json!({ "value": "{}" })).await;
    assert_eq!(s, StatusCode::BAD_REQUEST);

    // Only the engine Forge runs can be restarted from Forge.
    let (s, _) = admin.post("/api/v1/analytics/restart", json!({})).await;
    assert_eq!(s, StatusCode::BAD_REQUEST);
    let (s, _) = admin.call(Method::DELETE, "/api/v1/analytics/restart", None).await;
    assert_eq!(s, StatusCode::NOT_FOUND);

    // A workspace operator can neither read nor change them.
    let mut op =
        user_with_role(&h, &admin, "op@example.com", "operator", &format!("workspace:{}", h.default_org)).await;
    let (s, _) = op.get("/api/v1/analytics/settings").await;
    assert_eq!(s, StatusCode::FORBIDDEN);
    step_up(&mut op).await;
    let (s, _) = op.put("/api/v1/analytics/settings", json!({ "settings": { "search_threads": 1 } })).await;
    assert_eq!(s, StatusCode::FORBIDDEN);
    h.app.stop().await;
}

/// Overrides are stored in the operational database and survive a restart.
#[tokio::test]
async fn analytics_overrides_survive_a_restart() {
    let h = start().await;
    let mut admin = Client::login(&h.base, ADMIN_EMAIL, PASSWORD).await;
    step_up(&mut admin).await;
    let (s, r) =
        admin.put("/api/v1/analytics/settings", json!({ "settings": { "ingest_max_batch_rows": 5000 } })).await;
    assert_eq!(s, StatusCode::OK, "{r}");
    h.app.stop().await;

    let state = forge_server::state::Services::build(test_config(h.dir.path())).await.unwrap();
    let view = state.analytics_control.view().await;
    let rows =
        view.settings.iter().find(|s| s.setting == forge_server::analytics_tuning::Setting::ingest_max_batch_rows);
    assert_eq!(rows.map(|s| s.effective), Some(5000.0));
    assert_eq!(state.analytics_control.subscribe().borrow().ingest_max_batch_rows, 5000, "published to the worker");
}
