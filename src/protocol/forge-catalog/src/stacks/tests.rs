use super::*;

fn policy() -> Policy {
    Policy {
        allow_privileged: false,
        bind_roots: vec!["/srv/data".into()],
        stack_dir: "/opt/forge/stacks/web/shop".into(),
    }
}

fn blocked(tree: &Value) -> Vec<String> {
    check(tree, &policy()).blocking(false).iter().map(|f| f.message.clone()).collect()
}

fn svc(extra: Value) -> Value {
    let mut s = json!({ "image": "nginx:1.27" });
    if let (Some(m), Value::Object(e)) = (s.as_object_mut(), extra) {
        m.extend(e);
    }

    json!({ "services": { "web": s } })
}

#[test]
fn a_plain_stack_passes() {
    let t = json!({
        "services": {
            "web": { "image": "nginx", "ports": ["8080:80"], "volumes": ["data:/var/lib/x", "./conf:/etc/nginx/conf.d:ro", "/srv/data/site:/usr/share/nginx/html"], "depends_on": ["db"] },
            "db": { "image": "postgres:16", "environment": { "POSTGRES_PASSWORD": "x" }, "env_file": [".env"] }
        },
        "volumes": { "data": {} },
        "x-common": { "a": 1 }
    });
    let r = check(&t, &policy());
    assert!(r.blocking(false).is_empty(), "{:?}", r.findings);
    assert!(!r.privileged);
}

#[test]
fn host_takeover_settings_need_privileged() {
    for extra in [
        json!({ "privileged": true }),
        json!({ "privileged": "true" }),
        json!({ "network_mode": "host" }),
        json!({ "network_mode": "container:other" }),
        json!({ "pid": "host" }),
        json!({ "ipc": "host" }),
        json!({ "userns_mode": "host" }),
        json!({ "cap_add": ["SYS_ADMIN"] }),
        json!({ "devices": ["/dev/sda:/dev/sda"] }),
        json!({ "security_opt": ["seccomp:unconfined"] }),
        json!({ "security_opt": ["apparmor=unconfined"] }),
        json!({ "security_opt": ["label:disable"] }),
        json!({ "sysctls": { "net.ipv4.ip_forward": 1 } }),
        json!({ "cgroup_parent": "/" }),
        json!({ "runtime": "runc-custom" }),
        json!({ "volumes": ["/var/run/docker.sock:/var/run/docker.sock"] }),
        json!({ "volumes": ["/:/host"] }),
        json!({ "volumes": ["/etc:/host-etc:ro"] }),
        json!({ "volumes": ["/srv/other:/x"] }),
        json!({ "volumes": [{ "type": "bind", "source": "/root", "target": "/r" }] }),
        json!({ "volumes_from": ["container:db"] }),
        json!({ "deploy": { "resources": { "reservations": { "devices": [{ "capabilities": ["gpu"] }] } } } }),
    ] {
        let t = svc(extra.clone());
        let r = check(&t, &policy());
        assert!(r.privileged, "{extra} should need stack.privileged");
        assert!(!r.blocking(false).is_empty(), "{extra}");
        assert!(r.blocking(true).is_empty(), "{extra} should pass with stack.privileged: {:?}", r.findings);
    }
}

#[test]
fn some_things_are_never_allowed() {
    for extra in [
        json!({ "build": "." }),
        json!({ "extends": { "file": "x.yml", "service": "a" } }),
        json!({ "env_file": "/etc/shadow" }),
        json!({ "env_file": ["../other/.env"] }),
        json!({ "volumes": ["../../etc:/x"] }),
        json!({ "volumes": ["~/.ssh:/x"] }),
        json!({ "volumes": [{ "type": "npipe", "source": "x", "target": "/x" }] }),
    ] {
        let r = check(&svc(extra.clone()), &policy());
        assert!(!r.blocking(true).is_empty(), "{extra} must be blocked even with stack.privileged");
    }

    let r = check(&json!({ "include": ["a.yml"], "services": { "a": { "image": "x" } } }), &policy());
    assert!(!r.blocking(true).is_empty());
    assert!(!blocked(&json!({ "services": { "a": {} } })).is_empty(), "image required");
    assert!(!blocked(&json!({ "services": {} })).is_empty());
    assert!(!blocked(&json!({ "foo": 1, "services": { "a": { "image": "x" } } })).is_empty());
}

#[test]
fn disguised_binds_are_caught() {
    let t = json!({
        "services": { "a": { "image": "x", "volumes": ["h:/h"] } },
        "volumes": { "h": { "driver": "local", "driver_opts": { "type": "none", "o": "bind", "device": "/" } } }
    });
    assert!(check(&t, &policy()).privileged);
    let ok = json!({
        "services": { "a": { "image": "x", "volumes": ["h:/h"] } },
        "volumes": { "h": { "driver_opts": { "type": "none", "o": "bind", "device": "/srv/data/h" } } }
    });
    assert!(check(&ok, &policy()).blocking(false).is_empty());
    let host_net = json!({ "services": { "a": { "image": "x" } }, "networks": { "n": { "driver": "host" } } });
    assert!(check(&host_net, &policy()).privileged);
    let secret = json!({ "services": { "a": { "image": "x" } }, "secrets": { "s": { "file": "/etc/shadow" } } });
    assert!(check(&secret, &policy()).privileged);
}

#[test]
fn approved_roots_respect_component_boundaries() {
    assert!(within("/srv/data/x", "/srv/data"));
    assert!(within("/srv/data", "/srv/data"));
    assert!(!within("/srv/database", "/srv/data"));
    assert_eq!(normalize_abs("/a//b/./c/"), Some("/a/b/c".into()));
    assert_eq!(normalize_abs("/a/../b"), None);
    assert_eq!(normalize_abs("a/b"), None);
    let t = svc(json!({ "volumes": ["/srv/database:/x"] }));
    assert!(check(&t, &policy()).privileged);

    // The stack's own folder is fine for relative binds.
    let t = svc(json!({ "volumes": ["./data:/x", ".:/y:ro"] }));
    assert!(check(&t, &policy()).findings.is_empty());
}

#[test]
fn host_paths_are_listed_for_the_node_recheck() {
    let t = json!({
        "services": { "a": { "image": "x", "volumes": ["./d:/d", "named:/n", "/srv/data/x:/x", { "type": "bind", "source": "/srv/y", "target": "/y" }, "/anon"] } },
        "volumes": { "v": { "driver_opts": { "device": "/srv/data/v", "o": "bind" } } },
        "secrets": { "s": { "file": "./s.txt" } }
    });
    let dir = "/opt/forge/stacks/web/shop";
    assert_eq!(
        host_paths(&t, dir),
        vec![format!("{dir}/d"), "/srv/data/x".into(), "/srv/y".into(), "/srv/data/v".into(), format!("{dir}/s.txt")]
    );

    // A symlink inside an approved root that points at /etc is caught.
    assert!(check_path("/etc", &policy()).privileged);
    assert!(check_path("/srv/data/ok", &policy()).findings.is_empty());
}

#[test]
fn env_files_parse_and_refuse_compose_keys() {
    let e = parse_env("# c\nA=1\nexport B = \"two words\"\nC='x # y'\nD=v # comment\n\n").unwrap();
    assert_eq!(e["A"], "1");
    assert_eq!(e["B"], "two words");
    assert_eq!(e["C"], "x # y");
    assert_eq!(e["D"], "v");
    assert!(parse_env("COMPOSE_FILE=/etc/x.yml").is_err());
    assert!(parse_env("docker_host=tcp://x").is_err());
    assert!(parse_env("1A=x").is_err());
    assert!(parse_env("novalue").is_err());
    assert_eq!(parse_env(&render_env(&e)).unwrap(), e);
}

#[test]
fn interpolation_follows_compose() {
    let env = BTreeMap::from([("A".to_owned(), "1".to_owned()), ("E".to_owned(), String::new())]);
    let i = |s: &str| interpolate_str(s, &env);
    assert_eq!(i("${A}/$A/$$A").unwrap(), "1/1/$A");
    assert_eq!(i("${E:-d}|${E-d}|${U-d}|${U:-${A}}").unwrap(), "d||d|1");
    assert_eq!(i("${A:+set}|${E:+set}|${E+set}").unwrap(), "set||set");
    assert!(i("${U:?needed}").is_err());
    assert!(i("${E:?needed}").is_err());
    assert_eq!(i("${E?needed}").unwrap(), "");
    assert!(i("${A").is_err());
    assert_eq!(i("cost $5 é").unwrap(), "cost $5 é");

    // A variable cannot smuggle a host path past the check.
    let env = BTreeMap::from([("D".to_owned(), "/".to_owned())]);
    let t = interpolate(&svc(json!({ "volumes": ["${D}:/host"] })), &env).unwrap();
    assert!(check(&t, &policy()).privileged);
}

#[test]
fn ports_bind_internally_unless_public() {
    let mut t = json!({ "services": {
        "web": { "image": "x", "ports": ["8080:80", "443:443/tcp", "0.0.0.0:9000:9000", "127.0.0.2:81:81", 53, "[::1]:82:82", { "target": 3000, "published": "3000" }], "x-forge": { "public": [443] } },
        "dns": { "image": "y", "ports": ["5353:53/udp"], "x-forge": { "public": true } }
    }});
    let mut r = Report::default();
    let ports = bind_ports(&mut t, "10.8.0.2", &mut r);
    assert_eq!(
        t.pointer("/services/web/ports").unwrap(),
        &json!(["10.8.0.2:8080:80/tcp", "0.0.0.0:443:443/tcp", "10.8.0.2:9000:9000/tcp", "127.0.0.2:81:81/tcp", "10.8.0.2::53/tcp", "[::1]:82:82/tcp", { "target": 3000, "published": "3000", "host_ip": "10.8.0.2" }])
    );
    assert_eq!(t.pointer("/services/dns/ports/0").unwrap(), "0.0.0.0:5353:53/udp");
    assert_eq!(ports.iter().filter(|p| p.public).count(), 2);
    assert!(r.findings.iter().any(|f| f.severity == Severity::Info && f.message.contains("9000")));
    let mut bad = json!({ "services": { "a": { "image": "x", "ports": ["a:b:c:d"] } } });
    let mut r = Report::default();
    bind_ports(&mut bad, "127.0.0.1", &mut r);
    assert!(!r.blocking(true).is_empty());
    let mut v6 = json!({ "services": { "a": { "image": "x", "ports": ["80"] } } });
    bind_ports(&mut v6, "fd00::2", &mut Report::default());
    assert_eq!(v6.pointer("/services/a/ports/0").unwrap(), "[fd00::2]::80/tcp");
}

#[test]
fn placement_reads_constraints_and_x_forge() {
    let t = json!({ "services": {
        "a": { "image": "x", "deploy": { "placement": { "constraints": ["node.hostname == lab-b"] } } },
        "b": { "image": "x", "x-forge": { "node": "lab-c" } },
        "c": { "image": "x", "x-forge": { "cluster": "lab" } },
        "d": { "image": "x" }
    }});
    let p = placements(&t).unwrap();
    assert_eq!(p["a"], Placement::Node("lab-b".into()));
    assert_eq!(p["b"], Placement::Node("lab-c".into()));
    assert_eq!(p["c"], Placement::Cluster("lab".into()));
    assert_eq!(p["d"], Placement::Default);
    let bad = json!({ "services": { "a": { "image": "x", "deploy": { "placement": { "constraints": ["node.role == manager"] } } } } });
    assert!(placements(&bad).is_err());
    let both = json!({ "services": { "a": { "image": "x", "x-forge": { "node": "a", "cluster": "b" } } } });
    assert!(placements(&both).is_err());
}

#[test]
fn split_keeps_only_what_each_node_needs() {
    let t = json!({
        "name": "shop",
        "services": {
            "web": { "image": "nginx", "depends_on": { "api": { "condition": "service_healthy" } }, "networks": ["front"], "x-forge": { "node": "a" }, "labels": ["x=1"] },
            "api": { "image": "api", "networks": ["front", "back"], "volumes": ["cache:/c"], "deploy": { "placement": { "constraints": ["node.hostname==a"] }, "resources": { "limits": { "cpus": "1" } } } },
            "db": { "image": "pg", "volumes": ["pg:/var/lib/postgresql/data"], "networks": ["back"], "secrets": ["pw"] }
        },
        "networks": { "front": {}, "back": {}, "default": {} },
        "volumes": { "cache": {}, "pg": {} },
        "secrets": { "pw": { "environment": "PW" } }
    });
    let a = part_for(&t, &BTreeSet::from(["web".to_owned(), "api".to_owned()]), "ns/shop").unwrap();
    assert_eq!(a["services"].as_object().unwrap().len(), 2);
    assert!(a.pointer("/services/web/x-forge").is_none());
    assert!(a.pointer("/services/api/deploy/placement").is_none());
    assert_eq!(a.pointer("/services/api/deploy/resources/limits/cpus").unwrap(), "1");
    assert_eq!(a.pointer("/services/web/labels").unwrap(), &json!(["x=1", "forge.stack=ns/shop"]));
    assert_eq!(a.pointer("/services/api/labels/forge.stack").unwrap(), "ns/shop");
    assert_eq!(a["volumes"], json!({ "cache": {} }));
    assert_eq!(a["networks"], json!({ "front": {}, "back": {}, "default": {} }));
    assert!(a.get("secrets").is_none());
    let b = part_for(&t, &BTreeSet::from(["db".to_owned()]), "ns/shop").unwrap();
    assert_eq!(b["secrets"], json!({ "pw": { "environment": "PW" } }));

    // web depends on api: splitting them apart is an error.
    assert!(part_for(&t, &BTreeSet::from(["web".to_owned()]), "ns/shop").is_err());
    assert_eq!(with_dependencies(&t, &["web".to_owned()]), BTreeSet::from(["web".to_owned(), "api".to_owned()]));
}

#[test]
fn public_ports_survive_the_node_split() {
    // Regression: part_for dropped all of `x-forge`, so `x-forge.public`
    // never reached bind_ports and every port stayed on the internal address.
    let t = json!({
        "services": {
            "seq": {
                "image": "datalust/seq",
                "ports": ["5380:80", "5341:5341"],
                "x-forge": { "node": "lab-b", "public": [5380] }
            }
        }
    });
    let mut part = part_for(&t, &BTreeSet::from(["seq".to_owned()]), "web/seq").unwrap();
    assert!(part.pointer("/services/seq/x-forge/node").is_none(), "placement is spent");
    assert_eq!(part.pointer("/services/seq/x-forge/public/0").unwrap(), 5380);
    let mut r = Report::default();
    let ports = bind_ports(&mut part, "127.0.0.1", &mut r);
    let ui = ports.iter().find(|p| p.published == "5380").unwrap();
    assert!(ui.public);
    assert_eq!(ui.host_ip, "0.0.0.0");
    let ingest = ports.iter().find(|p| p.published == "5341").unwrap();
    assert!(!ingest.public);
    assert_eq!(ingest.host_ip, "127.0.0.1");
}

#[test]
fn yaml_output_round_trips_through_json_rules() {
    let t = json!({
        "services": { "web": {
            "image": "nginx:1.27", "command": ["sh", "-c", "echo \"hi\" $$HOME"], "environment": { "ON": "on", "V": "1.10", "N": "" },
            "ports": ["127.0.0.1:80:80/tcp", { "target": 1, "host_ip": "::1" }], "read_only": true, "x": null, "tmpfs": [], "e": {}
        } },
        "on": "yes", "1": 2
    });
    let y = to_yaml(&t);
    assert!(y.contains("    image: \"nginx:1.27\"\n"), "{y}");
    assert!(y.contains("\"on\": \"yes\"\n"), "{y}");
    assert!(y.contains("\"1\": 2\n"), "{y}");
    assert!(y.contains("      - \"sh\"\n"), "{y}");

    // Key order depends on serde_json's `preserve_order` (feature unification).
    assert!(
        y.contains("      - host_ip: \"::1\"\n        target: 1\n")
            || y.contains("      - target: 1\n        host_ip: \"::1\"\n"),
        "{y}"
    );
    assert!(y.contains("    tmpfs: []\n") && y.contains("    e: {}\n"), "{y}");
    assert!(y.contains("\"ON\": \"on\"") && y.contains("V: \"1.10\""), "{y}");
    assert!(y.contains("\"echo \\\"hi\\\" $$HOME\""), "{y}");
}

#[test]
fn parts_are_rechecked_and_literal_on_the_node() {
    let part = |compose: Value, env: &[(&str, &str)], dir: &str| StackPart {
        environment: "web".into(),
        stack: "shop".into(),
        dir: dir.into(),
        compose,
        env: env.iter().map(|(k, v)| ((*k).to_owned(), (*v).to_owned())).collect(),
        policy: Policy { stack_dir: dir.into(), ..policy() },
        internal_ip: "10.0.0.5".into(),
        mesh_network: None,
        dns: None,
    };
    let dir = "/opt/forge/stacks/web/shop";
    let ok =
        part(svc(json!({ "ports": ["${P}:80"], "command": "echo $$HOME ${A}" })), &[("P", "8080"), ("A", "$x")], dir);
    let (tree, ports) = ok.prepare().unwrap();
    assert_eq!(tree.pointer("/services/web/ports/0").unwrap(), "10.0.0.5:8080:80/tcp");
    assert_eq!(tree.pointer("/services/web/command").unwrap(), "echo $$HOME $$x");
    assert_eq!(ports[0].published, "8080");
    assert!(part(svc(json!({ "privileged": true })), &[], dir).prepare().is_err());
    let mut p = part(svc(json!({ "privileged": true })), &[], dir);
    p.policy.allow_privileged = true;
    assert!(p.prepare().is_ok());
    assert!(part(svc(json!({})), &[("COMPOSE_FILE", "x")], dir).prepare().is_err());

    // Folder: the default, or inside an approved root, and matching the policy.
    assert!(part(svc(json!({})), &[], "/srv/data/shop").prepare().is_ok());
    assert!(part(svc(json!({})), &[], "/etc/shop").prepare().is_err());
    let mut wrong = part(svc(json!({})), &[], dir);
    wrong.policy.stack_dir = "/srv/data/x".into();
    assert!(wrong.prepare().is_err());
    let mut ip = part(svc(json!({})), &[], dir);
    ip.internal_ip = "0.0.0.0; rm".into();
    assert!(ip.prepare().is_err());
}

#[test]
fn stack_services_get_the_log_url_unless_they_set_it() {
    let mut tree = json!({ "services": {
        "web": { "image": "nginx" },
        "api": { "image": "api", "environment": ["A=1", "FORGE_LOG_URL=http://seq:5341"], "extra_hosts": ["forge-node:10.0.0.9"] },
        "job": { "image": "job", "environment": { "A": "1" }, "extra_hosts": { "db": "10.0.0.2" } },
        "host": { "image": "h", "network_mode": "host" },
        "side": { "image": "s", "network_mode": "service:web" },
        "off": { "image": "o", "network_mode": "none" },
    }});
    add_log_url(&mut tree);
    let at = |p: &str| tree.pointer(p).cloned().unwrap_or(Value::Null);
    assert_eq!(at("/services/web/environment"), json!({ "FORGE_LOG_URL": "http://forge-node:5341" }));
    assert_eq!(at("/services/web/extra_hosts"), json!(["forge-node:host-gateway"]));
    assert_eq!(
        at("/services/api/environment"),
        json!(["A=1", "FORGE_LOG_URL=http://seq:5341"]),
        "the stack's value wins"
    );
    assert_eq!(at("/services/api/extra_hosts"), json!(["forge-node:10.0.0.9"]));
    assert_eq!(at("/services/job/environment/FORGE_LOG_URL"), json!("http://forge-node:5341"));
    assert_eq!(at("/services/job/extra_hosts"), json!({ "db": "10.0.0.2", "forge-node": "host-gateway" }));
    assert_eq!(at("/services/host/environment/FORGE_LOG_URL"), json!("http://127.0.0.1:5341"));
    assert_eq!(at("/services/host/extra_hosts"), Value::Null, "host networking needs no entry");
    assert_eq!(at("/services/side/extra_hosts"), Value::Null, "a shared network takes no host entries");
    assert_eq!(at("/services/off"), json!({ "image": "o", "network_mode": "none" }));
}

#[test]
fn helper_operations_validate() {
    let ok = |op: StackOp| op.validate().is_ok();
    let d = "/opt/forge/stacks/web/shop".to_owned();
    assert!(ok(StackOp::Ps { project: "forge-web-shop".into(), dir: d.clone() }));
    assert!(!ok(StackOp::Ps { project: "other".into(), dir: d.clone() }));
    assert!(!ok(StackOp::Ps { project: "forge-web-shop".into(), dir: "/etc".into() }));
    assert!(!ok(StackOp::Ps { project: "forge-web-shop".into(), dir: "/opt/../etc".into() }));
    assert!(ok(StackOp::Service {
        project: "forge-web-shop".into(),
        dir: d.clone(),
        action: "restart".into(),
        services: vec!["web".into()]
    }));
    assert!(!ok(StackOp::Service {
        project: "forge-web-shop".into(),
        dir: d.clone(),
        action: "exec".into(),
        services: vec![]
    }));
    assert!(!ok(StackOp::Service {
        project: "forge-web-shop".into(),
        dir: d.clone(),
        action: "stop".into(),
        services: vec!["--rm".into()]
    }));
    assert!(!ok(StackOp::Logs { project: "forge-web-shop".into(), dir: d, service: "-f".into(), tail: 10 }));
    assert!(
        valid_stack_name("shop-1") && !valid_stack_name("Shop") && !valid_stack_name("-x") && !valid_stack_name("")
    );
    assert_eq!(project_name("web", "shop"), "forge-web-shop");
}

// ---- security regressions ------------------------------------------------------------

#[test]
fn binds_through_a_writable_bind_are_refused() {
    // a writes ./data (and could plant data/x -> /); b mounts ./data/x.
    let t = json!({ "services": {
        "a": { "image": "x", "volumes": ["./data:/data"] },
        "b": { "image": "x", "volumes": ["./data/x:/host:ro"] }
    }});
    let r = check(&t, &policy());
    assert!(!r.blocking(true).is_empty(), "{:?}", r.findings);

    // Long syntax and approved roots alike.
    let t = json!({ "services": {
        "a": { "image": "x", "volumes": [{ "type": "bind", "source": "/srv/data", "target": "/d" }] },
        "b": { "image": "x", "volumes": ["/srv/data/site:/s"] }
    }});
    assert!(!check(&t, &policy()).blocking(true).is_empty());

    // Files Compose reads may not lie in a writable bind either.
    for extra in [
        json!({ "volumes": ["./data:/data"], "env_file": ["./data/app.env"] }),
        json!({ "volumes": ["./data:/data"], "label_file": "./data/l" }),
    ] {
        assert!(!check(&svc(extra.clone()), &policy()).blocking(true).is_empty(), "{extra}");
    }

    let t = json!({ "services": { "a": { "image": "x", "volumes": ["./data:/data"] } }, "configs": { "c": { "file": "./data/c.conf" } } });
    assert!(!check(&t, &policy()).blocking(true).is_empty());

    // A writable bind of the stack folder exposes compose.yaml and .env.
    assert!(!check(&svc(json!({ "volumes": [".:/app"] })), &policy()).blocking(true).is_empty());

    // Read-only parents, shared folders and siblings are fine.
    let ok = json!({ "services": {
        "a": { "image": "x", "volumes": [".:/app:ro", "./data:/data", "./html:/html"] },
        "b": { "image": "x", "volumes": ["./data:/data", "./html:/html:ro", { "type": "bind", "source": "./conf", "target": "/c", "read_only": true }, "./conf/site:/s"] }
    }});
    let r = check(&ok, &policy());
    assert!(r.blocking(false).is_empty(), "{:?}", r.findings);

    // env_file is listed for the node's link check.
    let t = svc(json!({ "env_file": [".env", { "path": "./x.env" }] }));
    let dir = "/opt/forge/stacks/web/shop";
    assert_eq!(host_paths(&t, dir), vec![format!("{dir}/.env"), format!("{dir}/x.env")]);
}

#[test]
fn host_network_by_name_needs_privileged() {
    for nets in [
        json!({ "h": { "external": true, "name": "host" } }),
        json!({ "host": { "external": true } }),
        json!({ "h": { "external": { "name": "host" } } }),
        json!({ "h": { "name": "host" } }),
    ] {
        let t = json!({ "services": { "a": { "image": "x", "networks": ["h", "host"] } }, "networks": nets.clone() });
        let r = check(&t, &policy());
        assert!(r.privileged, "{nets}");
        assert!(r.blocking(true).is_empty(), "{nets}");
    }

    // A project network keyed `host` is an ordinary network.
    let t = json!({ "services": { "a": { "image": "x", "networks": ["host"] } }, "networks": { "host": {} } });
    assert!(!check(&t, &policy()).privileged);

    // `service:` must name a service of the stack.
    let t = json!({ "services": { "a": { "image": "x", "network_mode": "service:b" }, "b": { "image": "x" } } });
    assert!(!check(&t, &policy()).privileged);
    let t = json!({ "services": { "a": { "image": "x", "network_mode": "service:other" } } });
    assert!(check(&t, &policy()).privileged);
}

#[test]
fn hooks_environments_and_unknown_keys_are_caught() {
    for extra in [
        json!({ "post_start": [{ "command": "id", "privileged": true }] }),
        json!({ "pre_stop": [{ "command": "id", "privileged": "true" }] }),
        json!({ "pid": "container:abc" }),
        json!({ "ipc": "container:abc" }),
        json!({ "ipc": "service:elsewhere" }),
        json!({ "use_api_socket": true }),
    ] {
        let r = check(&svc(extra.clone()), &policy());
        assert!(r.privileged, "{extra}");
        assert!(r.blocking(true).is_empty(), "{extra}: {:?}", r.findings);
    }

    for extra in [
        json!({ "provider": { "type": "model", "options": {} } }),
        json!({ "some_future_key": true }),
        json!({ "label_file": "/etc/shadow" }),
    ] {
        assert!(!check(&svc(extra.clone()), &policy()).blocking(true).is_empty(), "{extra}");
    }

    let ok = svc(json!({ "post_start": [{ "command": "id", "user": "app" }], "x-note": 1, "init": true }));
    assert!(check(&ok, &policy()).findings.is_empty());
}

#[test]
fn system_paths_win_over_the_stack_folder() {
    let p = Policy { allow_privileged: false, bind_roots: vec!["/var".into()], stack_dir: "/var/lib/docker".into() };
    let r = check(&svc(json!({ "volumes": ["./overlay2:/x"] })), &p);
    assert!(r.privileged, "{:?}", r.findings);
    assert!(!r.blocking(true).is_empty(), "the folder itself is refused");
    let part = StackPart {
        environment: "web".into(),
        stack: "shop".into(),
        dir: "/var/lib/docker/overlay2".into(),
        compose: svc(json!({})),
        env: BTreeMap::new(),
        policy: Policy { stack_dir: "/var/lib/docker/overlay2".into(), ..p },
        internal_ip: "10.0.0.5".into(),
        mesh_network: None,
        dns: None,
    };
    assert!(part.prepare().is_err());
    assert!(!StackOp::Ps { project: "forge-web-shop".into(), dir: "/etc/forge/x".into() }.validate().is_ok());
    assert!(is_system_path("/var/lib/docker/overlay2") && is_system_path("/") && !is_system_path("/srv"));
}

#[test]
fn env_values_are_redacted_from_messages() {
    let env = BTreeMap::from([
        ("DB_PASSWORD".to_owned(), "hunter2".to_owned()),
        ("SRC".to_owned(), "/srv/very-secret-dir".to_owned()),
        ("PIN_KEY".to_owned(), "7".to_owned()),
        ("N".to_owned(), "1".to_owned()),
    ]);
    assert_eq!(redact("ping -phunter2 /srv/very-secret-dir 127", &env), "ping -p${DB_PASSWORD} ${SRC} 12${PIN_KEY}");
    let v = redact_value(&json!({ "m": ["hunter2"] }), &env);
    assert_eq!(v, json!({ "m": ["${DB_PASSWORD}"] }));

    // The node's errors do not carry .env values either.
    let part = StackPart {
        environment: "web".into(),
        stack: "shop".into(),
        dir: "/opt/forge/stacks/web/shop".into(),
        compose: svc(json!({ "volumes": ["${SRC}:/x"] })),
        env,
        policy: policy(),
        internal_ip: "10.0.0.5".into(),
        mesh_network: None,
        dns: None,
    };
    let e = part.prepare().unwrap_err();
    assert!(!e.contains("very-secret") && e.contains("${SRC}"), "{e}");
}

const MESH: &str = "forge-forgea1b2c3";

fn meshed(tree: Value) -> Value {
    let mut t = tree;
    add_mesh_network(&mut t, MESH);

    t
}

#[test]
fn mesh_network_joins_list_map_and_absent_networks() {
    let t = meshed(json!({ "services": {
        "a": { "image": "x" },
        "b": { "image": "x", "networks": ["back"] },
        "c": { "image": "x", "networks": { "back": { "aliases": ["c1"] } } },
        "d": { "image": "x", "networks": ["back", "forge-mesh"] },
    }}));

    assert_eq!(t.pointer("/services/a/networks").unwrap(), &json!(["default", "forge-mesh"]));
    assert_eq!(t.pointer("/services/b/networks").unwrap(), &json!(["back", "forge-mesh"]));
    assert_eq!(t.pointer("/services/c/networks/forge-mesh").unwrap(), &json!({}));
    assert_eq!(t.pointer("/services/c/networks/back/aliases/0").unwrap(), "c1");
    assert_eq!(t.pointer("/services/d/networks").unwrap(), &json!(["back", "forge-mesh"]));
    assert_eq!(t.pointer("/networks/forge-mesh").unwrap(), &json!({ "external": true, "name": MESH }));
}

#[test]
fn mesh_network_respects_network_mode_and_opt_outs() {
    let t = meshed(json!({ "services": {
        "host": { "image": "x", "network_mode": "host" },
        "side": { "image": "x", "network_mode": "service:host" },
        "off": { "image": "x", "x-forge": { "mesh": false } },
        "on": { "image": "x", "x-forge": { "mesh": true } },
    }}));

    for name in ["host", "side", "off"] {
        assert!(t.pointer(&format!("/services/{name}/networks")).is_none(), "{name}");
    }

    assert_eq!(t.pointer("/services/on/networks").unwrap(), &json!(["default", "forge-mesh"]));

    let nothing = meshed(json!({ "services": { "off": { "image": "x", "x-forge": { "mesh": false } } } }));
    assert!(nothing.get("networks").is_none());

    let stack_off = meshed(json!({ "x-forge": { "mesh": false }, "services": { "a": { "image": "x" } } }));
    assert!(stack_off.pointer("/services/a/networks").is_none());
    assert!(stack_off.get("networks").is_none());
}

#[test]
fn mesh_network_key_is_reserved() {
    let tree = json!({
        "services": { "a": { "image": "x", "networks": ["forge-mesh"] } },
        "networks": { "forge-mesh": {} },
    });

    assert!(blocked(&tree).iter().any(|m| m.contains("`forge-mesh` is reserved")), "{:?}", blocked(&tree));
}

#[test]
fn only_the_parts_own_cluster_network_may_be_external() {
    let tree = |name: &str| {
        json!({
            "services": { "a": { "image": "x", "networks": ["n"] } },
            "networks": { "n": { "external": true, "name": name } },
        })
    };

    assert!(check_mesh_refs(&tree(MESH), Some(MESH)).is_ok());
    assert!(check_mesh_refs(&tree(MESH), None).is_err());
    assert!(check_mesh_refs(&tree("forge-forgeffffff"), Some(MESH)).is_err());
    assert!(check_mesh_refs(&tree("shared-proxy"), None).is_ok());
}

#[test]
fn prepared_part_joins_the_mesh_and_keeps_service_opt_out() {
    let dir = "/opt/forge/stacks/web/shop";
    let compose = json!({ "services": {
        "web": { "image": "x", "x-forge": { "mesh": false, "node": "lab-b" } },
        "api": { "image": "x" },
    }});
    let part_tree = part_for(&compose, &["web".to_owned(), "api".to_owned()].into(), "s1").unwrap();

    assert_eq!(part_tree.pointer("/services/web/x-forge/mesh").unwrap(), false);
    assert!(part_tree.pointer("/services/web/x-forge/node").is_none());

    let part = StackPart {
        environment: "web".into(),
        stack: "shop".into(),
        dir: dir.into(),
        compose: part_tree,
        env: BTreeMap::new(),
        policy: policy(),
        internal_ip: "10.0.0.5".into(),
        mesh_network: Some(MESH.into()),
        dns: None,
    };
    let (tree, _) = part.prepare().unwrap();

    assert_eq!(tree.pointer("/services/api/networks").unwrap(), &json!(["default", "forge-mesh"]));
    assert!(tree.pointer("/services/web/networks").is_none());
    assert_eq!(tree.pointer("/networks/forge-mesh/name").unwrap(), MESH);
}

#[test]
fn dns_goes_only_on_meshed_services_without_their_own() {
    let mut t = meshed(json!({ "services": {
        "api": { "image": "x" },
        "own": { "image": "x", "dns": ["192.0.2.53"] },
        "off": { "image": "x", "x-forge": { "mesh": false } },
    }}));
    let dns = StackDns { servers: vec!["10.64.12.1".parse().unwrap()], search: vec!["prod.c1.local".into()] };
    add_dns(&mut t, &dns);

    assert_eq!(t.pointer("/services/api/dns").unwrap(), &json!(["10.64.12.1"]));
    assert_eq!(t.pointer("/services/api/dns_search").unwrap(), &json!(["prod.c1.local"]));
    assert_eq!(t.pointer("/services/own/dns").unwrap(), &json!(["192.0.2.53"]));
    assert!(t.pointer("/services/off/dns").is_none());
}
