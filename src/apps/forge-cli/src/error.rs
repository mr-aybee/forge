//! Typed CLI errors and the documented exit codes.

use std::fmt;

/// Process exit codes (stable; documented in `docs/reference/cli.md`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Exit {
    General = 1,
    Usage = 2,
    Auth = 3,
    Denied = 4,
    MfaRequired = 5,
    NotFound = 6,
    Conflict = 7,
    Timeout = 8,
    Transport = 9,
    JobFailed = 10,
}

impl Exit {
    /// Stable machine code for JSON output.
    #[must_use]
    pub fn code(self) -> &'static str {
        match self {
            Self::General => "error",
            Self::Usage => "invalid_input",
            Self::Auth => "unauthenticated",
            Self::Denied => "permission_denied",
            Self::MfaRequired => "mfa_required",
            Self::NotFound => "not_found",
            Self::Conflict => "conflict",
            Self::Timeout => "timeout",
            Self::Transport => "transport",
            Self::JobFailed => "job_failed",
        }
    }
}

/// An error with an exit class, a stable code and a human message.
#[derive(Debug)]
pub struct CliError {
    pub exit: Exit,
    /// Server problem `title` when there is one (e.g. `step_up_required`), else `exit.code()`.
    pub code: String,
    pub message: String,
    pub retryable: bool,
    pub request_id: Option<String>,
    /// Seconds from a `Retry-After` response header.
    pub retry_after: Option<u64>,
}

impl CliError {
    pub fn new(exit: Exit, message: impl Into<String>) -> Self {
        Self {
            exit,
            code: exit.code().to_owned(),
            message: message.into(),
            retryable: false,
            request_id: None,
            retry_after: None,
        }
    }

    pub fn usage(message: impl Into<String>) -> Self {
        Self::new(Exit::Usage, message)
    }

    pub fn auth(message: impl Into<String>) -> Self {
        Self::new(Exit::Auth, message)
    }

    #[must_use]
    pub fn with_code(mut self, code: impl Into<String>) -> Self {
        self.code = code.into();
        self
    }

    /// Maps an HTTP status and RFC 9457 problem title to an exit class.
    #[must_use]
    pub fn from_http(status: u16, title: Option<&str>, detail: Option<&str>, request_id: Option<String>) -> Self {
        let title = title.unwrap_or_default();
        let mfa = matches!(
            title,
            "mfa_required"
                | "step_up_required"
                | "fresh_mfa_required"
                | "mfa_enrollment_required"
                | "approval_required"
        );
        let exit = match status {
            _ if mfa => Exit::MfaRequired,
            400 | 422 => Exit::Usage,
            401 => Exit::Auth,
            403 => Exit::Denied,
            404 => Exit::NotFound,
            409 | 412 => Exit::Conflict,
            408 | 504 => Exit::Timeout,
            502 | 503 => Exit::Transport,
            _ => Exit::General,
        };
        let message = detail.filter(|d| !d.is_empty()).map_or_else(|| format!("HTTP {status}"), str::to_owned);
        Self {
            exit,
            code: if title.is_empty() { exit.code().to_owned() } else { title.to_owned() },
            message,
            // `idempotency_in_progress`: the same request is still running.
            retryable: matches!(status, 408 | 429 | 502 | 503 | 504) || title == "idempotency_in_progress",
            request_id,
            retry_after: None,
        }
    }
}

impl fmt::Display for CliError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for CliError {}

impl From<std::io::Error> for CliError {
    fn from(e: std::io::Error) -> Self {
        Self::new(Exit::General, e.to_string())
    }
}

impl From<serde_json::Error> for CliError {
    fn from(e: serde_json::Error) -> Self {
        Self::new(Exit::General, format!("invalid JSON: {e}"))
    }
}

pub type Result<T> = std::result::Result<T, CliError>;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn http_statuses_map_to_documented_exit_codes() {
        assert_eq!(CliError::from_http(401, None, None, None).exit, Exit::Auth);
        assert_eq!(CliError::from_http(403, Some("forbidden"), None, None).exit, Exit::Denied);
        // A step-up demand is not a permission denial.
        let e = CliError::from_http(403, Some("step_up_required"), Some("verify with MFA"), None);
        assert_eq!((e.exit, e.code.as_str()), (Exit::MfaRequired, "step_up_required"));
        assert_eq!(CliError::from_http(404, None, None, None).exit, Exit::NotFound);
        assert_eq!(CliError::from_http(409, None, None, None).exit, Exit::Conflict);
        assert!(CliError::from_http(503, None, None, None).retryable);
        assert!(CliError::from_http(409, Some("idempotency_in_progress"), None, None).retryable);
        assert!(!CliError::from_http(409, Some("conflict"), None, None).retryable);
        assert_eq!(CliError::from_http(422, Some("idempotency_key_reused"), None, None).exit, Exit::Usage);
        assert_eq!(CliError::from_http(500, None, None, None).exit as i32, 1);
    }
}
