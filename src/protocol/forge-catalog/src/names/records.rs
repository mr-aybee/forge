//! The full record set for `forge-dns` (`names.json`, D-175): A, SRV and PTR records with a short TTL. Built by the control plane,
//! carried in `HostNames.records_json`, written by the agent and read by
//! `forge-dns`. Unknown fields are accepted so `forge-dns` can read a newer
//! file.

use std::collections::{BTreeMap, BTreeSet};
use std::net::Ipv4Addr;

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::fabric::Cidr;

/// Format version of `names.json`.
pub const RECORDS_VERSION: u32 = 1;
/// Default TTL: names follow replicas within seconds.
pub const RECORDS_TTL: u32 = 5;
/// File the agent writes next to its data and `forge-dns` reads.
pub const RECORDS_FILE: &str = "names.json";

/// `names.json`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Records {
    pub version: u32,
    pub generated_at_ms: i64,
    /// Zones answered from `records` (cluster zones and reverse zones).
    pub zones: Vec<String>,
    pub ttl: u32,
    pub records: Vec<Record>,
}

/// One record; names are fully qualified without the trailing dot.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum Record {
    A {
        name: String,
        value: Ipv4Addr,
    },
    #[serde(rename = "SRV")]
    Srv {
        name: String,
        priority: u16,
        weight: u16,
        port: u16,
        target: String,
    },
    #[serde(rename = "PTR")]
    Ptr {
        name: String,
        value: String,
    },
}

impl Record {
    /// The owner name of the record.
    #[must_use]
    pub fn name(&self) -> &str {
        match self {
            Record::A { name, .. } | Record::Srv { name, .. } | Record::Ptr { name, .. } => name,
        }
    }
}

/// A port a name offers, for its SRV record.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct SrvPort {
    /// The service label (`_<service>._<proto>.<name>`).
    pub service: String,
    pub port: u16,
    /// `tcp` or `udp`.
    pub proto: String,
}

/// One address of a name, as the builder takes it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RecordInput {
    pub name: String,
    pub ip: Ipv4Addr,
    pub srv: Option<SrvPort>,
    /// The name the address's PTR record points to when several names share
    /// it (a replica's own name beats its service name).
    pub ptr_preferred: bool,
}

/// `d.c.b.a.in-addr.arpa` of an address.
#[must_use]
pub fn ptr_name(ip: Ipv4Addr) -> String {
    let [a, b, c, d] = ip.octets();

    format!("{d}.{c}.{b}.{a}.in-addr.arpa")
}

/// The reverse zone of a subnet, widened to whole octets (`10.64.0.0/16` →
/// `64.10.in-addr.arpa`, `10.64.0.0/20` → `64.10.in-addr.arpa`).
#[must_use]
pub fn reverse_zone(net: &Cidr) -> String {
    let octets = usize::from(net.prefix() / 8);
    let ip = net.first().octets();
    let mut parts: Vec<String> = ip[..octets].iter().rev().map(ToString::to_string).collect();
    parts.push("in-addr.arpa".into());

    parts.join(".")
}

/// Builds the record set: A records for every input, one SRV per name and
/// port, and a PTR for every address inside one of `subnets`, whose reverse
/// zones are added to `zones`.
#[must_use]
pub fn build_records(zones: &[String], rows: &[RecordInput], subnets: &[Cidr], now_ms: i64) -> Records {
    let mut records = BTreeSet::new();
    let mut ptr: BTreeMap<Ipv4Addr, (bool, String)> = BTreeMap::new();

    for r in rows {
        records.insert(Record::A { name: r.name.clone(), value: r.ip });

        if let Some(s) = &r.srv {
            records.insert(Record::Srv {
                name: format!("_{}._{}.{}", s.service, s.proto, r.name),
                priority: 0,
                weight: 0,
                port: s.port,
                target: r.name.clone(),
            });
        }

        if !subnets.iter().any(|n| n.contains(r.ip)) {
            continue;
        }

        // Preferred names first, then the shortest-sorting one: stable.
        let candidate = (!r.ptr_preferred, r.name.clone());
        let better = ptr.get(&r.ip).is_none_or(|(p, n)| candidate < (!*p, n.clone()));

        if better {
            ptr.insert(r.ip, (r.ptr_preferred, r.name.clone()));
        }
    }

    for (ip, (_, name)) in ptr {
        records.insert(Record::Ptr { name: ptr_name(ip), value: name });
    }

    let mut all_zones: Vec<String> = zones.to_vec();
    all_zones.extend(subnets.iter().map(reverse_zone));
    all_zones.sort();
    all_zones.dedup();

    Records {
        version: RECORDS_VERSION,
        generated_at_ms: now_ms,
        zones: all_zones,
        ttl: RECORDS_TTL,
        records: records.into_iter().collect(),
    }
}

/// The `dns_search` list of a stack's containers: the stack, its
/// environment and its cluster, most specific first.
#[must_use]
pub fn search_list(stack: &str, environment: &str, cluster: Option<&str>, suffix: &str) -> Vec<String> {
    let tail = match cluster {
        Some(c) => format!("{c}.{suffix}"),
        None => suffix.to_owned(),
    };

    vec![format!("{stack}.{environment}.{tail}"), format!("{environment}.{tail}"), tail]
}

/// The first container-side port of a compose service, from `ports:` or
/// `expose:`, with its protocol. A service with neither has no name.
#[must_use]
pub fn first_container_port(svc: &Value) -> Option<(u16, String)> {
    let from_text = |s: &str| -> Option<(u16, String)> {
        let (spec, proto) = s.split_once('/').unwrap_or((s, "tcp"));
        let container = spec.rsplit(':').next()?;
        let first = container.split('-').next()?;

        Some((first.parse().ok()?, proto.to_ascii_lowercase()))
    };
    let from_value = |v: &Value| -> Option<(u16, String)> {
        match v {
            Value::String(s) => from_text(s),
            Value::Number(n) => Some((u16::try_from(n.as_u64()?).ok()?, "tcp".into())),
            Value::Object(m) => {
                let port = match m.get("target")? {
                    Value::Number(n) => u16::try_from(n.as_u64()?).ok()?,
                    Value::String(s) => s.parse().ok()?,
                    _ => return None,
                };

                let proto = m.get("protocol").and_then(Value::as_str).unwrap_or("tcp").to_ascii_lowercase();

                Some((port, proto))
            }
            _ => None,
        }
    };

    ["ports", "expose"].iter().filter_map(|k| svc.get(*k)?.as_array()).flatten().find_map(from_value)
}

/// Services whose name equals `<other>-<n>` for another service `other`:
/// they clash with replica `n` of `other`. Returns `(service, other, n)`.
#[must_use]
pub fn replica_clashes<'a>(services: &[&'a str]) -> Vec<(&'a str, &'a str, u32)> {
    let mut out = Vec::new();

    for s in services {
        let Some((base, n)) = s.rsplit_once('-') else { continue };
        let Ok(n) = n.parse::<u32>() else { continue };

        if let Some(other) = services.iter().find(|o| **o == base) {
            out.push((*s, *other, n));
        }
    }

    out
}

#[cfg(test)]
#[allow(clippy::unwrap_used)] // Reason: test assertions.
mod tests {
    use serde_json::json;

    use super::*;

    fn ip(s: &str) -> Ipv4Addr {
        s.parse().unwrap()
    }

    #[test]
    fn records_round_trip_and_build_ptr_and_srv() {
        let srv = SrvPort { service: "api".into(), port: 8080, proto: "tcp".into() };
        let rows = vec![
            RecordInput {
                name: "api.orders.prod.c1.local".into(),
                ip: ip("10.64.10.2"),
                srv: Some(srv),
                ptr_preferred: false,
            },
            RecordInput {
                name: "api-1.orders.prod.c1.local".into(),
                ip: ip("10.64.10.2"),
                srv: None,
                ptr_preferred: true,
            },
            RecordInput { name: "edge.c1.local".into(), ip: ip("192.0.2.4"), srv: None, ptr_preferred: false },
        ];
        let subnets = vec!["10.64.0.0/16".parse().unwrap()];
        let r = build_records(&["c1.local".to_owned()], &rows, &subnets, 1);

        assert_eq!(r.zones, vec!["64.10.in-addr.arpa", "c1.local"]);
        assert!(r.records.contains(&Record::Ptr {
            name: "2.10.64.10.in-addr.arpa".into(),
            value: "api-1.orders.prod.c1.local".into()
        }));
        assert!(r.records.iter().any(
            |x| matches!(x, Record::Srv { name, port: 8080, .. } if name == "_api._tcp.api.orders.prod.c1.local")
        ));
        assert!(!r.records.iter().any(|x| x.name() == "4.2.0.192.in-addr.arpa"));

        let text = serde_json::to_string(&r).unwrap();
        assert!(text.contains("\"type\":\"SRV\""));

        let back: Records = serde_json::from_str(&text).unwrap();
        assert_eq!(back, r);

        let newer = json!({ "version": 2, "generated_at_ms": 1, "zones": [], "ttl": 5, "records": [], "extra": 1 });
        assert!(serde_json::from_value::<Records>(newer).is_ok());
    }

    #[test]
    fn reverse_zones_widen_to_octets() {
        assert_eq!(reverse_zone(&"10.64.0.0/16".parse().unwrap()), "64.10.in-addr.arpa");
        assert_eq!(reverse_zone(&"10.64.16.0/20".parse().unwrap()), "64.10.in-addr.arpa");
        assert_eq!(reverse_zone(&"10.64.12.0/24".parse().unwrap()), "12.64.10.in-addr.arpa");
    }

    #[test]
    fn search_list_is_most_specific_first() {
        assert_eq!(
            search_list("orders", "prod", Some("c1"), "local"),
            vec!["orders.prod.c1.local", "prod.c1.local", "c1.local"]
        );
        assert_eq!(search_list("orders", "prod", None, "local"), vec!["orders.prod.local", "prod.local", "local"]);
    }

    #[test]
    fn container_ports_come_from_ports_or_expose() {
        assert_eq!(first_container_port(&json!({ "ports": ["8080:80"] })), Some((80, "tcp".into())));
        assert_eq!(first_container_port(&json!({ "ports": ["127.0.0.1:53:53/udp"] })), Some((53, "udp".into())));
        assert_eq!(first_container_port(&json!({ "ports": [{ "target": 9000 }] })), Some((9000, "tcp".into())));
        assert_eq!(first_container_port(&json!({ "expose": ["5432"] })), Some((5432, "tcp".into())));
        assert_eq!(first_container_port(&json!({ "ports": ["3000-3005"] })), Some((3000, "tcp".into())));
        assert_eq!(first_container_port(&json!({ "image": "redis" })), None);
    }

    #[test]
    fn a_service_named_like_a_replica_clashes() {
        assert_eq!(replica_clashes(&["api", "api-2", "db-1"]), vec![("api-2", "api", 2)]);
        assert!(replica_clashes(&["api", "web"]).is_empty());
    }
}
