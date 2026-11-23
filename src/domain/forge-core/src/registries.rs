//! Container registries (D-158): where images are pulled from and pushed
//! to, with credentials sealed at rest. A registry serves the images whose
//! reference starts with its host; Docker Hub serves references without one.

use async_trait::async_trait;
use serde::{Deserialize, Serialize};

use crate::access::Principal;
use crate::clock::Timestamp;
use crate::error::{DomainError, DomainResult};
use crate::ids::{RegistryId, WorkspaceId};

/// Docker Hub's host in image references and credentials.
pub const DOCKER_HUB: &str = "docker.io";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RegistryKind {
    DockerHub,
    Ghcr,
    Gitlab,
    /// Amazon ECR: the password is a token that expires; refresh it in Forge.
    Ecr,
    Custom,
}

impl RegistryKind {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::DockerHub => "docker_hub",
            Self::Ghcr => "ghcr",
            Self::Gitlab => "gitlab",
            Self::Ecr => "ecr",
            Self::Custom => "custom",
        }
    }
}

impl std::str::FromStr for RegistryKind {
    type Err = DomainError;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "docker_hub" => Ok(Self::DockerHub),
            "ghcr" => Ok(Self::Ghcr),
            "gitlab" => Ok(Self::Gitlab),
            "ecr" => Ok(Self::Ecr),
            "custom" => Ok(Self::Custom),
            other => Err(DomainError::internal(format!("unknown registry kind {other:?}"))),
        }
    }
}

/// A registry of a workspace. The secret (password or token) is sealed and
/// never serialized.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Registry {
    pub id: RegistryId,
    pub workspace_id: WorkspaceId,
    pub name: String,
    pub kind: RegistryKind,
    /// `host[:port]`, lower case, e.g. `ghcr.io`, `registry.example.com:5000`.
    pub host: String,
    pub username: String,
    #[serde(skip)]
    pub secret_sealed: String,
    pub created_by: Principal,
    pub created_at: Timestamp,
    pub updated_at: Timestamp,
}

/// The registry host of an image reference: the first path part when it
/// looks like a host (has a `.` or `:`, or is `localhost`), else Docker Hub.
#[must_use]
pub fn image_host(image: &str) -> String {
    match image.split_once('/') {
        Some((first, _)) if first.contains('.') || first.contains(':') || first == "localhost" => {
            let h = first.to_ascii_lowercase();
            if h == "index.docker.io" || h == "registry-1.docker.io" { DOCKER_HUB.into() } else { h }
        }
        _ => DOCKER_HUB.into(),
    }
}

/// A registry host as stored: `host[:port]`, lower case, no scheme or path.
pub fn normalize_host(host: &str) -> Result<String, DomainError> {
    let h = host.trim().trim_start_matches("https://").trim_end_matches('/').to_ascii_lowercase();
    let h = if h == "index.docker.io" || h == "registry-1.docker.io" { DOCKER_HUB.to_owned() } else { h };
    let (name, port) = h.split_once(':').map_or((h.as_str(), None), |(n, p)| (n, Some(p)));
    let name_ok = (1..=253).contains(&name.len())
        && name
            .split('.')
            .all(|l| (1..=63).contains(&l.len()) && l.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-'));
    let port_ok = port.is_none_or(|p| p.parse::<u16>().is_ok_and(|n| n > 0));
    if name_ok && port_ok && !h.contains('/') {
        Ok(h)
    } else {
        Err(DomainError::Invalid("the registry host is `host[:port]`, e.g. ghcr.io".into()))
    }
}

#[async_trait]
pub trait RegistryRepository: Send + Sync + std::fmt::Debug {
    async fn list_registries(&self, ws: WorkspaceId) -> DomainResult<Vec<Registry>>;
    async fn get_registry(&self, id: RegistryId) -> DomainResult<Option<Registry>>;
    /// `Conflict` when the workspace already has a registry for the host.
    async fn create_registry(&self, r: &Registry) -> DomainResult<()>;
    async fn update_registry(&self, r: &Registry) -> DomainResult<()>;
    async fn delete_registry(&self, id: RegistryId) -> DomainResult<()>;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn image_hosts() {
        assert_eq!(image_host("nginx:1.27"), DOCKER_HUB);
        assert_eq!(image_host("library/nginx"), DOCKER_HUB);
        assert_eq!(image_host("ghcr.io/acme/app:v1"), "ghcr.io");
        assert_eq!(image_host("Registry.Example.com:5000/team/app"), "registry.example.com:5000");
        assert_eq!(image_host("localhost/app"), "localhost");
        assert_eq!(image_host("index.docker.io/library/nginx"), DOCKER_HUB);
    }

    #[test]
    fn hosts_are_normalized() {
        assert_eq!(normalize_host("https://GHCR.io/").ok().as_deref(), Some("ghcr.io"));
        assert_eq!(normalize_host("registry.example.com:5000").ok().as_deref(), Some("registry.example.com:5000"));
        assert!(normalize_host("ghcr.io/acme").is_err());
        assert!(normalize_host("bad host").is_err());
        assert!(normalize_host("x:0").is_err());
    }
}
