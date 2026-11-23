//! Schema stand-ins for types defined outside the server (D-077).
//!
//! `schemars` lives in forge-server only (forge-core is shared with the
//! agent), so enums from other crates that appear in request bodies are
//! mirrored here and used through `#[schemars(with = "…")]`. The
//! `from_core` functions match every variant, so a new variant in the real
//! enum is a compile error here, and the test checks the wire names agree.
// Schema-only types: `schemars` reads them, the server never builds a value.
#![allow(dead_code)]

use schemars::JsonSchema;

/// What a connected AI agent may do (`read_only`, `ask`, `auto`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, JsonSchema)]
#[schemars(rename_all = "snake_case")]
pub enum AgentMode {
    ReadOnly,
    Ask,
    Auto,
}

impl AgentMode {
    #[must_use]
    pub const fn from_core(m: forge_core::model::AgentMode) -> Self {
        use forge_core::model::AgentMode as M;
        match m {
            M::ReadOnly => Self::ReadOnly,
            M::Ask => Self::Ask,
            M::Auto => Self::Auto,
        }
    }
}

/// What a bucket key or a bucket grant may do (`read`, `readwrite`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, JsonSchema)]
#[schemars(rename_all = "lowercase")]
pub enum BucketAccess {
    Read,
    #[schemars(rename = "readwrite")]
    ReadWrite,
}

impl BucketAccess {
    #[must_use]
    pub const fn from_core(a: forge_core::storage::BucketAccess) -> Self {
        use forge_core::storage::BucketAccess as A;
        match a {
            A::Read => Self::Read,
            A::ReadWrite => Self::ReadWrite,
        }
    }
}

/// Archive format (`zip`, `tar_gz`; `tar` is read only).
#[derive(Debug, Clone, Copy, PartialEq, Eq, JsonSchema)]
#[schemars(rename_all = "snake_case")]
pub enum ArchiveFormat {
    Zip,
    TarGz,
    Tar,
}

impl ArchiveFormat {
    #[must_use]
    pub const fn from_core(f: forge_core::files::ArchiveFormat) -> Self {
        use forge_core::files::ArchiveFormat as F;
        match f {
            F::Zip => Self::Zip,
            F::TarGz => Self::TarGz,
            F::Tar => Self::Tar,
        }
    }
}

/// Job status.
#[derive(Debug, Clone, Copy, PartialEq, Eq, JsonSchema)]
#[schemars(rename_all = "snake_case")]
pub enum JobStatus {
    Queued,
    Running,
    Succeeded,
    Failed,
    Cancelled,
}

impl JobStatus {
    #[must_use]
    pub const fn from_core(s: forge_core::model::JobStatus) -> Self {
        use forge_core::model::JobStatus as S;
        match s {
            S::Queued => Self::Queued,
            S::Running => Self::Running,
            S::Succeeded => Self::Succeeded,
            S::Failed => Self::Failed,
            S::Cancelled => Self::Cancelled,
        }
    }
}

/// Where a file root comes from (`node`, `service`, `container`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, JsonSchema)]
#[schemars(rename_all = "snake_case")]
pub enum RootKind {
    Node,
    Service,
    Container,
}

impl RootKind {
    #[must_use]
    pub const fn from_core(k: forge_core::files::RootKind) -> Self {
        use forge_core::files::RootKind as K;
        match k {
            K::Node => Self::Node,
            K::Service => Self::Service,
            K::Container => Self::Container,
        }
    }
}

/// Request bodies whose Rust type lives in another crate. The scanner maps
/// the body type's name to these (`build_routes.rs`, `FOREIGN_BODIES`); each
/// test below destructures the real type without `..`, so a field added
/// there is a compile error until it is mirrored here.
///
/// Changes to temporary SFTP access (`forge_files::GrantChanges`); absent
/// fields stay, `null` clears a limit.
#[derive(Debug, JsonSchema)]
pub struct GrantChanges {
    pub name: Option<String>,
    /// Replaces the roots.
    pub roots: Option<Vec<String>>,
    pub write: Option<bool>,
    pub delete: Option<bool>,
    /// New lifetime in minutes, counted from now.
    pub expires_in_minutes: Option<u32>,
    /// `true`: never expires (needs `[sftp] allow_never_expiry` and `file.admin`).
    pub never_expires: Option<bool>,
    /// Replaces the allowed client addresses (empty: any).
    pub allowed_ips: Option<Vec<String>>,
    pub max_sessions: Option<u32>,
    pub bandwidth_kbps: Option<u32>,
    pub transfer_limit: Option<u64>,
}

/// Temporary SFTP access to a node's roots (`forge_files::NewGrant`).
#[derive(Debug, JsonSchema)]
pub struct NewGrant {
    pub name: String,
    /// Root ids the grant may use.
    pub roots: Vec<String>,
    #[serde(default)]
    pub write: bool,
    #[serde(default)]
    pub delete: bool,
    /// Lifetime in minutes (at most the configured maximum); ignored with `never_expires`.
    #[serde(default)]
    pub expires_in_minutes: u32,
    /// No expiry (needs `[sftp] allow_never_expiry` and `file.admin`).
    #[serde(default)]
    pub never_expires: bool,
    /// Client addresses or CIDRs allowed to connect (empty: any).
    #[serde(default)]
    pub allowed_ips: Vec<String>,
    /// Concurrent SFTP sessions (default 1).
    #[serde(default)]
    pub max_sessions: Option<u32>,
    #[serde(default)]
    pub bandwidth_kbps: Option<u32>,
    /// Bytes the grant may transfer in total.
    #[serde(default)]
    pub transfer_limit: Option<u64>,
}

/// The browser's answer to `navigator.credentials.create()`, base64url
/// fields as the console sends them (`forge_identity::passkeys::RegistrationResponse`).
#[derive(Debug, JsonSchema)]
pub struct RegistrationResponse {
    pub client_data_json: String,
    pub attestation_object: String,
    /// Name shown in the passkey list.
    #[serde(default)]
    pub name: Option<String>,
}

/// The browser's answer to `navigator.credentials.get()`
/// (`forge_identity::passkeys::AssertionResponse`).
#[derive(Debug, JsonSchema)]
pub struct AssertionResponse {
    pub credential_id: String,
    pub client_data_json: String,
    pub authenticator_data: String,
    pub signature: String,
}

#[cfg(test)]
mod tests {
    use serde_json::Value;

    use super::*;

    /// The wire names in the schema are exactly the real enum's serde names.
    fn names<T: JsonSchema>() -> Vec<String> {
        let s = schemars::schema_for!(T);
        let v = s.as_value();
        let list = v.get("enum").or_else(|| v.get("oneOf")).cloned().unwrap_or(Value::Null);
        let mut out: Vec<String> = match list {
            Value::Array(a) => a
                .iter()
                .filter_map(|x| x.as_str().map(str::to_owned).or_else(|| x["const"].as_str().map(str::to_owned)))
                .collect(),
            _ => Vec::new(),
        };
        out.sort();
        out
    }

    fn wire<T: serde::Serialize>(all: &[T]) -> Vec<String> {
        let mut out: Vec<String> =
            all.iter().filter_map(|x| serde_json::to_value(x).ok()?.as_str().map(str::to_owned)).collect();
        out.sort();
        out
    }

    #[test]
    fn mirrors_use_the_real_wire_names() {
        use forge_core::files::{ArchiveFormat as F, RootKind as K};
        use forge_core::model::{AgentMode as M, JobStatus as S};
        assert_eq!(names::<AgentMode>(), wire(&[M::ReadOnly, M::Ask, M::Auto]));
        assert_eq!(names::<ArchiveFormat>(), wire(&[F::Zip, F::TarGz, F::Tar]));
        assert_eq!(names::<JobStatus>(), wire(&[S::Queued, S::Running, S::Succeeded, S::Failed, S::Cancelled]));
        assert_eq!(names::<RootKind>(), wire(&[K::Node, K::Service, K::Container]));
        assert_eq!(names::<BucketAccess>(), wire(&forge_core::storage::BucketAccess::ALL));
        // Each mirror converts from every real variant (compile-time exhaustive).
        assert_eq!(BucketAccess::from_core(forge_core::storage::BucketAccess::ReadWrite), BucketAccess::ReadWrite);
        assert_eq!(AgentMode::from_core(M::Ask), AgentMode::Ask);
        assert_eq!(ArchiveFormat::from_core(F::TarGz), ArchiveFormat::TarGz);
        assert_eq!(JobStatus::from_core(S::Failed), JobStatus::Failed);
        assert_eq!(RootKind::from_core(K::Container), RootKind::Container);
    }

    /// The mirrors list every field of the real bodies (no `..`: a new field
    /// there fails to compile here), and an example built from the mirror's
    /// fields is accepted by the real type.
    #[test]
    fn body_mirrors_match_the_real_types() {
        let g: forge_files::NewGrant = serde_json::from_value(serde_json::json!({
            "name": "n", "roots": ["r"], "write": true, "delete": false, "expires_in_minutes": 5,
            "allowed_ips": [], "max_sessions": 1, "bandwidth_kbps": null, "transfer_limit": null
        }))
        .unwrap_or_else(|e| panic!("{e}"));
        let forge_files::NewGrant {
            name: _,
            roots: _,
            write: _,
            delete: _,
            expires_in_minutes: _,
            never_expires: _,
            allowed_ips: _,
            max_sessions: _,
            bandwidth_kbps: _,
            transfer_limit: _,
        } = g;
        let c: forge_files::GrantChanges = serde_json::from_value(serde_json::json!({
            "name": "n", "roots": ["r"], "write": true, "delete": false, "expires_in_minutes": 5,
            "never_expires": false, "allowed_ips": [], "max_sessions": 1, "bandwidth_kbps": null,
            "transfer_limit": null
        }))
        .unwrap_or_else(|e| panic!("{e}"));
        let forge_files::GrantChanges {
            name: _,
            roots: _,
            write: _,
            delete: _,
            expires_in_minutes: _,
            never_expires: _,
            allowed_ips: _,
            max_sessions: _,
            bandwidth_kbps: _,
            transfer_limit: _,
        } = c;
        let r: forge_identity::passkeys::RegistrationResponse = serde_json::from_value(serde_json::json!({
            "client_data_json": "a", "attestation_object": "b", "name": "key"
        }))
        .unwrap_or_else(|e| panic!("{e}"));
        let forge_identity::passkeys::RegistrationResponse { client_data_json: _, attestation_object: _, name: _ } = r;
        let a: forge_identity::passkeys::AssertionResponse = serde_json::from_value(serde_json::json!({
            "credential_id": "c", "client_data_json": "a", "authenticator_data": "d", "signature": "s"
        }))
        .unwrap_or_else(|e| panic!("{e}"));
        let forge_identity::passkeys::AssertionResponse {
            credential_id: _,
            client_data_json: _,
            authenticator_data: _,
            signature: _,
        } = a;
        for (name, fields) in [("NewGrant", 10), ("RegistrationResponse", 3), ("AssertionResponse", 4)] {
            let s = match name {
                "NewGrant" => schemars::schema_for!(NewGrant),
                "RegistrationResponse" => schemars::schema_for!(RegistrationResponse),
                _ => schemars::schema_for!(AssertionResponse),
            };
            let n = s.as_value()["properties"].as_object().map_or(0, serde_json::Map::len);
            assert_eq!(n, fields, "{name}");
        }
    }
}
