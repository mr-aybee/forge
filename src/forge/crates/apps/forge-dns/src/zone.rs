//! The in-memory answer map built from `names.json` (D-173,
//! D-175). A map is immutable once built; a reload builds a new one and
//! swaps it in whole.

use std::collections::{HashMap, HashSet};
use std::net::Ipv4Addr;
use std::sync::atomic::{AtomicUsize, Ordering};

use forge_catalog::names::{Record as FileRecord, Records};
use hickory_proto::rr::rdata::{A, PTR, SOA, SRV};
use hickory_proto::rr::{Name, RData, Record, RecordType};

/// SOA timers: the data changes within seconds, so secondaries are moot and
/// negative answers are cached no longer than positive ones.
const SOA_REFRESH: i32 = 60;
const SOA_RETRY: i32 = 10;
const SOA_EXPIRE: i32 = 300;

/// Why a record file cannot become a map.
#[derive(Debug, thiserror::Error)]
pub enum ZoneError {
    #[error("invalid JSON: {0}")]
    Json(#[from] serde_json::Error),
    #[error("invalid name {name:?}: {source}")]
    Name { name: String, source: hickory_proto::ProtoError },
    #[error("record {0:?} is outside every zone")]
    OutsideZones(String),
}

/// The A records of one name and its round-robin counter.
#[derive(Debug, Default)]
struct AddrSet {
    addrs: Vec<Ipv4Addr>,
    next: AtomicUsize,
}

/// What a query for a name in one of the zones gets.
#[derive(Debug)]
pub enum Answer {
    /// NOERROR with these answers; empty means the name exists without
    /// records of the asked type (the SOA goes in the authority section).
    Records { answers: Vec<Record>, soa: Option<Record> },
    /// NXDOMAIN with the zone's SOA in the authority section.
    NxDomain { soa: Record },
}

/// One loaded `names.json`.
#[derive(Debug)]
pub struct Zone {
    /// Zone apexes, lower case, without the trailing dot.
    zones: Vec<String>,
    ttl: u32,
    serial: u32,
    a: HashMap<String, AddrSet>,
    srv: HashMap<String, Vec<SRV>>,
    ptr: HashMap<String, Vec<Name>>,
    /// Every owner name and every name between it and its zone apex (empty
    /// non-terminals exist, so they answer NOERROR, not NXDOMAIN).
    known: HashSet<String>,
}

/// Lower case without the trailing dot: the map's key form.
#[must_use]
pub fn key(name: &str) -> String {
    name.trim_end_matches('.').to_ascii_lowercase()
}

fn fqdn(name: &str) -> Result<Name, ZoneError> {
    let mut n = Name::from_ascii(name).map_err(|source| ZoneError::Name { name: name.to_owned(), source })?;
    n.set_fqdn(true);

    Ok(n)
}

fn is_under(name: &str, zone: &str) -> bool {
    name == zone || name.strip_suffix(zone).is_some_and(|head| head.ends_with('.'))
}

impl Zone {
    /// Parses the bytes of `names.json`; `ttl` overrides the file's TTL.
    pub fn parse(bytes: &[u8], ttl: Option<u32>) -> Result<Self, ZoneError> {
        let records: Records = serde_json::from_slice(bytes)?;

        Self::build(&records, ttl)
    }

    /// Builds the map, validating every name.
    pub fn build(records: &Records, ttl: Option<u32>) -> Result<Self, ZoneError> {
        let mut zones = Vec::with_capacity(records.zones.len());

        for z in &records.zones {
            fqdn(z)?;
            zones.push(key(z));
        }

        // Longest apex first, so the most specific zone wins.
        zones.sort_by_key(|z| std::cmp::Reverse(z.len()));
        zones.dedup();

        let mut zone = Zone {
            known: zones.iter().cloned().collect(),
            zones,
            ttl: ttl.unwrap_or(records.ttl),
            // Seconds since the epoch wrap in 2106; the serial only needs to move.
            serial: u32::try_from((records.generated_at_ms / 1000).rem_euclid(1 << 32)).unwrap_or(0),
            a: HashMap::new(),
            srv: HashMap::new(),
            ptr: HashMap::new(),
        };

        for r in &records.records {
            zone.insert(r)?;
        }

        Ok(zone)
    }

    fn insert(&mut self, record: &FileRecord) -> Result<(), ZoneError> {
        let owner = key(record.name());
        fqdn(&owner)?;

        let Some(apex) = self.zone_of(&owner).map(str::to_owned) else {
            return Err(ZoneError::OutsideZones(owner));
        };

        match record {
            FileRecord::A { value, .. } => self.a.entry(owner.clone()).or_default().addrs.push(*value),
            FileRecord::Srv { priority, weight, port, target, .. } => {
                let srv = SRV::new(*priority, *weight, *port, fqdn(target)?);
                self.srv.entry(owner.clone()).or_default().push(srv);
            }
            FileRecord::Ptr { value, .. } => {
                let target = fqdn(value)?;
                self.ptr.entry(owner.clone()).or_default().push(target);
            }
        }

        // Mark the owner and its ancestors up to the apex as existing.
        let mut name = owner.as_str();

        while name.len() > apex.len() && self.known.insert(name.to_owned()) {
            let Some((_, parent)) = name.split_once('.') else { break };
            name = parent;
        }

        Ok(())
    }

    /// The zone apex `name` (key form) falls in, if any.
    #[must_use]
    pub fn zone_of(&self, name: &str) -> Option<&str> {
        self.zones.iter().find(|z| is_under(name, z)).map(String::as_str)
    }

    fn soa(&self, apex: &str) -> Option<Record> {
        let mname = fqdn(apex).ok()?;
        let rname = fqdn(&format!("hostmaster.{apex}")).ok()?;
        let soa = SOA::new(mname.clone(), rname, self.serial, SOA_REFRESH, SOA_RETRY, SOA_EXPIRE, self.ttl);

        Some(Record::from_rdata(mname, self.ttl, RData::SOA(soa)))
    }

    /// Answers a query, or `None` when the name is in none of the zones (the
    /// caller forwards it). A records rotate on every call.
    #[must_use]
    pub fn answer(&self, qname: &Name, qtype: RecordType) -> Option<Answer> {
        let name = key(&qname.to_ascii());
        let apex = self.zone_of(&name)?;
        let soa = self.soa(apex)?;

        if !self.known.contains(&name) {
            return Some(Answer::NxDomain { soa });
        }

        let owner = qname.clone();
        let ttl = self.ttl;

        let answers: Vec<Record> = match qtype {
            RecordType::A => self.a.get(&name).map(|set| rotated(set, &owner, ttl)).unwrap_or_default(),
            RecordType::SRV => {
                self.srv.get(&name).into_iter().flatten().map(|s| record(&owner, ttl, RData::SRV(s.clone()))).collect()
            }
            RecordType::PTR => self
                .ptr
                .get(&name)
                .into_iter()
                .flatten()
                .map(|p| record(&owner, ttl, RData::PTR(PTR(p.clone()))))
                .collect(),
            RecordType::SOA if name == apex => vec![soa.clone()],
            _ => Vec::new(),
        };

        let soa = answers.is_empty().then_some(soa);

        Some(Answer::Records { answers, soa })
    }
}

fn record(owner: &Name, ttl: u32, data: RData) -> Record {
    Record::from_rdata(owner.clone(), ttl, data)
}

fn rotated(set: &AddrSet, owner: &Name, ttl: u32) -> Vec<Record> {
    let len = set.addrs.len();

    if len == 0 {
        return Vec::new();
    }

    let start = set.next.fetch_add(1, Ordering::Relaxed) % len;

    (0..len).map(|i| record(owner, ttl, RData::A(A(set.addrs[(start + i) % len])))).collect()
}

#[cfg(test)]
#[allow(clippy::unwrap_used)] // Reason: test assertions.
mod tests {
    use super::*;

    const FILE: &str = r#"{
        "version": 1, "generated_at_ms": 1764115200000, "zones": ["c1.local", "64.10.in-addr.arpa"], "ttl": 5,
        "records": [
            { "name": "api.orders.prod.c1.local", "type": "A", "value": "10.64.10.2" },
            { "name": "api.orders.prod.c1.local", "type": "A", "value": "10.64.12.4" },
            { "name": "_http._tcp.api.orders.prod.c1.local", "type": "SRV", "priority": 0, "weight": 0, "port": 8080, "target": "api.orders.prod.c1.local" },
            { "name": "2.10.64.10.in-addr.arpa", "type": "PTR", "value": "api-1.orders.prod.c1.local" }
        ]
    }"#;

    fn name(s: &str) -> Name {
        Name::from_ascii(s).unwrap()
    }

    #[test]
    fn empty_non_terminals_exist_and_other_names_do_not() {
        let z = Zone::parse(FILE.as_bytes(), None).unwrap();

        assert!(matches!(z.answer(&name("prod.c1.local."), RecordType::A), Some(Answer::Records { .. })));
        assert!(matches!(z.answer(&name("nope.c1.local."), RecordType::A), Some(Answer::NxDomain { .. })));
        assert!(z.answer(&name("example.org."), RecordType::A).is_none());
        assert!(z.answer(&name("2.10.64.10.in-addr.arpa."), RecordType::PTR).is_some());
    }

    #[test]
    fn records_outside_the_zones_are_rejected() {
        let bad = FILE.replace("\"zones\": [\"c1.local\", ", "\"zones\": [");

        assert!(matches!(Zone::parse(bad.as_bytes(), None), Err(ZoneError::OutsideZones(_))));
    }

    #[test]
    fn ttl_override_wins() {
        let z = Zone::parse(FILE.as_bytes(), Some(30)).unwrap();

        let Some(Answer::Records { answers, .. }) = z.answer(&name("api.orders.prod.c1.local."), RecordType::A) else {
            panic!("expected records");
        };

        assert!(answers.iter().all(|r| r.ttl == 30));
    }
}
