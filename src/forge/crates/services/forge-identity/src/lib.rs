//! Identity: who the caller is. Authorization (what they may do) lives in
//! `forge-authz`.
//!
//! * [`password`] — Argon2id hashing.
//! * [`totp`] — RFC 6238 one-time codes (MFA).
//! * [`IdentityService`] — login, sessions (web cookie + CSRF, bearer),
//!   OAuth 2.1 authorization code + PKCE for the desktop/mobile apps.
//! * [`oidc`] — SSO against an external OpenID Connect provider.
//! * [`passkeys`] — passkeys (WebAuthn) for sign-in and step-up, verified
//!   in-house by [`webauthn`] with ring (see DECISIONS.md D-055).

pub mod cbor;
mod connectors;
mod device;
pub mod oidc;
pub mod passkeys;
pub mod password;
pub mod pkce;
mod service;
pub mod totp;
pub mod webauthn;

pub use connectors::{MAX_REDIRECT_URIS, redirect_host, validate_redirect_uri};
pub use device::device_code_hash;
pub use service::{
    AuthenticatedSession, IdentityConfig, IdentityError, IdentityService, LoginOutcome, OAuthClient, TokenPair,
    WebSessionTokens, sign_in_names,
};
