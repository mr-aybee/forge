//! Policy recipients (D-117): people emailed by user and by role, resolved
//! when a delivery is planned and kept with it.

use forge_core::model::{AgentStatus, Environment, NodeRole, RoleBinding, Server, User};
use forge_core::notifications::{RecipientSkip, SkippedRecipient};
use forge_core::ports::{RoleBindingRepository, ServerRepository, TenancyRepository, UserRepository};
use forge_core::silences::{Silence, SilenceRepository, SilenceScope};
use forge_core::{EnvironmentId, Role, RoleBindingId, ServerId, SilenceId};

use super::*;

fn email_channel_input() -> ChannelInput {
    ChannelInput {
        name: "Mail".into(),
        kind: ChannelKind::Email,
        enabled: true,
        url: None,
        secret_item: None,
        email: Some(EmailInput {
            smtp_host: "smtp.example.com".into(),
            smtp_port: None,
            tls: SmtpTls::Starttls,
            username: None,
            password: None,
            from: "forge@example.com".into(),
            to: vec!["team@example.com".into()],
            subject_prefix: None,
        }),
    }
}

impl Env {
    async fn mail_channel(&self) -> Channel {
        self.n.create_channel(self.ws, email_channel_input(), self.who.clone()).await.unwrap()
    }

    /// A user with an optional address, allowed to see alarms or not, with
    /// `role` bound on `scope` (default: the workspace).
    async fn person(&self, email: Option<&str>, allowed: bool, role: Role, scope: Option<EntityRef>) -> UserId {
        self.account(email, allowed, false, role, scope).await
    }

    async fn account(
        &self,
        email: Option<&str>,
        allowed: bool,
        disabled: bool,
        role: Role,
        scope: Option<EntityRef>,
    ) -> UserId {
        let u = User {
            id: UserId::new(),
            username: Some(format!("u-{}", UserId::new())),
            email: email.map(str::to_owned),
            display_name: "Someone".into(),
            password_hash: None,
            totp_secret_sealed: None,
            oidc_subject: None,
            disabled,
            created_at: self.now(),
        };
        self.store.create_user(&u).await.unwrap();
        let scope = scope.unwrap_or_else(|| EntityRef::new(EntityKind::Workspace, self.ws));
        let b = RoleBinding { id: RoleBindingId::new(), user_id: u.id, role, scope, created_at: self.now() };
        self.store.create_binding(&b).await.unwrap();
        if allowed {
            self.access.allow(u.id);
        }
        u.id
    }

    async fn environment(&self, name: &str) -> EntityRef {
        let ws = forge_core::model::Workspace {
            id: self.ws,
            name: "Acme".into(),
            slug: "acme".into(),
            created_at: Timestamp(1),
        };
        // The first call creates the workspace; later ones find it.
        self.store.create_workspace(&ws).await.ok();
        let ns = Environment {
            id: EnvironmentId::new(),
            workspace_id: self.ws,
            cluster_id: None,
            project_id: None,
            name: name.into(),
            protected: false,
            created_at: Timestamp(1),
        };
        self.store.create_environment(&ns).await.unwrap();
        EntityRef::new(EntityKind::Environment, ns.id)
    }

    async fn server_in(&self, ns: &EntityRef) -> EntityRef {
        let s = Server {
            id: ServerId::new(),
            workspace_id: self.ws,
            environment_id: Some(ns.id.parse().unwrap()),
            cluster_id: None,
            role: NodeRole::Worker,
            display_name: None,
            hostname: format!("node-{}", &ServerId::new().to_string()[..8]),
            os: "linux".into(),
            arch: "x86_64".into(),
            agent_version: "0".into(),
            status: AgentStatus::Online,
            last_seen_at: None,
            cert_serial: None,
            cert_not_after: None,
            created_at: Timestamp(1),
        };
        self.store.create_server(&s).await.unwrap();
        EntityRef::new(EntityKind::Server, s.id)
    }

    async fn people_policy(&self, mail: &Channel, users: Vec<UserId>, roles: Vec<Role>) -> NotificationPolicy {
        let mut input = self.policy_input(&[]);
        input.email_channel = Some(mail.id);
        input.email_users = users;
        input.email_roles = roles;
        self.n.create_policy(self.ws, input, self.who.clone()).await.unwrap()
    }

    /// The deliveries to people, oldest first.
    async fn people(&self) -> Vec<Delivery> {
        self.deliveries().await.into_iter().filter(|d| d.recipients.is_some()).collect()
    }

    fn bcc(&self) -> Vec<Vec<String>> {
        self.mailer.mails().into_iter().map(|m| m.bcc).collect()
    }
}

fn skipped(d: &Delivery) -> Vec<SkippedRecipient> {
    let mut s = d.recipients.clone().unwrap().skipped;
    s.sort_by_key(|x| x.user_id);
    s
}

#[tokio::test]
async fn named_users_are_emailed_only_with_access_an_address_and_an_enabled_account() {
    let e = env().await;
    let mail = e.mail_channel().await;
    let ok = e.person(Some("ok@example.com"), true, Role::Customer, None).await;
    let blind = e.person(Some("blind@example.com"), false, Role::Customer, None).await;
    let nomail = e.person(None, true, Role::Customer, None).await;
    let off = e.account(Some("off@example.com"), true, true, Role::Customer, None).await;
    let p = e.people_policy(&mail, vec![ok, blind, nomail, off], vec![]).await;
    assert_eq!(e.n.policy_warnings(&p).await.unwrap(), vec![nomail], "the editor warns about a user without email");

    e.fire(Severity::Major).await;
    e.n.plan().await.unwrap();
    e.n.dispatch().await.unwrap();

    assert_eq!(e.bcc(), vec![vec!["ok@example.com".to_owned()]], "never the channel's own `to`, never a skipped user");
    assert!(e.mailer.mails()[0].to.is_empty());
    let d = e.people().await.remove(0);
    assert_eq!(d.status, DeliveryStatus::Sent);
    assert_eq!(d.recipients.as_ref().unwrap().count, 1);
    let mut want = vec![
        SkippedRecipient { user_id: blind, reason: RecipientSkip::NoAccess },
        SkippedRecipient { user_id: nomail, reason: RecipientSkip::NoEmail },
        SkippedRecipient { user_id: off, reason: RecipientSkip::Disabled },
    ];
    want.sort_by_key(|x| x.user_id);
    assert_eq!(skipped(&d), want);
    let json = serde_json::to_string(&d).unwrap();
    assert!(!json.contains("blind@") && !json.contains("off@"), "no address of a skipped user: {json}");
}

#[tokio::test]
async fn roles_reach_holders_on_the_resource_and_its_ancestors_but_not_siblings() {
    let e = env().await;
    let mail = e.mail_channel().await;
    let prod = e.environment("prod").await;
    let staging = e.environment("staging").await;
    let node = e.server_in(&prod).await;
    let on_env = e.person(Some("env@example.com"), true, Role::Operator, Some(prod.clone())).await;
    let on_ws = e.person(Some("ws@example.com"), true, Role::Operator, None).await;
    let on_node = e.person(Some("node@example.com"), true, Role::Operator, Some(node.clone())).await;
    let _sibling = e.person(Some("sibling@example.com"), true, Role::Operator, Some(staging)).await;
    let _other_role = e.person(Some("dev@example.com"), true, Role::Developer, Some(prod)).await;
    e.people_policy(&mail, vec![], vec![Role::Operator]).await;

    e.fire_at(Severity::Critical, node).await;
    e.n.plan().await.unwrap();
    e.n.dispatch().await.unwrap();

    let mut got = e.bcc().remove(0);
    got.sort();
    assert_eq!(got, ["env@example.com", "node@example.com", "ws@example.com"]);
    let d = e.people().await.remove(0);
    assert!(d.recipients.unwrap().skipped.is_empty());
    let _ = (on_env, on_ws, on_node);
}

#[tokio::test]
async fn people_are_deduplicated_by_address() {
    let e = env().await;
    let mail = e.mail_channel().await;
    let a = e.person(Some("same@example.com"), true, Role::Operator, None).await;
    // A second binding of the same person on the node's environment.
    e.store
        .create_binding(&RoleBinding {
            id: RoleBindingId::new(),
            user_id: a,
            role: Role::Operator,
            scope: EntityRef::new(EntityKind::Workspace, e.ws),
            created_at: e.now(),
        })
        .await
        .ok();
    // Named and holding the role (twice): one address.
    e.people_policy(&mail, vec![a], vec![Role::Operator]).await;
    e.fire(Severity::Major).await;
    e.n.plan().await.unwrap();
    e.n.dispatch().await.unwrap();
    assert_eq!(e.bcc().remove(0).len(), 1);
    assert_eq!(e.people().await[0].recipients.as_ref().unwrap().count, 1);
}

#[tokio::test]
async fn nobody_to_email_records_a_cancelled_delivery() {
    let e = env().await;
    let mail = e.mail_channel().await;
    let blind = e.person(Some("blind@example.com"), false, Role::Operator, None).await;
    e.people_policy(&mail, vec![blind], vec![]).await;
    e.fire(Severity::Major).await;
    assert_eq!(e.n.plan().await.unwrap().deliveries, 1);
    e.n.dispatch().await.unwrap();
    assert!(e.mailer.mails().is_empty());
    let d = e.only().await;
    assert_eq!((d.status, d.detail.as_deref()), (DeliveryStatus::Cancelled, Some(NO_RECIPIENTS)));
    assert!(d.finished_at.is_some());
    assert_eq!(skipped(&d), [SkippedRecipient { user_id: blind, reason: RecipientSkip::NoAccess }]);
}

#[tokio::test]
async fn a_retry_sends_to_the_addresses_planned_with_the_delivery() {
    let e = env().await;
    let mail = e.mail_channel().await;
    let u = e.person(Some("first@example.com"), true, Role::Operator, None).await;
    e.people_policy(&mail, vec![u], vec![]).await;
    e.fire(Severity::Major).await;
    e.mailer.script(&[SendOutcome::Smtp(451)]);
    e.n.plan().await.unwrap();
    assert_eq!(e.n.dispatch().await.unwrap().retried, 1);
    e.store.update_user_profile(u, None, Some("second@example.com"), "Someone").await.unwrap();
    e.advance(30 * MIN);
    e.n.plan().await.unwrap();
    assert_eq!(e.n.dispatch().await.unwrap().sent, 1);
    assert_eq!(e.bcc(), vec![vec!["first@example.com".to_owned()]; 2], "the snapshot, not the new address");
}

#[tokio::test]
async fn a_silence_holds_back_messages_to_people() {
    let e = env().await;
    let mail = e.mail_channel().await;
    let u = e.person(Some("p@example.com"), true, Role::Operator, None).await;
    e.people_policy(&mail, vec![u], vec![]).await;
    let s = Silence {
        id: SilenceId::new(),
        workspace_id: e.ws,
        scope: SilenceScope::Workspace,
        rules: vec![],
        min_severity: None,
        max_severity: None,
        starts_at: e.now(),
        ends_at: Timestamp(e.now().0 + 60 * MIN),
        reason: "Upgrade".into(),
        created_by: e.who.clone(),
        created_at: e.now(),
        updated_at: e.now(),
        ended_by: None,
    };
    e.store.create_silence(&s).await.unwrap();
    e.fire(Severity::Major).await;
    e.n.plan().await.unwrap();
    assert_eq!(e.n.dispatch().await.unwrap().suppressed, 1);
    assert!(e.mailer.mails().is_empty());

    // After the silence: one message to the same people.
    e.advance(61 * MIN);
    e.n.plan().await.unwrap();
    assert_eq!(e.n.dispatch().await.unwrap().sent, 1);
    assert_eq!(e.bcc(), vec![vec!["p@example.com".to_owned()]]);
    let after = e.people().await.into_iter().find(|d| d.after_silence).unwrap();
    assert_eq!(after.recipients.unwrap().to, ["p@example.com"]);
}

#[tokio::test]
async fn the_same_channel_as_a_channel_and_for_people_sends_two_messages() {
    let e = env().await;
    let mail = e.mail_channel().await;
    let u = e.person(Some("p@example.com"), true, Role::Operator, None).await;
    let mut input = e.policy_input(&[&mail]);
    input.email_channel = Some(mail.id);
    input.email_users = vec![u];
    e.n.create_policy(e.ws, input, e.who.clone()).await.unwrap();
    let a = e.fire(Severity::Major).await;
    e.n.plan().await.unwrap();
    assert_eq!(e.n.dispatch().await.unwrap().sent, 2);
    let mails = e.mailer.mails();
    assert!(mails.iter().any(|m| m.to == ["team@example.com"] && m.bcc.is_empty()));
    assert!(mails.iter().any(|m| m.to.is_empty() && m.bcc == ["p@example.com"]));

    // The recovery goes to both, each where its firing message arrived.
    e.resolve(&a).await;
    e.n.plan().await.unwrap();
    assert_eq!(e.n.dispatch().await.unwrap().sent, 2);
}

#[tokio::test]
async fn policy_people_are_checked() {
    let e = env().await;
    let mail = e.mail_channel().await;
    let hook = e.webhook("ops", false).await;
    let u = e.person(Some("p@example.com"), true, Role::Operator, None).await;
    let bad =
        |m: &str, err: DomainError| assert!(matches!(&err, DomainError::Invalid(t) if t.contains(m)), "{m}: {err}");

    let mut i = e.policy_input(&[]);
    i.email_users = vec![u];
    bad("email_channel: required", e.n.create_policy(e.ws, i.clone(), e.who.clone()).await.unwrap_err());
    i.email_channel = Some(hook.id);
    bad("not an email channel", e.n.create_policy(e.ws, i.clone(), e.who.clone()).await.unwrap_err());
    i.email_channel = Some(mail.id);
    i.email_users = vec![UserId::new()];
    bad("email_users: no user", e.n.create_policy(e.ws, i.clone(), e.who.clone()).await.unwrap_err());
    let stranger = e
        .person(
            Some("x@example.com"),
            true,
            Role::Operator,
            Some(EntityRef::new(EntityKind::Workspace, WorkspaceId::new())),
        )
        .await;
    i.email_users = vec![stranger];
    bad("no role in this workspace", e.n.create_policy(e.ws, i.clone(), e.who.clone()).await.unwrap_err());
    i.email_users = (0..=MAX_POLICY_EMAIL_USERS).map(|_| UserId::new()).collect();
    bad("email_users: at most", e.n.create_policy(e.ws, i.clone(), e.who.clone()).await.unwrap_err());
    i.email_users = vec![];
    bad("channels: 1 to", e.n.create_policy(e.ws, i.clone(), e.who.clone()).await.unwrap_err());

    // Users and roles without channels are a whole policy; the email
    // channel cannot be deleted while it sends for one.
    i.email_users = vec![u, u];
    i.email_roles = vec![Role::Operator, Role::Operator];
    let p = e.n.create_policy(e.ws, i, e.who.clone()).await.unwrap();
    assert_eq!((p.email_users.len(), p.email_roles.len(), p.channels.len()), (1, 1, 0));
    assert_eq!(e.n.policy(p.id).await.unwrap(), p, "round trip");
    assert!(matches!(e.n.delete_channel(mail.id).await, Err(DomainError::Conflict(_))));
}
