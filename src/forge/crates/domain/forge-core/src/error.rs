//! Errors shared across ports.

use thiserror::Error;

/// Error type returned by ports. Adapters map their own failures into it so
/// callers never depend on adapter-specific error types.
#[derive(Debug, Error)]
pub enum DomainError {
    #[error("not found: {0}")]
    NotFound(String),
    #[error("conflict: {0}")]
    Conflict(String),
    #[error("invalid input: {0}")]
    Invalid(String),
    #[error("access denied: {0}")]
    Denied(String),
    #[error("unauthenticated")]
    Unauthenticated,
    #[error("lease lost for job {0}")]
    LeaseLost(String),
    #[error("internal error: {0}")]
    Internal(String),
}

impl DomainError {
    pub fn internal(e: impl std::fmt::Display) -> Self {
        Self::Internal(e.to_string())
    }
}

pub type DomainResult<T> = Result<T, DomainError>;
