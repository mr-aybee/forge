#![allow(clippy::unwrap_used)]
use forge_core::alerts::{AlarmState, Severity};
use forge_core::notifications::DeliveryEvent;
use forge_core::{AlarmId, AlertRuleId, DeliveryId, ServerId, SilenceId, Timestamp, WorkspaceId};

use super::super::super::payload::{
    AfterSilence, AlarmView, EventView, Links, ObservationView, Payload, ResourceView, RuleView,
};
use super::*;

const FIRED: i64 = 1_763_285_400_000;

fn payload(event: DeliveryEvent, severity: Severity, subject: &str, value: serde_json::Value) -> Payload {
    let closed = matches!(event, DeliveryEvent::Resolved | DeliveryEvent::Ended);
    Payload {
        schema_version: 1,
        delivery_id: DeliveryId::new(),
        workspace_id: WorkspaceId::new(),
        event: EventView {
            kind: event,
            is_resolved: event == DeliveryEvent::Resolved,
            occurrence: i64::from(event == DeliveryEvent::Reminder),
            at: Timestamp(FIRED + 12 * 60_000),
        },
        alarm: Some(AlarmView {
            id: AlarmId::new(),
            title: "Node offline".into(),
            severity,
            state: match event {
                DeliveryEvent::Resolved => AlarmState::Resolved,
                DeliveryEvent::Ended => AlarmState::Ended,
                _ => AlarmState::Firing,
            },
            fired_at: Timestamp(FIRED),
            resolved_at: closed.then_some(Timestamp(FIRED + 11 * 60_000)),
            end_reason: (event == DeliveryEvent::Ended).then(|| "the rule was deleted".into()),
            acknowledged: false,
            acknowledged_at: None,
        }),
        rule: Some(RuleView { id: AlertRuleId::new(), revision: 3 }),
        resource: Some(ResourceView {
            kind: "server".into(),
            id: "s-1".into(),
            name: Some("lab-b".into()),
            server_id: None,
        }),
        observation: Some(ObservationView { subject: subject.into(), value }),
        links: Some(Links { alarm: "https://forge.example.com/alerts?alarm=a-1".into() }),
        message: None,
        after_silence: None,
    }
}

fn offline(event: DeliveryEvent) -> Payload {
    payload(event, Severity::Critical, "node.agent.status", serde_json::json!({"type": "state", "value": "offline"}))
}

const WHY: Why<'static> = Why { policy: "On-call", channel: "Ops mail" };

#[test]
fn titles_names_and_values_are_escaped() {
    let mut p = offline(DeliveryEvent::Fired);
    if let Some(a) = p.alarm.as_mut() {
        a.title = "<script>alert(\"x\")</script> & 'q'".into();
    }
    if let Some(r) = p.resource.as_mut() {
        r.name = Some("<img src=x onerror=alert(1)>\"".into());
    }
    let h = html(&p, Why { policy: "<b>p</b>", channel: "\"c\"" });
    assert!(!h.contains("<script>") && !h.contains("<img src=x") && !h.contains("<b>p</b>"), "{h}");
    assert!(h.contains("&lt;script&gt;alert(&quot;x&quot;)&lt;/script&gt; &amp; &#39;q&#39;"), "{h}");
    assert!(h.contains("&lt;img src=x onerror=alert(1)&gt;&quot;"), "{h}");
    assert!(h.contains("&quot;c&quot;"), "{h}");
    // The only images are the inline brand mark; nothing remote.
    assert_eq!(h.matches("<img ").count(), 1);
    assert!(h.contains(&format!("src=\"cid:{MARK_CID}\"")));
    assert!(!h.contains("http://") && !h.contains("<script"), "{h}");
}

#[test]
fn the_banner_is_coloured_by_event_and_severity() {
    let fired = |s| html(&payload(DeliveryEvent::Fired, s, "node.cpu.percent", serde_json::json!({"value": 97})), WHY);
    for (s, colour) in
        [(Severity::Critical, RED), (Severity::Major, ORANGE), (Severity::Warning, AMBER), (Severity::Info, ACCENT)]
    {
        assert!(fired(s).contains(&format!("background:{colour};padding:10px 24px")), "{s:?}");
    }
    for (event, colour, words) in [
        (DeliveryEvent::Resolved, GREEN, "RESOLVED"),
        (DeliveryEvent::Ended, GREY, "ENDED"),
        (DeliveryEvent::Reminder, RED, "REMINDER"),
    ] {
        let h = html(&offline(event), WHY);
        assert!(h.contains(&format!("background:{colour};padding:10px 24px")), "{event:?}");
        assert!(h.contains(words), "{event:?}");
    }
    let mut p = offline(DeliveryEvent::Fired);
    p.after_silence = Some(AfterSilence { silence_id: SilenceId::new() });
    assert!(html(&p, WHY).contains("sent after a silence ended"));
    let mut t = offline(DeliveryEvent::Test);
    t.alarm = None;
    t.message = Some("Test notification from Orbyn Forge. No alarm fired.".into());
    let h = html(&t, WHY);
    assert!(h.contains(&format!("background:{VIOLET};")) && h.contains("Test notification"), "{h}");
}

#[test]
fn the_explanation_follows_the_subject_family() {
    let num = |n: f64| serde_json::json!({"type": "num", "value": n});
    let state = |s: &str| serde_json::json!({"type": "state", "value": s});
    for (subject, value, want, check) in [
        ("node.agent.status", state("offline"), "Node lab-b has not reported to Forge for 12 minutes", "forge-agent"),
        ("node.cpu.percent", num(97.5), "Node lab-b is reporting high CPU use: 97.5%", "processes"),
        ("node.mem.used_percent", num(91.0), "high memory use: 91.0%", "leak"),
        ("node.disk.used_percent", num(95.0), "high disk use", "filling the disk"),
        ("health.readiness.web", state("failing"), "The health check \u{201c}web\u{201d} on node lab-b", "last result"),
        ("app.instance.status", state("crashed"), "unhealthy (observed app.instance.status: crashed)", "logs"),
        ("package.updates.security", num(4.0), "package problem", "security updates"),
        ("tls.cert.days_left", num(5.0), "A TLS certificate on node lab-b", "renewal"),
        ("node.load", num(9.0), "Node lab-b is reporting node.load: 9.0", "recent metrics"),
        ("custom.thing", num(1.0), "A rule&#39;s condition is met on node lab-b", "recent changes"),
    ] {
        let h = html(&payload(DeliveryEvent::Fired, Severity::Major, subject, value), WHY);
        assert!(h.contains(&escape(want)) || h.contains(want), "{subject}: {want} in {h}");
        assert!(h.contains(check), "{subject}: {check}");
    }
    let h = html(&offline(DeliveryEvent::Resolved), WHY);
    assert!(h.contains("Node lab-b is back to normal") && h.contains("recovered after 11 minutes"), "{h}");
    let h = html(&offline(DeliveryEvent::Ended), WHY);
    assert!(h.contains("without seeing it recover (the rule was deleted)"), "{h}");
    let h = html(&offline(DeliveryEvent::Reminder), WHY);
    assert!(h.contains("has still not reported") && h.contains("Nobody has acknowledged"), "{h}");
}

#[test]
fn details_show_times_rule_and_node() {
    let mut p = offline(DeliveryEvent::Resolved);
    let node = ServerId::new();
    if let Some(r) = p.resource.as_mut() {
        r.kind = "service".into();
        r.server_id = Some(node);
    }
    let h = html(&p, WHY);
    for want in [
        "2025-11-16 09:30:00 UTC",
        "(12 minutes ago)",
        "Closed at",
        "(after 11 minutes)",
        "(revision 3)",
        &node.to_string(),
        "On-call",
        "Ops mail",
    ] {
        assert!(h.contains(want), "{want} in {h}");
    }
    assert_eq!(span(0), "less than a minute");
    assert_eq!(span(65 * 60_000), "1 hour 5 minutes");
    assert_eq!(span((2 * 1440 + 3 * 60) * 60_000), "2 days 3 hours");
}

#[test]
fn links_appear_only_with_a_public_url() {
    let h = html(&offline(DeliveryEvent::Fired), WHY);
    assert!(h.contains("href=\"https://forge.example.com/alerts?alarm=a-1\"") && h.contains("Open alarm in Forge"));
    assert!(h.contains("href=\"https://forge.example.com/alerts?tab=silences\""));
    let mut p = offline(DeliveryEvent::Fired);
    p.links = None;
    assert!(!html(&p, WHY).contains("href="));
    // No public URL configured: the payload's link is relative.
    p.links = Some(Links { alarm: "/alerts?alarm=a-1".into() });
    let h = html(&p, WHY);
    assert!(!h.contains("href=") && !h.contains("Open alarm in Forge"), "{h}");
}

/// Writes one rendered email per event to `target/email-preview/` for a
/// look in a browser: `cargo test -p forge-alerts email_preview -- --ignored`.
#[test]
#[ignore = "writes preview files; run on demand"]
fn email_preview() {
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../target/email-preview");
    std::fs::create_dir_all(&dir).unwrap();
    // A browser cannot resolve `cid:`; point it at the committed PNG.
    let mark = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../apps/forge-server/assets/email/forge-mark-128.png")
        .canonicalize()
        .unwrap();
    let mark = format!("file:///{}", mark.display().to_string().trim_start_matches(r"\\?\").replace('\\', "/"));
    let mut cases = vec![
        ("fired-critical", offline(DeliveryEvent::Fired)),
        (
            "fired-warning-disk",
            payload(
                DeliveryEvent::Fired,
                Severity::Warning,
                "node.disk.used_percent",
                serde_json::json!({"type": "num", "value": 91.4}),
            ),
        ),
        (
            "fired-major-health",
            payload(
                DeliveryEvent::Fired,
                Severity::Major,
                "health.readiness.web",
                serde_json::json!({"type": "state", "value": "failing"}),
            ),
        ),
        (
            "fired-info-cpu",
            payload(
                DeliveryEvent::Fired,
                Severity::Info,
                "node.cpu.percent",
                serde_json::json!({"type": "num", "value": 88}),
            ),
        ),
        ("reminder", offline(DeliveryEvent::Reminder)),
        ("resolved", offline(DeliveryEvent::Resolved)),
        ("ended", offline(DeliveryEvent::Ended)),
    ];
    let mut silenced = offline(DeliveryEvent::Fired);
    silenced.after_silence = Some(AfterSilence { silence_id: SilenceId::new() });
    cases.push(("after-silence", silenced));
    let mut test = offline(DeliveryEvent::Test);
    test.alarm = None;
    test.resource = None;
    test.observation = None;
    test.rule = None;
    test.links = None;
    test.message = Some("Test notification from Orbyn Forge. No alarm fired.".into());
    cases.push(("test", test));
    for (name, p) in cases {
        let h = html(&p, WHY).replace(&format!("cid:{MARK_CID}"), &mark);
        std::fs::write(dir.join(format!("{name}.html")), h).unwrap();
    }
}
