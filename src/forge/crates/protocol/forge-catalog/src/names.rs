//! Local names (D-084): every node learns the names of the fleet's nodes and
//! services (`<node>.<cluster>.local`, `<app>.<ns>.<cluster>.local`, or
//! without the cluster label outside a cluster) in a marked block of its
//! hosts file.
//!
//! Shared by the control plane, which builds the lists, and the node, which
//! checks every entry again before it writes anything. Names are only ever
//! made of validated labels and IPs are parsed addresses written through
//! `Display`, so no entry can carry whitespace, a newline or `#` into the
//! hosts file.

use std::net::IpAddr;

use serde::{Deserialize, Serialize};

mod records;

pub use records::{
    RECORDS_FILE, RECORDS_TTL, RECORDS_VERSION, Record, RecordInput, Records, SrvPort, build_records,
    first_container_port, ptr_name, replica_clashes, reverse_zone, search_list,
};

/// Suffix of every name unless the operator sets another (`names.suffix`).
pub const DEFAULT_SUFFIX: &str = "local";
/// The hosts-file block written for the fleet's names.
pub const BLOCK_TAG: &str = "names";
/// Most entries one node receives.
pub const MAX_ENTRIES: usize = 8192;

/// Labels no node, cluster, environment or service name may use: they
/// would shadow names every system resolves itself.
const RESERVED: &[&str] = &[
    "localhost",
    "local",
    "localdomain",
    "localhost6",
    "ip6-localhost",
    "ip6-loopback",
    "ip6-localnet",
    "ip6-mcastprefix",
    "ip6-allnodes",
    "ip6-allrouters",
    "broadcasthost",
    "wpad",
    "isatap",
];

/// One-label suffixes accepted: special-use or private names that can never
/// be a public top-level domain. A suffix of two or more labels (a domain
/// the operator owns, such as `corp.example.com`) is accepted too.
const SINGLE_LABEL_SUFFIXES: &[&str] = &["local", "internal", "lan", "home", "test", "localdomain", "intranet", "corp"];

/// A DNS label: `[a-z0-9-]`, 1 to 63 characters, no leading or trailing `-`.
#[must_use]
pub fn valid_label(l: &str) -> bool {
    !l.is_empty()
        && l.len() <= 63
        && !l.starts_with('-')
        && !l.ends_with('-')
        && l.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
}

/// A label a user may give a node (or a cluster, environment or service
/// may yield): valid and not reserved.
pub fn check_label(l: &str) -> Result<(), String> {
    if !valid_label(l) {
        return Err(format!(
            "`{l}` is not a DNS label: use 1 to 63 lowercase letters, digits and `-`, not starting or ending with `-`"
        ));
    }

    if RESERVED.contains(&l) {
        return Err(format!("`{l}` is reserved"));
    }

    Ok(())
}

/// A full name: at least two valid labels, at most 253 characters, and no
/// label that would shadow `localhost`.
#[must_use]
pub fn valid_name(n: &str) -> bool {
    n.len() <= 253
        && n.contains('.')
        && n.split('.').all(valid_label)
        && !n.split('.').any(|l| l == "localhost" || l == "localhost6")
}

/// Longest suffix accepted, so every name built on it stays well inside
/// the 253-character DNS limit.
pub const MAX_SUFFIX_LEN: usize = 63;

/// Checks the suffix every name ends in: lowercase DNS labels separated by
/// dots, at most [`MAX_SUFFIX_LEN`] characters, no `localhost` label, and a
/// single label only when it can never be a public top-level domain. The
/// control plane (config file and runtime setting) and the agent use this
/// same check.
pub fn check_suffix(s: &str) -> Result<(), String> {
    let labels: Vec<&str> = s.split('.').collect();
    if s.len() > MAX_SUFFIX_LEN || !labels.iter().all(|l| valid_label(l)) {
        return Err(format!(
            "`{s}` is not a domain suffix: use lowercase DNS labels separated by dots, at most 63 characters in all"
        ));
    }

    if labels.iter().any(|l| *l == "localhost" || *l == "localhost6") {
        return Err("the suffix cannot contain `localhost`".into());
    }

    if labels.len() == 1 && !SINGLE_LABEL_SUFFIXES.contains(&s) {
        return Err(format!(
            "`{s}` could be a public top-level domain; use one of {} or a domain you own (`corp.example.com`)",
            SINGLE_LABEL_SUFFIXES.join(", ")
        ));
    }

    Ok(())
}

/// A DNS label from a free-form name (`Web Server_1` → `web-server-1`), or
/// `None` when nothing usable is left. Reserved results are refused too.
#[must_use]
pub fn label_from(name: &str) -> Option<String> {
    let mut l = String::new();

    for c in name.to_ascii_lowercase().chars() {
        let c = if c.is_ascii_alphanumeric() { c } else { '-' };
        if !(c == '-' && (l.is_empty() || l.ends_with('-'))) {
            l.push(c);
        }
    }

    let l: String = l.trim_matches('-').chars().take(63).collect();
    let l = l.trim_end_matches('-').to_owned();

    check_label(&l).is_ok().then_some(l)
}

/// One hosts-file line: a name and the address it resolves to.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NameEntry {
    pub name: String,
    pub ip: IpAddr,
}

impl NameEntry {
    /// A valid name and a usable unicast address.
    pub fn validate(&self) -> Result<(), String> {
        if !valid_name(&self.name) {
            return Err(format!("invalid name `{}`", self.name.escape_default()));
        }

        if self.ip.is_unspecified() || self.ip.is_multicast() {
            return Err(format!("invalid address for `{}`", self.name));
        }

        Ok(())
    }
}

/// A container port published on the node, from an app's `ports` entry
/// (`8080:80`, `127.0.0.1:9000:9000/udp`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PublishedPort {
    /// Port on the node.
    pub port: u16,
    /// `tcp` or `udp`.
    pub proto: String,
    /// Port inside the container.
    pub target: u16,
    /// Address the port is bound to on the node, when not all addresses.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub bind: Option<IpAddr>,
}

/// Parses an app's port entry; `None` for anything malformed.
#[must_use]
pub fn published_port(p: &str) -> Option<PublishedPort> {
    let (body, proto) = p.split_once('/').unwrap_or((p, "tcp"));
    if !matches!(proto, "tcp" | "udp") {
        return None;
    }

    let num = |s: &str| s.parse::<u16>().ok().filter(|n| *n > 0);
    let parts: Vec<&str> = body.split(':').collect();
    let (bind, port, target) = match parts.as_slice() {
        [h, c] => (None, num(h)?, num(c)?),
        [ip, h, c] => (Some(ip.parse::<IpAddr>().ok()?), num(h)?, num(c)?),
        _ => return None,
    };

    Some(PublishedPort { port, proto: proto.to_owned(), target, bind: bind.filter(|b| !b.is_unspecified()) })
}

/// Checks a whole list (size and every entry).
pub fn validate_entries(entries: &[NameEntry]) -> Result<(), String> {
    if entries.len() > MAX_ENTRIES {
        return Err(format!("at most {MAX_ENTRIES} names"));
    }

    entries.iter().try_for_each(NameEntry::validate)
}

/// The block's marker lines for `tag` (same format as the fabric's block).
#[must_use]
pub fn markers(tag: &str) -> (String, String) {
    (format!("# BEGIN orbyn-forge {tag}"), format!("# END orbyn-forge {tag}"))
}

/// Byte ranges of each line, with the line's text without its line ending.
fn lines_with_ranges(text: &str) -> Vec<(usize, usize, &str)> {
    let mut out = Vec::new();
    let mut start = 0;

    for piece in text.split_inclusive('\n') {
        let end = start + piece.len();
        out.push((start, end, piece.trim_end_matches('\n').trim_end_matches('\r')));
        start = end;
    }

    out
}

/// Replaces the `tag` block of a hosts file's text with `entries` (an empty
/// list removes it) and keeps every byte outside the block as it was.
///
/// * One well-formed block: only its bytes are replaced.
/// * No block: a new one is appended (after a line break, if the text
///   lacks a final one).
/// * Damaged markers (one missing, duplicated or out of order): nothing is
///   deleted; a block that ends the file and follows the last begin marker
///   is ours from an earlier repair and is replaced, otherwise a fresh
///   block is appended.
pub fn render_block(current: &str, tag: &str, entries: &[NameEntry]) -> Result<String, String> {
    validate_entries(entries)?;
    let (begin, end) = markers(tag);
    let nl = if current.contains("\r\n") { "\r\n" } else { "\n" };
    let mut block = String::new();

    if !entries.is_empty() {
        block.push_str(&begin);
        block.push_str(nl);
        block.push_str("# Names on the Forge network. Managed by Orbyn Forge; edits are overwritten.");
        block.push_str(nl);

        for e in entries {
            block.push_str(&format!("{}\t{}{nl}", e.ip, e.name));
        }

        block.push_str(&end);
        block.push_str(nl);
    }

    let lines = lines_with_ranges(current);
    let begins: Vec<usize> = (0..lines.len()).filter(|i| lines[*i].2 == begin).collect();
    let ends: Vec<usize> = (0..lines.len()).filter(|i| lines[*i].2 == end).collect();
    let replace = |from: usize, to: usize| {
        let mut out = String::with_capacity(current.len() + block.len());
        out.push_str(&current[..lines[from].0]);
        out.push_str(&block);
        out.push_str(&current[lines[to].1..]);
        out
    };
    let append = || {
        if block.is_empty() {
            return current.to_owned();
        }

        let mut out = current.to_owned();
        if !out.is_empty() && !out.ends_with('\n') {
            out.push_str(nl);
        }

        out.push_str(&block);
        out
    };

    Ok(match (begins.as_slice(), ends.as_slice()) {
        ([], []) => append(),
        ([b], [e]) if b < e => replace(*b, *e),
        _ => {
            // Damaged: reuse a trailing block after the last begin marker.
            let last_begin = begins.last().copied();
            let trailing_end = last_begin.and_then(|b| ends.iter().copied().find(|e| *e > b));
            match (last_begin, trailing_end) {
                (Some(b), Some(e)) if lines[e + 1..].iter().all(|l| l.2.trim().is_empty()) => replace(b, e),
                _ => append(),
            }
        }
    })
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    fn e(name: &str, ip: &str) -> NameEntry {
        NameEntry { name: name.into(), ip: ip.parse().unwrap() }
    }

    #[test]
    fn labels_and_names_are_strict() {
        for ok in ["web-1", "a", "x0", &"a".repeat(63)] {
            assert!(check_label(ok).is_ok(), "{ok}");
        }

        for bad in ["", "-a", "a-", "A", "a_b", "a.b", "a b", "a\nb", "a#b", "é", &"a".repeat(64), "localhost", "local"]
        {
            assert!(check_label(bad).is_err(), "{bad:?}");
        }

        assert!(valid_name("web-1.lab.local"));
        assert!(!valid_name("web"), "one label");
        assert!(!valid_name("localhost.local"));
        assert!(!valid_name("web..local"));
        assert!(!valid_name(&format!("{}.local", vec!["a".repeat(63); 4].join("."))), "over 253");
    }

    #[test]
    fn suffixes_never_shadow_public_names() {
        for ok in [
            "local",
            "internal",
            "home.arpa",
            "corp.internal",
            "corp.example.com",
            "lan",
            &format!("{}.internal", "a".repeat(54)),
        ] {
            assert!(check_suffix(ok).is_ok(), "{ok}");
        }

        for bad in [
            "com",
            "net",
            "org",
            "io",
            "",
            "Local",
            "a..b",
            "localhost",
            "x.localhost",
            "-a.b",
            "a-.b",
            "a.b.",
            ".a.b",
            "lab_1.internal",
            &format!("{}.internal", "a".repeat(55)),
        ] {
            assert!(check_suffix(bad).is_err(), "{bad:?}");
        }
    }

    #[test]
    fn labels_from_free_names() {
        assert_eq!(label_from("Web Server_1").as_deref(), Some("web-server-1"));
        assert_eq!(label_from("--a--b--").as_deref(), Some("a-b"));
        assert_eq!(label_from("üï ."), None);
        assert_eq!(label_from("localhost"), None);
        let long = label_from(&format!("{}-b", "a".repeat(62))).unwrap();
        assert!(long.len() <= 63 && !long.ends_with('-'));
    }

    #[test]
    fn entries_cannot_inject_lines() {
        for bad in ["a.local\n1.2.3.4 evil", "a.local #x", "a .local", "a\t.local"] {
            assert!(NameEntry { name: bad.into(), ip: "10.0.0.1".parse().unwrap() }.validate().is_err(), "{bad:?}");
        }

        assert!(e("a.local", "0.0.0.0").validate().is_err());
        assert!(e("a.local", "224.0.0.1").validate().is_err());

        // Unknown fields are refused on the wire.
        assert!(serde_json::from_str::<NameEntry>(r#"{"name":"a.local","ip":"10.0.0.1","x":1}"#).is_err());
        assert!(serde_json::from_str::<NameEntry>(r#"{"name":"a.local","ip":"10.0.0.1 b"}"#).is_err());
    }

    #[test]
    fn the_block_is_replaced_and_nothing_else_changes() {
        let base = "127.0.0.1\tlocalhost\n# my note\n10.1.1.1 printer";
        let one = render_block(base, "names", &[e("a.local", "10.0.0.1")]).unwrap();
        assert!(one.starts_with(base), "every byte kept");
        assert!(one.contains("\n# BEGIN orbyn-forge names\n") && one.contains("10.0.0.1\ta.local\n"));
        let two = render_block(&one, "names", &[e("b.lab.local", "fd00::5")]).unwrap();
        assert!(two.starts_with(base) && !two.contains("a.local") && two.contains("fd00::5\tb.lab.local"));

        // Same list: same text.
        assert_eq!(render_block(&two, "names", &[e("b.lab.local", "fd00::5")]).unwrap(), two);

        // Lines after the block stay too.
        let after = format!("{two}10.9.9.9 later\n");
        let three = render_block(&after, "names", &[e("c.local", "10.0.0.3")]).unwrap();
        assert!(three.starts_with(base) && three.ends_with("10.9.9.9 later\n"));

        // Empty list: the block goes, the rest stays.
        let gone = render_block(&three, "names", &[]).unwrap();
        assert_eq!(gone, format!("{base}\n10.9.9.9 later\n"));

        // Nothing to write and no block: unchanged, byte for byte.
        assert_eq!(render_block(base, "names", &[]).unwrap(), base);

        // Other blocks (the fabric's) are left alone.
        let fab = "# BEGIN orbyn-forge forge0\n10.64.0.1\tn.lab.local\n# END orbyn-forge forge0\n";
        let mixed = render_block(fab, "names", &[e("a.local", "10.0.0.1")]).unwrap();
        assert!(mixed.starts_with(fab));
    }

    #[test]
    fn crlf_files_keep_crlf() {
        let base = "127.0.0.1 localhost\r\n";
        let out = render_block(base, "names", &[e("a.local", "10.0.0.1")]).unwrap();
        assert_eq!(
            out,
            "127.0.0.1 localhost\r\n# BEGIN orbyn-forge names\r\n# Names on the Forge network. Managed by Orbyn Forge; edits are overwritten.\r\n10.0.0.1\ta.local\r\n# END orbyn-forge names\r\n"
        );
    }

    #[test]
    fn damaged_markers_never_delete_lines_and_do_not_grow() {
        let lost_end = "127.0.0.1 localhost\n# BEGIN orbyn-forge names\n10.0.0.1\told.local\n10.1.1.1 printer\n";
        let one = render_block(lost_end, "names", &[e("a.local", "10.0.0.1")]).unwrap();
        assert!(one.starts_with(lost_end), "nothing deleted");

        // The appended block is reused next time: no growth.
        let two = render_block(&one, "names", &[e("b.local", "10.0.0.2")]).unwrap();
        assert!(two.starts_with(lost_end) && two.contains("b.local") && !two.contains("a.local"));
        assert_eq!(two.matches("# BEGIN orbyn-forge names").count(), 2);
        let stray_end = "# END orbyn-forge names\n127.0.0.1 localhost\n";
        let three = render_block(stray_end, "names", &[e("a.local", "10.0.0.1")]).unwrap();
        assert!(three.starts_with(stray_end));
        let twice = format!("{one}{one}");
        let four = render_block(&twice, "names", &[e("c.local", "10.0.0.3")]).unwrap();
        assert_eq!(four.matches("10.1.1.1 printer").count(), 2, "user lines kept");
        assert!(four.contains("c.local"));
        assert_eq!(render_block(&four, "names", &[e("c.local", "10.0.0.3")]).unwrap(), four, "stable");
    }

    #[test]
    fn published_ports_are_parsed() {
        let p = published_port("8080:80").unwrap();
        assert_eq!((p.port, p.proto.as_str(), p.target, p.bind), (8080, "tcp", 80, None));
        let p = published_port("127.0.0.1:9000:9001/udp").unwrap();
        assert_eq!((p.port, p.proto.as_str(), p.target), (9000, "udp", 9001));
        assert_eq!(p.bind, Some("127.0.0.1".parse().unwrap()));
        assert_eq!(published_port("0.0.0.0:1:2").unwrap().bind, None);

        for bad in ["80", "a:80", "0:80", "1:2/sctp", "x:1:2", "1:2:3:4"] {
            assert!(published_port(bad).is_none(), "{bad}");
        }
    }

    #[test]
    fn invalid_entries_are_refused_before_writing() {
        assert!(render_block("", "names", &[e("a.local", "10.0.0.1"), e("bad", "10.0.0.2")]).is_err());
    }
}
