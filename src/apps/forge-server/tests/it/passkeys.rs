//! Passkeys end to end over the API, with a software authenticator (P-256).
#![allow(clippy::unwrap_used)]

use crate::common::{ADMIN_EMAIL, Client, PASSWORD, start, user_with_role};
use reqwest::{Method, StatusCode};
use ring::rand::SystemRandom;
use ring::signature::{ECDSA_P256_SHA256_ASN1_SIGNING, EcdsaKeyPair, KeyPair};
use serde_json::{Value, json};

fn b64(d: &[u8]) -> String {
    forge_secrets::crypto::b64url(d)
}
fn sha(d: &[u8]) -> Vec<u8> {
    forge_secrets::crypto::sha256(d).to_vec()
}
fn head(major: u8, n: usize) -> Vec<u8> {
    if n < 24 {
        vec![(major << 5) | u8::try_from(n).unwrap()]
    } else {
        vec![(major << 5) | 24, u8::try_from(n).unwrap()]
    }
}
fn bstr(b: &[u8]) -> Vec<u8> {
    let mut v = head(2, b.len());
    v.extend(b);

    v
}
fn tstr(s: &str) -> Vec<u8> {
    let mut v = head(3, s.len());
    v.extend(s.as_bytes());

    v
}
fn int(i: i64) -> Vec<u8> {
    if i >= 0 { head(0, usize::try_from(i).unwrap()) } else { head(1, usize::try_from(-1 - i).unwrap()) }
}

/// A software authenticator holding one P-256 credential.
struct Authenticator {
    key: EcdsaKeyPair,
    id: Vec<u8>,
    count: u32,
    rp_id: String,
    origin: String,
}

impl Authenticator {
    fn new(base: &str) -> Self {
        let rng = SystemRandom::new();
        let pk8 = EcdsaKeyPair::generate_pkcs8(&ECDSA_P256_SHA256_ASN1_SIGNING, &rng).unwrap();
        let u = url::Url::parse(base).unwrap();

        Self {
            key: EcdsaKeyPair::from_pkcs8(&ECDSA_P256_SHA256_ASN1_SIGNING, pk8.as_ref(), &rng).unwrap(),
            id: b"software-credential-1".to_vec(),
            count: 0,
            rp_id: u.host_str().unwrap().to_owned(),
            origin: u.origin().ascii_serialization(),
        }
    }

    fn client_data(&self, kind: &str, challenge: &str) -> Vec<u8> {
        json!({ "type": kind, "challenge": challenge, "origin": self.origin, "crossOrigin": false })
            .to_string()
            .into_bytes()
    }

    fn auth_data(&self, flags: u8, with_key: bool) -> Vec<u8> {
        let mut d = sha(self.rp_id.as_bytes());
        d.push(flags);
        d.extend(self.count.to_be_bytes());

        if with_key {
            let p = self.key.public_key().as_ref();
            d.extend([0u8; 16]);
            d.extend(u16::try_from(self.id.len()).unwrap().to_be_bytes());
            d.extend(&self.id);
            let mut m = head(5, 5);

            for (k, v) in [(1, int(2)), (3, int(-7)), (-1, int(1)), (-2, bstr(&p[1..33])), (-3, bstr(&p[33..65]))] {
                m.extend(int(k));
                m.extend(v);
            }

            d.extend(m);
        }

        d
    }

    fn create(&self, options: &Value) -> Value {
        let cd = self.client_data("webauthn.create", options["challenge"].as_str().unwrap());
        let mut att = head(5, 3);
        att.extend(tstr("fmt"));
        att.extend(tstr("none"));
        att.extend(tstr("attStmt"));
        att.extend(head(5, 0));
        att.extend(tstr("authData"));
        att.extend(bstr(&self.auth_data(0x45, true)));

        json!({ "client_data_json": b64(&cd), "attestation_object": b64(&att), "name": "Test key" })
    }

    fn get(&mut self, options: &Value) -> Value {
        self.count += 1;
        let cd = self.client_data("webauthn.get", options["challenge"].as_str().unwrap());
        let auth = self.auth_data(0x05, false);
        let mut msg = auth.clone();
        msg.extend(sha(&cd));
        let sig = self.key.sign(&SystemRandom::new(), &msg).unwrap();

        json!({
            "credential_id": b64(&self.id),
            "client_data_json": b64(&cd),
            "authenticator_data": b64(&auth),
            "signature": b64(sig.as_ref()),
        })
    }
}

#[tokio::test]
async fn passkeys_register_sign_in_and_step_up() {
    let h = start().await;
    let mut admin = Client::login(&h.base, ADMIN_EMAIL, PASSWORD).await;
    let mut auth = Authenticator::new(&h.base);

    // Register.
    let (s, r) = admin.post("/api/v1/me/passkeys/register/begin", json!({})).await;
    assert_eq!(s, StatusCode::UNAUTHORIZED, "no MFA yet: the current password is required: {r}");
    let (s, r) = admin.post("/api/v1/me/passkeys/register/begin", json!({"password": "wrong password here"})).await;
    assert_eq!(s, StatusCode::UNAUTHORIZED, "{r}");
    let (s, opts) = admin.post("/api/v1/me/passkeys/register/begin", json!({"password": PASSWORD})).await;
    assert_eq!(s, StatusCode::OK, "{opts}");
    assert_eq!(opts["authenticatorSelection"]["userVerification"], "required");
    let created = auth.create(&opts);
    let (s, p) = admin.post("/api/v1/me/passkeys/register/finish", created.clone()).await;
    assert_eq!(s, StatusCode::CREATED, "{p}");
    assert!(p.get("public_key").is_none(), "no key material in responses");
    let (s, _) = admin.post("/api/v1/me/passkeys/register/finish", created).await;
    assert_eq!(s, StatusCode::UNAUTHORIZED, "a challenge is used once");

    // Step up with the passkey instead of a code: the session becomes MFA.
    let (_, me) = admin.get("/api/v1/me").await;
    assert_eq!(me["mfa"], false);
    let (_, opts) = admin.post("/api/v1/auth/step-up/passkey/begin", json!({})).await;
    assert_eq!(opts["allowCredentials"][0]["id"], b64(&auth.id));
    let (s, r) = admin.post("/api/v1/auth/step-up/passkey/finish", auth.get(&opts)).await;
    assert_eq!(s, StatusCode::OK, "{r}");
    admin.csrf = r["csrf_token"].as_str().map(str::to_owned);
    let (_, me) = admin.get("/api/v1/me").await;
    assert_eq!(me["mfa"], true);

    // Passwordless sign-in in a fresh client.
    let anon = Client::anonymous(&h.base);
    let (_, opts) = anon.post("/api/v1/auth/passkey/begin", json!({})).await;
    let assertion = auth.get(&opts);
    let (s, r) = anon.post("/api/v1/auth/passkey/finish", assertion.clone()).await;
    assert_eq!(s, StatusCode::OK, "{r}");
    assert_eq!(r["mfa"], true);
    let (s, _) = anon.post("/api/v1/auth/passkey/finish", assertion).await;
    assert_eq!(s, StatusCode::UNAUTHORIZED, "replay refused");

    // The same assertion sent twice at once: exactly one session.
    let (_, opts) = anon.post("/api/v1/auth/passkey/begin", json!({})).await;
    let a = auth.get(&opts);
    let (c1, c2) = (Client::anonymous(&h.base), Client::anonymous(&h.base));
    let (r1, r2) =
        tokio::join!(c1.post("/api/v1/auth/passkey/finish", a.clone()), c2.post("/api/v1/auth/passkey/finish", a));
    assert_eq!([r1.0, r2.0].iter().filter(|s| **s == StatusCode::OK).count(), 1, "single use under a race");

    // A response for another origin is refused.
    let (_, opts) = anon.post("/api/v1/auth/passkey/begin", json!({})).await;
    let mut evil = Authenticator::new(&h.base);
    evil.key = std::mem::replace(
        &mut auth.key,
        EcdsaKeyPair::from_pkcs8(
            &ECDSA_P256_SHA256_ASN1_SIGNING,
            EcdsaKeyPair::generate_pkcs8(&ECDSA_P256_SHA256_ASN1_SIGNING, &SystemRandom::new()).unwrap().as_ref(),
            &SystemRandom::new(),
        )
        .unwrap(),
    );
    evil.origin = "https://evil.example.com".into();
    evil.count = 10;
    let (s, _) = anon.post("/api/v1/auth/passkey/finish", evil.get(&opts)).await;
    assert_eq!(s, StatusCode::UNAUTHORIZED);

    // Recovery by an administrator, with MFA.
    let (_, list) = admin.get("/api/v1/me/passkeys").await;
    assert_eq!(list.as_array().unwrap().len(), 1);
    let me_id = me["id"].as_str().or_else(|| me["user"]["id"].as_str()).unwrap().to_owned();

    // A workspace admin cannot strip another user's passkeys.
    let wsa =
        user_with_role(&h, &admin, "wsa@example.com", "workspace_admin", &format!("workspace:{}", h.default_org)).await;
    let (s, r) = wsa.call(Method::DELETE, &format!("/api/v1/users/{me_id}/passkeys"), None).await;
    assert_eq!(s, StatusCode::FORBIDDEN, "{r}");
    assert_eq!(admin.get("/api/v1/me/passkeys").await.1.as_array().unwrap().len(), 1);
    let (s, r) = admin.call(Method::DELETE, &format!("/api/v1/users/{me_id}/passkeys"), None).await;
    assert_eq!(s, StatusCode::OK, "{r}");
    assert_eq!(r["removed"], 1);

    // Recovery also signed that user out everywhere (this session included).
    let (s, _) = admin.get("/api/v1/me/passkeys").await;
    assert_eq!(s, StatusCode::UNAUTHORIZED);
    let admin = Client::login(&h.base, ADMIN_EMAIL, PASSWORD).await;
    let (_, list) = admin.get("/api/v1/me/passkeys").await;
    assert!(list.as_array().unwrap().is_empty());

    let (_, audit) = admin.get("/api/v1/audit?limit=200").await;
    let text = audit.to_string();
    assert!(text.contains("auth.passkey.add") && text.contains("auth.passkey.remove_all"));
}
