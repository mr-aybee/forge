//! Alarm notifications (#11, D-110): the HTTP transport, the vault-backed
//! secrets and the background loop behind `forge_alerts::notify::Notifier`.
//!
//! The transport is the only place Forge sends a notification from. It
//! connects only to addresses the target rules allow, checked on every
//! connection (so a host name cannot be re-pointed after a channel is
//! saved), follows no redirect, uses no proxy, verifies TLS, bounds the
//! time of a request and never reads the receiver's answer beyond its
//! status.

use std::error::Error as StdError;
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use forge_alerts::notify::target::{TargetRules, address_allowed, check_address};
use forge_alerts::notify::{ChannelSecrets, Notifier, NotifyConfig, Outbound, ResourceNames, SendOutcome, Transport};
use forge_core::notifications::FailureClass;
use forge_core::ports::{ServerRepository, ServiceRepository, VaultRepository};
use forge_core::{Backoff, ChannelId, DomainError, DomainResult, EntityKind, EntityRef, VaultItemId, WorkspaceId};
use forge_secrets::Envelope;
use tokio::sync::watch;

use crate::config::AlertSettings;
use crate::state::AppState;

/// Old deliveries are pruned this often.
const PRUNE_EVERY: Duration = Duration::from_secs(3600);
/// Time to connect to a receiver.
const CONNECT_TIMEOUT: Duration = Duration::from_secs(5);
/// Time for a whole request, answer headers included.
const REQUEST_TIMEOUT: Duration = Duration::from_secs(15);
/// How long one attempt holds a delivery; longer than [`REQUEST_TIMEOUT`].
const LEASE: Duration = Duration::from_secs(60);

/// A name whose addresses the rules refuse. Carried through the HTTP
/// client's error so the attempt is recorded as a blocked target, not as a
/// connection failure.
#[derive(Debug)]
struct Blocked;

impl std::fmt::Display for Blocked {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("the name resolves to an address notifications are not sent to")
    }
}

impl StdError for Blocked {}

/// Resolves a receiver's name and refuses it when **any** of its addresses
/// is not allowed: a name that answers with a public and an internal
/// address is not one to send to. The client connects only to the addresses
/// returned here.
#[derive(Debug, Clone, Copy)]
struct GuardedResolver {
    rules: TargetRules,
}

impl reqwest::dns::Resolve for GuardedResolver {
    fn resolve(&self, name: reqwest::dns::Name) -> reqwest::dns::Resolving {
        let rules = self.rules;
        let host = name.as_str().to_owned();
        Box::pin(async move {
            let addrs: Vec<SocketAddr> = tokio::net::lookup_host((host.as_str(), 0)).await?.collect();
            if addrs.is_empty() || addrs.iter().any(|a| !address_allowed(a.ip(), rules)) {
                return Err(Box::new(Blocked) as Box<dyn StdError + Send + Sync>);
            }
            let iter: reqwest::dns::Addrs = Box::new(addrs.into_iter());
            Ok(iter)
        })
    }
}

/// Whether `T` is anywhere in an error's chain of causes.
fn caused_by<T: StdError + 'static>(e: &(dyn StdError + 'static)) -> bool {
    let mut cur: Option<&(dyn StdError + 'static)> = Some(e);
    while let Some(c) = cur {
        if c.is::<T>() {
            return true;
        }
        // An I/O error can wrap its cause without naming it as the source.
        if let Some(inner) = c.downcast_ref::<std::io::Error>().and_then(std::io::Error::get_ref)
            && caused_by::<T>(inner)
        {
            return true;
        }
        cur = c.source();
    }
    false
}

/// The class of a failed request. Never the error's text: it can quote the
/// URL, and the URL can carry a token.
fn classify(e: &reqwest::Error) -> FailureClass {
    if caused_by::<Blocked>(e) {
        FailureClass::BlockedTarget
    } else if e.is_timeout() {
        FailureClass::Timeout
    } else if caused_by::<rustls::Error>(e) {
        FailureClass::Tls
    } else {
        FailureClass::Connect
    }
}

/// Sends notifications over HTTP(S).
#[derive(Debug, Clone)]
pub struct HttpTransport {
    client: reqwest::Client,
    rules: TargetRules,
}

impl HttpTransport {
    pub fn new(rules: TargetRules) -> Result<Self, reqwest::Error> {
        let client = reqwest::Client::builder()
            .connect_timeout(CONNECT_TIMEOUT)
            .timeout(REQUEST_TIMEOUT)
            // A redirect could lead anywhere; a receiver that moved is fixed
            // in the channel, by a person.
            .redirect(reqwest::redirect::Policy::none())
            // A proxy would resolve the name itself, past the address rules.
            .no_proxy()
            .dns_resolver(Arc::new(GuardedResolver { rules }))
            .user_agent(concat!("Orbyn-Forge-Alerts/", env!("CARGO_PKG_VERSION")))
            .build()?;
        Ok(Self { client, rules })
    }
}

#[async_trait]
impl Transport for HttpTransport {
    async fn send(&self, request: &Outbound) -> SendOutcome {
        // The resolver is not asked about a literal address: check it here.
        let literal = match request.url.host() {
            Some(url::Host::Ipv4(a)) => Some(std::net::IpAddr::V4(a)),
            Some(url::Host::Ipv6(a)) => Some(std::net::IpAddr::V6(a)),
            Some(url::Host::Domain(_)) => None,
            None => return SendOutcome::Failed(FailureClass::Config),
        };
        if literal.is_some_and(|ip| check_address(ip, self.rules).is_err()) {
            return SendOutcome::Failed(FailureClass::BlockedTarget);
        }
        if request.url.scheme() == "http" && !self.rules.allow_http {
            return SendOutcome::Failed(FailureClass::BlockedTarget);
        }
        let mut req = self.client.post(request.url.clone()).body(request.body.clone());
        for (name, value) in &request.headers {
            req = req.header(*name, value);
        }
        match req.send().await {
            // The answer's body is dropped unread: only the status counts.
            Ok(resp) => SendOutcome::Status(resp.status().as_u16()),
            Err(e) => SendOutcome::Failed(classify(&e)),
        }
    }
}

/// Seals channel URLs with the server's envelope and reads signing secrets
/// from the credential vault when a message is sent.
#[derive(Debug, Clone)]
pub struct VaultSecrets {
    pub vault: Arc<dyn VaultRepository>,
    pub envelope: Envelope,
}

/// Binds a sealed channel URL to its channel, so a sealed value copied to
/// another row does not open.
fn channel_aad(id: ChannelId) -> String {
    format!("notification-channel:{id}")
}

#[async_trait]
impl ChannelSecrets for VaultSecrets {
    fn seal_url(&self, channel: ChannelId, url: &str) -> DomainResult<String> {
        self.envelope.seal(url.as_bytes(), &channel_aad(channel)).map_err(DomainError::internal)
    }

    fn open_url(&self, channel: ChannelId, sealed: &str) -> DomainResult<String> {
        let plain = self.envelope.open(sealed, &channel_aad(channel)).map_err(DomainError::internal)?;
        String::from_utf8(plain).map_err(|_| DomainError::Internal("stored channel URL is not text".into()))
    }

    async fn signing_secret(&self, workspace: WorkspaceId, item: VaultItemId) -> DomainResult<Vec<u8>> {
        match self.vault.get_vault_item(item).await? {
            // An item of another workspace is as good as missing.
            Some(i) if i.workspace_id == workspace => {
                self.envelope.open(&i.sealed, &crate::jobs::storage::vault_aad(item)).map_err(DomainError::internal)
            }
            _ => Err(DomainError::NotFound("vault item".into())),
        }
    }
}

/// Names of nodes and services, for the text of a message.
#[derive(Debug, Clone)]
pub struct StoreNames {
    pub servers: Arc<dyn ServerRepository>,
    pub apps: Arc<dyn ServiceRepository>,
}

#[async_trait]
impl ResourceNames for StoreNames {
    async fn name(&self, resource: &EntityRef) -> Option<String> {
        // A name is decoration: a resource that cannot be read (deleted, or
        // a store error) is shown by its id instead.
        match resource.kind {
            EntityKind::Server => {
                let s = self.servers.get_server(resource.id.parse().ok()?).await.ok()?;
                Some(s.display_name.unwrap_or(s.hostname))
            }
            EntityKind::Service => {
                let a = self.apps.get_service(resource.id.parse().ok()?).await.ok()??;
                Some(a.name)
            }
            _ => None,
        }
    }
}

/// The target rules the operator configured.
#[must_use]
pub const fn target_rules(alerts: &AlertSettings) -> TargetRules {
    TargetRules {
        allow_http: alerts.webhook_allow_http,
        allow_private: alerts.webhook_allow_private,
        allow_loopback: alerts.webhook_allow_loopback,
    }
}

/// Delivery settings from the server's configuration.
#[must_use]
pub fn notify_config(alerts: &AlertSettings, public_url: forge_core::public_address::PublicAddress) -> NotifyConfig {
    NotifyConfig {
        rules: target_rules(alerts),
        max_attempts: alerts.delivery_attempts,
        retry: Backoff::new(
            Duration::from_secs(alerts.delivery_retry_base_secs),
            Duration::from_secs(alerts.delivery_retry_max_secs),
        ),
        lease: LEASE,
        public_url,
        ..NotifyConfig::default()
    }
}

/// One pass: plan what the outbox holds, then send what is due.
async fn pass(notifier: &Notifier) {
    match notifier.plan().await {
        Ok(r) if r.deliveries + r.reminders + r.after_silence + r.failed > 0 => {
            tracing::info!(
                transitions = r.transitions,
                deliveries = r.deliveries,
                reminders = r.reminders,
                after_silence = r.after_silence,
                failed = r.failed,
                abandoned = r.abandoned,
                "notifications planned"
            );
        }
        Ok(_) => {}
        Err(e) => tracing::warn!(error = %e, "notification planning failed"),
    }
    match notifier.dispatch().await {
        Ok(r) if r.sent + r.retried + r.failed + r.cancelled + r.suppressed > 0 => {
            tracing::info!(
                sent = r.sent,
                retried = r.retried,
                failed = r.failed,
                cancelled = r.cancelled,
                suppressed = r.suppressed,
                "notifications dispatched"
            );
        }
        Ok(_) => {}
        Err(e) => tracing::warn!(error = %e, "notification dispatch failed"),
    }
}

/// Background loop.
pub(crate) async fn run(state: AppState, mut shutdown: watch::Receiver<bool>) {
    let mut tick = tokio::time::interval(Duration::from_millis(state.config.alerts.deliver_interval_ms));
    let mut prune = tokio::time::interval(PRUNE_EVERY);
    loop {
        tokio::select! {
            _ = tick.tick() => pass(&state.notifier).await,
            _ = prune.tick() => match state.notifier.prune().await {
                Ok(0) => {}
                Ok(n) => tracing::info!(deliveries = n, "old notification deliveries pruned"),
                Err(e) => tracing::warn!(error = %e, "pruning notification deliveries failed"),
            },
            _ = shutdown.changed() => break,
        }
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]
    use reqwest::dns::Resolve;

    use super::*;

    const LAB: TargetRules = TargetRules { allow_http: true, allow_private: true, allow_loopback: true };

    #[tokio::test]
    async fn the_resolver_refuses_names_that_reach_forbidden_addresses() {
        let name = || "localhost".parse::<reqwest::dns::Name>().unwrap();
        let strict = GuardedResolver { rules: TargetRules::default() };
        let err = strict.resolve(name()).await.err().unwrap();
        assert!(err.is::<Blocked>(), "{err}");
        let lab = GuardedResolver { rules: LAB };
        assert!(lab.resolve(name()).await.unwrap().all(|a| a.ip().is_loopback()));
    }

    #[tokio::test]
    async fn literal_addresses_and_plain_http_are_checked_before_any_connection() {
        let strict = HttpTransport::new(TargetRules::default()).unwrap();
        let to = |url: &str| Outbound { url: url.parse().unwrap(), headers: vec![], body: b"{}".to_vec() };
        for url in [
            "https://169.254.169.254/latest/meta-data/",
            "https://127.0.0.1:1/x",
            "https://[::1]:1/x",
            "https://10.0.0.5/x",
            "https://[::ffff:169.254.169.254]/x",
            "http://example.com/x",
        ] {
            assert_eq!(strict.send(&to(url)).await, SendOutcome::Failed(FailureClass::BlockedTarget), "{url}");
        }
        // A name that resolves to loopback is blocked by the resolver.
        assert_eq!(
            strict.send(&to("https://localhost:1/x")).await,
            SendOutcome::Failed(FailureClass::BlockedTarget),
            "the cause survives the HTTP client's error"
        );
        // Allowed, and nothing listens: a connection failure, not a block.
        let lab = HttpTransport::new(LAB).unwrap();
        let free = std::net::TcpListener::bind("127.0.0.1:0").unwrap().local_addr().unwrap().port();
        assert_eq!(
            lab.send(&to(&format!("http://127.0.0.1:{free}/x"))).await,
            SendOutcome::Failed(FailureClass::Connect)
        );
    }

    #[test]
    fn a_sealed_url_opens_only_for_its_channel() {
        let dir = tempfile::tempdir().unwrap();
        let key = forge_secrets::LocalKeyProvider::load_or_create(&dir.path().join("master.key")).unwrap();
        #[derive(Debug)]
        struct NoVault;
        #[async_trait]
        impl VaultRepository for NoVault {
            async fn put_vault_item(&self, _: &forge_core::model::VaultItem) -> DomainResult<()> {
                Ok(())
            }
            async fn get_vault_item(&self, _: VaultItemId) -> DomainResult<Option<forge_core::model::VaultItem>> {
                Ok(None)
            }
            async fn list_vault_items(&self, _: WorkspaceId) -> DomainResult<Vec<forge_core::model::VaultItem>> {
                Ok(Vec::new())
            }
            async fn delete_vault_item(&self, _: VaultItemId) -> DomainResult<()> {
                Ok(())
            }
        }
        let s = VaultSecrets { vault: Arc::new(NoVault), envelope: Envelope::new(Arc::new(key)) };
        let (a, b) = (ChannelId::new(), ChannelId::new());
        let sealed = s.seal_url(a, "https://hooks.example.com/t0ken").unwrap();
        assert!(!sealed.contains("t0ken") && !sealed.contains("hooks.example.com"));
        assert_eq!(s.open_url(a, &sealed).unwrap(), "https://hooks.example.com/t0ken");
        assert!(s.open_url(b, &sealed).is_err());
    }
}
