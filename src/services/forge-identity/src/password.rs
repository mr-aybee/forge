//! Argon2id password hashing (PHC string format).

use argon2::password_hash::{PasswordHash, PasswordHasher, PasswordVerifier, SaltString};
use argon2::{Algorithm, Argon2, Params, Version};

/// Minimum accepted password length.
pub const MIN_LENGTH: usize = 12;

fn hasher() -> Argon2<'static> {
    // OWASP 2024 recommendation: m=19 MiB, t=2, p=1.
    let params = Params::new(19 * 1024, 2, 1, None).unwrap_or_default();
    Argon2::new(Algorithm::Argon2id, Version::V0x13, params)
}

/// Hashes a password with a random salt.
pub fn hash(password: &str) -> Result<String, String> {
    let salt = SaltString::encode_b64(&forge_secrets::crypto::random_bytes::<16>()).map_err(|e| e.to_string())?;
    hasher().hash_password(password.as_bytes(), &salt).map(|h| h.to_string()).map_err(|e| e.to_string())
}

/// Verifies a password against a stored PHC hash (constant time).
#[must_use]
pub fn verify(password: &str, phc: &str) -> bool {
    PasswordHash::new(phc).is_ok_and(|parsed| hasher().verify_password(password.as_bytes(), &parsed).is_ok())
}

/// At most this many Argon2 computations run at once (19 MiB each).
const PARALLEL: usize = 4;
static SLOTS: tokio::sync::Semaphore = tokio::sync::Semaphore::const_new(PARALLEL);

/// [`verify`] on a blocking thread, at most [`PARALLEL`] at a time, so a
/// flood of sign-ins cannot stall the async workers that also run the agent
/// hub and the job engine.
pub async fn verify_blocking(password: &str, phc: &str) -> bool {
    let (password, phc) = (password.to_owned(), phc.to_owned());
    let Ok(_slot) = SLOTS.acquire().await else { return false };
    tokio::task::spawn_blocking(move || verify(&password, &phc)).await.unwrap_or(false)
}

/// [`hash`] on a blocking thread (see [`verify_blocking`]).
pub async fn hash_blocking(password: &str) -> Result<String, String> {
    let password = password.to_owned();
    let _slot = SLOTS.acquire().await.map_err(|e| e.to_string())?;
    tokio::task::spawn_blocking(move || hash(&password)).await.map_err(|e| e.to_string())?
}

/// Basic strength policy. Length beats composition rules (NIST SP 800-63B).
pub fn validate(password: &str) -> Result<(), String> {
    if password.chars().count() < MIN_LENGTH {
        return Err(format!("password must be at least {MIN_LENGTH} characters"));
    }
    Ok(())
}

/// A precomputed hash used to keep login timing constant for unknown users.
pub(crate) fn dummy_hash() -> &'static str {
    static DUMMY: std::sync::OnceLock<String> = std::sync::OnceLock::new();
    DUMMY.get_or_init(|| hash("dummy-password-for-timing").unwrap_or_default())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hash_and_verify() {
        let h = hash("correct horse battery staple").expect("hash");
        assert!(h.starts_with("$argon2id$"));
        assert!(verify("correct horse battery staple", &h));
        assert!(!verify("wrong", &h));
        assert!(!verify("x", "not-a-hash"));
    }

    #[test]
    fn length_policy() {
        assert!(validate("short").is_err());
        assert!(validate("long enough passphrase").is_ok());
    }
}
