//! D-083 confinement with real accounts: needs root, so it runs only with
//! `FORGE_ROOT_TESTS=1` as root. In WSL, build as your user
//! (`cargo test -p forge-agent --test app_worker_root --no-run`) and run the
//! printed test binary with `wsl -u root env FORGE_ROOT_TESTS=1 <binary>`.
//! It creates two app accounts under the environment `f1test` and removes
//! them, their folders and stores afterwards.
#![cfg(target_os = "linux")]
#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::os::unix::fs::MetadataExt;
use std::os::unix::process::CommandExt;
use std::process::Command;

use forge_core::files::{FileOp, FilePath, RootSpec};

const NS: &str = "f1test";

fn root_tests() -> bool {
    std::env::var_os("FORGE_ROOT_TESTS").is_some() && rustix::process::geteuid().is_root()
}

fn sh(cmd: &str) -> String {
    let out = Command::new("sh").args(["-c", cmd]).output().unwrap();
    String::from_utf8_lossy(&out.stdout).trim().to_owned()
}

fn p(s: &str) -> FilePath {
    FilePath::parse(s).unwrap()
}

fn app(name: &str) -> RootSpec {
    RootSpec::App { environment: NS.into(), app: name.into() }
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
    let uid: u32 = sh(&format!("id -u {user}")).parse().unwrap();
    let gid: u32 = sh(&format!("id -g {user}")).parse().unwrap();
    (uid, gid)
}

fn cleanup() {
    for name in ["alpha", "beta"] {
        sh(&format!("userdel {NS}-{name} 2>/dev/null; groupdel {NS}-{name} 2>/dev/null; true"));
    }
    sh(&format!("rm -rf /srv/forge/apps/{NS} /etc/forge/apps/{NS} /var/lib/forge-app-store/{NS}"));
}

async fn run(op: FileOp) -> Result<serde_json::Value, String> {
    forge_agent::files::run_async(op).await
}

#[tokio::test]
async fn app_folders_are_worked_on_as_the_app() {
    if !root_tests() {
        eprintln!("skipped: needs root and FORGE_ROOT_TESTS=1");
        return;
    }
    cleanup();
    // The app accounts must be able to run the helper: a copy outside the
    // build user's (private) home.
    let helper = format!("/tmp/forge-agent-helper-test-{}", std::process::id());
    std::fs::copy(env!("CARGO_BIN_EXE_forge-agent-helper"), &helper).unwrap();
    sh(&format!("chmod 0755 {helper}"));
    forge_agent::files::app_worker::worker_exe_override(std::path::Path::new(&helper));
    let (alpha_uid, alpha_gid) = make_app("alpha");
    let (beta_uid, beta_gid) = make_app("beta");
    let alpha = format!("/srv/forge/apps/{NS}/alpha");
    let beta = format!("/srv/forge/apps/{NS}/beta");
    sh(&format!(
        "echo hello > {alpha}/hello.txt && chown {NS}-alpha:{NS}-alpha {alpha}/hello.txt; \
         echo secret > {beta}/secret.txt && chown {NS}-beta:{NS}-beta {beta}/secret.txt && chmod 0640 {beta}/secret.txt; \
         ln -s /etc/shadow {alpha}/shadow-link; ln -s {beta}/secret.txt {alpha}/beta-link; \
         mkdir -p /etc/forge/apps/{NS} && chmod 0711 /etc/forge/apps/{NS} && echo K=v > /etc/forge/apps/{NS}/beta.env && chown root:{NS}-beta /etc/forge/apps/{NS}/beta.env && chmod 0640 /etc/forge/apps/{NS}/beta.env"
    ));
    let worker = helper.clone();
    let result = std::panic::AssertUnwindSafe(async move {
        // Listing and reading the app's own files works, as the app.
        let list =
            run(FileOp::List { root: app("alpha"), path: p("/"), offset: 0, limit: 100, hidden: true }).await.unwrap();
        assert_eq!(list["as_reported"], format!("{NS}-alpha"), "{list}");
        let text = list.to_string();
        assert!(text.contains("hello.txt") && text.contains("shadow-link"), "{text}");
        let read =
            run(FileOp::Read { root: app("alpha"), path: p("/hello.txt"), offset: 0, length: 100, version: None })
                .await;
        assert!(read.is_ok(), "{read:?}");
        // Links are listed, never followed.
        for link in ["/shadow-link", "/beta-link"] {
            let r =
                run(FileOp::Read { root: app("alpha"), path: p(link), offset: 0, length: 100, version: None }).await;
            assert!(r.is_err(), "{link} was followed: {r:?}");
        }
        // New files belong to the app.
        run(FileOp::Mkdir { root: app("alpha"), path: p("/made"), parents: false }).await.unwrap();
        assert_eq!(std::fs::metadata(format!("{alpha}/made")).unwrap().uid(), alpha_uid);
        let data = b"uploaded by the app";
        let sha = forge_secrets::crypto::sha256_hex(data);
        let b64 = base64::Engine::encode(&base64::engine::general_purpose::STANDARD, data);
        run(FileOp::UploadChunk {
            root: app("alpha"),
            path: p("/up.txt"),
            upload: "u1".into(),
            offset: 0,
            data_b64: b64,
        })
        .await
        .unwrap();
        run(FileOp::UploadCommit {
            root: app("alpha"),
            path: p("/up.txt"),
            upload: "u1".into(),
            size: data.len() as u64,
            sha256: sha,
            overwrite: forge_core::files::Overwrite::Never,
        })
        .await
        .unwrap();
        assert_eq!(std::fs::metadata(format!("{alpha}/up.txt")).unwrap().uid(), alpha_uid);
        // Trash goes to the app's store, outside its folder.
        run(FileOp::Delete { root: app("alpha"), path: p("/hello.txt"), permanent: false }).await.unwrap();
        let store = format!("/var/lib/forge-app-store/{NS}/alpha");
        assert_eq!(std::fs::metadata(&store).unwrap().uid(), alpha_uid);
        assert!(!sh(&format!("find {store} -type f | head -1")).is_empty(), "nothing in the app's store");
        // Never: owners, the launcher folder, the folder itself, folders around apps.
        let own = FileOp::SetOwner {
            root: app("alpha"),
            path: p("/up.txt"),
            owner: Some("root".into()),
            group: None,
            recursive: false,
            dry_run: false,
        };
        assert!(run(own).await.is_err());
        assert!(run(FileOp::Delete { root: app("alpha"), path: p("/.forge"), permanent: false }).await.is_err());
        let around = RootSpec::Dir { base: p(&format!("/srv/forge/apps/{NS}")) };
        assert!(run(FileOp::List { root: around, path: p("/"), offset: 0, limit: 10, hidden: false }).await.is_err());
        // Even if the helper picked the wrong folder, the kernel stops the
        // app's account: a worker as alpha on beta's folder gets nothing.
        let op = FileOp::Read {
            root: RootSpec::Dir { base: p(&beta) },
            path: p("/secret.txt"),
            offset: 0,
            length: 100,
            version: None,
        };
        let out = Command::new(&worker)
            .args(["file-worker", &alpha_uid.to_string(), &alpha_gid.to_string(), &beta, "/nonexistent-store"])
            .env_clear()
            .uid(alpha_uid)
            .gid(alpha_gid)
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .spawn()
            .map(|mut c| {
                use std::io::Write;
                c.stdin.take().unwrap().write_all(serde_json::to_string(&op).unwrap().as_bytes()).unwrap();
                c.wait_with_output().unwrap()
            })
            .unwrap();
        let answer = String::from_utf8_lossy(&out.stdout);
        assert!(answer.contains("\"ok\":false"), "alpha read beta's file: {answer}");
        // Landlock ran (full, partial or none on a kernel without it), never failed.
        eprintln!("worker confinement on this kernel: {answer}");
        assert!(answer.contains("\"confined\":\"") && !answer.contains("\"confined\":\"error"), "{answer}");
        let env = Command::new(&worker)
            .args([
                "file-worker",
                &alpha_uid.to_string(),
                &alpha_gid.to_string(),
                "/etc/forge/apps",
                "/nonexistent-store",
            ])
            .env_clear()
            .uid(alpha_uid)
            .gid(alpha_gid)
            .stdin(std::process::Stdio::null())
            .output()
            .unwrap();
        assert!(String::from_utf8_lossy(&env.stdout).contains("\"ok\":false"));
        // The worker refuses ids that are not all the app's.
        let mixed = Command::new(&worker)
            .args(["file-worker", &beta_uid.to_string(), &beta_gid.to_string(), &beta, "/nonexistent-store"])
            .env_clear()
            .uid(alpha_uid)
            .gid(alpha_gid)
            .stdin(std::process::Stdio::null())
            .output()
            .unwrap();
        assert!(String::from_utf8_lossy(&mixed.stdout).contains("does not run as the app"));
    });
    let outcome = futures_catch(result).await;
    cleanup();
    let _ = std::fs::remove_file(&helper);
    if let Err(e) = outcome {
        std::panic::resume_unwind(e);
    }
}

/// Runs the checks and always returns, so cleanup runs after a failure.
async fn futures_catch<F: std::future::Future<Output = ()> + Send + 'static>(
    f: std::panic::AssertUnwindSafe<F>,
) -> Result<(), Box<dyn std::any::Any + Send>> {
    tokio::spawn(f.0).await.map_err(|e| e.into_panic())
}
