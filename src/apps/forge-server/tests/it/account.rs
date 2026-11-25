//! The installer's first account: username `admin` without an email, a
//! generated password saved to `data_dir`, and self-service profile and
//! password changes.
#![allow(clippy::unwrap_used)]

use crate::common::{Client, PASSWORD, login_body, test_config};
use forge_server::bootstrap::{INITIAL_PASSWORD_FILE, init_admin};
use forge_server::{App, Services};
use reqwest::{Method, StatusCode};
use serde_json::json;
use tokio::net::TcpListener;

#[tokio::test]
async fn username_admin_signs_in_edits_profile_and_rotates_the_generated_password() {
    let dir = tempfile::tempdir().unwrap();
    let api = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let agent = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let mut config = test_config(dir.path());
    config.public_url = Some(format!("http://{}", api.local_addr().unwrap()));
    let state = Services::build(config).await.unwrap();
    // What install.sh does: save the generated password, then init.
    let saved = dir.path().join(INITIAL_PASSWORD_FILE);
    std::fs::write(&saved, PASSWORD).unwrap();
    init_admin(&state, Some("admin"), None, "Administrator", PASSWORD, true).await.unwrap();
    let app = App::start_with(state, api, agent).unwrap();
    let base = format!("http://{}", app.api_addr);

    // Sign in with the username (the `login` field; `email` still works too).
    let mut c = Client::anonymous(&base);
    let (s, b) = c.post("/api/v1/auth/login", login_body(&base, "admin", PASSWORD).await).await;
    assert_eq!(s, StatusCode::OK, "{b}");
    assert_eq!(b["user"]["username"], "admin");
    assert!(b["user"]["email"].is_null());
    c.csrf = b["csrf_token"].as_str().map(str::to_owned);

    // A display name alone needs no proof of identity.
    let (s, b) =
        c.call(Method::PATCH, "/api/v1/me", Some(json!({ "username": "admin", "display_name": "Admin" }))).await;
    assert_eq!(s, StatusCode::OK, "{b}");
    // A sign-in name needs the current password (#13 inventory): a stolen
    // session must not be able to take the account over.
    let add_email = json!({ "username": "admin", "email": "Ops@Example.com", "display_name": "Ops" });
    let (s, b) = c.call(Method::PATCH, "/api/v1/me", Some(add_email.clone())).await;
    assert_eq!((s, b["title"].as_str()), (StatusCode::UNAUTHORIZED, Some("password_required")), "{b}");
    // Add an email later from the console, with the password.
    let mut with_pw = add_email;
    with_pw["current_password"] = json!(PASSWORD);
    let (s, b) = c.call(Method::PATCH, "/api/v1/me", Some(with_pw)).await;
    assert_eq!(s, StatusCode::OK, "{b}");
    assert_eq!(b["user"]["email"], "ops@example.com");
    let other = Client::login(&base, "ops@example.com", PASSWORD).await;
    let (_, me) = other.get("/api/v1/me").await;
    assert_eq!(me["user"]["username"], "admin");

    // A wrong current password is refused and the saved file stays.
    let new = "a brand new long password";
    let (s, _) =
        c.post("/api/v1/me/password", json!({ "current_password": "not the password", "new_password": new })).await;
    assert_eq!(s, StatusCode::UNAUTHORIZED);
    assert!(saved.exists());

    // Changing it removes the installer's copy.
    let other = Client::login(&base, "admin", PASSWORD).await;
    let (s, b) = c.post("/api/v1/me/password", json!({ "current_password": PASSWORD, "new_password": new })).await;
    assert_eq!(s, StatusCode::NO_CONTENT, "{b}");
    assert!(!saved.exists(), "the generated password file should be deleted");
    // Security review: every other sign-in ends; the browser that changed
    // the password gets a fresh session and stays signed in.
    let (s, _) = other.get("/api/v1/me").await;
    assert_eq!(s, StatusCode::UNAUTHORIZED, "the other session ended");
    let (s, _) = c.get("/api/v1/me").await;
    assert_eq!(s, StatusCode::OK, "the changing session continues");
    Client::login(&base, "admin", new).await;
    let (s, _) = Client::anonymous(&base).post("/api/v1/auth/login", login_body(&base, "admin", PASSWORD).await).await;
    assert_eq!(s, StatusCode::UNAUTHORIZED);

    // Both changes are in the audit log.
    let admin = Client::login(&base, "admin", new).await;
    let (_, audit) = admin.get("/api/v1/audit?limit=50").await;
    let text = serde_json::to_string(&audit).unwrap();
    assert!(text.contains("user.password.change"), "audit has the password change");
    assert!(text.contains("user.profile.update"), "audit has the profile update");
    app.stop().await;
}
