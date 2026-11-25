#![allow(clippy::unwrap_used)]

use serde_json::json;

use super::*;

fn doc(spec: Value) -> Value {
    json!({
        "apiVersion": "forge/v1",
        "kind": "Deployment",
        "metadata": { "name": "web", "environment": "prod" },
        "spec": spec,
    })
}

const DIGEST: &str = "sha256:0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";

fn container() -> Value {
    json!({ "runtime": "container", "artifact": { "image": "ghcr.io/acme/web:1.4.2", "digest": DIGEST },
            "template": { "ports": ["8080:80"] } })
}

#[test]
fn durations_parse() {
    assert_eq!(Secs::parse("30s").unwrap(), Secs(30));
    assert_eq!(Secs::parse("5m").unwrap(), Secs(300));
    assert_eq!(Secs::parse("2h").unwrap(), Secs(7_200));
    assert_eq!(Secs::parse("1d").unwrap(), Secs(86_400));
    assert_eq!(Secs::parse("45").unwrap(), Secs(45));
    assert!(Secs::parse("soon").is_err());
    let s: StrategySpec = serde_json::from_value(
        json!({ "type": "canary", "steps": [{ "weight": 10, "pause": "5m" }, { "weight": 100, "pause": 60 }] }),
    )
    .unwrap();
    assert_eq!(s.steps[0].pause, Secs(300));
    assert_eq!(s.steps[1].pause, Secs(60));
}

#[test]
fn a_document_parses_and_carries_its_policy() {
    let d = parse(doc(container()), None).unwrap();
    assert_eq!(d.metadata.name, "web");
    let p = d.policy().unwrap();
    assert_eq!((p.strategy, p.replacement, p.batch_size), (Strategy::Rolling, Replacement::StartFirst, 1));
    assert!(p.auto_rollback && p.slotted());
    let a = d.spec.artifact.resolve(d.spec.runtime).unwrap();
    assert_eq!(a.image_ref().unwrap(), format!("ghcr.io/acme/web:1.4.2@{DIGEST}"));
}

#[test]
fn typos_and_bad_values_are_refused_with_a_path() {
    let mut v = doc(container());
    v["spec"]["stratgy"] = json!({});
    assert!(parse(v, None).unwrap_err().message.contains("stratgy"));

    let mut v = doc(container());
    v["apiVersion"] = json!("forge/v2");
    assert_eq!(parse(v, None).unwrap_err().path, "apiVersion");

    let mut v = doc(container());
    v["spec"]["artifact"]["digest"] = json!("sha256:nothex");
    assert_eq!(parse(v, None).unwrap_err().path, "spec.artifact.digest");

    let mut v = doc(container());
    v["spec"]["artifact"]["git"] = json!({ "repo": "https://x/y.git", "commit": "a".repeat(40) });
    assert_eq!(parse(v, None).unwrap_err().path, "spec.artifact", "two artifacts");

    let mut v = doc(container());
    v["spec"]["template"]["env"] = json!([["A", "b"]]);
    assert!(parse(v, None).unwrap_err().path.starts_with("spec.template"));

    let mut v = doc(container());
    v["spec"]["strategy"] = json!({ "type": "canary", "steps": [{ "weight": 50 }, { "weight": 20 }] });
    assert!(parse(v, None).unwrap_err().path.contains("steps[1]"));

    let mut v = doc(container());
    v["spec"]["secrets"] = json!(["API_KEY"]);
    v["spec"]["env"] = json!({ "API_KEY": "oops" });
    assert_eq!(parse(v, None).unwrap_err().path, "spec.secrets");
}

#[test]
fn native_artifacts_need_pinned_sources() {
    let git = |commit: &str| {
        doc(
            json!({ "runtime": "systemd", "artifact": { "git": { "repo": "https://github.com/acme/api.git", "commit": commit } } }),
        )
    };
    assert!(parse(git(&"ab".repeat(20)), None).is_ok());
    assert!(parse(git("main"), None).is_err(), "a branch is not pinned");
    let archive = |url: &str, sha: &str| {
        doc(json!({ "runtime": "systemd", "artifact": { "archive": { "url": url, "sha256": sha } } }))
    };
    assert!(
        parse(archive("https://github.com/acme/api/releases/download/v1/api.tar.gz", &"0".repeat(64)), None).is_ok()
    );
    assert!(parse(archive("http://insecure/api.tgz", &"0".repeat(64)), None).is_err());
    assert!(parse(archive("https://x/api.tgz", "short"), None).is_err());
    let image_on_service = doc(json!({ "runtime": "systemd", "artifact": { "image": "nginx:1" } }));
    assert!(parse(image_on_service, None).is_err());
}

#[test]
fn overrides_apply_to_their_environment() {
    let mut v = doc(container());
    v["spec"]["overrides"] = json!({ "staging": { "strategy": { "type": "recreate" } } });
    assert_eq!(parse(v.clone(), None).unwrap().spec.strategy.kind, Strategy::Rolling);
    let d = parse(v, Some("staging")).unwrap();
    assert_eq!(d.metadata.environment, "staging");
    assert_eq!(d.spec.strategy.kind, Strategy::Recreate);
}

#[test]
fn snake_keys_converts_camel_case() {
    assert_eq!(snake("listenPort"), "listen_port");
    assert_eq!(snake("cpu_millis"), "cpu_millis");
    assert_eq!(
        snake_keys(json!({ "periodSecs": 5, "probe": { "expectedStatuses": [] } })),
        json!({ "period_secs": 5, "probe": { "expected_statuses": [] } })
    );
}

fn spec(runtime: &str) -> crate::AppSpec {
    serde_json::from_value(json!({
        "environment": "prod", "name": "web", "runtime": runtime, "command": "x",
        "image": if runtime == "container" { "nginx:1" } else { "" },
        "ports": if runtime == "container" { json!(["8080:80", "127.0.0.1:9090:9000/udp"]) } else { json!([]) },
        "listen_port": if runtime == "systemd" { json!(8000) } else { json!(null) },
        "health": { "readiness": [ { "name": "http", "probe": { "type": "http", "port": 8080, "path": "/ready" } } ] },
    }))
    .unwrap()
}

#[test]
fn a_slot_has_its_own_names_and_ports() {
    let mut s = spec("container");
    let map = into_slot(&mut s, Slot::B, &[21_000, 21_001]).unwrap();
    assert_eq!(s.unit(), "forge-app-prod-web--b");
    assert_eq!(s.env_path(), "/etc/forge/apps/prod/web--b.env");
    assert_eq!(s.ports, vec!["21000:80".to_owned(), "127.0.0.1:21001:9000/udp".to_owned()]);
    assert_eq!(map, BTreeMap::from([(8080, 21_000), (9090, 21_001)]));

    match &s.health.unwrap().readiness[0].probe {
        crate::health::Probe::Http(h) => assert_eq!(h.port, 21_000, "probes follow the slot"),
        other => panic!("{other:?}"),
    }

    s = spec("systemd");
    into_slot(&mut s, Slot::A, &[22_000]).unwrap();
    assert_eq!((s.listen_port, s.launcher_file().as_str()), (Some(22_000), "run--a.sh"));
    assert!(s.env.contains(&("PORT".to_owned(), "22000".to_owned())));
    assert!(s.validate().is_ok());
    let mut bad = spec("systemd");
    bad.slot = "c".into();
    assert!(bad.validate().is_err());
}

fn node(id: &str, serving: Option<Option<Slot>>) -> PlanNode {
    PlanNode { server_id: id.into(), name: id.into(), online: true, serving }
}

fn input(policy: RolloutPolicy, nodes: Vec<PlanNode>, routed: bool) -> PlanInput {
    PlanInput { nodes, policy, runtime: Runtime::Container, routed, has_ports: true }
}

fn policy(strategy: Strategy, replacement: Replacement) -> RolloutPolicy {
    RolloutPolicy { strategy, replacement, allow_downtime: false, ..RolloutPolicy::default() }
}

fn ops(p: &Plan) -> Vec<&'static str> {
    p.steps
        .iter()
        .map(|s| match s {
            Step::Start { .. } => "start",
            Step::Gate { .. } => "gate",
            Step::Route { .. } => "route",
            Step::Wait { .. } => "wait",
            Step::Approve { .. } => "approve",
            Step::Stop { .. } => "stop",
        })
        .collect()
}

#[test]
fn start_first_rolls_node_by_node_into_the_other_slot() {
    let nodes = vec![node("n1", Some(Some(Slot::A))), node("n2", Some(Some(Slot::A)))];
    let p = plan(&input(policy(Strategy::Rolling, Replacement::StartFirst), nodes, true)).unwrap();
    assert!(p.slotted && !p.downtime);
    assert_eq!(ops(&p), ["start", "gate", "route", "wait", "stop", "start", "gate", "route", "wait", "stop"]);
    assert_eq!(p.targets[0].slot, Some(Slot::B));
    let Step::Route { weights } = &p.steps[2] else { panic!() };
    assert_eq!(weights.len(), 2, "new slot in, old slot out on n1 only");
    assert_eq!(
        (weights[0].slot, weights[0].weight, weights[1].slot, weights[1].weight),
        (Some(Slot::B), 100, Some(Slot::A), 0)
    );
}

#[test]
fn the_first_slotted_deploy_moves_off_the_unslotted_instance() {
    let p =
        plan(&input(policy(Strategy::BlueGreen, Replacement::StartFirst), vec![node("n1", Some(None))], true)).unwrap();
    assert_eq!(ops(&p), ["start", "gate", "route", "wait", "stop"]);
    let Step::Stop { targets } = p.steps.last().unwrap() else { panic!() };
    assert_eq!(targets[0].slot, None, "the old single instance is stopped");
}

#[test]
fn side_by_side_copies_need_an_endpoint() {
    let e = plan(&input(policy(Strategy::Rolling, Replacement::StartFirst), vec![node("n1", Some(None))], false))
        .unwrap_err();
    assert!(e.0.contains("endpoint"), "{e}");
}

#[test]
fn stop_first_on_one_node_needs_accepted_downtime() {
    let one = || vec![node("n1", Some(None))];
    let e = plan(&input(policy(Strategy::Rolling, Replacement::StopFirst), one(), false)).unwrap_err();
    assert!(e.0.contains("allowDowntime"), "{e}");
    let mut ok = policy(Strategy::Rolling, Replacement::StopFirst);
    ok.allow_downtime = true;
    let p = plan(&input(ok, one(), false)).unwrap();
    assert!(p.downtime && !p.slotted);
    assert_eq!(ops(&p), ["start", "gate"]);
    assert_eq!(p.targets[0].slot, None, "in place");

    // A first deploy has nothing to take down.
    assert!(plan(&input(policy(Strategy::Rolling, Replacement::StopFirst), vec![node("n1", None)], false)).is_ok());
}

#[test]
fn stop_first_with_two_nodes_drains_each_before_replacing_it() {
    let nodes = vec![node("n1", Some(Some(Slot::A))), node("n2", Some(Some(Slot::A)))];
    let p = plan(&input(policy(Strategy::Rolling, Replacement::StopFirst), nodes, true)).unwrap();
    assert!(!p.downtime);

    // n1 back in and n2 out are one route.
    assert_eq!(ops(&p), ["route", "wait", "start", "gate", "route", "wait", "start", "gate", "route"]);
    assert_eq!(p.targets[0].slot, Some(Slot::A), "in the slot that serves now");
    let Step::Route { weights } = &p.steps[0] else { panic!() };
    assert_eq!((weights[0].server_id.as_str(), weights[0].weight), ("n1", 0));
    let Step::Route { weights } = &p.steps[4] else { panic!() };
    let w: Vec<(&str, u32)> = weights.iter().map(|w| (w.server_id.as_str(), w.weight)).collect();
    assert_eq!(w, [("n1", 100), ("n2", 0)], "merging keeps n1's way back in");
}

#[test]
fn canary_shifts_weights_and_gates_each_step() {
    let mut pol = policy(Strategy::Canary, Replacement::StartFirst);
    pol.steps = vec![
        CanaryStep {
            weight: 10,
            pause: Secs(300),
            approval: false,
            analysis: Some(Analysis { max_error_rate: Some(1), max_p95_ms: None, min_requests: 20 }),
        },
        CanaryStep { weight: 50, pause: Secs(0), approval: true, analysis: None },
    ];
    pol.approval = true;
    let p = plan(&input(pol, vec![node("n1", Some(Some(Slot::B)))], true)).unwrap();
    assert_eq!(ops(&p), ["start", "gate", "approve", "route", "wait", "route", "approve", "route", "wait", "stop"]);
    let Step::Route { weights } = &p.steps[3] else { panic!() };
    assert_eq!((weights[0].slot, weights[0].weight, weights[1].weight), (Some(Slot::A), 10, 90));
}

#[test]
fn a_first_canary_takes_all_traffic() {
    let mut pol = policy(Strategy::Canary, Replacement::StartFirst);
    pol.steps = vec![CanaryStep { weight: 10, pause: Secs(60), approval: false, analysis: None }];
    let p = plan(&input(pol, vec![node("n1", None)], true)).unwrap();
    assert_eq!(ops(&p), ["start", "gate", "route"]);
}

#[test]
fn manual_waits_before_every_batch_and_offline_nodes_refuse() {
    let nodes = vec![node("n1", Some(None)), node("n2", Some(None)), node("n3", Some(None))];
    let p = plan(&input(policy(Strategy::Manual, Replacement::StartFirst), nodes.clone(), true)).unwrap();
    assert_eq!(ops(&p).iter().filter(|o| **o == "approve").count(), 3);
    let mut off = nodes;
    off[1].online = false;
    assert!(
        plan(&input(policy(Strategy::Rolling, Replacement::StartFirst), off, true)).unwrap_err().0.contains("offline")
    );
}

#[test]
fn recreate_replaces_everything_at_once() {
    let nodes = vec![node("n1", Some(None)), node("n2", Some(None))];
    let p = plan(&input(policy(Strategy::Recreate, Replacement::StartFirst), nodes, false)).unwrap();
    assert!(p.downtime && !p.slotted);
    assert_eq!(ops(&p), ["start", "gate"]);
    let Step::Start { targets } = &p.steps[0] else { panic!() };
    assert_eq!(targets.len(), 2);
}
