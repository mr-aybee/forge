//! Object storage services (D-170): discovery of RustFS containers on nodes,
//! settings after install, and access keys kept in RustFS's own IAM. The API
//! handlers in `api/objstore.rs` parse, authorize, call these and audit.

pub mod adopt;
pub mod discovery;
pub mod iam;
pub mod settings;
