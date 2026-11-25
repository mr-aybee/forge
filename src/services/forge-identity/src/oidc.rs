//! OpenID Connect SSO (authorization code + PKCE, confidential client).
//!
//! [`OidcProvider`] is the port; [`OpenIdConnectProvider`] implements it with
//! the `openidconnect` crate. Pending logins (state → nonce + PKCE verifier)
//! are kept server-side in [`PendingLogins`], so nothing sensitive is placed
//! in the browser beyond the opaque `state`.

use std::collections::HashMap;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use async_trait::async_trait;
use openidconnect::core::{CoreAuthenticationFlow, CoreClient, CoreProviderMetadata};
use openidconnect::{
    AuthorizationCode, ClientId, ClientSecret, CsrfToken, IssuerUrl, Nonce, PkceCodeChallenge, PkceCodeVerifier,
    RedirectUrl, Scope, TokenResponse,
};

use crate::IdentityError;

/// Verified claims from an ID token.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OidcClaims {
    pub issuer: String,
    pub subject: String,
    pub email: Option<String>,
    pub email_verified: bool,
    pub name: Option<String>,
}

/// State the server keeps between redirect and callback.
#[derive(Debug, Clone)]
pub struct PendingLogin {
    pub nonce: String,
    pub pkce_verifier: String,
}

/// Port for an external identity provider.
#[async_trait]
pub trait OidcProvider: Send + Sync + std::fmt::Debug {
    /// Returns the authorization URL plus `(state, pending)` to store.
    fn begin(&self) -> (String, String, PendingLogin);
    /// Exchanges the callback code and verifies the ID token.
    async fn complete(&self, code: &str, pending: PendingLogin) -> Result<OidcClaims, IdentityError>;
}

/// Static provider configuration.
#[derive(Debug, Clone)]
pub struct OidcConfig {
    pub issuer_url: String,
    pub client_id: String,
    /// Read from the environment or the secrets store, never from config files in Git.
    pub client_secret: String,
    pub redirect_url: String,
    pub scopes: Vec<String>,
}

/// `openidconnect`-backed provider. Metadata is discovered once at startup.
#[derive(Debug)]
pub struct OpenIdConnectProvider {
    metadata: CoreProviderMetadata,
    config: OidcConfig,
    http: reqwest::Client,
}

impl OpenIdConnectProvider {
    pub async fn discover(config: OidcConfig) -> Result<Self, IdentityError> {
        let http = reqwest::Client::builder()
            // Following redirects here would enable SSRF against internal hosts.
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .map_err(|e| IdentityError::Provider(e.to_string()))?;
        let issuer = IssuerUrl::new(config.issuer_url.clone()).map_err(|e| IdentityError::Provider(e.to_string()))?;
        let metadata = CoreProviderMetadata::discover_async(issuer, &http)
            .await
            .map_err(|e| IdentityError::Provider(e.to_string()))?;

        Ok(Self { metadata, config, http })
    }

    fn redirect(&self) -> Result<RedirectUrl, IdentityError> {
        RedirectUrl::new(self.config.redirect_url.clone()).map_err(|e| IdentityError::Provider(e.to_string()))
    }
}

#[async_trait]
impl OidcProvider for OpenIdConnectProvider {
    fn begin(&self) -> (String, String, PendingLogin) {
        let client = CoreClient::from_provider_metadata(
            self.metadata.clone(),
            ClientId::new(self.config.client_id.clone()),
            Some(ClientSecret::new(self.config.client_secret.clone())),
        );

        let client = match self.redirect() {
            Ok(r) => client.set_redirect_uri(r),
            Err(_) => client,
        };

        let (challenge, verifier) = PkceCodeChallenge::new_random_sha256();
        let mut req =
            client.authorize_url(CoreAuthenticationFlow::AuthorizationCode, CsrfToken::new_random, Nonce::new_random);

        for s in &self.config.scopes {
            req = req.add_scope(Scope::new(s.clone()));
        }

        let (url, state, nonce) = req.set_pkce_challenge(challenge).url();

        (
            url.to_string(),
            state.secret().clone(),
            PendingLogin { nonce: nonce.secret().clone(), pkce_verifier: verifier.secret().clone() },
        )
    }

    async fn complete(&self, code: &str, pending: PendingLogin) -> Result<OidcClaims, IdentityError> {
        let perr = |e: &dyn std::fmt::Display| IdentityError::Provider(e.to_string());
        let client = CoreClient::from_provider_metadata(
            self.metadata.clone(),
            ClientId::new(self.config.client_id.clone()),
            Some(ClientSecret::new(self.config.client_secret.clone())),
        )
        .set_redirect_uri(self.redirect()?);
        let token = client
            .exchange_code(AuthorizationCode::new(code.to_owned()))
            .map_err(|e| perr(&e))?
            .set_pkce_verifier(PkceCodeVerifier::new(pending.pkce_verifier))
            .request_async(&self.http)
            .await
            .map_err(|e| perr(&e))?;
        let id_token = token.id_token().ok_or_else(|| IdentityError::Provider("no id_token returned".into()))?;
        let claims = id_token.claims(&client.id_token_verifier(), &Nonce::new(pending.nonce)).map_err(|e| perr(&e))?;

        Ok(OidcClaims {
            issuer: claims.issuer().as_str().to_owned(),
            subject: claims.subject().as_str().to_owned(),
            email: claims.email().map(|e| e.as_str().to_owned()),
            email_verified: claims.email_verified().unwrap_or(false),
            name: claims.name().and_then(|n| n.get(None)).map(|n| n.as_str().to_owned()),
        })
    }
}

/// Bounded, expiring store of in-flight OIDC logins.
#[derive(Debug, Default)]
pub struct PendingLogins {
    inner: Mutex<HashMap<String, (Instant, PendingLogin)>>,
}

const PENDING_TTL: Duration = Duration::from_secs(600);
const PENDING_MAX: usize = 10_000;

impl PendingLogins {
    pub fn insert(&self, state: String, pending: PendingLogin) -> Result<(), IdentityError> {
        let mut map = self.inner.lock().map_err(|_| IdentityError::Provider("lock poisoned".into()))?;
        map.retain(|_, (t, _)| t.elapsed() < PENDING_TTL);

        if map.len() >= PENDING_MAX {
            return Err(IdentityError::Provider("too many pending logins".into()));
        }

        map.insert(state, (Instant::now(), pending));

        Ok(())
    }

    /// Removes and returns the pending login for `state` (single use).
    pub fn take(&self, state: &str) -> Option<PendingLogin> {
        let mut map = self.inner.lock().ok()?;
        map.remove(state).filter(|(t, _)| t.elapsed() < PENDING_TTL).map(|(_, p)| p)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pending_logins_are_single_use() {
        let p = PendingLogins::default();
        p.insert("s1".into(), PendingLogin { nonce: "n".into(), pkce_verifier: "v".into() }).expect("insert");
        assert!(p.take("s1").is_some());
        assert!(p.take("s1").is_none());
        assert!(p.take("unknown").is_none());
    }
}
