//! Durable job engine.
//!
//! A job is a row plus an ordered list of named steps defined by a
//! [`JobHandler`]. Workers claim jobs with a time-bounded lease and a fencing
//! token, run steps one at a time, and persist state after every step. If the
//! control plane dies mid-step, the lease expires and the next worker resumes
//! at the first incomplete step. Steps must therefore be idempotent.
//!
//! The runner re-authorizes the job's principal before **every** step, so a
//! revoked role stops a job that was allowed when it was submitted.

mod engine;
mod handler;

pub use engine::{EngineConfig, JobEngine, JobEvent};
pub use handler::{JobHandler, JobRegistry, StepContext, StepError};
