#![allow(clippy::unwrap_used)]

use serde_json::json;

use super::*;

fn doc(spec: Value) -> Value {
    json!({
        "apiVersion": "forge/v1",
        "kind": "Stack",
        "metadata": { "name": "orders", "environment": "prod" },
        "spec": spec,
    })
}

fn err(v: Value) -> String {
    parse(v, None).unwrap_err().to_string()
}

#[test]
fn a_minimal_document_takes_the_defaults() {
    let d = parse(doc(json!({ "compose": { "file": "compose.yaml" } })), None).unwrap();
    assert_eq!(d.metadata.name, "orders");
    assert_eq!(d.spec.compose.file.as_deref(), Some("compose.yaml"));
    assert!(d.spec.network.mesh);
    assert_eq!(d.spec.rollout.order, RolloutOrder::NodeByNode);
    assert_eq!(d.health_timeout_s(), DEFAULT_HEALTH_TIMEOUT);
    assert!(d.spec.placement.nodes.is_empty());
}

#[test]
fn every_field_reads() {
    let d = parse(
        doc(json!({
            "compose": { "inline": "services: {}\n", "envFile": "prod.env" },
            "placement": { "cluster": "c1" },
            "network": { "mesh": false },
            "rollout": { "order": "all-at-once", "healthTimeout": "90s" },
        })),
        None,
    )
    .unwrap();
    assert_eq!(d.spec.placement.cluster.as_deref(), Some("c1"));
    assert!(!d.spec.network.mesh);
    assert_eq!(d.spec.rollout.order, RolloutOrder::AllAtOnce);
    assert_eq!(d.health_timeout_s(), 90);
    assert_eq!(d.spec.compose.env_file.as_deref(), Some("prod.env"));
}

#[test]
fn unknown_fields_are_refused_with_their_path() {
    assert!(
        err(json!({ "apiVersion": "forge/v1", "kind": "Stack", "metadata": {}, "spec": {}, "x": 1 }))
            .starts_with("x: unknown field")
    );
    let mut v = doc(json!({ "compose": { "file": "c.yaml", "url": "x" } }));
    assert!(err(v.clone()).starts_with("spec.compose.url: unknown field"), "{}", err(v.clone()));
    v["spec"] = json!({ "compose": { "file": "c.yaml" }, "rollout": { "speed": 1 } });
    assert!(err(v.clone()).starts_with("spec.rollout.speed:"));
    v["metadata"]["labels"] = json!({});
    assert!(err(v).starts_with("metadata.labels:"));
}

#[test]
fn types_and_rules_name_the_field() {
    assert!(
        err(doc(json!({ "compose": { "file": "c.yaml" }, "network": { "mesh": "no" } }))).starts_with("spec.network:")
    );
    assert!(err(doc(json!({ "compose": { "file": "../c.yaml" } }))).starts_with("spec.compose.file:"));
    assert!(err(doc(json!({ "compose": { "file": "/etc/c.yaml" } }))).starts_with("spec.compose.file:"));
    assert!(err(doc(json!({ "compose": { "file": "c.yaml", "inline": "x" } }))).starts_with("spec.compose:"));
    assert!(err(doc(json!({ "compose": {} }))).starts_with("spec.compose:"));
    assert!(
        err(doc(json!({ "compose": { "file": "c.yaml", "envFile": "../.env" } }))).starts_with("spec.compose.envFile:")
    );
    assert!(
        err(doc(json!({ "compose": { "file": "c" }, "placement": { "nodes": ["a"], "cluster": "c" } })))
            .starts_with("spec.placement:")
    );
    assert!(
        err(doc(json!({ "compose": { "file": "c" }, "rollout": { "order": "random" } }))).starts_with("spec.rollout:")
    );
    assert!(err(doc(json!({}))).starts_with("spec.compose: missing"));

    let mut v = doc(json!({ "compose": { "file": "c" } }));
    v["kind"] = json!("Deployment");
    assert!(err(v.clone()).starts_with("kind: must be Stack"));
    v["kind"] = json!("Stack");
    v["metadata"]["name"] = json!("Orders!");
    assert!(err(v).starts_with("metadata.name:"));
}

#[test]
fn health_timeout_is_between_ten_seconds_and_an_hour() {
    let t = |h: Value| parse(doc(json!({ "compose": { "file": "c" }, "rollout": { "healthTimeout": h } })), None);
    assert_eq!(t(json!("10s")).unwrap().health_timeout_s(), 10);
    assert_eq!(t(json!("1h")).unwrap().health_timeout_s(), 3600);
    assert_eq!(t(json!(600)).unwrap().health_timeout_s(), 600);
    assert!(t(json!("9s")).unwrap_err().to_string().starts_with("spec.rollout.healthTimeout: between 10s and 1h"));
    assert!(t(json!("2h")).is_err());
}

#[test]
fn overrides_merge_for_the_chosen_environment_only() {
    let v = doc(json!({
        "compose": { "file": "compose.yaml", "envFile": "dev.env" },
        "placement": { "nodes": ["lab-a"] },
        "rollout": { "healthTimeout": "2m" },
        "overrides": {
            "prod": {
                "placement": { "nodes": null, "cluster": "c1" },
                "rollout": { "order": "all-at-once" },
                "compose": { "envFile": "prod.env" },
            },
            "staging": { "network": { "mesh": false } },
        },
    }));
    let prod = parse(v.clone(), None).unwrap();
    assert_eq!(prod.spec.placement.cluster.as_deref(), Some("c1"));
    assert!(prod.spec.placement.nodes.is_empty());
    assert_eq!(prod.spec.rollout.order, RolloutOrder::AllAtOnce);
    assert_eq!(prod.health_timeout_s(), 120, "deep merge keeps what the override leaves out");
    assert_eq!(prod.spec.compose.env_file.as_deref(), Some("prod.env"));
    assert_eq!(prod.spec.compose.file.as_deref(), Some("compose.yaml"));

    let staging = parse(v, Some("staging")).unwrap();
    assert_eq!(staging.metadata.environment, "staging");
    assert!(!staging.spec.network.mesh);
    assert_eq!(staging.spec.placement.nodes, ["lab-a"]);
    assert_eq!(staging.spec.compose.env_file.as_deref(), Some("dev.env"));
}

#[test]
fn overrides_may_only_change_placement_network_rollout_and_env_file() {
    let o = |o: Value| err(doc(json!({ "compose": { "file": "c" }, "overrides": { "prod": o } })));
    assert!(
        o(json!({ "compose": { "file": "other.yaml" } }))
            .starts_with("spec.overrides.prod.compose.file: unknown field")
    );
    assert!(o(json!({ "overrides": {} })).starts_with("spec.overrides.prod.overrides: unknown field"));
    assert!(o(json!({ "rollout": { "x": 1 } })).starts_with("spec.overrides.prod.rollout.x:"));
    assert!(o(json!("x")).starts_with("spec.overrides.prod: must be a mapping"));
}

#[test]
fn several_nodes_need_per_service_placement() {
    let d = parse(doc(json!({ "compose": { "file": "c" }, "placement": { "nodes": ["a", "b"] } })), None).unwrap();
    let plain = json!({ "services": { "web": { "image": "nginx" } } });
    assert_eq!(d.check_compose(&plain).unwrap_err().to_string(), format!("spec.placement.nodes: {SEVERAL_NODES}"));
    let placed = json!({ "services": { "web": { "image": "nginx", "x-forge": { "node": "b" } } } });
    assert!(d.check_compose(&placed).is_ok());
    let one = parse(doc(json!({ "compose": { "file": "c" }, "placement": { "nodes": ["a"] } })), None).unwrap();
    assert!(one.check_compose(&plain).is_ok());
}

#[test]
fn the_overlay_adds_cluster_and_mesh_to_the_compose_file() {
    let d = parse(
        doc(json!({ "compose": { "file": "c" }, "placement": { "cluster": "c1" }, "network": { "mesh": false } })),
        None,
    )
    .unwrap();
    let plain = json!({ "services": {} });
    let o = d.compose_overlay(&plain).unwrap().unwrap();
    assert!(o.contains("x-forge:\n  cluster: \"c1\"\n  mesh: false\n"), "{o}");
    let agrees = json!({ "x-forge": { "cluster": "c1", "mesh": false } });
    assert_eq!(d.compose_overlay(&agrees).unwrap(), None);
    let differs = json!({ "x-forge": { "cluster": "c2" } });
    assert!(d.compose_overlay(&differs).unwrap_err().to_string().starts_with("spec.placement.cluster:"));
    let none = parse(doc(json!({ "compose": { "file": "c" } })), None).unwrap();
    assert_eq!(none.compose_overlay(&plain).unwrap(), None);

    // The placement reads the top-level cluster for services without one.
    let tree = json!({ "x-forge": { "cluster": "c1" }, "services": { "a": {}, "b": { "x-forge": { "node": "n" } } } });
    let p = crate::stacks::placements(&tree).unwrap();
    assert_eq!(p["a"], crate::stacks::Placement::Cluster("c1".into()));
    assert_eq!(p["b"], crate::stacks::Placement::Node("n".into()));
}
