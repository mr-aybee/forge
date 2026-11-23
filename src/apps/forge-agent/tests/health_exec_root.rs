//! D-111 command health checks with real accounts: needs root, so it runs
//! only with `FORGE_ROOT_TESTS=1` as root. In WSL, build as your user
//! (`cargo test -p forge-agent --test health_exec_root --no-run`) and run the
//! printed test binary with `wsl -u root env FORGE_ROOT_TESTS=1 <binary>`.
//! It creates two app accounts under the environment `f2test` and removes
//! them, their folders and the cgroups of the runs afterwards.
//!
//! It needs cgroup v2 with the pids and memory controllers (every run is
//! held in a cgroup of its own). On a kernel that does not enforce Landlock
//! ABI 3 the test proves the refusal and stops there, unless it was built
//! with `--features test-unconfined`: then the rest runs unconfined, so the
//! account switch, environment, timeout and containment are still
//! exercised, and the write confinement is reported as not tested.
#![cfg(target_os = "linux")]
#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::os::unix::process::CommandExt;
use std::process::Command;
use std::time::{Duration, Instant};

use forge_agent::health_exec::{CheckRef, ExecResult, Refusal, RunOptions, Runner, Times, landlock_abi, store};
use forge_catalog::health::{ApplySpec, Purpose};
use serde_json::json;

const NS: &str = "f2test";
const REVISION: i64 = 41;

fn root_tests() -> bool {
    std::env::var_os("FORGE_ROOT_TESTS").is_some() && rustix::process::geteuid().is_root()
}

fn sh(cmd: &str) -> String {
    let out = Command::new("sh").args(["-c", cmd]).output().unwrap();
    String::from_utf8_lossy(&out.stdout).trim().to_owned()
}

fn now_ms() -> i64 {
    i64::try_from(std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_millis()).unwrap()
}

/// An app account the way the agent creates one (D-081).
fn make_app(name: &str) -> (u32, u32) {
    let home = format!("/srv/forge/apps/{NS}/{name}");
    let user = format!("{NS}-{name}");
    let nologin =
        ["/usr/sbin/nologin", "/sbin/nologin"].into_iter().find(|p| std::path::Path::new(p).exists()).unwrap();
    sh(&format!(
        "useradd --system --user-group --home-dir {home} --shell {nologin} --comment 'Orbyn Forge app {NS}/{name}' {user}; \
         mkdir -p {home}/.forge && chmod 0750 {home} && chown {user}:{user} {home} && chown root:root {home}/.forge"
    ));
    (sh(&format!("id -u {user}")).parse().unwrap(), sh(&format!("id -g {user}")).parse().unwrap())
}

fn cleanup() {
    for name in ["alpha", "beta"] {
        sh(&format!("pkill -KILL -u {NS}-{name} 2>/dev/null; true"));
        sh(&format!("userdel {NS}-{name} 2>/dev/null; groupdel {NS}-{name} 2>/dev/null; true"));
    }
    // The parents only if this test made them and nothing else is in them.
    sh(&format!("rm -rf /srv/forge/apps/{NS} /tmp/{NS}-probe; rmdir /srv/forge/apps /srv/forge 2>/dev/null; true"));
    sh(&format!("rmdir /sys/fs/cgroup/forge-health/{NS}--* /sys/fs/cgroup/forge-health 2>/dev/null; true"));
}

/// A shell check whose failed runs report their output, so the test can
/// read what the command saw.
fn shell(name: &str, script: &str) -> serde_json::Value {
    json!({ "name": name, "period_secs": 30, "timeout_secs": 5,
        "probe": { "type": "shell", "script": script, "show_output": true } })
}

fn policy() -> serde_json::Value {
    let home = format!("/srv/forge/apps/{NS}/alpha");
    // At most ten checks per purpose: `LIVENESS` and `STARTUP` name the rest.
    json!({ "startup": [
        { "name": "bomb", "period_secs": 30, "timeout_secs": 2,
          "probe": { "type": "shell", "script": "f() { f | f & }; f; sleep 30" } }
    ], "liveness": [
        { "name": "again", "probe": { "type": "exec", "argv": ["/bin/true"] } },
        shell("signal", "kill -9 $$"),
        shell("big", "head -c 3000000 /dev/zero | tr '\\0' 'x'; echo; echo THE-END; exit 3"),
        { "name": "slow", "period_secs": 30, "timeout_secs": 1,
          "probe": { "type": "shell", "script": "sleep 300 & sleep 300" } },
        { "name": "bash", "probe": { "type": "shell", "interpreter": "bash", "script": "[[ -n $BASH_VERSION ]]" } },
        shell("writes", &format!(
            "touch {home}/made 2>/dev/null && echo HOME; (echo x > /tmp/{NS}-probe) 2>/dev/null && echo TMP; \
             (echo x >> {home}/check.sh) 2>/dev/null && echo APPEND; (: > {home}/check.sh) 2>/dev/null && echo TRUNCATE; \
             (rm {home}/check.sh) 2>/dev/null && echo REMOVE; chmod 0750 {home}/check.sh 2>/dev/null && echo CHMOD; \
             echo x > /dev/null && echo NULL; exit 3")),
        shell("other", &format!("cat /srv/forge/apps/{NS}/beta/secret.txt 2>&1 | head -c 40; exit 3")),
        // Output is not reported unless the check asks for it.
        { "name": "quiet", "probe": { "type": "shell", "script": "echo token=hunter2; exit 3" } },
        // A command that leaves its process group and session behind it
        // (with its output closed: one that keeps the output open has not
        // finished, and runs into the timeout like `runaway`).
        { "name": "daemon", "probe": { "type": "shell", "script": "setsid sleep 300 >/dev/null 2>&1 & setsid sh -c 'sleep 300' >/dev/null 2>&1 & exit 0" } },
        { "name": "runaway", "period_secs": 30, "timeout_secs": 1,
          "probe": { "type": "shell", "script": "setsid sleep 300 & setsid sleep 300" } }
    ], "readiness": [
        { "name": "pass", "probe": { "type": "exec", "argv": ["/bin/true"] } },
        { "name": "codes", "probe": { "type": "exec", "argv": ["/bin/sh", "-c", "exit 3"], "expected_exit_codes": [0, 3] } },
        { "name": "argv", "probe": { "type": "exec", "show_output": true,
            "argv": ["/bin/sh", "-c", "echo \"$0|$1|$2\"; exit 3", "a b", "$(id)", "; rm -rf /"] } },
        { "name": "relative", "probe": { "type": "exec", "argv": ["./check.sh"] } },
        { "name": "missing", "probe": { "type": "exec", "argv": ["/nonexistent/prog"] } },
        shell("ids", "echo \"$(id -u):$(id -g):$(id -G)\"; exit 3"),
        shell("env", "env | cut -d= -f1 | sort | tr '\\n' ' '; exit 3"),
        shell("where", "echo \"$PWD $HOME $USER\"; exit 3"),
        shell("privs", "grep NoNewPrivs /proc/self/status; exit 3"),
        shell("cmdline", "tr '\\0' ' ' < /proc/$$/cmdline; echo; exit 3")
    ] })
}

/// The checks of [`policy`] that are not readiness checks.
const LIVENESS: [&str; 10] =
    ["again", "signal", "big", "slow", "bash", "writes", "other", "quiet", "daemon", "runaway"];
const STARTUP: [&str; 1] = ["bomb"];

fn spec(name: &str, runtime: &str) -> ApplySpec {
    serde_json::from_value(json!({
        "environment": NS, "name": name, "runtime": runtime, "revision": REVISION, "policy": policy()
    }))
    .unwrap()
}

fn check(app: &str, name: &str) -> CheckRef {
    let purpose = if LIVENESS.contains(&name) {
        Purpose::Liveness
    } else if STARTUP.contains(&name) {
        Purpose::Startup
    } else {
        Purpose::Readiness
    };
    CheckRef { environment: NS.into(), app: app.into(), purpose, check: name.into(), revision: REVISION }
}

/// How the test's runner starts workers. `unconfined` only has an effect
/// in a build with the `test-unconfined` feature.
fn options(worker: &str, unconfined: bool) -> RunOptions {
    #[cfg(feature = "test-unconfined")]
    {
        RunOptions { worker_exe: Some(worker.into()), allow_unconfined: unconfined }
    }
    #[cfg(not(feature = "test-unconfined"))]
    {
        let _ = unconfined;
        RunOptions { worker_exe: Some(worker.into()) }
    }
}

fn output(r: &ExecResult) -> &str {
    match r {
        ExecResult::Exited { code: 3, passed: false, output } => output,
        other => panic!("expected a failed run with exit code 3, got {other:?}"),
    }
}

/// Processes of the app's account, by name.
fn processes() -> String {
    sh(&format!("pgrep -u {NS}-alpha -l | tr '\\n' ' '"))
}

/// Waits until nothing runs as the app's account any more.
async fn nothing_left(what: &str) {
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        let left = processes();
        if left.is_empty() {
            return;
        }
        assert!(Instant::now() < deadline, "{what}: processes survived the run: {left}");
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

#[allow(clippy::too_many_lines)] // one account setup and teardown around every case
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn command_checks_run_as_the_app_confined_or_not_at_all() {
    if !root_tests() {
        eprintln!("skipped: needs root and FORGE_ROOT_TESTS=1");
        return;
    }
    cleanup();
    // The app accounts must be able to run the helper: a copy outside the
    // build user's (private) home.
    let helper = format!("/tmp/forge-agent-helper-hc-test-{}", std::process::id());
    std::fs::copy(env!("CARGO_BIN_EXE_forge-agent-helper"), &helper).unwrap();
    sh(&format!("chmod 0755 {helper}"));
    let state = tempfile::tempdir().unwrap();
    let dir = state.path().join("health");
    let (uid, gid) = make_app("alpha");
    let (beta_uid, beta_gid) = make_app("beta");
    let home = format!("/srv/forge/apps/{NS}/alpha");
    sh(&format!(
        "printf '#!/bin/sh\\nexit 0\\n' > {home}/check.sh && chmod 0755 {home}/check.sh && chown {NS}-alpha:{NS}-alpha {home}/check.sh; \
         echo beta-secret > /srv/forge/apps/{NS}/beta/secret.txt && chown {NS}-beta:{NS}-beta /srv/forge/apps/{NS}/beta/secret.txt && \
         chmod 0640 /srv/forge/apps/{NS}/beta/secret.txt"
    ));
    let worker = helper.clone();
    let result = std::panic::AssertUnwindSafe(async move {
        let at = Times { issued_at_ms: now_ms(), now_ms: now_ms() };
        store(&dir, &spec("alpha", "systemd"), at).unwrap();
        store(&dir, &spec("ghost", "systemd"), at).unwrap();
        store(&dir, &spec("ctr", "container"), at).unwrap();
        let strict = Runner::with_options(dir.clone(), options(&worker, false));

        // What the helper never runs, whatever the kernel: a check it does
        // not hold, another revision, an app without its own account.
        let refused = |r: ExecResult| match r {
            ExecResult::Refused { why } => why,
            other => panic!("expected a refusal, got {other:?}"),
        };
        assert_eq!(refused(strict.run(&check("alpha", "nope")).await), Refusal::NoSuchCheck);
        assert_eq!(refused(strict.run(&check("nobody", "pass")).await), Refusal::NoPolicy);
        let stale = CheckRef { revision: REVISION + 1, ..check("alpha", "pass") };
        assert_eq!(refused(strict.run(&stale).await), Refusal::StalePolicy);
        assert_eq!(refused(strict.run(&check("ghost", "pass")).await), Refusal::NoAccount);
        // No Docker in this environment: a container app's check is refused, not run on the host.
        if !std::path::Path::new("/var/run/docker.sock").exists() {
            assert_eq!(refused(strict.run(&check("ctr", "pass")).await), Refusal::NoDocker);
        }
        // The signed checks are in a folder only root can read.
        let mode = std::os::unix::fs::MetadataExt::mode(&std::fs::metadata(&dir).unwrap()) & 0o777;
        assert_eq!(mode, 0o700);
        // A policy the helper has not heard of for too long is not run.
        let old = now_ms() - 31 * 60 * 1000;
        let aged = state.path().join("aged");
        store(&aged, &spec("alpha", "systemd"), Times { issued_at_ms: old, now_ms: old }).unwrap();
        let aged = Runner::with_options(aged, options(&worker, false));
        assert_eq!(refused(aged.run(&check("alpha", "pass")).await), Refusal::PolicyExpired);

        // Does this kernel enforce Landlock with truncate control? If not, nothing runs.
        let abi = landlock_abi();
        let first = strict.run(&check("alpha", "pass")).await;
        let landlock = first != ExecResult::Refused { why: Refusal::NotConfined };
        eprintln!("landlock ABI of this kernel: {abi}; enforced for command checks: {landlock} (first run: {first:?})");
        assert_eq!(landlock, abi >= 3, "command checks run exactly where Landlock ABI 3 is enforced");
        assert_ne!(
            first,
            ExecResult::Refused { why: Refusal::NotContained },
            "this test needs cgroup v2 with pids and memory"
        );
        let runner = if landlock { strict } else { Runner::with_options(dir.clone(), options(&worker, true)) };
        if !landlock && cfg!(not(feature = "test-unconfined")) {
            eprintln!("NOT TESTED on this kernel (no Landlock ABI 3): everything past the refusal");
            return;
        }
        if landlock {
            assert_eq!(first, ExecResult::Exited { code: 0, passed: true, output: String::new() });
        }
        let runner = std::sync::Arc::new(runner);
        let run = |name: &'static str| {
            let runner = runner.clone();
            async move { runner.run(&check("alpha", name)).await }
        };
        let passed = |code| ExecResult::Exited { code, passed: true, output: String::new() };

        // Exit codes decide; a pass returns no output.
        assert_eq!(run("again").await, passed(0));
        // Asked again at once: the helper keeps the starts of one check apart.
        assert_eq!(run("again").await, ExecResult::Refused { why: Refusal::Throttled });
        assert_eq!(run("codes").await, passed(3));
        assert_eq!(run("relative").await, passed(0));
        assert_eq!(run("bash").await, passed(0));
        assert!(matches!(run("missing").await, ExecResult::StartFailed { .. }));
        assert_eq!(run("signal").await, ExecResult::Signaled { signal: 9 });
        // A failed run reports its exit code only, unless the check asks for more.
        assert_eq!(run("quiet").await, ExecResult::Exited { code: 3, passed: false, output: String::new() });

        // The app's uid and gid, and no other group.
        assert_eq!(output(&run("ids").await), format!("{uid}:{gid}:{gid}"));
        // A minimal environment: nothing of the helper's, nothing of the app's.
        let env = run("env").await;
        let keys: Vec<&str> = output(&env).split(' ').collect();
        for k in &keys {
            assert!(
                ["HOME", "LANG", "LOGNAME", "PATH", "PWD", "USER", "OLDPWD", "SHLVL", "_"].contains(k),
                "{k} in {keys:?}"
            );
        }
        for k in ["HOME", "PATH", "USER", "LOGNAME"] {
            assert!(keys.contains(&k), "{k} missing in {keys:?}");
        }
        assert_eq!(output(&run("where").await), format!("{home} {home} {NS}-alpha"));
        // Arguments reach the program as they are: no shell of ours reads them.
        assert_eq!(output(&run("argv").await), "a b|$(id)|; rm -rf /");
        // A shell probe's script is its interpreter's stdin, not an argument.
        assert_eq!(output(&run("cmdline").await), "sh");
        // No new privileges through setuid programs.
        let privs = run("privs").await;
        assert!(output(&privs).ends_with('1'), "no_new_privs is not set: {privs:?}");
        // Another app's files stay out of reach (its account, its mode).
        let other = run("other").await;
        assert!(!output(&other).contains("beta-secret"), "{other:?}");
        // Output is read to the end; only a short clean tail is kept.
        let big = run("big").await;
        assert!(
            output(&big).ends_with("THE-END") && output(&big).len() <= forge_agent::health_exec::MAX_DETAIL,
            "{big:?}"
        );

        // Files: a check can neither create, write, append, truncate nor
        // remove one, not even the app's own. What Landlock does not cover
        // is the mode of a file the account owns, and `/dev/null` is open.
        let writes = run("writes").await;
        if landlock {
            assert_eq!(output(&writes), "CHMOD NULL", "what a confined check could change");
            assert!(!std::path::Path::new(&format!("{home}/made")).exists());
            assert!(!std::path::Path::new(&format!("/tmp/{NS}-probe")).exists());
            assert_eq!(std::fs::read_to_string(format!("{home}/check.sh")).unwrap(), "#!/bin/sh\nexit 0\n");
        } else {
            eprintln!(
                "NOT TESTED on this kernel (no Landlock ABI 3): write confinement; unconfined run did: {writes:?}"
            );
        }

        // Nothing outlives a run. A command that passes but left daemons
        // in sessions of their own: they die with the run's cgroup.
        assert_eq!(run("daemon").await, passed(0));
        nothing_left("after a passing run that started daemons").await;
        // The same when the timeout ends it: no process group to escape from.
        let started = Instant::now();
        assert_eq!(run("runaway").await, ExecResult::TimedOut);
        assert!(started.elapsed() < Duration::from_secs(4), "{:?}", started.elapsed());
        nothing_left("after a timed-out run that left its process group").await;

        // A fork bomb stays within the run's process limit and dies with it.
        let sampler = async {
            let mut most = 0usize;
            let end = Instant::now() + Duration::from_millis(1800);
            while Instant::now() < end {
                most = most.max(sh(&format!("pgrep -u {NS}-alpha -c")).parse().unwrap_or(0));
                tokio::time::sleep(Duration::from_millis(100)).await;
            }
            most
        };
        let (bomb, most) = tokio::join!(run("bomb"), sampler);
        eprintln!("fork bomb: {bomb:?}; most processes seen at once: {most}");
        assert!(matches!(bomb, ExecResult::TimedOut | ExecResult::Exited { passed: false, .. }), "{bomb:?}");
        assert!((2..=64).contains(&most), "the bomb was not held by pids.max (64): {most}");
        nothing_left("after a fork bomb").await;

        // The timeout ends the run, and a check never overlaps itself.
        let started = Instant::now();
        let (first, second) = tokio::join!(run("slow"), async {
            tokio::time::sleep(Duration::from_millis(300)).await;
            run("slow").await
        });
        assert_eq!(first, ExecResult::TimedOut);
        assert_eq!(second, ExecResult::Refused { why: Refusal::Busy });
        assert!(started.elapsed() < Duration::from_secs(4), "{:?}", started.elapsed());
        nothing_left("after a timeout").await;
        // And it can run again afterwards.
        assert_eq!(run("slow").await, ExecResult::TimedOut);
        nothing_left("after the second timeout").await;

        // The cgroups of the runs are gone with them.
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            let left = sh(&format!("ls -d /sys/fs/cgroup/forge-health/{NS}--* 2>/dev/null | tr '\\n' ' '"));
            if left.is_empty() {
                break;
            }
            assert!(Instant::now() < deadline, "cgroups of finished runs remain: {left}");
            tokio::time::sleep(Duration::from_millis(100)).await;
        }

        // The worker itself refuses ids that are not all the app's, and root.
        let direct = |args: [String; 4], as_uid: u32, as_gid: u32| {
            let out = Command::new(&worker)
                .arg("health-worker")
                .args(args)
                .env_clear()
                .uid(as_uid)
                .gid(as_gid)
                .stdin(std::process::Stdio::null())
                .output()
                .unwrap();
            String::from_utf8_lossy(&out.stdout).into_owned()
        };
        let mixed = direct([beta_uid.to_string(), beta_gid.to_string(), home.clone(), "5".into()], uid, gid);
        assert!(mixed.contains("\"kind\":\"refused\"") && mixed.contains("does not run as the app"), "{mixed}");
        let as_root = direct(["0".into(), "0".into(), home.clone(), "5".into()], 0, 0);
        assert!(as_root.contains("\"kind\":\"refused\"") && as_root.contains("never runs as root"), "{as_root}");
        // A release build's worker knows no way to run unconfined.
        if cfg!(not(feature = "test-unconfined")) {
            let out = Command::new(&worker)
                .args(["health-worker", &uid.to_string(), &gid.to_string(), &home, "5", "--unconfined-ok"])
                .env_clear()
                .uid(uid)
                .gid(gid)
                .stdin(std::process::Stdio::null())
                .output()
                .unwrap();
            let answer = String::from_utf8_lossy(&out.stdout);
            assert!(answer.contains("\"kind\":\"refused\"") && answer.contains("usage"), "{answer}");
        }
    });
    let outcome = tokio::spawn(result.0).await.map_err(|e| e.into_panic());
    cleanup();
    let _ = std::fs::remove_file(&helper);
    if let Err(e) = outcome {
        std::panic::resume_unwind(e);
    }
}
