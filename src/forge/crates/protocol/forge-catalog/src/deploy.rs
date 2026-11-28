//! Deployments (#73): the `forge.deploy.yaml` document, the rollout policy a
//! release carries, slots, and the planner that turns a policy and the
//! current state of a service's nodes into an ordered list of steps.
//!
//! Everything here is pure: the control plane parses and plans, the job
//! executes the plan step by step.
//!
//! # Slots
//!
//! A service can run two copies side by side on one node, in slots `a` and
//! `b`. Each slot has its own container or systemd unit
//! (`forge-app-<ns>-<app>--<slot>`) and its own host ports from a pool, so a
//! new release starts next to the old one and takes traffic only after it
//! is ready. Strategies that never run two copies at once (recreate,
//! stop-first) deploy in place: into the slot that serves now, or into the
//! unslotted instance (`forge-app-<ns>-<app>`) of a service deployed before.

use std::collections::BTreeMap;

use serde::{Deserialize, Deserializer, Serialize};
use serde_json::Value;

use crate::apps::Runtime;

pub const API_VERSION: &str = "forge/v1";
pub const KIND: &str = "Deployment";
/// Largest deployment document accepted.
pub const MAX_DOC_BYTES: usize = 256 * 1024;
/// Host ports slots are given (one per published port per slot).
pub const SLOT_PORTS: std::ops::RangeInclusive<u16> = 20_000..=29_999;
/// Most nodes one deployment rolls out to.
pub const MAX_NODES: usize = 200;
/// Most canary steps.
pub const MAX_STEPS: usize = 20;

// ---- durations --------------------------------------------------------------------

/// A duration in seconds, written as `90`, `"30s"`, `"5m"`, `"2h"` or `"1d"`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize)]
#[serde(transparent)]
pub struct Secs(pub u64);

impl Secs {
    /// Parses `"30s"`, `"5m"`, `"2h"`, `"1d"` or plain seconds.
    pub fn parse(s: &str) -> Result<Self, String> {
        let s = s.trim();
        let (num, mult) = match s.char_indices().last() {
            Some((i, 's')) => (&s[..i], 1),
            Some((i, 'm')) => (&s[..i], 60),
            Some((i, 'h')) => (&s[..i], 3_600),
            Some((i, 'd')) => (&s[..i], 86_400),
            _ => (s, 1),
        };

        let n: u64 = num.trim().parse().map_err(|_| format!("`{s}` is not a duration (use 30s, 5m, 2h or 1d)"))?;

        n.checked_mul(mult).map(Self).ok_or_else(|| format!("`{s}` is too long"))
    }
}

impl<'de> Deserialize<'de> for Secs {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        #[derive(Deserialize)]
        #[serde(untagged)]
        enum Raw {
            N(u64),
            S(String),
        }
        match Raw::deserialize(d)? {
            Raw::N(n) => Ok(Self(n)),
            Raw::S(s) => Self::parse(&s).map_err(serde::de::Error::custom),
        }
    }
}

// ---- the document -----------------------------------------------------------------

/// `forge.deploy.yaml`: one release of one service in one environment.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct DeploymentDoc {
    /// `forge/v1`.
    pub api_version: String,
    /// `Deployment`.
    pub kind: String,
    pub metadata: Metadata,
    pub spec: DeploySpec,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Metadata {
    /// The service's name (created on first apply).
    pub name: String,
    /// The environment's slug (`prod`, `staging`, …).
    pub environment: String,
    /// Free text shown with the release.
    #[serde(default)]
    pub description: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct DeploySpec {
    pub runtime: Runtime,
    pub artifact: ArtifactSpec,
    /// Runtime fields of the service (`command`, `ports`, `volumes`,
    /// `listenPort`, `cpuMillis`, …); camelCase or snake_case.
    #[serde(default)]
    pub template: serde_json::Map<String, Value>,
    /// Plain environment variables. Secret values are never written here:
    /// list their names under `secrets` and set them on the service.
    #[serde(default)]
    pub env: BTreeMap<String, String>,
    /// Names of secret variables the service must already have.
    #[serde(default)]
    pub secrets: Vec<String>,
    #[serde(default)]
    pub placement: Option<PlacementSpec>,
    #[serde(default)]
    pub strategy: StrategySpec,
    #[serde(default)]
    pub health: HealthSpec,
    #[serde(default)]
    pub traffic: TrafficSpec,
    #[serde(default)]
    pub rollback: RollbackSpec,
    /// Per-environment changes, merged over `spec` (JSON merge patch) when
    /// the document is applied to that environment.
    #[serde(default)]
    pub overrides: BTreeMap<String, Value>,
}

/// What runs. Exactly one of `image`, `git` and `archive`.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ArtifactSpec {
    /// Container image: `registry/repo:tag`, `registry/repo@sha256:…`.
    #[serde(default)]
    pub image: Option<String>,
    /// `sha256:<64 hex>`: pins `image`.
    #[serde(default)]
    pub digest: Option<String>,
    #[serde(default)]
    pub git: Option<GitSpec>,
    #[serde(default)]
    pub archive: Option<ArchiveSpec>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct GitSpec {
    /// `https://` clone URL.
    pub repo: String,
    /// Full 40-character commit SHA.
    pub commit: String,
    /// Folder inside the repository to run from.
    #[serde(default)]
    pub path: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ArchiveSpec {
    /// `https://` URL of a `.tar.gz`, `.tgz` or `.zip`.
    pub url: String,
    /// Lower-case hex SHA-256 of the file.
    pub sha256: String,
    /// Leading path components to drop when unpacking (GitHub source archives: 1).
    #[serde(default)]
    pub strip_components: u32,
}

#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PlacementSpec {
    /// Node names or ids.
    #[serde(default)]
    pub nodes: Vec<String>,
    /// Node tags (`key: value`) a node must all have.
    #[serde(default)]
    pub node_selector: BTreeMap<String, String>,
    /// How many of the matching nodes (default: all).
    #[serde(default)]
    pub replicas: Option<u32>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Strategy {
    /// Stop every copy, then start the new release everywhere: downtime.
    Recreate,
    /// Node by node (or batch by batch).
    #[default]
    Rolling,
    /// New release on every node next to the old, then all traffic at once.
    BlueGreen,
    /// New release next to the old, traffic shifted in weighted steps.
    Canary,
    /// Rolling, but each batch waits for an approval.
    Manual,
}

impl Strategy {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Recreate => "recreate",
            Self::Rolling => "rolling",
            Self::BlueGreen => "blue-green",
            Self::Canary => "canary",
            Self::Manual => "manual",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Replacement {
    /// Start the new copy next to the old, then switch and stop the old.
    #[default]
    StartFirst,
    /// Take the node out of traffic and stop the old copy first.
    StopFirst,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct StrategySpec {
    #[serde(default, rename = "type")]
    pub kind: Strategy,
    #[serde(default)]
    pub replacement: Replacement,
    /// Nodes updated at once (start-first).
    #[serde(default = "one")]
    pub max_surge: u32,
    /// Nodes out of traffic at once (stop-first).
    #[serde(default)]
    pub max_unavailable: u32,
    /// Overrides both: nodes per batch.
    #[serde(default)]
    pub batch_size: Option<u32>,
    /// Canary: traffic weights in order, each with a pause and gates.
    #[serde(default)]
    pub steps: Vec<CanaryStep>,
    #[serde(default)]
    pub approval: ApprovalSpec,
    /// Accept that a service with one node is down while it is replaced
    /// (stop-first on one node).
    #[serde(default)]
    pub allow_downtime: bool,
}

impl Default for StrategySpec {
    fn default() -> Self {
        Self {
            kind: Strategy::Rolling,
            replacement: Replacement::StartFirst,
            max_surge: 1,
            max_unavailable: 0,
            batch_size: None,
            steps: Vec::new(),
            approval: ApprovalSpec::default(),
            allow_downtime: false,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CanaryStep {
    /// Percent of traffic on the new release (1–100).
    pub weight: u32,
    /// How long to hold this weight before the next step.
    #[serde(default)]
    pub pause: Secs,
    /// Hold until someone approves.
    #[serde(default)]
    pub approval: bool,
    /// Fail the step (and roll back) when the new release misbehaves.
    #[serde(default)]
    pub analysis: Option<Analysis>,
}

/// Gates on the new release's traffic during a canary step, from the edge
/// nodes' traffic log.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Analysis {
    /// Most 5xx answers, in percent of requests (`1` = 1 %).
    #[serde(default)]
    pub max_error_rate: Option<u32>,
    /// Slowest acceptable 95th percentile of upstream time, ms.
    #[serde(default)]
    pub max_p95_ms: Option<u32>,
    /// Fewer requests than this: the gate waits (up to the pause, then passes).
    #[serde(default = "twenty")]
    pub min_requests: u32,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ApprovalSpec {
    /// Wait for an approval before traffic first moves to the new release.
    #[serde(default)]
    pub required: bool,
    /// Fail the deployment when nobody approved in time.
    #[serde(default = "day")]
    pub timeout: Secs,
}

impl Default for ApprovalSpec {
    fn default() -> Self {
        Self { required: false, timeout: day() }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct HealthSpec {
    /// Longest wait for a new copy to become ready.
    #[serde(default = "five_min")]
    pub startup_timeout: Secs,
    /// It must then stay ready this long before it gets traffic.
    #[serde(default = "half_min")]
    pub stabilization_period: Secs,
    /// The service's checks (`readiness`, `liveness`, `startup`,
    /// `defaultChecks`); leave out to keep the service's own.
    #[serde(default)]
    pub checks: Option<Value>,
}

impl Default for HealthSpec {
    fn default() -> Self {
        Self { startup_timeout: five_min(), stabilization_period: half_min(), checks: None }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct TrafficSpec {
    /// How long a copy taken out of traffic keeps running for requests in flight.
    #[serde(default = "half_min")]
    pub drain_timeout: Secs,
}

impl Default for TrafficSpec {
    fn default() -> Self {
        Self { drain_timeout: half_min() }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RollbackSpec {
    /// Go back to the previous release when the rollout fails.
    #[serde(default = "yes")]
    pub automatic: bool,
    /// Releases kept (older ones are deleted; never the current one).
    #[serde(default = "twenty")]
    pub retain_releases: u32,
}

impl Default for RollbackSpec {
    fn default() -> Self {
        Self { automatic: true, retain_releases: twenty() }
    }
}

const fn one() -> u32 {
    1
}
const fn twenty() -> u32 {
    20
}
const fn yes() -> bool {
    true
}
const fn day() -> Secs {
    Secs(86_400)
}
const fn five_min() -> Secs {
    Secs(300)
}
const fn half_min() -> Secs {
    Secs(30)
}

/// A problem with the document, at a dotted path.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DocError {
    pub path: String,
    pub message: String,
}

impl DocError {
    /// A problem at a dotted `path` (empty for the whole document).
    pub fn new(path: &str, message: impl Into<String>) -> Self {
        Self { path: path.to_owned(), message: message.into() }
    }
}

impl std::fmt::Display for DocError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        if self.path.is_empty() { f.write_str(&self.message) } else { write!(f, "{}: {}", self.path, self.message) }
    }
}

/// `camelCase` → `snake_case` (template and health keys follow the YAML's
/// style; the service's own fields are snake_case).
#[must_use]
pub fn snake(k: &str) -> String {
    let mut out = String::with_capacity(k.len() + 4);

    for c in k.chars() {
        if c.is_ascii_uppercase() {
            out.push('_');
            out.push(c.to_ascii_lowercase());
        } else {
            out.push(c);
        }
    }

    out
}

/// Converts every key in `v` to snake_case, recursively.
#[must_use]
pub fn snake_keys(v: Value) -> Value {
    match v {
        Value::Object(m) => Value::Object(m.into_iter().map(|(k, v)| (snake(&k), snake_keys(v))).collect()),
        Value::Array(a) => Value::Array(a.into_iter().map(snake_keys).collect()),
        other => other,
    }
}

/// RFC 7396 JSON merge patch.
pub fn merge_patch(target: &mut Value, patch: &Value) {
    let Value::Object(p) = patch else {
        *target = patch.clone();
        return;
    };

    if !target.is_object() {
        *target = Value::Object(serde_json::Map::new());
    }

    if let Value::Object(t) = target {
        for (k, v) in p {
            if v.is_null() {
                t.remove(k);
            } else {
                merge_patch(t.entry(k.clone()).or_insert(Value::Null), v);
            }
        }
    }
}

/// Parses a document from its JSON form (the server reads the YAML), with
/// `overrides.<environment>` merged in. `environment` replaces
/// `metadata.environment` when given (`forge deploy apply --environment`).
pub fn parse(mut doc: Value, environment: Option<&str>) -> Result<DeploymentDoc, DocError> {
    if let Some(env) = environment {
        doc["metadata"]["environment"] = Value::String(env.to_owned());
    }

    let env = doc.pointer("/metadata/environment").and_then(Value::as_str).unwrap_or_default().to_owned();
    if let Some(patch) = doc.pointer(&format!("/spec/overrides/{env}")).cloned()
        && let Some(spec) = doc.get_mut("spec")
    {
        merge_patch(spec, &patch);
    }

    let d: DeploymentDoc = serde_json::from_value(doc).map_err(|e| DocError::new("", e.to_string()))?;
    d.check()?;

    Ok(d)
}

fn hex(s: &str, len: usize) -> bool {
    s.len() == len && s.bytes().all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

fn https_url(s: &str) -> bool {
    s.starts_with("https://") && s.len() <= 2048 && !s.bytes().any(|b| b.is_ascii_whitespace() || b.is_ascii_control())
}

impl DeploymentDoc {
    fn check(&self) -> Result<(), DocError> {
        if self.api_version != API_VERSION {
            return Err(DocError::new("apiVersion", format!("must be {API_VERSION}")));
        }

        if self.kind != KIND {
            return Err(DocError::new("kind", format!("must be {KIND}")));
        }

        if !crate::apps::valid_slug(&self.metadata.name, 40) {
            return Err(DocError::new("metadata.name", "lowercase letters, digits and dashes, at most 40"));
        }

        if !crate::apps::valid_slug(&self.metadata.environment, 40) {
            return Err(DocError::new("metadata.environment", "the environment's slug, e.g. prod"));
        }

        if self.metadata.description.len() > 500 || self.metadata.description.contains(['\0', '\r']) {
            return Err(DocError::new("metadata.description", "at most 500 characters"));
        }

        self.spec.artifact.resolve(self.spec.runtime)?;

        for k in self.spec.env.keys() {
            if !crate::apps::valid_env_key(k) {
                return Err(DocError::new(&format!("spec.env.{k}"), "invalid variable name"));
            }
        }

        for k in &self.spec.secrets {
            if !crate::apps::valid_env_key(k) {
                return Err(DocError::new("spec.secrets", format!("invalid variable name `{k}`")));
            }

            if self.spec.env.contains_key(k) {
                return Err(DocError::new("spec.secrets", format!("`{k}` is also a plain variable")));
            }
        }

        for k in self.spec.template.keys() {
            if matches!(snake(k).as_str(), "env" | "environment" | "name" | "runtime" | "image" | "slot" | "health") {
                return Err(DocError::new(
                    &format!("spec.template.{k}"),
                    "set this with spec.env, metadata, spec.runtime, spec.artifact or spec.health",
                ));
            }
        }

        if let Some(p) = &self.spec.placement {
            if p.nodes.len() > MAX_NODES || p.replicas.is_some_and(|r| r == 0 || r as usize > MAX_NODES) {
                return Err(DocError::new("spec.placement", format!("between 1 and {MAX_NODES} nodes")));
            }

            if p.nodes.is_empty() && p.node_selector.is_empty() {
                return Err(DocError::new("spec.placement", "list nodes or a nodeSelector"));
            }
        }

        self.policy()?;

        Ok(())
    }

    /// The rollout policy the release carries.
    pub fn policy(&self) -> Result<RolloutPolicy, DocError> {
        let s = &self.spec.strategy;
        if s.kind == Strategy::Canary {
            if s.steps.is_empty() || s.steps.len() > MAX_STEPS {
                return Err(DocError::new("spec.strategy.steps", format!("a canary needs 1 to {MAX_STEPS} steps")));
            }

            let mut last = 0;

            for (i, st) in s.steps.iter().enumerate() {
                if st.weight == 0 || st.weight > 100 || st.weight <= last {
                    return Err(DocError::new(
                        &format!("spec.strategy.steps[{i}].weight"),
                        "weights rise from 1 to 100",
                    ));
                }

                last = st.weight;
            }
        } else if !s.steps.is_empty() {
            return Err(DocError::new("spec.strategy.steps", "only a canary has steps"));
        }

        if s.batch_size == Some(0) {
            return Err(DocError::new("spec.strategy.batchSize", "at least 1"));
        }

        let h = &self.spec.health;
        if h.startup_timeout.0 == 0 || h.startup_timeout.0 > 3 * 3_600 {
            return Err(DocError::new("spec.health.startupTimeout", "between 1s and 3h"));
        }

        if h.stabilization_period.0 > 3_600 {
            return Err(DocError::new("spec.health.stabilizationPeriod", "at most 1h"));
        }

        if self.spec.traffic.drain_timeout.0 > 3_600 {
            return Err(DocError::new("spec.traffic.drainTimeout", "at most 1h"));
        }

        if s.approval.timeout.0 == 0 || s.approval.timeout.0 > 30 * 86_400 {
            return Err(DocError::new("spec.strategy.approval.timeout", "between 1s and 30d"));
        }

        if self.spec.rollback.retain_releases == 0 || self.spec.rollback.retain_releases > 500 {
            return Err(DocError::new("spec.rollback.retainReleases", "between 1 and 500"));
        }

        let batch = s.batch_size.unwrap_or(match s.replacement {
            Replacement::StartFirst => s.max_surge.max(1),
            Replacement::StopFirst => s.max_unavailable.max(1),
        });

        Ok(RolloutPolicy {
            strategy: s.kind,
            replacement: s.replacement,
            batch_size: batch,
            steps: s.steps.clone(),
            approval: s.approval.required,
            approval_timeout_secs: s.approval.timeout.0,
            allow_downtime: s.allow_downtime,
            startup_timeout_secs: h.startup_timeout.0,
            stabilization_secs: h.stabilization_period.0,
            drain_secs: self.spec.traffic.drain_timeout.0,
            auto_rollback: self.spec.rollback.automatic,
            retain_releases: self.spec.rollback.retain_releases,
        })
    }
}

impl ArtifactSpec {
    /// The artifact as the release records it.
    pub fn resolve(&self, runtime: Runtime) -> Result<ArtifactRef, DocError> {
        let count =
            usize::from(self.image.is_some()) + usize::from(self.git.is_some()) + usize::from(self.archive.is_some());

        if count != 1 {
            return Err(DocError::new("spec.artifact", "exactly one of image, git and archive"));
        }

        if let Some(image) = &self.image {
            if runtime != Runtime::Container {
                return Err(DocError::new("spec.artifact.image", "an image needs runtime: container"));
            }

            let (name, inline_digest) = match image.split_once('@') {
                Some((n, d)) => (n.to_owned(), Some(d.to_owned())),
                None => (image.clone(), None),
            };

            let digest = match (inline_digest, &self.digest) {
                (Some(a), Some(b)) if a != *b => {
                    return Err(DocError::new("spec.artifact.digest", "differs from the digest in image"));
                }
                (Some(a), _) => Some(a),
                (None, b) => b.clone(),
            };

            if let Some(d) = &digest
                && !d.strip_prefix("sha256:").is_some_and(|h| hex(h, 64))
            {
                return Err(DocError::new("spec.artifact.digest", "sha256: and 64 lower-case hex digits"));
            }

            let probe = format!("{name}{}", digest.as_ref().map(|d| format!("@{d}")).unwrap_or_default());
            if !crate::apps::valid_image_ref(&probe) {
                return Err(DocError::new("spec.artifact.image", "not an image reference"));
            }

            return Ok(ArtifactRef::Image { image: name, digest });
        }

        if runtime == Runtime::Container {
            return Err(DocError::new("spec.artifact", "a container runs an image"));
        }

        if let Some(g) = &self.git {
            if !https_url(&g.repo) {
                return Err(DocError::new("spec.artifact.git.repo", "an https:// URL"));
            }

            if !hex(&g.commit, 40) {
                return Err(DocError::new("spec.artifact.git.commit", "the full 40-character commit SHA, lower case"));
            }

            if !valid_subpath(&g.path) {
                return Err(DocError::new("spec.artifact.git.path", "a relative folder without .."));
            }

            return Ok(ArtifactRef::Git { repo: g.repo.clone(), commit: g.commit.clone(), path: g.path.clone() });
        }

        let Some(a) = &self.archive else { return Err(DocError::new("spec.artifact", "missing")) };
        if !https_url(&a.url) {
            return Err(DocError::new("spec.artifact.archive.url", "an https:// URL"));
        }

        if !hex(&a.sha256, 64) {
            return Err(DocError::new("spec.artifact.archive.sha256", "64 lower-case hex digits"));
        }

        if a.strip_components > 8 {
            return Err(DocError::new("spec.artifact.archive.stripComponents", "at most 8"));
        }

        if ArchiveKind::of(&a.url).is_none() {
            return Err(DocError::new("spec.artifact.archive.url", "a .tar.gz, .tgz, .tar or .zip file"));
        }

        Ok(ArtifactRef::Archive { url: a.url.clone(), sha256: a.sha256.clone(), strip_components: a.strip_components })
    }
}

/// A relative path without `.` or `..` components (empty is allowed).
#[must_use]
pub fn valid_subpath(p: &str) -> bool {
    p.is_empty()
        || (p.len() <= 256
            && !p.starts_with('/')
            && !p.split('/').any(|c| c == ".." || c == ".")
            && p.bytes().all(|b| b.is_ascii_alphanumeric() || matches!(b, b'/' | b'_' | b'-' | b'.')))
}

/// What a release runs, as recorded.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum ArtifactRef {
    /// `image` without a digest: the tag is resolved to one when the first
    /// node pulls it, and every other node then pulls that digest.
    Image {
        image: String,
        digest: Option<String>,
    },
    Git {
        repo: String,
        commit: String,
        path: String,
    },
    Archive {
        url: String,
        sha256: String,
        strip_components: u32,
    },
}

impl ArtifactRef {
    /// The image reference a node pulls: by digest when known.
    #[must_use]
    pub fn image_ref(&self) -> Option<String> {
        match self {
            Self::Image { image, digest: Some(d) } => {
                // `repo:tag@sha256:…` pulls by digest; the tag is kept for people.
                Some(format!("{image}@{d}"))
            }
            Self::Image { image, digest: None } => Some(image.clone()),
            _ => None,
        }
    }

    /// One line for lists: `ghcr.io/acme/web@sha256:abc123…`, `git abc1234`, …
    #[must_use]
    pub fn label(&self) -> String {
        match self {
            Self::Image { image, digest: Some(d) } => format!("{image}@{}", &d[..d.len().min(19)]),
            Self::Image { image, digest: None } => image.clone(),
            Self::Git { repo, commit, .. } => format!("{repo}#{}", &commit[..commit.len().min(12)]),
            Self::Archive { url, sha256, .. } => format!("{url} ({})", &sha256[..sha256.len().min(12)]),
        }
    }

    /// Native artifacts are unpacked into a release folder on each node.
    #[must_use]
    pub const fn staged(&self) -> bool {
        matches!(self, Self::Git { .. } | Self::Archive { .. })
    }

    #[must_use]
    pub const fn kind(&self) -> &'static str {
        match self {
            Self::Image { .. } => "image",
            Self::Git { .. } => "git",
            Self::Archive { .. } => "archive",
        }
    }

    /// Checks what a node is asked to fetch (the node checks again).
    pub fn validate(&self) -> Result<(), String> {
        match self {
            Self::Image { image, digest } => {
                if !crate::apps::valid_image_ref(image) {
                    return Err("not an image reference".into());
                }

                if digest.as_deref().is_some_and(|d| !d.strip_prefix("sha256:").is_some_and(|h| hex(h, 64))) {
                    return Err("the digest must be sha256: and 64 hex digits".into());
                }
            }
            Self::Git { repo, commit, path } => {
                if !https_url(repo) || repo.contains(['\'', '"', ' ']) {
                    return Err("the repository must be an https:// URL".into());
                }

                if !hex(commit, 40) {
                    return Err("the commit must be a full 40-character SHA".into());
                }

                if !valid_subpath(path) {
                    return Err("the path must be a relative folder without ..".into());
                }
            }
            Self::Archive { url, sha256, strip_components } => {
                if !https_url(url) {
                    return Err("the archive must be an https:// URL".into());
                }

                if !hex(sha256, 64) {
                    return Err("sha256 must be 64 lower-case hex digits".into());
                }

                if *strip_components > 8 {
                    return Err("strip at most 8 leading folders".into());
                }

                if ArchiveKind::of(url).is_none() {
                    return Err("the archive must be a .tar.gz, .tgz, .tar or .zip".into());
                }
            }
        }

        Ok(())
    }
}

/// How an archive is unpacked, from its URL's path.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ArchiveKind {
    TarGz,
    Tar,
    Zip,
}

impl ArchiveKind {
    #[must_use]
    pub fn of(url: &str) -> Option<Self> {
        let path = url.split(['?', '#']).next().unwrap_or(url).to_ascii_lowercase();
        if path.ends_with(".tar.gz") || path.ends_with(".tgz") {
            Some(Self::TarGz)
        } else if path.ends_with(".tar") {
            Some(Self::Tar)
        } else if path.ends_with(".zip") {
            Some(Self::Zip)
        } else {
            None
        }
    }
}

/// The resolved rollout policy a release carries.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RolloutPolicy {
    pub strategy: Strategy,
    pub replacement: Replacement,
    /// Nodes per batch.
    pub batch_size: u32,
    #[serde(default)]
    pub steps: Vec<CanaryStep>,
    /// Approval before traffic first moves.
    #[serde(default)]
    pub approval: bool,
    pub approval_timeout_secs: u64,
    #[serde(default)]
    pub allow_downtime: bool,
    pub startup_timeout_secs: u64,
    pub stabilization_secs: u64,
    pub drain_secs: u64,
    pub auto_rollback: bool,
    pub retain_releases: u32,
}

impl Default for RolloutPolicy {
    fn default() -> Self {
        Self {
            strategy: Strategy::Rolling,
            replacement: Replacement::StopFirst,
            batch_size: 1,
            steps: Vec::new(),
            approval: false,
            approval_timeout_secs: 86_400,
            allow_downtime: true,
            startup_timeout_secs: 300,
            stabilization_secs: 10,
            drain_secs: 10,
            auto_rollback: false,
            retain_releases: 20,
        }
    }
}

impl RolloutPolicy {
    /// Two copies run side by side on a node during the rollout.
    #[must_use]
    pub fn slotted(&self) -> bool {
        match self.strategy {
            Strategy::Recreate => false,
            Strategy::Rolling | Strategy::Manual => self.replacement == Replacement::StartFirst,
            Strategy::BlueGreen | Strategy::Canary => true,
        }
    }
}

// ---- slots ------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Slot {
    A,
    B,
}

impl Slot {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::A => "a",
            Self::B => "b",
        }
    }

    #[must_use]
    pub const fn other(self) -> Self {
        match self {
            Self::A => Self::B,
            Self::B => Self::A,
        }
    }

    #[must_use]
    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "a" => Some(Self::A),
            "b" => Some(Self::B),
            _ => None,
        }
    }

    /// `""` for the unslotted instance.
    #[must_use]
    pub fn name(slot: Option<Self>) -> &'static str {
        slot.map_or("", Self::as_str)
    }
}

/// The host ports of a service's published ports, in the order of `ports`.
#[must_use]
pub fn published_host_ports(ports: &[String]) -> Vec<u16> {
    ports
        .iter()
        .filter_map(|p| {
            let body = p.split('/').next().unwrap_or(p);
            let parts: Vec<&str> = body.split(':').collect();
            match parts.as_slice() {
                [h, _] | [_, h, _] => h.parse().ok(),
                _ => None,
            }
        })
        .collect()
}

/// Rewrites `spec` to run in `slot` on `slot_ports` (one per published port,
/// or one for a native service's `listen_port`). Returns service port →
/// slot port, which the ingress uses to reach the slot. Probes of the
/// service's own ports are moved with it.
pub fn into_slot(spec: &mut crate::AppSpec, slot: Slot, slot_ports: &[u16]) -> Result<BTreeMap<u16, u16>, String> {
    let mut map = BTreeMap::new();
    spec.slot = slot.as_str().to_owned();

    match spec.runtime {
        Runtime::Container => {
            if slot_ports.len() < spec.ports.len() {
                return Err("not enough slot ports".into());
            }

            let mut out = Vec::with_capacity(spec.ports.len());

            for (p, slot_port) in spec.ports.iter().zip(slot_ports) {
                let (body, proto) = p.split_once('/').map_or((p.as_str(), None), |(b, pr)| (b, Some(pr)));
                let parts: Vec<&str> = body.split(':').collect();
                let (ip, host, ctr) = match parts.as_slice() {
                    [h, c] => (None, *h, *c),
                    [ip, h, c] => (Some(*ip), *h, *c),
                    _ => return Err(format!("invalid port mapping `{p}`")),
                };

                let host: u16 = host.parse().map_err(|_| format!("invalid port mapping `{p}`"))?;
                map.insert(host, *slot_port);

                let mut m = match ip {
                    Some(ip) => format!("{ip}:{slot_port}:{ctr}"),
                    None => format!("{slot_port}:{ctr}"),
                };

                if let Some(pr) = proto {
                    m.push('/');
                    m.push_str(pr);
                }

                out.push(m);
            }

            spec.ports = out;
        }
        Runtime::Systemd => {
            if let Some(port) = spec.listen_port {
                let slot_port = *slot_ports.first().ok_or("no slot port")?;
                map.insert(port, slot_port);
                spec.listen_port = Some(slot_port);
                spec.env.retain(|(k, _)| k != "PORT");
                spec.env.push(("PORT".into(), slot_port.to_string()));
            }
        }
    }

    if let Some(h) = spec.health.as_mut() {
        retarget(h, &map);
    }

    Ok(map)
}

/// Moves HTTP and TCP probes of the workload from service ports to slot ports.
pub fn retarget(policy: &mut crate::health::HealthPolicy, map: &BTreeMap<u16, u16>) {
    use crate::health::Probe;

    for c in policy.startup.iter_mut().chain(policy.readiness.iter_mut()).chain(policy.liveness.iter_mut()) {
        match &mut c.probe {
            Probe::Http(h) if crate::health::workload_host(&h.host) => {
                if let Some(p) = map.get(&h.port) {
                    h.port = *p;
                }
            }
            Probe::Tcp(t) if crate::health::workload_host(&t.host) => {
                if let Some(p) = map.get(&t.port) {
                    t.port = *p;
                }
            }
            _ => {}
        }
    }
}

// ---- the planner ------------------------------------------------------------------

/// One node of the service as the planner sees it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PlanNode {
    pub server_id: String,
    pub name: String,
    pub online: bool,
    /// The copy serving now: `Some(Some(slot))`, `Some(None)` for the
    /// unslotted instance, `None` when nothing runs there yet.
    pub serving: Option<Option<Slot>>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PlanInput {
    pub nodes: Vec<PlanNode>,
    pub policy: RolloutPolicy,
    pub runtime: Runtime,
    /// A Forge endpoint routes traffic to the service.
    pub routed: bool,
    /// The service publishes host ports (container) or declares a listen
    /// port (native service).
    pub has_ports: bool,
}

/// A copy of the service on one node.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Target {
    pub server_id: String,
    /// `None`: the unslotted instance.
    pub slot: Option<Slot>,
}

/// Traffic weight of one copy (0 = out of traffic).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Weight {
    pub server_id: String,
    pub slot: Option<Slot>,
    pub weight: u32,
}

/// One step of a rollout. A step with several targets runs on them at once.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case")]
pub enum Step {
    /// Deploy the release into these copies (replacing what runs there).
    Start { targets: Vec<Target> },
    /// Wait until the copies are ready, then stay ready for the stabilization period.
    Gate { targets: Vec<Target> },
    /// Set traffic weights and re-apply the service's endpoints.
    Route { weights: Vec<Weight> },
    /// Hold (drain or a canary pause), then check the gates if any.
    Wait {
        secs: u64,
        why: String,
        #[serde(default)]
        analysis: Option<Analysis>,
    },
    /// Wait for an approval.
    Approve { reason: String },
    /// Stop and remove these copies.
    Stop { targets: Vec<Target> },
}

impl Step {
    /// One line for people.
    #[must_use]
    pub fn describe(&self, names: &BTreeMap<String, String>) -> String {
        let name = |id: &str| names.get(id).cloned().unwrap_or_else(|| id.to_owned());
        let copies = |t: &[Target]| {
            t.iter()
                .map(|t| match t.slot {
                    Some(s) => format!("{} (slot {})", name(&t.server_id), s.as_str()),
                    None => name(&t.server_id),
                })
                .collect::<Vec<_>>()
                .join(", ")
        };

        match self {
            Self::Start { targets } => format!("start the release on {}", copies(targets)),
            Self::Gate { targets } => format!("wait until {} is ready and stable", copies(targets)),
            Self::Route { weights } => {
                let w: Vec<String> = weights
                    .iter()
                    .map(|w| {
                        let slot = w.slot.map(|s| format!(" slot {}", s.as_str())).unwrap_or_default();
                        format!("{}{slot} {}%", name(&w.server_id), w.weight)
                    })
                    .collect();
                format!("route traffic: {}", w.join(", "))
            }
            Self::Wait { secs, why, .. } => format!("wait {secs}s ({why})"),
            Self::Approve { reason } => format!("wait for approval ({reason})"),
            Self::Stop { targets } => format!("stop {}", copies(targets)),
        }
    }

    /// Copies this step deploys.
    #[must_use]
    pub fn started(&self) -> &[Target] {
        match self {
            Self::Start { targets } => targets,
            _ => &[],
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Plan {
    pub strategy: Strategy,
    /// The release runs in the other slot while the old one serves.
    pub slotted: bool,
    pub steps: Vec<Step>,
    /// Requests fail while the plan runs.
    pub downtime: bool,
    /// What the plan does and why, for people.
    pub notes: Vec<String>,
    /// Where each node's release ends up.
    pub targets: Vec<Target>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PlanError(pub String);

impl std::fmt::Display for PlanError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

/// Plans a rollout. Refuses what cannot be done safely: offline nodes,
/// side-by-side copies nobody can reach, and downtime nobody accepted.
pub fn plan(input: &PlanInput) -> Result<Plan, PlanError> {
    let p = &input.policy;

    if input.nodes.is_empty() {
        return Err(PlanError("the service is not placed on any node".into()));
    }

    if input.nodes.len() > MAX_NODES {
        return Err(PlanError(format!("at most {MAX_NODES} nodes per deployment")));
    }

    let offline: Vec<&str> = input.nodes.iter().filter(|n| !n.online).map(|n| n.name.as_str()).collect();
    if !offline.is_empty() {
        return Err(PlanError(format!(
            "{} offline; bring {} back or change the placement",
            offline.join(", "),
            if offline.len() == 1 { "it" } else { "them" }
        )));
    }

    let mut notes = Vec::new();
    let slotted = p.slotted();
    if slotted && !input.routed {
        return Err(PlanError(if input.has_ports {
            format!(
                "{} starts the new release next to the old one on other host ports, which only a Forge endpoint \
                 routes to; no endpoint targets this service. Add one, or use replacement: stop-first or recreate",
                p.strategy.as_str()
            )
        } else {
            format!(
                "{} switches traffic between two copies, but the service has no ports and no endpoint; use recreate \
                 or rolling with replacement: stop-first",
                p.strategy.as_str()
            )
        }));
    }

    if slotted && input.runtime == Runtime::Systemd && input.has_ports {
        notes.push(
            "each copy gets its own port in $PORT (and listen_port): the service must listen on $PORT".to_owned(),
        );
    }

    let new_slot = |n: &PlanNode| -> Option<Slot> {
        if slotted {
            Some(match n.serving {
                Some(Some(s)) => s.other(),
                _ => Slot::A,
            })
        } else {
            n.serving.flatten()
        }
    };
    let targets: Vec<Target> =
        input.nodes.iter().map(|n| Target { server_id: n.server_id.clone(), slot: new_slot(n) }).collect();

    // Copies that serve now and are replaced by another slot.
    let old_of = |n: &PlanNode| -> Option<Target> {
        match n.serving {
            Some(s) if s != new_slot(n) => Some(Target { server_id: n.server_id.clone(), slot: s }),
            _ => None,
        }
    };
    let weights = |nodes: &[&PlanNode], new: u32| -> Vec<Weight> {
        let mut w = Vec::new();

        for n in nodes {
            w.push(Weight { server_id: n.server_id.clone(), slot: new_slot(n), weight: new });

            if let Some(o) = old_of(n) {
                w.push(Weight { server_id: o.server_id, slot: o.slot, weight: 100 - new });
            }
        }

        w
    };
    let target_of = |n: &PlanNode| Target { server_id: n.server_id.clone(), slot: new_slot(n) };
    let all: Vec<&PlanNode> = input.nodes.iter().collect();
    let batch = (p.batch_size.max(1) as usize).min(all.len());
    let mut steps = Vec::new();
    let drain = |why: &str| Step::Wait { secs: p.drain_secs, why: why.into(), analysis: None };
    let approve_first = |steps: &mut Vec<Step>| {
        if p.approval {
            steps.push(Step::Approve { reason: "before the new release takes traffic".into() });
        }
    };
    let mut downtime = false;

    match (p.strategy, slotted) {
        (Strategy::Recreate, _) => {
            downtime = all.iter().any(|n| n.serving.is_some());
            approve_first(&mut steps);
            let ts: Vec<Target> = all.iter().map(|n| target_of(n)).collect();
            steps.push(Step::Start { targets: ts.clone() });
            steps.push(Step::Gate { targets: ts });

            if input.routed {
                steps.push(Step::Route { weights: weights(&all, 100) });
            }

            notes.push("recreate replaces every copy at once: requests fail until the new release is up".into());
        }
        (Strategy::Rolling | Strategy::Manual, false) => {
            let deployed = all.iter().filter(|n| n.serving.is_some()).count();
            if deployed > 0 && batch >= deployed {
                if !p.allow_downtime {
                    return Err(PlanError(format!(
                        "stop-first with {} would take every running copy down at once; add a node, lower \
                         batchSize, use start-first behind an endpoint, or set strategy.allowDowntime: true",
                        if all.len() == 1 { "one node".to_owned() } else { format!("batches of {batch}") }
                    )));
                }

                downtime = true;
                notes.push("every running copy is replaced at once: requests fail meanwhile (allowDowntime)".into());
            }

            approve_first(&mut steps);

            for (i, chunk) in all.chunks(batch).enumerate() {
                if p.strategy == Strategy::Manual && !(i == 0 && p.approval) {
                    steps.push(Step::Approve { reason: format!("batch {}", i + 1) });
                }

                let ts: Vec<Target> = chunk.iter().map(|n| target_of(n)).collect();
                let running: Vec<&PlanNode> = chunk.iter().copied().filter(|n| n.serving.is_some()).collect();
                if input.routed && !running.is_empty() && all.len() > chunk.len() {
                    steps.push(Step::Route { weights: weights(&running, 0) });
                    steps.push(drain("in-flight requests finish"));
                }

                steps.push(Step::Start { targets: ts.clone() });
                steps.push(Step::Gate { targets: ts });

                if input.routed {
                    steps.push(Step::Route { weights: weights(chunk, 100) });
                }
            }
        }
        (Strategy::Rolling | Strategy::Manual, true) => {
            approve_first(&mut steps);

            for (i, chunk) in all.chunks(batch).enumerate() {
                if p.strategy == Strategy::Manual && !(i == 0 && p.approval) {
                    steps.push(Step::Approve { reason: format!("batch {}", i + 1) });
                }

                let ts: Vec<Target> = chunk.iter().map(|n| target_of(n)).collect();
                steps.push(Step::Start { targets: ts.clone() });
                steps.push(Step::Gate { targets: ts });
                steps.push(Step::Route { weights: weights(chunk, 100) });
                let old: Vec<Target> = chunk.iter().filter_map(|n| old_of(n)).collect();
                if !old.is_empty() {
                    steps.push(drain("the old copies finish their requests"));
                    steps.push(Step::Stop { targets: old });
                }
            }
        }
        (Strategy::BlueGreen, _) => {
            let ts: Vec<Target> = all.iter().map(|n| target_of(n)).collect();
            steps.push(Step::Start { targets: ts.clone() });
            steps.push(Step::Gate { targets: ts });
            approve_first(&mut steps);
            steps.push(Step::Route { weights: weights(&all, 100) });
            let old: Vec<Target> = all.iter().filter_map(|n| old_of(n)).collect();
            if !old.is_empty() {
                steps.push(drain("the old release finishes its requests"));
                steps.push(Step::Stop { targets: old });
            }

            notes.push("every node runs both releases until traffic switches at once".into());
        }
        (Strategy::Canary, _) => {
            let ts: Vec<Target> = all.iter().map(|n| target_of(n)).collect();
            steps.push(Step::Start { targets: ts.clone() });
            steps.push(Step::Gate { targets: ts });
            approve_first(&mut steps);
            let first_release = all.iter().all(|n| n.serving.is_none());

            for st in &p.steps {
                if first_release && st.weight < 100 {
                    // Nothing to compare with: the first release takes all traffic.
                    continue;
                }

                steps.push(Step::Route { weights: weights(&all, st.weight) });

                if st.pause.0 > 0 || st.analysis.is_some() {
                    steps.push(Step::Wait {
                        secs: st.pause.0,
                        why: format!("canary at {}%", st.weight),
                        analysis: st.analysis.clone(),
                    });
                }

                if st.approval {
                    steps.push(Step::Approve { reason: format!("canary at {}%", st.weight) });
                }
            }

            if p.steps.last().is_none_or(|s| s.weight < 100) || first_release {
                steps.push(Step::Route { weights: weights(&all, 100) });
            }

            let old: Vec<Target> = all.iter().filter_map(|n| old_of(n)).collect();
            if !old.is_empty() {
                steps.push(drain("the old release finishes its requests"));
                steps.push(Step::Stop { targets: old });
            }
        }
    }

    // Back-to-back routes become one: the later weight of a copy wins.
    steps.dedup_by(|later, earlier| match (earlier, later) {
        (Step::Route { weights: first }, Step::Route { weights: second }) => {
            for w in second.drain(..) {
                match first.iter_mut().find(|x| x.server_id == w.server_id && x.slot == w.slot) {
                    Some(x) => x.weight = w.weight,
                    None => first.push(w),
                }
            }

            true
        }
        _ => false,
    });

    if !input.routed && input.has_ports && !downtime {
        notes.push("no Forge endpoint routes to this service: clients reach each node directly".into());
    }

    Ok(Plan { strategy: p.strategy, slotted, steps, downtime, notes, targets })
}

#[cfg(test)]
mod tests;
