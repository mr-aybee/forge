#![allow(clippy::unwrap_used)]

use std::time::Duration;

use forge_core::alerts::AlertRule;
use forge_core::model::{AgentStatus, Cluster, Environment, NodeRole, Server, Workspace};
use forge_core::ports::{ClusterRepository, ServerRepository, TenancyRepository};
use forge_core::{AlarmId, ClusterId, EnvironmentId, ManualClock, ServerId, UserId};
use forge_store::Store;
use serde_json::json;

use super::*;

const MIN: i64 = 60 * 1000;
const HOUR: i64 = 60 * MIN;
const DAY: i64 = 24 * HOUR;

fn alarm(ws: WorkspaceId, resource: EntityRef, severity: Severity) -> Alarm {
    Alarm {
        id: AlarmId::new(),
        workspace_id: ws,
        rule_id: AlertRuleId::new(),
        rule_revision: 1,
        title: "CPU high".into(),
        severity,
        fingerprint: format!("fp-{}", AlarmId::new()),
        resource,
        server: None,
        subject: "node.cpu.percent".into(),
        state: AlarmState::Firing,
        fired_value: json!({"type": "num", "value": 91.0}),
        fired_at: Timestamp(0),
        resolved_at: None,
        end_reason: None,
        acknowledged_by: None,
        acknowledged_at: None,
        previous: None,
    }
}

fn silence(ws: WorkspaceId, scope: SilenceScope) -> Silence {
    Silence {
        id: SilenceId::new(),
        workspace_id: ws,
        scope,
        rules: vec![],
        min_severity: None,
        max_severity: None,
        starts_at: Timestamp(0),
        ends_at: Timestamp(HOUR),
        reason: "maintenance".into(),
        created_by: Principal::User(UserId::new()),
        created_at: Timestamp(0),
        updated_at: Timestamp(0),
        ended_by: None,
    }
}

fn node(id: ServerId) -> EntityRef {
    EntityRef::new(EntityKind::Server, id)
}

fn app(id: &str) -> EntityRef {
    EntityRef::new(EntityKind::Service, id)
}

// ---- matching ----------------------------------------------------------------

#[test]
fn a_workspace_silence_covers_every_alarm_of_its_workspace_only() {
    let ws = WorkspaceId::new();
    let s = silence(ws, SilenceScope::Workspace);
    let place = AlarmPlace::default();
    assert!(silence_matches(&s, &alarm(ws, node(ServerId::new()), Severity::Critical), &place));
    assert!(silence_matches(&s, &alarm(ws, app("a1"), Severity::Info), &place));
    assert!(!silence_matches(&s, &alarm(WorkspaceId::new(), node(ServerId::new()), Severity::Info), &place));
}

#[test]
fn node_and_service_scopes_cover_what_they_name() {
    let ws = WorkspaceId::new();
    let (n1, n2) = (ServerId::new(), ServerId::new());
    let place = AlarmPlace::default();
    let on_nodes = silence(ws, SilenceScope::Servers { ids: vec![n1] });
    assert!(silence_matches(&on_nodes, &alarm(ws, node(n1), Severity::Major), &place));
    assert!(!silence_matches(&on_nodes, &alarm(ws, node(n2), Severity::Major), &place));
    // A service instance on a silenced node is not covered: the
    // service may sit in an environment the silencer holds no role on.
    let mut instance = alarm(ws, app("a1"), Severity::Major);
    instance.server = Some(n1);
    assert!(!silence_matches(&on_nodes, &instance, &place));
    // A package installation sits under its node, so the node's silence
    // covers it (D-118).
    let mut package = alarm(ws, EntityRef::new(EntityKind::Installation, "i1"), Severity::Major);
    package.server = Some(n1);
    assert!(silence_matches(&on_nodes, &package, &place));
    package.server = Some(n2);
    assert!(!silence_matches(&on_nodes, &package, &place));

    let on_apps = silence(ws, SilenceScope::Services { ids: vec!["a1".into()] });
    assert!(silence_matches(&on_apps, &alarm(ws, app("a1"), Severity::Major), &place));
    assert!(!silence_matches(&on_apps, &alarm(ws, app("a2"), Severity::Major), &place));
    assert!(!silence_matches(&on_apps, &alarm(ws, node(n1), Severity::Major), &place));
}

#[test]
fn cluster_and_environment_scopes_follow_where_the_resource_sits() {
    let ws = WorkspaceId::new();
    let (cluster, other_cluster) = (ClusterId::new(), ClusterId::new());
    let (ns, other_ns) = (EnvironmentId::new(), EnvironmentId::new());
    let a = alarm(ws, node(ServerId::new()), Severity::Major);
    let in_both = AlarmPlace {
        ancestors: vec![EntityRef::new(EntityKind::Environment, ns), EntityRef::new(EntityKind::Cluster, cluster)],
    };
    assert!(silence_matches(&silence(ws, SilenceScope::Cluster { id: cluster }), &a, &in_both));
    assert!(!silence_matches(&silence(ws, SilenceScope::Cluster { id: other_cluster }), &a, &in_both));
    assert!(silence_matches(&silence(ws, SilenceScope::Environment { id: ns }), &a, &in_both));
    assert!(!silence_matches(&silence(ws, SilenceScope::Environment { id: other_ns }), &a, &in_both));
    // A service instance on a node of the cluster, in an environment
    // that is not under the cluster: not covered.
    let mut instance = alarm(ws, app("a1"), Severity::Major);
    instance.server = Some(ServerId::new());
    let app_place = AlarmPlace { ancestors: vec![EntityRef::new(EntityKind::Environment, other_ns)] };
    assert!(!silence_matches(&silence(ws, SilenceScope::Cluster { id: cluster }), &instance, &app_place));
    // A resource that no longer resolves has no place: neither covers it.
    let nowhere = AlarmPlace::default();
    assert!(!silence_matches(&silence(ws, SilenceScope::Cluster { id: cluster }), &a, &nowhere));
    assert!(!silence_matches(&silence(ws, SilenceScope::Environment { id: ns }), &a, &nowhere));
}

#[test]
fn rules_and_severity_bounds_narrow_a_silence() {
    let ws = WorkspaceId::new();
    let place = AlarmPlace::default();
    let a = alarm(ws, node(ServerId::new()), Severity::Major);

    let mut by_rule = silence(ws, SilenceScope::Workspace);
    by_rule.rules = vec![a.rule_id];
    assert!(silence_matches(&by_rule, &a, &place));
    by_rule.rules = vec![AlertRuleId::new()];
    assert!(!silence_matches(&by_rule, &a, &place));

    // Up to major: a critical alarm is not silenced.
    let mut capped = silence(ws, SilenceScope::Workspace);
    capped.max_severity = Some(Severity::Major);
    for (severity, covered) in
        [(Severity::Info, true), (Severity::Warning, true), (Severity::Major, true), (Severity::Critical, false)]
    {
        let a = alarm(ws, node(ServerId::new()), severity);
        assert_eq!(silence_matches(&capped, &a, &place), covered, "{severity:?} with max major");
    }
    // From warning up: an info alarm is not silenced.
    let mut floored = silence(ws, SilenceScope::Workspace);
    floored.min_severity = Some(Severity::Warning);
    assert!(!silence_matches(&floored, &alarm(ws, node(ServerId::new()), Severity::Info), &place));
    assert!(silence_matches(&floored, &alarm(ws, node(ServerId::new()), Severity::Critical), &place));
}

// ---- the service -------------------------------------------------------------

struct Env {
    store: Arc<Store>,
    clock: Arc<ManualClock>,
    s: Silences,
    ws: WorkspaceId,
    node: ServerId,
    cluster: ClusterId,
    ns: EnvironmentId,
    who: Principal,
    _dir: tempfile::TempDir,
}

fn server(ws: WorkspaceId, ns: Option<EnvironmentId>, cluster: Option<ClusterId>) -> Server {
    Server {
        id: ServerId::new(),
        workspace_id: ws,
        environment_id: ns,
        cluster_id: cluster,
        role: NodeRole::Worker,
        display_name: None,
        hostname: "web-01".into(),
        os: "linux".into(),
        arch: "x86_64".into(),
        agent_version: "0".into(),
        status: AgentStatus::Online,
        last_seen_at: None,
        cert_serial: None,
        cert_not_after: None,
        created_at: Timestamp(1),
    }
}

async fn env() -> Env {
    let dir = tempfile::tempdir().unwrap();
    let store = Arc::new(Store::open_sqlite(&dir.path().join("forge.db")).await.unwrap());
    let clock = Arc::new(ManualClock::new(Timestamp(1_000 * DAY)));
    let ws = Workspace { id: WorkspaceId::new(), name: "Acme".into(), slug: "acme".into(), created_at: Timestamp(1) };
    store.create_workspace(&ws).await.unwrap();
    let cluster = Cluster {
        id: ClusterId::new(),
        workspace_id: ws.id,
        name: "edge".into(),
        description: String::new(),
        environment_id: None,
        abbr: String::new(),
        icon: None,
        created_at: Timestamp(1),
    };
    store.create_cluster(&cluster).await.unwrap();
    let ns = Environment {
        id: EnvironmentId::new(),
        workspace_id: ws.id,
        project_id: None,
        cluster_id: Some(cluster.id),
        name: "prod".into(),
        protected: false,
        created_at: Timestamp(1),
    };
    store.create_environment(&ns).await.unwrap();
    let node = server(ws.id, Some(ns.id), Some(cluster.id));
    store.create_server(&node).await.unwrap();
    let s = Silences { store: store.clone(), alerts: store.clone(), resolver: store.clone(), clock: clock.clone() };
    Env {
        store,
        clock,
        s,
        ws: ws.id,
        node: node.id,
        cluster: cluster.id,
        ns: ns.id,
        who: Principal::User(UserId::new()),
        _dir: dir,
    }
}

impl Env {
    fn now(&self) -> Timestamp {
        self.clock.now()
    }

    fn advance(&self, ms: i64) {
        self.clock.advance(Duration::from_millis(u64::try_from(ms).unwrap()));
    }

    fn input(&self, scope: SilenceScope) -> SilenceInput {
        SilenceInput {
            scope,
            rules: vec![],
            min_severity: None,
            max_severity: None,
            starts_at: None,
            ends_at: Timestamp(self.now().0 + 2 * HOUR),
            reason: "Kernel upgrade".into(),
            manages_workspace: false,
        }
    }

    async fn create(&self, input: SilenceInput) -> DomainResult<Silence> {
        self.s.create(self.ws, input, self.who.clone()).await
    }

    fn invalid(r: DomainResult<Silence>, field: &str) {
        match r {
            Err(DomainError::Invalid(m)) => assert!(m.starts_with(field), "{m}"),
            other => panic!("expected an invalid {field}, got {other:?}"),
        }
    }
}

#[tokio::test]
async fn a_silence_starts_now_or_later_and_its_state_follows_the_clock() {
    let e = env().await;
    let now_one = e.create(e.input(SilenceScope::Workspace)).await.unwrap();
    assert_eq!((now_one.starts_at, now_one.ends_at.0 - now_one.starts_at.0), (e.now(), 2 * HOUR));
    assert_eq!(now_one.state(e.now()), SilenceState::Active);
    assert_eq!(now_one.created_by, e.who);

    // A maintenance window: the same object, starting later.
    let mut later = e.input(SilenceScope::Servers { ids: vec![e.node] });
    later.starts_at = Some(Timestamp(e.now().0 + DAY));
    later.ends_at = Timestamp(e.now().0 + DAY + 4 * HOUR);
    let window = e.create(later).await.unwrap();
    assert_eq!(window.state(e.now()), SilenceState::Scheduled);
    assert_eq!(e.s.get(window.id).await.unwrap(), window, "round trip");

    let ids = |list: Vec<Silence>| list.into_iter().map(|s| s.id).collect::<Vec<_>>();
    assert_eq!(ids(e.s.list(e.ws, None, None, 100).await.unwrap()), [window.id, now_one.id], "latest start first");
    assert_eq!(ids(e.s.list(e.ws, Some(SilenceState::Active), None, 100).await.unwrap()), [now_one.id]);
    assert_eq!(ids(e.s.list(e.ws, Some(SilenceState::Scheduled), None, 100).await.unwrap()), [window.id]);
    assert!(e.s.list(e.ws, Some(SilenceState::Expired), None, 100).await.unwrap().is_empty());
    // The next page starts after the last one seen.
    assert_eq!(ids(e.s.list(e.ws, None, Some((window.starts_at, window.id)), 100).await.unwrap()), [now_one.id]);

    // Time passes: the first expires, the window opens.
    e.advance(DAY + HOUR);
    assert_eq!(ids(e.s.list(e.ws, Some(SilenceState::Expired), None, 100).await.unwrap()), [now_one.id]);
    assert_eq!(ids(e.s.list(e.ws, Some(SilenceState::Active), None, 100).await.unwrap()), [window.id]);
}

#[tokio::test]
async fn a_silence_needs_a_reason_a_bounded_window_and_things_that_exist() {
    let e = env().await;
    let ws = || e.input(SilenceScope::Workspace);

    Env::invalid(e.create(SilenceInput { reason: "  ".into(), ..ws() }).await, "reason");
    Env::invalid(e.create(SilenceInput { reason: "x".repeat(501), ..ws() }).await, "reason");
    Env::invalid(e.create(SilenceInput { reason: "bell\u{7}".into(), ..ws() }).await, "reason");

    let at = |ms: i64| Timestamp(e.now().0 + ms);
    Env::invalid(e.create(SilenceInput { ends_at: at(-1), ..ws() }).await, "ends_at");
    Env::invalid(e.create(SilenceInput { ends_at: at(30 * 1000), ..ws() }).await, "ends_at");
    Env::invalid(e.create(SilenceInput { ends_at: at(30 * DAY + 1), ..ws() }).await, "ends_at");
    assert!(e.create(SilenceInput { ends_at: at(30 * DAY), ..ws() }).await.is_ok(), "30 days is allowed");
    // The bound is on the length, so a scheduled window may end later.
    let far = SilenceInput { starts_at: Some(at(10 * DAY)), ends_at: at(40 * DAY), ..ws() };
    assert!(e.create(far).await.is_ok());
    Env::invalid(e.create(SilenceInput { starts_at: Some(at(-HOUR)), ..ws() }).await, "starts_at");
    Env::invalid(
        e.create(SilenceInput { starts_at: Some(at(366 * DAY)), ends_at: at(367 * DAY), ..ws() }).await,
        "starts_at",
    );
    // A start a moment ago (a caller's clock slightly behind) means now.
    let s = e.create(SilenceInput { starts_at: Some(at(-MIN)), ..ws() }).await.unwrap();
    assert_eq!(s.starts_at, e.now());

    let severities =
        SilenceInput { min_severity: Some(Severity::Major), max_severity: Some(Severity::Warning), ..ws() };
    Env::invalid(e.create(severities).await, "max_severity");

    // Scopes name things of this workspace.
    Env::invalid(e.create(e.input(SilenceScope::Servers { ids: vec![] })).await, "scope");
    Env::invalid(e.create(e.input(SilenceScope::Servers { ids: vec![ServerId::new()] })).await, "scope");
    Env::invalid(e.create(e.input(SilenceScope::Cluster { id: ClusterId::new() })).await, "scope");
    Env::invalid(e.create(e.input(SilenceScope::Environment { id: EnvironmentId::new() })).await, "scope");
    Env::invalid(e.create(e.input(SilenceScope::Services { ids: vec!["nope".into()] })).await, "scope");
    let many = (0..=MAX_SILENCE_SCOPE_IDS).map(|_| ServerId::new()).collect();
    Env::invalid(e.create(e.input(SilenceScope::Servers { ids: many })).await, "scope");
    // A node of another workspace reads like one that does not exist.
    let other =
        Workspace { id: WorkspaceId::new(), name: "Other".into(), slug: "other".into(), created_at: Timestamp(1) };
    e.store.create_workspace(&other).await.unwrap();
    let foreign = server(other.id, None, None);
    e.store.create_server(&foreign).await.unwrap();
    Env::invalid(e.create(e.input(SilenceScope::Servers { ids: vec![foreign.id] })).await, "scope");
    for scope in [
        SilenceScope::Servers { ids: vec![e.node] },
        SilenceScope::Cluster { id: e.cluster },
        SilenceScope::Environment { id: e.ns },
    ] {
        assert!(e.create(e.input(scope.clone())).await.is_ok(), "{scope:?}");
    }

    // Rules must exist in the workspace.
    Env::invalid(e.create(SilenceInput { rules: vec![AlertRuleId::new()], ..ws() }).await, "rules");
    let rule = AlertRule {
        id: AlertRuleId::new(),
        workspace_id: e.ws,
        name: "CPU high".into(),
        description: String::new(),
        enabled: true,
        severity: Severity::Major,
        scope: forge_core::alerts::RuleScope::Servers { ids: vec![] },
        subject: "node.cpu.percent".into(),
        spec: json!({}),
        revision: 1,
        created_by: e.who.clone(),
        created_at: e.now(),
        updated_at: e.now(),
    };
    e.store.create_rule(&rule).await.unwrap();
    let s = e.create(SilenceInput { rules: vec![rule.id, rule.id], ..ws() }).await.unwrap();
    assert_eq!(s.rules, vec![rule.id], "deduplicated");
}

#[tokio::test]
async fn a_workspace_holds_a_bounded_number_of_open_silences() {
    let mut e = env().await;
    // One person holds a bounded number.
    for _ in 0..MAX_OPEN_SILENCES_PER_PERSON {
        e.create(e.input(SilenceScope::Workspace)).await.unwrap();
    }
    assert!(matches!(e.create(e.input(SilenceScope::Workspace)).await, Err(DomainError::Conflict(_))));
    // Others fill the workspace up to its cap.
    let per = usize::try_from(MAX_OPEN_SILENCES_PER_PERSON).unwrap();
    let total = usize::try_from(MAX_OPEN_SILENCES).unwrap();
    for _ in 0..(total / per - 1) {
        e.who = Principal::User(UserId::new());
        for _ in 0..per {
            e.create(e.input(SilenceScope::Workspace)).await.unwrap();
        }
    }
    e.who = Principal::User(UserId::new());
    assert!(matches!(e.create(e.input(SilenceScope::Workspace)).await, Err(DomainError::Conflict(_))));
    // A workspace manager can still silence during an incident.
    let admin = SilenceInput { manages_workspace: true, ..e.input(SilenceScope::Workspace) };
    assert!(e.create(admin).await.is_ok());
    // Ended silences do not count.
    e.advance(3 * HOUR);
    assert!(e.create(e.input(SilenceScope::Workspace)).await.is_ok());
}

#[tokio::test]
async fn a_silence_is_extended_and_ended_but_never_after_it_expired() {
    let e = env().await;
    let s = e.create(e.input(SilenceScope::Workspace)).await.unwrap();

    // Extending moves the end later; never earlier, never past 30 days.
    let later = Timestamp(s.ends_at.0 + HOUR);
    let extended = e.s.extend(s.id, later).await.unwrap();
    assert_eq!((extended.starts_at, extended.ends_at, extended.ended_by.clone()), (s.starts_at, later, None));
    assert!(matches!(e.s.extend(s.id, s.ends_at).await, Err(DomainError::Invalid(_))), "not later");
    assert!(matches!(e.s.extend(s.id, Timestamp(s.starts_at.0 + 30 * DAY + 1)).await, Err(DomainError::Invalid(_))));
    assert!(e.s.extend(s.id, Timestamp(s.starts_at.0 + 30 * DAY)).await.is_ok());

    // Ending early: it stays, expired, with who ended it.
    e.advance(10 * MIN);
    let other = Principal::User(UserId::new());
    let ended = e.s.end(s.id, other.clone()).await.unwrap();
    assert_eq!((ended.ends_at, ended.ended_by.clone()), (e.now(), Some(other)));
    assert_eq!(ended.state(e.now()), SilenceState::Expired);
    assert_eq!(e.s.get(s.id).await.unwrap(), ended, "kept as history");
    // An expired silence never changes.
    assert!(matches!(e.s.end(s.id, e.who.clone()).await, Err(DomainError::Conflict(_))));
    assert!(matches!(e.s.extend(s.id, Timestamp(e.now().0 + HOUR)).await, Err(DomainError::Conflict(_))));
    assert_eq!(e.s.get(s.id).await.unwrap(), ended);

    // A scheduled window ended before it began never takes effect.
    let mut window = e.input(SilenceScope::Workspace);
    window.starts_at = Some(Timestamp(e.now().0 + DAY));
    window.ends_at = Timestamp(e.now().0 + DAY + HOUR);
    let window = e.create(window).await.unwrap();
    let cancelled = e.s.end(window.id, e.who.clone()).await.unwrap();
    assert_eq!((cancelled.starts_at, cancelled.ends_at), (e.now(), e.now()));
    e.advance(DAY + 30 * MIN);
    assert_eq!(e.s.get(window.id).await.unwrap().state(e.now()), SilenceState::Expired);
    assert!(matches!(e.s.get(SilenceId::new()).await, Err(DomainError::NotFound(_))));
}

#[tokio::test]
async fn alarms_show_the_active_silence_that_covers_them() {
    let e = env().await;
    let on_node = alarm(e.ws, node(e.node), Severity::Major);
    let elsewhere = alarm(e.ws, node(ServerId::new()), Severity::Major);
    let mut closed = alarm(e.ws, node(e.node), Severity::Major);
    closed.state = AlarmState::Resolved;
    let alarms = [on_node.clone(), elsewhere.clone(), closed];
    assert_eq!(e.s.marks(&alarms).await.unwrap(), [None, None, None], "no silence yet");

    // A scheduled window covers nothing until it starts.
    let mut window = e.input(SilenceScope::Environment { id: e.ns });
    window.starts_at = Some(Timestamp(e.now().0 + HOUR));
    window.ends_at = Timestamp(e.now().0 + 3 * HOUR);
    let window = e.create(window).await.unwrap();
    assert_eq!(e.s.marks(&alarms).await.unwrap(), [None, None, None]);

    // Through the node's environment (resolved from the store): the node's
    // firing alarm is marked; an alarm elsewhere and a closed one are not.
    e.advance(HOUR);
    let marks = e.s.marks(&alarms).await.unwrap();
    let mark = SilenceMark { silence_id: window.id, until: window.ends_at, reason: "Kernel upgrade".into() };
    assert_eq!(marks, [Some(mark), None, None]);

    // The cluster covers the node too; of two silences the later end shows.
    let longer =
        SilenceInput { ends_at: Timestamp(e.now().0 + 5 * HOUR), ..e.input(SilenceScope::Cluster { id: e.cluster }) };
    let longer = e.create(longer).await.unwrap();
    assert_eq!(e.s.covering_one(&on_node, e.now()).await.unwrap().map(|s| s.id), Some(longer.id));
    assert_eq!(e.s.covering_one(&elsewhere, e.now()).await.unwrap(), None);

    // Both ended: nothing is marked.
    e.s.end(longer.id, e.who.clone()).await.unwrap();
    e.advance(3 * HOUR);
    assert_eq!(e.s.marks(&alarms).await.unwrap(), [None, None, None]);
}

#[tokio::test]
async fn old_silences_are_pruned_once_nothing_names_them() {
    let e = env().await;
    let s = e.create(e.input(SilenceScope::Workspace)).await.unwrap();
    e.advance(40 * DAY);
    let fresh = e.create(e.input(SilenceScope::Workspace)).await.unwrap();
    assert_eq!(e.s.prune(Timestamp(e.now().0 - 30 * DAY)).await.unwrap(), 1);
    assert!(matches!(e.s.get(s.id).await, Err(DomainError::NotFound(_))));
    assert!(e.s.get(fresh.id).await.is_ok());
}
