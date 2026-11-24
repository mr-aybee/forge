//! API error type: maps domain errors to RFC 9457 problem responses without
//! leaking internals.

use axum::Json;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use forge_core::DomainError;
use forge_identity::IdentityError;
use serde_json::json;

#[derive(Debug)]
pub struct ApiError {
    pub status: StatusCode,
    pub code: &'static str,
    pub message: String,
}

impl ApiError {
    pub fn new(status: StatusCode, code: &'static str, message: impl Into<String>) -> Self {
        Self { status, code, message: message.into() }
    }

    pub fn bad_request(message: impl Into<String>) -> Self {
        Self::new(StatusCode::BAD_REQUEST, "bad_request", message)
    }

    pub fn unauthorized() -> Self {
        Self::new(StatusCode::UNAUTHORIZED, "unauthenticated", "authentication required")
    }

    pub fn forbidden(message: impl Into<String>) -> Self {
        Self::new(StatusCode::FORBIDDEN, "forbidden", message)
    }

    pub fn not_found(message: impl Into<String>) -> Self {
        Self::new(StatusCode::NOT_FOUND, "not_found", message)
    }

    pub fn conflict(message: impl Into<String>) -> Self {
        Self::new(StatusCode::CONFLICT, "conflict", message)
    }
}

pub type ApiResult<T> = Result<T, ApiError>;

/// Error code: analytics is off in `forge.toml`.
pub const ERR_ANALYTICS_OFF: &str = "analytics_off";
/// Error code: the analytics worker process is not running (D-141).
pub const ERR_ANALYTICS_WORKER_DOWN: &str = "analytics_worker_down";
/// Error code: the analytics database is unreachable or refused.
pub const ERR_ANALYTICS_UNAVAILABLE: &str = "analytics_unavailable";
/// Error code: the log ingest queue is full.
pub const ERR_INGEST_BUSY: &str = "ingest_busy";
/// Error code: the analytics engine is short of disk.
pub const ERR_INGEST_DISK_LOW: &str = "ingest_disk_low";
/// Error code: a feature needs a pinned public address (D-167).
pub const ERR_NO_PUBLIC_ADDRESS: &str = "no_public_address";
/// Error code: every log search slot is in use.
pub const ERR_SEARCH_BUSY: &str = "search_busy";

impl From<forge_core::analytics::AnalyticsError> for ApiError {
    fn from(e: forge_core::analytics::AnalyticsError) -> Self {
        use forge_core::analytics::AnalyticsError as A;
        let busy = |code, message: String| ApiError::new(StatusCode::SERVICE_UNAVAILABLE, code, message);
        match e {
            A::Off => {
                busy(ERR_ANALYTICS_OFF, "analytics is off on this server; set [analytics] engine in forge.toml".into())
            }
            A::WorkerDown => {
                busy(ERR_ANALYTICS_WORKER_DOWN, "the analytics worker is not running; retry shortly".into())
            }
            A::Unavailable(m) => {
                // The reason names internals (hosts, paths): logged, not sent.
                tracing::warn!(reason = %m, "analytics is unavailable");
                busy(ERR_ANALYTICS_UNAVAILABLE, "analytics is unavailable".to_owned())
            }
            A::Busy => busy(ERR_INGEST_BUSY, "too many log batches are waiting to be stored; retry shortly".into()),
            A::DiskLow => busy(ERR_INGEST_DISK_LOW, "the analytics database is low on disk space; retry later".into()),
            A::SearchBusy => busy(ERR_SEARCH_BUSY, "too many log searches are running; retry shortly".into()),
            A::Invalid(m) | A::BadBody(m) => ApiError::bad_request(m),
        }
    }
}

impl From<DomainError> for ApiError {
    fn from(e: DomainError) -> Self {
        match e {
            DomainError::NotFound(m) => Self::not_found(m),
            DomainError::Conflict(m) => Self::new(StatusCode::CONFLICT, "conflict", m),
            DomainError::Invalid(m) => Self::new(StatusCode::UNPROCESSABLE_ENTITY, "invalid", m),
            DomainError::Denied(m) => match m.strip_prefix(forge_authz::MFA_REQUIRED_PREFIX) {
                Some(rest) => Self::new(StatusCode::FORBIDDEN, "step_up_required", rest.trim().to_owned()),
                None => Self::forbidden(m),
            },
            DomainError::Unauthenticated => Self::unauthorized(),
            DomainError::LeaseLost(_) | DomainError::Internal(_) => {
                tracing::error!(error = %e, "internal error");
                Self::new(StatusCode::INTERNAL_SERVER_ERROR, "internal", "internal error")
            }
        }
    }
}

impl From<IdentityError> for ApiError {
    fn from(e: IdentityError) -> Self {
        match e {
            IdentityError::InvalidCredentials => {
                Self::new(StatusCode::UNAUTHORIZED, "invalid_credentials", "invalid email or password")
            }
            IdentityError::MfaRequired => {
                Self::new(StatusCode::UNAUTHORIZED, "mfa_required", "a one-time code is required")
            }
            IdentityError::InvalidMfa => Self::new(StatusCode::UNAUTHORIZED, "invalid_mfa", "invalid one-time code"),
            IdentityError::Disabled => Self::new(StatusCode::FORBIDDEN, "disabled", "account disabled"),
            IdentityError::InvalidGrant(m) => Self::new(StatusCode::BAD_REQUEST, "invalid_grant", m),
            IdentityError::InvalidRequest(m) => Self::bad_request(m),
            IdentityError::Provider(m) => {
                tracing::warn!(error = %m, "identity provider error");
                Self::new(StatusCode::BAD_GATEWAY, "provider_error", "identity provider error")
            }
            IdentityError::InvalidRedirect(m) => Self::new(StatusCode::BAD_REQUEST, "invalid_redirect_uri", m),
            IdentityError::Throttled => {
                Self::new(StatusCode::TOO_MANY_REQUESTS, "throttled", "too many requests; try later")
            }
            IdentityError::InvalidTarget => {
                Self::new(StatusCode::BAD_REQUEST, "invalid_target", "this client cannot request that resource")
            }
            IdentityError::Store(d) => d.into(),
        }
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        let body = json!({
            "type": format!("https://docs.orbyn.dev/errors/{}", self.code),
            "title": self.code,
            "status": self.status.as_u16(),
            "detail": self.message,
        });
        let mut resp = (self.status, Json(body)).into_response();
        resp.headers_mut()
            .insert(axum::http::header::CONTENT_TYPE, axum::http::HeaderValue::from_static("application/problem+json"));
        if self.code == "step_up_required" {
            // RFC 9470: the client may raise the authentication level and retry
            // (POST /api/v1/auth/step-up with a one-time code).
            resp.headers_mut().insert(
                axum::http::header::WWW_AUTHENTICATE,
                axum::http::HeaderValue::from_static(
                    "Bearer error=\"insufficient_user_authentication\", \
                     error_description=\"multi-factor authentication required\", acr_values=\"mfa\"",
                ),
            );
        }
        resp
    }
}
