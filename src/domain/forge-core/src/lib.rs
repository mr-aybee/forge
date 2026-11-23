//! Orbyn Forge domain core.
//!
//! This crate is dependency-light on purpose: it holds the vocabulary of the
//! platform (identifiers, entities, roles, actions) and the *ports* — traits
//! that infrastructure crates implement. Higher-level crates depend on these
//! abstractions, never on concrete adapters (dependency inversion).

pub mod access;
pub mod alerts;
pub mod analytics;
pub mod backoff;
pub mod catalog;
pub mod clock;
pub mod deploy;
pub mod error;
pub mod files;
pub mod ids;
pub mod logs;
pub mod model;
pub mod notifications;
pub mod observation;
pub mod passkey;
pub mod permissions;
pub mod pins;
pub mod ports;
pub mod public_address;
pub mod remote;
pub mod silences;
pub mod spool;
pub mod stacks;
pub mod storage;
pub mod swarm;
pub mod tags;
pub mod tunnels;

pub use access::{AccessRequest, Action, AuthzContext, EntityKind, EntityRef, Principal, ResourceRef, Role};
pub use backoff::Backoff;
pub use clock::{Clock, ManualClock, NEVER_EXPIRES, SystemClock, Timestamp};
pub use error::{DomainError, DomainResult};
pub use ids::*;
