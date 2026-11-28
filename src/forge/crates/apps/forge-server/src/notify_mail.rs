//! Email notifications (#11, D-113): the SMTP transport behind
//! `forge_alerts::notify::MailTransport`, built on `lettre` (rustls on
//! `ring`, native roots; no OpenSSL).
//!
//! Like the HTTP transport, it is the only place an email leaves Forge. It
//! resolves the SMTP host itself and refuses it when **any** address is one
//! the target rules do not allow, then connects to a checked address while
//! the TLS name stays the host name, so a name cannot be re-pointed between
//! the check and the connection. STARTTLS is required, never opportunistic;
//! certificates are verified; every SMTP command and the whole send are
//! bounded in time. A failure is recorded as a class and an SMTP reply code,
//! never as the server's text: it can quote the user name.
//!
//! A message with an HTML part (D-119) is `multipart/alternative`: the plain
//! text first, then `multipart/related` with the HTML and the brand mark as
//! an inline PNG (`Content-ID: <forge-mark@orbyn-forge>`). The mark is
//! compiled in; nothing is fetched from elsewhere.

use std::net::{IpAddr, SocketAddr};
use std::time::{Duration, SystemTime};

use async_trait::async_trait;
use forge_alerts::notify::email::MARK_CID;
use forge_alerts::notify::target::{TargetRules, address_allowed};
use forge_alerts::notify::{MailTransport, OutboundMail, SendOutcome};
use forge_core::notifications::{FailureClass, SmtpTls};
use forge_core::{DomainError, DomainResult};
use lettre::message::header::{ContentType, Header, HeaderName, HeaderValue};
use lettre::message::{Attachment, Mailbox, Message, MultiPart, SinglePart};
use lettre::transport::smtp::authentication::Credentials;
use lettre::transport::smtp::client::{Tls, TlsParameters};
use lettre::transport::smtp::extension::ClientId;
use lettre::{AsyncSmtpTransport, AsyncTransport, Tokio1Executor};

/// Time for one SMTP command (connecting included).
const COMMAND_TIMEOUT: Duration = Duration::from_secs(10);
/// Time for a whole send; shorter than the delivery lease (60 s).
const SEND_TIMEOUT: Duration = Duration::from_secs(40);
/// The `Message-ID` domain when the console has no public host name.
const FALLBACK_DOMAIN: &str = "orbyn-forge.invalid";

/// The brand mark (128 px, shown at 32 px), rendered once from
/// `branding/forge-mark.svg`: mail clients do not show SVG.
const MARK_PNG: &[u8] = include_bytes!("../assets/email/forge-mark-128.png");

/// `Auto-Submitted: auto-generated` (RFC 3834): out-of-office and other
/// automatic replies are not sent back to the sender address.
#[derive(Debug, Clone, Copy)]
struct AutoSubmitted;

impl Header for AutoSubmitted {
    fn name() -> HeaderName {
        HeaderName::new_from_ascii_str("Auto-Submitted")
    }

    fn parse(_: &str) -> Result<Self, Box<dyn std::error::Error + Send + Sync>> {
        Ok(Self)
    }

    fn display(&self) -> HeaderValue {
        HeaderValue::new(Self::name(), "auto-generated".to_owned())
    }
}

/// Sends notification emails over SMTP.
#[derive(Debug, Clone)]
pub struct SmtpMailer {
    rules: TargetRules,
    /// The `EHLO` name: the console's public host, or `localhost`.
    hello: ClientId,
    /// The right-hand side of `Message-ID`.
    id_domain: String,
}

impl SmtpMailer {
    /// `public_url` is the console's URL; its host names this server in
    /// `EHLO` and in `Message-ID`.
    #[must_use]
    pub fn new(rules: TargetRules, public_url: &str) -> Self {
        let host = url::Url::parse(public_url).ok().and_then(|u| u.host().map(|h| h.to_owned()));
        let (hello, id_domain) = match host {
            Some(url::Host::Domain(d)) if !d.is_empty() => (ClientId::Domain(d.clone()), d),
            Some(url::Host::Ipv4(a)) => (ClientId::Ipv4(a), FALLBACK_DOMAIN.to_owned()),
            Some(url::Host::Ipv6(a)) => (ClientId::Ipv6(a), FALLBACK_DOMAIN.to_owned()),
            _ => (ClientId::Domain("localhost".to_owned()), FALLBACK_DOMAIN.to_owned()),
        };

        Self { rules, hello, id_domain }
    }

    fn message(&self, m: &OutboundMail) -> Result<Message, lettre::error::Error> {
        let mut b = Message::builder()
            .from(m.from.parse::<Mailbox>().map_err(|_| lettre::error::Error::MissingFrom)?)
            .subject(m.subject.clone())
            .message_id(Some(format!("<{}@{}>", m.delivery_id, self.id_domain)))
            .date(SystemTime::UNIX_EPOCH + Duration::from_millis(u64::try_from(m.at.0).unwrap_or(0)))
            .header(AutoSubmitted);

        for to in &m.to {
            b = b.to(to.parse::<Mailbox>().map_err(|_| lettre::error::Error::MissingTo)?);
        }

        // Envelope recipients only: the header is not written (D-117).
        for bcc in &m.bcc {
            b = b.bcc(bcc.parse::<Mailbox>().map_err(|_| lettre::error::Error::MissingTo)?);
        }

        match &m.html {
            None => b.header(ContentType::TEXT_PLAIN).body(m.body.clone()),
            Some(html) => b.multipart(alternative(&m.body, html)),
        }
    }

    /// The one address to connect to, or why there is none.
    async fn address(&self, m: &OutboundMail) -> Result<IpAddr, FailureClass> {
        let addrs: Vec<SocketAddr> =
            tokio::net::lookup_host((m.host.as_str(), m.port)).await.map_err(|_| FailureClass::Connect)?.collect();

        // A name that answers with a public and an internal address is not
        // one to send to.
        if addrs.iter().any(|a| !address_allowed(a.ip(), self.rules)) {
            return Err(FailureClass::BlockedTarget);
        }

        addrs.first().map(SocketAddr::ip).ok_or(FailureClass::Connect)
    }

    async fn deliver(&self, m: &OutboundMail) -> SendOutcome {
        if m.tls == SmtpTls::None && (!self.rules.allow_http || m.username.is_some()) {
            return SendOutcome::Failed(FailureClass::BlockedTarget);
        }

        let Ok(message) = self.message(m) else { return SendOutcome::Failed(FailureClass::Config) };

        let ip = match self.address(m).await {
            Ok(ip) => ip,
            Err(class) => return SendOutcome::Failed(class),
        };

        let tls = match m.tls {
            SmtpTls::None => Tls::None,
            mode => {
                // Verified against the host name, not the address.
                let Ok(params) = TlsParameters::builder(m.host.clone()).build_rustls() else {
                    return SendOutcome::Failed(FailureClass::Tls);
                };

                if mode == SmtpTls::Tls { Tls::Wrapper(params) } else { Tls::Required(params) }
            }
        };

        let mut builder = AsyncSmtpTransport::<Tokio1Executor>::builder_dangerous(ip.to_string())
            .port(m.port)
            .tls(tls)
            .hello_name(self.hello.clone())
            .timeout(Some(COMMAND_TIMEOUT));

        if let (Some(user), Some(password)) = (&m.username, &m.password) {
            builder = builder.credentials(Credentials::new(user.clone(), password.clone()));
        }

        // One transport per message: each channel has its own server and
        // credentials, and messages are few.
        let transport = builder.build();
        match transport.send(message).await {
            Ok(reply) => SendOutcome::Smtp(u16::from(reply.code())),
            Err(e) => classify(&e),
        }
    }
}

/// Plain text and HTML with its inline brand mark.
fn alternative(text: &str, html: &str) -> MultiPart {
    let png = ContentType::parse("image/png").unwrap_or(ContentType::TEXT_PLAIN);
    MultiPart::alternative().singlepart(SinglePart::plain(text.to_owned())).multipart(
        MultiPart::related()
            .singlepart(SinglePart::html(html.to_owned()))
            .singlepart(Attachment::new_inline(MARK_CID.to_owned()).body(MARK_PNG.to_vec(), png)),
    )
}

/// The outcome of a failed send: the SMTP reply code when the server
/// refused, otherwise a class. Never the error's text.
fn classify(e: &lettre::transport::smtp::Error) -> SendOutcome {
    if let Some(code) = e.status() {
        return SendOutcome::Smtp(u16::from(code));
    }

    SendOutcome::Failed(if e.is_timeout() {
        FailureClass::Timeout
    } else if e.is_tls() || e.is_client() {
        // A client error is a server that does not offer STARTTLS: the
        // message is not sent in the clear instead.
        FailureClass::Tls
    } else {
        FailureClass::Connect
    })
}

#[async_trait]
impl MailTransport for SmtpMailer {
    fn mailbox(&self, raw: &str) -> DomainResult<String> {
        raw.parse::<Mailbox>().map(|m| m.to_string()).map_err(|_| DomainError::Invalid("not an email address".into()))
    }

    async fn send(&self, mail: &OutboundMail) -> SendOutcome {
        tokio::time::timeout(SEND_TIMEOUT, self.deliver(mail))
            .await
            .unwrap_or(SendOutcome::Failed(FailureClass::Timeout))
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]
    use forge_core::{DeliveryId, Timestamp};

    use super::*;

    const LAB: TargetRules = TargetRules { allow_http: true, allow_private: true, allow_loopback: true };

    fn mail(host: &str, port: u16, tls: SmtpTls) -> OutboundMail {
        OutboundMail {
            host: host.into(),
            port,
            tls,
            username: None,
            password: None,
            from: "Forge <forge@example.com>".into(),
            to: vec!["ops@example.com".into()],
            bcc: Vec::new(),
            subject: "[TEST] Test notification from Orbyn Forge".into(),
            body: "hello\n".into(),
            html: None,
            delivery_id: DeliveryId::new(),
            at: Timestamp(1_763_285_400_000),
        }
    }

    #[test]
    fn addresses_are_parsed_with_lettre() {
        let m = SmtpMailer::new(TargetRules::default(), "https://forge.example.com");
        assert_eq!(m.mailbox("ops@example.com").unwrap(), "ops@example.com");
        assert!(m.mailbox("Ops Team <ops@example.com>").unwrap().contains("<ops@example.com>"));

        for bad in ["", "ops", "ops@", "@example.com", "a b@example.com", "ops@example.com\r\nBcc: x@y.z"] {
            assert!(m.mailbox(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn the_message_names_the_delivery_and_asks_for_no_auto_reply() {
        let m = SmtpMailer::new(TargetRules::default(), "https://forge.example.com/");
        let out = mail("smtp.example.com", 587, SmtpTls::Starttls);
        let text = String::from_utf8(m.message(&out).unwrap().formatted()).unwrap();
        assert!(text.contains(&format!("Message-ID: <{}@forge.example.com>", out.delivery_id)), "{text}");
        assert!(text.contains("Auto-Submitted: auto-generated"), "{text}");
        assert!(text.contains("Date: Sun, 16 Nov 2025 09:30:00"), "{text}");
        assert!(matches!(SmtpMailer::new(LAB, "").hello, ClientId::Domain(ref d) if d == "localhost"));
    }

    #[test]
    fn html_mail_is_alternative_with_an_inline_mark() {
        let m = SmtpMailer::new(TargetRules::default(), "https://forge.example.com");
        let mut out = mail("smtp.example.com", 587, SmtpTls::Starttls);
        out.html = Some(format!("<p>hi</p><img src=\"cid:{MARK_CID}\">"));
        let text = String::from_utf8(m.message(&out).unwrap().formatted()).unwrap();
        let alt = text.find("Content-Type: multipart/alternative").unwrap();
        let plain = text.find("Content-Type: text/plain").unwrap();
        let related = text.find("Content-Type: multipart/related").unwrap();
        let html = text.find("Content-Type: text/html").unwrap();
        let png = text.find("Content-Type: image/png").unwrap();
        assert!(alt < plain && plain < related && related < html && html < png, "{text}");
        assert!(text.contains(&format!("Content-ID: <{MARK_CID}>")), "{text}");
        assert!(text.contains("Content-Disposition: inline"), "{text}");
        assert!(text.contains("hello"), "{text}");

        // Plain text only when there is no HTML.
        let plain_only =
            String::from_utf8(m.message(&mail("smtp.example.com", 587, SmtpTls::Tls)).unwrap().formatted()).unwrap();
        assert!(plain_only.contains("Content-Type: text/plain") && !plain_only.contains("multipart"), "{plain_only}");
    }

    #[tokio::test]
    async fn forbidden_hosts_and_plain_text_are_refused_before_any_connection() {
        let strict = SmtpMailer::new(TargetRules::default(), "https://forge.example.com");

        for (host, tls) in [
            ("169.254.169.254", SmtpTls::Tls),
            ("127.0.0.1", SmtpTls::Starttls),
            ("localhost", SmtpTls::Starttls),
            ("10.0.0.5", SmtpTls::Tls),
            ("smtp.example.com", SmtpTls::None),
        ] {
            assert_eq!(
                strict.send(&mail(host, 25, tls)).await,
                SendOutcome::Failed(FailureClass::BlockedTarget),
                "{host}"
            );
        }

        let lab = SmtpMailer::new(LAB, "https://forge.example.com");
        let mut with_user = mail("127.0.0.1", 25, SmtpTls::None);
        with_user.username = Some("forge".into());
        with_user.password = Some("pw".into());
        assert_eq!(lab.send(&with_user).await, SendOutcome::Failed(FailureClass::BlockedTarget));
        let free = std::net::TcpListener::bind("127.0.0.1:0").unwrap().local_addr().unwrap().port();
        assert_eq!(lab.send(&mail("127.0.0.1", free, SmtpTls::None)).await, SendOutcome::Failed(FailureClass::Connect));
    }
}
