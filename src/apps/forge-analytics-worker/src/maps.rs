//! The control-plane maps forge-server pushes (D-141): which nodes are
//! active, which services are placed where and under which names, and the
//! ingest keys' hashes. Replaced whole on every push; readers take a
//! snapshot (`Arc`) and never hold the lock across an await.

use std::collections::{HashMap, HashSet};
use std::sync::{Arc, PoisonError, RwLock};

use forge_core::analytics::{AnalyticsMaps, MAX_MAP_ENTRIES};
use forge_core::{ServerId, ServiceId};

use crate::ingest::attribution::PlacedNode;

/// The maps, indexed for lookups.
#[derive(Debug, Default)]
pub struct Indexed {
    pub active: HashSet<ServerId>,
    /// Token hash → service (used by the agent upload listener, later).
    pub keys: HashMap<String, ServiceId>,
    /// Per node: its services by `forge.app` value (`<environment>/<name>`).
    pub apps: HashMap<ServerId, HashMap<String, ServiceId>>,
    /// Per service: the nodes it is placed on, with their names.
    pub placements: HashMap<ServiceId, Vec<PlacedNode>>,
    /// Per active node: the serial (lower-case hex) of its current certificate.
    pub certs: HashMap<ServerId, String>,
}

impl Indexed {
    /// Whether `node` may upload with the certificate `serial`: the node is
    /// active and this is its current certificate, so one replaced at
    /// renewal or revoked is refused (D-145).
    #[must_use]
    pub fn admits(&self, node: ServerId, serial: &str) -> bool {
        self.active.contains(&node) && self.certs.get(&node).is_some_and(|s| *s == forge_pki::canonical_serial(serial))
    }
}

/// Why a push was refused.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("a map holds more than {MAX_MAP_ENTRIES} entries")]
pub struct TooLarge;

/// The current maps.
#[derive(Debug, Default)]
pub struct Maps {
    current: RwLock<Arc<Indexed>>,
}

impl Maps {
    /// A snapshot of the current maps.
    #[must_use]
    pub fn get(&self) -> Arc<Indexed> {
        self.current.read().unwrap_or_else(PoisonError::into_inner).clone()
    }

    /// Replaces the maps; refuses lists over [`MAX_MAP_ENTRIES`].
    pub fn replace(&self, m: AnalyticsMaps) -> Result<(), TooLarge> {
        if !m.within_limits() {
            return Err(TooLarge);
        }

        let mut apps: HashMap<ServerId, HashMap<String, ServiceId>> = HashMap::new();

        for a in m.app_keys {
            apps.entry(a.server_id).or_default().insert(a.app, a.service_id);
        }

        let mut placements: HashMap<ServiceId, Vec<PlacedNode>> = HashMap::new();

        for p in m.placements {
            placements.entry(p.service_id).or_default().push(PlacedNode { server_id: p.server_id, names: p.names });
        }

        let indexed = Indexed {
            active: m.active_nodes.into_iter().collect(),
            keys: m.key_services.into_iter().map(|k| (k.token_hash, k.service_id)).collect(),
            apps,
            placements,
            certs: m.node_certs.into_iter().map(|c| (c.server_id, forge_pki::canonical_serial(&c.serial))).collect(),
        };
        *self.current.write().unwrap_or_else(PoisonError::into_inner) = Arc::new(indexed);

        Ok(())
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)] // reason: tests
mod tests {
    use super::*;
    use forge_core::analytics::{AppKey, NodeCert, Placement};

    #[test]
    fn indexes_and_refuses_oversized_pushes() {
        let maps = Maps::default();
        let (node, svc) = (ServerId::new(), ServiceId::new());
        maps.replace(AnalyticsMaps {
            active_nodes: vec![node],
            key_services: vec![],
            app_keys: vec![AppKey { server_id: node, app: "prod/api".into(), service_id: svc }],
            placements: vec![Placement { service_id: svc, server_id: node, names: vec!["n1".into()] }],
            node_certs: vec![NodeCert { server_id: node, serial: "0A1b".into() }],
        })
        .unwrap();
        let m = maps.get();
        assert_eq!(m.apps[&node]["prod/api"], svc);
        assert!(m.admits(node, "0a1B"), "the current certificate, in any case");
        assert!(!m.admits(node, "0a1c"), "a replaced or revoked certificate");
        assert!(!m.admits(ServerId::new(), "0a1b"), "another node");

        // A serial recorded with the leading zero byte DER drops (before D-145).
        maps.replace(AnalyticsMaps {
            active_nodes: vec![node],
            node_certs: vec![NodeCert { server_id: node, serial: "0021cb62".into() }],
            ..AnalyticsMaps::default()
        })
        .unwrap();
        assert!(maps.get().admits(node, "21cb62"), "compared in DER form");
        assert_eq!(m.placements[&svc][0].server_id, node);
        let big = AnalyticsMaps { active_nodes: vec![node; MAX_MAP_ENTRIES + 1], ..AnalyticsMaps::default() };
        assert_eq!(maps.replace(big), Err(TooLarge));
        assert!(maps.get().active.contains(&node), "the previous maps stay");
    }
}
