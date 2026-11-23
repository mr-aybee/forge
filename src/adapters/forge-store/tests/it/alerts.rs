//! Alert rules, alarms and evaluator state (#11, D-087).

use forge_core::alerts::*;
use forge_core::*;
use forge_store::Store;
use serde_json::json;

async fn store() -> (Store, tempfile::TempDir) {
    let dir = tempfile::tempdir().unwrap();
    let s = Store::open_sqlite(&dir.path().join("forge.db")).await.unwrap();
    (s, dir)
}

fn rule(ws: WorkspaceId) -> AlertRule {
    AlertRule {
        id: AlertRuleId::new(),
        workspace_id: ws,
        name: "CPU high".into(),
        description: String::new(),
        enabled: true,
        severity: Severity::Major,
        scope: RuleScope::Servers { ids: vec![] },
        subject: "node.cpu.percent".into(),
        spec: json!({"trigger": {"kind": "threshold", "cmp": "gt", "value": 85.0}}),
        revision: 1,
        created_by: Principal::User(UserId::new()),
        created_at: Timestamp(1),
        updated_at: Timestamp(1),
    }
}

fn alarm(r: &AlertRule, fp: &str, at: i64) -> Alarm {
    Alarm {
        id: AlarmId::new(),
        workspace_id: r.workspace_id,
        rule_id: r.id,
        rule_revision: r.revision,
        title: r.name.clone(),
        severity: r.severity,
        fingerprint: fp.into(),
        resource: EntityRef::new(EntityKind::Server, ServerId::new()),
        server: None,
        subject: r.subject.clone(),
        state: AlarmState::Firing,
        fired_value: json!({"type": "num", "value": 91.0}),
        fired_at: Timestamp(at),
        resolved_at: None,
        end_reason: None,
        acknowledged_by: None,
        acknowledged_at: None,
        previous: None,
    }
}

fn ev(a: &Alarm, kind: &str, at: i64) -> AlarmEvent {
    AlarmEvent { alarm_id: a.id, at: Timestamp(at), kind: kind.into(), actor: None, detail: json!({}) }
}

#[tokio::test]
async fn rules_round_trip_and_update() {
    let (s, _d) = store().await;
    let ws = WorkspaceId::new();
    let mut r = rule(ws);
    s.create_rule(&r).await.unwrap();
    assert_eq!(s.get_rule(r.id).await.unwrap(), r);
    r.revision = 2;
    r.enabled = false;
    r.scope = RuleScope::Services { ids: vec!["a".into()] };
    s.update_rule(&r).await.unwrap();
    assert_eq!(s.list_rules(Some(ws)).await.unwrap(), vec![r.clone()]);
    assert!(s.list_rules(Some(WorkspaceId::new())).await.unwrap().is_empty());
    s.delete_rule(r.id).await.unwrap();
    assert!(matches!(s.get_rule(r.id).await, Err(DomainError::NotFound(_))));
}

#[tokio::test]
async fn one_active_alarm_per_series_and_history_after_it() {
    let (s, _d) = store().await;
    let r = rule(WorkspaceId::new());
    let a = alarm(&r, "fp-1", 10);
    s.open_alarm(&a, &ev(&a, "fired", 10)).await.unwrap();
    // A second firing alarm for the same series is refused by the store.
    let dup = alarm(&r, "fp-1", 11);
    assert!(matches!(s.open_alarm(&dup, &ev(&dup, "fired", 11)).await, Err(DomainError::Conflict(_))));
    assert_eq!(s.active_alarm("fp-1").await.unwrap().map(|x| x.id), Some(a.id));
    // Acknowledge does not resolve.
    let who = Principal::User(UserId::new());
    s.acknowledge(a.id, Some(&who), Timestamp(12), &ev(&a, "acknowledged", 12)).await.unwrap();
    let got = s.get_alarm(a.id).await.unwrap();
    assert_eq!((got.state, got.acknowledged_at), (AlarmState::Firing, Some(Timestamp(12))));
    // Resolve once; a second close is a no-op.
    assert!(s.close_alarm(a.id, AlarmState::Resolved, Timestamp(20), None, &ev(&a, "resolved", 20)).await.unwrap());
    assert!(!s.close_alarm(a.id, AlarmState::Ended, Timestamp(21), Some("x"), &ev(&a, "ended", 21)).await.unwrap());
    assert!(s.active_alarm("fp-1").await.unwrap().is_none());
    // A recurrence is a new episode linked to the last one.
    let mut again = alarm(&r, "fp-1", 30);
    again.previous = s.last_alarm("fp-1").await.unwrap().map(|x| x.id);
    s.open_alarm(&again, &ev(&again, "fired", 30)).await.unwrap();
    assert_eq!(s.get_alarm(again.id).await.unwrap().previous, Some(a.id));
    let kinds: Vec<String> = s.alarm_events(a.id).await.unwrap().into_iter().map(|e| e.kind).collect();
    assert_eq!(kinds, ["fired", "acknowledged", "resolved"]);
    // Filters.
    let f = AlarmFilter { workspaces: Some(vec![r.workspace_id]), active: true, ..AlarmFilter::default() };
    assert_eq!(s.list_alarms(&f, 50).await.unwrap().len(), 1);
    let f = AlarmFilter { workspaces: Some(vec![r.workspace_id]), ..AlarmFilter::default() };
    assert_eq!(s.list_alarms(&f, 50).await.unwrap().len(), 2);
    let f = AlarmFilter { workspaces: Some(vec![WorkspaceId::new()]), ..AlarmFilter::default() };
    assert!(s.list_alarms(&f, 50).await.unwrap().is_empty(), "other workspaces see nothing");
    let f = AlarmFilter { workspaces: Some(vec![]), ..AlarmFilter::default() };
    assert!(s.list_alarms(&f, 50).await.unwrap().is_empty());
}

#[tokio::test]
async fn trackers_and_cursors_persist() {
    let (s, _d) = store().await;
    let rule = AlertRuleId::new();
    assert_eq!(s.cursor("feed").await.unwrap(), 0);
    s.set_cursor("feed", 42).await.unwrap();
    s.set_cursor("feed", 43).await.unwrap();
    assert_eq!(s.cursor("feed").await.unwrap(), 43);
    s.save_trackers(&[("a".into(), rule, 1, json!({"x": 1})), ("b".into(), rule, 1, json!({}))], Timestamp(1))
        .await
        .unwrap();
    s.save_trackers(&[("a".into(), rule, 2, json!({"x": 2}))], Timestamp(2)).await.unwrap();
    let mut t = s.trackers().await.unwrap();
    t.sort_by(|x, y| x.0.cmp(&y.0));
    assert_eq!(t, vec![("a".into(), 2, json!({"x": 2})), ("b".into(), 1, json!({}))]);
    s.delete_trackers(&["b".into()]).await.unwrap();
    assert_eq!(s.trackers().await.unwrap().len(), 1);
}

/// Review of #23: pages continue after the last alarm seen, ties included.
#[tokio::test]
async fn alarm_pages_follow_a_cursor() {
    let (s, _d) = store().await;
    let r = rule(WorkspaceId::new());
    let mut ids = Vec::new();
    for i in 0..5 {
        // Two alarms share each fired_at, so the id breaks ties.
        let a = alarm(&r, &format!("fp-{i}"), 100 + i / 2);
        s.open_alarm(&a, &ev(&a, "fired", a.fired_at.0)).await.unwrap();
        ids.push(a.id);
    }
    let mut f = AlarmFilter { workspaces: Some(vec![r.workspace_id]), ..AlarmFilter::default() };
    let mut seen = Vec::new();
    loop {
        let page = s.list_alarms(&f, 2).await.unwrap();
        if page.is_empty() {
            break;
        }
        f.before = page.last().map(|a| (a.fired_at, a.id));
        seen.extend(page.into_iter().map(|a| a.id));
    }
    assert_eq!(seen.len(), 5, "every alarm once");
    ids.sort();
    seen.sort();
    assert_eq!(seen, ids);
}
