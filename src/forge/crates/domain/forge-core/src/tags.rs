//! Tags and node facts.
//!
//! Tags are free-text labels users put on nodes, clusters, environments and
//! services for their own reference: plain (`active`, `standby`) or
//! `key:value` (`location:karachi`, `dc:fra1`). Node facts are the last
//! system details a node reported (OS, kernel, CPUs).

use std::collections::HashMap;

use async_trait::async_trait;
use serde::{Deserialize, Serialize};

use crate::{DomainResult, ServerId, Timestamp};

/// Longest tag, in characters.
pub const MAX_TAG_CHARS: usize = 64;
/// Most tags on one item.
pub const MAX_TAGS: usize = 50;

/// What a tag is attached to.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TagKind {
    Server,
    Cluster,
    Environment,
    Service,
}

impl TagKind {
    pub const ALL: [TagKind; 4] = [TagKind::Server, TagKind::Cluster, TagKind::Environment, TagKind::Service];

    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            TagKind::Server => "server",
            TagKind::Cluster => "cluster",
            TagKind::Environment => "environment",
            TagKind::Service => "service",
        }
    }
}

impl std::str::FromStr for TagKind {
    type Err = String;
    fn from_str(s: &str) -> Result<Self, String> {
        TagKind::ALL
            .into_iter()
            .find(|k| k.as_str() == s)
            .ok_or_else(|| format!("unknown tag kind `{s}` (server, cluster, environment, service)"))
    }
}

/// Checks and normalises a tag set: trimmed, non-empty, at most
/// [`MAX_TAG_CHARS`], no control characters, `key:value` with a non-empty
/// key, duplicates (case-insensitive) dropped, at most [`MAX_TAGS`], sorted.
pub fn normalise_tags(input: &[String]) -> Result<Vec<String>, String> {
    let mut out: Vec<String> = Vec::new();

    for raw in input {
        let t = raw.trim();
        if t.is_empty() {
            continue;
        }

        if t.chars().count() > MAX_TAG_CHARS {
            return Err(format!("tag `{t}` is longer than {MAX_TAG_CHARS} characters"));
        }

        if t.chars().any(char::is_control) || t.contains(',') {
            return Err(format!("tag `{t}` contains a control character or a comma"));
        }

        if let Some((k, v)) = t.split_once(':')
            && (k.trim().is_empty() || v.trim().is_empty())
        {
            return Err(format!("tag `{t}`: use `key:value` with both parts, or a plain word"));
        }

        let t = match t.split_once(':') {
            Some((k, v)) => format!("{}:{}", k.trim(), v.trim()),
            None => t.to_owned(),
        };

        if !out.iter().any(|o| o.eq_ignore_ascii_case(&t)) {
            out.push(t);
        }
    }

    if out.len() > MAX_TAGS {
        return Err(format!("at most {MAX_TAGS} tags per item"));
    }

    out.sort_by_key(|t| t.to_lowercase());

    Ok(out)
}

/// Whether `tags` contains every tag in `wanted` (case-insensitive).
#[must_use]
pub fn has_all(tags: &[String], wanted: &[String]) -> bool {
    wanted.iter().all(|w| tags.iter().any(|t| t.eq_ignore_ascii_case(w.trim())))
}

#[async_trait]
pub trait TagRepository: Send + Sync + std::fmt::Debug {
    /// Replaces the tags of one item.
    async fn set_tags(&self, kind: TagKind, id: &str, tags: &[String], now: Timestamp) -> DomainResult<()>;
    async fn tags_of(&self, kind: TagKind, id: &str) -> DomainResult<Vec<String>>;
    /// Tags of every item of a kind: id → tags.
    async fn tags_of_kind(&self, kind: TagKind) -> DomainResult<HashMap<String, Vec<String>>>;
    /// Tags in use for a kind, with how many items carry each.
    async fn distinct_tags(&self, kind: TagKind) -> DomainResult<Vec<(String, u64)>>;
}

#[async_trait]
pub trait FactsRepository: Send + Sync + std::fmt::Debug {
    async fn put_facts(&self, server: ServerId, facts: &serde_json::Value, now: Timestamp) -> DomainResult<()>;
    /// Every node's facts and when they were collected.
    async fn all_facts(&self) -> DomainResult<HashMap<ServerId, (serde_json::Value, Timestamp)>>;
    /// One node's facts and when they were collected, if any.
    async fn facts_of(&self, server: ServerId) -> DomainResult<Option<(serde_json::Value, Timestamp)>>;
}

/// SSH accounts offered for a node's terminal.
#[async_trait]
pub trait NodeLoginRepository: Send + Sync + std::fmt::Debug {
    async fn set_logins(&self, server: ServerId, logins: &[String], now: Timestamp) -> DomainResult<()>;
    async fn logins_of(&self, server: ServerId) -> DomainResult<Vec<String>>;
}

/// POSIX login names, as sshd accepts them: 1–32 of `A-Za-z0-9_.-`, not
/// starting with `-`.
#[must_use]
pub fn valid_login(login: &str) -> bool {
    !login.is_empty()
        && login.len() <= 32
        && !login.starts_with('-')
        && login.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '-' | '.'))
}

/// Names Forge never creates as a terminal account (D-078), even when they
/// are free on a node: system accounts and groups that grant privileges.
const RESERVED_ACCOUNTS: &[&str] = &[
    "root",
    "nobody",
    "forge-agent",
    "daemon",
    "bin",
    "sys",
    "sync",
    "games",
    "man",
    "lp",
    "mail",
    "news",
    "uucp",
    "proxy",
    "www-data",
    "backup",
    "list",
    "irc",
    "gnats",
    "sshd",
    "messagebus",
    "admin",
    "adm",
    "sudo",
    "wheel",
    "docker",
    "polkitd",
    "halt",
    "shutdown",
    "operator",
];

/// Terminal account names Forge may create (D-078): `[a-z_][a-z0-9_-]{0,31}`,
/// the portable subset `useradd` accepts everywhere, and no reserved or
/// `systemd-*` name. The node also refuses names of existing system users.
#[must_use]
pub fn valid_account_name(name: &str) -> bool {
    let mut c = name.chars();
    c.next().is_some_and(|f| f.is_ascii_lowercase() || f == '_')
        && name.len() <= 32
        && c.all(|ch| ch.is_ascii_lowercase() || ch.is_ascii_digit() || ch == '_' || ch == '-')
        && !RESERVED_ACCOUNTS.contains(&name)
        && !name.starts_with("systemd-")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn v(xs: &[&str]) -> Vec<String> {
        xs.iter().map(|s| (*s).to_owned()).collect()
    }

    #[test]
    fn tags_are_normalised_and_validated() {
        let got = normalise_tags(&v(&[" dc : fra1 ", "active", "", "ACTIVE", "location:karachi"])).unwrap_or_default();
        assert_eq!(got, v(&["active", "dc:fra1", "location:karachi"]));
        assert!(normalise_tags(&v(&[":x"])).is_err());
        assert!(normalise_tags(&v(&["k:"])).is_err());
        assert!(normalise_tags(&v(&["a,b"])).is_err());
        assert!(normalise_tags(&v(&["bad\ntag"])).is_err());
        assert!(normalise_tags(&[("x".repeat(65))]).is_err());
        let many: Vec<String> = (0..51).map(|i| format!("t{i}")).collect();
        assert!(normalise_tags(&many).is_err());
    }

    #[test]
    fn login_names_follow_sshd_rules() {
        for ok in ["alice", "deploy", "svc.web", "a_b-c", "root"] {
            assert!(valid_login(ok), "{ok}");
        }

        for bad in ["", "-x", "a b", "a;b", "a/b", &"x".repeat(33)] {
            assert!(!valid_login(bad), "{bad}");
        }
    }

    #[test]
    fn filters_match_case_insensitively() {
        let tags = v(&["dc:fra1", "active"]);
        assert!(has_all(&tags, &v(&["DC:FRA1"])));
        assert!(has_all(&tags, &v(&["active", "dc:fra1"])));
        assert!(!has_all(&tags, &v(&["standby"])));
        assert!("server".parse::<TagKind>().is_ok() && "x".parse::<TagKind>().is_err());
    }
}
