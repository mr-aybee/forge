//! Which nodes a service runs on: one placer for `forge.deploy.yaml`
//! (`spec.placement`) and Compose-format services files
//! (`deploy.replicas`, `mode: global`, constraints, spread and
//! reservations, `plans/service-orchestration.md` step 3).
//!
//! Pure: the caller loads the nodes (with their tags, state and capacity)
//! and gets node ids back. One copy runs per node.

use std::collections::{BTreeMap, BTreeSet};

/// A node the placer may pick.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Candidate {
    /// The node's id, as a string.
    pub id: String,
    pub hostname: String,
    pub display_name: Option<String>,
    /// Node tags as `key:value`, lowercase.
    pub tags: BTreeSet<String>,
    pub online: bool,
    pub drained: bool,
    /// The service already runs (or is placed) here.
    pub current: bool,
    /// CPUs in thousandths of a core, when the node reported them.
    pub cpu_millis: Option<u64>,
    /// Memory in MiB, when the node reported it.
    pub memory_mb: Option<u64>,
}

impl Candidate {
    /// Whether `name` is this node's id, host name or console name
    /// (case-insensitive).
    #[must_use]
    pub fn named(&self, name: &str) -> bool {
        let w = name.to_lowercase();

        self.id == w
            || self.hostname.to_lowercase() == w
            || self.display_name.as_deref().is_some_and(|d| d.to_lowercase() == w)
    }

    /// The value of tag `key` (lowercase), if the node has it.
    #[must_use]
    pub fn tag(&self, key: &str) -> Option<&str> {
        let prefix = format!("{}:", key.to_lowercase());

        self.tags.iter().find_map(|t| t.strip_prefix(&prefix))
    }
}

/// How many of the matching nodes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Count {
    /// Every matching node, online or not (`forge.deploy.yaml` without
    /// `replicas`).
    All,
    /// This many, online nodes first (an offline node only when too few
    /// online ones match).
    Replicas(u32),
    /// Every matching node that is online, plus the ones it already runs on
    /// (`deploy.mode: global`).
    Global,
}

/// What a service asks of its nodes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Rules<'a> {
    /// Node names or ids; empty: every node.
    pub nodes: &'a [String],
    /// Tags a node must all have (`key: value`).
    pub tags: &'a BTreeMap<String, String>,
    pub count: Count,
    /// Spread the copies over the values of this tag key.
    pub spread: Option<&'a str>,
    /// CPU a node must have (thousandths of a core, `0` = none).
    pub reserve_cpu_millis: u32,
    /// Memory a node must have (MiB, `0` = none).
    pub reserve_memory_mb: u32,
}

/// Why no placement was found.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PlaceError {
    /// A named node does not exist.
    UnknownNode(String),
    /// No node matches the rules (drained nodes are skipped).
    NoneMatch,
}

/// The nodes for `rules` among `candidates`: named nodes (or all), filtered
/// by tags, drained state and reservations, ordered with nodes the service
/// runs on first, then online ones, then by name, and cut to the count.
pub fn select(candidates: &[Candidate], rules: &Rules<'_>) -> Result<Vec<String>, PlaceError> {
    let mut picked: Vec<&Candidate> = Vec::new();

    if rules.nodes.is_empty() {
        picked.extend(candidates.iter());
    } else {
        for want in rules.nodes {
            let c = candidates.iter().find(|c| c.named(want)).ok_or_else(|| PlaceError::UnknownNode(want.clone()))?;

            if !picked.iter().any(|x| x.id == c.id) {
                picked.push(c);
            }
        }
    }

    let wanted: Vec<String> = rules.tags.iter().map(|(k, v)| format!("{k}:{v}").to_lowercase()).collect();
    picked.retain(|c| wanted.iter().all(|w| c.tags.contains(w)));
    picked.retain(|c| !c.drained);

    // Reservations: a node that reported less than asked cannot take it;
    // one that reported nothing is not ruled out.
    let cpu = u64::from(rules.reserve_cpu_millis);
    let memory = u64::from(rules.reserve_memory_mb);
    picked.retain(|c| c.cpu_millis.is_none_or(|n| n >= cpu) && c.memory_mb.is_none_or(|n| n >= memory));
    picked.sort_by_key(|c| (!c.current, !c.online, c.display_name.clone().unwrap_or_default(), c.hostname.clone()));

    match rules.count {
        Count::All => {}
        Count::Global => picked.retain(|c| c.online || c.current),
        Count::Replicas(n) => {
            let n = n as usize;
            let online = picked.iter().filter(|c| c.online).count();

            if online >= n {
                picked.retain(|c| c.online);
            }

            picked = match rules.spread {
                Some(key) => spread(&picked, key, n),
                None => picked.into_iter().take(n).collect(),
            };
        }
    }

    if picked.is_empty() {
        return Err(PlaceError::NoneMatch);
    }

    Ok(picked.into_iter().map(|c| c.id.clone()).collect())
}

/// `n` of `ordered`, taken in turns from each value of tag `key` (nodes
/// without the tag form one group), so the copies land on as many values
/// as possible. Groups keep the order of their best node.
fn spread<'a>(ordered: &[&'a Candidate], key: &str, n: usize) -> Vec<&'a Candidate> {
    let mut groups: Vec<(Option<&str>, Vec<&'a Candidate>)> = Vec::new();

    for c in ordered {
        let value = c.tag(key);

        match groups.iter_mut().find(|(v, _)| *v == value) {
            Some((_, members)) => members.push(c),
            None => groups.push((value, vec![c])),
        }
    }

    let mut out = Vec::with_capacity(n);
    let mut round = 0;

    while out.len() < n {
        let before = out.len();

        for (_, members) in &groups {
            if out.len() == n {
                break;
            }

            if let Some(c) = members.get(round) {
                out.push(*c);
            }
        }

        if out.len() == before {
            break;
        }

        round += 1;
    }

    out
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    fn node(id: &str, tags: &[&str]) -> Candidate {
        Candidate {
            id: id.into(),
            hostname: id.into(),
            tags: tags.iter().map(|t| (*t).to_owned()).collect(),
            online: true,
            ..Candidate::default()
        }
    }

    fn rules<'a>(count: Count, tags: &'a BTreeMap<String, String>) -> Rules<'a> {
        Rules { nodes: &[], tags, count, spread: None, reserve_cpu_millis: 0, reserve_memory_mb: 0 }
    }

    #[test]
    fn replicas_take_online_nodes_first_and_prefer_current_ones() {
        let none = BTreeMap::new();
        let mut a = node("a", &[]);
        a.online = false;
        let mut c = node("c", &[]);
        c.current = true;
        let nodes = vec![a, node("b", &[]), c];

        assert_eq!(select(&nodes, &rules(Count::Replicas(2), &none)).unwrap(), ["c", "b"]);
        assert_eq!(select(&nodes, &rules(Count::Replicas(1), &none)).unwrap(), ["c"]);

        // Too few online nodes: an offline one fills the gap.
        assert_eq!(select(&nodes, &rules(Count::Replicas(3), &none)).unwrap().len(), 3);
        assert_eq!(select(&nodes, &rules(Count::All, &none)).unwrap().len(), 3);
    }

    #[test]
    fn global_takes_every_online_matching_node_and_skips_drained_ones() {
        let none = BTreeMap::new();
        let mut off = node("off", &[]);
        off.online = false;
        let mut drained = node("drained", &[]);
        drained.drained = true;
        let nodes = vec![node("a", &[]), node("b", &[]), off, drained];

        assert_eq!(select(&nodes, &rules(Count::Global, &none)).unwrap(), ["a", "b"]);
    }

    #[test]
    fn constraints_name_nodes_and_require_tags() {
        let zone = BTreeMap::from([("zone".to_owned(), "a".to_owned())]);
        let nodes = vec![node("n1", &["zone:a"]), node("n2", &["zone:b"]), node("n3", &["zone:a", "disk:ssd"])];
        let got = select(&nodes, &rules(Count::Global, &zone)).unwrap();
        assert_eq!(got, ["n1", "n3"]);

        let named = ["N2".to_owned()];
        let none = BTreeMap::new();
        let r = Rules { nodes: &named, ..rules(Count::Global, &none) };
        assert_eq!(select(&nodes, &r).unwrap(), ["n2"]);

        let missing = ["ghost".to_owned()];
        let r = Rules { nodes: &missing, ..rules(Count::Global, &none) };
        assert_eq!(select(&nodes, &r), Err(PlaceError::UnknownNode("ghost".into())));

        let nowhere = BTreeMap::from([("zone".to_owned(), "c".to_owned())]);
        assert_eq!(select(&nodes, &rules(Count::Global, &nowhere)), Err(PlaceError::NoneMatch));
    }

    #[test]
    fn spread_lands_on_as_many_tag_values_as_possible() {
        let none = BTreeMap::new();
        let nodes = vec![node("a1", &["zone:a"]), node("a2", &["zone:a"]), node("b1", &["zone:b"]), node("x", &[])];
        let r = Rules { spread: Some("zone"), ..rules(Count::Replicas(3), &none) };
        let got = select(&nodes, &r).unwrap();
        assert_eq!(got, ["a1", "b1", "x"]);

        let r = Rules { spread: Some("zone"), ..rules(Count::Replicas(4), &none) };
        assert_eq!(select(&nodes, &r).unwrap(), ["a1", "b1", "x", "a2"]);
    }

    #[test]
    fn reservations_rule_out_nodes_that_reported_too_little() {
        let none = BTreeMap::new();
        let mut small = node("small", &[]);
        small.cpu_millis = Some(1000);
        small.memory_mb = Some(512);
        let mut big = node("big", &[]);
        big.cpu_millis = Some(8000);
        big.memory_mb = Some(16_384);
        let unknown = node("unknown", &[]);
        let nodes = vec![small, big, unknown];
        let r = Rules { reserve_cpu_millis: 2000, reserve_memory_mb: 1024, ..rules(Count::Global, &none) };

        assert_eq!(select(&nodes, &r).unwrap(), ["big", "unknown"]);
    }
}
