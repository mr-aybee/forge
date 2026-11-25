//! MCP connectors on Forge's OAuth server (issue #6): dynamic client
//! registration (RFC 7591), the consent that creates an [`AgentConnection`],
//! and audience-bound codes (RFC 8707). Connector tokens are bearer sessions
//! with a `connection_id`; the API refuses them outside `/mcp`.

use std::sync::Arc;
use std::time::Duration;

use forge_core::model::{AgentConnection, AgentMode, OAuthClientRecord, OAuthCode};
use forge_core::ports::ConnectorRepository;
use forge_core::{Timestamp, UserId};
use forge_secrets::crypto::{random_token, sha256_hex};

use crate::service::{IdentityError, IdentityService};

/// Redirect URIs per registered client.
pub const MAX_REDIRECT_URIS: usize = 10;
const MAX_URI_LEN: usize = 2048;
const MAX_NAME_LEN: usize = 80;
/// Schemes a redirect URI may never use.
/// Custom URI schemes of editors that run MCP clients.
const EDITOR_SCHEMES: &[&str] = &["cursor", "vscode", "vscode-insiders", "windsurf", "zed"];
const FORBIDDEN_SCHEMES: &[&str] = &["javascript", "data", "file", "vbscript", "about", "blob", "ftp", "ws", "wss"];

impl IdentityService {
    /// Enables connectors (dynamic registration and consent).
    #[must_use]
    pub fn with_connectors(mut self, repo: Arc<dyn ConnectorRepository>, enabled: bool) -> Self {
        self.connectors = Some(repo);
        self.connectors_enabled = enabled;

        self
    }

    pub(crate) fn connector_repo(&self) -> Result<&Arc<dyn ConnectorRepository>, IdentityError> {
        self.connectors.as_ref().ok_or_else(|| IdentityError::InvalidRequest("connectors are not enabled".into()))
    }

    /// RFC 7591 registration of a public client. `hourly_cap` limits how
    /// many clients anyone may register per hour (the endpoint is anonymous).
    pub async fn register_client(
        &self,
        name: &str,
        redirect_uris: &[String],
        created_ip: Option<String>,
        hourly_cap: u64,
    ) -> Result<OAuthClientRecord, IdentityError> {
        let repo = self.connector_repo()?;
        let name = clean_name(name)?;

        if redirect_uris.is_empty() || redirect_uris.len() > MAX_REDIRECT_URIS {
            return Err(invalid_redirect(format!("between 1 and {MAX_REDIRECT_URIS} redirect_uris are required")));
        }

        for uri in redirect_uris {
            validate_redirect_uri(uri)?;
        }

        let now = self.clock.now();
        let since = Timestamp(now.0 - 3_600_000);
        if repo.count_clients_since(since).await? >= hourly_cap {
            return Err(IdentityError::Throttled);
        }

        let client = OAuthClientRecord {
            client_id: format!("mcp_{}", random_token::<18>()),
            name,
            redirect_uris: redirect_uris.to_vec(),
            created_at: now,
            created_ip,
            revoked_at: None,
            last_used_at: None,
        };
        repo.create_client(&client).await?;

        Ok(client)
    }

    /// An active registered client.
    pub async fn connector_client(&self, client_id: &str) -> Result<OAuthClientRecord, IdentityError> {
        self.connector_repo()?
            .get_client(client_id)
            .await?
            .filter(|c| c.revoked_at.is_none())
            .ok_or_else(|| IdentityError::InvalidRequest("unknown client_id".into()))
    }

    /// Whether `client_id` is a first-party client from the configuration.
    #[must_use]
    pub fn is_first_party(&self, client_id: &str) -> bool {
        self.config.clients.iter().any(|c| c.client_id == client_id)
    }

    /// Checks an authorization request from a registered connector before
    /// anyone is asked for consent. Redirect URIs compare exactly.
    pub async fn check_connector_request(
        &self,
        client_id: &str,
        redirect_uri: &str,
        code_challenge: &str,
        code_challenge_method: &str,
    ) -> Result<OAuthClientRecord, IdentityError> {
        let client = self.connector_client(client_id).await?;
        if !client.redirect_uris.iter().any(|u| u == redirect_uri) {
            return Err(IdentityError::InvalidRequest("redirect_uri not registered".into()));
        }

        if code_challenge_method != "S256" || code_challenge.len() != 43 {
            return Err(IdentityError::InvalidRequest("PKCE S256 code_challenge required".into()));
        }

        Ok(client)
    }

    /// The person approved the connector on the consent page: records the
    /// connection and returns the one-time code for `redirect_uri`. The code
    /// never carries MFA: agents cannot use step-up.
    #[allow(clippy::too_many_arguments)]
    pub async fn approve_connection(
        &self,
        user: UserId,
        client_id: &str,
        redirect_uri: &str,
        code_challenge: &str,
        resource: &str,
        mode: AgentMode,
    ) -> Result<(AgentConnection, String), IdentityError> {
        let client = self.check_connector_request(client_id, redirect_uri, code_challenge, "S256").await?;
        let repo = self.connector_repo()?;
        let now = self.clock.now();
        let connection = AgentConnection {
            id: format!("ac_{}", random_token::<12>()),
            user_id: user,
            client_id: client.client_id.clone(),
            client_name: client.name.clone(),
            redirect_host: redirect_host(redirect_uri),
            resource: resource.to_owned(),
            mode,
            created_at: now,
            last_used_at: None,
            revoked_at: None,
        };
        repo.create_connection(&connection).await?;
        let code = random_token::<32>();
        self.sessions
            .create_oauth_code(&OAuthCode {
                code_hash: sha256_hex(code.as_bytes()),
                user_id: user,
                client_id: client.client_id,
                redirect_uri: redirect_uri.to_owned(),
                code_challenge: code_challenge.to_owned(),
                mfa: false,
                expires_at: now.plus(self.config.oauth_code_ttl),
                connection_id: Some(connection.id.clone()),
            })
            .await?;

        Ok((connection, code))
    }

    /// An active connection (not revoked).
    pub(crate) async fn active_connection(&self, id: &str) -> Result<Option<AgentConnection>, IdentityError> {
        let Some(repo) = self.connectors.as_ref() else { return Ok(None) };
        let Some(c) = repo.get_connection(id).await?.filter(|c| c.revoked_at.is_none()) else { return Ok(None) };

        // A revoked client ends its connections, even one approved meanwhile.
        let client_ok = repo.get_client(&c.client_id).await?.is_some_and(|cl| cl.revoked_at.is_none());

        Ok(client_ok.then_some(c))
    }

    pub async fn list_connections(&self, user: Option<UserId>) -> Result<Vec<AgentConnection>, IdentityError> {
        Ok(self.connector_repo()?.list_connections(user).await?)
    }

    pub async fn get_connection(&self, id: &str) -> Result<Option<AgentConnection>, IdentityError> {
        Ok(self.connector_repo()?.get_connection(id).await?)
    }

    /// Ends a connection: its tokens stop working at once.
    pub async fn revoke_connection(&self, id: &str) -> Result<(), IdentityError> {
        Ok(self.connector_repo()?.revoke_connection(id, self.clock.now()).await?)
    }

    pub async fn list_clients(&self) -> Result<Vec<OAuthClientRecord>, IdentityError> {
        Ok(self.connector_repo()?.list_clients().await?)
    }

    /// Revokes a registered client and every connection made with it.
    pub async fn revoke_client(&self, client_id: &str) -> Result<(), IdentityError> {
        Ok(self.connector_repo()?.revoke_client(client_id, self.clock.now()).await?)
    }

    /// Records use of a connection, at most once a minute.
    pub async fn touch_connection(&self, c: &AgentConnection) {
        let now = self.clock.now();
        if c.last_used_at.is_some_and(|t| now.0 - t.0 < Duration::from_secs(60).as_millis() as i64) {
            return;
        }

        if let Some(repo) = self.connectors.as_ref() {
            let _ = repo.touch_connection(&c.id, now).await;
        }
    }
}

/// Invisible formatting characters (Unicode Cf) that can disguise a name:
/// bidi overrides and isolates, zero-width characters, soft hyphen, BOM.
fn is_format_char(c: char) -> bool {
    matches!(c, '\u{00AD}' | '\u{061C}' | '\u{180E}' | '\u{200B}'..='\u{200F}' | '\u{202A}'..='\u{202E}'
        | '\u{2060}'..='\u{206F}' | '\u{FEFF}' | '\u{FFF9}'..='\u{FFFB}')
}

fn invalid_redirect(m: impl Into<String>) -> IdentityError {
    IdentityError::InvalidRedirect(m.into())
}

fn clean_name(name: &str) -> Result<String, IdentityError> {
    let n: String = name.trim().chars().filter(|c| !c.is_control() && !is_format_char(*c)).collect();
    let n = n.trim().to_owned();
    if n.is_empty() || n.chars().count() > MAX_NAME_LEN {
        return Err(IdentityError::InvalidRequest(format!(
            "client_name is required (up to {MAX_NAME_LEN} characters)"
        )));
    }

    Ok(n)
}

/// A registrable redirect URI: `https` with a host, `http` on a loopback
/// host (RFC 8252), or a private-use scheme for native apps. Never a
/// fragment, wildcard, user info or a dangerous scheme.
pub fn validate_redirect_uri(uri: &str) -> Result<(), IdentityError> {
    if uri.len() > MAX_URI_LEN || uri.contains('*') || uri.contains('#') || uri.chars().any(char::is_whitespace) {
        return Err(invalid_redirect(format!("invalid redirect URI `{}`", short(uri))));
    }

    let u = url::Url::parse(uri).map_err(|_| invalid_redirect(format!("invalid redirect URI `{}`", short(uri))))?;
    if !u.username().is_empty() || u.password().is_some() {
        return Err(invalid_redirect("redirect URIs may not contain user info"));
    }

    match u.scheme() {
        "https" if u.host_str().is_some_and(|h| !h.is_empty()) => Ok(()),
        "http" if matches!(u.host_str(), Some("127.0.0.1" | "[::1]" | "localhost")) => Ok(()),
        "http" => Err(invalid_redirect("http redirect URIs must use a loopback host (127.0.0.1, [::1] or localhost)")),
        // Native apps: a reverse-domain private-use scheme (RFC 8252 §7.1),
        // such as `com.example.app`, or a known editor's scheme; never a
        // browser or OS handler scheme.
        s if !FORBIDDEN_SCHEMES.contains(&s) && (s.contains('.') || EDITOR_SCHEMES.contains(&s)) => Ok(()),
        s => Err(invalid_redirect(format!(
            "redirect URI scheme `{s}` is not allowed: use https, http on a loopback host, or a reverse-domain scheme"
        ))),
    }
}

/// What the consent page shows for a redirect URI: `host[:port]`, or the
/// scheme of a native app's private-use URI.
#[must_use]
pub fn redirect_host(uri: &str) -> String {
    match url::Url::parse(uri) {
        Ok(u) => {
            let host = match (u.host_str(), u.port()) {
                (Some(h), Some(p)) => format!("{h}:{p}"),
                (Some(h), None) => h.to_owned(),
                _ => String::new(),
            };

            if matches!(u.scheme(), "http" | "https") { host } else { format!("{}://{host}", u.scheme()) }
        }
        Err(_) => String::new(),
    }
}

fn short(s: &str) -> String {
    s.chars().take(80).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn redirect_uris_are_strict() {
        for ok in [
            "https://claude.ai/api/mcp/auth_callback",
            "http://127.0.0.1:33418/callback",
            "http://localhost:6274/oauth/callback",
            "http://[::1]:8080/cb",
            "cursor://anysphere.cursor-retrieval/oauth/callback",
            "vscode://vscode.github-authentication/did-authenticate",
            "com.example.agent:/oauth/callback",
        ] {
            assert!(validate_redirect_uri(ok).is_ok(), "{ok}");
        }

        for bad in [
            "https://*.example.com/cb",
            "https://example.com/cb#frag",
            "http://example.com/cb",
            "https://user:pw@example.com/cb",
            "javascript:alert(1)",
            "data:text/html,x",
            "file:///etc/passwd",
            "not a uri",
            "vbscript:x",
            "ms-msdt:/id",
            "search-ms:query=x",
            "mailto:a@b.c",
        ] {
            assert!(validate_redirect_uri(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn consent_shows_the_redirect_host() {
        assert_eq!(redirect_host("https://claude.ai/api/mcp/auth_callback"), "claude.ai");
        assert_eq!(redirect_host("http://127.0.0.1:33418/callback"), "127.0.0.1:33418");
        assert_eq!(redirect_host("cursor://anysphere.cursor-retrieval/cb"), "cursor://anysphere.cursor-retrieval");
        assert_eq!(clean_name("Cla\u{202E}ude\u{200B}").ok().as_deref(), Some("Claude"));
    }
}
