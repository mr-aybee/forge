//! Token storage per context.
//!
//! Default: the OS credential store (Windows Credential Manager, macOS
//! Keychain, Secret Service on Linux). Where none is available (headless
//! servers, containers) or `FORGE_CREDENTIAL_STORE=file` is set, an
//! owner-only file under the config directory is used: that is access
//! control, not encryption. Tokens are bound to the server URL they were
//! issued by and never used for another one.

use serde::{Deserialize, Serialize};

use crate::ctx::{config_dir, write_private};
use crate::error::{CliError, Exit, Result};

const SERVICE: &str = "orbyn-forge-cli";

#[derive(Clone, Serialize, Deserialize)]
pub struct Tokens {
    /// Server the tokens were issued by.
    pub server: String,
    pub access_token: String,
    pub refresh_token: String,
    /// Epoch ms when the access token expires.
    pub expires_at_ms: i64,
}

impl std::fmt::Debug for Tokens {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Tokens")
            .field("server", &self.server)
            .field("expires_at_ms", &self.expires_at_ms)
            .finish_non_exhaustive()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Store {
    Keyring,
    File,
}

fn preferred() -> Store {
    match std::env::var("FORGE_CREDENTIAL_STORE").as_deref() {
        Ok("file") => Store::File,
        _ => Store::Keyring,
    }
}

fn file_path(context: &str) -> Result<std::path::PathBuf> {
    Ok(config_dir()?.join("credentials").join(format!("{context}.json")))
}

/// Saves tokens; returns where they went.
pub fn save(context: &str, t: &Tokens) -> Result<Store> {
    let json = serde_json::to_string(t)?;
    if preferred() == Store::Keyring {
        match keyring::Entry::new(SERVICE, context).and_then(|e| e.set_password(&json)) {
            Ok(()) => {
                // Do not leave an older file copy behind.
                if let Ok(p) = file_path(context) {
                    let _ = std::fs::remove_file(p);
                }
                return Ok(Store::Keyring);
            }
            Err(e) => {
                eprintln!("forge: OS credential store unavailable ({e}); using an owner-only file instead");
            }
        }
    }
    write_private(&file_path(context)?, json.as_bytes())?;
    Ok(Store::File)
}

pub fn load(context: &str) -> Result<Option<Tokens>> {
    if preferred() == Store::Keyring
        && let Ok(entry) = keyring::Entry::new(SERVICE, context)
        && let Ok(json) = entry.get_password()
    {
        return Ok(Some(serde_json::from_str(&json)?));
    }
    match std::fs::read(file_path(context)?) {
        Ok(b) => Ok(Some(serde_json::from_slice(&b)?)),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(e.into()),
    }
}

/// Removes tokens from every store.
pub fn delete(context: &str) -> Result<()> {
    if let Ok(e) = keyring::Entry::new(SERVICE, context) {
        let _ = e.delete_credential();
    }
    match std::fs::remove_file(file_path(context)?) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(e.into()),
    }
}

/// Loads tokens for `context`, refusing ones issued by another server.
pub fn for_server(context: &str, server: &str) -> Result<Option<Tokens>> {
    match load(context)? {
        Some(t) if t.server == server => Ok(Some(t)),
        Some(_) => Err(CliError::new(
            Exit::Auth,
            format!(
                "the stored login for `{context}` belongs to another server; run `forge auth login --context {context}`"
            ),
        )),
        None => Ok(None),
    }
}
