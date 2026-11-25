//! What a notification says (#11, D-110): one fixed JSON document for
//! webhooks, one fixed text for Slack-compatible receivers, and the
//! signature header. Templates are a later slice; until then the shapes here
//! are the contract and are versioned by [`SCHEMA_VERSION`].
//!
//! A payload holds alarm facts only: what fired, where, how bad, and since
//! when. Nothing from the vault, no environment values, no URLs but a link
//! back to the console.

use forge_core::alerts::{Alarm, AlarmState, Severity};
use forge_core::notifications::{Delivery, DeliveryEvent};
use forge_core::{AlarmId, AlertRuleId, DeliveryId, ServerId, SilenceId, Timestamp, WorkspaceId};
use ring::hmac;
use serde::Serialize;

/// Version of the webhook payload. It changes only when a field is removed
/// or changes meaning; new optional fields do not change it.
pub const SCHEMA_VERSION: u32 = 1;

/// Header naming the event (`fired`, `resolved`, `ended`, `reminder`, `test`).
pub const HEADER_EVENT: &str = "x-forge-event";
/// Header with the delivery's id: the same on every retry, so a receiver can
/// drop a repeat.
pub const HEADER_DELIVERY: &str = "x-forge-delivery";
/// Header with the time of signing, in seconds since the epoch.
pub const HEADER_TIMESTAMP: &str = "x-forge-timestamp";
/// Header with `v1=<hex HMAC-SHA256>` over `<timestamp>.<body>`.
pub const HEADER_SIGNATURE: &str = "x-forge-signature";

/// Longest interpolated name or value in a text message, in characters.
const MAX_TEXT_FIELD: usize = 200;

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Payload {
    pub schema_version: u32,
    pub delivery_id: DeliveryId,
    pub workspace_id: WorkspaceId,
    pub event: EventView,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub alarm: Option<AlarmView>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub rule: Option<RuleView>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub resource: Option<ResourceView>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub observation: Option<ObservationView>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub links: Option<Links>,
    /// Only on a test message.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
    /// Only on the message sent when a silence has ended: it stands for the
    /// messages that silence held back.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub after_silence: Option<AfterSilence>,
}

/// The silence a message follows.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct AfterSilence {
    pub silence_id: SilenceId,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct EventView {
    #[serde(rename = "type")]
    pub kind: DeliveryEvent,
    /// The alarm recovered. `false` for `ended`: tracking stopped without
    /// recovery evidence.
    pub is_resolved: bool,
    /// 0, or the number of the reminder.
    pub occurrence: i64,
    /// When the delivery was planned (epoch ms).
    pub at: Timestamp,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct AlarmView {
    pub id: AlarmId,
    pub title: String,
    pub severity: Severity,
    pub state: AlarmState,
    pub fired_at: Timestamp,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub resolved_at: Option<Timestamp>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub end_reason: Option<String>,
    pub acknowledged: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub acknowledged_at: Option<Timestamp>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct RuleView {
    pub id: AlertRuleId,
    pub revision: i64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ResourceView {
    pub kind: String,
    pub id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// The node a service instance runs on.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub server_id: Option<ServerId>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ObservationView {
    pub subject: String,
    /// The value that fired the alarm: `{"type": "num"|"state", "value": …}`.
    pub value: serde_json::Value,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Links {
    /// The alarm in the console.
    pub alarm: String,
}

/// What the payload is built from.
#[derive(Debug, Clone, Copy)]
pub struct Context<'a> {
    pub delivery: &'a Delivery,
    /// `None` for a test message.
    pub alarm: Option<&'a Alarm>,
    pub resource_name: Option<&'a str>,
    /// The console's public URL, without a trailing slash.
    pub public_url: &'a str,
}

/// The payload of a delivery.
#[must_use]
pub fn build(cx: Context<'_>) -> Payload {
    let d = cx.delivery;
    let a = cx.alarm;

    Payload {
        schema_version: SCHEMA_VERSION,
        delivery_id: d.id,
        workspace_id: d.workspace_id,
        event: EventView {
            kind: d.event,
            is_resolved: d.event == DeliveryEvent::Resolved,
            occurrence: d.occurrence,
            at: d.created_at,
        },
        alarm: a.map(|a| AlarmView {
            id: a.id,
            title: a.title.clone(),
            severity: a.severity,
            state: a.state,
            fired_at: a.fired_at,
            resolved_at: a.resolved_at,
            end_reason: a.end_reason.clone(),
            acknowledged: a.acknowledged_at.is_some(),
            acknowledged_at: a.acknowledged_at,
        }),
        rule: a.map(|a| RuleView { id: a.rule_id, revision: a.rule_revision }),
        resource: a.map(|a| ResourceView {
            kind: a.resource.kind.as_str().to_owned(),
            id: a.resource.id.clone(),
            name: cx.resource_name.map(str::to_owned),
            server_id: a.server,
        }),
        observation: a.map(|a| ObservationView { subject: a.subject.clone(), value: a.fired_value.clone() }),
        // No console address known yet (nothing pinned, no administrator
        // signed in, D-167): no link rather than a relative one.
        links: a
            .filter(|_| !cx.public_url.trim().is_empty())
            .map(|a| Links { alarm: format!("{}/alerts?alarm={}", cx.public_url.trim_end_matches('/'), a.id) }),
        message: a.is_none().then(|| "Test notification from Orbyn Forge. No alarm fired.".to_owned()),
        after_silence: match d.silence_id {
            Some(silence_id) if d.after_silence => Some(AfterSilence { silence_id }),
            _ => None,
        },
    }
}

/// Characters that change how text is laid out or read without showing:
/// control characters, line and paragraph separators, bidirectional
/// overrides and isolates, zero-width and other format characters. With
/// them a name could reorder or hide part of a message.
pub(crate) fn invisible(c: char) -> bool {
    c.is_control()
        || matches!(
            c,
            '\u{00ad}'
                | '\u{061c}'
                | '\u{180e}'
                | '\u{200b}'..='\u{200f}'
                | '\u{2028}'..='\u{202e}'
                | '\u{2060}'..='\u{206f}'
                | '\u{feff}'
                | '\u{fff9}'..='\u{fffb}'
        )
}

/// Makes a value safe inside a Slack-style message: one line, bounded, no
/// markup (`<…>` links and `<!channel>` mentions), no `@` mention that a
/// compatible receiver would expand (`@everyone`, `@here`), no invisible or
/// direction-changing characters, and no `scheme://` a receiver would turn
/// into a link. Names and observed states come from rules and from nodes;
/// neither may page a whole channel or plant a link in it.
#[must_use]
pub fn plain(value: &str) -> String {
    let mut out = String::new();

    for c in value.chars().filter(|c| !invisible(*c)).take(MAX_TEXT_FIELD) {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            // A zero-width space after the sign breaks the mention.
            '@' => out.push_str("@\u{200b}"),
            _ => out.push(c),
        }
    }

    // The only link in a message is the console's, added by `text`.
    out.replace("://", ":\u{200b}//")
}

fn observed(value: &serde_json::Value) -> Option<String> {
    match value.get("value") {
        Some(serde_json::Value::String(s)) => Some(plain(s)),
        Some(serde_json::Value::Number(n)) => Some(n.to_string()),
        _ => None,
    }
}

/// The text of a Slack-compatible message.
#[must_use]
pub fn text(p: &Payload) -> String {
    let Some(alarm) = &p.alarm else {
        return p.message.clone().unwrap_or_default();
    };

    let head = match p.event.kind {
        DeliveryEvent::Fired => "ALERT",
        DeliveryEvent::Reminder => "STILL FIRING",
        DeliveryEvent::Resolved => "RECOVERED",
        DeliveryEvent::Ended => "ENDED",
        DeliveryEvent::Test => "TEST",
    };

    let mut lines = vec![format!("{head} [{}] {}", alarm.severity.as_str(), plain(&alarm.title))];

    if let Some(r) = &p.resource {
        let name = r.name.as_deref().unwrap_or(&r.id);
        lines.push(format!("Resource: {} {}", plain(&r.kind), plain(name)));
    }

    if let Some(o) = &p.observation {
        match observed(&o.value) {
            Some(v) => lines.push(format!("Observed: {} = {v}", plain(&o.subject))),
            None => lines.push(format!("Observed: {}", plain(&o.subject))),
        }
    }

    match p.event.kind {
        DeliveryEvent::Ended => {
            let why = alarm.end_reason.as_deref().unwrap_or("tracking stopped");
            lines.push(format!("Tracking ended without recovery: {}", plain(why)));
        }
        DeliveryEvent::Reminder if !alarm.acknowledged => lines.push("Nobody has acknowledged this alarm.".to_owned()),
        _ => {}
    }

    if p.after_silence.is_some() {
        lines.push("Sent now because a silence that held this back has ended.".to_owned());
    }

    if let Some(l) = &p.links {
        lines.push(l.alarm.clone());
    }

    lines.join("\n")
}

/// The Slack-compatible body: `{"text": …}`, serialized (never assembled by
/// hand, so quotes and newlines in a name stay inside the string).
#[derive(Debug, Serialize)]
pub struct SlackBody {
    pub text: String,
}

/// `v1=<hex>`: HMAC-SHA256 over `<timestamp>.<body>` with the channel's
/// secret. The timestamp is part of the signed text so a captured request
/// cannot be replayed later with a new time.
#[must_use]
pub fn sign(secret: &[u8], timestamp_secs: i64, body: &[u8]) -> String {
    let key = hmac::Key::new(hmac::HMAC_SHA256, secret);
    let mut cx = hmac::Context::with_key(&key);
    cx.update(timestamp_secs.to_string().as_bytes());
    cx.update(b".");
    cx.update(body);

    format!("v1={}", hex::encode(cx.sign().as_ref()))
}

/// Checks a signature the way a receiver does (constant time).
#[must_use]
pub fn verify(secret: &[u8], timestamp_secs: i64, body: &[u8], signature: &str) -> bool {
    let Some(hex_tag) = signature.strip_prefix("v1=") else { return false };
    let Ok(tag) = hex::decode(hex_tag) else { return false };
    let key = hmac::Key::new(hmac::HMAC_SHA256, secret);
    let mut msg = timestamp_secs.to_string().into_bytes();
    msg.push(b'.');
    msg.extend_from_slice(body);

    hmac::verify(&key, &msg, &tag).is_ok()
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]
    use forge_core::notifications::DeliveryStatus;
    use forge_core::{ChannelId, EntityKind, EntityRef};
    use serde_json::json;

    use super::*;

    fn alarm() -> Alarm {
        let node = ServerId::new();
        Alarm {
            id: AlarmId::new(),
            workspace_id: WorkspaceId::new(),
            rule_id: AlertRuleId::new(),
            rule_revision: 3,
            title: "Node offline".into(),
            severity: Severity::Critical,
            fingerprint: "fp".into(),
            resource: EntityRef::new(EntityKind::Server, node),
            server: None,
            subject: "node.agent.status".into(),
            state: AlarmState::Firing,
            fired_value: json!({"type": "state", "value": "offline"}),
            fired_at: Timestamp(1_000),
            resolved_at: None,
            end_reason: None,
            acknowledged_by: None,
            acknowledged_at: None,
            previous: None,
        }
    }

    fn delivery(a: Option<&Alarm>, event: DeliveryEvent) -> Delivery {
        Delivery {
            id: DeliveryId::new(),
            workspace_id: a.map_or_else(WorkspaceId::new, |a| a.workspace_id),
            alarm_id: a.map(|a| a.id),
            event,
            occurrence: 0,
            policy_id: None,
            channel_id: ChannelId::new(),
            dedupe_key: "k".into(),
            status: DeliveryStatus::Pending,
            attempts: 0,
            next_attempt_at: Timestamp(2_000),
            lease_until: None,
            last_error: None,
            last_http_status: None,
            detail: None,
            silence_id: None,
            after_silence: false,
            released_at: None,
            created_at: Timestamp(2_000),
            finished_at: None,
            channel_name: "ops".into(),
            policy_name: "all".into(),
            alarm_title: "Node offline".into(),
            resource: None,
            recipients: None,
        }
    }

    #[test]
    fn the_webhook_payload_has_the_documented_shape() {
        let a = alarm();
        let d = delivery(Some(&a), DeliveryEvent::Fired);
        let p = build(Context {
            delivery: &d,
            alarm: Some(&a),
            resource_name: Some("web-01"),
            public_url: "https://forge.example.com/",
        });
        let v = serde_json::to_value(&p).unwrap();
        assert_eq!(
            v,
            json!({
                "schema_version": 1,
                "delivery_id": d.id,
                "workspace_id": a.workspace_id,
                "event": {"type": "fired", "is_resolved": false, "occurrence": 0, "at": 2000},
                "alarm": {
                    "id": a.id,
                    "title": "Node offline",
                    "severity": "critical",
                    "state": "firing",
                    "fired_at": 1000,
                    "acknowledged": false,
                },
                "rule": {"id": a.rule_id, "revision": 3},
                "resource": {"kind": "server", "id": a.resource.id, "name": "web-01"},
                "observation": {"subject": "node.agent.status", "value": {"type": "state", "value": "offline"}},
                "links": {"alarm": format!("https://forge.example.com/alerts?alarm={}", a.id)},
            })
        );
    }

    #[test]
    fn resolved_ended_and_test_payloads_differ_where_they_should() {
        let mut a = alarm();
        a.state = AlarmState::Resolved;
        a.resolved_at = Some(Timestamp(5_000));
        let d = delivery(Some(&a), DeliveryEvent::Resolved);
        let p = build(Context { delivery: &d, alarm: Some(&a), resource_name: None, public_url: "https://f" });
        assert!(p.event.is_resolved);
        assert_eq!(p.alarm.as_ref().unwrap().resolved_at, Some(Timestamp(5_000)));
        assert!(text(&p).starts_with("RECOVERED [critical] Node offline"));

        a.state = AlarmState::Ended;
        a.end_reason = Some("the rule was deleted".into());
        let d = delivery(Some(&a), DeliveryEvent::Ended);
        let p = build(Context { delivery: &d, alarm: Some(&a), resource_name: None, public_url: "https://f" });
        assert!(!p.event.is_resolved, "ended is not a recovery");
        assert!(text(&p).contains("Tracking ended without recovery: the rule was deleted"));

        let d = delivery(None, DeliveryEvent::Test);
        let p = build(Context { delivery: &d, alarm: None, resource_name: None, public_url: "https://f" });
        let v = serde_json::to_value(&p).unwrap();
        assert_eq!(v["event"]["type"], "test");
        assert!(v.get("alarm").is_none() && v.get("links").is_none());
        assert_eq!(text(&p), "Test notification from Orbyn Forge. No alarm fired.");
    }

    #[test]
    fn text_values_cannot_inject_markup_mentions_or_lines() {
        assert_eq!(
            plain("a <!channel> & <https://evil|ok>"),
            "a &lt;!channel&gt; &amp; &lt;https:\u{200b}//evil|ok&gt;"
        );
        assert_eq!(plain("@everyone"), "@\u{200b}everyone");
        assert_eq!(plain("one\ntwo\r\tthree\u{0}"), "onetwothree");
        assert_eq!(plain(&"x".repeat(500)).chars().count(), 200);

        // Direction overrides, isolates, zero-width and separator characters go.
        assert_eq!(plain("ok\u{202e}gnp.exe\u{202c}"), "okgnp.exe");
        assert_eq!(plain("a\u{2066}b\u{2069}c\u{200f}d\u{061c}e"), "abcde");
        assert_eq!(plain("one\u{2028}two\u{2029}three"), "onetwothree");
        assert_eq!(plain("ze\u{200b}ro\u{feff}\u{2060}\u{00ad}"), "zero");

        // No value becomes a link.
        assert_eq!(plain("see https://evil.example/x"), "see https:\u{200b}//evil.example/x");
        assert_eq!(plain("slack://open"), "slack:\u{200b}//open");
        assert!(!plain("h\u{200b}ttps:\u{200b}//x & ftp://y").contains("://"));

        let mut a = alarm();
        a.title = "@here <!channel>\nALERT [critical] fake".into();
        a.fired_value = json!({"type": "state", "value": "<@U123> @everyone"});
        let d = delivery(Some(&a), DeliveryEvent::Fired);
        let p =
            build(Context { delivery: &d, alarm: Some(&a), resource_name: Some("web\n01"), public_url: "https://f" });
        let t = text(&p);
        assert!(!t.contains("<!") && !t.contains("<@") && !t.contains("@everyone") && !t.contains("@here"), "{t}");
        assert_eq!(t.lines().count(), 4, "a name cannot add a line: {t}");

        // The JSON body keeps everything inside one string.
        let body = serde_json::to_string(&SlackBody { text: t.clone() }).unwrap();
        assert_eq!(serde_json::from_str::<serde_json::Value>(&body).unwrap()["text"], t);
    }

    #[test]
    fn signatures_bind_the_secret_the_time_and_the_body() {
        let sig = sign(b"s3cret", 1_700_000_000, b"{\"a\":1}");
        assert!(sig.starts_with("v1=") && sig.len() == 3 + 64);

        // RFC 4231-style check against an independently computed value.
        assert_eq!(sig, sign(b"s3cret", 1_700_000_000, b"{\"a\":1}"), "deterministic");
        assert!(verify(b"s3cret", 1_700_000_000, b"{\"a\":1}", &sig));
        assert!(!verify(b"other", 1_700_000_000, b"{\"a\":1}", &sig));
        assert!(!verify(b"s3cret", 1_700_000_001, b"{\"a\":1}", &sig));
        assert!(!verify(b"s3cret", 1_700_000_000, b"{\"a\":2}", &sig));
        assert!(!verify(b"s3cret", 1_700_000_000, b"{\"a\":1}", "v1=zz"));
        assert!(!verify(b"s3cret", 1_700_000_000, b"{\"a\":1}", &sig[3..]));

        // Known answer: HMAC-SHA256("key", "1.The quick brown fox jumps over the lazy dog").
        let known = sign(b"key", 1, b"The quick brown fox jumps over the lazy dog");
        let key = hmac::Key::new(hmac::HMAC_SHA256, b"key");
        let tag = hmac::sign(&key, b"1.The quick brown fox jumps over the lazy dog");
        assert_eq!(known, format!("v1={}", hex::encode(tag.as_ref())));
    }
}
