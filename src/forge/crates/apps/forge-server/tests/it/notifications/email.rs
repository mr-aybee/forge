//! Email channels (#11, D-113) end to end, against a tiny SMTP server on
//! loopback: a fired alarm becomes exactly one email, a 4xx reply is retried,
//! a 5xx reply fails at once, a test message goes out, and the SMTP password
//! never comes back in an answer, a delivery or the audit log.

use std::collections::VecDeque;

use tokio::io::{AsyncBufReadExt, BufReader};

use super::*;

const SMTP_PASSWORD: &str = "Sm7p-Pa55word-n3ver-shown";

/// One message the fake server took to the end of `DATA`.
#[derive(Debug, Clone)]
struct Mail {
    rcpts: Vec<String>,
    data: String,
    reply: u16,
}

/// Speaks just enough SMTP: `220`, `EHLO` (no STARTTLS, no AUTH), `MAIL`,
/// `RCPT`, `DATA`, `RSET`, `NOOP`, `QUIT`. The reply to the end of `DATA`
/// is the next scripted code, then `always`.
#[derive(Clone)]
struct FakeSmtp {
    port: u16,
    mails: Arc<Mutex<Vec<Mail>>>,
    script: Arc<Mutex<VecDeque<u16>>>,
    always: Arc<Mutex<u16>>,
}

impl FakeSmtp {
    async fn start(script: &[u16], always: u16) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let me = Self {
            port: listener.local_addr().unwrap().port(),
            mails: Arc::default(),
            script: Arc::new(Mutex::new(script.iter().copied().collect())),
            always: Arc::new(Mutex::new(always)),
        };
        let this = me.clone();
        tokio::spawn(async move {
            while let Ok((stream, _)) = listener.accept().await {
                let this = this.clone();
                tokio::spawn(async move { this.session(stream).await });
            }
        });

        me
    }

    async fn session(&self, stream: tokio::net::TcpStream) {
        let (r, mut w) = stream.into_split();
        let mut lines = BufReader::new(r).lines();

        if w.write_all(b"220 fake.test ESMTP\r\n").await.is_err() {
            return;
        }

        let mut rcpts = Vec::new();

        while let Ok(Some(line)) = lines.next_line().await {
            let upper = line.to_ascii_uppercase();
            let answer = if upper.starts_with("EHLO") || upper.starts_with("HELO") {
                "250-fake.test\r\n250 8BITMIME\r\n".to_owned()
            } else if upper.starts_with("MAIL FROM:") {
                rcpts.clear();
                "250 OK\r\n".to_owned()
            } else if upper.starts_with("RCPT TO:") {
                rcpts.push(line[8..].trim().to_owned());
                "250 OK\r\n".to_owned()
            } else if upper == "DATA" {
                if w.write_all(b"354 go ahead\r\n").await.is_err() {
                    return;
                }

                let mut data = String::new();

                while let Ok(Some(l)) = lines.next_line().await {
                    if l == "." {
                        break;
                    }

                    data.push_str(&l);
                    data.push('\n');
                }

                let reply = self.script.lock().unwrap().pop_front().unwrap_or(*self.always.lock().unwrap());
                self.mails.lock().unwrap().push(Mail { rcpts: std::mem::take(&mut rcpts), data, reply });
                format!("{reply} done\r\n")
            } else if upper == "QUIT" {
                let _ = w.write_all(b"221 bye\r\n").await;
                return;
            } else if upper == "RSET" || upper == "NOOP" {
                "250 OK\r\n".to_owned()
            } else {
                "500 unknown\r\n".to_owned()
            };

            if w.write_all(answer.as_bytes()).await.is_err() {
                return;
            }
        }
    }

    fn mails(&self) -> Vec<Mail> {
        self.mails.lock().unwrap().clone()
    }
}

fn email_channel(name: &str, smtp: &FakeSmtp) -> Value {
    json!({
        "name": name,
        "kind": "email",
        "email": {
            "smtp_host": "127.0.0.1",
            "smtp_port": smtp.port,
            "tls": "none",
            "from": "Forge <forge@example.com>",
            "to": ["ops@example.com", "oncall@example.com"],
            "subject_prefix": "[lab]",
        },
    })
}

#[tokio::test]
async fn a_fired_alarm_sends_exactly_one_email_and_a_test_message_goes_out() {
    let h = start_lab().await;
    let mut admin = Client::login(&h.base, ADMIN_EMAIL, PASSWORD).await;
    step_up(&mut admin).await;
    let smtp = FakeSmtp::start(&[], 250).await;
    let c = created(&admin, "/api/v1/notification-channels", email_channel("Ops mail", &smtp)).await;
    assert_eq!(c["target"], format!("smtp://127.0.0.1:{}", smtp.port));
    assert_eq!(c["email"]["to"].as_array().unwrap().len(), 2);
    assert_eq!(c["email"]["password_set"], false);
    created(&admin, "/api/v1/notification-policies", policy(&[&c])).await;
    let (server, _dir) = node(&h, &admin, "lab-b").await;
    let alarm = fire(&h, &server).await;

    let sent = wait_for("the email", 30, || {
        let (c, aid) = (&admin, alarm.id);
        async move { deliveries(c, &format!("?alarm={aid}&status=sent")).await.pop() }
    })
    .await;
    assert_eq!((sent["attempt_count"].as_i64(), sent["last_http_status"].as_i64()), (Some(1), Some(250)), "{sent}");

    // Several more passes add nothing.
    tokio::time::sleep(quiet_period(&h)).await;
    let mails = smtp.mails();
    assert_eq!(mails.len(), 1, "exactly one email for one firing: {mails:?}");
    let m = &mails[0];
    assert_eq!(m.rcpts, ["<ops@example.com>", "<oncall@example.com>"]);
    assert_eq!(subject(&m.data), "[lab] [FIRING][major] Disk almost full \u{2014} lab-b", "{}", m.data);
    assert!(m.data.contains("Resource: server lab-b"), "{}", m.data);
    assert!(m.data.contains(&format!("Message-ID: <{}@", sent["id"].as_str().unwrap())), "{}", m.data);
    assert!(m.data.contains("Auto-Submitted: auto-generated"), "{}", m.data);

    // The body is quoted-printable: `=` is written `=3D`.
    assert!(m.data.contains("Observed: node.disk.root.percent =3D 97.5"), "{}", m.data);

    // Plain text and a designed HTML part with the brand mark inline (D-119).
    assert!(m.data.contains("Content-Type: multipart/alternative"), "{}", m.data);
    let (plain, html) = (m.data.find("Content-Type: text/plain"), m.data.find("Content-Type: text/html"));
    assert!(matches!((plain, html), (Some(p), Some(h)) if p < h), "{}", m.data);
    assert!(m.data.contains("Content-Type: image/png") && m.data.contains("Content-ID: <forge-mark@orbyn-forge>"));

    // The test endpoint works for email.
    let cid = c["id"].as_str().unwrap();
    let (s, view) = admin.post(&format!("/api/v1/notification-channels/{cid}/test"), json!({})).await;
    assert_eq!((s, view["status"].as_str()), (StatusCode::OK, Some("sent")), "{view}");
    let mails = smtp.mails();
    assert_eq!(mails.len(), 2);
    assert_eq!(subject(&mails[1].data), "[lab] [TEST] Test notification from Orbyn Forge", "{}", mails[1].data);
    h.app.stop().await;
}

#[tokio::test]
async fn an_smtp_4xx_is_retried_and_a_5xx_fails_at_once() {
    let h = start_lab().await;
    let mut admin = Client::login(&h.base, ADMIN_EMAIL, PASSWORD).await;
    step_up(&mut admin).await;
    let busy = FakeSmtp::start(&[451], 250).await;
    let refusing = FakeSmtp::start(&[], 550).await;
    let a = created(&admin, "/api/v1/notification-channels", email_channel("Greylisted", &busy)).await;
    let b = created(&admin, "/api/v1/notification-channels", email_channel("Refusing", &refusing)).await;
    created(&admin, "/api/v1/notification-policies", policy(&[&a, &b])).await;
    let (server, _dir) = node(&h, &admin, "lab-c").await;
    let alarm = fire(&h, &server).await;

    let ok = wait_for("the greylisted email to go out", 60, || {
        let (c, aid) = (&admin, alarm.id);
        async move { deliveries(c, &format!("?alarm={aid}&status=sent")).await.pop() }
    })
    .await;
    assert_eq!(ok["channel_name"], "Greylisted");
    assert_eq!(ok["attempt_count"], 2, "{ok}");
    let (_, view) = admin.get(&format!("/api/v1/notification-deliveries/{}", ok["id"].as_str().unwrap())).await;
    let first = &view["attempts"][0];
    assert_eq!((first["error"].as_str(), first["http_status"].as_i64()), (Some("smtp_reply"), Some(451)), "{view}");
    assert_eq!(busy.mails().iter().map(|m| m.reply).collect::<Vec<_>>(), [451, 250]);

    let failed = wait_for("the refused email to fail", 30, || {
        let (c, aid) = (&admin, alarm.id);
        async move { deliveries(c, &format!("?alarm={aid}&status=failed")).await.pop() }
    })
    .await;
    assert_eq!(failed["channel_name"], "Refusing");
    assert_eq!(
        (failed["attempt_count"].as_i64(), failed["last_error"].as_str(), failed["last_http_status"].as_i64()),
        (Some(1), Some("smtp_reply"), Some(550)),
        "{failed}"
    );
    tokio::time::sleep(quiet_period(&h)).await;
    assert_eq!(refusing.mails().len(), 1, "a permanent refusal is not tried again");
    h.app.stop().await;
}

#[tokio::test]
async fn the_smtp_password_is_write_only() {
    let h = start_lab().await;
    let mut admin = Client::login(&h.base, ADMIN_EMAIL, PASSWORD).await;
    step_up(&mut admin).await;
    let smtp = FakeSmtp::start(&[], 250).await;
    let mut body = email_channel("Authenticated", &smtp);
    body["email"]["tls"] = json!("starttls");
    body["email"]["username"] = json!("forge");
    body["email"]["password"] = json!(SMTP_PASSWORD);

    // A user name over an unencrypted connection is refused.
    let mut plain = body.clone();
    plain["email"]["tls"] = json!("none");
    let (s, e) = admin.post("/api/v1/notification-channels", plain).await;
    assert_eq!(s, StatusCode::UNPROCESSABLE_ENTITY, "{e}");
    assert!(!e.to_string().contains(SMTP_PASSWORD), "{e}");

    let c = created(&admin, "/api/v1/notification-channels", body.clone()).await;
    assert_eq!(c["email"]["password_set"], true);
    let cid = c["id"].as_str().unwrap().to_owned();

    // An update without the password keeps it.
    let mut keep = body.clone();
    keep["email"].as_object_mut().unwrap().remove("password");
    keep["name"] = json!("Authenticated mail");
    let (s, u) = admin.put(&format!("/api/v1/notification-channels/{cid}"), keep).await;
    assert_eq!((s, u["email"]["password_set"].as_bool()), (StatusCode::OK, Some(true)), "{u}");

    // The fake server offers no STARTTLS: the message is not sent in the clear.
    let (s, view) = admin.post(&format!("/api/v1/notification-channels/{cid}/test"), json!({})).await;
    assert_eq!(
        (s, view["status"].as_str(), view["last_error"].as_str()),
        (StatusCode::OK, Some("failed"), Some("tls")),
        "{view}"
    );
    assert!(smtp.mails().is_empty());

    let (_, list) = admin.get("/api/v1/notification-channels").await;
    let (_, one) = admin.get(&format!("/api/v1/notification-channels/{cid}")).await;
    let (_, history) = admin.get("/api/v1/notification-deliveries").await;
    let (_, audit) = admin.get("/api/v1/audit?limit=200").await;

    for (what, v) in [("create", &c), ("list", &list), ("get", &one), ("test", &view), ("deliveries", &history)] {
        assert!(!v.to_string().contains(SMTP_PASSWORD), "{what}: {v}");
    }

    let audit = audit.to_string();
    assert!(audit.contains(AUDIT_CREATE) && audit.contains(AUDIT_UPDATE), "channel changes are audited");
    assert!(!audit.contains(SMTP_PASSWORD), "the audit log never holds the SMTP password");
    assert!(audit.contains("password_changed"), "{audit}");
    h.app.stop().await;
}

const AUDIT_CREATE: &str = "notification.channel.create";
const AUDIT_UPDATE: &str = "notification.channel.update";

/// The `Subject` header of a raw message, with RFC 2047 encoded words
/// (`=?utf-8?b?…?=`) decoded.
fn subject(data: &str) -> String {
    use base64::Engine as _;
    let mut raw = String::new();
    let mut inside = false;

    for line in data.lines() {
        if let Some(rest) = line.strip_prefix("Subject:") {
            raw.push_str(rest.trim());
            inside = true;
        } else if inside && line.starts_with([' ', '\t']) {
            raw.push(' ');
            raw.push_str(line.trim());
        } else if inside {
            break;
        }
    }

    // White space between two encoded words is dropped; any other stays.
    let mut out = String::new();
    let mut last_encoded = false;

    for word in raw.split(' ') {
        let encoded = word.strip_prefix("=?utf-8?b?").or_else(|| word.strip_prefix("=?UTF-8?B?"));
        let decoded = encoded
            .and_then(|w| w.strip_suffix("?="))
            .map(|b64| String::from_utf8(base64::engine::general_purpose::STANDARD.decode(b64).unwrap()).unwrap());

        if !out.is_empty() && !(last_encoded && decoded.is_some()) {
            out.push(' ');
        }

        last_encoded = decoded.is_some();
        out.push_str(&decoded.unwrap_or_else(|| word.to_owned()));
    }

    out
}

/// Policy recipients (D-117): a policy emails a named user and the holders
/// of a role through an email channel. The SMTP server sees exactly the
/// resolved people as envelope recipients (blind copies), never the
/// channel's own `to`; a named user without an address is a warning; only a
/// notification manager sees the addresses in the history.
#[tokio::test]
async fn a_policy_emails_named_users_and_role_holders() {
    use forge_core::ports::UserRepository;
    let h = start_lab().await;
    let mut admin = Client::login(&h.base, ADMIN_EMAIL, PASSWORD).await;
    step_up(&mut admin).await;
    let ws = format!("workspace:{}", h.default_org);
    let developer = user_with_role(&h, &admin, "dev@example.com", "developer", &ws).await;
    let _operator = user_with_role(&h, &admin, "op@example.com", "operator", &ws).await;
    let _other = user_with_role(&h, &admin, "auditor@example.com", "auditor", &ws).await;
    let state = &h.app.state;
    let nomail = state.identity.create_account(Some("nomail"), None, "No Mail", PASSWORD).await.unwrap();
    state
        .grant(&forge_core::model::RoleBinding {
            id: forge_core::RoleBindingId::new(),
            user_id: nomail.id,
            role: forge_core::Role::Developer,
            scope: ws.parse().unwrap(),
            created_at: state.clock.now(),
        })
        .await
        .unwrap();
    let dev = state.store.find_user_by_email("dev@example.com").await.unwrap().unwrap();

    let smtp = FakeSmtp::start(&[], 250).await;
    let c = created(&admin, "/api/v1/notification-channels", email_channel("Ops mail", &smtp)).await;

    // An email channel is required, and it must be one.
    let mut body = policy(&[]);
    body["email_users"] = json!([dev.id, nomail.id]);
    body["email_roles"] = json!(["operator"]);
    let (s, e) = admin.post("/api/v1/notification-policies", body.clone()).await;
    assert_eq!(s, StatusCode::UNPROCESSABLE_ENTITY, "{e}");
    body["email_channel"] = c["id"].clone();
    let p = created(&admin, "/api/v1/notification-policies", body).await;
    assert_eq!(p["warnings"]["users_without_email"], json!([nomail.id]), "{p}");
    assert_eq!(p["channels"], json!([]));
    let (_, audit) = admin.get("/api/v1/audit?limit=50").await;
    let created_entry = audit.to_string();
    assert!(created_entry.contains("email_roles") && created_entry.contains(&dev.id.to_string()), "{audit}");

    let (server, _dir) = node(&h, &admin, "lab-b").await;
    let alarm = fire(&h, &server).await;
    let sent = wait_for("the email to people", 30, || {
        let (c, aid) = (&admin, alarm.id);
        async move { deliveries(c, &format!("?alarm={aid}&status=sent")).await.pop() }
    })
    .await;
    let mails = smtp.mails();
    assert_eq!(mails.len(), 1, "{mails:?}");
    let mut rcpts = mails[0].rcpts.clone();
    rcpts.sort();
    assert_eq!(rcpts, ["<dev@example.com>", "<op@example.com>"], "named user and role holder, nobody else");
    let header_block = mails[0].data.split("\n\n").next().unwrap_or_default().to_owned();
    assert!(!header_block.contains("dev@example.com") && !header_block.contains("ops@example.com"), "{header_block}");

    // The manager sees the addresses and the skipped user; a reader sees the count.
    assert_eq!(sent["recipients"]["count"], 2, "{sent}");
    assert_eq!(sent["recipients"]["to"].as_array().map(Vec::len), Some(2), "{sent}");
    assert_eq!(sent["recipients"]["skipped"], json!([{"user_id": nomail.id, "reason": "no_email"}]), "{sent}");
    let (s, seen) = developer.get(&format!("/api/v1/notification-deliveries/{}", sent["id"].as_str().unwrap())).await;
    assert_eq!(s, StatusCode::OK, "{seen}");
    assert_eq!(seen["recipients"]["count"], 2, "{seen}");
    assert!(seen["recipients"].get("to").is_none(), "{seen}");
    assert!(!seen.to_string().contains("@example.com"), "{seen}");
    h.app.stop().await;
}

/// Links in alert emails use the runtime public address over `public_url`
/// (D-120), with no restart.
#[tokio::test]
async fn alert_emails_link_to_the_runtime_public_address() {
    let h = start_lab().await;
    let mut admin = Client::login(&h.base, ADMIN_EMAIL, PASSWORD).await;
    step_up(&mut admin).await;
    let (s, v) = admin.put("/api/v1/server/public-address", json!({ "value": "https://forge.example.com/" })).await;
    assert_eq!((s, v["effective"].as_str()), (StatusCode::OK, Some("https://forge.example.com")), "{v}");
    let smtp = FakeSmtp::start(&[], 250).await;
    let c = created(&admin, "/api/v1/notification-channels", email_channel("Ops mail", &smtp)).await;
    created(&admin, "/api/v1/notification-policies", policy(&[&c])).await;
    let (server, _dir) = node(&h, &admin, "lab-b").await;
    let alarm = fire(&h, &server).await;
    wait_for("the email", 30, || {
        let (c, aid) = (&admin, alarm.id);
        async move { deliveries(c, &format!("?alarm={aid}&status=sent")).await.pop() }
    })
    .await;

    // Undo quoted-printable: soft line breaks and `=3D`.
    let body = smtp.mails()[0].data.replace("=\r\n", "").replace("=\n", "").replace("=3D", "=");
    let link = format!("https://forge.example.com/alerts?alarm={}", alarm.id);
    assert!(body.contains(&format!("Open the alarm: {link}")), "{body}");
    assert!(!body.contains(&format!("{}/alerts?alarm", h.base)), "the configured URL is not used: {body}");
    h.app.stop().await;
}
