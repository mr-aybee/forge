#![allow(clippy::unwrap_used)]

use std::sync::Arc;
use std::time::Duration;

use forge_core::model::SessionKind;
use forge_core::{Clock, ManualClock, Timestamp};
use forge_identity::oidc::OidcClaims;
use forge_identity::*;
use forge_secrets::{Envelope, LocalKeyProvider};
use forge_store::Store;

struct Fx {
    svc: IdentityService,
    clock: Arc<ManualClock>,
    _dir: tempfile::TempDir,
}

async fn fx(auto_provision: bool) -> Fx {
    let dir = tempfile::tempdir().unwrap();
    let store = Arc::new(Store::open_sqlite(&dir.path().join("i.db")).await.unwrap());
    let clock = Arc::new(ManualClock::new(Timestamp(1_700_000_000_000)));
    let envelope = Envelope::new(Arc::new(LocalKeyProvider::from_bytes(&[1; 32]).unwrap()));
    let config = IdentityConfig { oidc_auto_provision: auto_provision, ..IdentityConfig::default() };
    let svc = IdentityService::new(store.clone(), store, envelope, clock.clone(), config);
    Fx { svc, clock, _dir: dir }
}

const PW: &str = "a long enough password";

#[tokio::test]
async fn password_login_and_web_session_with_csrf() {
    let f = fx(false).await;
    f.svc.create_local_user("Alice@Example.com", "Alice", PW).await.unwrap();
    assert!(matches!(
        f.svc.login_password("alice@example.com", "wrong password!!", None).await,
        Err(IdentityError::InvalidCredentials)
    ));
    assert!(matches!(
        f.svc.login_password("nobody@example.com", PW, None).await,
        Err(IdentityError::InvalidCredentials)
    ));

    let LoginOutcome::Authenticated { user, mfa } = f.svc.login_password("alice@example.com", PW, None).await.unwrap();
    assert!(!mfa);
    let tokens = f.svc.create_web_session(user.id, mfa).await.unwrap();
    let s = f.svc.authenticate(&tokens.session_token, SessionKind::Web).await.unwrap().unwrap();
    assert!(IdentityService::verify_csrf(&s, Some(&tokens.csrf_token)));
    assert!(!IdentityService::verify_csrf(&s, Some("forged")));
    assert!(!IdentityService::verify_csrf(&s, None));
    // A web cookie cannot be used as a bearer token.
    assert!(f.svc.authenticate(&tokens.session_token, SessionKind::Bearer).await.unwrap().is_none());

    f.svc.logout(&tokens.session_token).await.unwrap();
    assert!(f.svc.authenticate(&tokens.session_token, SessionKind::Web).await.unwrap().is_none());
}

#[tokio::test]
async fn sessions_expire() {
    let f = fx(false).await;
    let u = f.svc.create_local_user("b@example.com", "B", PW).await.unwrap();
    let t = f.svc.create_web_session(u.id, false).await.unwrap();
    f.clock.advance(Duration::from_secs(13 * 3600));
    assert!(f.svc.authenticate(&t.session_token, SessionKind::Web).await.unwrap().is_none());
}

#[tokio::test]
async fn weak_password_rejected() {
    let f = fx(false).await;
    assert!(f.svc.create_local_user("c@example.com", "C", "short").await.is_err());
}

#[tokio::test]
async fn totp_enrollment_then_required_at_login() {
    let f = fx(false).await;
    let u = f.svc.create_local_user("d@example.com", "D", PW).await.unwrap();
    let (secret_b32, uri) = f.svc.new_totp_secret(&u);
    assert!(uri.starts_with("otpauth://totp/"));
    let secret = totp::base32_decode(&secret_b32).unwrap();
    let now_s = f.clock.now().0 / 1000;
    let code_num = totp::code_at(&secret, now_s);
    let code = format!("{code_num:06}");
    let wrong = format!("{:06}", (code_num + 500_000) % 1_000_000);

    assert!(f.svc.confirm_totp(u.id, &secret_b32, &wrong).await.is_err());
    f.svc.confirm_totp(u.id, &secret_b32, &code).await.unwrap();

    assert!(matches!(f.svc.login_password("d@example.com", PW, None).await, Err(IdentityError::MfaRequired)));
    assert!(matches!(f.svc.login_password("d@example.com", PW, Some(&wrong)).await, Err(IdentityError::InvalidMfa)));
    let LoginOutcome::Authenticated { mfa, .. } = f.svc.login_password("d@example.com", PW, Some(&code)).await.unwrap();
    assert!(mfa);
}

fn verifier() -> String {
    "dBjftJeZ4CVP-mB92K27uhbUJU1p1r_wW1gFWFOEjXk".into()
}

async fn web_session(f: &Fx) -> AuthenticatedSession {
    let u = f.svc.create_local_user("e@example.com", "E", PW).await.unwrap();
    let t = f.svc.create_web_session(u.id, true).await.unwrap();
    f.svc.authenticate(&t.session_token, SessionKind::Web).await.unwrap().unwrap()
}

#[tokio::test]
async fn oauth_pkce_code_flow_issues_bearer_and_rotates_refresh() {
    let f = fx(false).await;
    let s = web_session(&f).await;
    let challenge = pkce::s256_challenge(&verifier());
    let redirect = "http://127.0.0.1:49152/callback";
    let code = f.svc.authorize_code(&s, "forge-app", redirect, &challenge, "S256").await.unwrap();

    // Wrong verifier fails and burns the code.
    let bad = "x".repeat(43);
    assert!(f.svc.exchange_code(&code, &bad, "forge-app", redirect).await.is_err());
    assert!(f.svc.exchange_code(&code, &verifier(), "forge-app", redirect).await.is_err(), "code is single use");

    let code = f.svc.authorize_code(&s, "forge-app", redirect, &challenge, "S256").await.unwrap();
    let tokens = f.svc.exchange_code(&code, &verifier(), "forge-app", redirect).await.unwrap();
    let bearer = f.svc.authenticate(&tokens.access_token, SessionKind::Bearer).await.unwrap().unwrap();
    assert!(bearer.session.mfa, "MFA state carries over to app tokens");

    let rotated = f.svc.refresh(&tokens.refresh_token).await.unwrap();
    assert!(f.svc.refresh(&tokens.refresh_token).await.is_err(), "old refresh token revoked");
    assert!(f.svc.authenticate(&tokens.access_token, SessionKind::Bearer).await.unwrap().is_none());
    assert!(f.svc.authenticate(&rotated.access_token, SessionKind::Bearer).await.unwrap().is_some());
}

#[tokio::test]
async fn oauth_rejects_unregistered_redirect_and_plain_pkce() {
    let f = fx(false).await;
    let s = web_session(&f).await;
    let challenge = pkce::s256_challenge(&verifier());
    assert!(f.svc.authorize_code(&s, "forge-app", "https://evil.example/cb", &challenge, "S256").await.is_err());
    assert!(f.svc.authorize_code(&s, "forge-app", "http://127.0.0.1/callback", &verifier(), "plain").await.is_err());
    assert!(f.svc.authorize_code(&s, "unknown", "http://127.0.0.1/callback", &challenge, "S256").await.is_err());
}

#[tokio::test]
async fn oauth_code_expires() {
    let f = fx(false).await;
    let s = web_session(&f).await;
    let challenge = pkce::s256_challenge(&verifier());
    let code = f.svc.authorize_code(&s, "forge-app", "orbynforge://auth/callback", &challenge, "S256").await.unwrap();
    f.clock.advance(Duration::from_secs(120));
    assert!(f.svc.exchange_code(&code, &verifier(), "forge-app", "orbynforge://auth/callback").await.is_err());
}

fn claims(email: &str, verified: bool) -> OidcClaims {
    OidcClaims {
        issuer: "https://idp.example".into(),
        subject: "sub-1".into(),
        email: Some(email.into()),
        email_verified: verified,
        name: Some("SSO User".into()),
    }
}

#[tokio::test]
async fn oidc_links_existing_user_by_verified_email() {
    let f = fx(false).await;
    let u = f.svc.create_local_user("sso@example.com", "S", PW).await.unwrap();
    assert!(f.svc.login_oidc(&claims("sso@example.com", false)).await.is_err(), "unverified email refused");
    let linked = f.svc.login_oidc(&claims("sso@example.com", true)).await.unwrap();
    assert_eq!(linked.id, u.id);
    // Subsequent logins match by subject even if the email changes.
    let again = f.svc.login_oidc(&claims("changed@example.com", true)).await.unwrap();
    assert_eq!(again.id, u.id);
}

#[tokio::test]
async fn oidc_unknown_user_needs_auto_provisioning() {
    let off = fx(false).await;
    assert!(off.svc.login_oidc(&claims("new@example.com", true)).await.is_err());
    let on = fx(true).await;
    let u = on.svc.login_oidc(&claims("new@example.com", true)).await.unwrap();
    assert_eq!(u.email.as_deref(), Some("new@example.com"));
    assert!(u.password_hash.is_none());
}

#[tokio::test]
async fn username_only_account_signs_in_and_can_add_an_email_later() {
    let f = fx(false).await;
    let u = f.svc.create_account(Some("Admin"), None, "", PW).await.unwrap();
    assert_eq!(u.username.as_deref(), Some("admin"));
    assert_eq!(u.email, None);
    assert_eq!(u.display_name, "admin");
    assert!(f.svc.login_password("admin", PW, None).await.is_ok());
    assert!(f.svc.login_password("ADMIN ", PW, None).await.is_ok());

    let u = f.svc.update_profile(&u, Some("admin"), Some("Ops@Example.com"), "Operations").await.unwrap();
    assert_eq!(u.email.as_deref(), Some("ops@example.com"));
    // Either sign-in name works now.
    assert!(f.svc.login_password("ops@example.com", PW, None).await.is_ok());
    assert!(f.svc.login_password("admin", PW, None).await.is_ok());
    // One sign-in name must remain.
    assert!(f.svc.update_profile(&u, Some(""), None, "Operations").await.is_err());
}

#[tokio::test]
async fn sign_in_names_are_validated_and_unique() {
    let f = fx(false).await;
    assert!(f.svc.create_account(None, None, "x", PW).await.is_err());
    for bad in ["ab", "-admin", "ad min", "ad@min", "Ädmin"] {
        assert!(f.svc.create_account(Some(bad), None, "x", PW).await.is_err(), "{bad}");
    }
    assert!(f.svc.create_account(None, Some("not-an-email"), "x", PW).await.is_err());
    f.svc.create_account(Some("admin"), Some("a@example.com"), "x", PW).await.unwrap();
    let taken = f.svc.create_account(Some("admin"), None, "x", PW).await;
    assert!(matches!(taken, Err(IdentityError::InvalidRequest(m)) if m.contains("already in use")));
    let taken = f.svc.create_account(Some("other"), Some("A@example.com"), "x", PW).await;
    assert!(matches!(taken, Err(IdentityError::InvalidRequest(m)) if m.contains("already in use")));
}

#[tokio::test]
async fn change_password_checks_the_current_one() {
    let f = fx(false).await;
    let u = f.svc.create_account(Some("admin"), None, "", PW).await.unwrap();
    let new = "another long password";
    assert!(matches!(f.svc.change_password(&u, "wrong password!!", new).await, Err(IdentityError::InvalidCredentials)));
    assert!(f.svc.change_password(&u, PW, "short").await.is_err());
    assert!(f.svc.change_password(&u, PW, PW).await.is_err());
    f.svc.change_password(&u, PW, new).await.unwrap();
    assert!(f.svc.login_password("admin", PW, None).await.is_err());
    assert!(f.svc.login_password("admin", new, None).await.is_ok());
}

/// Security review: a seen authenticator code could be replayed for about
/// 90 seconds. Each code (time step) now works once per user.
#[tokio::test]
async fn an_authenticator_code_works_only_once() {
    let f = fx(false).await;
    let u = f.svc.create_local_user("r@example.com", "R", PW).await.unwrap();
    let (secret_b32, _) = f.svc.new_totp_secret(&u);
    let secret = totp::base32_decode(&secret_b32).unwrap();
    let code_at = |s: i64| format!("{:06}", totp::code_at(&secret, s));
    let now_s = f.clock.now().0 / 1000;
    f.svc.confirm_totp(u.id, &secret_b32, &code_at(now_s)).await.unwrap();

    let code = code_at(now_s);
    assert!(f.svc.login_password("r@example.com", PW, Some(&code)).await.is_ok());
    assert!(
        matches!(f.svc.login_password("r@example.com", PW, Some(&code)).await, Err(IdentityError::InvalidMfa)),
        "the same code again is refused"
    );
    let u = f.svc.login_password("r@example.com", PW, Some(&code_at(now_s + 30))).await;
    assert!(u.is_ok(), "the next code works");
    let LoginOutcome::Authenticated { user, .. } = u.unwrap();
    assert!(f.svc.verify_step_up(&user, &code_at(now_s + 30)).is_err(), "step-up cannot reuse it either");
    assert!(f.svc.verify_step_up(&user, &code).is_err(), "nor an older code within the drift window");
    f.clock.advance(Duration::from_secs(60));
    assert!(f.svc.verify_step_up(&user, &code_at(now_s + 60)).is_ok());
}

/// Security review: refreshing restarted the 30 days each time, so a stolen
/// refresh token used once a month never expired.
#[tokio::test]
async fn refresh_tokens_expire_counted_from_the_sign_in() {
    let f = fx(false).await;
    let s = web_session(&f).await;
    let challenge = pkce::s256_challenge(&verifier());
    let redirect = "http://127.0.0.1:49152/callback";
    let code = f.svc.authorize_code(&s, "forge-app", redirect, &challenge, "S256").await.unwrap();
    let mut tokens = f.svc.exchange_code(&code, &verifier(), "forge-app", redirect).await.unwrap();
    // Refresh every 20 days: the first one works, but 40 days after the
    // sign-in the 30-day lifetime is over, however recently it was renewed.
    f.clock.advance(Duration::from_secs(20 * 24 * 3600));
    tokens = f.svc.refresh(&tokens.refresh_token).await.unwrap();
    f.clock.advance(Duration::from_secs(20 * 24 * 3600));
    assert!(
        matches!(f.svc.refresh(&tokens.refresh_token).await, Err(IdentityError::InvalidGrant(_))),
        "40 days after the sign-in"
    );
}
