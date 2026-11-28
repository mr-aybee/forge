//! `forge.stack.yaml`: a compose stack, where it runs and how it rolls out
//! (D-174).
//!
//! The server and the CLI read the YAML (with the bounds of
//! `forge_server::deploy::parse_yaml`); this module validates the result as
//! a [`Value`], so it stays free of I/O and of a YAML parser. The CLI reads
//! `spec.compose.file` and `spec.compose.envFile` and sends their text; the
//! server never reads files.
//!
//! Every problem is a [`DocError`] that names the field path
//! (`spec.rollout.healthTimeout: …`).

use std::collections::BTreeMap;
use std::fmt::Write as _;

use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

use crate::deploy::{DocError, Secs, merge_patch, valid_subpath};
use crate::stacks::valid_stack_name;

/// `apiVersion` of a stack document.
pub const API_VERSION: &str = "forge/v1";
/// `kind` of a stack document.
pub const KIND: &str = "Stack";
/// Default `spec.rollout.healthTimeout`, in seconds.
pub const DEFAULT_HEALTH_TIMEOUT: u64 = 300;
/// Shortest `spec.rollout.healthTimeout`, in seconds.
pub const MIN_HEALTH_TIMEOUT: u64 = 10;
/// Longest `spec.rollout.healthTimeout`, in seconds.
pub const MAX_HEALTH_TIMEOUT: u64 = 3_600;
/// Most nodes `spec.placement.nodes` may list.
pub const MAX_NODES: usize = 200;
/// The refusal for several nodes without per-service placement.
pub const SEVERAL_NODES: &str =
    "placement.nodes lists several nodes; name the node per service with x-forge.node or use placement.cluster";

/// The parsed document, with the environment's override merged in.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct StackDoc {
    /// `forge/v1`.
    pub api_version: String,
    /// `Stack`.
    pub kind: String,
    pub metadata: Metadata,
    pub spec: StackSpec,
}

/// `metadata`: the stack's name and environment.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Metadata {
    /// The stack's name (created on the first apply).
    pub name: String,
    /// The environment's slug (`prod`, `staging`, …).
    pub environment: String,
}

/// `spec`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct StackSpec {
    pub compose: ComposeSource,
    #[serde(default)]
    pub placement: PlacementSpec,
    #[serde(default)]
    pub network: NetworkSpec,
    #[serde(default)]
    pub rollout: RolloutSpec,
    /// Per-environment changes, merged over `spec` before it is read.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub overrides: BTreeMap<String, Value>,
}

/// `spec.compose`: exactly one of `file` and `inline`.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ComposeSource {
    /// The compose file, relative to the document.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub file: Option<String>,
    /// The compose document itself.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub inline: Option<String>,
    /// A `.env` file, relative to the document (sealed by the server).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub env_file: Option<String>,
}

/// `spec.placement`: nodes or a cluster, never both.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PlacementSpec {
    /// Node names; the first is the stack's default node.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub nodes: Vec<String>,
    /// A cluster: every service without its own placement runs on each node.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cluster: Option<String>,
}

/// `spec.network`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct NetworkSpec {
    /// Join the cluster container network (top-level `x-forge.mesh`).
    #[serde(default = "yes")]
    pub mesh: bool,
}

impl Default for NetworkSpec {
    fn default() -> Self {
        Self { mesh: true }
    }
}

const fn yes() -> bool {
    true
}

/// `spec.rollout`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RolloutSpec {
    #[serde(default)]
    pub order: RolloutOrder,
    /// How long a node's services may take to become healthy.
    #[serde(default = "default_health_timeout")]
    pub health_timeout: Secs,
}

impl Default for RolloutSpec {
    fn default() -> Self {
        Self { order: RolloutOrder::default(), health_timeout: default_health_timeout() }
    }
}

const fn default_health_timeout() -> Secs {
    Secs(DEFAULT_HEALTH_TIMEOUT)
}

/// The order nodes are deployed in.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum RolloutOrder {
    /// One node at a time; a failed node stops the rollout.
    #[default]
    NodeByNode,
    /// Every node at once.
    AllAtOnce,
}

// ---- field paths -------------------------------------------------------------------

const ROOT_KEYS: &[&str] = &["apiVersion", "kind", "metadata", "spec"];
const METADATA_KEYS: &[&str] = &["name", "environment"];
const SPEC_KEYS: &[&str] = &["compose", "placement", "network", "rollout", "overrides"];
const COMPOSE_KEYS: &[&str] = &["file", "inline", "envFile"];
const PLACEMENT_KEYS: &[&str] = &["nodes", "cluster"];
const NETWORK_KEYS: &[&str] = &["mesh"];
const ROLLOUT_KEYS: &[&str] = &["order", "healthTimeout"];
const OVERRIDE_KEYS: &[&str] = &["placement", "network", "rollout", "compose"];
const OVERRIDE_COMPOSE_KEYS: &[&str] = &["envFile"];

fn join(path: &str, key: &str) -> String {
    if path.is_empty() { key.to_owned() } else { format!("{path}.{key}") }
}

/// The object at `path`, or an error; a missing value is an empty object.
fn object<'a>(v: Option<&'a Value>, path: &str) -> Result<Option<&'a Map<String, Value>>, DocError> {
    match v {
        None => Ok(None),
        Some(Value::Object(m)) => Ok(Some(m)),
        Some(_) => Err(DocError::new(path, "must be a mapping")),
    }
}

/// Refuses keys of the object at `path` that are not in `allowed`.
fn known_keys(v: Option<&Value>, path: &str, allowed: &[&str]) -> Result<(), DocError> {
    let Some(m) = object(v, path)? else { return Ok(()) };

    if let Some(k) = m.keys().find(|k| !allowed.contains(&k.as_str())) {
        return Err(DocError::new(&join(path, k), format!("unknown field; expected one of {}", allowed.join(", "))));
    }

    Ok(())
}

/// Checks the shape of a `spec`-like object: `spec` itself or an override.
fn known_spec_keys(spec: Option<&Value>, path: &str, keys: &[&str], compose: &[&str]) -> Result<(), DocError> {
    known_keys(spec, path, keys)?;
    let at = |k: &str| spec.and_then(|s| s.get(k));
    known_keys(at("compose"), &join(path, "compose"), compose)?;
    known_keys(at("placement"), &join(path, "placement"), PLACEMENT_KEYS)?;
    known_keys(at("network"), &join(path, "network"), NETWORK_KEYS)?;

    known_keys(at("rollout"), &join(path, "rollout"), ROLLOUT_KEYS)
}

/// Refuses unknown fields anywhere in the document, with their path.
fn check_shape(doc: &Value) -> Result<(), DocError> {
    if !doc.is_object() {
        return Err(DocError::new("", "the document must be a mapping with apiVersion, kind, metadata and spec"));
    }

    known_keys(Some(doc), "", ROOT_KEYS)?;
    known_keys(doc.get("metadata"), "metadata", METADATA_KEYS)?;
    let spec = doc.get("spec");
    known_spec_keys(spec, "spec", SPEC_KEYS, COMPOSE_KEYS)?;
    let overrides = spec.and_then(|s| s.get("overrides"));

    for (env, o) in object(overrides, "spec.overrides")?.into_iter().flatten() {
        let path = format!("spec.overrides.{env}");
        object(Some(o), &path)?;
        known_spec_keys(Some(o), &path, OVERRIDE_KEYS, OVERRIDE_COMPOSE_KEYS)?;
    }

    Ok(())
}

/// Reads `v[key]` as `T`, naming `path` on a type error.
fn field<T: DeserializeOwned>(v: &Value, key: &str, path: &str) -> Result<Option<T>, DocError> {
    v.get(key)
        .map(|x| serde_json::from_value(x.clone()).map_err(|e| DocError::new(&join(path, key), strip_position(&e))))
        .transpose()
}

/// serde's message without the ` at line 1 column 2` of a `Value` decode.
fn strip_position(e: &serde_json::Error) -> String {
    let msg = e.to_string();

    match msg.find(" at line ") {
        Some(i) => msg[..i].to_owned(),
        None => msg,
    }
}

fn need<T>(v: Option<T>, path: &str) -> Result<T, DocError> {
    v.ok_or_else(|| DocError::new(path, "missing"))
}

/// Decodes each section on its own, so a type error names its path.
fn decode(doc: &Value) -> Result<StackDoc, DocError> {
    let spec = doc.get("spec").ok_or_else(|| DocError::new("spec", "missing"))?;
    let metadata = doc.get("metadata").ok_or_else(|| DocError::new("metadata", "missing"))?;

    Ok(StackDoc {
        api_version: need(field(doc, "apiVersion", "")?, "apiVersion")?,
        kind: need(field(doc, "kind", "")?, "kind")?,
        metadata: Metadata {
            name: need(field(metadata, "name", "metadata")?, "metadata.name")?,
            environment: need(field(metadata, "environment", "metadata")?, "metadata.environment")?,
        },
        spec: StackSpec {
            compose: need(field(spec, "compose", "spec")?, "spec.compose")?,
            placement: field(spec, "placement", "spec")?.unwrap_or_default(),
            network: field(spec, "network", "spec")?.unwrap_or_default(),
            rollout: field(spec, "rollout", "spec")?.unwrap_or_default(),
            overrides: field(spec, "overrides", "spec")?.unwrap_or_default(),
        },
    })
}

// ---- parsing -----------------------------------------------------------------------

/// Parses a document from its JSON form (the caller reads the YAML).
/// `environment` replaces `metadata.environment` (`--environment`); the
/// matching `spec.overrides.<env>` is then deep-merged over `spec` (a `null`
/// in an override removes the field, as in a JSON merge patch).
pub fn parse(mut doc: Value, environment: Option<&str>) -> Result<StackDoc, DocError> {
    check_shape(&doc)?;

    if let Some(env) = environment {
        doc["metadata"]["environment"] = Value::String(env.to_owned());
    }

    let env = doc.pointer("/metadata/environment").and_then(Value::as_str).unwrap_or_default().to_owned();

    if let Some(patch) = doc.pointer("/spec/overrides").and_then(|o| o.get(&env)).cloned()
        && let Some(spec) = doc.get_mut("spec")
    {
        merge_patch(spec, &patch);
    }

    let d = decode(&doc)?;
    d.check()?;

    Ok(d)
}

fn check_rel_path(p: &str, path: &str) -> Result<(), DocError> {
    if p.is_empty() || !valid_subpath(p) {
        return Err(DocError::new(path, "a path relative to the document, without `.` or `..` parts"));
    }

    Ok(())
}

impl StackDoc {
    fn check(&self) -> Result<(), DocError> {
        if self.api_version != API_VERSION {
            return Err(DocError::new("apiVersion", format!("must be {API_VERSION}")));
        }

        if self.kind != KIND {
            return Err(DocError::new("kind", format!("must be {KIND}")));
        }

        if !valid_stack_name(&self.metadata.name) {
            return Err(DocError::new(
                "metadata.name",
                "a-z, 0-9, `-` and `_` (up to 63), starting with a letter or digit",
            ));
        }

        if !crate::apps::valid_slug(&self.metadata.environment, 63) {
            return Err(DocError::new("metadata.environment", "the environment's slug, e.g. prod"));
        }

        let c = &self.spec.compose;

        match (&c.file, &c.inline) {
            (Some(f), None) => check_rel_path(f, "spec.compose.file")?,
            (None, Some(i)) if !i.trim().is_empty() => {}
            (None, Some(_)) => return Err(DocError::new("spec.compose.inline", "is empty")),
            _ => return Err(DocError::new("spec.compose", "set exactly one of file and inline")),
        }

        if let Some(e) = &c.env_file {
            check_rel_path(e, "spec.compose.envFile")?;
        }

        let p = &self.spec.placement;

        if !p.nodes.is_empty() && p.cluster.is_some() {
            return Err(DocError::new("spec.placement", "set either nodes or cluster"));
        }

        if p.nodes.len() > MAX_NODES {
            return Err(DocError::new("spec.placement.nodes", format!("at most {MAX_NODES} nodes")));
        }

        if p.nodes.iter().any(|n| n.trim().is_empty()) || p.cluster.as_ref().is_some_and(|c| c.trim().is_empty()) {
            return Err(DocError::new("spec.placement", "node and cluster names are not empty"));
        }

        let t = self.spec.rollout.health_timeout.0;

        if !(MIN_HEALTH_TIMEOUT..=MAX_HEALTH_TIMEOUT).contains(&t) {
            return Err(DocError::new("spec.rollout.healthTimeout", "between 10s and 1h"));
        }

        Ok(())
    }

    /// `spec.rollout.healthTimeout` in seconds.
    #[must_use]
    pub fn health_timeout_s(&self) -> u64 {
        self.spec.rollout.health_timeout.0
    }

    /// Checks the document against its compose tree (parsed, not
    /// interpolated): several `placement.nodes` need per-service placement.
    pub fn check_compose(&self, tree: &Value) -> Result<(), DocError> {
        if self.spec.placement.nodes.len() < 2 {
            return Ok(());
        }

        let placed = |svc: &Value| {
            svc.pointer("/x-forge/node").is_some()
                || svc.pointer("/x-forge/cluster").is_some()
                || svc.pointer("/deploy/placement/constraints").is_some()
        };
        let services = tree.get("services").and_then(Value::as_object);

        if !services.into_iter().flatten().any(|(_, s)| placed(s)) {
            return Err(DocError::new("spec.placement.nodes", SEVERAL_NODES));
        }

        Ok(())
    }

    /// The top-level `x-forge` keys the document adds to the compose file
    /// (`cluster` from `placement.cluster`, `mesh: false` from
    /// `network.mesh`), as YAML to append to its text; `None` when it adds
    /// nothing. A compose file with its own top-level `x-forge` must agree
    /// with the document, since text cannot be merged into it.
    pub fn compose_overlay(&self, tree: &Value) -> Result<Option<String>, DocError> {
        let mut wanted: Vec<(&str, Value, &str)> = Vec::new();

        if let Some(c) = &self.spec.placement.cluster {
            wanted.push(("cluster", Value::String(c.clone()), "spec.placement.cluster"));
        }

        if !self.spec.network.mesh {
            wanted.push(("mesh", Value::Bool(false), "spec.network.mesh"));
        }

        if wanted.is_empty() {
            return Ok(None);
        }

        if let Some(top) = tree.get("x-forge") {
            for (k, v, path) in &wanted {
                if top.get(*k) != Some(v) {
                    return Err(DocError::new(
                        path,
                        format!(
                            "the compose file has its own top-level x-forge; set x-forge.{k} there to match, or remove it"
                        ),
                    ));
                }
            }

            return Ok(None);
        }

        let mut out = String::from("\n# Added by forge.stack.yaml\nx-forge:\n");

        for (k, v, _) in wanted {
            // JSON scalars are YAML scalars: a quoted string stays a string.
            let _ = writeln!(out, "  {k}: {v}");
        }

        Ok(Some(out))
    }
}

#[cfg(test)]
#[path = "stack_doc/tests.rs"]
mod tests;
