//! File explorer API against a real enrolled agent: authorization, roots,
//! path validation, the protected-path policy and audit on every platform;
//! on Windows (where the agent runs file operations in process) the whole
//! read-write flow against the real filesystem.
#![allow(clippy::unwrap_used)]

use std::time::Duration;

use crate::common::*;
use forge_agent::enroll::{HostFacts, enroll};
use forge_agent::{Agent, AgentConfig, StatePaths};
use reqwest::StatusCode;
use serde_json::{Value, json};

struct Enrolled {
    server_id: String,
    stop: tokio::sync::watch::Sender<bool>,
    _dir: tempfile::TempDir,
}

async fn enroll_and_run(h: &Harness, admin: &Client, os: &str) -> Enrolled {
    let (s, body) = admin.post("/api/v1/enrollment-tokens", json!({"ttl_minutes": 10})).await;
    assert_eq!(s, StatusCode::CREATED, "{body}");
    let dir = tempfile::tempdir().unwrap();
    let paths = StatePaths::new(dir.path());
    let config = AgentConfig::new(h.agent_url.clone());
    let facts = HostFacts { hostname: "files-01".into(), os: os.into(), arch: "x86_64".into() };
    let id = enroll(&paths, config, body["token"].as_str().unwrap(), facts).await.unwrap();
    let agent = Agent::load(paths).unwrap();
    let (stop, rx) = tokio::sync::watch::channel(false);
    tokio::spawn(agent.run(rx));
    let deadline = tokio::time::Instant::now() + Duration::from_secs(60);

    loop {
        let (_, s) = admin.get(&format!("/api/v1/servers/{}", id.server_id)).await;
        if s["status"] == "online" {
            break;
        }

        assert!(tokio::time::Instant::now() < deadline, "agent never came online");
        tokio::time::sleep(Duration::from_millis(100)).await;
    }

    Enrolled { server_id: id.server_id, stop, _dir: dir }
}

/// Enrols TOTP for the signed-in user and steps the session up to MFA.
#[cfg(windows)]
async fn step_up(c: &mut Client) {
    use forge_identity::totp;
    let (_, t) = c.post("/api/v1/auth/totp", json!({})).await;
    let secret = totp::base32_decode(t["secret"].as_str().unwrap()).unwrap();
    let code = || {
        let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_secs();
        format!("{:06}", totp::code_at(&secret, i64::try_from(now).unwrap()))
    };
    let (s, _) =
        c.post("/api/v1/auth/totp/confirm", json!({"password": PASSWORD, "secret": t["secret"], "code": code()})).await;
    assert_eq!(s, StatusCode::NO_CONTENT);
    let (s, r) = c.post("/api/v1/auth/step-up", json!({"code": code()})).await;
    assert_eq!(s, StatusCode::OK, "{r}");
    c.csrf = r["csrf_token"].as_str().map(str::to_owned);
}

fn q(s: &str) -> String {
    s.bytes()
        .map(|b| {
            if b.is_ascii_alphanumeric() || b == b'/' || b == b'.' || b == b'-' || b == b'_' {
                char::from(b).to_string()
            } else {
                format!("%{b:02X}")
            }
        })
        .collect()
}

async fn audit(admin: &Client) -> Vec<Value> {
    let (_, a) = admin.get("/api/v1/audit?limit=500").await;
    a.as_array().cloned().or_else(|| a["items"].as_array().cloned()).unwrap_or_default()
}

#[tokio::test]
async fn file_api_enforces_roles_roots_paths_and_protection() {
    let h = start().await;
    let admin = Client::login(&h.base, ADMIN_EMAIL, PASSWORD).await;
    let a = enroll_and_run(&h, &admin, "linux").await;
    let base = format!("/api/v1/servers/{}/files", a.server_id);

    let (s, roots) = admin.get(&format!("{base}/roots")).await;
    assert_eq!(s, StatusCode::OK, "{roots}");
    assert_eq!(roots["roots"][0]["id"], "node");
    assert_eq!(roots["roots"][0]["permissions"]["read"], true);

    // Customers may see the server but not its files.
    let customer =
        user_with_role(&h, &admin, "customer@example.com", "customer", &format!("workspace:{}", h.default_org)).await;
    let (s, _) = customer.get(&format!("{base}/roots")).await;
    assert_eq!(s, StatusCode::FORBIDDEN);

    // Operators may read files, but not the whole-node root (`file.node`).
    let operator =
        user_with_role(&h, &admin, "operator@example.com", "operator", &format!("workspace:{}", h.default_org)).await;
    let (s, r) = operator.get(&format!("{base}/roots")).await;
    assert_eq!(s, StatusCode::OK, "{r}");
    assert_eq!(r["roots"].as_array().map(Vec::len), Some(0), "{r}");

    for ep in ["list?path=/etc", "stat?path=/etc", "read?path=/etc/hosts", "download?path=/etc/hosts"] {
        let (s, _) = operator.get(&format!("{base}/{ep}")).await;
        assert_eq!(s, StatusCode::FORBIDDEN, "{ep}");
    }

    // Permission changes need file.admin: operators are refused before the node.
    let (s, _) = operator.post(&format!("{base}/chmod"), json!({"path": "/tmp", "mode": "0755"})).await;
    assert_eq!(s, StatusCode::FORBIDDEN);

    // Archive names must match a supported format.
    let (s, _) = admin.post(&format!("{base}/compress"), json!({"sources": ["/tmp/a"], "dest": "/tmp/a.rar"})).await;
    assert_eq!(s, StatusCode::BAD_REQUEST);

    // Approving a root needs file.admin with MFA.
    let (s, e) =
        admin.post(&format!("{base}/roots"), json!({"kind": "service", "label": "web", "path": "/srv/web"})).await;
    assert_eq!(s, StatusCode::FORBIDDEN, "{e}");
    assert_eq!(e["title"], "step_up_required", "{e}");

    // Traversal fails validation before reaching the node; unknown roots are 404.
    for bad in ["/etc/../root", "relative", "/a%00b"] {
        let (s, body) = admin.get(&format!("{base}/list?path={bad}")).await;
        assert_eq!(s, StatusCode::BAD_REQUEST, "{bad}: {body}");
    }

    let (s, _) = admin.get(&format!("{base}/list?root=fr_unknown&path=/")).await;
    assert_eq!(s, StatusCode::NOT_FOUND);
    let (s, _) = admin.get(&format!("{base}/read?path=/etc/hosts&length=99999999")).await;
    assert_eq!(s, StatusCode::BAD_REQUEST);
    let (s, _) = admin
        .post(&format!("{base}/search"), json!({"path": "/", "spec": {"content": "x", "max_file_size": 1_000_000_000}}))
        .await;
    assert_eq!(s, StatusCode::BAD_REQUEST);

    // Writes need MFA (guardrail) before anything reaches the node.
    let (s, e) = admin.post(&format!("{base}/mkdir"), json!({"path": "/tmp/x"})).await;
    assert_eq!(s, StatusCode::FORBIDDEN, "{e}");
    assert_eq!(e["title"], "step_up_required", "{e}");

    // Protected paths are refused and audited, for reads and writes alike.
    for p in ["/etc/shadow", "/etc/forge/pkgs/redis.env", "/proc/1/environ", "/home/u/.ssh/id_ed25519"] {
        let (s, body) = admin.get(&format!("{base}/read?path={}", q(p))).await;
        assert_eq!(s, StatusCode::FORBIDDEN, "{p}: {body}");
    }

    let denied =
        audit(&admin).await.into_iter().filter(|e| e["action"] == "file.read" && e["outcome"] == "denied").count();
    assert!(denied >= 4, "expected audited denials, got {denied}");
    let _ = a.stop.send(true);
}

/// Windows runs file operations inside the agent, so the whole flow runs
/// against the real filesystem.
#[cfg(windows)]
#[tokio::test]
async fn windows_agent_full_read_write_flow() {
    use base64::Engine as _;

    // Trash and revisions go to a temporary store, not C:\ProgramData.
    forge_agent::files::store_override(agent_store());
    let h = start().await;
    let mut admin = Client::login(&h.base, ADMIN_EMAIL, PASSWORD).await;
    let a = enroll_and_run(&h, &admin, "windows").await;
    let base = format!("/api/v1/servers/{}/files", a.server_id);
    step_up(&mut admin).await;

    let tmp = tempfile::tempdir().unwrap();
    let native = tmp.path().canonicalize().unwrap().display().to_string();
    let dir = format!("/{}", native.trim_start_matches(r"\\?\").replace('\\', "/"));
    let at = |p: &str| q(&format!("{dir}{p}"));

    let (s, drives) = admin.get(&format!("{base}/list?path=/")).await;
    assert_eq!(s, StatusCode::OK, "{drives}");
    assert!(drives["entries"].as_array().unwrap().iter().any(|e| e["name"] == "C:"), "{drives}");

    // mkdir and save a new file.
    let (s, r) = admin.post(&format!("{base}/mkdir"), json!({"path": format!("{dir}/conf")})).await;
    assert_eq!(s, StatusCode::OK, "{r}");
    let b64 = |s: &[u8]| base64::engine::general_purpose::STANDARD.encode(s);
    let (s, saved) = admin
        .post(&format!("{base}/save"), json!({"path": format!("{dir}/conf/app.env"), "content_b64": b64(b"A=1\n")}))
        .await;
    assert_eq!(s, StatusCode::OK, "{saved}");
    let v1 = saved["version"].as_str().unwrap().to_owned();

    // Saving over it needs the version it was opened at; a stale one is a conflict.
    let (s, r) = admin
        .post(
            &format!("{base}/save"),
            json!({"path": format!("{dir}/conf/app.env"), "content_b64": b64(b"A=2\n"), "version": v1}),
        )
        .await;
    assert_eq!(s, StatusCode::OK, "{r}");
    let (s, r) = admin
        .post(
            &format!("{base}/save"),
            json!({"path": format!("{dir}/conf/app.env"), "content_b64": b64(b"A=3\n"), "version": v1}),
        )
        .await;
    assert_eq!(s, StatusCode::CONFLICT, "{r}");
    assert_eq!(std::fs::read(tmp.path().join("conf/app.env")).unwrap(), b"A=2\n");

    // The replaced content is a revision.
    let (_, revs) = admin.get(&format!("{base}/revisions?path={}", at("/conf/app.env"))).await;
    let rev = revs["revisions"][0]["id"].as_str().unwrap().to_owned();
    let (_, old) = admin.get(&format!("{base}/revision?path={}&revision={rev}", at("/conf/app.env"))).await;
    assert_eq!(old["content_b64"], b64(b"A=1\n"));

    // Chunked, resumable upload with checksum verification.
    // Larger than one client chunk (1 MiB), each split into 256 KiB node chunks (#15).
    // Pseudo-random, so the archive steps below do not see a "zip bomb".
    let mut x = 0x2545_f491_u32;
    let data: Vec<u8> = (0..2_600_000u32)
        .map(|_| {
            x ^= x << 13;
            x ^= x >> 17;
            x ^= x << 5;
            x.to_le_bytes()[0]
        })
        .collect();
    let (s, up) =
        admin.post(&format!("{base}/uploads"), json!({"path": format!("{dir}/big.bin"), "size": data.len()})).await;
    assert_eq!(s, StatusCode::CREATED, "{up}");
    let uid = up["id"].as_str().unwrap().to_owned();
    let chunk = usize::try_from(up["chunk_size"].as_u64().unwrap()).unwrap();
    let put = |offset: usize, bytes: Vec<u8>| {
        let req = admin
            .http
            .put(format!("{}{base}/uploads/{uid}?offset={offset}", h.base))
            .header("x-csrf-token", admin.csrf.clone().unwrap_or_default())
            .body(bytes);
        async move { req.send().await.unwrap() }
    };
    let r = put(0, data[..chunk].to_vec()).await;
    assert_eq!(r.status(), StatusCode::OK);

    // Resume: the status says where to continue.
    let (_, st) = admin.get(&format!("{base}/uploads/{uid}")).await;
    assert_eq!(st["received"], chunk);
    let mut off = chunk;

    while off < data.len() {
        let end = (off + chunk).min(data.len());
        assert_eq!(put(off, data[off..end].to_vec()).await.status(), StatusCode::OK);
        off = end;
    }

    let sha: String =
        ring::digest::digest(&ring::digest::SHA256, &data).as_ref().iter().map(|b| format!("{b:02x}")).collect();
    let (s, r) = admin.post(&format!("{base}/uploads/{uid}/commit"), json!({"sha256": "0".repeat(64)})).await;
    assert_eq!(s, StatusCode::UNPROCESSABLE_ENTITY, "bad checksum must fail: {r}");
    let (s, r) = admin.post(&format!("{base}/uploads/{uid}/commit"), json!({"sha256": sha})).await;
    assert_eq!(s, StatusCode::OK, "{r}");
    assert_eq!(std::fs::read(tmp.path().join("big.bin")).unwrap(), data);

    // Chunks sent together, even in reverse order, reach the node in order
    // (the server keeps them in line, #15).
    assert_eq!(up["parallel"], 3);
    let (s, up2) =
        admin.post(&format!("{base}/uploads"), json!({"path": format!("{dir}/big2.bin"), "size": data.len()})).await;
    assert_eq!(s, StatusCode::CREATED, "{up2}");
    let uid2 = up2["id"].as_str().unwrap().to_owned();
    let put2 = |offset: usize, bytes: Vec<u8>| {
        let req = admin
            .http
            .put(format!("{}{base}/uploads/{uid2}?offset={offset}", h.base))
            .header("x-csrf-token", admin.csrf.clone().unwrap_or_default())
            .body(bytes);
        async move { req.send().await.unwrap().status() }
    };
    let starts: Vec<usize> = (0..data.len()).step_by(chunk).collect();
    let sends: Vec<_> = starts.iter().rev().map(|&o| put2(o, data[o..(o + chunk).min(data.len())].to_vec())).collect();

    for status in futures::future::join_all(sends).await {
        assert_eq!(status, StatusCode::OK);
    }

    let (s, r) = admin.post(&format!("{base}/uploads/{uid2}/commit"), json!({"sha256": sha.clone()})).await;
    assert_eq!(s, StatusCode::OK, "{r}");
    assert_eq!(std::fs::read(tmp.path().join("big2.bin")).unwrap(), data);

    // A read of 1 MiB (up to 2 MiB allowed), served from several node reads (#15).
    let (s, r) = admin.get(&format!("{base}/read?path={dir}/big.bin&offset=100&length=1048576")).await;
    assert_eq!(s, StatusCode::OK, "{r}");
    let got =
        base64::Engine::decode(&base64::engine::general_purpose::STANDARD, r["content_b64"].as_str().unwrap()).unwrap();
    assert_eq!(got.as_slice(), &data[100..100 + 1_048_576]);
    assert_eq!(r["length"], 1_048_576);
    assert_eq!(r["eof"], false);
    let (s, _) = admin.get(&format!("{base}/read?path={dir}/big.bin&length=2097153")).await;
    assert_eq!(s, StatusCode::BAD_REQUEST, "at most 2 MiB per read");

    // Copy, move, delete to trash, restore.
    let (s, r) =
        admin.post(&format!("{base}/copy"), json!({"from": format!("{dir}/conf"), "to": format!("{dir}/conf2")})).await;
    assert_eq!(s, StatusCode::OK, "{r}");
    let (s, r) = admin
        .post(&format!("{base}/move"), json!({"from": format!("{dir}/conf2"), "to": format!("{dir}/conf3")}))
        .await;
    assert_eq!(s, StatusCode::OK, "{r}");
    assert!(tmp.path().join("conf3/app.env").exists() && !tmp.path().join("conf2").exists());
    let (s, d) = admin.post(&format!("{base}/delete"), json!({"path": format!("{dir}/conf3")})).await;
    assert_eq!(s, StatusCode::OK, "{d}");
    assert!(!tmp.path().join("conf3").exists());
    let (_, t) = admin.get(&format!("{base}/trash")).await;
    assert!(t["entries"].as_array().unwrap().iter().any(|e| e["id"] == d["trash_id"]), "{t}");
    let (s, r) = admin.post(&format!("{base}/trash/restore"), json!({"id": d["trash_id"]})).await;
    assert_eq!(s, StatusCode::OK, "{r}");
    assert!(tmp.path().join("conf3/app.env").exists());

    // Search names and content.
    let (s, r) =
        admin.post(&format!("{base}/search"), json!({"path": dir, "spec": {"name": "*.env", "content": "A=2"}})).await;
    assert_eq!(s, StatusCode::OK, "{r}");
    assert_eq!(r["status"], "complete");
    assert_eq!(r["files"].as_array().unwrap().len(), 2, "{r}");

    // Alternate data streams never reach the filesystem.
    let (s, _) = admin.get(&format!("{base}/read?path={}", at("/conf/app.env:hidden"))).await;
    assert_eq!(s, StatusCode::UNPROCESSABLE_ENTITY);

    // A multi-chunk download arrives intact, sandboxed as an attachment.
    let resp = admin.http.get(format!("{}{base}/download?path={}", h.base, at("/big.bin"))).send().await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let hd = resp.headers().clone();
    assert_eq!(hd["content-type"], "application/octet-stream");
    assert_eq!(hd["x-content-type-options"], "nosniff");
    assert!(hd["content-security-policy"].to_str().unwrap().starts_with("sandbox"));
    assert!(hd["content-disposition"].to_str().unwrap().contains("big.bin"));
    assert_eq!(resp.bytes().await.unwrap().as_ref(), data.as_slice());

    // A browser offers compression: the download is sent as-is, complete, and
    // the server survives (a compressed stream used to be polled past its end
    // and panicked the process).
    for _ in 0..2 {
        let resp = admin
            .http
            .get(format!("{}{base}/download?path={}", h.base, at("/big.bin")))
            .header("accept-encoding", "br, zstd, gzip")
            .send()
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        assert!(resp.headers().get("content-encoding").is_none(), "downloads are not compressed");
        assert_eq!(resp.bytes().await.unwrap().as_ref(), data.as_slice());
    }

    let (s, _) = admin.get(&format!("{base}/stat?path={}", at("/big.bin"))).await;
    assert_eq!(s, StatusCode::OK, "the server still answers after the downloads");

    // An approved read-only root: operators read it, nobody changes it.
    let (s, root) = admin
        .post(
            &format!("{base}/roots"),
            json!({"kind": "service", "label": "conf", "path": format!("{dir}/conf"), "read_only": true}),
        )
        .await;
    assert_eq!(s, StatusCode::CREATED, "{root}");
    let rid = root["id"].as_str().unwrap().to_owned();
    let mut operator =
        user_with_role(&h, &admin, "op2@example.com", "operator", &format!("workspace:{}", h.default_org)).await;
    let (_, r) = operator.get(&format!("{base}/roots")).await;
    assert_eq!(r["roots"].as_array().unwrap().len(), 1, "{r}");
    let (s, l) = operator.get(&format!("{base}/list?root={rid}&path=/")).await;
    assert_eq!(s, StatusCode::OK, "{l}");
    assert_eq!(l["entries"][0]["name"], "app.env");

    // The root confines: the parent is out of reach.
    let (s, _) = operator.get(&format!("{base}/read?root={rid}&path=/../big.bin")).await;
    assert_eq!(s, StatusCode::BAD_REQUEST);
    step_up(&mut operator).await;
    let (s, e) = operator.post(&format!("{base}/delete"), json!({"root": rid, "path": "/app.env"})).await;
    assert_eq!(s, StatusCode::FORBIDDEN, "{e}");
    assert!(tmp.path().join("conf/app.env").exists());

    // Step 6: compress, extract, bulk copy and trash, download as archive.
    let (s, r) = admin
        .post(
            &format!("{base}/compress"),
            json!({"sources": [format!("{dir}/conf"), format!("{dir}/big.bin")], "dest": format!("{dir}/bundle.zip")}),
        )
        .await;
    assert_eq!(s, StatusCode::OK, "{r}");
    let (s, r) = admin
        .post(
            &format!("{base}/extract"),
            json!({"archive": format!("{dir}/bundle.zip"), "dest": format!("{dir}/unpacked")}),
        )
        .await;
    assert_eq!(s, StatusCode::OK, "{r}");
    assert_eq!(std::fs::read(tmp.path().join("unpacked/big.bin")).unwrap(), data);
    std::fs::create_dir(tmp.path().join("dst")).unwrap();
    let (s, r) = admin
        .post(&format!("{base}/bulk"), json!({"action": "copy", "paths": [format!("{dir}/conf"), format!("{dir}/nope")], "to": format!("{dir}/dst")}))
        .await;
    assert_eq!(s, StatusCode::OK, "{r}");
    assert_eq!((r["ok"].as_u64(), r["failed"].as_u64()), (Some(1), Some(1)), "{r}");
    let (s, r) =
        admin.post(&format!("{base}/bulk"), json!({"action": "trash", "paths": [format!("{dir}/dst/conf")]})).await;
    assert_eq!(s, StatusCode::OK, "{r}");
    assert!(!tmp.path().join("dst/conf").exists());
    let (s, arch) = admin.post(&format!("{base}/archive"), json!({"paths": [format!("{dir}/conf")]})).await;
    assert_eq!(s, StatusCode::OK, "{arch}");
    let resp =
        admin.http.get(format!("{}{base}/archive/{}", h.base, arch["temp_id"].as_str().unwrap())).send().await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    assert!(resp.headers()["content-disposition"].to_str().unwrap().contains(".zip"));
    assert!(resp.bytes().await.unwrap().starts_with(b"PK"));

    let log = audit(&admin).await;

    for action in [
        "file.compress",
        "file.extract",
        "file.mkdir",
        "file.upload",
        "file.copy",
        "file.move",
        "file.delete",
        "file.trash.restore",
        "file.download",
        "file.root.create",
    ] {
        assert!(log.iter().any(|e| e["action"] == action && e["outcome"] == "success"), "{action} not audited");
    }

    let _ = a.stop.send(true);
}
