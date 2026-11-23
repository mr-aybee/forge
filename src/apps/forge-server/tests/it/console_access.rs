//! `GET /me/capabilities`: console navigation gates per role and scope.
#![allow(clippy::unwrap_used)]

use crate::common::*;
use reqwest::StatusCode;
use serde_json::{Value, json};

const FIELDS: [&str; 17] = [
    "fleet_view",
    "terminal",
    "package_install",
    "software_updates",
    "sftp_access",
    "tunnels",
    "alerts_view",
    "alerts_manage",
    "notifications_manage",
    "audit_view",
    "users_manage",
    "access_manage",
    "vault",
    "forge_updates",
    "analytics_configure",
    "plugins",
    "platform_settings",
];

/// The capabilities object with exactly `on` set to true.
fn only(on: &[&str]) -> Value {
    for f in on {
        assert!(FIELDS.contains(f), "unknown field {f}");
    }
    Value::Object(FIELDS.iter().map(|f| ((*f).to_owned(), json!(on.contains(f)))).collect())
}

async fn caps(c: &Client) -> Value {
    let (s, v) = c.get("/api/v1/me/capabilities").await;
    assert_eq!(s, StatusCode::OK, "{v}");
    v
}

#[tokio::test]
async fn capabilities_follow_each_role_and_scope() {
    let h = start().await;
    let admin = Client::login(&h.base, ADMIN_EMAIL, PASSWORD).await;
    let ws = format!("workspace:{}", h.default_org);
    let (s, cl) = admin.post("/api/v1/clusters", json!({ "name": "lab" })).await;
    assert_eq!(s, StatusCode::CREATED, "{cl}");
    let (s, ag) = admin.post("/api/v1/projects", json!({ "name": "shop" })).await;
    assert_eq!(s, StatusCode::CREATED, "{ag}");
    let (s, ns) = admin
        .post("/api/v1/environments", json!({ "name": "prod", "cluster_id": cl["id"], "project_id": ag["id"] }))
        .await;
    assert_eq!(s, StatusCode::CREATED, "{ns}");
    let cluster = format!("cluster:{}", cl["id"].as_str().unwrap());
    let environment = format!("environment:{}", ns["id"].as_str().unwrap());

    // Platform admin, signed in without MFA: everything (the MFA guardrail
    // is not a reason to hide an entry).
    assert_eq!(caps(&admin).await, only(&FIELDS));

    let wsa = user_with_role(&h, &admin, "wsa@example.com", "workspace_admin", &ws).await;
    assert_eq!(
        caps(&wsa).await,
        only(&[
            "fleet_view",
            "terminal",
            "package_install",
            "software_updates",
            "sftp_access",
            "tunnels",
            "alerts_view",
            "alerts_manage",
            "notifications_manage",
            "users_manage",
            "access_manage",
        ])
    );

    // A role on one cluster still shows the infrastructure sections; the
    // workspace-wide pages stay hidden.
    let op = user_with_role(&h, &admin, "op@example.com", "operator", &cluster).await;
    assert_eq!(
        caps(&op).await,
        only(&["fleet_view", "terminal", "package_install", "software_updates", "sftp_access", "tunnels"])
    );

    let dev = user_with_role(&h, &admin, "dev@example.com", "developer", &environment).await;
    assert_eq!(caps(&dev).await, only(&["fleet_view", "tunnels"]));

    let auditor = user_with_role(&h, &admin, "aud@example.com", "auditor", &ws).await;
    assert_eq!(caps(&auditor).await, only(&["fleet_view", "alerts_view"]));

    let customer = user_with_role(&h, &admin, "cust@example.com", "customer", &environment).await;
    assert_eq!(caps(&customer).await, only(&["fleet_view"]));

    // The gates match the endpoints: what is hidden is refused.
    assert_eq!(dev.get("/api/v1/audit/page").await.0, StatusCode::FORBIDDEN);
    // The console reads the whole log (platform scope), so a workspace
    // auditor does not get the entry.
    assert_eq!(auditor.get("/api/v1/audit/page").await.0, StatusCode::FORBIDDEN);
    assert_eq!(admin.get("/api/v1/audit/page").await.0, StatusCode::OK);
    assert_eq!(op.get("/api/v1/server/settings").await.0, StatusCode::FORBIDDEN);
    assert_eq!(admin.get("/api/v1/server/settings").await.0, StatusCode::OK);

    // Everyone with a role inside the workspace can load the console (its
    // workspace record), and sees the cluster they hold a role on.
    for c in [&op, &dev, &customer, &auditor] {
        assert_eq!(c.get("/api/v1/workspace").await.0, StatusCode::OK);
    }
    let (s, clusters) = op.get("/api/v1/clusters").await;
    assert_eq!(s, StatusCode::OK, "{clusters}");
    assert!(clusters.to_string().contains(&cluster.trim_start_matches("cluster:").to_owned()), "{clusters}");
    // Signed out: no capabilities at all.
    assert_eq!(Client::anonymous(&h.base).get("/api/v1/me/capabilities").await.0, StatusCode::UNAUTHORIZED);
}

/// Endpoints (sites) and the certificates they serve, as each scoped role
/// sees them: rows filtered per cluster, no workspace-wide gate.
#[tokio::test]
async fn scoped_members_see_only_their_rows() {
    use forge_core::model::{Site, TlsCertificate};
    use forge_core::ports::{CertificateRepository, SiteRepository};
    let h = start().await;
    let admin = Client::login(&h.base, ADMIN_EMAIL, PASSWORD).await;
    let state = &h.app.state;
    let ws: forge_core::WorkspaceId = h.default_org.parse().unwrap();
    let (s, ag) = admin.post("/api/v1/projects", json!({ "name": "shop" })).await;
    assert_eq!(s, StatusCode::CREATED, "{ag}");
    let mut ids = Vec::new();
    for name in ["lab", "other"] {
        let (s, cl) = admin.post("/api/v1/clusters", json!({ "name": name })).await;
        assert_eq!(s, StatusCode::CREATED, "{cl}");
        let body = json!({ "name": format!("{name}-env"), "cluster_id": cl["id"], "project_id": ag["id"] });
        let (s, ns) = admin.post("/api/v1/environments", body).await;
        assert_eq!(s, StatusCode::CREATED, "{ns}");
        ids.push((cl["id"].as_str().unwrap().to_owned(), ns["id"].as_str().unwrap().to_owned()));
    }
    let now = state.clock.now();
    let mut certs = Vec::new();
    for (i, (cluster, _)) in ids.iter().enumerate() {
        let cert = TlsCertificate {
            id: forge_core::CertificateId::new(),
            workspace_id: ws,
            name: format!("cert-{i}"),
            domains: vec![format!("site{i}.example.com")],
            issuer: "test".into(),
            not_before: now,
            not_after: now,
            fingerprint: format!("fp{i}"),
            chain_pem: String::new(),
            sealed_key: String::new(),
            created_by: forge_core::Principal::User(forge_core::UserId::new()),
            created_at: now,
            updated_at: now,
        };
        state.store.put_certificate(&cert).await.unwrap();
        let site = Site {
            id: format!("site-{i}"),
            workspace_id: ws,
            name: format!("site-{i}"),
            config: json!({ "spec": { "tls": "uploaded", "cert_id": cert.id.to_string() }, "edges": [], "cluster_id": cluster }),
            edge_status: json!({}),
            created_at: now,
            updated_at: now,
        };
        state.store.put_site(&site).await.unwrap();
        certs.push(cert.id.to_string());
    }
    let op = user_with_role(&h, &admin, "op@example.com", "operator", &format!("cluster:{}", ids[0].0)).await;
    let dev = user_with_role(&h, &admin, "dev@example.com", "developer", &format!("environment:{}", ids[0].1)).await;
    let cust = user_with_role(&h, &admin, "cust@example.com", "customer", &format!("environment:{}", ids[1].1)).await;
    // A user with no role in the workspace at all.
    let outsider = {
        state.identity.create_account(None, Some("out@example.com"), "Out", PASSWORD).await.unwrap();
        Client::login(&h.base, "out@example.com", PASSWORD).await
    };

    let names = |v: &Value| -> Vec<String> {
        v.as_array().unwrap().iter().map(|r| r["name"].as_str().unwrap().to_owned()).collect()
    };
    // Admin sees both.
    assert_eq!(names(&admin.get("/api/v1/sites").await.1), ["site-0", "site-1"]);
    assert_eq!(admin.get("/api/v1/certificates").await.1.as_array().unwrap().len(), 2);

    // Operator on cluster `lab`: its endpoint and certificate only.
    let (s, v) = op.get("/api/v1/sites").await;
    assert_eq!(s, StatusCode::OK, "{v}");
    assert_eq!(names(&v), ["site-0"]);
    let (s, v) = op.get("/api/v1/certificates").await;
    assert_eq!(s, StatusCode::OK, "{v}");
    assert_eq!(names(&v), ["cert-0"]);
    assert_eq!(op.get("/api/v1/sites/site-0").await.0, StatusCode::OK);
    assert_eq!(op.get("/api/v1/sites/site-1").await.0, StatusCode::FORBIDDEN);

    // Developer and customer on an environment: 200, no endpoint (they live
    // on clusters), and nothing of the other cluster.
    for c in [&dev, &cust] {
        for path in ["/api/v1/sites", "/api/v1/certificates"] {
            let (s, v) = c.get(path).await;
            assert_eq!(s, StatusCode::OK, "{path}: {v}");
            assert!(v.as_array().unwrap().is_empty(), "{path}: {v}");
        }
        assert_eq!(c.get("/api/v1/sites/site-0").await.0, StatusCode::FORBIDDEN);
    }

    // Workspace-level reads every member needs.
    for c in [&op, &dev, &cust] {
        assert_eq!(c.get("/api/v1/names/suffix").await.0, StatusCode::OK);
        let (s, v) = c.get("/api/v1/service-ports").await;
        assert_eq!(s, StatusCode::OK, "{v}");
    }

    // No role in the workspace: still refused.
    for path in ["/api/v1/sites", "/api/v1/certificates", "/api/v1/names/suffix", "/api/v1/service-ports"] {
        assert_eq!(outsider.get(path).await.0, StatusCode::FORBIDDEN, "{path}");
    }
}
