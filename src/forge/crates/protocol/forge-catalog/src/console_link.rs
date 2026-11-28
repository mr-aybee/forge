//! A service's web console link (`plans/service-orchestration.md` step 5):
//! the port, path and scheme of the admin page it serves. Forge only
//! builds the URL; it neither proxies the page nor signs anyone in.
//!
//! The link is a server and console concern: it is stored on the service
//! spec under [`SPEC_KEY`] and never sent to a node.

use serde::{Deserialize, Serialize};

/// The key of [`ConsoleLink`] in a stored service spec.
pub const SPEC_KEY: &str = "console";
/// Longest path accepted.
const MAX_PATH: usize = 512;

/// The scheme a console is served with.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ConsoleScheme {
    #[default]
    Http,
    Https,
}

impl ConsoleScheme {
    /// `http` or `https`.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Http => "http",
            Self::Https => "https",
        }
    }
}

/// Where a service serves its web console.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ConsoleLink {
    /// The port the console listens on (on the node, or the container's
    /// published host port).
    pub port: u16,
    /// The path of the console page, starting with `/`.
    #[serde(default = "root")]
    pub path: String,
    #[serde(default)]
    pub scheme: ConsoleScheme,
}

fn root() -> String {
    "/".to_owned()
}

impl ConsoleLink {
    /// Checks the port and path. The message names the problem.
    pub fn validate(&self) -> Result<(), String> {
        if self.port == 0 {
            return Err("console port must be 1-65535".into());
        }

        if !self.path.starts_with('/') {
            return Err("console path must start with `/`".into());
        }

        if self.path.len() > MAX_PATH {
            return Err(format!("console path is longer than {MAX_PATH} characters"));
        }

        if self.path.chars().any(|c| c.is_whitespace() || c.is_control() || c == '\\') {
            return Err("console path must not contain spaces, control characters or `\\`".into());
        }

        Ok(())
    }

    /// The URL on `host` at the console's own port.
    #[must_use]
    pub fn url_on(&self, host: &str) -> String {
        url(self.scheme, host, Some(self.port), &self.path)
    }
}

/// `scheme://host[:port]path`, bracketing an IPv6 address and leaving out
/// the scheme's default port.
#[must_use]
pub fn url(scheme: ConsoleScheme, host: &str, port: Option<u16>, path: &str) -> String {
    let host = if host.contains(':') && !host.starts_with('[') { format!("[{host}]") } else { host.to_owned() };

    let default = match scheme {
        ConsoleScheme::Http => 80,
        ConsoleScheme::Https => 443,
    };

    match port.filter(|p| *p != default) {
        Some(p) => format!("{}://{host}:{p}{path}", scheme.as_str()),
        None => format!("{}://{host}{path}", scheme.as_str()),
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    #[test]
    fn defaults_and_urls() {
        let c: ConsoleLink = serde_json::from_value(serde_json::json!({ "port": 9001 })).unwrap();
        assert_eq!((c.path.as_str(), c.scheme), ("/", ConsoleScheme::Http));
        assert_eq!(c.url_on("192.0.2.10"), "http://192.0.2.10:9001/");
        assert_eq!(c.url_on("2001:db8::1"), "http://[2001:db8::1]:9001/");
        assert_eq!(url(ConsoleScheme::Https, "app.example.com", Some(443), "/admin"), "https://app.example.com/admin");
    }

    #[test]
    fn validation() {
        let ok = ConsoleLink { port: 8080, path: "/ui/".into(), scheme: ConsoleScheme::Https };
        assert!(ok.validate().is_ok());
        assert!(ConsoleLink { port: 0, ..ok.clone() }.validate().is_err());
        assert!(ConsoleLink { path: "ui".into(), ..ok.clone() }.validate().is_err());
        assert!(ConsoleLink { path: "/a b".into(), ..ok }.validate().is_err());
        let bad = serde_json::from_value::<ConsoleLink>(serde_json::json!({ "port": 1, "scheme": "ftp" }));
        assert!(bad.is_err());
    }
}
