//! Who data belongs to (moved from forge-server's `logs.rs` and
//! `container_telemetry.rs`, D-141).
//!
//! - The node is the one on the authenticated agent link, never a value
//!   the node sends.
//! - `forge.app = "<environment>/<name>"` names a service, but only one
//!   placed on that node, so a node cannot write into another node's
//!   services; `forge.stack`, else the compose project, names the stack.
//! - A Seq/CLEF event names its node with one of [`NODE_PROPERTIES`], and
//!   only the service's own nodes can match.

use std::collections::HashMap;

use forge_core::{ServerId, ServiceId};
use forge_proto::messages::{ContainerLabels, MAX_CONTAINER_FIELD_BYTES};

/// Event properties that name the sending machine, in order of preference:
/// Serilog's environment enricher, the lab portals, common logger fields.
pub const NODE_PROPERTIES: [&str; 5] = ["MachineName", "Node", "Host", "HostName", "hostname"];

/// A node a service runs on, with the names an event may use for it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlacedNode {
    pub server_id: ServerId,
    /// Lower-case: hostname, its first label, and the console name.
    pub names: Vec<String>,
}

/// The node an event came from: the first of [`NODE_PROPERTIES`] that names
/// one of the service's nodes.
#[must_use]
pub fn attribute_node(
    properties: &serde_json::Map<String, serde_json::Value>,
    nodes: &[PlacedNode],
) -> Option<ServerId> {
    NODE_PROPERTIES.iter().find_map(|k| {
        let v = properties.get(*k)?.as_str()?.trim().to_lowercase();
        let short = v.split('.').next().unwrap_or_default().to_owned();
        nodes.iter().find(|n| n.names.iter().any(|name| *name == v || *name == short)).map(|n| n.server_id)
    })
}

/// Where a container belongs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Attribution {
    pub service_id: Option<ServiceId>,
    pub stack: Option<String>,
}

/// Attribution from labels and a node's services by `forge.app` value.
#[must_use]
pub fn attribute(labels: Option<&ContainerLabels>, services: Option<&HashMap<String, ServiceId>>) -> Attribution {
    let Some(l) = labels else { return Attribution { service_id: None, stack: None } };
    let some = |s: &str| (!s.is_empty()).then(|| clip(s, MAX_CONTAINER_FIELD_BYTES).to_owned());
    Attribution {
        service_id: (!l.forge_app.is_empty()).then(|| services.and_then(|m| m.get(&l.forge_app).copied())).flatten(),
        stack: some(&l.forge_stack).or_else(|| some(&l.compose_project)),
    }
}

/// `s` cut to at most `max` bytes on a character boundary.
#[must_use]
pub fn clip(s: &str, max: usize) -> &str {
    if s.len() <= max {
        return s;
    }
    let mut cut = max;
    while !s.is_char_boundary(cut) {
        cut -= 1;
    }
    &s[..cut]
}

#[cfg(test)]
#[allow(clippy::unwrap_used)] // reason: tests
mod tests {
    use super::*;

    fn props(pairs: &[(&str, &str)]) -> serde_json::Map<String, serde_json::Value> {
        pairs.iter().map(|(k, v)| ((*k).to_owned(), serde_json::Value::from(*v))).collect()
    }

    fn labels(app: &str, stack: &str, project: &str) -> ContainerLabels {
        ContainerLabels {
            forge_app: app.into(),
            forge_stack: stack.into(),
            compose_project: project.into(),
            compose_service: String::new(),
        }
    }

    #[test]
    fn events_are_attributed_to_the_services_own_nodes_only() {
        let (a, b) = (ServerId::new(), ServerId::new());
        let nodes = vec![
            PlacedNode { server_id: a, names: vec!["dev-node-1".into(), "web a".into()] },
            PlacedNode { server_id: b, names: vec!["dev-node-2.lab.local".into(), "dev-node-2".into()] },
        ];
        assert_eq!(attribute_node(&props(&[("MachineName", "DEV-NODE-1")]), &nodes), Some(a), "case-insensitive");
        assert_eq!(attribute_node(&props(&[("Node", "dev-node-2.lab.local")]), &nodes), Some(b), "full hostname");
        assert_eq!(attribute_node(&props(&[("Host", "dev-node-1.example.org")]), &nodes), Some(a), "first label");
        assert_eq!(attribute_node(&props(&[("hostname", "web a")]), &nodes), Some(a), "console name");
        assert_eq!(attribute_node(&props(&[("MachineName", "elsewhere")]), &nodes), None, "not one of its nodes");
        assert_eq!(attribute_node(&props(&[("Other", "dev-node-1")]), &nodes), None, "only known properties");
        assert_eq!(
            attribute_node(&props(&[("MachineName", "dev-node-2"), ("Node", "dev-node-1")]), &nodes),
            Some(b),
            "MachineName wins"
        );
        assert_eq!(attribute_node(&props(&[("MachineName", "dev-node-1")]), &[]), None, "no placement");
    }

    #[test]
    fn attributes_to_a_service_a_stack_or_nothing() {
        let svc = ServiceId::new();
        let map: HashMap<String, ServiceId> = [("prod/api".to_owned(), svc)].into();
        let a = attribute(Some(&labels("prod/api", "", "")), Some(&map));
        assert_eq!(a, Attribution { service_id: Some(svc), stack: None }, "service");
        let a = attribute(Some(&labels("", "prod/shop", "shop")), Some(&map));
        assert_eq!(a, Attribution { service_id: None, stack: Some("prod/shop".into()) }, "forge.stack wins");
        let a = attribute(Some(&labels("", "", "blog")), Some(&map));
        assert_eq!(a.stack.as_deref(), Some("blog"), "compose project");
        let a = attribute(Some(&labels("other/api", "", "")), Some(&map));
        assert_eq!(a, Attribution { service_id: None, stack: None }, "a service not placed on the node");
        let a = attribute(Some(&labels("prod/api", "", "")), None);
        assert_eq!(a.service_id, None, "a node with no services");
        assert_eq!(attribute(None, Some(&map)), Attribution { service_id: None, stack: None }, "unattributed");
    }
}
