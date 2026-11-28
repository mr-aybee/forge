//! Who may reach the control plane at all, and how often (D-161).
//!
//! `[security.access]`: `deny` CIDRs are refused everywhere; when `allow` is
//! not empty, only those CIDRs reach the console, API, MCP and OAuth;
//! `/hooks/*` uses `webhooks_allow` and the node installers and agent
//! downloads use `nodes_allow` instead, so CI and new nodes keep working
//! behind a closed console. `[security.rate_limit]` limits requests per
//! client address (an IPv6 /64 counts as one). Agents connect on their own
//! listener and are not affected. The client address honours
//! `[security] trusted_proxies`.

use std::net::IpAddr;
use std::time::Duration;

use axum::extract::{ConnectInfo, Request, State};
use axum::http::{HeaderValue, StatusCode, header};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};

use crate::rate_limit::Windows;
use crate::state::AppState;
use crate::throttle::TrustedNet;

/// Paths never limited (load balancer probes).
const EXEMPT: &[&str] = &["/api/health"];
const WINDOW: Duration = Duration::from_secs(60);

/// Per-address counters for the API-wide limit.
#[derive(Debug, Default)]
pub struct Limiter {
    pub all: Windows<IpAddr>,
}

/// The parsed lists (checked by `ServerConfig::validate`).
#[derive(Debug, Clone, Default)]
pub struct Lists {
    pub allow: Vec<TrustedNet>,
    pub deny: Vec<TrustedNet>,
    pub webhooks_allow: Vec<TrustedNet>,
    pub nodes_allow: Vec<TrustedNet>,
}

/// Paths new nodes fetch while they enroll.
fn node_path(path: &str) -> bool {
    path == "/install.sh" || path == "/install.ps1" || path.starts_with("/agent/download/")
}

impl Lists {
    /// Whether `ip` may reach `path`.
    #[must_use]
    pub fn permits(&self, ip: IpAddr, path: &str) -> bool {
        if self.deny.iter().any(|n| n.contains(ip)) {
            return false;
        }

        let allow = if path.starts_with("/hooks/") {
            &self.webhooks_allow
        } else if node_path(path) {
            &self.nodes_allow
        } else {
            &self.allow
        };

        allow.is_empty() || allow.iter().any(|n| n.contains(ip))
    }
}

fn refuse(status: StatusCode, message: &'static str) -> Response {
    let mut r = (status, axum::Json(serde_json::json!({ "error": message }))).into_response();

    if status == StatusCode::TOO_MANY_REQUESTS {
        r.headers_mut().insert(header::RETRY_AFTER, HeaderValue::from_static("60"));
    }

    r
}

pub async fn guard(
    State(state): State<AppState>,
    ConnectInfo(peer): ConnectInfo<std::net::SocketAddr>,
    req: Request,
    next: Next,
) -> Response {
    let ip = crate::throttle::client_ip(req.headers(), peer, &state.config.trusted_proxies());
    let path = req.uri().path();
    if !state.access_lists.permits(ip, path) {
        return refuse(StatusCode::FORBIDDEN, "access from this address is not allowed");
    }

    let limits = &state.config.security.rate_limit;
    if limits.enabled && !EXEMPT.contains(&path) {
        let limit = usize::try_from(limits.per_ip_per_minute).unwrap_or(usize::MAX);
        if !state.limiter.all.allow(&crate::rate_limit::client_key(ip), limit, WINDOW) {
            return refuse(StatusCode::TOO_MANY_REQUESTS, "too many requests from this address; slow down");
        }
    }

    next.run(req).await
}

#[cfg(test)]
mod tests {
    use super::*;

    fn nets(v: &[&str]) -> Vec<TrustedNet> {
        v.iter().map(|s| s.parse().unwrap_or_else(|e| panic!("{e}"))).collect()
    }

    #[test]
    fn deny_wins_allow_narrows_and_webhooks_have_their_own_list() {
        let ip = |s: &str| s.parse::<IpAddr>().unwrap_or_else(|e| panic!("{e}"));
        let open = Lists::default();
        assert!(open.permits(ip("198.51.100.7"), "/api/v1/me"));
        let l = Lists { allow: nets(&["10.0.0.0/8"]), deny: nets(&["10.9.0.0/16"]), ..Lists::default() };
        assert!(l.permits(ip("10.1.2.3"), "/api/v1/me"));
        assert!(!l.permits(ip("10.9.0.1"), "/api/v1/me"), "deny wins");
        assert!(!l.permits(ip("198.51.100.7"), "/"));
        assert!(l.permits(ip("198.51.100.7"), "/hooks/whk_x"), "webhooks are not behind the console allow list");
        assert!(!l.permits(ip("10.9.0.1"), "/hooks/whk_x"), "deny applies to webhooks too");
        let ci = Lists { webhooks_allow: nets(&["203.0.113.0/24"]), ..Lists::default() };
        assert!(ci.permits(ip("203.0.113.5"), "/hooks/whk_x"));
        assert!(!ci.permits(ip("198.51.100.7"), "/hooks/whk_x"));
        assert!(l.permits(ip("198.51.100.7"), "/install.sh"), "nodes are not behind the console allow list");
        let nodes = Lists { nodes_allow: nets(&["192.0.2.0/24"]), ..Lists::default() };
        assert!(!nodes.permits(ip("198.51.100.7"), "/agent/download/linux/x86_64/forge-agent"));
    }
}
