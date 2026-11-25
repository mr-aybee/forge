use serde_json::json;

use super::*;

fn http(host: &str, allow: bool) -> Check {
    serde_json::from_value(json!({
        "name": "ready",
        "probe": { "type": "http", "host": host, "allow_remote_host": allow, "port": 8080, "path": "/health/ready" }
    }))
    .unwrap()
}

fn policy(readiness: Vec<Check>) -> HealthPolicy {
    HealthPolicy { readiness, ..HealthPolicy::default() }
}

fn fields(p: &HealthPolicy) -> Vec<String> {
    p.validate().into_iter().map(|e| e.field).collect()
}

#[test]
fn defaults_fill_in_and_a_minimal_policy_is_valid() {
    let p: HealthPolicy = serde_json::from_value(json!({
        "readiness": [{ "name": "web", "probe": { "type": "http", "port": 8080 } }],
        "liveness": [{ "name": "tcp", "probe": { "type": "tcp", "port": 8080 } }]
    }))
    .unwrap();
    let c = &p.readiness[0];
    assert_eq!((c.period_secs, c.timeout_secs, c.failure_threshold, c.success_threshold), (10, 2, 3, 1));
    assert!(c.enabled && c.required);
    let Probe::Http(h) = &c.probe else { panic!() };
    assert_eq!((h.path.as_str(), h.method, h.scheme, h.tls_verify), ("/", Method::Get, Scheme::Http, true));
    assert_eq!(h.expected_statuses, vec![StatusRange { from: 200, to: 299 }]);
    assert!(p.validate().is_empty(), "{:?}", p.validate());
}

#[test]
fn unknown_fields_are_refused() {
    let r: Result<HealthPolicy, _> = serde_json::from_value(json!({
        "readiness": [{ "name": "a", "probe": { "type": "tcp", "port": 1, "bogus": true } }]
    }));
    assert!(r.is_err());
    let r: Result<HealthPolicy, _> = serde_json::from_value(json!({ "readyness": [] }));
    assert!(r.is_err());
}

#[test]
fn the_json_round_trip_keeps_arrays_scripts_and_secret_refs() {
    let p: HealthPolicy = serde_json::from_value(json!({
        "startup": [{ "name": "init", "probe": { "type": "exec", "argv": ["/opt/app/check", "--quick", "a b"] },
                       "failure_threshold": 60 }],
        "readiness": [{ "name": "api", "required": false, "success_threshold": 2,
            "probe": { "type": "http", "scheme": "https", "port": 443, "path": "/ready", "server_name": "api.local",
                       "headers": [{ "name": "Accept", "value": "application/json" },
                                   { "name": "Authorization", "secret": "vault:0192abcd" }],
                       "expected_statuses": [{ "from": 200, "to": 204 }, { "from": 301, "to": 301 }] } }],
        "liveness": [{ "name": "beat", "probe": { "type": "shell", "interpreter": "bash",
                       "script": "test -r /run/app/beat || exit 1\n/opt/app/bin/check --max-age 60\n" } }]
    }))
    .unwrap();
    assert!(p.validate().is_empty(), "{:?}", p.validate());
    let back: HealthPolicy = serde_json::from_value(serde_json::to_value(&p).unwrap()).unwrap();
    assert_eq!(back, p);
    let Probe::Exec(e) = &back.startup[0].probe else { panic!() };
    assert_eq!(e.argv, ["/opt/app/check", "--quick", "a b"], "argv is not flattened into a shell string");
    assert!(p.runs_code());
}

#[test]
fn timing_and_threshold_bounds() {
    let mut c = http("", false);
    c.period_secs = 5;
    c.timeout_secs = 5;
    assert_eq!(fields(&policy(vec![c.clone()])), ["readiness[0].timeout_secs"]);
    c.timeout_secs = 2;
    c.period_secs = 0;
    assert!(fields(&policy(vec![c.clone()])).contains(&"readiness[0].period_secs".to_owned()));
    c.period_secs = 10;
    c.failure_threshold = 0;
    assert_eq!(fields(&policy(vec![c.clone()])), ["readiness[0].failure_threshold"]);
    c.failure_threshold = 3;

    // Startup and liveness: success threshold is 1; only readiness may be optional.
    let mut l = c.clone();
    l.success_threshold = 2;
    l.required = false;
    let p = HealthPolicy { liveness: vec![l], ..HealthPolicy::default() };
    assert_eq!(fields(&p), ["liveness[0].success_threshold", "liveness[0].required"]);
}

#[test]
fn names_are_validated_and_unique_per_purpose() {
    let a = http("", false);
    let mut bad = http("", false);
    bad.name = "Bad Name".into();
    assert_eq!(fields(&policy(vec![a.clone(), a.clone(), bad])), ["readiness[1].name", "readiness[2].name"]);

    // The same name in another purpose is fine.
    let p = HealthPolicy { readiness: vec![a.clone()], liveness: vec![a], ..HealthPolicy::default() };
    assert!(p.validate().is_empty());
}

#[test]
fn probes_target_the_workload_unless_a_remote_host_is_allowed() {
    for ok in [http("", false), http("127.0.0.1", false), http("::1", false), http("localhost", false)] {
        assert!(policy(vec![ok.clone()]).validate().is_empty(), "{ok:?}");
    }

    assert_eq!(fields(&policy(vec![http("10.0.0.5", false)])), ["readiness[0].probe.allow_remote_host"]);
    assert!(policy(vec![http("10.0.0.5", true)]).validate().is_empty());
    assert!(policy(vec![http("api.internal", true)]).validate().is_empty());

    // Allowed but no host.
    assert_eq!(fields(&policy(vec![http("", true)])), ["readiness[0].probe.allow_remote_host"]);
}

#[test]
fn metadata_and_link_local_targets_are_always_refused() {
    for host in [
        "169.254.169.254",
        "169.254.1.1",
        "fe80::1",
        "fd00:ec2::254",
        "::ffff:169.254.169.254",
        "0.0.0.0",
        "metadata.google.internal",
        "METADATA.google.internal.",
        "instance-data",
    ] {
        assert_eq!(fields(&policy(vec![http(host, true)])), ["readiness[0].probe.host"], "{host}");
    }

    assert_eq!(fields(&policy(vec![http("bad host!", true)])), ["readiness[0].probe.host"]);
}

#[test]
fn credentials_in_headers_must_be_vault_references() {
    let with = |headers: serde_json::Value| -> Vec<String> {
        let c: Check = serde_json::from_value(json!({
            "name": "a", "probe": { "type": "http", "port": 80, "headers": headers }
        }))
        .unwrap();
        fields(&policy(vec![c]))
    };
    assert!(with(json!([{ "name": "Accept", "value": "text/plain" }])).is_empty());
    assert!(with(json!([{ "name": "Authorization", "secret": "vault:abc-123" }])).is_empty());

    for h in [
        json!([{ "name": "Authorization", "value": "Bearer x" }]),
        json!([{ "name": "X-Api-Key", "value": "x" }]),
        json!([{ "name": "Cookie", "value": "s=1" }]),
    ] {
        assert_eq!(with(h.clone()), ["readiness[0].probe.headers[0].value"], "{h}");
    }

    assert_eq!(
        with(json!([{ "name": "X-Token", "secret": "plain-secret" }])),
        ["readiness[0].probe.headers[0].secret"]
    );
    assert_eq!(with(json!([{ "name": "A" }])), ["readiness[0].probe.headers[0]"]);
    assert_eq!(with(json!([{ "name": "A", "value": "x", "secret": "vault:a" }])), ["readiness[0].probe.headers[0]"]);
    assert_eq!(with(json!([{ "name": "Bad Name", "value": "x" }])), ["readiness[0].probe.headers[0].name"]);
}

/// D-111: a vault value leaves the node only over HTTPS with a verified
/// certificate; plain HTTP, or an unverified peer, only for the workload.
#[test]
fn vault_secrets_need_verified_https_beyond_the_workload() {
    let with = |extra: serde_json::Value| -> Vec<String> {
        let mut probe = json!({ "type": "http", "port": 8443,
            "headers": [{ "name": "Authorization", "secret": "vault:abc-123" }] });

        for (k, v) in extra.as_object().unwrap() {
            probe[k] = v.clone();
        }

        let c: Check = serde_json::from_value(json!({ "name": "a", "probe": probe })).unwrap();
        fields(&policy(vec![c]))
    };

    // The workload itself, in every spelling: any scheme.
    for host in ["", "localhost", "127.0.0.1", "::1", "::ffff:127.0.0.1"] {
        assert!(with(json!({ "host": host })).is_empty(), "{host}");
        assert!(with(json!({ "host": host, "scheme": "https", "tls_verify": false })).is_empty(), "{host}");
    }

    let remote = json!({ "host": "api.internal", "allow_remote_host": true });
    let and = |extra: serde_json::Value| {
        let mut v = remote.clone();

        for (k, x) in extra.as_object().unwrap() {
            v[k] = x.clone();
        }

        with(v)
    };
    assert_eq!(and(json!({})), ["readiness[0].probe.scheme"], "plain HTTP");
    assert_eq!(and(json!({ "scheme": "https", "tls_verify": false })), ["readiness[0].probe.tls_verify"]);
    assert!(and(json!({ "scheme": "https" })).is_empty());

    // Without a vault header the rule does not apply.
    let plain: Check = serde_json::from_value(json!({ "name": "a", "probe": { "type": "http", "port": 80,
        "host": "api.internal", "allow_remote_host": true } }))
    .unwrap();
    assert!(policy(vec![plain]).validate().is_empty());
}

/// The end of a failed run's output is reported only when the check asks
/// for it; the field is absent from the stored form otherwise.
#[test]
fn command_output_is_an_opt_in() {
    let p: Probe = serde_json::from_value(json!({ "type": "exec", "argv": ["x"] })).unwrap();
    let Probe::Exec(e) = &p else { panic!() };
    assert!(!e.show_output && !serde_json::to_string(&p).unwrap().contains("show_output"));
    let p: Probe = serde_json::from_value(json!({ "type": "shell", "script": "x", "show_output": true })).unwrap();
    let Probe::Shell(s) = &p else { panic!() };
    assert!(s.show_output && serde_json::to_string(&p).unwrap().contains("\"show_output\":true"));
}

#[test]
fn exec_and_shell_inputs_are_bounded() {
    let exec = |argv: serde_json::Value| -> Vec<String> {
        let c: Check =
            serde_json::from_value(json!({ "name": "a", "probe": { "type": "exec", "argv": argv } })).unwrap();
        fields(&policy(vec![c]))
    };
    assert_eq!(exec(json!([])), ["readiness[0].probe.argv"]);
    assert_eq!(exec(json!([" "])), ["readiness[0].probe.argv"]);
    assert!(exec(json!(["/bin/true"])).is_empty());
    let c: Check = serde_json::from_value(json!({
        "name": "a", "probe": { "type": "shell", "script": "", "expected_exit_codes": [256] }
    }))
    .unwrap();
    assert_eq!(fields(&policy(vec![c])), ["readiness[0].probe.script", "readiness[0].probe.expected_exit_codes"]);
}

#[test]
fn the_summary_reads_like_the_issue_example() {
    let mut c = http("", false);
    c.success_threshold = 2;
    assert_eq!(
        summary(Purpose::Readiness, &c),
        "Check GET http://the workload:8080/health/ready every 10 s (timeout 2 s). \
         Mark unready after 3 consecutive failures; ready again after 2 successes."
    );
    assert!(summary(Purpose::Liveness, &c).ends_with("Do not restart."));
}

#[test]
fn default_checks_are_on_unless_turned_off() {
    let p: HealthPolicy = serde_json::from_value(json!({})).unwrap();
    assert!(p.default_checks && p.is_empty());
    assert_eq!(serde_json::to_value(&p).unwrap(), json!({}), "the default isn't written out");
    let off: HealthPolicy = serde_json::from_value(json!({ "default_checks": false })).unwrap();
    assert!(!off.default_checks && !off.is_empty(), "turning them off must be stored");
    assert_eq!(serde_json::to_value(&off).unwrap(), json!({ "default_checks": false }));
    assert!(off.validate().is_empty());
}

#[test]
fn encoded_and_embedded_forms_of_metadata_addresses_are_refused() {
    // Decimal, hex and octal spellings of 169.254.169.254, and digit-only
    // last labels a resolver might read as an address.
    for host in ["2852039166", "0xa9fea9fe", "0251.0376.0251.0376", "0xA9.0xFE.0xA9.0xFE", "169.254.43518", "x.123"] {
        assert_eq!(fields(&policy(vec![http(host, true)])), ["readiness[0].probe.host"], "{host}");
    }

    // IPv4 embedded in IPv6: mapped, compatible and NAT64.
    for host in ["::ffff:169.254.169.254", "::ffff:a9fe:a9fe", "::a9fe:a9fe", "::169.254.169.254", "64:ff9b::a9fe:a9fe"]
    {
        assert_eq!(fields(&policy(vec![http(host, true)])), ["readiness[0].probe.host"], "{host}");
    }

    // IPv6 transition prefixes (6to4, Teredo, local-use NAT64,
    // IPv4-translated): a relay delivers them to the IPv4 address they name,
    // so they are refused whatever that address is.
    for host in
        ["2002:a9fe:a9fe::1", "64:ff9b:1::a9fe:a9fe", "2001:0:a9fe:a9fe::1", "::ffff:0:a9fe:a9fe", "2002:a00:5::1"]
    {
        let errs = policy(vec![http(host, true)]).validate();
        assert_eq!(errs.len(), 1, "{host}: {errs:?}");
        assert_eq!(errs[0].field, "readiness[0].probe.host", "{host}");
    }

    let ip = |s: &str| s.parse::<std::net::IpAddr>().unwrap();

    // The fleet list travels with the policy and is not part of the revision.
    let spec: ApplySpec = serde_json::from_value(serde_json::json!({
        "environment": "uat", "name": "api", "runtime": "systemd", "revision": 1, "fleet": ["10.0.0.7", "fd00::7"]
    }))
    .unwrap();
    assert_eq!(spec.fleet, [ip("10.0.0.7"), ip("fd00::7")]);
    assert!(!serde_json::to_string(&ApplySpec { fleet: Vec::new(), ..spec }).unwrap().contains("fleet"));
    assert_eq!(canonical(ip("64:ff9b::a00:1")), ip("10.0.0.1"));
    assert_eq!(canonical(ip("::1")), ip("::1"), "loopback is not an embedded IPv4");
    assert_eq!(canonical(ip("::")), ip("::"));
    assert_eq!(canonical(ip("2001:db8::1")), ip("2001:db8::1"));

    // Ordinary hosts still pass.
    for host in ["10.0.0.5", "api.internal", "db-1.example.com", "::ffff:10.0.0.5"] {
        assert!(policy(vec![http(host, true)]).validate().is_empty(), "{host}");
    }

    // An embedded loopback counts as the workload (no allow needed).
    assert!(policy(vec![http("::ffff:127.0.0.1", false)]).validate().is_empty());
}

#[test]
fn revisions_follow_the_policy_content() {
    let empty = HealthPolicy::default();
    assert_eq!(revision(Some(8080), &empty), 0, "defaults only");
    let p: HealthPolicy = serde_json::from_value(serde_json::json!({
        "liveness": [{ "name": "http", "probe": { "type": "http", "port": 8080, "path": "/healthz" } }]
    }))
    .unwrap();
    let r = revision(Some(8080), &p);
    assert!(r > 0);
    assert_eq!(r, revision(Some(8080), &p.clone()), "stable");
    assert_ne!(r, revision(Some(8081), &p), "the port is part of it");
    let off = HealthPolicy { default_checks: false, ..HealthPolicy::default() };
    assert!(revision(None, &off) > 0, "turning the defaults off is a policy");
}
