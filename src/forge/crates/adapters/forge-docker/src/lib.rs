//! Docker Engine API over mutual TLS (D-125).
//!
//! * [`catalog`]: every Engine API endpoint Forge forwards, with the class
//!   of action each needs; anything else is refused.
//! * [`guard`]: request bodies that reach the node itself (privileged
//!   containers, host mounts) and so need the administrator class.
//! * [`client`]: the control plane's TLS client for one node's daemon.

pub mod catalog;
pub mod client;
pub mod guard;

pub use catalog::{Class, Endpoint, MatchError, Matched, Transfer, lookup};
pub use client::{ClientError, Target};
