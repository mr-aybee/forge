//! Container registries (D-158): stored with their secret sealed, and turned
//! into Docker's `X-Registry-Auth` for the image being pulled, pushed or
//! deployed. The credentials go into the transient signed command only,
//! never into a stored job payload, the audit log or an API response.

use std::sync::Arc;

use forge_catalog::swarm::RegistryAuth;
use forge_core::registries::{Registry, RegistryKind, RegistryRepository, image_host, normalize_host};
use forge_core::{Clock, DomainError, DomainResult, Principal, RegistryId, WorkspaceId};
use forge_secrets::Envelope;

/// What a registry is created or changed with.
#[derive(Debug, Clone)]
pub struct RegistryInput {
    pub name: String,
    pub kind: RegistryKind,
    pub host: String,
    pub username: String,
    /// Password or token; `None` on update keeps the current one.
    pub secret: Option<String>,
}

#[derive(Debug)]
pub struct RegistryService {
    pub repo: Arc<dyn RegistryRepository>,
    pub envelope: Envelope,
    pub clock: Arc<dyn Clock>,
}

fn aad(id: RegistryId) -> String {
    format!("registry:{id}")
}

fn invalid(m: &str) -> DomainError {
    DomainError::Invalid(m.into())
}

/// URL-safe base64 with padding, as Docker decodes `X-Registry-Auth`.
fn b64url_padded(data: &[u8]) -> String {
    let mut s = forge_secrets::crypto::b64url(data);
    while !s.len().is_multiple_of(4) {
        s.push('=');
    }
    s
}

fn check(input: &RegistryInput) -> DomainResult<String> {
    let name = input.name.trim();
    if name.is_empty() || name.len() > 100 || name.chars().any(char::is_control) {
        return Err(invalid("the name is 1 to 100 characters"));
    }
    if input.username.len() > 256 || input.username.chars().any(char::is_control) {
        return Err(invalid("the user name is at most 256 characters"));
    }
    if input.secret.as_ref().is_some_and(|s| s.is_empty() || s.len() > 8192 || s.contains('\0')) {
        return Err(invalid("the password or token is 1 to 8192 characters"));
    }
    let host = normalize_host(&input.host)?;
    if input.kind == RegistryKind::DockerHub && host != forge_core::registries::DOCKER_HUB {
        return Err(invalid("Docker Hub's host is docker.io"));
    }
    Ok(host)
}

impl RegistryService {
    pub async fn create(&self, ws: WorkspaceId, by: &Principal, input: RegistryInput) -> DomainResult<Registry> {
        let host = check(&input)?;
        let secret = input.secret.ok_or_else(|| invalid("a password or token is required"))?;
        let id = RegistryId::new();
        let now = self.clock.now();
        let r = Registry {
            id,
            workspace_id: ws,
            name: input.name.trim().to_owned(),
            kind: input.kind,
            host,
            username: input.username.trim().to_owned(),
            secret_sealed: self.envelope.seal(secret.as_bytes(), &aad(id)).map_err(DomainError::internal)?,
            created_by: by.clone(),
            created_at: now,
            updated_at: now,
        };
        self.repo.create_registry(&r).await?;
        Ok(r)
    }

    pub async fn update(&self, id: RegistryId, input: RegistryInput) -> DomainResult<Registry> {
        let mut r = self.get(id).await?;
        r.host = check(&input)?;
        r.name = input.name.trim().to_owned();
        r.kind = input.kind;
        r.username = input.username.trim().to_owned();
        if let Some(secret) = input.secret {
            r.secret_sealed = self.envelope.seal(secret.as_bytes(), &aad(id)).map_err(DomainError::internal)?;
        }
        r.updated_at = self.clock.now();
        self.repo.update_registry(&r).await?;
        Ok(r)
    }

    pub async fn get(&self, id: RegistryId) -> DomainResult<Registry> {
        self.repo.get_registry(id).await?.ok_or_else(|| DomainError::NotFound("registry not found".into()))
    }

    /// The `X-Registry-Auth` for `image` in workspace `ws`: the registry of
    /// the image's host, or `None` (public image, no registry in Forge).
    pub async fn auth_for(&self, ws: WorkspaceId, image: &str) -> DomainResult<Option<RegistryAuth>> {
        let host = image_host(image);
        let Some(r) = self.repo.list_registries(ws).await?.into_iter().find(|r| r.host == host) else {
            return Ok(None);
        };
        let secret = self.envelope.open(&r.secret_sealed, &aad(r.id)).map_err(DomainError::internal)?;
        let secret = String::from_utf8(secret).map_err(DomainError::internal)?;
        let body = serde_json::json!({ "username": r.username, "password": secret, "serveraddress": r.host });
        Ok(Some(RegistryAuth(b64url_padded(body.to_string().as_bytes()))))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn auth_is_padded_url_safe_base64() {
        assert_eq!(b64url_padded(b"a").len() % 4, 0);
        assert!(b64url_padded(&[0xfb, 0xff]).bytes().all(|b| b != b'+' && b != b'/'));
    }

    #[test]
    fn inputs_are_checked() {
        let base = RegistryInput {
            name: "GitHub".into(),
            kind: RegistryKind::Ghcr,
            host: "ghcr.io".into(),
            username: "bot".into(),
            secret: Some("t".into()),
        };
        assert!(check(&base).is_ok());
        assert!(check(&RegistryInput { host: "ghcr.io/acme".into(), ..base.clone() }).is_err());
        assert!(check(&RegistryInput { kind: RegistryKind::DockerHub, ..base.clone() }).is_err());
        assert!(check(&RegistryInput { secret: Some(String::new()), ..base }).is_err());
    }
}
