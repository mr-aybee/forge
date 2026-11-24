//! One lazily (re)connected HTTP/2 connection to the control plane.

use std::sync::Arc;
use std::time::Duration;

use anyhow::{Context, bail};
use bytes::Bytes;
use http_body_util::{BodyExt, Full, Limited};
use hyper::client::conn::http2::SendRequest;
use hyper_util::rt::{TokioExecutor, TokioIo, TokioTimer};
use rustls::ClientConfig;
use rustls_pki_types::ServerName;
use tokio::net::TcpStream;

const MAX_RESPONSE: usize = 4 * 1024 * 1024;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Endpoint {
    pub host: String,
    pub port: u16,
}

impl Endpoint {
    /// Parses `https://host:port`.
    pub fn parse(url: &str) -> anyhow::Result<Self> {
        let rest = url.strip_prefix("https://").context("server URL must start with https://")?;
        let authority = rest.split('/').next().unwrap_or_default();
        let (host, port) = match authority.rsplit_once(':') {
            Some((h, p)) if !h.ends_with(']') || h.starts_with('[') => (h, p.parse().context("bad port")?),
            _ => (authority, 3843),
        };
        if host.is_empty() {
            bail!("missing host in server URL");
        }
        Ok(Self { host: host.trim_matches(['[', ']']).to_owned(), port })
    }
}

/// HTTP/2 client over rustls. Not thread-safe by design: the agent owns it.
pub struct Link {
    endpoint: Endpoint,
    tls: Arc<ClientConfig>,
    sender: Option<SendRequest<Full<Bytes>>>,
    timeout: Duration,
}

impl std::fmt::Debug for Link {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Link").field("endpoint", &self.endpoint).field("connected", &self.sender.is_some()).finish()
    }
}

impl Link {
    #[must_use]
    pub fn new(endpoint: Endpoint, tls: Arc<ClientConfig>, timeout: Duration) -> Self {
        Self { endpoint, tls, sender: None, timeout }
    }

    /// Replaces the TLS identity (after certificate renewal) and reconnects lazily.
    pub fn set_tls(&mut self, tls: Arc<ClientConfig>) {
        self.tls = tls;
        self.sender = None;
    }

    /// The control plane endpoint.
    #[must_use]
    pub fn endpoint(&self) -> &Endpoint {
        &self.endpoint
    }

    /// The current mTLS client configuration.
    #[must_use]
    pub fn tls(&self) -> Arc<ClientConfig> {
        self.tls.clone()
    }

    pub fn disconnect(&mut self) {
        self.sender = None;
    }

    async fn connect(&mut self) -> anyhow::Result<&mut SendRequest<Full<Bytes>>> {
        if self.sender.as_ref().is_some_and(|s| !s.is_closed()) {
            return self.sender.as_mut().context("sender");
        }
        let tcp = TcpStream::connect((self.endpoint.host.as_str(), self.endpoint.port)).await?;
        tcp.set_nodelay(true)?;
        let name = ServerName::try_from(self.endpoint.host.clone())?;
        let tls = tokio_rustls::TlsConnector::from(self.tls.clone()).connect(name, tcp).await?;
        let (sender, conn) = hyper::client::conn::http2::Builder::new(TokioExecutor::new())
            .timer(TokioTimer::new())
            .keep_alive_interval(Duration::from_secs(30))
            .keep_alive_timeout(Duration::from_secs(10))
            .handshake(TokioIo::new(tls))
            .await?;
        tokio::spawn(async move {
            if let Err(e) = conn.await {
                tracing::debug!(error = %e, "agent link connection closed");
            }
        });
        self.sender = Some(sender);
        self.sender.as_mut().context("sender")
    }

    /// Sends one request and returns the body. Non-2xx statuses are errors.
    pub async fn request(&mut self, method: http::Method, path: &str, body: Vec<u8>) -> anyhow::Result<Vec<u8>> {
        let req = Outgoing { method, path, content_type: forge_proto::CONTENT_TYPE, header: None, body };
        let (status, bytes) = self.send(req).await?;
        if !status.is_success() {
            bail!("control plane returned {status}: {}", String::from_utf8_lossy(&bytes));
        }
        Ok(bytes)
    }

    /// Sends one request and returns the status and the body (at most
    /// 4 MiB). Transport failures and timeouts are errors and drop the
    /// connection; any status is returned.
    pub async fn send(&mut self, out: Outgoing<'_>) -> anyhow::Result<(http::StatusCode, Vec<u8>)> {
        let uri = format!("https://{}:{}{}", self.endpoint.host, self.endpoint.port, out.path);
        let timeout = self.timeout;
        let result = async {
            let sender = self.connect().await?;
            sender.ready().await?;
            let mut req = http::Request::builder()
                .method(out.method)
                .uri(uri)
                .header(http::header::CONTENT_TYPE, out.content_type);
            if let Some((k, v)) = out.header {
                req = req.header(k, v);
            }
            let resp = sender.send_request(req.body(Full::new(Bytes::from(out.body)))?).await?;
            let status = resp.status();
            let bytes = Limited::new(resp.into_body(), MAX_RESPONSE)
                .collect()
                .await
                .map_err(|e| anyhow::anyhow!("reading response: {e}"))?
                .to_bytes();
            Ok((status, bytes.to_vec()))
        };
        match tokio::time::timeout(timeout, result).await {
            Ok(Ok(b)) => Ok(b),
            Ok(Err(e)) => {
                self.sender = None;
                Err(e)
            }
            Err(_) => {
                self.sender = None;
                bail!("request timed out after {timeout:?}")
            }
        }
    }
}

/// One request for [`Link::send`].
#[derive(Debug)]
pub struct Outgoing<'a> {
    pub method: http::Method,
    pub path: &'a str,
    pub content_type: &'a str,
    /// One extra header.
    pub header: Option<(&'a str, &'a str)>,
    pub body: Vec<u8>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_endpoints() {
        assert_eq!(
            Endpoint::parse("https://cp.example:9000").ok(),
            Some(Endpoint { host: "cp.example".into(), port: 9000 })
        );
        assert_eq!(
            Endpoint::parse("https://cp.example").ok(),
            Some(Endpoint { host: "cp.example".into(), port: 3843 })
        );
        assert_eq!(
            Endpoint::parse("https://127.0.0.1:7443/").ok(),
            Some(Endpoint { host: "127.0.0.1".into(), port: 7443 })
        );
        assert!(Endpoint::parse("http://cp.example").is_err());
    }
}
