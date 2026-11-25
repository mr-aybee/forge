//! Image CAPTCHA after failed sign-ins (T01): needed from the third attempt,
//! per sign-in name and per address, single use.

use crate::common::*;
use reqwest::StatusCode;
use serde_json::json;

async fn start_captcha() -> Harness {
    start_configured(|c| c.security.captcha_after_failures = 2).await
}

const LOOPBACK: std::net::IpAddr = std::net::IpAddr::V4(std::net::Ipv4Addr::LOCALHOST);

async fn attempt(
    h: &Harness,
    login: &str,
    password: &str,
    captcha: Option<(&str, &str)>,
) -> (StatusCode, serde_json::Value) {
    let mut body = login_body(&h.base, login, password).await;

    if let Some((id, answer)) = captcha {
        body["captcha_id"] = json!(id);
        body["captcha_answer"] = json!(answer);
    }

    Client::anonymous(&h.base).post("/api/v1/auth/login", body).await
}

#[tokio::test]
async fn the_third_attempt_needs_the_picture_and_a_right_answer_signs_in() {
    let h = start_captcha().await;
    let anon = Client::anonymous(&h.base);
    let (_, c) = anon.get(&format!("/api/v1/auth/captcha?login={ADMIN_EMAIL}")).await;
    assert_eq!(c["required"], false, "no failures yet");

    for _ in 0..2 {
        let (s, e) = attempt(&h, ADMIN_EMAIL, "wrong password!!", None).await;
        assert_eq!(s, StatusCode::UNAUTHORIZED, "attempts 1-2 need no picture: {e}");
    }

    // The third attempt, even with the right password, needs the picture.
    let (s, e) = attempt(&h, ADMIN_EMAIL, PASSWORD, None).await;
    assert_eq!(s, StatusCode::BAD_REQUEST, "{e}");
    assert_eq!(e["title"], "captcha_required", "{e}");
    let (s, c) = anon.get(&format!("/api/v1/auth/captcha?login={ADMIN_EMAIL}")).await;
    assert_eq!(s, StatusCode::OK);
    assert_eq!(c["required"], true, "{c}");
    assert!(c["image"].as_str().unwrap().starts_with("data:image/png;base64,iVBOR"), "a PNG");
    assert!(c.get("answer").is_none() && c.get("text").is_none(), "the characters are never sent as text");

    let captchas = &h.app.state.captchas;

    // A wrong answer uses the picture up; so does a reused one.
    let id = captchas.issue_known(LOOPBACK, "AC3D4E");
    let (s, e) = attempt(&h, ADMIN_EMAIL, PASSWORD, Some((&id, "AC3D4F"))).await;
    assert_eq!((s, e["title"].as_str()), (StatusCode::BAD_REQUEST, Some("captcha_failed")), "{e}");
    let (s, e) = attempt(&h, ADMIN_EMAIL, PASSWORD, Some((&id, "AC3D4E"))).await;
    assert_eq!((s, e["title"].as_str()), (StatusCode::BAD_REQUEST, Some("captcha_failed")), "reused: {e}");

    // A right answer (any case) with the right password signs in.
    let id = captchas.issue_known(LOOPBACK, "AC3D4E");
    let (s, e) = attempt(&h, ADMIN_EMAIL, PASSWORD, Some((&id, "ac3d4e"))).await;
    assert_eq!(s, StatusCode::OK, "{e}");
}

#[tokio::test]
async fn switching_names_does_not_skip_it() {
    let h = start_captcha().await;

    // Two failures from this address with two different, unknown names …
    for name in ["nobody-1", "nobody-2"] {
        let (s, _) = attempt(&h, name, "wrong password!!", None).await;
        assert_eq!(s, StatusCode::UNAUTHORIZED);
    }

    // … and a third name from the same address needs the picture.
    let (s, e) = attempt(&h, ADMIN_EMAIL, PASSWORD, None).await;
    assert_eq!((s, e["title"].as_str()), (StatusCode::BAD_REQUEST, Some("captcha_required")), "{e}");
    let (_, c) = Client::anonymous(&h.base).get("/api/v1/auth/captcha?login=someone-new").await;
    assert_eq!(c["required"], true, "{c}");
}
