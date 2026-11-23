#![allow(clippy::unwrap_used)] // reason: tests

use std::collections::HashSet;

use super::*;

#[test]
fn every_template_parses_validates_and_names_a_known_subject() {
    let all = templates(None);
    assert!(all.len() >= 20);
    let mut ids = HashSet::new();
    for t in &all {
        assert!(ids.insert(t.id.clone()), "duplicate id {}", t.id);
        t.spec.validate().unwrap_or_else(|e| panic!("{}: {e}", t.id));
        // The JSON form the API stores parses back through the rule parser.
        let json = serde_json::to_value(&t.spec).unwrap();
        assert_eq!(crate::parse_spec(&json).unwrap(), t.spec, "{}", t.id);
        let subject = t.subject_for(Some("http")).unwrap();
        check_subject(&subject, t.scope).unwrap_or_else(|e| panic!("{}: {e}", t.id));
        // State triggers only use states the subject can take.
        let info = describe(&subject).unwrap();
        match &t.spec.trigger {
            Condition::StateIn { states } | Condition::StateNotIn { states } => {
                assert_eq!(info.kind, ValueKind::State, "{}", t.id);
                assert!(states.iter().all(|s| info.states.contains(s)), "{}", t.id);
            }
            Condition::Threshold { .. } => assert_eq!(info.kind, ValueKind::Number, "{}", t.id),
        }
        assert!(!t.name.is_empty() && t.name.chars().count() <= forge_core::alerts::MAX_RULE_NAME);
    }
}

#[test]
fn templates_filter_by_place() {
    let nodes = templates(Some(Place::Node));
    assert!(nodes.iter().all(|t| t.place == Place::Node));
    assert!(nodes.iter().any(|t| t.id == "node-offline"));
    assert_eq!(templates(Some(Place::Any)).len(), templates(None).len());
    assert!(template("database-stopped").is_ok());
    assert!(matches!(template("nope"), Err(DomainError::NotFound(_))));
}

#[test]
fn check_placeholders_need_a_valid_check() {
    let t = template("service-liveness-failing").unwrap();
    assert!(t.needs_check());
    assert_eq!(t.subject_for(Some("http")).unwrap(), "health.liveness.http");
    assert!(t.subject_for(None).is_err());
    assert!(t.subject_for(Some("Bad Name")).is_err());
    assert!(!template("node-offline").unwrap().needs_check());
}

#[test]
fn subjects_are_checked_against_the_catalogue() {
    use ScopeKind::*;
    for (s, scope) in [
        ("node.agent.status", Servers),
        ("node.disk.max_used_percent", Servers),
        ("node.load.per_cpu", Servers),
        ("app.instance.state", Services),
        ("package.state", Installations),
        ("tls.cert.expires_in_days", Certificates),
        ("health.readiness.accept-requests", Services),
        ("health.startup.boot", Services),
        // A newer agent's measurement under a known prefix.
        ("node.gpu.temperature_celsius", Servers),
    ] {
        check_subject(s, scope).unwrap_or_else(|e| panic!("{s}: {e}"));
    }
    let err = |s: &str, scope| check_subject(s, scope).unwrap_err().to_string();
    assert!(err("node.cpu.precent", Servers).contains("did you mean `node.cpu.percent`"));
    assert!(err("package.state", Servers).contains("installations"));
    assert!(err("node.cpu.percent", Services).contains("servers"));
    assert!(err("health.ready.http", Services).contains("health.<readiness|liveness|startup>"));
    assert!(err("app.something.else", Services).contains("unknown subject"));
    assert!(err("disk.used", Servers).contains("unknown subject"));
}

#[test]
fn the_subject_list_covers_patterns_and_control_plane_subjects() {
    let all = subjects();
    for s in CONTROL_PLANE_SUBJECTS {
        let i = all.iter().find(|i| i.subject == *s).unwrap();
        assert_eq!(i.source, SubjectSource::ControlPlane, "{s}");
    }
    assert_eq!(all.iter().filter(|i| i.pattern).count(), 3);
    assert!(all.iter().all(|i| (i.kind == ValueKind::State) != i.unit.is_some()));
}
