//! Email channels (D-113): settings checks, the password kept sealed and
//! write-only, one email per firing, and SMTP replies classified for retry.

use super::*;

const PASSWORD: &str = "Sm7p-Pa55word";

fn email_input(username: Option<&str>, password: Option<&str>) -> EmailInput {
    EmailInput {
        smtp_host: "SMTP.Example.com".into(),
        smtp_port: None,
        tls: SmtpTls::Starttls,
        username: username.map(str::to_owned),
        password: password.map(str::to_owned),
        from: "forge@example.com".into(),
        to: vec!["ops@example.com".into(), "oncall@example.com".into(), "ops@example.com".into()],
        subject_prefix: Some(" [prod] ".into()),
    }
}

fn channel_input(email: EmailInput) -> ChannelInput {
    ChannelInput {
        name: "Ops mail".into(),
        kind: ChannelKind::Email,
        enabled: true,
        url: None,
        secret_item: None,
        email: Some(email),
    }
}

impl Env {
    async fn email(&self) -> Channel {
        let input = channel_input(email_input(Some("forge"), Some(PASSWORD)));
        self.n.create_channel(self.ws, input, self.who.clone()).await.unwrap()
    }
}

#[tokio::test]
async fn an_email_channel_keeps_its_password_sealed_and_shows_the_rest() {
    let e = env().await;
    let c = e.email().await;
    assert_eq!(c.target, "smtp://smtp.example.com:587");
    let s = c.email.clone().unwrap();
    assert_eq!((s.smtp_host.as_str(), s.smtp_port, s.tls), ("smtp.example.com", 587, SmtpTls::Starttls));
    assert_eq!(s.to, ["ops@example.com", "oncall@example.com"], "duplicates dropped, order kept");
    assert_eq!(s.subject_prefix.as_deref(), Some("[prod]"));
    assert!(s.password_set);
    let json = serde_json::to_string(&c).unwrap();
    assert!(!json.contains(PASSWORD) && !format!("{c:?}").contains(PASSWORD), "{json}");
    assert_eq!(e.secrets.open_url(c.id, &c.sealed_url).unwrap(), PASSWORD);

    // An update without a password keeps the stored one for the same
    // server, port, TLS mode and user.
    let mut keep = email_input(Some("forge"), None);
    keep.from = "alerts@example.com".into();
    let u = e.n.update_channel(c.id, channel_input(keep)).await.unwrap();
    assert_eq!(e.secrets.open_url(c.id, &u.sealed_url).unwrap(), PASSWORD);

    // A different host, port, TLS mode or user needs the password again, so
    // the stored one is never sent to a server it was not set for.
    let changes: [&dyn Fn(&mut EmailInput); 4] = [
        &|i| i.smtp_host = "smtp.attacker.example".into(),
        &|i| i.smtp_port = Some(2525),
        &|i| i.tls = SmtpTls::Tls,
        &|i| i.username = Some("other".into()),
    ];

    for change in changes {
        let mut i = email_input(Some("forge"), None);
        change(&mut i);
        let err = e.n.update_channel(c.id, channel_input(i)).await.unwrap_err();
        assert!(err.to_string().contains("password: required"), "{err}");
    }

    assert_eq!(e.secrets.open_url(c.id, &e.n.channel(c.id).await.unwrap().sealed_url).unwrap(), PASSWORD);
    let mut moved = email_input(Some("forge"), Some(PASSWORD));
    moved.smtp_port = Some(2525);
    let u = e.n.update_channel(c.id, channel_input(moved)).await.unwrap();
    assert_eq!(u.target, "smtp://smtp.example.com:2525");

    // Without a user name no password is kept.
    let anon = e.n.update_channel(c.id, channel_input(email_input(None, None))).await.unwrap();
    assert!(!anon.email.unwrap().password_set);
    assert_eq!(e.secrets.open_url(c.id, &anon.sealed_url).unwrap(), "");

    // ... and adding a user name back needs the password again.
    let err = e.n.update_channel(c.id, channel_input(email_input(Some("forge"), None))).await.unwrap_err();
    assert!(err.to_string().contains("password: required"), "{err}");

    // A sealed password never becomes a webhook URL.
    let to_webhook = ChannelInput { kind: ChannelKind::Webhook, email: None, ..channel_input(email_input(None, None)) };
    let err = e.n.update_channel(c.id, to_webhook).await.unwrap_err();
    assert!(err.to_string().contains("url: required"), "{err}");
}

#[tokio::test]
async fn email_settings_are_checked() {
    let mut e = env().await;
    let refused = |m: &str, f: &dyn Fn(&mut EmailInput)| {
        let mut i = email_input(Some("forge"), Some(PASSWORD));
        f(&mut i);
        (m.to_owned(), channel_input(i))
    };
    let cases = [
        refused("smtp_host", &|i| i.smtp_host = "169.254.169.254".into()),
        refused("smtp_host", &|i| i.smtp_host = "10.0.0.5".into()),
        refused("smtp_host", &|i| i.smtp_host = "smtp.example.com/x".into()),
        refused("smtp_port", &|i| i.smtp_port = Some(0)),
        refused("tls", &|i| i.tls = SmtpTls::None),
        refused("from", &|i| i.from = "not an address".into()),
        refused("to", &|i| i.to.clear()),
        refused("to", &|i| i.to = (0..21).map(|n| format!("u{n}@example.com")).collect()),
        refused("to", &|i| i.to = vec!["nobody".into()]),
        refused("subject_prefix", &|i| i.subject_prefix = Some("x".repeat(41))),
        refused("password", &|i| i.password = Some("a\r\nb".into())),
        refused("password", &|i| i.password = None),
        refused("password", &|i| i.username = None),
    ];

    for (field, input) in cases {
        let err = e.n.create_channel(e.ws, input, e.who.clone()).await.unwrap_err();
        assert!(
            err.to_string().starts_with(&format!("invalid: {field}")) || err.to_string().contains(&field),
            "{field}: {err}"
        );
    }

    let mut no_email = channel_input(email_input(None, None));
    no_email.email = None;
    assert!(e.n.create_channel(e.ws, no_email, e.who.clone()).await.is_err());
    let mut with_url = channel_input(email_input(None, None));
    with_url.url = Some("https://hooks.example.com/x".into());
    assert!(e.n.create_channel(e.ws, with_url, e.who.clone()).await.is_err());

    // Where the operator allows unencrypted targets, `none` works without a user.
    e.n.config.rules.allow_http = true;
    let mut plain = email_input(None, None);
    plain.tls = SmtpTls::None;
    let c = e.n.create_channel(e.ws, channel_input(plain.clone()), e.who.clone()).await.unwrap();
    assert_eq!(c.target, "smtp://smtp.example.com:25");
    plain.username = Some("forge".into());
    plain.password = Some(PASSWORD.into());
    let err = e.n.create_channel(e.ws, channel_input(plain), e.who.clone()).await.unwrap_err();
    assert!(err.to_string().contains("never sent without TLS"), "{err}");
}

#[tokio::test]
async fn a_fired_alarm_sends_one_email_with_the_facts() {
    let e = env().await;
    let c = e.email().await;
    e.policy(&[&c]).await;
    let a = e.fire(Severity::Critical).await;
    e.n.plan().await.unwrap();
    assert_eq!(e.n.dispatch().await.unwrap().sent, 1);
    e.n.plan().await.unwrap();
    assert_eq!(e.n.dispatch().await.unwrap(), DispatchReport::default());
    let mails = e.mailer.mails();
    assert_eq!(mails.len(), 1, "one email for one firing");
    let m = &mails[0];
    assert_eq!(m.subject, "[prod] [FIRING][critical] Node offline \u{2014} web-01");
    assert_eq!((m.username.as_deref(), m.password.as_deref()), (Some("forge"), Some(PASSWORD)));
    assert_eq!(m.to.len(), 2);
    assert!(m.body.contains(&format!("https://forge.example.com/alerts?alarm={}", a.id)), "{}", m.body);
    assert!(!m.body.contains(PASSWORD) && !format!("{m:?}").contains(PASSWORD));
    let d = e.only().await;
    assert_eq!((d.status, d.last_http_status, m.delivery_id), (DeliveryStatus::Sent, Some(250), d.id));

    e.resolve(&a).await;
    e.n.plan().await.unwrap();
    assert_eq!(e.n.dispatch().await.unwrap().sent, 1);
    assert!(e.mailer.mails()[1].subject.starts_with("[prod] [RESOLVED][critical]"));
}

#[tokio::test]
async fn smtp_4xx_is_retried_and_5xx_fails_at_once() {
    let e = env().await;
    let c = e.email().await;
    e.policy(&[&c]).await;
    e.fire(Severity::Major).await;
    e.mailer.script(&[SendOutcome::Smtp(451), SendOutcome::Failed(FailureClass::Connect)]);
    e.n.plan().await.unwrap();
    assert_eq!(e.n.dispatch().await.unwrap().retried, 1);
    let d = e.only().await;
    assert_eq!(
        (d.status, d.last_error, d.last_http_status),
        (DeliveryStatus::Pending, Some(FailureClass::SmtpReply), Some(451))
    );
    e.advance(d.next_attempt_at.0 - e.now().0);
    assert_eq!(e.n.dispatch().await.unwrap().retried, 1, "a connection failure is retried");
    let d = e.only().await;
    e.advance(d.next_attempt_at.0 - e.now().0);
    assert_eq!(e.n.dispatch().await.unwrap().sent, 1);
    assert_eq!(e.mailer.mails().len(), 3);

    let e = env().await;
    let c = e.email().await;
    e.policy(&[&c]).await;
    e.fire(Severity::Major).await;
    e.mailer.script(&[SendOutcome::Smtp(550)]);
    e.n.plan().await.unwrap();
    assert_eq!(e.n.dispatch().await.unwrap().failed, 1);
    let d = e.only().await;
    assert_eq!(
        (d.status, d.attempts, d.last_error, d.last_http_status),
        (DeliveryStatus::Failed, 1, Some(FailureClass::SmtpReply), Some(550))
    );

    for c in [421, 450, 451, 452, 499] {
        assert!(retryable_smtp(c), "{c}");
    }

    for c in [500, 535, 550, 552, 554, 250] {
        assert!(!retryable_smtp(c), "{c}");
    }
}

#[tokio::test]
async fn a_test_message_goes_out_by_email() {
    let e = env().await;
    let c = e.email().await;
    let (d, attempts) = e.n.test_channel(c.id).await.unwrap();
    assert_eq!((d.status, attempts.len()), (DeliveryStatus::Sent, 1));
    let m = e.mailer.mails().pop().unwrap();
    assert_eq!(m.subject, "[prod] [TEST] Test notification from Orbyn Forge");
    assert_eq!(e.receiver.count(), 0, "nothing goes over HTTP");
}
