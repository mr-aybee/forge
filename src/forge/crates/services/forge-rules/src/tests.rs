#![allow(clippy::unwrap_used)]

use super::*;

const MIN: i64 = 60_000;

fn num(at: i64, v: f64) -> Sample {
    // Evidence for 2 minutes, sampled every minute.
    Sample { at, valid_until: at + 2 * MIN, value: Some(Value::Num(v)) }
}

fn unknown(at: i64) -> Sample {
    Sample { at, valid_until: at + 2 * MIN, value: None }
}

fn state(at: i64, s: &str) -> Sample {
    Sample { at, valid_until: at + 2 * MIN, value: Some(Value::State(s.into())) }
}

fn cpu_sustained() -> RuleSpec {
    // CPU > 85 % for 30 min fires; < 75 % for 5 min clears.
    RuleSpec {
        trigger: Condition::Threshold { cmp: Cmp::Gt, value: 85.0 },
        mode: Mode::Sustained { for_ms: 30 * MIN },
        recovery: Recovery { condition: Some(Condition::Threshold { cmp: Cmp::Lt, value: 75.0 }), hold_ms: 5 * MIN },
    }
}

/// Feeds one sample per minute; returns every event.
fn feed(t: &mut Tracker, spec: &RuleSpec, from_min: i64, values: &[f64]) -> Vec<Event> {
    let mut out = Vec::new();

    for (i, v) in values.iter().enumerate() {
        let at = (from_min + i64::try_from(i).unwrap()) * MIN;
        out.extend(t.observe(spec, num(at, *v), at).unwrap());
    }

    out
}

fn fired(events: &[Event]) -> bool {
    events.iter().any(|e| matches!(e, Event::Fired { .. }))
}

fn resolved(events: &[Event]) -> bool {
    events.iter().any(|e| matches!(e, Event::Resolved { .. }))
}

#[test]
fn sustained_fires_after_the_full_duration_and_clears_with_hysteresis() {
    let spec = cpu_sustained();
    spec.validate().unwrap();
    let mut t = Tracker::new();
    let ev = feed(&mut t, &spec, 0, &[90.0; 30]);
    assert!(ev.iter().any(|e| matches!(e, Event::Pending { at: 0 })), "{ev:?}");
    assert!(!fired(&ev), "29 minutes is not 30");
    let ev = feed(&mut t, &spec, 30, &[90.0]);
    assert!(fired(&ev), "{ev:?}");
    assert!(matches!(t.phase, Phase::Firing { .. }));

    // 80 % is below the trigger but above the recovery threshold: still firing.
    let ev = feed(&mut t, &spec, 31, &[80.0; 10]);
    assert!(!resolved(&ev), "hysteresis: 80 % does not clear");

    // Below 75 % for 5 minutes clears.
    let ev = feed(&mut t, &spec, 41, &[70.0; 5]);
    assert!(!resolved(&ev), "4 minutes of recovery is not 5");
    let ev = feed(&mut t, &spec, 46, &[70.0]);
    assert!(resolved(&ev), "{ev:?}");
    assert_eq!(t.phase, Phase::Normal);
}

#[test]
fn a_dip_restarts_the_sustained_timer() {
    let spec = cpu_sustained();
    let mut t = Tracker::new();
    let mut values = vec![90.0; 20];
    values.push(50.0);
    values.extend([90.0; 25]);
    let ev = feed(&mut t, &spec, 0, &values);
    assert!(ev.iter().any(|e| matches!(e, Event::PendingCancelled { unknown: false, .. })));
    assert!(!fired(&ev), "20 + 25 minutes with a dip between is not 30 in a row");
}

#[test]
fn unknown_interrupts_pending_but_never_resolves_a_firing_alarm() {
    let spec = cpu_sustained();
    let mut t = Tracker::new();
    feed(&mut t, &spec, 0, &[90.0; 10]);

    // Explicitly unknown: pending ends, marked as unknown.
    let ev = t.observe(&spec, unknown(10 * MIN), 10 * MIN).unwrap();
    assert!(ev.iter().any(|e| matches!(e, Event::PendingCancelled { unknown: true, .. })), "{ev:?}");

    // Fire, then the evidence stops: still firing.
    let ev = feed(&mut t, &spec, 11, &[90.0; 31]);
    assert!(fired(&ev));
    let ev = t.tick(&spec, 200 * MIN);
    assert!(ev.is_empty() && matches!(t.phase, Phase::Firing { .. }), "silence is not recovery");

    // Valid recovery evidence returns and holds: resolves.
    let ev = feed(&mut t, &spec, 201, &[10.0; 6]);
    assert!(resolved(&ev));
}

#[test]
fn a_gap_in_the_evidence_breaks_a_sustained_run() {
    let spec = cpu_sustained();
    let mut t = Tracker::new();
    feed(&mut t, &spec, 0, &[90.0; 20]);

    // Nothing for 10 minutes (each sample covers 2): the run restarts.
    let ev = feed(&mut t, &spec, 30, &[90.0; 25]);
    assert!(!fired(&ev), "{ev:?}");
    let ev = feed(&mut t, &spec, 55, &[90.0; 6]);
    assert!(fired(&ev), "30 unbroken minutes after the gap");
}

#[test]
fn stale_evidence_never_extends_a_run() {
    let spec = cpu_sustained();
    let mut t = Tracker::new();
    feed(&mut t, &spec, 0, &[90.0; 5]);

    // Ticking long after the last sample: unknown, no firing.
    assert!(!fired(&t.tick(&spec, 60 * MIN)));
    assert!(!t.known(60 * MIN));
}

#[test]
fn late_duplicate_and_future_samples_are_refused() {
    let spec = cpu_sustained();
    let mut t = Tracker::new();
    t.observe(&spec, num(10 * MIN, 90.0), 10 * MIN).unwrap();
    assert_eq!(t.observe(&spec, num(10 * MIN, 90.0), 10 * MIN), Err(Refused::Late), "duplicate");
    assert_eq!(t.observe(&spec, num(9 * MIN, 90.0), 10 * MIN), Err(Refused::Late), "out of order");
    assert_eq!(t.observe(&spec, num(20 * MIN, 90.0), 10 * MIN), Err(Refused::Future));
    let bad = Sample { at: 11 * MIN, valid_until: 10 * MIN, value: None };
    assert_eq!(t.observe(&spec, bad, 11 * MIN), Err(Refused::Invalid));
}

#[test]
fn window_average_is_time_weighted_and_needs_coverage() {
    // Average CPU over 30 minutes > 80 %, at least 80 % of the window covered.
    let spec = RuleSpec {
        trigger: Condition::Threshold { cmp: Cmp::Gt, value: 80.0 },
        mode: Mode::WindowAverage { window_ms: 30 * MIN, min_coverage_pct: 80 },
        recovery: Recovery::default(),
    };
    spec.validate().unwrap();
    let mut t = Tracker::new();

    // 10 minutes of evidence only: not enough coverage, even at 100 %.
    let ev = feed(&mut t, &spec, 0, &[100.0; 10]);
    assert!(!fired(&ev));

    // A single spike in an otherwise calm window: the average stays low.
    let mut calm = vec![50.0; 24];
    calm.push(100.0);
    let ev = feed(&mut t, &spec, 10, &calm);
    assert!(!fired(&ev), "{ev:?}");

    // High for long enough: the average crosses 80.
    let ev = feed(&mut t, &spec, 35, &[95.0; 30]);
    let value = ev.iter().find_map(|e| match e {
        Event::Fired { value: Some(Value::Num(v)), .. } => Some(*v),
        _ => None,
    });
    assert!(value.is_some_and(|v| v > 80.0), "{ev:?}");

    // Calm again: resolves when the window no longer qualifies.
    let ev = feed(&mut t, &spec, 65, &[20.0; 30]);
    assert!(resolved(&ev));
}

#[test]
fn any_in_window_fires_on_one_spike_and_clears_when_it_leaves_the_window() {
    let spec = RuleSpec {
        trigger: Condition::Threshold { cmp: Cmp::Gt, value: 85.0 },
        mode: Mode::AnyInWindow { window_ms: 30 * MIN },
        recovery: Recovery::default(),
    };
    let mut t = Tracker::new();
    let ev = feed(&mut t, &spec, 0, &[50.0, 99.0, 50.0]);
    assert!(fired(&ev));
    let ev = feed(&mut t, &spec, 3, &[50.0; 27]);
    assert!(!resolved(&ev), "the spike is still in the window");
    let ev = feed(&mut t, &spec, 30, &[50.0; 3]);
    assert!(resolved(&ev), "{ev:?}");
}

#[test]
fn repeated_counts_distinct_breaches_not_samples() {
    // At least 3 breaches in 30 minutes, 5 minutes apart at least.
    let spec = RuleSpec {
        trigger: Condition::Threshold { cmp: Cmp::Gt, value: 85.0 },
        mode: Mode::Repeated { count: 3, window_ms: 30 * MIN, min_spacing_ms: 5 * MIN },
        recovery: Recovery::default(),
    };
    let mut t = Tracker::new();

    // One long breach is one breach, however many samples it has.
    let ev = feed(&mut t, &spec, 0, &[90.0; 10]);
    assert!(!fired(&ev));

    // Two more distinct breaches, spaced out.
    let ev = feed(&mut t, &spec, 10, &[50.0, 90.0, 50.0, 50.0, 50.0, 50.0, 50.0, 90.0]);
    assert!(fired(&ev), "{ev:?}");

    // Flapping faster than the spacing does not count again.
    let mut t2 = Tracker::new();
    let ev = feed(&mut t2, &spec, 0, &[90.0, 50.0, 90.0, 50.0, 90.0, 50.0]);
    assert!(!fired(&ev), "breaches 2 minutes apart count once: {ev:?}");
}

#[test]
fn state_rules_fire_and_recover() {
    let spec = RuleSpec {
        trigger: Condition::StateIn { states: vec!["failing".into()] },
        mode: Mode::Sustained { for_ms: 2 * MIN },
        recovery: Recovery { condition: Some(Condition::StateIn { states: vec!["passing".into()] }), hold_ms: MIN },
    };
    spec.validate().unwrap();
    let mut t = Tracker::new();
    let mut all = Vec::new();

    for (m, s) in [(0, "failing"), (1, "failing"), (2, "failing"), (3, "pending"), (4, "passing"), (5, "passing")] {
        all.extend(t.observe(&spec, state(m * MIN, s), m * MIN).unwrap());
    }

    assert!(fired(&all), "{all:?}");
    assert!(resolved(&all), "pending is not recovery, passing for 1 min is: {all:?}");

    // A number never satisfies a state condition.
    assert!(!spec.trigger.holds(&Value::Num(1.0)));
}

#[test]
fn immediate_fires_on_the_first_matching_sample() {
    let spec = RuleSpec {
        trigger: Condition::StateIn { states: vec!["stopped".into()] },
        mode: Mode::Immediate,
        recovery: Recovery::default(),
    };
    let mut t = Tracker::new();
    assert!(fired(&t.observe(&spec, state(0, "stopped"), 0).unwrap()));
    assert!(resolved(&t.observe(&spec, state(MIN, "running"), MIN).unwrap()));
}

#[test]
fn trackers_survive_a_restart() {
    let spec = cpu_sustained();
    let mut t = Tracker::new();
    feed(&mut t, &spec, 0, &[90.0; 20]);
    let saved = serde_json::to_string(&t).unwrap();
    let mut back: Tracker = serde_json::from_str(&saved).unwrap();
    assert_eq!(back, t);

    // The 20 minutes already seen still count.
    let ev = feed(&mut back, &spec, 20, &[90.0; 11]);
    assert!(fired(&ev));
}

#[test]
fn specs_are_bounded() {
    let mut s = cpu_sustained();
    s.mode = Mode::Sustained { for_ms: 25 * 3600 * 1000 };
    assert!(s.validate().is_err());
    s.mode = Mode::WindowAverage { window_ms: MIN, min_coverage_pct: 0 };
    assert!(s.validate().is_err());
    s.trigger = Condition::Threshold { cmp: Cmp::Gt, value: f64::NAN };
    assert!(s.validate().is_err());
    let states = RuleSpec {
        trigger: Condition::StateIn { states: vec![] },
        mode: Mode::Immediate,
        recovery: Recovery::default(),
    };
    assert!(states.validate().is_err());
    let avg_on_state = RuleSpec {
        trigger: Condition::StateIn { states: vec!["x".into()] },
        mode: Mode::WindowAverage { window_ms: MIN, min_coverage_pct: 50 },
        recovery: Recovery::default(),
    };
    assert!(avg_on_state.validate().is_err());
    assert_eq!(cpu_sustained().describe(), "> 85 for 30 min");
}
