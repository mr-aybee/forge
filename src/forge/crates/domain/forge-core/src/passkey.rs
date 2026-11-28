//! Passkeys (WebAuthn credentials) and their single-use challenges (D-055).

use async_trait::async_trait;
use serde::{Deserialize, Serialize};

use crate::{DomainResult, Timestamp, UserId};

/// One registered passkey. Only the public key is stored.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Passkey {
    pub id: String,
    pub user_id: UserId,
    /// base64url credential id.
    pub credential_id: String,
    /// `forge_identity::webauthn::PublicKey` as JSON.
    #[serde(skip_serializing)]
    pub public_key: String,
    pub sign_count: u32,
    /// What the user called it ("MacBook Touch ID").
    pub name: String,
    pub created_at: Timestamp,
    pub last_used_at: Option<Timestamp>,
}

/// What a challenge is for.
pub mod purpose {
    pub const REGISTER: &str = "register";
    pub const LOGIN: &str = "login";
    pub const STEP_UP: &str = "step_up";
}

#[async_trait]
pub trait PasskeyRepository: Send + Sync + std::fmt::Debug {
    async fn add_passkey(&self, p: &Passkey) -> DomainResult<()>;
    async fn list_passkeys(&self, user: UserId) -> DomainResult<Vec<Passkey>>;
    async fn passkey_by_credential(&self, credential_id: &str) -> DomainResult<Option<Passkey>>;
    /// Records a use; `false` if the counter would go backwards.
    async fn touch_passkey(&self, id: &str, sign_count: u32, at: Timestamp) -> DomainResult<bool>;
    async fn rename_passkey(&self, id: &str, user: UserId, name: &str) -> DomainResult<bool>;
    /// Removes one passkey of a user; `false` if there was none.
    async fn delete_passkey(&self, id: &str, user: UserId) -> DomainResult<bool>;
    /// Removes every passkey of a user (recovery); returns how many.
    async fn delete_user_passkeys(&self, user: UserId) -> DomainResult<u64>;
    /// Stores a challenge (by its SHA-256 hash) until `expires_at`.
    async fn put_challenge(
        &self,
        hash: &str,
        user: Option<UserId>,
        purpose: &str,
        expires_at: Timestamp,
    ) -> DomainResult<()>;
    /// Removes and returns a challenge (single use): `(user, purpose,
    /// expires_at)`. Expired challenges are removed too.
    async fn take_challenge(&self, hash: &str) -> DomainResult<Option<(Option<UserId>, String, Timestamp)>>;
}
