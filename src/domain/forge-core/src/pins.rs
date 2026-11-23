//! Pins: the projects, services, nodes and clusters a user keeps at hand in
//! the console sidebar. Pins are personal; a user only reads and writes
//! their own.

use async_trait::async_trait;
use serde::{Deserialize, Serialize};

use crate::{DomainError, DomainResult, Timestamp, UserId};

/// Most pins one user can hold.
pub const MAX_PINS: usize = 50;

/// What a pin points at.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PinKind {
    Project,
    Service,
    Node,
    Cluster,
}

impl PinKind {
    pub const ALL: [PinKind; 4] = [PinKind::Project, PinKind::Service, PinKind::Node, PinKind::Cluster];

    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            PinKind::Project => "project",
            PinKind::Service => "service",
            PinKind::Node => "node",
            PinKind::Cluster => "cluster",
        }
    }
}

impl std::fmt::Display for PinKind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

impl std::str::FromStr for PinKind {
    type Err = DomainError;
    fn from_str(s: &str) -> Result<Self, DomainError> {
        PinKind::ALL
            .into_iter()
            .find(|k| k.as_str() == s)
            .ok_or_else(|| DomainError::Invalid(format!("unknown pin kind `{s}` (project, service, node, cluster)")))
    }
}

/// One stored pin. `entity_id` is the typed id of the entity as text.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Pin {
    pub user_id: UserId,
    pub kind: PinKind,
    pub entity_id: String,
    pub position: i64,
    pub created_at: Timestamp,
}

/// What [`PinRepository::add_pin`] did.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PinAdded {
    /// Appended at the end.
    Added,
    /// It was pinned already; nothing changed.
    Exists,
    /// The user holds `max` pins already; nothing changed.
    Full,
}

/// The new order of a user's pins: the entries of `wanted` that are pinned,
/// in that order, then the remaining pins in their current order. Unknown
/// and repeated entries in `wanted` are ignored.
#[must_use]
pub fn reorder(current: &[Pin], wanted: &[(PinKind, String)]) -> Vec<(PinKind, String)> {
    let mut out: Vec<(PinKind, String)> = Vec::with_capacity(current.len());
    for (k, id) in wanted {
        let pinned = current.iter().any(|p| p.kind == *k && p.entity_id == *id);
        if pinned && !out.iter().any(|(ok, oid)| ok == k && oid == id) {
            out.push((*k, id.clone()));
        }
    }
    for p in current {
        if !out.iter().any(|(k, id)| *k == p.kind && *id == p.entity_id) {
            out.push((p.kind, p.entity_id.clone()));
        }
    }
    out
}

#[async_trait]
pub trait PinRepository: Send + Sync + std::fmt::Debug {
    /// The user's pins ordered by position (at most [`MAX_PINS`]).
    async fn list_pins(&self, user: UserId) -> DomainResult<Vec<Pin>>;
    /// Appends a pin unless it exists or the user holds `max` pins already.
    /// One statement, so two concurrent adds cannot pass the cap together.
    async fn add_pin(
        &self,
        user: UserId,
        kind: PinKind,
        entity_id: &str,
        max: usize,
        now: Timestamp,
    ) -> DomainResult<PinAdded>;
    /// Removes a pin; `false` if it was not pinned.
    async fn remove_pin(&self, user: UserId, kind: PinKind, entity_id: &str) -> DomainResult<bool>;
    /// Sets positions 0.. in the given order, in one transaction. Entries
    /// that are not pinned are skipped.
    async fn set_pin_order(&self, user: UserId, order: &[(PinKind, String)]) -> DomainResult<()>;
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pin(kind: PinKind, id: &str, position: i64) -> Pin {
        Pin { user_id: UserId::new(), kind, entity_id: id.into(), position, created_at: Timestamp(0) }
    }

    #[test]
    fn reorder_puts_wanted_first_and_keeps_the_rest() {
        let cur = vec![pin(PinKind::Node, "a", 0), pin(PinKind::Cluster, "b", 1), pin(PinKind::Project, "c", 2)];
        let wanted = vec![
            (PinKind::Project, "c".to_owned()),
            (PinKind::Service, "zzz".to_owned()),
            (PinKind::Project, "c".to_owned()),
            (PinKind::Node, "a".to_owned()),
        ];
        let got = reorder(&cur, &wanted);
        assert_eq!(
            got,
            vec![(PinKind::Project, "c".into()), (PinKind::Node, "a".into()), (PinKind::Cluster, "b".into())]
        );
    }

    #[test]
    fn kinds_round_trip() {
        for k in PinKind::ALL {
            assert_eq!(k.as_str().parse::<PinKind>().ok(), Some(k));
        }
        assert!("server".parse::<PinKind>().is_err());
    }
}
