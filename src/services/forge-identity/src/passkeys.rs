//! Passkey flows (D-055): register, passwordless sign-in and step-up.
//!
//! Each flow is two calls. `*_begin` creates a random 32-byte challenge,
//! stores only its SHA-256 hash with a five-minute expiry and returns the
//! WebAuthn options for the browser. `*_finish` reads the challenge out of
//! the browser's client data, takes the stored challenge (single use: it is
//! deleted whatever the outcome), checks its purpose, user and expiry, then
//! verifies the response with [`crate::webauthn`].
//!
//! A passkey always requires user verification (PIN or biometrics), so a
//! passkey sign-in or step-up yields an MFA-verified session, like a
//! password plus a one-time code.

use std::sync::Arc;
use std::time::Duration;

use forge_core::model::User;
use forge_core::passkey::{Passkey, PasskeyRepository, purpose};
use forge_core::ports::UserRepository;
use forge_core::{Clock, DomainError, UserId};
use serde::Deserialize;
use serde_json::{Value, json};

use crate::webauthn::{self, Assertion, PublicKey, RelyingParty, StoredCredential, WebAuthnError};

/// How long a challenge is valid.
pub const CHALLENGE_TTL: Duration = Duration::from_secs(300);
/// Passkeys one person may register.
pub const MAX_PASSKEYS: usize = 20;

#[derive(Debug, thiserror::Error)]
pub enum PasskeyError {
    #[error("{0}")]
    WebAuthn(#[from] WebAuthnError),
    #[error("this passkey sign-in expired or was already used; try again")]
    Challenge,
    #[error("unknown passkey")]
    UnknownCredential,
    #[error("the account is disabled")]
    Disabled,
    #[error("{0}")]
    Invalid(String),
    #[error(transparent)]
    Domain(#[from] DomainError),
}

/// The browser's response to `navigator.credentials.create()`, base64url
/// fields as the console sends them.
#[derive(Debug, Deserialize)]
pub struct RegistrationResponse {
    pub client_data_json: String,
    pub attestation_object: String,
    #[serde(default)]
    pub name: Option<String>,
}

/// The browser's response to `navigator.credentials.get()`.
#[derive(Debug, Deserialize)]
pub struct AssertionResponse {
    pub credential_id: String,
    pub client_data_json: String,
    pub authenticator_data: String,
    pub signature: String,
}

#[derive(Debug, Clone)]
pub struct PasskeyService {
    repo: Arc<dyn PasskeyRepository>,
    users: Arc<dyn UserRepository>,
    clock: Arc<dyn Clock>,
    rp_name: String,
}

fn b64(data: &[u8]) -> String {
    forge_secrets::crypto::b64url(data)
}

fn unb64(s: &str, what: &str) -> Result<Vec<u8>, PasskeyError> {
    if s.len() > 64 * 1024 {
        return Err(PasskeyError::Invalid(format!("{what} too large")));
    }
    forge_secrets::crypto::b64url_decode(s).ok_or_else(|| PasskeyError::Invalid(format!("{what}: base64url")))
}

fn hash(challenge: &[u8]) -> String {
    hex_lower(&forge_secrets::crypto::sha256(challenge))
}

fn hex_lower(b: &[u8]) -> String {
    b.iter().map(|x| format!("{x:02x}")).collect()
}

/// The challenge a browser response carries (before verification).
fn challenge_of(client_data_json: &[u8]) -> Result<Vec<u8>, PasskeyError> {
    let v: Value =
        serde_json::from_slice(client_data_json).map_err(|_| PasskeyError::Invalid("client data JSON".into()))?;
    let c = v.get("challenge").and_then(Value::as_str).ok_or(PasskeyError::Challenge)?;
    forge_secrets::crypto::b64url_decode(c).ok_or(PasskeyError::Challenge)
}

impl PasskeyService {
    #[must_use]
    pub fn new(repo: Arc<dyn PasskeyRepository>, users: Arc<dyn UserRepository>, clock: Arc<dyn Clock>) -> Self {
        Self { repo, users, clock, rp_name: "Orbyn Forge".into() }
    }

    async fn new_challenge(&self, user: Option<UserId>, purpose: &str) -> Result<Vec<u8>, PasskeyError> {
        use ring::rand::SecureRandom;
        let mut c = vec![0u8; 32];
        ring::rand::SystemRandom::new().fill(&mut c).map_err(|_| DomainError::Internal("no randomness".into()))?;
        let expires = self.clock.now().plus(CHALLENGE_TTL);
        self.repo.put_challenge(&hash(&c), user, purpose, expires).await?;
        Ok(c)
    }

    /// Takes the challenge in `client_data_json` (single use) and checks it
    /// was issued for `purpose` (and `user`, when given) and is unexpired.
    async fn take(
        &self,
        client_data_json: &[u8],
        purpose: &str,
        user: Option<UserId>,
    ) -> Result<(Vec<u8>, Option<UserId>), PasskeyError> {
        let c = challenge_of(client_data_json)?;
        let (owner, p, expires) = self.repo.take_challenge(&hash(&c)).await?.ok_or(PasskeyError::Challenge)?;
        if p != purpose || expires <= self.clock.now() || (user.is_some() && owner != user) {
            return Err(PasskeyError::Challenge);
        }
        Ok((c, owner))
    }

    fn allow_list(keys: &[Passkey]) -> Vec<Value> {
        keys.iter().map(|k| json!({ "type": "public-key", "id": k.credential_id })).collect()
    }

    /// Options for registering a new passkey for `user` on `rp` (the
    /// console address of this request: a passkey is scoped to its name).
    pub async fn register_begin(&self, user: &User, rp: &RelyingParty) -> Result<Value, PasskeyError> {
        let existing = self.repo.list_passkeys(user.id).await?;
        if existing.len() >= MAX_PASSKEYS {
            return Err(PasskeyError::Invalid(format!("at most {MAX_PASSKEYS} passkeys per account")));
        }
        let c = self.new_challenge(Some(user.id), purpose::REGISTER).await?;
        let name = user.username.clone().or_else(|| user.email.clone()).unwrap_or_else(|| user.id.to_string());
        Ok(json!({
            "challenge": b64(&c),
            "rp": { "id": rp.id, "name": self.rp_name },
            "user": { "id": b64(user.id.to_string().as_bytes()), "name": name, "displayName": user.display_name },
            "pubKeyCredParams": [
                { "type": "public-key", "alg": -7 },
                { "type": "public-key", "alg": -8 },
                { "type": "public-key", "alg": -257 }
            ],
            "timeout": CHALLENGE_TTL.as_millis(),
            "attestation": "none",
            "authenticatorSelection": { "residentKey": "required", "requireResidentKey": true, "userVerification": "required" },
            "excludeCredentials": Self::allow_list(&existing),
        }))
    }

    /// Verifies and stores a new passkey.
    pub async fn register_finish(
        &self,
        user: &User,
        r: &RegistrationResponse,
        rp: &RelyingParty,
    ) -> Result<Passkey, PasskeyError> {
        let cd = unb64(&r.client_data_json, "client data")?;
        let att = unb64(&r.attestation_object, "attestation")?;
        let (challenge, _) = self.take(&cd, purpose::REGISTER, Some(user.id)).await?;
        // Checked again here: several begins could otherwise exceed the limit.
        if self.repo.list_passkeys(user.id).await?.len() >= MAX_PASSKEYS {
            return Err(PasskeyError::Invalid(format!("at most {MAX_PASSKEYS} passkeys per account")));
        }
        let cred = webauthn::verify_registration(rp, &challenge, &cd, &att)?;
        let credential_id = b64(&cred.credential_id);
        if self.repo.passkey_by_credential(&credential_id).await?.is_some() {
            return Err(PasskeyError::Invalid("this passkey is already registered".into()));
        }
        let name = r.name.as_deref().map(str::trim).filter(|n| !n.is_empty()).unwrap_or("Passkey");
        if name.chars().count() > 60 || name.chars().any(char::is_control) {
            return Err(PasskeyError::Invalid("name: up to 60 characters".into()));
        }
        let p = Passkey {
            id: forge_secrets::crypto::random_token::<12>(),
            user_id: user.id,
            credential_id,
            public_key: serde_json::to_string(&cred.public_key).map_err(|e| DomainError::Internal(e.to_string()))?,
            sign_count: cred.sign_count,
            name: name.to_owned(),
            created_at: self.clock.now(),
            last_used_at: None,
        };
        self.repo.add_passkey(&p).await?;
        Ok(p)
    }

    /// Options for a passwordless sign-in (discoverable credentials).
    pub async fn login_begin(&self, rp: &RelyingParty) -> Result<Value, PasskeyError> {
        let c = self.new_challenge(None, purpose::LOGIN).await?;
        Ok(json!({
            "challenge": b64(&c),
            "rpId": rp.id,
            "timeout": CHALLENGE_TTL.as_millis(),
            "userVerification": "required",
            "allowCredentials": [],
        }))
    }

    /// Verifies a passwordless sign-in; returns the user (MFA-verified) and
    /// the passkey used. The signature is checked before the account state,
    /// so failures reveal nothing about accounts.
    pub async fn login_finish(&self, a: &AssertionResponse, rp: &RelyingParty) -> Result<(User, String), PasskeyError> {
        let cd = unb64(&a.client_data_json, "client data")?;
        let (challenge, _) = self.take(&cd, purpose::LOGIN, None).await?;
        let key = self.repo.passkey_by_credential(&a.credential_id).await?.ok_or(PasskeyError::UnknownCredential)?;
        self.verify(rp, &challenge, &key, &cd, a).await?;
        let user = self.users.get_user(key.user_id).await?;
        if user.disabled {
            return Err(PasskeyError::Disabled);
        }
        Ok((user, key.id))
    }

    /// Options for re-verifying `user` with one of their passkeys.
    pub async fn step_up_begin(&self, user: &User, rp: &RelyingParty) -> Result<Value, PasskeyError> {
        let keys = self.repo.list_passkeys(user.id).await?;
        if keys.is_empty() {
            return Err(PasskeyError::Invalid(
                "no passkey registered; add one in Profile → Password & security".into(),
            ));
        }
        let c = self.new_challenge(Some(user.id), purpose::STEP_UP).await?;
        Ok(json!({
            "challenge": b64(&c),
            "rpId": rp.id,
            "timeout": CHALLENGE_TTL.as_millis(),
            "userVerification": "required",
            "allowCredentials": Self::allow_list(&keys),
        }))
    }

    /// Verifies a step-up: the credential must be one of `user`'s. Returns
    /// the passkey used.
    pub async fn step_up_finish(
        &self,
        user: &User,
        a: &AssertionResponse,
        rp: &RelyingParty,
    ) -> Result<String, PasskeyError> {
        let cd = unb64(&a.client_data_json, "client data")?;
        let (challenge, _) = self.take(&cd, purpose::STEP_UP, Some(user.id)).await?;
        let key = self
            .repo
            .passkey_by_credential(&a.credential_id)
            .await?
            .filter(|k| k.user_id == user.id)
            .ok_or(PasskeyError::UnknownCredential)?;
        self.verify(rp, &challenge, &key, &cd, a).await?;
        Ok(key.id)
    }

    async fn verify(
        &self,
        rp: &RelyingParty,
        challenge: &[u8],
        key: &Passkey,
        cd: &[u8],
        a: &AssertionResponse,
    ) -> Result<(), PasskeyError> {
        let public: PublicKey =
            serde_json::from_str(&key.public_key).map_err(|e| DomainError::Internal(e.to_string()))?;
        let id = unb64(&a.credential_id, "credential id")?;
        let auth = unb64(&a.authenticator_data, "authenticator data")?;
        let sig = unb64(&a.signature, "signature")?;
        let stored = StoredCredential { credential_id: &id, public_key: &public, sign_count: key.sign_count };
        let count = webauthn::verify_assertion(
            rp,
            challenge,
            &stored,
            &Assertion { credential_id: &id, client_data_json: cd, authenticator_data: &auth, signature: &sig },
        )?;
        // Conditional update: a concurrent sign-in with a higher counter wins.
        if !self.repo.touch_passkey(&key.id, count, self.clock.now()).await? {
            return Err(WebAuthnError::CounterRegression.into());
        }
        Ok(())
    }

    pub async fn list(&self, user: UserId) -> Result<Vec<Passkey>, PasskeyError> {
        Ok(self.repo.list_passkeys(user).await?)
    }

    pub async fn rename(&self, user: UserId, id: &str, name: &str) -> Result<bool, PasskeyError> {
        let name = name.trim();
        if name.is_empty() || name.chars().count() > 60 || name.chars().any(char::is_control) {
            return Err(PasskeyError::Invalid("name: 1 to 60 characters".into()));
        }
        Ok(self.repo.rename_passkey(id, user, name).await?)
    }

    pub async fn remove(&self, user: UserId, id: &str) -> Result<bool, PasskeyError> {
        Ok(self.repo.delete_passkey(id, user).await?)
    }

    /// Recovery: removes every passkey of a user (administrators, CLI).
    pub async fn remove_all(&self, user: UserId) -> Result<u64, PasskeyError> {
        Ok(self.repo.delete_user_passkeys(user).await?)
    }

    pub async fn has_passkeys(&self, user: UserId) -> Result<bool, PasskeyError> {
        Ok(!self.repo.list_passkeys(user).await?.is_empty())
    }
}
