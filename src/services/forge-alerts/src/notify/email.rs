//! What an email notification says (#11, D-113): a subject that sorts and
//! filters well, a plain-text body with the same facts as the
//! Slack-compatible text, and a designed HTML part ([`html`], D-119). Built
//! from the same [`Payload`] as the other channel kinds, so every kind tells
//! the same story.
//!
//! Values that come from rules and nodes (titles, names, observed states)
//! are made one line, bounded, stripped of invisible and direction-changing
//! characters, and kept from turning into links. The only link is the
//! console's. Nothing from the vault and no SMTP setting is ever part of a
//! message.

use forge_core::alerts::AlarmState;
use forge_core::notifications::DeliveryEvent;

use super::payload::{Payload, invisible};

pub mod html;

pub use html::{MARK_CID, Why, html};

/// Longest interpolated value, in characters.
const MAX_FIELD: usize = 200;

/// One line, bounded, nothing invisible, no `scheme://` a mail client would
/// turn into a link. Unlike the Slack text, nothing is escaped: the body is
/// plain text.
#[must_use]
pub fn mail_plain(value: &str) -> String {
    let out: String = value.chars().filter(|c| !invisible(*c)).take(MAX_FIELD).collect();
    out.replace("://", ":\u{200b}//")
}

/// The marker of an event in the subject: `FIRING`, `REMINDER`, `RESOLVED`,
/// `ENDED` or `TEST`.
#[must_use]
pub const fn marker(event: DeliveryEvent) -> &'static str {
    match event {
        DeliveryEvent::Fired => "FIRING",
        DeliveryEvent::Reminder => "REMINDER",
        DeliveryEvent::Resolved => "RESOLVED",
        DeliveryEvent::Ended => "ENDED",
        DeliveryEvent::Test => "TEST",
    }
}

/// The subject: `[FIRING][critical] Node offline — lab-b`, after the
/// channel's prefix, with ` (after silence)` when a silence held the news
/// back.
#[must_use]
pub fn subject(p: &Payload, prefix: Option<&str>) -> String {
    let mut s = String::new();

    if let Some(pre) = prefix.map(mail_plain).filter(|x| !x.trim().is_empty()) {
        s.push_str(pre.trim());
        s.push(' ');
    }

    s.push('[');
    s.push_str(marker(p.event.kind));
    s.push(']');

    match &p.alarm {
        Some(a) => {
            s.push_str(&format!("[{}] {}", a.severity.as_str(), mail_plain(&a.title)));

            if let Some(r) = &p.resource {
                s.push_str(" \u{2014} ");
                s.push_str(&mail_plain(r.name.as_deref().unwrap_or(&r.id)));
            }
        }
        None => s.push_str(" Test notification from Orbyn Forge"),
    }

    if p.after_silence.is_some() {
        s.push_str(" (after silence)");
    }

    s
}

/// A time in epoch ms as RFC 3339 UTC (`2026-11-16T09:30:00Z`).
fn utc(ms: i64) -> String {
    let secs = ms.div_euclid(1000);
    let (days, rem) = (secs.div_euclid(86_400), secs.rem_euclid(86_400));

    // Civil date from days since 1970-01-01 (Howard Hinnant's algorithm).
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400 + i64::from(month <= 2);

    format!("{year:04}-{month:02}-{day:02}T{:02}:{:02}:{:02}Z", rem / 3600, rem % 3600 / 60, rem % 60)
}

fn observed(value: &serde_json::Value) -> Option<String> {
    match value.get("value") {
        Some(serde_json::Value::String(s)) => Some(mail_plain(s)),
        Some(serde_json::Value::Number(n)) => Some(n.to_string()),
        _ => None,
    }
}

/// The plain-text body.
#[must_use]
pub fn body(p: &Payload) -> String {
    let Some(alarm) = &p.alarm else {
        return format!(
            "{}\n\nThis channel can deliver Orbyn Forge alarm notifications.\nDelivery: {}\n",
            p.message.as_deref().unwrap_or_default(),
            p.delivery_id
        );
    };

    let head = match p.event.kind {
        DeliveryEvent::Fired => "An alarm is firing.",
        DeliveryEvent::Reminder => "An alarm is still firing.",
        DeliveryEvent::Resolved => "An alarm recovered.",
        DeliveryEvent::Ended => "Tracking of an alarm ended without recovery.",
        DeliveryEvent::Test => "Test notification.",
    };

    let mut lines = vec![head.to_owned(), String::new()];
    lines.push(format!("Alarm:    {}", mail_plain(&alarm.title)));
    lines.push(format!("Severity: {}", alarm.severity.as_str()));

    let state = match alarm.state {
        AlarmState::Firing => "firing",
        AlarmState::Resolved => "resolved",
        AlarmState::Ended => "ended",
    };

    lines.push(format!("State:    {state}"));

    if let Some(r) = &p.resource {
        let name = r.name.as_deref().unwrap_or(&r.id);
        lines.push(format!("Resource: {} {}", mail_plain(&r.kind), mail_plain(name)));
    }

    if let Some(o) = &p.observation {
        match observed(&o.value) {
            Some(v) => lines.push(format!("Observed: {} = {v}", mail_plain(&o.subject))),
            None => lines.push(format!("Observed: {}", mail_plain(&o.subject))),
        }
    }

    lines.push(format!("Fired at: {}", utc(alarm.fired_at.0)));

    if let Some(at) = alarm.resolved_at {
        lines.push(format!("Closed at: {}", utc(at.0)));
    }

    match p.event.kind {
        DeliveryEvent::Ended => {
            let why = alarm.end_reason.as_deref().unwrap_or("tracking stopped");
            lines.push(format!("Tracking ended without recovery: {}", mail_plain(why)));
        }
        DeliveryEvent::Reminder if !alarm.acknowledged => lines.push("Nobody has acknowledged this alarm.".to_owned()),
        _ => {}
    }

    if p.after_silence.is_some() {
        lines.push("Sent now because a silence that held this back has ended.".to_owned());
    }

    if let Some(l) = &p.links {
        lines.push(String::new());
        lines.push(format!("Open the alarm: {}", l.alarm));
    }

    lines.push(String::new());
    lines.push(format!("Alarm {}, delivery {}", alarm.id, p.delivery_id));
    let mut text = lines.join("\n");
    text.push('\n');

    text
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]
    use forge_core::alerts::Severity;
    use forge_core::{AlarmId, DeliveryId, SilenceId, Timestamp, WorkspaceId};

    use super::super::payload::{AfterSilence, AlarmView, EventView, Links, ObservationView, ResourceView};
    use super::*;

    fn payload(event: DeliveryEvent) -> Payload {
        Payload {
            schema_version: 1,
            delivery_id: DeliveryId::new(),
            workspace_id: WorkspaceId::new(),
            event: EventView {
                kind: event,
                is_resolved: event == DeliveryEvent::Resolved,
                occurrence: 0,
                at: Timestamp(0),
            },
            alarm: Some(AlarmView {
                id: AlarmId::new(),
                title: "Node offline".into(),
                severity: Severity::Critical,
                state: AlarmState::Firing,
                fired_at: Timestamp(1_763_285_400_000),
                resolved_at: None,
                end_reason: None,
                acknowledged: false,
                acknowledged_at: None,
            }),
            rule: None,
            resource: Some(ResourceView {
                kind: "server".into(),
                id: "s-1".into(),
                name: Some("lab-b".into()),
                server_id: None,
            }),
            observation: Some(ObservationView {
                subject: "node.agent.status".into(),
                value: serde_json::json!({"type": "state", "value": "offline"}),
            }),
            links: Some(Links { alarm: "https://forge.example.com/alerts?alarm=a".into() }),
            message: None,
            after_silence: None,
        }
    }

    #[test]
    fn subjects_name_the_event_severity_alarm_and_resource() {
        assert_eq!(subject(&payload(DeliveryEvent::Fired), None), "[FIRING][critical] Node offline \u{2014} lab-b");
        assert_eq!(
            subject(&payload(DeliveryEvent::Resolved), Some("[prod]")),
            "[prod] [RESOLVED][critical] Node offline \u{2014} lab-b"
        );
        assert!(subject(&payload(DeliveryEvent::Reminder), None).starts_with("[REMINDER][critical]"));
        assert!(subject(&payload(DeliveryEvent::Ended), None).starts_with("[ENDED]"));
        let mut p = payload(DeliveryEvent::Fired);
        p.after_silence = Some(AfterSilence { silence_id: SilenceId::new() });
        assert!(subject(&p, None).ends_with("lab-b (after silence)"));
        let mut t = payload(DeliveryEvent::Test);
        t.alarm = None;
        t.resource = None;
        t.message = Some("Test notification from Orbyn Forge. No alarm fired.".into());
        assert_eq!(subject(&t, Some("  ")), "[TEST] Test notification from Orbyn Forge");
        assert!(body(&t).starts_with("Test notification from Orbyn Forge. No alarm fired."));
    }

    #[test]
    fn values_cannot_add_lines_headers_or_links() {
        let mut p = payload(DeliveryEvent::Fired);
        if let Some(a) = p.alarm.as_mut() {
            a.title = "x\r\nBcc: victim@example.com\u{202e} see https://evil.example".into();
        }

        let s = subject(&p, Some("[ops]\r\nX-Evil: 1"));
        assert!(!s.contains('\n') && !s.contains('\r') && !s.contains('\u{202e}'), "{s}");
        assert!(!s.contains("https://evil"), "{s}");
        let b = body(&p);
        assert!(!b.contains("\nBcc:") && !b.contains("https://evil"), "{b}");
        assert_eq!(mail_plain(&"y".repeat(500)).chars().count(), 200);
    }

    #[test]
    fn the_body_carries_the_facts_and_the_console_link() {
        let b = body(&payload(DeliveryEvent::Fired));

        for want in [
            "An alarm is firing.",
            "Alarm:    Node offline",
            "Severity: critical",
            "Resource: server lab-b",
            "Observed: node.agent.status = offline",
            "Fired at: 2025-11-16T09:30:00Z",
            "Open the alarm: https://forge.example.com/alerts?alarm=a",
        ] {
            assert!(b.contains(want), "{want} in {b}");
        }

        let mut r = payload(DeliveryEvent::Reminder);
        r.links = None;
        let b = body(&r);
        assert!(b.contains("Nobody has acknowledged") && !b.contains("Open the alarm"), "{b}");
    }

    #[test]
    fn utc_formats_epoch_milliseconds() {
        assert_eq!(utc(0), "1970-01-01T00:00:00Z");
        assert_eq!(utc(951_782_400_000), "2000-02-29T00:00:00Z");
        assert_eq!(utc(1_763_285_400_000), "2025-11-16T09:30:00Z");
    }
}
