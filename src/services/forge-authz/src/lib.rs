//! Authorization for Orbyn Forge, built on the embedded Cedar policy engine.
//!
//! * [`PolicyEngine`] owns the schema, the role templates, the guardrails
//!   and the template links created from role bindings. It evaluates fully
//!   resolved requests and has no I/O.
//! * [`CedarAuthorizer`] implements the [`forge_core::ports::Authorizer`]
//!   port by resolving resource ancestry and delegating to the engine. The
//!   same authorizer instance is used by the API, the job runner, the agent
//!   hub and the plugin host, so every enforcement point applies identical
//!   rules.

mod authorizer;
mod engine;

pub use authorizer::{CedarAuthorizer, MFA_REQUIRED_PREFIX};
pub use engine::{AuthzError, Decision, PolicyEngine};

/// The Cedar schema source, embedded at build time.
pub const SCHEMA_SRC: &str = include_str!("../schema/forge.cedarschema");
/// Role templates, one per built-in role.
pub const ROLE_TEMPLATES_SRC: &str = include_str!("../policies/roles.cedar");
/// Platform-wide forbid policies.
pub const GUARDRAILS_SRC: &str = include_str!("../policies/guardrails.cedar");
