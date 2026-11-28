#![allow(clippy::unwrap_used)]

use super::*;

fn one(svc: Value) -> FileService {
    let file = parse(&json!({ "services": { "web": svc } })).unwrap();

    file.services.into_iter().next().unwrap()
}

fn refused(tree: &Value) -> String {
    parse(tree).unwrap_err().to_string()
}

fn web(extra: Value) -> Value {
    let mut svc = json!({ "image": "nginx:1.27" });

    for (k, v) in extra.as_object().unwrap() {
        svc[k] = v.clone();
    }

    json!({ "services": { "web": svc } })
}

#[test]
fn every_container_key_maps_onto_the_spec() {
    let s = one(json!({
        "image": "nginx:1.27",
        "command": ["nginx", "-g", "daemon off;"],
        "entrypoint": ["/docker-entrypoint.sh"],
        "working_dir": "/srv",
        "environment": { "MODE": "prod", "PORT": 8080 },
        "ports": ["8080:80", { "target": 443, "published": 8443, "host_ip": "192.0.2.10" }, "53:53/udp"],
        "volumes": ["data:/var/lib/data", "/srv/conf:/etc/conf:ro", { "type": "tmpfs", "target": "/cache", "tmpfs": { "size": "64M" } }],
        "healthcheck": { "test": ["CMD-SHELL", "curl -f localhost"], "interval": "10s", "timeout": "2s", "retries": 5, "start_period": "1m" },
        "user": "1000:1000",
        "hostname": "web",
        "labels": ["team=core"],
        "extra_hosts": ["db:192.0.2.5", "cache=192.0.2.6"],
        "cap_add": ["NET_ADMIN"],
        "cap_drop": ["ALL"],
        "ulimits": { "nofile": { "soft": 1024, "hard": 4096 }, "nproc": 512 },
        "tmpfs": ["/run", "/tmp:size=32m"],
        "dns": "192.0.2.53",
        "sysctls": { "net.core.somaxconn": 1024 },
        "devices": ["/dev/fuse:/dev/fuse:rwm"],
        "init": true,
        "read_only": true,
        "stop_grace_period": "1m30s",
        "restart": "on-failure:3",
        "x-forge": { "on_node_failure": "alert", "mesh": false },
        "x-notes": "ignored extension field",
    }));

    assert_eq!(s.runtime, Runtime::Container);
    assert_eq!(s.spec.command, "nginx -g 'daemon off;'");
    assert_eq!(s.spec.entrypoint, "/docker-entrypoint.sh");
    assert_eq!(s.spec.working_dir, "/srv");
    assert!(s.env.contains(&("PORT".into(), "8080".into())));
    assert_eq!(s.spec.ports, ["8080:80", "192.0.2.10:8443:443", "53:53/udp"]);
    assert_eq!(s.spec.volumes, ["data:/var/lib/data", "/srv/conf:/etc/conf:ro"]);
    let hc = s.spec.healthcheck.clone().unwrap();
    assert_eq!(
        (hc.command.as_str(), hc.interval_secs, hc.timeout_secs, hc.retries, hc.start_period_secs),
        ("curl -f localhost", 10, 2, 5, 60)
    );
    assert_eq!(s.spec.user, "1000:1000");
    assert_eq!(s.spec.hostname, "web");
    assert_eq!(s.spec.labels, [("team".to_owned(), "core".to_owned())]);
    assert_eq!(s.spec.extra_hosts, ["db:192.0.2.5", "cache:192.0.2.6"]);
    let o = &s.spec.options;
    assert_eq!(o.cap_add, ["NET_ADMIN"]);
    assert_eq!(o.cap_drop, ["ALL"]);
    assert_eq!(o.ulimits.len(), 2);
    assert!(o.ulimits.iter().any(|u| u.name == "nproc" && u.soft == 512 && u.hard == 512));
    assert!(o.tmpfs.iter().any(|t| t.target == "/cache" && t.size_mb == 64));
    assert!(o.tmpfs.iter().any(|t| t.target == "/tmp" && t.size_mb == 32));
    assert!(o.tmpfs.iter().any(|t| t.target == "/run" && t.size_mb == 0));
    assert_eq!(o.dns, ["192.0.2.53".parse::<IpAddr>().unwrap()]);
    assert_eq!(o.sysctls, [("net.core.somaxconn".to_owned(), "1024".to_owned())]);
    assert_eq!(o.devices[0].host, "/dev/fuse");
    assert_eq!(o.devices[0].permissions, "rwm");
    assert!(o.init && s.spec.read_only);
    assert_eq!(s.spec.stop_timeout_secs, 90);
    assert_eq!((o.restart_policy, o.restart_max_attempts), (Some(RestartPolicy::OnFailure), 3));
    assert_eq!(s.on_node_failure, Some(NodeFailurePolicy::Alert));
    assert_eq!(s.mesh, Some(false));
}

#[test]
fn deploy_keys_map_onto_placement_limits_and_rollout() {
    let s = one(json!({
        "image": "api:2",
        "deploy": {
            "replicas": 3,
            "placement": {
                "constraints": ["node.hostname == lab-a", "node.labels.zone==eu"],
                "preferences": [{ "spread": "node.labels.rack" }],
                "max_replicas_per_node": 1,
            },
            "resources": { "limits": { "cpus": "0.5", "memory": "512M" }, "reservations": { "cpus": 0.25, "memory": "1G" } },
            "restart_policy": { "condition": "on-failure", "max_attempts": 4 },
            "update_config": { "parallelism": 2, "order": "start-first", "failure_action": "rollback", "delay": "5s", "monitor": "30s", "max_failure_ratio": 0.5 },
            "rollback_config": { "parallelism": 1, "order": "stop-first" },
        },
    }));
    let o = &s.orchestration;

    assert_eq!((o.mode, o.replicas), (ServiceMode::Replicated, 3));
    assert_eq!(o.nodes, ["lab-a"]);
    assert_eq!(o.node_tags.get("zone").map(String::as_str), Some("eu"));
    assert_eq!(o.spread.as_deref(), Some("rack"));
    assert_eq!((s.spec.cpu_millis, s.spec.memory_mb), (500, 512));
    assert_eq!((o.reserve_cpu_millis, o.reserve_memory_mb), (250, 1024));
    assert_eq!(
        (s.spec.options.restart_policy, s.spec.options.restart_max_attempts),
        (Some(RestartPolicy::OnFailure), 4)
    );
    let u = o.update.clone().unwrap();
    assert_eq!(
        (u.parallelism, u.order, u.failure_action, u.delay_secs, u.monitor_secs),
        (2, UpdateOrder::StartFirst, FailureAction::Rollback, 5, 30)
    );
    assert_eq!(o.rollback.clone().unwrap().order, UpdateOrder::StopFirst);
    assert_eq!(o.rules().count, Count::Replicas(3));

    let g = one(json!({ "image": "agent:1", "deploy": { "mode": "global" } }));
    assert_eq!(g.orchestration.rules().count, Count::Global);
}

#[test]
fn native_services_depend_on_each_other_in_order() {
    let file = parse(&json!({
        "name": "shop",
        "services": {
            "api": { "image": "api:1", "depends_on": { "db": { "condition": "service_healthy" } } },
            "db": { "image": "postgres:17" },
            "worker": { "command": "/usr/bin/worker", "depends_on": ["api"], "x-forge": { "runtime": "systemd", "listen_port": 9000 } },
        },
    }))
    .unwrap();
    let order: Vec<&str> = file.services.iter().map(|s| s.name.as_str()).collect();

    assert_eq!(file.name.as_deref(), Some("shop"));
    assert_eq!(order, ["db", "api", "worker"]);
    assert_eq!(file.services[2].runtime, Runtime::Systemd);
    assert_eq!(file.services[2].spec.listen_port, Some(9000));

    let cycle = json!({ "services": { "a": { "image": "x", "depends_on": ["b"] }, "b": { "image": "x", "depends_on": ["a"] } } });
    assert!(refused(&cycle).contains("cycle"));
    let unknown = json!({ "services": { "a": { "image": "x", "depends_on": ["ghost"] } } });
    assert!(refused(&unknown).contains("services.a.depends_on: no service `ghost`"));
}

#[test]
fn refusals_name_the_key() {
    for (extra, path, why) in [
        (json!({ "build": "." }), "services.web.build", "image"),
        (json!({ "privileged": true }), "services.web.privileged", "cap_add"),
        (json!({ "network_mode": "host" }), "services.web.network_mode", "host networking"),
        (json!({ "secrets": ["db"] }), "services.web.secrets[0]", "not declared"),
        (json!({ "configs": [{ "target": "/x" }] }), "services.web.configs[0].source", "required"),
        (json!({ "env_file": ".env" }), "services.web.env_file", "inlines"),
        (json!({ "colour": "blue" }), "services.web.colour", "unknown key"),
        (json!({ "deploy": { "endpoint_mode": "vip" } }), "services.web.deploy.endpoint_mode", "cluster DNS"),
        (
            json!({ "deploy": { "placement": { "max_replicas_per_node": 2 } } }),
            "services.web.deploy.placement.max_replicas_per_node",
            "one copy",
        ),
        (
            json!({ "deploy": { "placement": { "constraints": ["node.role==manager"] } } }),
            "services.web.deploy.placement.constraints[0]",
            "node.labels",
        ),
        (
            json!({ "deploy": { "placement": { "constraints": ["node.labels.zone!=eu"] } } }),
            "services.web.deploy.placement.constraints[0]",
            "==",
        ),
        (json!({ "deploy": { "mode": "global", "replicas": 2 } }), "services.web.deploy.replicas", "global"),
        (
            json!({ "deploy": { "resources": { "limits": { "pids": 5 } } } }),
            "services.web.deploy.resources.limits.pids",
            "unknown key",
        ),
        (
            json!({ "deploy": { "restart_policy": { "delay": "5s" } } }),
            "services.web.deploy.restart_policy.delay",
            "delay",
        ),
        (
            json!({ "restart": "always", "deploy": { "restart_policy": { "condition": "any" } } }),
            "services.web.deploy.restart_policy",
            "not both",
        ),
        (json!({ "ports": ["80"] }), "services.web.ports[0]", "host:container"),
        (json!({ "ports": ["8000-8010:8000-8010"] }), "services.web.ports[0]", "ranges"),
        (json!({ "volumes": ["/data"] }), "services.web.volumes[0]", "anonymous"),
        (json!({ "volumes": ["./data:/data"] }), "services.web.volumes[0]", "relative"),
        (json!({ "environment": ["TOKEN"] }), "services.web.environment[0]", "shell environment"),
        (json!({ "networks": ["backend"] }), "services.web.networks", "not declared"),
        (json!({ "x-forge": { "strategy": "canary" } }), "services.web.x-forge.strategy", "forge.deploy.yaml"),
        (json!({ "x-forge": { "console": { "port": 0 } } }), "services.web.x-forge.console", "1-65535"),
        (
            json!({ "x-forge": { "console": { "port": 80, "host": "x" } } }),
            "services.web.x-forge.console.host",
            "unknown key",
        ),
        (
            json!({ "x-forge": { "console": { "port": 80, "scheme": "ftp" } } }),
            "services.web.x-forge.console",
            "unknown variant",
        ),
        (json!({ "stop_grace_period": "soon" }), "services.web.stop_grace_period", "duration"),
    ] {
        let msg = refused(&web(extra.clone()));
        assert!(msg.starts_with(path), "{extra}: {msg}");
        assert!(msg.contains(why), "{extra}: {msg}");
    }

    assert!(
        refused(&json!({ "services": { "web": { "image": "x" } }, "include": [] })).starts_with("include: unknown key")
    );
    assert!(refused(&json!({ "services": { "web": {} } })).starts_with("services.web.image: required"));
    assert!(refused(&json!({ "services": { "Web": { "image": "x" } } })).starts_with("services.Web"));
    assert!(
        refused(&json!({ "services": { "web": { "image": "x" } }, "networks": { "back": {} } })).contains("external")
    );
}

#[test]
fn external_networks_and_named_volumes_are_accepted() {
    let file = parse(&json!({
        "services": { "web": { "image": "x", "networks": ["default", "back"], "volumes": ["data:/d"] } },
        "networks": { "back": { "external": true, "name": "corp-backend" } },
        "volumes": { "data": null },
    }))
    .unwrap();

    assert_eq!(file.services[0].spec.options.networks, ["corp-backend"]);
}

#[test]
fn sizes_durations_and_cpus_read_like_docker() {
    let p = "x";
    assert_eq!(duration_secs(&json!("1h2m3s"), p).unwrap(), 3723);
    assert_eq!(duration_secs(&json!("500ms"), p).unwrap(), 1);
    assert_eq!(duration_secs(&json!(15), p).unwrap(), 15);
    assert!(duration_secs(&json!("10"), p).is_err());
    assert_eq!(memory_mb(&json!("1g"), p).unwrap(), 1024);
    assert_eq!(memory_mb(&json!("256MiB"), p).unwrap(), 256);
    assert_eq!(memory_mb(&json!("1536k"), p).unwrap(), 2);
    assert!(memory_mb(&json!("lots"), p).is_err());
    assert_eq!(cpus(&json!("1.5"), p).unwrap(), 1500);
    assert_eq!(millis_cpus(1500), "1.5");
    assert_eq!(millis_cpus(2000), "2");
    assert_eq!(millis_cpus(250), "0.25");
}

#[test]
fn a_stored_spec_reads_back_with_its_orchestration() {
    let spec = json!({
        "environment": "prod", "image": "redis:7", "ports": ["6379:6379"], "restart": true,
        "cap_drop": ["ALL"], "on_node_failure": "move", "mesh": false, "console": { "port": 9001 },
        "orchestration": { "mode": "global", "node_tags": { "zone": "eu" } },
        "swarm": null,
    });
    let s = from_stored("cache", "container", &spec, vec![("A".into(), "1".into())]).unwrap();

    assert_eq!(s.spec.image, "redis:7");
    assert_eq!(s.spec.options.cap_drop, ["ALL"]);
    assert_eq!(s.orchestration.mode, ServiceMode::Global);
    assert_eq!(s.on_node_failure, Some(NodeFailurePolicy::Move));
    assert_eq!(s.mesh, Some(false));
    assert_eq!(s.console.as_ref().map(|c| (c.port, c.path.as_str())), Some((9001, "/")));
    assert!(from_stored("x", "compose-service", &spec, vec![]).is_err());
}

#[cfg(feature = "yaml")]
#[test]
fn export_parses_back_to_the_same_services() {
    let text = r#"
name: shop
services:
  api:
    image: api:2
    command: ["serve", "--port", "8080"]
    environment:
      MODE: prod
    ports: ["8080:8080"]
    volumes: ["/srv/api:/data:ro"]
    healthcheck:
      test: curl -f localhost:8080
      interval: 15s
    labels:
      team: core
    extra_hosts: ["db:192.0.2.5"]
    cap_drop: [ALL]
    ulimits:
      nofile: 2048
    tmpfs: [/run]
    dns: [192.0.2.53]
    sysctls:
      net.core.somaxconn: "1024"
    init: true
    read_only: true
    stop_grace_period: 20s
    networks: [back]
    restart: on-failure:2
    depends_on: [db]
    deploy:
      replicas: 2
      placement:
        constraints: ["node.labels.zone == eu"]
        preferences: [{ spread: node.labels.rack }]
      resources:
        limits: { cpus: "1.5", memory: 256M }
        reservations: { memory: 128M }
      update_config: { parallelism: 1, order: stop-first }
    x-forge:
      on_node_failure: alert
      console: { port: 8443, path: /admin, scheme: https }
  db:
    image: postgres:17
    volumes: ["pg:/var/lib/postgresql/data"]
    deploy:
      placement:
        constraints: ["node.hostname == lab-b"]
  worker:
    command: /usr/bin/worker --queue jobs
    x-forge: { runtime: systemd, listen_port: 9000 }
    deploy: { mode: global }
networks:
  back: { external: true }
"#;
    let first = parse(&crate::yaml::parse_document(text).unwrap()).unwrap();
    let written = export(first.name.as_deref(), &first.services);
    let again = parse(&crate::yaml::parse_document(&written).unwrap()).unwrap();

    assert_eq!(first, again, "{written}");
}

#[cfg(feature = "yaml")]
#[test]
fn secrets_and_configs_name_vault_items_and_export_back() {
    let tree = json!({
        "services": { "api": {
            "image": "x",
            "secrets": ["db_password", { "source": "tls", "target": "key.pem", "mode": "0440", "uid": "1000" }],
            "configs": [{ "source": "app_conf", "target": "/etc/app/app.conf" }],
        } },
        "secrets": {
            "db_password": { "external": true },
            "tls": { "external": true, "name": "API TLS key" },
        },
        "configs": { "app_conf": { "external": true, "name": "api-config" } },
    });
    let svc = parse(&tree).unwrap().services.remove(0);
    let files = &svc.spec.secret_files;
    let by = |item: &str| files.iter().find(|f| f.item == item).unwrap();

    assert_eq!(files.len(), 3);
    assert_eq!(by("db_password").target, "/run/secrets/db_password");
    assert_eq!(by("db_password").effective_mode(), 0o400);
    assert_eq!(by("API TLS key").target, "/run/secrets/key.pem");
    assert_eq!((by("API TLS key").mode, by("API TLS key").uid), (Some(0o440), Some(1000)));
    assert_eq!(by("api-config").kind, crate::secret_files::FileKind::Config);
    assert_eq!(by("api-config").target, "/etc/app/app.conf");
    assert_eq!(by("api-config").effective_mode(), 0o444);

    // Export writes references only and reads back to the same files.
    let written = export(None, std::slice::from_ref(&svc));
    let again = parse(&crate::yaml::parse_document(&written).unwrap()).unwrap().services.remove(0);
    assert_eq!(again.spec.secret_files, svc.spec.secret_files, "{written}");
    assert!(written.contains("API TLS key"));
}

#[test]
fn file_secrets_and_undeclared_sources_are_refused() {
    let with = |top: Value| {
        let mut t = json!({ "services": { "api": { "image": "x", "secrets": ["db"] } } });
        t["secrets"] = top;
        t
    };

    assert!(
        refused(&with(json!({ "db": { "file": "./db.txt" } })))
            .starts_with("secrets.db.file: the server never reads files")
    );
    assert!(refused(&with(json!({ "db": { "environment": "DB" } }))).contains("vault"));
    assert!(refused(&with(json!({ "db": {} }))).starts_with("secrets.db.external: must be true"));
    assert!(refused(&with(json!({ "db": { "external": true, "driver": "x" } }))).starts_with("secrets.db.driver"));
    assert!(refused(&with(json!({ "other": { "external": true } }))).contains("`db` is not declared"));

    let clash = json!({
        "services": { "api": { "image": "x", "secrets": ["a", { "source": "b", "target": "a" }] } },
        "secrets": { "a": { "external": true }, "b": { "external": true } },
    });
    assert!(refused(&clash).contains("named `a`"));
}

#[test]
fn the_console_link_is_read_with_its_defaults() {
    let s = one(json!({ "image": "minio:1", "x-forge": { "console": { "port": 9001 } } }));
    let c = s.console.unwrap();

    assert_eq!((c.port, c.path.as_str(), c.scheme), (9001, "/", crate::console_link::ConsoleScheme::Http));
}
