//! One deployment is one workspace (D-016). A store with more than one fails
//! closed instead of guessing (#13 inventory).

use crate::common::*;
use forge_core::model::Workspace;
use forge_core::ports::TenancyRepository;
use reqwest::StatusCode;

#[tokio::test]
async fn a_second_workspace_makes_workspace_requests_fail_closed() {
    let h = start().await;
    let admin = Client::login(&h.base, ADMIN_EMAIL, PASSWORD).await;
    assert_eq!(admin.get("/api/v1/settings").await.0, StatusCode::OK, "one workspace: fine");

    let second = Workspace {
        id: forge_core::WorkspaceId::new(),
        name: "Other".into(),
        slug: "other".into(),
        created_at: h.app.state.clock.now(),
    };
    h.app.state.store.create_workspace(&second).await.unwrap();
    let (s, e) = admin.get("/api/v1/settings").await;
    assert_eq!(s, StatusCode::SERVICE_UNAVAILABLE, "{e}");
    assert_eq!(e["title"], "multiple_workspaces", "{e}");
    // Routes without a workspace still answer.
    assert_eq!(admin.get("/api/v1/me").await.0, StatusCode::OK);
}
