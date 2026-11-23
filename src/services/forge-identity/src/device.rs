//! Device sign-in (RFC 8628) and step-up for bearer sessions (RFC 9470):
//! the token side. Pending device requests live in the server.

use forge_core::UserId;
use forge_core::model::Session;
use forge_secrets::crypto::sha256_hex;

use crate::service::{IdentityError, IdentityService, TokenPair};

impl IdentityService {
    /// Tokens for a device request a person approved in the console. They
    /// never carry MFA, whatever the approving session had: a code someone
    /// was tricked into approving gives no MFA-protected access, and the CLI
    /// steps up when it needs it (security review).
    pub async fn issue_device_tokens(&self, user: UserId) -> Result<TokenPair, IdentityError> {
        let u = self.get_user_checked(user).await?;
        self.issue_bearer(u, false, None, self.clock.now()).await
    }

    /// Step-up for an app or CLI session: after a verified code, the bearer
    /// session is replaced by one with MFA. Its refresh lifetime still counts
    /// from the original sign-in. Connector sessions never step up.
    pub async fn step_up_bearer(&self, session: &Session) -> Result<TokenPair, IdentityError> {
        if session.connection_id.is_some() {
            return Err(IdentityError::InvalidRequest("AI connectors cannot use multi-factor authentication".into()));
        }
        let user = self.get_user_checked(session.user_id).await?;
        let tokens = self.issue_bearer(user, true, None, session.created_at).await?;
        self.sessions.delete_session(&session.id_hash).await?;
        Ok(tokens)
    }

    async fn get_user_checked(&self, user: UserId) -> Result<UserId, IdentityError> {
        let u = self.users.get_user(user).await?;
        if u.disabled {
            return Err(IdentityError::Disabled);
        }
        Ok(u.id)
    }
}

/// SHA-256 of a device code, as the server stores it.
#[must_use]
pub fn device_code_hash(code: &str) -> String {
    sha256_hex(code.as_bytes())
}
