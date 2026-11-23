//! Per-user pins: the use case behind `/api/v1/me/pins`.
//!
//! The service resolves a pin to its entity (for the label and for the
//! `view` check the handler makes) and keeps the order. It never decides who
//! may see what: the handler authorizes every resolved entity with
//! `can_many`, so a pin the caller can no longer view is hidden, not deleted.

use std::sync::Arc;

use forge_core::pins::{MAX_PINS, Pin, PinAdded, PinKind, PinRepository, reorder};
use forge_core::ports::{ClusterRepository, ProjectRepository, ServerRepository, ServiceRepository, TenancyRepository};
use forge_core::{
    Clock, ClusterId, DomainError, DomainResult, EntityKind, EntityRef, ProjectId, ServerId, ServiceId, UserId,
};

use crate::error::ApiError;

/// Audit action: a pin was added.
pub const AUDIT_PIN_ADD: &str = "user.pin.add";
/// Audit action: a pin was removed.
pub const AUDIT_PIN_REMOVE: &str = "user.pin.remove";
/// Audit action: the pins were reordered.
pub const AUDIT_PIN_REORDER: &str = "user.pin.reorder";

/// Why a pin request failed.
#[derive(Debug, thiserror::Error)]
pub enum PinError {
    /// The id is not a valid id for the kind.
    #[error("invalid {0} id")]
    BadId(PinKind),
    /// The entity does not exist.
    #[error("{0} not found")]
    NotFound(PinKind),
    /// The user holds [`MAX_PINS`] pins already.
    #[error("at most {MAX_PINS} pins per user; unpin something first")]
    Limit,
    #[error(transparent)]
    Store(#[from] DomainError),
}

impl From<PinError> for ApiError {
    fn from(e: PinError) -> Self {
        use axum::http::StatusCode;
        match e {
            PinError::BadId(_) => ApiError::bad_request(e.to_string()),
            PinError::NotFound(_) => ApiError::not_found(e.to_string()),
            PinError::Limit => ApiError::new(StatusCode::CONFLICT, "pin_limit", e.to_string()),
            PinError::Store(d) => d.into(),
        }
    }
}

/// A pin's entity as it is now.
#[derive(Debug, Clone)]
pub struct Target {
    /// What `view` is checked on.
    pub entity: EntityRef,
    /// The entity's current name.
    pub label: String,
    /// Context: the environment of a service, the cluster of a node.
    pub sublabel: Option<String>,
}

/// A stored pin whose entity still exists.
#[derive(Debug, Clone)]
pub struct ResolvedPin {
    pub pin: Pin,
    pub target: Target,
}

#[derive(Debug)]
pub struct PinDeps {
    pub pins: Arc<dyn PinRepository>,
    pub projects: Arc<dyn ProjectRepository>,
    pub services: Arc<dyn ServiceRepository>,
    pub tenancy: Arc<dyn TenancyRepository>,
    pub servers: Arc<dyn ServerRepository>,
    pub clusters: Arc<dyn ClusterRepository>,
    pub clock: Arc<dyn Clock>,
}

#[derive(Debug)]
pub struct PinService {
    pins: Arc<dyn PinRepository>,
    projects: Arc<dyn ProjectRepository>,
    services: Arc<dyn ServiceRepository>,
    tenancy: Arc<dyn TenancyRepository>,
    servers: Arc<dyn ServerRepository>,
    clusters: Arc<dyn ClusterRepository>,
    clock: Arc<dyn Clock>,
}

/// `Ok(None)` for a missing entity; other store errors propagate.
fn missing<T>(r: DomainResult<T>) -> DomainResult<Option<T>> {
    match r {
        Ok(v) => Ok(Some(v)),
        Err(DomainError::NotFound(_)) => Ok(None),
        Err(e) => Err(e),
    }
}

impl PinService {
    #[must_use]
    pub fn new(d: PinDeps) -> Self {
        Self {
            pins: d.pins,
            projects: d.projects,
            services: d.services,
            tenancy: d.tenancy,
            servers: d.servers,
            clusters: d.clusters,
            clock: d.clock,
        }
    }

    /// The entity behind `kind`/`id` now, or `None` if it is gone.
    pub async fn resolve(&self, kind: PinKind, id: &str) -> Result<Option<Target>, PinError> {
        let bad = || PinError::BadId(kind);
        Ok(match kind {
            PinKind::Project => {
                let pid: ProjectId = id.parse().map_err(|_| bad())?;
                missing(self.projects.get_project(pid).await)?.map(|p| Target {
                    entity: EntityRef::new(EntityKind::Project, pid),
                    label: p.name,
                    sublabel: None,
                })
            }
            PinKind::Service => {
                let sid: ServiceId = id.parse().map_err(|_| bad())?;
                match self.services.get_service(sid).await? {
                    None => None,
                    Some(s) => {
                        let env = missing(self.tenancy.get_environment(s.environment_id).await)?;
                        Some(Target {
                            entity: EntityRef::new(EntityKind::Service, sid),
                            label: s.name,
                            sublabel: env.map(|e| e.name),
                        })
                    }
                }
            }
            PinKind::Node => {
                let nid: ServerId = id.parse().map_err(|_| bad())?;
                match missing(self.servers.get_server(nid).await)? {
                    None => None,
                    Some(s) => {
                        let cluster = match s.cluster_id {
                            Some(c) => missing(self.clusters.get_cluster(c).await)?.map(|c| c.name),
                            None => None,
                        };
                        Some(Target {
                            entity: EntityRef::new(EntityKind::Server, nid),
                            label: s.display_name.filter(|n| !n.trim().is_empty()).unwrap_or(s.hostname),
                            sublabel: cluster,
                        })
                    }
                }
            }
            PinKind::Cluster => {
                let cid: ClusterId = id.parse().map_err(|_| bad())?;
                missing(self.clusters.get_cluster(cid).await)?.map(|c| Target {
                    entity: EntityRef::new(EntityKind::Cluster, cid),
                    label: c.name,
                    sublabel: None,
                })
            }
        })
    }

    /// Like [`PinService::resolve`], with a missing entity as an error.
    pub async fn target(&self, kind: PinKind, id: &str) -> Result<Target, PinError> {
        self.resolve(kind, id).await?.ok_or(PinError::NotFound(kind))
    }

    /// The user's pins whose entity still exists, in order. A user holds at
    /// most [`MAX_PINS`], so resolving each one is bounded.
    pub async fn list(&self, user: UserId) -> Result<Vec<ResolvedPin>, PinError> {
        let mut out = Vec::new();
        for pin in self.pins.list_pins(user).await? {
            match self.resolve(pin.kind, &pin.entity_id).await {
                Ok(Some(target)) => out.push(ResolvedPin { pin, target }),
                // Gone, or a row whose id no longer parses: hidden, kept.
                Ok(None) | Err(PinError::BadId(_)) => {}
                Err(e) => return Err(e),
            }
        }
        Ok(out)
    }

    /// Appends a pin; `true` if it was added, `false` if it existed.
    pub async fn add(&self, user: UserId, kind: PinKind, id: &str) -> Result<bool, PinError> {
        match self.pins.add_pin(user, kind, id, MAX_PINS, self.clock.now()).await? {
            PinAdded::Added => Ok(true),
            PinAdded::Exists => Ok(false),
            PinAdded::Full => Err(PinError::Limit),
        }
    }

    /// Removes a pin; `true` if it was pinned.
    pub async fn remove(&self, user: UserId, kind: PinKind, id: &str) -> Result<bool, PinError> {
        Ok(self.pins.remove_pin(user, kind, id).await?)
    }

    /// Puts `wanted` first, in that order; other pins follow in their
    /// current order and unknown entries are ignored.
    pub async fn reorder(&self, user: UserId, wanted: &[(PinKind, String)]) -> Result<(), PinError> {
        let current = self.pins.list_pins(user).await?;
        let order = reorder(&current, wanted);
        self.pins.set_pin_order(user, &order).await?;
        Ok(())
    }
}
