//! Forge-managed SFTP gateway end to end: issuing access (MFA), login with
//! the grant token, SFTP-only sessions, the protected-path policy, root
//! confinement and revocation of open sessions; on Windows (agent in process)
//! uploads and downloads against the real filesystem.
#![allow(clippy::unwrap_used)]

use std::sync::Arc;
use std::time::Duration;

use crate::common::*;
use forge_agent::enroll::{HostFacts, enroll};
use forge_agent::{Agent, AgentConfig, StatePaths};
use forge_server::App;
use forge_server::state::Services;
use reqwest::StatusCode;
use russh::client;
use russh::keys::PublicKeyOrCertificate;
use russh_sftp::client::SftpSession;
use serde_json::{Value, json};
use tokio::net::TcpListener;

struct Gw {
    h: Harness,
    sftp: std::net::SocketAddr,
}

async fn start_with_sftp() -> Gw {
    start_with_sftp_cfg(|_| {}).await
}

async fn start_with_sftp_cfg(change: impl FnOnce(&mut forge_server::ServerConfig)) -> Gw {
    let dir = tempfile::tempdir().unwrap();
    let mut config = test_config(dir.path());
    change(&mut config);
    let api = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let agent = TcpListener::bind("127.0.0.1:0").await.unwrap();
    config.public_url = Some(format!("http://{}", api.local_addr().unwrap()));
    config.sftp.enabled = true;
    config.sftp.listen = "127.0.0.1:0".parse().unwrap();
    let state = Services::build(config).await.unwrap();
    let init =
        forge_server::bootstrap::init_admin(&state, None, Some(ADMIN_EMAIL), "Admin", PASSWORD, false).await.unwrap();
    let app = App::start_with(state, api, agent).unwrap();
    let sftp = app.sftp_addr.unwrap();
    let h = Harness {
        base: format!("http://{}", app.api_addr),
        agent_url: format!("https://127.0.0.1:{}", app.agent_addr.port()),
        default_org: init.workspace_id.to_string(),
        app,
        dir,
    };

    Gw { h, sftp }
}

async fn enroll_agent(
    h: &Harness,
    admin: &Client,
    os: &str,
) -> (String, tokio::sync::watch::Sender<bool>, tempfile::TempDir) {
    let (_, body) = admin.post("/api/v1/enrollment-tokens", json!({"ttl_minutes": 10})).await;
    let dir = tempfile::tempdir().unwrap();
    let paths = StatePaths::new(dir.path());
    let facts = HostFacts { hostname: "sftp-01".into(), os: os.into(), arch: "x86_64".into() };
    let id =
        enroll(&paths, AgentConfig::new(h.agent_url.clone()), body["token"].as_str().unwrap(), facts).await.unwrap();
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

    (id.server_id, stop, dir)
}

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

/// Pins nothing: tests compare the fingerprint the API reports instead.
struct AnyKey(Arc<std::sync::Mutex<Option<String>>>);

impl client::Handler for AnyKey {
    type Error = russh::Error;
    async fn check_server_key(&mut self, key: &PublicKeyOrCertificate) -> Result<bool, Self::Error> {
        if let PublicKeyOrCertificate::PublicKey { key, .. } = key {
            *self.0.lock().unwrap() = Some(key.fingerprint(russh::keys::HashAlg::Sha256).to_string());
        }
        Ok(true)
    }
}

async fn connect(addr: std::net::SocketAddr, user: &str, pass: &str) -> (client::Handle<AnyKey>, bool, Option<String>) {
    let seen = Arc::new(std::sync::Mutex::new(None));
    let config = Arc::new(client::Config::default());
    let mut handle = client::connect(config, addr, AnyKey(seen.clone())).await.unwrap();
    let ok = handle.authenticate_password(user, pass).await.unwrap().success();
    let fp = seen.lock().unwrap().clone();

    (handle, ok, fp)
}

async fn sftp(handle: &client::Handle<AnyKey>) -> SftpSession {
    let ch = handle.channel_open_session().await.unwrap();
    ch.request_subsystem(true, "sftp").await.unwrap();
    let s = SftpSession::new(ch.into_stream()).await.unwrap();

    // russh-sftp gives up on a request after 10 s by default. Closing an
    // upload commits it through the agent (up to the 30 s file-op timeout),
    // which takes longer than 10 s on a busy runner; OpenSSH clients wait.
    s.set_timeout(60);

    s
}

async fn audit(admin: &Client) -> Vec<Value> {
    let (_, a) = admin.get("/api/v1/audit?limit=500").await;
    a.as_array().cloned().or_else(|| a["items"].as_array().cloned()).unwrap_or_default()
}

#[tokio::test]
async fn grants_need_mfa_log_in_only_with_their_token_and_stop_when_revoked() {
    let Gw { h, sftp: addr } = start_with_sftp().await;
    let mut admin = Client::login(&h.base, ADMIN_EMAIL, PASSWORD).await;
    let (server, stop, _d) = enroll_agent(&h, &admin, "linux").await;
    let base = format!("/api/v1/servers/{server}/files");

    let (s, info) = admin.get(&format!("{base}/sftp")).await;
    assert_eq!(s, StatusCode::OK, "{info}");
    assert_eq!(info["enabled"], true);
    assert_eq!(info["port"], addr.port());

    // Issuing access needs MFA.
    let req = json!({"name": "vendor upload", "roots": ["node"], "expires_in_minutes": 60});
    let (s, e) = admin.post(&format!("{base}/grants"), req.clone()).await;
    assert_eq!(s, StatusCode::FORBIDDEN, "{e}");
    assert_eq!(e["title"], "step_up_required");
    step_up(&mut admin).await;

    // Longer than the maximum, unknown roots and bad address lists are refused.
    for bad in [
        json!({"name": "x", "roots": ["node"], "expires_in_minutes": 7 * 24 * 60 + 1}),
        json!({"name": "x", "roots": ["fr_nope"], "expires_in_minutes": 60}),
        json!({"name": "x", "roots": ["node"], "expires_in_minutes": 60, "allowed_ips": ["not-an-ip"]}),
    ] {
        let (s, _) = admin.post(&format!("{base}/grants"), bad.clone()).await;
        assert!(s == StatusCode::BAD_REQUEST || s == StatusCode::NOT_FOUND, "{bad}: {s}");
    }

    let (s, issued) = admin.post(&format!("{base}/grants"), req).await;
    assert_eq!(s, StatusCode::CREATED, "{issued}");
    let conn = &issued["connection"];
    let (user, pass) = (conn["username"].as_str().unwrap().to_owned(), conn["password"].as_str().unwrap().to_owned());
    assert!(issued["grant"].get("token_hash").is_none(), "the hash must not leave the server");

    // The token is never listed again.
    let (_, list) = admin.get(&format!("{base}/grants")).await;
    assert!(!list.to_string().contains(&pass));

    // Wrong token: refused. Right token: accepted, and the key matches the API.
    let (_, ok, _) = connect(addr, &user, "wrong").await;
    assert!(!ok);
    let (handle, ok, fp) = connect(addr, &user, &pass).await;
    assert!(ok);
    assert_eq!(fp.as_deref(), conn["fingerprint"].as_str());

    // No shell or command execution: only the SFTP subsystem.
    let ch = handle.channel_open_session().await.unwrap();
    ch.exec(true, "id").await.unwrap();
    let mut ch = ch;
    let mut refused = false;

    while let Some(msg) = tokio::time::timeout(Duration::from_secs(5), ch.wait()).await.ok().flatten() {
        if matches!(msg, russh::ChannelMsg::Failure | russh::ChannelMsg::Close | russh::ChannelMsg::Eof) {
            refused = true;
            break;
        }

        assert!(!matches!(msg, russh::ChannelMsg::Data { .. }), "exec produced output");
    }

    assert!(refused, "exec was not refused");

    let s = sftp(&handle).await;
    let top: Vec<String> = s.read_dir("/").await.unwrap().map(|e| e.file_name()).collect();
    assert_eq!(top, ["node"]);

    // Protected paths are refused before they reach the node; `..` cannot climb.
    assert!(s.metadata("/node/etc/shadow").await.is_err());
    assert!(s.read_dir("/node/etc/forge").await.is_err());
    assert!(s.metadata("/../../etc/shadow").await.is_err());

    // A read-only grant cannot write.
    assert!(s.create("/node/tmp/x").await.is_err());

    // Revoking closes the open session at the gateway's next sweep.
    let (s2, r) = admin.post(&format!("{base}/grants/{user}/revoke"), json!({})).await;
    assert_eq!(s2, StatusCode::OK, "{r}");
    let deadline = tokio::time::Instant::now() + Duration::from_secs(60);

    while !handle.is_closed() {
        assert!(tokio::time::Instant::now() < deadline, "the session stayed open after revocation");
        tokio::time::sleep(Duration::from_millis(200)).await;
    }

    let (_, ok, _) = connect(addr, &user, &pass).await;
    assert!(!ok, "a revoked grant must not log in");

    let log = audit(&admin).await;

    for a in ["file.grant.create", "file.sftp.connect", "file.grant.revoke"] {
        assert!(log.iter().any(|e| e["action"] == a), "{a} not audited");
    }

    let _ = stop.send(true);
}

/// Windows runs file operations in the agent, so files really move.
#[cfg(windows)]
#[tokio::test]
async fn windows_sftp_uploads_and_downloads_inside_an_approved_root() {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    forge_agent::files::store_override(agent_store());
    let Gw { h, sftp: addr } = start_with_sftp().await;
    let mut admin = Client::login(&h.base, ADMIN_EMAIL, PASSWORD).await;
    let (server, stop, _d) = enroll_agent(&h, &admin, "windows").await;
    let base = format!("/api/v1/servers/{server}/files");
    step_up(&mut admin).await;

    let tmp = tempfile::tempdir().unwrap();
    std::fs::create_dir(tmp.path().join("inbox")).unwrap();
    std::fs::write(tmp.path().join("readme.txt"), b"hello from the node").unwrap();
    let native = tmp.path().canonicalize().unwrap().display().to_string();
    let dir = format!("/{}", native.trim_start_matches(r"\\?\").replace('\\', "/"));
    let (s, root) = admin
        .post(
            &format!("{base}/roots"),
            json!({"kind": "service", "label": "Vendor Drop", "path": dir, "read_only": false}),
        )
        .await;
    assert_eq!(s, StatusCode::CREATED, "{root}");
    let rid = root["id"].as_str().unwrap();
    let (s, issued) = admin
        .post(
            &format!("{base}/grants"),
            json!({"name": "vendor", "roots": [rid], "write": true, "expires_in_minutes": 30}),
        )
        .await;
    assert_eq!(s, StatusCode::CREATED, "{issued}");
    let (user, pass) =
        (issued["connection"]["username"].as_str().unwrap(), issued["connection"]["password"].as_str().unwrap());

    let (handle, ok, _) = connect(addr, user, pass).await;
    assert!(ok);
    let s = sftp(&handle).await;
    let top: Vec<String> = s.read_dir("/").await.unwrap().map(|e| e.file_name()).collect();
    assert_eq!(top, ["Vendor-Drop"]);

    // Download.
    let mut f = s.open("/Vendor-Drop/readme.txt").await.unwrap();
    let mut got = Vec::new();
    f.read_to_end(&mut got).await.unwrap();
    assert_eq!(got, b"hello from the node");
    drop(f);

    // Upload (several chunks), committed only on close.
    let data: Vec<u8> = (0..700_000u32).map(|i| (i % 253) as u8).collect();
    let mut w = s.create("/Vendor-Drop/inbox/report.bin").await.unwrap();
    w.write_all(&data).await.unwrap();
    w.shutdown().await.unwrap();
    drop(w);
    let deadline = tokio::time::Instant::now() + Duration::from_secs(60);

    while std::fs::read(tmp.path().join("inbox/report.bin")).ok().as_deref() != Some(data.as_slice()) {
        assert!(tokio::time::Instant::now() < deadline, "upload never landed");
        tokio::time::sleep(Duration::from_millis(200)).await;
    }

    // No delete permission: replacing or removing is refused.
    assert!(s.remove_file("/Vendor-Drop/readme.txt").await.is_err());
    assert!(tmp.path().join("readme.txt").exists());

    // The root confines: nothing outside it is reachable.
    assert!(s.metadata("/Vendor-Drop/../../Windows").await.is_err());

    let log = audit(&admin).await;

    for a in ["file.sftp.download", "file.sftp.upload"] {
        assert!(log.iter().any(|e| e["action"] == a), "{a} not audited");
    }

    let _ = stop.send(true);
}

async fn patch(c: &Client, path: &str, body: Value) -> (StatusCode, Value) {
    c.call(reqwest::Method::PATCH, path, Some(body)).await
}

/// Waits until the server side closed `handle`.
async fn wait_closed(handle: &client::Handle<AnyKey>, what: &str) {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(30);

    while !handle.is_closed() {
        assert!(tokio::time::Instant::now() < deadline, "the session stayed open after {what}");
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
}

#[tokio::test]
async fn grants_are_listed_fleet_wide_changed_without_new_passwords_and_narrowing_drops_sessions() {
    let Gw { h, sftp: addr } = start_with_sftp().await;
    let mut admin = Client::login(&h.base, ADMIN_EMAIL, PASSWORD).await;
    let (server, stop, _d) = enroll_agent(&h, &admin, "linux").await;
    let base = format!("/api/v1/servers/{server}/files");
    step_up(&mut admin).await;

    let (s, issued) = admin
        .post(
            &format!("{base}/grants"),
            json!({"name": "vendor upload", "roots": ["node"], "write": true, "expires_in_minutes": 60}),
        )
        .await;
    assert_eq!(s, StatusCode::CREATED, "{issued}");
    let user = issued["connection"]["username"].as_str().unwrap().to_owned();
    let pass = issued["connection"]["password"].as_str().unwrap().to_owned();
    assert_eq!(issued["grant"]["status"], "active");
    assert_eq!(issued["grant"]["never_expires"], false);

    // Fleet-wide list, with filters.
    let (s, all) = admin.get("/api/v1/files/grants").await;
    assert_eq!(s, StatusCode::OK, "{all}");
    let row = all["grants"].as_array().unwrap().iter().find(|g| g["id"] == user.as_str()).cloned().unwrap();
    assert_eq!(row["server_id"], server.as_str());
    assert_eq!(row["server_name"], "sftp-01");
    assert_eq!(row["status"], "active");
    assert!(!all.to_string().contains(&pass), "the password must never be listed");

    for (q, n) in [
        (format!("?server={server}"), 1),
        ("?status=active".to_owned(), 1),
        ("?status=revoked".to_owned(), 0),
        ("?q=VENDOR".to_owned(), 1),
        ("?q=sftp-01".to_owned(), 1),
        ("?q=nothing-matches".to_owned(), 0),
    ] {
        let (s, l) = admin.get(&format!("/api/v1/files/grants{q}")).await;
        assert_eq!(s, StatusCode::OK, "{q}: {l}");
        assert_eq!(l["grants"].as_array().unwrap().len(), n, "{q}");
    }

    let (s, _) = admin.get("/api/v1/files/grants?status=bogus").await;
    assert_eq!(s, StatusCode::BAD_REQUEST);

    // Others see only what their authorization allows: an operator (file.read)
    // sees no one else's grants, a developer (no file.read) none at all.
    let op = user_with_role(&h, &admin, "op@example.com", "operator", &format!("workspace:{}", h.default_org)).await;
    let dev = user_with_role(&h, &admin, "dev@example.com", "developer", &format!("workspace:{}", h.default_org)).await;

    for c in [&op, &dev] {
        let (s, l) = c.get("/api/v1/files/grants").await;
        assert_eq!(s, StatusCode::OK, "{l}");
        assert_eq!(l["grants"].as_array().unwrap().len(), 0, "{l}");
    }

    // ...and cannot read, change or revoke it.
    let (s, _) = op.get(&format!("{base}/grants/{user}")).await;
    assert_eq!(s, StatusCode::NOT_FOUND);
    let (s, _) = patch(&op, &format!("{base}/grants/{user}"), json!({"name": "mine now"})).await;
    assert!(s == StatusCode::FORBIDDEN || s == StatusCode::NOT_FOUND, "{s}");
    let (s, _) = op.post(&format!("{base}/grants/{user}/revoke"), json!({})).await;
    assert!(s == StatusCode::FORBIDDEN || s == StatusCode::NOT_FOUND, "{s}");

    // Never-expiring access is refused while `allow_never_expiry` is off,
    // on create and on change.
    let (s, e) = admin
        .post(&format!("{base}/grants"), json!({"name": "forever", "roots": ["node"], "never_expires": true}))
        .await;
    assert_eq!(s, StatusCode::FORBIDDEN, "{e}");
    let (s, e) = patch(&admin, &format!("{base}/grants/{user}"), json!({"never_expires": true})).await;
    assert_eq!(s, StatusCode::FORBIDDEN, "{e}");
    let (s, _) = patch(&admin, &format!("{base}/grants/{user}"), json!({"expires_in_minutes": 7 * 24 * 60 + 1})).await;
    assert_eq!(s, StatusCode::BAD_REQUEST);
    let (s, _) = patch(&admin, &format!("{base}/grants/{user}"), json!({"allowed_ips": ["nope"]})).await;
    assert_eq!(s, StatusCode::BAD_REQUEST);
    let (s, _) = patch(&admin, &format!("{base}/grants/{user}"), json!({"token_hash": "x"})).await;
    assert!(s.is_client_error(), "unknown fields are refused");

    // Changing expiry and name keeps the password and the open session.
    let (handle, ok, _) = connect(addr, &user, &pass).await;
    assert!(ok);
    let sess = sftp(&handle).await;
    assert!(sess.read_dir("/").await.is_ok());
    let before = row["expires_at"].as_i64().unwrap();
    let (s, g) =
        patch(&admin, &format!("{base}/grants/{user}"), json!({"name": "vendor (renamed)", "expires_in_minutes": 120}))
            .await;
    assert_eq!(s, StatusCode::OK, "{g}");
    assert_eq!(g["name"], "vendor (renamed)");
    assert!(g["expires_at"].as_i64().unwrap() > before);
    assert!(g.get("token_hash").is_none());
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert!(!handle.is_closed(), "widening must not drop sessions");

    // Narrowing permissions closes the open session at once; the same
    // password still logs in, now read-only.
    let (s, g) = patch(&admin, &format!("{base}/grants/{user}"), json!({"write": false})).await;
    assert_eq!(s, StatusCode::OK, "{g}");
    assert_eq!(g["write"], false);
    wait_closed(&handle, "narrowing").await;
    let (handle, ok, _) = connect(addr, &user, &pass).await;
    assert!(ok, "changing a grant must not change its password");
    let sess = sftp(&handle).await;
    assert!(sess.create("/node/tmp/x").await.is_err(), "write was removed");

    // Reset password: the new one works, the old one does not, sessions close.
    let (s, r) = admin.post(&format!("{base}/grants/{user}/reset-password"), json!({})).await;
    assert_eq!(s, StatusCode::OK, "{r}");
    let new_pass = r["connection"]["password"].as_str().unwrap().to_owned();
    assert_ne!(new_pass, pass);
    wait_closed(&handle, "a password reset").await;
    let (_, ok, _) = connect(addr, &user, &pass).await;
    assert!(!ok, "the old password must stop working");
    let (handle, ok, _) = connect(addr, &user, &new_pass).await;
    assert!(ok);

    // Revoking closes open sessions at once and the grant shows as revoked.
    let (s, r) = admin.post(&format!("{base}/grants/{user}/revoke"), json!({})).await;
    assert_eq!(s, StatusCode::OK, "{r}");
    assert_eq!(r["status"], "revoked");
    wait_closed(&handle, "revocation").await;
    let (s, _) = patch(&admin, &format!("{base}/grants/{user}"), json!({"name": "again"})).await;
    assert_eq!(s, StatusCode::CONFLICT, "revoked access cannot be changed");
    let (_, l) = admin.get("/api/v1/files/grants?status=revoked").await;
    assert_eq!(l["grants"].as_array().unwrap().len(), 1);

    // Audit: changes with a before/after diff and no secrets; denials too.
    let log = audit(&admin).await;
    let updates: Vec<&Value> = log.iter().filter(|e| e["action"] == "file.grant.update").collect();
    assert!(
        updates.iter().any(|e| e["details"]["changes"]["write"]["before"] == true
            && e["details"]["changes"]["write"]["after"] == false
            && e["details"]["sessions_closed"] == true),
        "no before/after diff for write: {updates:?}"
    );
    assert!(log.iter().any(|e| e["action"] == "file.grant.reset_password"));
    assert!(
        log.iter().any(|e| e["action"] == "file.grant.create" && e["outcome"] == "denied"),
        "the refused never-expiring grant must be audited"
    );
    let text = serde_json::to_string(&log).unwrap();
    assert!(!text.contains(&pass) && !text.contains(&new_pass), "no secrets in the audit log");
    let _ = stop.send(true);
}

#[tokio::test]
async fn never_expiring_access_needs_the_setting_and_file_admin() {
    let Gw { h, sftp: _ } = start_with_sftp_cfg(|c| c.sftp.allow_never_expiry = true).await;
    let mut admin = Client::login(&h.base, ADMIN_EMAIL, PASSWORD).await;
    let (server, stop, _d) = enroll_agent(&h, &admin, "linux").await;
    let base = format!("/api/v1/servers/{server}/files");
    step_up(&mut admin).await;
    let (_, info) = admin.get(&format!("{base}/sftp")).await;
    assert_eq!(info["allow_never_expiry"], true);
    assert_eq!(info["can_never_expire"], true);

    let (s, issued) = admin
        .post(&format!("{base}/grants"), json!({"name": "backup robot", "roots": ["node"], "never_expires": true}))
        .await;
    assert_eq!(s, StatusCode::CREATED, "{issued}");
    assert_eq!(issued["grant"]["never_expires"], true);
    assert_eq!(issued["grant"]["expires_at"], forge_core::files::NEVER_EXPIRES);
    let id = issued["grant"]["id"].as_str().unwrap().to_owned();

    // Back to a normal expiry, and to never again.
    let (s, g) = patch(&admin, &format!("{base}/grants/{id}"), json!({"never_expires": false})).await;
    assert_eq!(s, StatusCode::BAD_REQUEST, "an expiry is needed: {g}");
    let (s, g) =
        patch(&admin, &format!("{base}/grants/{id}"), json!({"never_expires": false, "expires_in_minutes": 30})).await;
    assert_eq!(s, StatusCode::OK, "{g}");
    assert_eq!(g["never_expires"], false);
    let (s, g) = patch(&admin, &format!("{base}/grants/{id}"), json!({"never_expires": true})).await;
    assert_eq!(s, StatusCode::OK, "{g}");
    assert_eq!(g["never_expires"], true);
    let (_, l) = admin.get("/api/v1/files/grants").await;
    assert_eq!(l["grants"][0]["never_expires"], true);

    // Still revocable.
    let (s, r) = admin.post(&format!("{base}/grants/{id}/revoke"), json!({})).await;
    assert_eq!(s, StatusCode::OK, "{r}");
    assert_eq!(r["status"], "revoked");

    // An operator (no file.admin) may not choose it, even with MFA.
    let mut op =
        user_with_role(&h, &admin, "op@example.com", "operator", &format!("workspace:{}", h.default_org)).await;
    step_up(&mut op).await;
    let (_, info) = op.get(&format!("{base}/sftp")).await;
    assert_eq!(info["can_never_expire"], false);
    let (s, e) =
        op.post(&format!("{base}/grants"), json!({"name": "forever", "roots": ["node"], "never_expires": true})).await;
    assert_eq!(s, StatusCode::FORBIDDEN, "{e}");
    let log = audit(&admin).await;
    assert!(log.iter().any(|e| e["action"] == "file.grant.create" && e["details"]["never_expires"] == true));
    let _ = stop.send(true);
}
