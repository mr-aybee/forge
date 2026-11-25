//! Object storage: managed installs (authorization, validation, vault
//! credential, no secret in jobs) and connected S3 services (bucket and
//! object operations against an in-process S3 stand-in).
#![allow(clippy::unwrap_used)]

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};

use axum::body::Bytes;
use axum::extract::{Path, Query, State};
use axum::http::{HeaderMap, StatusCode as AxStatus};
use axum::routing::{get, post};
use reqwest::{Method, StatusCode};
use serde_json::json;

use crate::common::{ADMIN_EMAIL, Client, PASSWORD, start, user_with_role};

async fn step_up(admin: &mut Client) {
    use forge_identity::totp;
    let (_, t) = admin.post("/api/v1/auth/totp", json!({})).await;
    let secret = totp::base32_decode(t["secret"].as_str().unwrap()).unwrap();
    let now =
        i64::try_from(std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_secs()).unwrap();
    let code = format!("{:06}", totp::code_at(&secret, now));
    let (s, _) = admin
        .post("/api/v1/auth/totp/confirm", json!({"password": PASSWORD, "secret": t["secret"], "code": code}))
        .await;
    assert_eq!(s, StatusCode::NO_CONTENT);

    // A code is accepted once: use the next step for the step-up.
    let code = format!("{:06}", totp::code_at(&secret, now + 30));
    let (s, r) = admin.post("/api/v1/auth/step-up", json!({"code": code})).await;
    assert_eq!(s, StatusCode::OK, "{r}");
    admin.csrf = r["csrf_token"].as_str().map(str::to_owned);
}

async fn node(h: &crate::common::Harness, name: &str, os: &str) -> forge_core::ServerId {
    node_in(h, name, os, None).await
}

async fn node_in(
    h: &crate::common::Harness,
    name: &str,
    os: &str,
    cluster: Option<forge_core::ClusterId>,
) -> forge_core::ServerId {
    let id = forge_core::ServerId::new();
    let server = forge_core::model::Server {
        id,
        workspace_id: h.default_org.parse().unwrap(),
        environment_id: None,
        cluster_id: cluster,
        role: forge_core::model::NodeRole::Worker,
        display_name: None,
        hostname: name.into(),
        os: os.into(),
        arch: "x86_64".into(),
        agent_version: "0".into(),
        status: forge_core::model::AgentStatus::Online,
        last_seen_at: None,
        cert_serial: None,
        cert_not_after: None,
        created_at: forge_core::Timestamp(0),
    };
    forge_core::ports::ServerRepository::create_server(&h.app.state.store, &server).await.unwrap();

    id
}

#[tokio::test]
async fn managed_install_is_guarded_validated_and_keeps_the_key_in_the_vault() {
    let h = start().await;
    let mut admin = Client::login(&h.base, ADMIN_EMAIL, PASSWORD).await;
    let srv = node(&h, "store-1", "linux").await;
    let body = |slug: &str, path: &str, port: u16| json!({"name": "Media", "slug": slug, "server_id": srv, "data_path": path, "s3_port": port, "console_port": port + 1});

    let (s, _) = admin.post("/api/v1/object-stores", body("media", "/srv/objstore/media", 9000)).await;
    assert_eq!(s, StatusCode::FORBIDDEN, "storage.manage needs an MFA session");
    let (_, list) = admin.get("/api/v1/object-stores").await;
    assert_eq!(list.as_array().unwrap().len(), 0, "nothing recorded for a refused request");

    step_up(&mut admin).await;

    for bad in ["/", "/etc/rustfs", "/srv/../etc", "relative"] {
        let (s, r) = admin.post("/api/v1/object-stores", body("media", bad, 9000)).await;
        assert_eq!(s, StatusCode::BAD_REQUEST, "{bad}: {r}");
    }

    let (s, r) = admin.post("/api/v1/object-stores", body("Bad Name", "/srv/objstore/x", 9000)).await;
    assert_eq!(s, StatusCode::BAD_REQUEST, "{r}");

    let (s, r) = admin.post("/api/v1/object-stores", body("media", "/srv/objstore/media", 9000)).await;
    assert_eq!(s, StatusCode::ACCEPTED, "{r}");
    let store = &r["store"];
    assert_eq!(store["mode"], "managed");
    assert_eq!(store["status"], "installing");
    assert_eq!(store["endpoint"], "http://store-1:9000");
    assert_eq!(store["members"][0]["data_paths"], json!(["/srv/objstore/media"]));
    assert_eq!(store["topology"], "standalone");

    // The job carries ids only; the credential is a vault item.
    let (_, job) = admin.get(&format!("/api/v1/jobs/{}", r["job"]["id"].as_str().unwrap())).await;
    let mut keys: Vec<&str> = job["payload"].as_object().unwrap().keys().map(String::as_str).collect();
    keys.sort_unstable();
    assert_eq!(keys, vec!["action", "server_id", "store_id"], "{job}");
    let cred: forge_core::VaultItemId = store["credential_id"].as_str().unwrap().parse().unwrap();
    let item = forge_core::ports::VaultRepository::get_vault_item(&h.app.state.store, cred).await.unwrap().unwrap();
    assert!(item.username.starts_with("forge"));
    assert!(!item.sealed.is_empty());
    assert!(!r.to_string().contains(&item.sealed));

    // Instances on one node need distinct names, ports and data paths.
    let (s, _) = admin.post("/api/v1/object-stores", body("media", "/srv/objstore/other", 9100)).await;
    assert_eq!(s, StatusCode::CONFLICT);
    let (s, _) = admin.post("/api/v1/object-stores", body("logs", "/srv/objstore/media", 9100)).await;
    assert_eq!(s, StatusCode::CONFLICT);
    let (s, _) = admin.post("/api/v1/object-stores", body("logs", "/srv/objstore/logs", 9001)).await;
    assert_eq!(s, StatusCode::CONFLICT, "the console port of `media`");
    let (s, r) = admin.post("/api/v1/object-stores", body("logs", "/srv/objstore/logs", 9100)).await;
    assert_eq!(s, StatusCode::ACCEPTED, "{r}");

    // Windows: native only, Windows paths.
    let win = node(&h, "win-1", "windows").await;
    let wbody = |runtime: &str, path: &str| json!({"name": "W", "slug": "w", "server_id": win, "data_path": path, "runtime": runtime});
    let (s, _) = admin.post("/api/v1/object-stores", wbody("docker", "D:\\objstore")).await;
    assert_eq!(s, StatusCode::BAD_REQUEST);
    let (s, _) = admin.post("/api/v1/object-stores", wbody("native", "/srv/objstore/w")).await;
    assert_eq!(s, StatusCode::BAD_REQUEST);
    let (s, r) = admin.post("/api/v1/object-stores", wbody("native", "D:\\objstore\\w")).await;
    assert_eq!(s, StatusCode::ACCEPTED, "{r}");
}

/// Bucket name → object key → bytes.
type Buckets = BTreeMap<String, BTreeMap<String, Vec<u8>>>;

/// A tiny S3 stand-in: checks that requests are SigV4-signed with the
/// expected key and keeps objects in memory.
#[derive(Clone, Default)]
struct FakeS3 {
    buckets: Arc<Mutex<Buckets>>,
    /// RustFS service accounts: access key → (name, policy).
    accounts: Arc<Mutex<BTreeMap<String, (String, serde_json::Value)>>>,
}

fn signed(h: &HeaderMap) -> bool {
    h.get("authorization")
        .and_then(|v| v.to_str().ok())
        .is_some_and(|a| a.starts_with("AWS4-HMAC-SHA256 Credential=AKTEST/") && a.contains("Signature="))
}

async fn fake_root(State(s): State<FakeS3>, h: HeaderMap) -> (AxStatus, String) {
    if !signed(&h) {
        return (AxStatus::FORBIDDEN, "<Error><Code>AccessDenied</Code><Message>unsigned</Message></Error>".into());
    }

    let names: String = s
        .buckets
        .lock()
        .unwrap()
        .keys()
        .map(|b| format!("<Bucket><Name>{b}</Name><CreationDate>2026-01-01T00:00:00.000Z</CreationDate></Bucket>"))
        .collect();

    (AxStatus::OK, format!("<ListAllMyBucketsResult><Buckets>{names}</Buckets></ListAllMyBucketsResult>"))
}

async fn fake_bucket(
    State(s): State<FakeS3>,
    Path(bucket): Path<String>,
    q: Query<BTreeMap<String, String>>,
    method: axum::http::Method,
) -> (AxStatus, String) {
    let mut all = s.buckets.lock().unwrap();

    match method.as_str() {
        "PUT" => {
            all.entry(bucket).or_default();
            (AxStatus::OK, String::new())
        }
        "DELETE" => match all.get(&bucket) {
            Some(b) if !b.is_empty() => {
                (AxStatus::CONFLICT, "<Error><Code>BucketNotEmpty</Code><Message>not empty</Message></Error>".into())
            }
            _ => {
                all.remove(&bucket);
                (AxStatus::NO_CONTENT, String::new())
            }
        },
        _ => {
            let Some(objs) = all.get(&bucket) else {
                return (AxStatus::NOT_FOUND, "<Error><Code>NoSuchBucket</Code><Message>no</Message></Error>".into());
            };

            let prefix = q.get("prefix").cloned().unwrap_or_default();
            let mut contents = String::new();
            let mut prefixes = std::collections::BTreeSet::new();

            for (k, v) in objs.iter().filter(|(k, _)| k.starts_with(&prefix)) {
                match k[prefix.len()..].find('/') {
                    Some(i) => {
                        prefixes.insert(k[..prefix.len() + i + 1].to_owned());
                    }
                    None => contents.push_str(&format!("<Contents><Key>{k}</Key><Size>{}</Size></Contents>", v.len())),
                }
            }

            let cp: String =
                prefixes.iter().map(|p| format!("<CommonPrefixes><Prefix>{p}</Prefix></CommonPrefixes>")).collect();
            (
                AxStatus::OK,
                format!("<ListBucketResult><IsTruncated>false</IsTruncated>{contents}{cp}</ListBucketResult>"),
            )
        }
    }
}

async fn fake_object(
    State(s): State<FakeS3>,
    Path((bucket, key)): Path<(String, String)>,
    method: axum::http::Method,
    h: HeaderMap,
    body: Bytes,
) -> (AxStatus, Vec<u8>) {
    if !signed(&h) {
        return (AxStatus::FORBIDDEN, Vec::new());
    }

    let mut all = s.buckets.lock().unwrap();
    let Some(b) = all.get_mut(&bucket) else { return (AxStatus::NOT_FOUND, Vec::new()) };

    match method.as_str() {
        "PUT" => {
            b.insert(key, body.to_vec());
            (AxStatus::OK, Vec::new())
        }
        "DELETE" => {
            b.remove(&key);
            (AxStatus::NO_CONTENT, Vec::new())
        }
        _ => b.get(&key).map_or((AxStatus::NOT_FOUND, Vec::new()), |v| (AxStatus::OK, v.clone())),
    }
}

async fn fake_add_account(State(s): State<FakeS3>, h: HeaderMap, body: Bytes) -> (AxStatus, String) {
    if !signed(&h) {
        return (AxStatus::FORBIDDEN, String::new());
    }

    let req: serde_json::Value = serde_json::from_slice(&body).unwrap_or_default();
    assert!(req["policy"]["Statement"].is_array(), "a scoped policy is sent");
    let mut accts = s.accounts.lock().unwrap();
    let ak = format!("SVC{:05}", accts.len() + 1);
    accts.insert(ak.clone(), (req["name"].as_str().unwrap_or_default().to_owned(), req["policy"].clone()));

    (AxStatus::OK, json!({"credentials": {"accessKey": ak, "secretKey": "svcsecret-value-1"}}).to_string())
}

async fn fake_list_accounts(State(s): State<FakeS3>, h: HeaderMap) -> (AxStatus, String) {
    if !signed(&h) {
        return (AxStatus::FORBIDDEN, String::new());
    }

    let accts = s.accounts.lock().unwrap();
    let list: Vec<serde_json::Value> =
        accts.iter().map(|(k, (n, _))| json!({"accessKey": k, "name": n, "accountStatus": "on"})).collect();

    (AxStatus::OK, json!({"accounts": list}).to_string())
}

async fn fake_delete_account(State(s): State<FakeS3>, h: HeaderMap, q: Query<BTreeMap<String, String>>) -> AxStatus {
    if !signed(&h) {
        return AxStatus::FORBIDDEN;
    }

    match s.accounts.lock().unwrap().remove(q.get("accessKey").map_or("", String::as_str)) {
        Some(_) => AxStatus::NO_CONTENT,
        None => AxStatus::NOT_FOUND,
    }
}

async fn fake_info_account(
    State(s): State<FakeS3>,
    h: HeaderMap,
    q: Query<BTreeMap<String, String>>,
) -> (AxStatus, String) {
    if !signed(&h) {
        return (AxStatus::FORBIDDEN, String::new());
    }

    match s.accounts.lock().unwrap().get(q.get("accessKey").map_or("", String::as_str)) {
        // RustFS returns the policy as pretty-printed JSON text.
        Some((n, p)) => (
            AxStatus::OK,
            json!({"parentUser": "AKTEST", "accountStatus": "on", "impliedPolicy": false, "name": n,
                   "policy": serde_json::to_string_pretty(p).unwrap()})
            .to_string(),
        ),
        None => (AxStatus::NOT_FOUND, "<Error><Code>NoSuchKey</Code><Message>no</Message></Error>".into()),
    }
}

async fn fake_update_account(
    State(s): State<FakeS3>,
    h: HeaderMap,
    q: Query<BTreeMap<String, String>>,
    body: Bytes,
) -> AxStatus {
    if !signed(&h) {
        return AxStatus::FORBIDDEN;
    }

    let req: serde_json::Value = serde_json::from_slice(&body).unwrap_or_default();

    match s.accounts.lock().unwrap().get_mut(q.get("accessKey").map_or("", String::as_str)) {
        Some(a) => {
            a.1 = req["newPolicy"].clone();
            AxStatus::NO_CONTENT
        }
        None => AxStatus::NOT_FOUND,
    }
}

async fn start_fake_s3() -> String {
    start_fake_s3_with().await.0
}

/// The stand-in and its state, to change keys "directly in RustFS".
async fn start_fake_s3_with() -> (String, FakeS3) {
    let fake = FakeS3::default();
    let app = axum::Router::new()
        .route("/", get(fake_root))
        .route("/rustfs/admin/v3/add-service-accounts", axum::routing::put(fake_add_account))
        .route("/rustfs/admin/v3/list-service-accounts", get(fake_list_accounts))
        .route("/rustfs/admin/v3/delete-service-accounts", axum::routing::delete(fake_delete_account))
        .route("/rustfs/admin/v3/info-service-account", get(fake_info_account))
        .route("/rustfs/admin/v3/update-service-account", post(fake_update_account))
        .route("/{bucket}", get(fake_bucket).put(fake_bucket).delete(fake_bucket))
        .route("/{bucket}/{*key}", get(fake_object).put(fake_object).delete(fake_object))
        .with_state(fake.clone());
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(listener, app).await.ok() });

    (format!("http://{addr}"), fake)
}

#[tokio::test]
async fn distributed_store_spans_nodes_with_one_layout() {
    let h = start().await;
    let mut admin = Client::login(&h.base, ADMIN_EMAIL, PASSWORD).await;
    step_up(&mut admin).await;
    let (a, b, c) = (node(&h, "s3-a", "linux").await, node(&h, "s3-b", "linux").await, node(&h, "s3-c", "linux").await);
    let win = node(&h, "s3-win", "windows").await;
    let member = |id: forge_core::ServerId, n: usize| json!({"server_id": id, "data_paths": (0..n).map(|i| format!("/srv/rustfs/d{i}")).collect::<Vec<_>>()});
    let body = |members: serde_json::Value| json!({"name": "HA", "slug": "ha", "topology": "distributed", "runtime": "native", "members": members});

    let (s, r) = admin.post("/api/v1/object-stores", body(json!([member(a, 4)]))).await;
    assert_eq!(s, StatusCode::BAD_REQUEST, "one node is not HA: {r}");
    let (s, r) = admin.post("/api/v1/object-stores", body(json!([member(a, 1), member(b, 1), member(c, 1)]))).await;
    assert_eq!(s, StatusCode::BAD_REQUEST, "3 drives are below the minimum: {r}");
    let (s, r) = admin.post("/api/v1/object-stores", body(json!([member(a, 2), member(b, 1)]))).await;
    assert_eq!(s, StatusCode::BAD_REQUEST, "unequal drive counts: {r}");
    let (s, r) = admin.post("/api/v1/object-stores", body(json!([member(a, 2), member(win, 2)]))).await;
    assert_eq!(s, StatusCode::BAD_REQUEST, "Linux only: {r}");
    let (s, r) = admin.post("/api/v1/object-stores", body(json!([member(a, 2), member(a, 2)]))).await;
    assert_eq!(s, StatusCode::BAD_REQUEST, "a node once: {r}");
    let (s, r) = admin
        .post(
            "/api/v1/object-stores",
            json!({"name": "HA", "slug": "ha", "topology": "standalone", "members": [member(a, 1), member(b, 1)]}),
        )
        .await;
    assert_eq!(s, StatusCode::BAD_REQUEST, "standalone is one node: {r}");

    let (s, r) = admin.post("/api/v1/object-stores", body(json!([member(a, 2), member(b, 2)]))).await;
    assert_eq!(s, StatusCode::ACCEPTED, "{r}");
    assert_eq!(r["store"]["topology"], "distributed");
    assert_eq!(r["store"]["members"].as_array().unwrap().len(), 2);
    assert_eq!(r["jobs"].as_array().unwrap().len(), 2, "one install job per member");

    for j in r["jobs"].as_array().unwrap() {
        let (_, job) = admin.get(&format!("/api/v1/jobs/{}", j["id"].as_str().unwrap())).await;
        assert!(!job["payload"].to_string().contains("secret"), "{job}");
    }

    // The layout is the same on every member: all drives of all members.
    let store_id: forge_core::ObjectStoreId = r["store"]["id"].as_str().unwrap().parse().unwrap();
    let st = &h.app.state.store;
    let store = forge_core::storage::ObjectStoreRepository::get_object_store(st, store_id).await.unwrap().unwrap();
    let members = forge_core::storage::ObjectStoreRepository::list_store_members(st, store_id).await.unwrap();
    let specs: Vec<_> =
        members.iter().map(|m| forge_server::jobs::storage::spec(&store, m, &members, None).unwrap()).collect();
    assert_eq!(specs[0].peers, specs[1].peers);
    assert_eq!(
        specs[0].peers,
        vec![
            "http://s3-a:9000/srv/rustfs/d0",
            "http://s3-a:9000/srv/rustfs/d1",
            "http://s3-b:9000/srv/rustfs/d0",
            "http://s3-b:9000/srv/rustfs/d1"
        ]
    );

    // The same drives on a member node are taken now.
    let (s, _) = admin
        .post(
            "/api/v1/object-stores",
            body(json!([member(b, 2), member(c, 2)])).as_object().cloned().map_or(json!({}), |mut o| {
                o.insert("slug".into(), json!("ha2"));
                o.insert("s3_port".into(), json!(9200));
                serde_json::Value::Object(o)
            }),
        )
        .await;
    assert_eq!(s, StatusCode::CONFLICT);
}

#[tokio::test]
async fn connected_store_buckets_and_objects() {
    let h = start().await;
    let mut admin = Client::login(&h.base, ADMIN_EMAIL, PASSWORD).await;
    let endpoint = start_fake_s3().await;
    let connect = |ep: &str, sk: &str| {
        json!({"name": "Cloud", "provider": "s3", "endpoint": ep, "region": "us-east-1", "path_style": true,
               "access_key": "AKTEST", "secret_key": sk})
    };
    let (s, _) = admin.post("/api/v1/object-stores/connect", connect(&endpoint, "secret-1")).await;
    assert_eq!(s, StatusCode::FORBIDDEN, "connecting needs an MFA session");
    step_up(&mut admin).await;

    let (s, r) = admin.post("/api/v1/object-stores/connect", connect("http://127.0.0.1:1", "secret-1")).await;
    assert_eq!(s, StatusCode::BAD_GATEWAY, "an unreachable store is not saved: {r}");
    let (s, r) = admin.post("/api/v1/object-stores/connect", connect("https://u:p@host", "x")).await;
    assert_eq!(s, StatusCode::BAD_REQUEST, "{r}");
    let (_, list) = admin.get("/api/v1/object-stores").await;
    assert_eq!(list.as_array().unwrap().len(), 0);

    let (s, store) = admin.post("/api/v1/object-stores/connect", connect(&endpoint, "secret-1")).await;
    assert_eq!(s, StatusCode::CREATED, "{store}");
    assert_eq!(store["mode"], "external");
    assert!(!store.to_string().contains("secret-1"), "the secret never comes back");
    let base = format!("/api/v1/object-stores/{}", store["id"].as_str().unwrap());

    let (s, r) = admin.post(&format!("{base}/buckets"), json!({"name": "Bad_Name"})).await;
    assert_eq!(s, StatusCode::BAD_REQUEST, "{r}");
    let (s, r) = admin.post(&format!("{base}/buckets"), json!({"name": "media"})).await;
    assert_eq!(s, StatusCode::CREATED, "{r}");
    let (_, b) = admin.get(&format!("{base}/buckets")).await;
    assert_eq!(b[0]["name"], "media", "{b}");

    // Upload and download stream through the control plane.
    let payload = vec![7u8; 256 * 1024];
    let url = format!("{}{base}/buckets/media/object?key=docs%2Fa%20b.bin", h.base);
    let resp = admin
        .http
        .put(&url)
        .header("x-csrf-token", admin.csrf.clone().unwrap())
        .header("content-type", "application/octet-stream")
        .body(payload.clone())
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::CREATED, "{}", resp.text().await.unwrap());
    let (_, page) = admin.get(&format!("{base}/buckets/media/objects")).await;
    assert_eq!(page["prefixes"], json!(["docs/"]), "{page}");
    let (_, page) = admin.get(&format!("{base}/buckets/media/objects?prefix=docs%2F")).await;
    assert_eq!(page["objects"][0]["key"], "docs/a b.bin", "{page}");
    assert_eq!(page["objects"][0]["size"], 256 * 1024);
    let resp = admin.http.get(&url).send().await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    assert!(resp.headers()["content-disposition"].to_str().unwrap().contains("a_b.bin"));
    assert_eq!(resp.bytes().await.unwrap().to_vec(), payload);

    // Developers browse and transfer; deleting buckets is storage.manage.
    let dev = user_with_role(&h, &admin, "dev@example.com", "developer", &format!("workspace:{}", h.default_org)).await;
    let (s, _) = dev.get(&format!("{base}/buckets")).await;
    assert_eq!(s, StatusCode::OK);
    let (s, _) = dev.call(Method::DELETE, &format!("{base}/buckets/media"), None).await;
    assert_eq!(s, StatusCode::FORBIDDEN);
    let auditor =
        user_with_role(&h, &admin, "aud@example.com", "auditor", &format!("workspace:{}", h.default_org)).await;
    let (s, _) = auditor.get(&format!("{base}/buckets")).await;
    assert_eq!(s, StatusCode::FORBIDDEN);

    let (s, _) = admin.call(Method::DELETE, &format!("{base}/buckets/media"), None).await;
    assert_eq!(s, StatusCode::CONFLICT, "a bucket with objects is not deleted");
    let (s, _) = admin.call(Method::DELETE, &format!("{base}/buckets/media/object?key=docs%2Fa%20b.bin"), None).await;
    assert_eq!(s, StatusCode::NO_CONTENT);
    let (s, _) = admin.call(Method::DELETE, &format!("{base}/buckets/media"), None).await;
    assert_eq!(s, StatusCode::NO_CONTENT);

    let (_, t) = admin.post(&format!("{base}/test"), json!({})).await;
    assert_eq!(t["ok"], true, "{t}");
    let (s, _) = admin.post(&format!("{base}/actions"), json!({"action": "start"})).await;
    assert_eq!(s, StatusCode::BAD_REQUEST, "connected stores have no lifecycle");
    let (s, _) = admin.post(&format!("{base}/actions"), json!({"action": "disconnect"})).await;
    assert_eq!(s, StatusCode::OK);
    let (_, list) = admin.get("/api/v1/object-stores").await;
    assert_eq!(list.as_array().unwrap().len(), 0);
}

/// Records a managed RustFS store whose endpoint is the S3 stand-in, with
/// `AKTEST` as its root key in the vault.
async fn managed_store_on(
    h: &crate::common::Harness,
    endpoint: &str,
    server: forge_core::ServerId,
) -> forge_core::ObjectStoreId {
    managed_store_with(h, endpoint, server, "native").await
}

async fn managed_store_with(
    h: &crate::common::Harness,
    endpoint: &str,
    server: forge_core::ServerId,
    runtime: &str,
) -> forge_core::ObjectStoreId {
    use forge_core::storage::{ObjectStore, ObjectStoreRepository, StoreMember};
    let st = &h.app.state;
    let ws: forge_core::WorkspaceId = h.default_org.parse().unwrap();
    let cred = forge_core::VaultItemId::new();
    let sealed = st.envelope.seal(b"root-secret-1", &format!("vault:{cred}")).unwrap();
    forge_core::ports::VaultRepository::put_vault_item(
        &st.store,
        &forge_core::model::VaultItem {
            id: cred,
            workspace_id: ws,
            name: "root".into(),
            username: "AKTEST".into(),
            url: String::new(),
            notes: String::new(),
            tags: vec![],
            links: vec![],
            sealed,
            created_by: forge_core::Principal::User(forge_core::UserId::new()),
            created_at: forge_core::Timestamp(0),
            updated_at: forge_core::Timestamp(0),
            rotated_at: forge_core::Timestamp(0),
        },
    )
    .await
    .unwrap();
    let id = forge_core::ObjectStoreId::new();
    let port: u16 = endpoint.rsplit(':').next().unwrap().parse().unwrap();
    st.store
        .put_object_store(&ObjectStore {
            id,
            workspace_id: ws,
            environment_id: None,
            name: "Media".into(),
            slug: "media".into(),
            provider: "rustfs".into(),
            mode: "managed".into(),
            region: "us-east-1".into(),
            path_style: true,
            version: "latest".into(),
            deployed_version: Some("1.0.5".into()),
            runtime: runtime.into(),
            topology: "standalone".into(),
            s3_port: port,
            console_port: None,
            endpoint: endpoint.into(),
            credential_id: cred,
            status: "running".into(),
            last_error: None,
            last_job_id: None,
            created_by: forge_core::Principal::User(forge_core::UserId::new()),
            created_at: forge_core::Timestamp(0),
            updated_at: forge_core::Timestamp(0),
        })
        .await
        .unwrap();
    st.store
        .put_store_member(&StoreMember {
            store_id: id,
            server_id: server,
            data_paths: vec!["/srv/objstore/media".into()],
            endpoint_host: "127.0.0.1".into(),
            status: "running".into(),
            capacity_bytes: None,
            free_bytes: None,
            require_mount: false,
            dedicated: None,
            observed_at: None,
        })
        .await
        .unwrap();

    id
}

#[tokio::test]
async fn app_keys_are_bucket_scoped_and_live_in_the_vault() {
    let h = start().await;
    let mut admin = Client::login(&h.base, ADMIN_EMAIL, PASSWORD).await;
    let srv = node(&h, "store-1", "linux").await;
    let endpoint = start_fake_s3().await;
    let id = managed_store_on(&h, &endpoint, srv).await;
    let base = format!("/api/v1/object-stores/{id}");

    let (s, _) = admin.post(&format!("{base}/keys"), json!({"name": "shop", "bucket": "media"})).await;
    assert_eq!(s, StatusCode::FORBIDDEN, "issuing keys needs MFA");
    step_up(&mut admin).await;
    let (s, r) = admin.post(&format!("{base}/keys"), json!({"name": "shop", "bucket": "media"})).await;
    assert_eq!(s, StatusCode::NOT_FOUND, "the bucket must exist: {r}");
    let (s, _) = admin.post(&format!("{base}/buckets"), json!({"name": "media"})).await;
    assert_eq!(s, StatusCode::CREATED);
    let (s, r) = admin.post(&format!("{base}/keys"), json!({"name": "bad name", "bucket": "media"})).await;
    assert_eq!(s, StatusCode::BAD_REQUEST, "{r}");

    let (s, k) =
        admin.post(&format!("{base}/keys"), json!({"name": "shop", "bucket": "media", "access": "read"})).await;
    assert_eq!(s, StatusCode::CREATED, "{k}");
    assert!(!k.to_string().contains("svcsecret"), "the secret stays in the vault: {k}");
    let ak = k["access_key"].as_str().unwrap().to_owned();
    let item_id: forge_core::VaultItemId = k["vault_item_id"].as_str().unwrap().parse().unwrap();
    let item = forge_core::ports::VaultRepository::get_vault_item(&h.app.state.store, item_id).await.unwrap().unwrap();
    assert_eq!(item.username, ak);
    assert!(item.tags.contains(&"bucket:media".to_owned()) && item.tags.contains(&"access:read".to_owned()));

    let (_, keys) = admin.get(&format!("{base}/keys")).await;
    assert_eq!(keys[0]["access_key"], ak.as_str());
    assert_eq!(keys[0]["managed_by_forge"], true);
    assert_eq!(keys[0]["bucket"], "media");

    // Rotation proves the new key, then retires the old one.
    let (s, r) = admin.post(&format!("{base}/keys/{ak}/rotate"), json!({})).await;
    assert_eq!(s, StatusCode::OK, "{r}");
    let new_ak = r["access_key"].as_str().unwrap().to_owned();
    assert_ne!(new_ak, ak);
    let (_, keys) = admin.get(&format!("{base}/keys")).await;
    assert_eq!(keys.as_array().unwrap().len(), 1, "{keys}");
    assert_eq!(keys[0]["access_key"], new_ak.as_str());
    let item = forge_core::ports::VaultRepository::get_vault_item(&h.app.state.store, item_id).await.unwrap().unwrap();
    assert_eq!(item.username, new_ak, "same vault item, new key");

    // Developers may see keys but not issue or revoke them.
    let ws = format!("workspace:{}", h.default_org);
    let dev = user_with_role(&h, &admin, "dev2@example.com", "developer", &ws).await;
    let (s, _) = dev.get(&format!("{base}/keys")).await;
    assert_eq!(s, StatusCode::OK);
    let (s, _) = dev.call(Method::DELETE, &format!("{base}/keys/{new_ak}"), None).await;
    assert_eq!(s, StatusCode::FORBIDDEN);
    let (s, _) = admin.call(Method::DELETE, &format!("{base}/keys/AKTEST"), None).await;
    assert_eq!(s, StatusCode::CONFLICT, "never the root key");
    let (s, _) = admin.call(Method::DELETE, &format!("{base}/keys/{new_ak}"), None).await;
    assert_eq!(s, StatusCode::NO_CONTENT);
    assert!(forge_core::ports::VaultRepository::get_vault_item(&h.app.state.store, item_id).await.unwrap().is_none());

    let (_, audit) = admin.get("/api/v1/audit?limit=200").await;
    let text = audit.to_string();

    for a in ["storage.key.create", "storage.key.rotate", "storage.key.revoke"] {
        assert!(text.contains(a), "{a} audited");
    }

    assert!(!text.contains("svcsecret"));
}

#[tokio::test]
async fn adopting_the_app_store_container_keeps_it_in_place() {
    use forge_core::ports::InstallationRepository;
    let h = start().await;
    let mut admin = Client::login(&h.base, ADMIN_EMAIL, PASSWORD).await;
    step_up(&mut admin).await;
    let srv = node(&h, "legacy-1", "linux").await;
    let (s, _) = admin.post("/api/v1/object-stores/adopt", json!({"name": "Legacy", "server_id": srv})).await;
    assert_eq!(s, StatusCode::NOT_FOUND, "nothing to adopt yet");
    let inst = forge_core::model::Installation {
        id: forge_core::InstallationId::new(),
        workspace_id: h.default_org.parse().unwrap(),
        server_id: srv,
        package: "rustfs".into(),
        version: "latest".into(),
        params: BTreeMap::from([
            ("port".to_owned(), "9300".to_owned()),
            ("console_port".to_owned(), "9301".to_owned()),
        ]),
        status: "installed".into(),
        state: Some("running".into()),
        env: BTreeMap::new(),
        last_job_id: None,
        last_error: None,
        versions: None,
        update_policy: "notify".into(),
        created_at: forge_core::Timestamp(0),
        updated_at: forge_core::Timestamp(0),
    };
    h.app.state.store.put_installation(&inst).await.unwrap();
    let (s, r) = admin.post("/api/v1/object-stores/adopt", json!({"name": "Legacy", "server_id": srv})).await;
    assert_eq!(s, StatusCode::ACCEPTED, "{r}");
    assert_eq!(r["store"]["runtime"], "docker");
    assert_eq!(r["store"]["s3_port"], 9300);
    assert_eq!(r["store"]["members"][0]["data_paths"], json!(["/var/lib/forge/pkgs/rustfs"]));
    assert_eq!(r["job"]["payload"]["action"], "adopt");
    let (s, _) = admin.post("/api/v1/object-stores/adopt", json!({"name": "Again", "server_id": srv})).await;
    assert_eq!(s, StatusCode::CONFLICT, "adopted once");

    // While adopting, other actions wait.
    let id = r["store"]["id"].as_str().unwrap();
    let (s, _) = admin.post(&format!("/api/v1/object-stores/{id}/actions"), json!({"action": "restart"})).await;
    assert_eq!(s, StatusCode::CONFLICT);
}

/// Plays the agent: takes the next storage command queued for `server`,
/// answers it and returns its operation JSON.
async fn answer(h: &crate::common::Harness, server: forge_core::ServerId, ok: bool, state: &str) -> String {
    answer_with(h, server, ok, state, "object store action failed").await
}

async fn answer_with(
    h: &crate::common::Harness,
    server: forge_core::ServerId,
    ok: bool,
    state: &str,
    error: &str,
) -> String {
    use forge_proto::messages::{CommandBody, CommandResult, command_body::Command};
    use prost::Message;
    let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(20);

    let cmd = loop {
        if let Some(c) = h.app.state.hub.take(server, 1).pop() {
            break c;
        }

        assert!(tokio::time::Instant::now() < deadline, "no command for the node");
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    };

    let body = CommandBody::decode(cmd.body.as_slice()).unwrap();
    let Some(Command::StorageAction(a)) = body.command else { panic!("not a storage command") };

    let result = if ok {
        let out = json!({"output": "forge: done", "report": {"state": state, "version": "1.0.5"}});
        CommandResult { operation_id: body.operation_id, ok: true, output: out.to_string(), ..Default::default() }
    } else {
        let err = json!({"error": error, "output": "forge: error: boom"});
        CommandResult { operation_id: body.operation_id, ok: false, error: err.to_string(), ..Default::default() }
    };

    assert!(h.app.state.hub.resolve(server, result));

    a.op_json
}

async fn store_status(admin: &Client, id: forge_core::ObjectStoreId) -> Option<String> {
    let (s, r) = admin.get(&format!("/api/v1/object-stores/{id}")).await;
    (s == StatusCode::OK).then(|| r["status"].as_str().unwrap_or_default().to_owned())
}

async fn wait_status(admin: &Client, id: forge_core::ObjectStoreId, want: Option<&str>) {
    let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(20);

    loop {
        let now = store_status(admin, id).await;
        if now.as_deref() == want {
            return;
        }

        assert!(tokio::time::Instant::now() < deadline, "store status {now:?}, want {want:?}");
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    }
}

#[tokio::test]
async fn a_failed_first_install_leaves_no_store_behind() {
    let h = start().await;
    let mut admin = Client::login(&h.base, ADMIN_EMAIL, PASSWORD).await;
    step_up(&mut admin).await;
    let srv = node(&h, "store-rb", "linux").await;
    let (s, r) = admin
        .post(
            "/api/v1/object-stores",
            json!({"name": "Temp", "slug": "temp", "server_id": srv, "data_path": "/srv/objstore/temp"}),
        )
        .await;
    assert_eq!(s, StatusCode::ACCEPTED, "{r}");
    let id: forge_core::ObjectStoreId = r["store"]["id"].as_str().unwrap().parse().unwrap();
    let cred: forge_core::VaultItemId = r["store"]["credential_id"].as_str().unwrap().parse().unwrap();
    assert!(answer(&h, srv, false, "").await.contains("\"install\""));

    // The rollback cleans the node up, then the record goes.
    assert!(answer(&h, srv, true, "not-installed").await.contains("\"uninstall\""));
    wait_status(&admin, id, None).await;
    assert!(forge_core::ports::VaultRepository::get_vault_item(&h.app.state.store, cred).await.unwrap().is_none());
    let job =
        crate::common::wait_job(&admin, r["job"]["id"].as_str().unwrap(), std::time::Duration::from_secs(20)).await;
    assert_eq!(job["status"], "failed", "the job and its log stay: {job}");
}

#[tokio::test]
async fn disable_enable_and_remove_an_errored_store() {
    let h = start().await;
    let mut admin = Client::login(&h.base, ADMIN_EMAIL, PASSWORD).await;
    step_up(&mut admin).await;
    let srv = node(&h, "store-de", "linux").await;
    let id = managed_store_on(&h, "http://127.0.0.1:9555", srv).await;
    let act = |body: serde_json::Value| {
        let admin = &admin;
        async move { admin.post(&format!("/api/v1/object-stores/{id}/actions"), body).await }
    };

    let (s, r) = act(json!({"action": "disable"})).await;
    assert_eq!(s, StatusCode::ACCEPTED, "{r}");
    assert!(answer(&h, srv, true, "stopped").await.contains("\"stop\""));
    wait_status(&admin, id, Some("disabled")).await;

    let (s, r) = act(json!({"action": "enable"})).await;
    assert_eq!(s, StatusCode::ACCEPTED, "{r}");
    assert!(answer(&h, srv, true, "running").await.contains("\"start\""));
    wait_status(&admin, id, Some("running")).await;

    // A failed start leaves the store in error; it can still be removed.
    let (s, _) = act(json!({"action": "start"})).await;
    assert_eq!(s, StatusCode::ACCEPTED);
    answer(&h, srv, false, "").await;
    wait_status(&admin, id, Some("error")).await;

    let (s, _) = act(json!({"action": "uninstall", "delete_data": true})).await;
    assert_eq!(s, StatusCode::BAD_REQUEST, "deleting data needs the typed name");
    let (s, _) = act(json!({"action": "uninstall", "delete_data": true, "confirm": "Other"})).await;
    assert_eq!(s, StatusCode::BAD_REQUEST);
    let (s, r) = act(json!({"action": "uninstall", "delete_data": true, "confirm": "Media"})).await;
    assert_eq!(s, StatusCode::ACCEPTED, "{r}");
    let op = answer(&h, srv, true, "not-installed").await;
    assert!(op.contains("\"uninstall\"") && op.contains("\"delete_data\":true"), "{op}");
    wait_status(&admin, id, None).await;
}

#[tokio::test]
async fn an_agent_without_ownership_markers_keeps_working_but_cannot_delete_data() {
    let h = start().await;
    let mut admin = Client::login(&h.base, ADMIN_EMAIL, PASSWORD).await;
    step_up(&mut admin).await;
    let srv = node(&h, "store-old", "linux").await;
    let id = managed_store_on(&h, "http://127.0.0.1:9556", srv).await;
    let path = format!("/api/v1/object-stores/{id}/actions");
    const OLD: &str = "invalid object store operation";

    let (s, _) = admin.post(&path, json!({"action": "restart"})).await;
    assert_eq!(s, StatusCode::ACCEPTED);
    let first = answer_with(&h, srv, false, "", OLD).await;
    assert!(first.contains("\"owner\""), "{first}");
    let again = answer(&h, srv, true, "running").await;
    assert!(!again.contains("\"owner\""), "resent without the marker field: {again}");
    wait_status(&admin, id, Some("running")).await;

    let (s, r) = admin.post(&path, json!({"action": "uninstall", "delete_data": true, "confirm": "Media"})).await;
    assert_eq!(s, StatusCode::ACCEPTED, "{r}");
    answer_with(&h, srv, false, "", OLD).await;
    let job =
        crate::common::wait_job(&admin, r["jobs"][0]["id"].as_str().unwrap(), std::time::Duration::from_secs(20)).await;
    assert!(job["last_error"].as_str().unwrap_or_default().contains("update the agent on"), "{job}");
}

/// D-106: a bucket attached to a service gets its own bucket-limited
/// key in the vault, and the service's commands carry the endpoint,
/// bucket and key as environment variables. Detaching revokes the key.
#[tokio::test]
async fn a_bucket_attached_to_an_service_reaches_it_as_environment() {
    use forge_core::ports::{ServiceRepository, VaultRepository};
    use forge_proto::messages::{CommandBody, command_body::Command};
    use prost::Message;
    let h = start().await;
    let mut admin = Client::login(&h.base, ADMIN_EMAIL, PASSWORD).await;
    let srv = node(&h, "store-1", "linux").await;
    let endpoint = start_fake_s3().await;
    let store = managed_store_on(&h, &endpoint, srv).await;
    let (_, cl) = admin.post("/api/v1/clusters", json!({ "name": "shop" })).await;
    let (_, group) = admin.post("/api/v1/projects", json!({ "name": "web" })).await;
    let (_, ns) = admin
        .post("/api/v1/environments", json!({ "name": "uat", "cluster_id": cl["id"], "project_id": group["id"] }))
        .await;
    let (s, app) = admin
        .post(
            "/api/v1/services",
            json!({ "environment_id": ns["id"], "name": "shop", "runtime": "systemd", "command": "x",
                    "env": [{ "key": "MEDIA_BUCKET", "value": "mine" }] }),
        )
        .await;
    assert_eq!(s, StatusCode::CREATED, "{app}");
    let service_id = app["id"].as_str().unwrap().to_owned();
    let url = format!("/api/v1/services/{service_id}/storage");

    let (s, v) = admin.get(&url).await;
    assert_eq!(s, StatusCode::OK, "{v}");
    assert!(v["attachments"].as_array().unwrap().is_empty() && v["can_change"] == true, "{v}");

    // Issuing a key on the store needs MFA.
    let body = json!({ "store_id": store, "bucket": "media", "create_bucket": true });
    assert_eq!(admin.post(&url, body.clone()).await.0, StatusCode::FORBIDDEN);
    step_up(&mut admin).await;
    let (s, v) = admin.post(&url, body.clone()).await;
    assert_eq!(s, StatusCode::CREATED, "{v}");
    let a = &v["attachments"][0];
    assert_eq!((&a["bucket"], &a["prefix"], &a["access"]), (&json!("media"), &json!("S3"), &json!("readwrite")), "{v}");
    assert_eq!(a["env"]["secret_key"], "S3_SECRET_ACCESS_KEY", "{v}");
    assert_eq!(a["endpoint"], endpoint.as_str(), "{v}");
    assert!(!v.to_string().contains("SKTEST"), "no secret in the answer: {v}");
    let access_key = a["access_key"].as_str().unwrap().to_owned();

    // The prefix must be free, valid, and not clash with the app's own variables.
    assert_eq!(admin.post(&url, body).await.0, StatusCode::CONFLICT, "same prefix twice");
    let with = |prefix: &str| json!({ "store_id": store, "bucket": "media", "access": "read", "prefix": prefix });
    assert_eq!(admin.post(&url, with("s3")).await.0, StatusCode::BAD_REQUEST);
    let (s, e) = admin.post(&url, with("MEDIA")).await;
    assert_eq!(s, StatusCode::CONFLICT, "{e}");
    assert!(e["detail"].as_str().unwrap().contains("MEDIA_BUCKET"), "{e}");
    let (s, v) = admin.post(&url, with("ASSETS")).await;
    assert_eq!(s, StatusCode::CREATED, "{v}");
    assert_eq!(v["attachments"].as_array().unwrap().len(), 2, "{v}");

    // A bucket that doesn't exist is refused unless it is to be created.
    let missing = json!({ "store_id": store, "bucket": "nope", "prefix": "OTHER" });
    assert_eq!(admin.post(&url, missing).await.0, StatusCode::NOT_FOUND);

    // A command for the service carries the variables; the app's own
    // variables are untouched.
    h.app
        .state
        .store
        .put_instance(&forge_core::model::ServiceInstance {
            service_id: service_id.parse().unwrap(),
            server_id: srv,
            role: "active".into(),
            status: "deployed".into(),
            state: Some("running".into()),
            last_job_id: None,
            last_error: None,
            updated_at: forge_core::Timestamp(1),
        })
        .await
        .unwrap();

    // The test plays the agent while the action waits for its answer.
    let action_url = format!("/api/v1/services/{service_id}/instances/{srv}/actions");
    let action = admin.post(&action_url, json!({ "action": "status" }));
    let agent = async {
        let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(20);

        loop {
            let found = h.app.state.hub.take(srv, 8).into_iter().find_map(|c| {
                let body = CommandBody::decode(c.body.as_slice()).ok()?;
                match body.command? {
                    Command::AppAction(a) => Some((body.operation_id, serde_json::from_str(&a.spec_json).ok()?)),
                    _ => None,
                }
            });

            if let Some((operation_id, spec)) = found {
                let done = forge_proto::messages::CommandResult {
                    operation_id,
                    ok: true,
                    output: json!({"output": "active"}).to_string(),
                    ..Default::default()
                };
                assert!(h.app.state.hub.resolve(srv, done));

                break spec;
            }

            assert!(tokio::time::Instant::now() < deadline, "no service command for the node");
            tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        }
    };
    let ((s, r), spec): ((StatusCode, serde_json::Value), serde_json::Value) = tokio::join!(action, agent);
    assert!(s.is_success(), "{r}");
    let env: std::collections::BTreeMap<String, String> = spec["env"]
        .as_array()
        .unwrap()
        .iter()
        .map(|kv| (kv[0].as_str().unwrap().to_owned(), kv[1].as_str().unwrap().to_owned()))
        .collect();
    assert_eq!(env["S3_ENDPOINT"], endpoint, "{env:?}");
    assert_eq!((env["S3_BUCKET"].as_str(), env["S3_REGION"].as_str()), ("media", "us-east-1"), "{env:?}");
    assert_eq!(env["S3_ACCESS_KEY_ID"], access_key, "{env:?}");
    assert!(env["S3_SECRET_ACCESS_KEY"].len() >= 8, "the secret is unsealed for the node");
    assert_eq!(env["MEDIA_BUCKET"], "mine", "the app's own variable stays");
    assert!(env.contains_key("ASSETS_SECRET_ACCESS_KEY"), "{env:?}");

    // Detaching revokes the key at the store and removes it from the vault.
    let (s, v) = admin.call(Method::DELETE, &format!("{url}/{access_key}"), None).await;
    assert_eq!(s, StatusCode::OK, "{v}");
    assert_eq!(v["attachments"].as_array().unwrap().len(), 1, "{v}");
    let (_, keys) = admin.get(&format!("/api/v1/object-stores/{store}/keys")).await;
    assert!(keys.as_array().unwrap().iter().all(|k| k["access_key"] != access_key.as_str()), "{keys}");
    let ws: forge_core::WorkspaceId = h.default_org.parse().unwrap();
    let left = h.app.state.store.list_vault_items(ws).await.unwrap();
    assert!(left.iter().all(|i| i.username != access_key));
    assert_eq!(admin.call(Method::DELETE, &format!("{url}/{access_key}"), None).await.0, StatusCode::NOT_FOUND);

    let (_, audit) = admin.get("/api/v1/audit?limit=50").await;
    let actions: Vec<&str> = audit["events"]
        .as_array()
        .or_else(|| audit.as_array())
        .unwrap()
        .iter()
        .filter_map(|e| e["action"].as_str())
        .collect();
    assert!(actions.contains(&"service.storage.attach") && actions.contains(&"service.storage.detach"));
}

/// A cluster, a project, two environments (`uat`, `prod`) and one service
/// in each. Returns `(uat, prod, uat service, prod service)` ids.
async fn two_environments(admin: &Client) -> (String, String, String, String) {
    let (_, cl) = admin.post("/api/v1/clusters", json!({ "name": "shop" })).await;
    let (_, group) = admin.post("/api/v1/projects", json!({ "name": "web" })).await;
    let mut ids = Vec::new();

    for name in ["uat", "prod"] {
        let (s, ns) = admin
            .post("/api/v1/environments", json!({ "name": name, "cluster_id": cl["id"], "project_id": group["id"] }))
            .await;
        assert_eq!(s, StatusCode::CREATED, "{ns}");
        let (s, app) = admin
            .post(
                "/api/v1/services",
                json!({ "environment_id": ns["id"], "name": format!("shop-{name}"), "runtime": "systemd", "command": "x" }),
            )
            .await;
        assert_eq!(s, StatusCode::CREATED, "{app}");
        ids.push((ns["id"].as_str().unwrap().to_owned(), app["id"].as_str().unwrap().to_owned()));
    }

    (ids[0].0.clone(), ids[1].0.clone(), ids[0].1.clone(), ids[1].1.clone())
}

/// D-107: a store that belongs to an environment is open to the roles of
/// that environment for `view`, `storage.data` and attaching a bucket to its
/// own services. A store without an owner, another environment and
/// everything under `storage.manage` behave as before.
#[tokio::test]
async fn an_owned_store_is_open_to_the_roles_of_its_environment_only() {
    let h = start().await;
    let mut admin = Client::login(&h.base, ADMIN_EMAIL, PASSWORD).await;
    let owned = managed_store_on(&h, &start_fake_s3().await, node(&h, "store-1", "linux").await).await;
    let shared = managed_store_on(&h, &start_fake_s3().await, node(&h, "store-2", "linux").await).await;
    let (uat, prod, uat_app, prod_app) = two_environments(&admin).await;
    let dev = user_with_role(&h, &admin, "dev-uat@example.com", "developer", &format!("environment:{uat}")).await;
    let prod_dev =
        user_with_role(&h, &admin, "dev-prod@example.com", "developer", &format!("environment:{prod}")).await;
    let auditor = user_with_role(&h, &admin, "aud-uat@example.com", "auditor", &format!("environment:{uat}")).await;
    let store_url = |id: forge_core::ObjectStoreId, rest: &str| format!("/api/v1/object-stores/{id}{rest}");
    let attach_url = |app: &str| format!("/api/v1/services/{app}/storage");
    let attach = |store: forge_core::ObjectStoreId, prefix: &str| json!({ "store_id": store, "bucket": "media", "create_bucket": true, "prefix": prefix });

    // Without an owner nothing changes: a role on an environment reaches no store.
    let (_, list) = dev.get("/api/v1/object-stores").await;
    assert_eq!(list.as_array().unwrap().len(), 0, "{list}");

    for path in ["", "/buckets", "/keys"] {
        assert_eq!(dev.get(&store_url(owned, path)).await.0, StatusCode::FORBIDDEN, "unowned: {path}");
    }

    assert_eq!(dev.post(&attach_url(&uat_app), attach(owned, "S3")).await.0, StatusCode::FORBIDDEN);

    step_up(&mut admin).await;
    let (s, v) = admin.put(&store_url(owned, "/owner"), json!({ "environment_id": uat })).await;
    assert_eq!(s, StatusCode::OK, "{v}");
    assert_eq!(v["environment_id"], uat.as_str(), "{v}");

    // Seen and listed through the environment; the other store stays hidden.
    let (_, list) = dev.get("/api/v1/object-stores").await;
    let ids: Vec<&str> = list.as_array().unwrap().iter().filter_map(|x| x["id"].as_str()).collect();
    assert_eq!(ids, vec![owned.to_string().as_str()], "{list}");
    let (_, list) = admin.get(&format!("/api/v1/object-stores?environment_id={uat}")).await;
    assert_eq!(list.as_array().unwrap().len(), 1, "{list}");
    let (_, list) = admin.get(&format!("/api/v1/object-stores?environment_id={prod}")).await;
    assert_eq!(list.as_array().unwrap().len(), 0, "{list}");
    let (_, list) = admin.get("/api/v1/object-stores").await;
    assert_eq!(list.as_array().unwrap().len(), 2, "{list}");

    // storage.data through the environment: buckets, objects, key list.
    assert_eq!(dev.get(&store_url(owned, "")).await.0, StatusCode::OK);
    let (s, r) = dev.post(&store_url(owned, "/buckets"), json!({ "name": "media" })).await;
    assert_eq!(s, StatusCode::CREATED, "{r}");
    let (s, b) = dev.get(&store_url(owned, "/buckets")).await;
    assert_eq!((s, &b[0]["name"]), (StatusCode::OK, &json!("media")), "{b}");
    let object = format!("{}{}", h.base, store_url(owned, "/buckets/media/object?key=a.txt"));
    let resp = dev
        .http
        .put(&object)
        .header("x-csrf-token", dev.csrf.clone().unwrap())
        .header("content-type", "text/plain")
        .body("hello")
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::CREATED, "{}", resp.text().await.unwrap());
    let (s, page) = dev.get(&store_url(owned, "/buckets/media/objects")).await;
    assert_eq!((s, &page["objects"][0]["key"]), (StatusCode::OK, &json!("a.txt")), "{page}");
    assert_eq!(dev.http.get(&object).send().await.unwrap().status(), StatusCode::OK);
    let (s, _) = dev.call(Method::DELETE, &store_url(owned, "/buckets/media/object?key=a.txt"), None).await;
    assert_eq!(s, StatusCode::NO_CONTENT);
    assert_eq!(dev.get(&store_url(owned, "/keys")).await.0, StatusCode::OK);

    // storage.manage is not carried by the environment.
    assert_eq!(dev.call(Method::DELETE, &store_url(owned, "/buckets/media"), None).await.0, StatusCode::FORBIDDEN);
    let (s, _) = dev.post(&store_url(owned, "/keys"), json!({ "name": "shop", "bucket": "media" })).await;
    assert_eq!(s, StatusCode::FORBIDDEN);
    assert_eq!(dev.post(&store_url(owned, "/actions"), json!({ "action": "restart" })).await.0, StatusCode::FORBIDDEN);
    assert_eq!(dev.put(&store_url(owned, "/owner"), json!({ "environment_id": null })).await.0, StatusCode::FORBIDDEN);

    // A read-only role on the environment sees the store but not its data.
    assert_eq!(auditor.get(&store_url(owned, "")).await.0, StatusCode::OK);
    assert_eq!(auditor.get(&store_url(owned, "/buckets")).await.0, StatusCode::FORBIDDEN);

    // The shared store and another environment's people stay outside.
    for path in ["", "/buckets"] {
        assert_eq!(dev.get(&store_url(shared, path)).await.0, StatusCode::FORBIDDEN, "shared: {path}");
        assert_eq!(prod_dev.get(&store_url(owned, path)).await.0, StatusCode::FORBIDDEN, "other environment: {path}");
    }

    let (_, list) = prod_dev.get("/api/v1/object-stores").await;
    assert_eq!(list.as_array().unwrap().len(), 0, "{list}");

    // Attaching to a service of the owning environment needs `deploy` only
    // (no MFA session here); any other pairing still needs storage.manage.
    let (s, v) = dev.post(&attach_url(&uat_app), attach(owned, "S3")).await;
    assert_eq!(s, StatusCode::CREATED, "{v}");
    let access_key = v["attachments"][0]["access_key"].as_str().unwrap().to_owned();
    assert_eq!(dev.post(&attach_url(&uat_app), attach(shared, "SHARED")).await.0, StatusCode::FORBIDDEN);
    assert_eq!(prod_dev.post(&attach_url(&prod_app), attach(owned, "S3")).await.0, StatusCode::FORBIDDEN);
    assert_eq!(
        dev.post(&attach_url(&prod_app), attach(owned, "S3")).await.0,
        StatusCode::FORBIDDEN,
        "no deploy on prod"
    );
    let (s, v) = admin.post(&attach_url(&prod_app), attach(owned, "S3")).await;
    assert_eq!(s, StatusCode::CREATED, "storage.manage still attaches across environments: {v}");
    let prod_key = v["attachments"][0]["access_key"].as_str().unwrap().to_owned();
    let (s, v) = dev.call(Method::DELETE, &format!("{}/{access_key}", attach_url(&uat_app)), None).await;
    assert_eq!(s, StatusCode::OK, "{v}");

    // `deploy` on the service's own environment is enough to detach, even
    // from a store another environment owns.
    let detach_prod = format!("{}/{prod_key}", attach_url(&prod_app));
    assert_eq!(dev.call(Method::DELETE, &detach_prod, None).await.0, StatusCode::FORBIDDEN, "no deploy on prod");
    assert_eq!(prod_dev.call(Method::DELETE, &detach_prod, None).await.0, StatusCode::OK);

    // Clearing the owner closes the store to the environment again.
    let (s, v) = admin.put(&store_url(owned, "/owner"), json!({ "environment_id": null })).await;
    assert_eq!(s, StatusCode::OK, "{v}");
    assert!(v["environment_id"].is_null(), "{v}");
    assert_eq!(dev.get(&store_url(owned, "/buckets")).await.0, StatusCode::FORBIDDEN);
    assert_eq!(dev.post(&attach_url(&uat_app), attach(owned, "S3")).await.0, StatusCode::FORBIDDEN);
}

/// D-107: assigning a store needs `storage.manage` (MFA) on the store and
/// `deploy` on the environment; it is audited; an environment that owns a
/// store is not deleted.
#[tokio::test]
async fn assigning_an_owner_is_guarded_audited_and_blocks_deleting_the_environment() {
    let h = start().await;
    let mut admin = Client::login(&h.base, ADMIN_EMAIL, PASSWORD).await;
    let (_, infra) = admin.post("/api/v1/clusters", json!({ "name": "infra" })).await;
    let infra_id = infra["id"].as_str().unwrap().to_owned();
    let srv = node_in(&h, "store-1", "linux", Some(infra_id.parse().unwrap())).await;
    let store = managed_store_on(&h, &start_fake_s3().await, srv).await;
    let (uat, _prod, _, _) = two_environments(&admin).await;
    let owner_url = format!("/api/v1/object-stores/{store}/owner");
    let to_uat = json!({ "environment_id": uat });

    assert_eq!(admin.put(&owner_url, to_uat.clone()).await.0, StatusCode::FORBIDDEN, "storage.manage needs MFA");
    step_up(&mut admin).await;
    let unknown = json!({ "environment_id": forge_core::EnvironmentId::new() });
    assert_eq!(admin.put(&owner_url, unknown).await.0, StatusCode::NOT_FOUND);

    // storage.manage on the store's node, but no `deploy` on the environment.
    let mut node_op =
        user_with_role(&h, &admin, "op-infra@example.com", "operator", &format!("cluster:{infra_id}")).await;
    step_up(&mut node_op).await;
    assert_eq!(node_op.put(&owner_url, to_uat.clone()).await.0, StatusCode::FORBIDDEN, "no deploy on uat");

    // `deploy` on the environment, but no storage.manage on the store.
    let mut dev = user_with_role(&h, &admin, "dev-uat@example.com", "developer", &format!("environment:{uat}")).await;
    step_up(&mut dev).await;
    assert_eq!(dev.put(&owner_url, to_uat.clone()).await.0, StatusCode::FORBIDDEN, "no storage.manage");
    let (_, v) = admin.get(&format!("/api/v1/object-stores/{store}")).await;
    assert!(v["environment_id"].is_null(), "refused requests change nothing: {v}");

    // Installing into an environment checks the environment the same way.
    let install = json!({ "name": "Other", "slug": "other", "server_id": srv, "data_path": "/srv/objstore/other",
                          "s3_port": 9400, "environment_id": uat });
    assert_eq!(node_op.post("/api/v1/object-stores", install.clone()).await.0, StatusCode::FORBIDDEN);
    let (s, r) = admin.post("/api/v1/object-stores", install).await;
    assert_eq!(s, StatusCode::ACCEPTED, "{r}");
    assert_eq!(r["store"]["environment_id"], uat.as_str(), "{r}");

    let (s, v) = admin.put(&owner_url, to_uat).await;
    assert_eq!(s, StatusCode::OK, "{v}");

    // Clearing needs storage.manage only.
    let (s, v) = node_op.put(&owner_url, json!({ "environment_id": null })).await;
    assert_eq!(s, StatusCode::OK, "{v}");

    // An environment that owns a store is kept.
    let (_, cl) = admin.get("/api/v1/clusters").await;
    let shop = cl.as_array().unwrap().iter().find(|c| c["name"] == "shop").unwrap()["id"].clone();
    let (_, groups) = admin.get("/api/v1/projects").await;
    let web = groups.as_array().unwrap().iter().find(|g| g["name"] == "web").unwrap()["id"].clone();
    let (s, tmp) =
        admin.post("/api/v1/environments", json!({ "name": "tmp", "cluster_id": shop, "project_id": web })).await;
    assert_eq!(s, StatusCode::CREATED, "{tmp}");
    let tmp_url = format!("/api/v1/environments/{}", tmp["id"].as_str().unwrap());
    assert_eq!(admin.put(&owner_url, json!({ "environment_id": tmp["id"] })).await.0, StatusCode::OK);
    let (s, e) = admin.call(Method::DELETE, &tmp_url, None).await;
    assert_eq!(s, StatusCode::CONFLICT, "{e}");
    assert!(e["detail"].as_str().unwrap().contains("object store"), "{e}");
    assert_eq!(admin.put(&owner_url, json!({ "environment_id": null })).await.0, StatusCode::OK);
    assert_eq!(admin.call(Method::DELETE, &tmp_url, None).await.0, StatusCode::NO_CONTENT);

    let (_, audit) = admin.get("/api/v1/audit?limit=200").await;
    let events = audit["events"].as_array().or_else(|| audit.as_array()).unwrap();
    let sets: Vec<&serde_json::Value> = events.iter().filter(|e| e["action"] == "storage.owner.set").collect();
    let done = sets.iter().filter(|e| e["outcome"] != "denied").count();
    assert_eq!(done, 4, "assign, clear, assign, clear: {sets:?}");
    assert!(sets.iter().any(|e| e.to_string().contains(&uat)), "the audit event names the environment");
}

/// D-107: owning a store opens every bucket of it to the environment, so a
/// bucket another environment's service uses is not attached on the `deploy`
/// shortcut, a store with such buckets is assigned only with `force`, and a
/// service's own attachment can always be detached with `deploy`.
#[tokio::test]
async fn ownership_does_not_quietly_open_buckets_of_other_environments() {
    let h = start().await;
    let mut admin = Client::login(&h.base, ADMIN_EMAIL, PASSWORD).await;
    let store = managed_store_on(&h, &start_fake_s3().await, node(&h, "store-1", "linux").await).await;
    let (uat, prod, uat_app, prod_app) = two_environments(&admin).await;
    let dev = user_with_role(&h, &admin, "dev-uat@example.com", "developer", &format!("environment:{uat}")).await;
    let owner_url = format!("/api/v1/object-stores/{store}/owner");
    let attach_url = |app: &str| format!("/api/v1/services/{app}/storage");
    let attach = |bucket: &str, prefix: &str| json!({ "store_id": store, "bucket": bucket, "create_bucket": true, "prefix": prefix });
    step_up(&mut admin).await;

    // A shared store with one bucket attached in each environment.
    let (s, v) = admin.post(&attach_url(&prod_app), attach("media", "S3")).await;
    assert_eq!(s, StatusCode::CREATED, "{v}");
    let (s, v) = admin.post(&attach_url(&uat_app), attach("assets", "ASSETS")).await;
    assert_eq!(s, StatusCode::CREATED, "{v}");
    let uat_key = v["attachments"][0]["access_key"].as_str().unwrap().to_owned();

    // Detaching your own service's bucket needs `deploy` only, on any store.
    let (s, v) = dev.call(Method::DELETE, &format!("{}/{uat_key}", attach_url(&uat_app)), None).await;
    assert_eq!(s, StatusCode::OK, "{v}");
    assert!(v["attachments"].as_array().unwrap().is_empty(), "{v}");
    let (_, keys) = admin.get(&format!("/api/v1/object-stores/{store}/keys")).await;
    assert!(keys.as_array().unwrap().iter().all(|k| k["access_key"] != uat_key.as_str()), "revoked: {keys}");

    // Assigning the store would open prod's bucket to uat: refused without force.
    let (s, e) = admin.put(&owner_url, json!({ "environment_id": uat })).await;
    assert_eq!(s, StatusCode::CONFLICT, "{e}");
    assert_eq!(e["title"], "attached_elsewhere", "{e}");
    assert!(e["detail"].as_str().unwrap().contains("shop-prod (media)"), "{e}");
    let (_, v) = admin.get(&format!("/api/v1/object-stores/{store}")).await;
    assert!(v["environment_id"].is_null(), "{v}");
    let (s, v) = admin.put(&owner_url, json!({ "environment_id": uat, "force": true })).await;
    assert_eq!(s, StatusCode::OK, "{v}");
    let (_, audit) = admin.get("/api/v1/audit?limit=200").await;
    let events = audit["events"].as_array().or_else(|| audit.as_array()).unwrap();
    let forced = events
        .iter()
        .find(|e| e["action"] == "storage.owner.set" && e["outcome"] != "denied")
        .map(ToString::to_string)
        .unwrap();
    assert!(forced.contains("\"forced\":true") && forced.contains(&prod_app) && forced.contains("media"), "{forced}");

    // On the shortcut, the bucket prod uses is refused; another bucket is fine.
    let (s, e) = dev.post(&attach_url(&uat_app), attach("media", "S3")).await;
    assert_eq!(s, StatusCode::CONFLICT, "{e}");
    assert_eq!(e["title"], "bucket_attached_elsewhere", "{e}");
    let (s, v) = dev.post(&attach_url(&uat_app), attach("uat-data", "DATA")).await;
    assert_eq!(s, StatusCode::CREATED, "{v}");

    // With storage.manage the shared bucket can still be attached.
    let (s, v) = admin.post(&attach_url(&uat_app), attach("media", "S3")).await;
    assert_eq!(s, StatusCode::CREATED, "{v}");

    // Moving it on meets the same check; clearing the owner never does.
    let (s, e) = admin.put(&owner_url, json!({ "environment_id": prod })).await;
    assert_eq!((s, &e["title"]), (StatusCode::CONFLICT, &json!("attached_elsewhere")), "{e}");
    assert_eq!(admin.put(&owner_url, json!({ "environment_id": null })).await.0, StatusCode::OK);
}

/// D-107: a caller who reaches a store only through its environment gets the
/// reduced view and, of its keys, only those of that environment's services.
/// A refused request on an owned store names the store in the audit log.
#[tokio::test]
async fn the_owner_path_sees_a_reduced_store_and_only_its_own_keys() {
    let h = start().await;
    let mut admin = Client::login(&h.base, ADMIN_EMAIL, PASSWORD).await;
    let store = managed_store_on(&h, &start_fake_s3().await, node(&h, "store-1", "linux").await).await;
    let (uat, prod, uat_app, prod_app) = two_environments(&admin).await;
    let dev = user_with_role(&h, &admin, "dev-uat@example.com", "developer", &format!("environment:{uat}")).await;
    let auditor = user_with_role(&h, &admin, "aud-uat@example.com", "auditor", &format!("environment:{uat}")).await;
    let prod_dev =
        user_with_role(&h, &admin, "dev-prod@example.com", "developer", &format!("environment:{prod}")).await;
    let base = format!("/api/v1/object-stores/{store}");
    let attach_url = |app: &str| format!("/api/v1/services/{app}/storage");
    step_up(&mut admin).await;
    assert_eq!(admin.put(&format!("{base}/owner"), json!({ "environment_id": uat })).await.0, StatusCode::OK);

    // Three keys: a standalone one, one of a prod service, one of a uat service.
    assert_eq!(admin.post(&format!("{base}/buckets"), json!({ "name": "media" })).await.0, StatusCode::CREATED);
    let (s, k) = admin.post(&format!("{base}/keys"), json!({ "name": "standalone", "bucket": "media" })).await;
    assert_eq!(s, StatusCode::CREATED, "{k}");
    let (s, v) = admin.post(&attach_url(&prod_app), json!({ "store_id": store, "bucket": "media" })).await;
    assert_eq!(s, StatusCode::CREATED, "{v}");
    let body = json!({ "store_id": store, "bucket": "uat-data", "create_bucket": true });
    let (s, v) = dev.post(&attach_url(&uat_app), body).await;
    assert_eq!(s, StatusCode::CREATED, "{v}");
    let uat_key = v["attachments"][0]["access_key"].as_str().unwrap().to_owned();

    // The reduced view: no credential, paths, ports, jobs or errors.
    let (s, one) = auditor.get(&base).await;
    assert_eq!(s, StatusCode::OK, "{one}");
    let (_, list) = auditor.get("/api/v1/object-stores").await;

    for v in [&one, &list[0]] {
        assert_eq!(v["limited"], true, "{v}");
        assert_eq!((&v["name"], &v["environment_id"]), (&json!("Media"), &json!(uat)), "{v}");
        assert_eq!(v["members"], json!([{ "hostname": "store-1", "status": "running" }]), "{v}");
        let fields = v.as_object().unwrap();

        for hidden in ["credential_id", "s3_port", "console_port", "last_error", "last_job_id", "version", "runtime"] {
            assert!(!fields.contains_key(hidden), "{hidden} is not for the owner path: {v}");
        }
    }

    assert!(!one.to_string().contains("/srv/objstore"), "no data paths: {one}");

    // On the node or workspace rule the view is complete.
    let (_, full) = admin.get(&base).await;
    assert_eq!(full["limited"], false, "{full}");
    assert!(full["credential_id"].is_string(), "{full}");
    assert_eq!(full["members"][0]["data_paths"], json!(["/srv/objstore/media"]), "{full}");

    // Keys: the environment's own only; everything on the node rule.
    let (s, keys) = dev.get(&format!("{base}/keys")).await;
    assert_eq!(s, StatusCode::OK, "{keys}");
    let seen: Vec<&str> = keys.as_array().unwrap().iter().filter_map(|k| k["access_key"].as_str()).collect();
    assert_eq!(seen, vec![uat_key.as_str()], "{keys}");
    let (_, keys) = admin.get(&format!("{base}/keys")).await;
    assert_eq!(keys.as_array().unwrap().len(), 3, "{keys}");

    // A refusal on an owned store says which store and whose it is.
    assert_eq!(prod_dev.get(&format!("{base}/buckets")).await.0, StatusCode::FORBIDDEN);
    let (_, audit) = admin.get("/api/v1/audit?limit=200").await;
    let events = audit["events"].as_array().or_else(|| audit.as_array()).unwrap();
    let denied = events.iter().find(|e| e["action"] == "storage.data" && e["outcome"] == "denied").unwrap().to_string();
    assert!(denied.contains(&store.to_string()) && denied.contains(&uat), "{denied}");
}

/// Lab finding (#1): a store's address defaulted to the node's host name,
/// which nothing could resolve, so a distributed store never formed and the
/// control plane could not reach it. The default is the address the node's
/// agent connects from; a host given explicitly is kept.
#[tokio::test]
async fn a_store_is_addressed_by_the_nodes_address_not_its_host_name() {
    use forge_core::tags::FactsRepository;
    let h = start().await;
    let mut admin = Client::login(&h.base, ADMIN_EMAIL, PASSWORD).await;
    step_up(&mut admin).await;
    let a = node(&h, "store-a", "linux").await;
    let b = node(&h, "store-b", "linux").await;
    let now = h.app.state.clock.now();
    h.app.state.store.put_facts(a, &json!({ "address": "10.20.0.11" }), now).await.unwrap();
    h.app.state.store.put_facts(b, &json!({ "address": "10.20.0.12" }), now).await.unwrap();
    let (s, v) = admin
        .post(
            "/api/v1/object-stores",
            json!({ "name": "HA", "slug": "ha", "topology": "distributed", "members": [
                { "server_id": a, "data_paths": ["/srv/o/d1", "/srv/o/d2"] },
                { "server_id": b, "data_paths": ["/srv/o/d1", "/srv/o/d2"], "endpoint_host": "storage-b.example.com" }
            ] }),
        )
        .await;
    assert!(s.is_success(), "{v}");
    assert_eq!(v["store"]["endpoint"], "http://10.20.0.11:9000", "{v}");
    let hosts: Vec<&str> =
        v["store"]["members"].as_array().unwrap().iter().map(|m| m["endpoint_host"].as_str().unwrap()).collect();
    assert_eq!(hosts, ["10.20.0.11", "storage-b.example.com"], "{v}");

    // A node that never connected has no address: its host name is the fallback.
    let c = node(&h, "store-c", "linux").await;
    let (s, v) = admin
        .post(
            "/api/v1/object-stores",
            json!({ "name": "Solo", "slug": "solo", "topology": "standalone", "server_id": c, "data_path": "/srv/o/solo" }),
        )
        .await;
    assert!(s.is_success(), "{v}");
    assert_eq!(v["store"]["endpoint"], "http://store-c:9000", "{v}");
}

/// Uploads a small object as `c`; the status of the answer.
async fn put_object(c: &Client, base: &str, path: &str) -> StatusCode {
    let mut req = c.http.put(format!("{base}{path}")).header("content-type", "text/plain").body("hello");

    if let Some(csrf) = &c.csrf {
        req = req.header("x-csrf-token", csrf);
    }

    if let Some(b) = &c.bearer {
        req = req.bearer_auth(b);
    }

    req.send().await.unwrap().status()
}

/// What a person without a role and without a live grant gets: nothing.
async fn assert_closed(c: &Client, base: &str, store: forge_core::ObjectStoreId) {
    let url = |rest: &str| format!("/api/v1/object-stores/{store}{rest}");

    for p in ["", "/buckets", "/buckets/media/objects", "/keys", "/grants"].map(url) {
        assert_eq!(c.get(&p).await.0, StatusCode::FORBIDDEN, "GET {p}");
    }

    let (object, bucket) = (url("/buckets/media/object?key=a.txt"), url("/buckets/media"));
    assert_eq!(c.get(&object).await.0, StatusCode::FORBIDDEN, "download");
    assert_eq!(put_object(c, base, &object).await, StatusCode::FORBIDDEN, "upload");
    assert_eq!(c.call(Method::DELETE, &object, None).await.0, StatusCode::FORBIDDEN, "delete object");
    assert_eq!(c.call(Method::DELETE, &bucket, None).await.0, StatusCode::FORBIDDEN, "delete bucket");
    let (_, list) = c.get("/api/v1/object-stores").await;
    assert_eq!(list.as_array().unwrap().len(), 0, "{list}");
    let (s, mine) = c.get("/api/v1/object-stores/grants/mine").await;
    assert_eq!((s, mine["grants"].as_array().unwrap().len()), (StatusCode::OK, 0), "{mine}");
}

/// D-108: a bucket grant opens one bucket of a store to one person who has
/// no role: `read` lists and downloads, `readwrite` also uploads and deletes.
/// Nothing else of the store opens, a revocation or an expiry holds from the
/// next request, granting needs `storage.manage` with MFA, and every step is
/// audited with the id of the grant.
#[tokio::test]
async fn a_bucket_grant_opens_one_bucket_to_one_person_and_nothing_else() {
    let h = start().await;
    let mut admin = Client::login(&h.base, ADMIN_EMAIL, PASSWORD).await;
    let store = managed_store_on(&h, &start_fake_s3().await, node(&h, "store-1", "linux").await).await;
    let other_store = managed_store_on(&h, &start_fake_s3().await, node(&h, "store-2", "linux").await).await;
    let url = |rest: &str| format!("/api/v1/object-stores/{store}{rest}");
    let guest_account =
        h.app.state.identity.create_account(None, Some("guest@example.com"), "Guest", PASSWORD).await.unwrap();
    let guest = Client::login(&h.base, "guest@example.com", PASSWORD).await;
    let dev = user_with_role(&h, &admin, "dev@example.com", "developer", &format!("workspace:{}", h.default_org)).await;

    for bucket in ["media", "private"] {
        let (s, r) = admin.post(&url("/buckets"), json!({ "name": bucket })).await;
        assert_eq!(s, StatusCode::CREATED, "{r}");
        let object = url(&format!("/buckets/{bucket}/object?key=a.txt"));
        assert_eq!(put_object(&admin, &h.base, &object).await, StatusCode::CREATED);
    }

    let (s, r) = admin.post(&format!("/api/v1/object-stores/{other_store}/buckets"), json!({ "name": "media" })).await;
    assert_eq!(s, StatusCode::CREATED, "{r}");
    assert_closed(&guest, &h.base, store).await;

    // Granting needs storage.manage on the store, with an MFA session.
    let ask = |user: &str, bucket: &str, access: &str| json!({ "user": user, "bucket": bucket, "access": access });
    let (s, r) = admin.post(&url("/grants"), ask("guest@example.com", "media", "read")).await;
    assert_eq!(s, StatusCode::FORBIDDEN, "no MFA session yet: {r}");
    assert_eq!(dev.post(&url("/grants"), ask("guest@example.com", "media", "read")).await.0, StatusCode::FORBIDDEN);
    assert_eq!(dev.get(&url("/grants")).await.0, StatusCode::FORBIDDEN);
    step_up(&mut admin).await;
    assert_eq!(dev.post(&url("/grants"), ask("guest@example.com", "media", "read")).await.0, StatusCode::FORBIDDEN);

    // The bucket and the person must exist; the request is validated.
    let (s, r) = admin.post(&url("/grants"), ask("guest@example.com", "nope", "read")).await;
    assert_eq!(s, StatusCode::NOT_FOUND, "{r}");
    let (s, r) = admin.post(&url("/grants"), ask("nobody@example.com", "media", "read")).await;
    assert_eq!(s, StatusCode::NOT_FOUND, "{r}");
    assert_eq!(r["detail"], "unknown or disabled user", "one answer, whether the account exists or not: {r}");
    let (s, r) = admin.post(&url("/grants"), ask("guest@example.com", "media", "admin")).await;
    assert!(s.is_client_error() && s != StatusCode::FORBIDDEN, "unknown access: {s} {r}");
    let both = json!({ "user": "guest@example.com", "bucket": "media", "lifetime_minutes": 5, "expires_at": 1 });
    assert_eq!(admin.post(&url("/grants"), both).await.0, StatusCode::BAD_REQUEST);
    let past = json!({ "user": "guest@example.com", "bucket": "media", "expires_at": 1 });
    assert_eq!(admin.post(&url("/grants"), past).await.0, StatusCode::UNPROCESSABLE_ENTITY);
    assert_closed(&guest, &h.base, store).await;

    let note = json!({ "user": "guest@example.com", "bucket": "media", "note": "press photos" });
    let (s, read) = admin.post(&url("/grants"), note).await;
    assert_eq!(s, StatusCode::CREATED, "{read}");
    let read_id = read["id"].as_str().unwrap().to_owned();
    assert_eq!(
        (&read["access"], &read["status"], &read["never_expires"], &read["expires_at"]),
        (&json!("read"), &json!("active"), &json!(true), &json!(null)),
        "read is the default access and a grant has no end unless one is given: {read}"
    );
    assert_eq!(
        (&read["user"], &read["bucket"], &read["note"]),
        (&json!("guest@example.com"), &json!("media"), &json!("press photos"))
    );
    assert_eq!(read["created_by_name"], ADMIN_EMAIL, "{read}");
    let (s, r) = admin.post(&url("/grants"), ask("guest@example.com", "media", "readwrite")).await;
    assert_eq!(s, StatusCode::CONFLICT, "one live grant per person and bucket: {r}");

    // The holder sees the store reduced, and of its buckets the granted one.
    let (_, list) = guest.get("/api/v1/object-stores").await;
    let rows = list.as_array().unwrap();
    assert_eq!(rows.len(), 1, "the other store stays hidden: {list}");
    let (s, one) = guest.get(&url("")).await;
    assert_eq!(s, StatusCode::OK, "{one}");

    for v in [&rows[0], &one] {
        assert_eq!((&v["limited"], &v["by_grant"], &v["name"]), (&json!(true), &json!(true), &json!("Media")), "{v}");
        assert_eq!(
            (&v["endpoint"], &v["members"], &v["environment_id"]),
            (&json!(""), &json!([]), &json!(null)),
            "{v}"
        );

        for hidden in ["credential_id", "s3_port", "region", "last_job_id", "version"] {
            assert!(v.get(hidden).is_none(), "{hidden} is not shown: {v}");
        }
    }

    let (s, b) = guest.get(&url("/buckets")).await;
    assert_eq!(s, StatusCode::OK, "{b}");
    assert_eq!(b.as_array().unwrap().iter().map(|x| &x["name"]).collect::<Vec<_>>(), [&json!("media")], "{b}");

    // The holder's own list: what opens and until when, not who gave it or why.
    let (s, mine) = guest.get("/api/v1/object-stores/grants/mine").await;
    assert_eq!(s, StatusCode::OK, "{mine}");
    assert_eq!(
        mine["grants"],
        json!([{ "id": read["id"], "store_id": store, "store": "Media", "bucket": "media", "access": "read",
                 "expires_at": null, "status": "active" }]),
        "no note, no issuer"
    );
    assert!(!mine.to_string().contains("press photos") && !mine.to_string().contains(ADMIN_EMAIL), "{mine}");

    // The owner of the store stays hidden: the reduced view leaves it out,
    // and a list by environment does not confirm a guess.
    let (uat, _, _, _) = two_environments(&admin).await;
    let (s, v) = admin.put(&url("/owner"), json!({ "environment_id": uat })).await;
    assert_eq!(s, StatusCode::OK, "{v}");
    let (s, one) = guest.get(&url("")).await;
    assert_eq!((s, &one["environment_id"], &one["by_grant"]), (StatusCode::OK, &json!(null), &json!(true)), "{one}");
    let (_, list) = guest.get(&format!("/api/v1/object-stores?environment_id={uat}")).await;
    assert_eq!(list.as_array().unwrap().len(), 0, "{list}");
    let (_, list) = guest.get("/api/v1/object-stores").await;
    assert_eq!(list.as_array().unwrap().len(), 1, "{list}");
    let (s, v) = admin.put(&url("/owner"), json!({ "environment_id": null })).await;
    assert_eq!(s, StatusCode::OK, "{v}");

    // read: list and download in that bucket; no upload, no delete.
    let object = url("/buckets/media/object?key=a.txt");
    let (s, page) = guest.get(&url("/buckets/media/objects")).await;
    assert_eq!((s, &page["objects"][0]["key"]), (StatusCode::OK, &json!("a.txt")), "{page}");
    let resp = guest.http.get(format!("{}{object}", h.base)).send().await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    assert_eq!(resp.text().await.unwrap(), "hello");
    assert_eq!(put_object(&guest, &h.base, &url("/buckets/media/object?key=b.txt")).await, StatusCode::FORBIDDEN);
    assert_eq!(guest.call(Method::DELETE, &object, None).await.0, StatusCode::FORBIDDEN);

    // Never through a grant: other buckets, bucket and key management,
    // the connection test, lifecycle, the owner, the grants themselves, and
    // a bucket of the same name in another store.
    assert_eq!(guest.get(&url("/buckets/private/objects")).await.0, StatusCode::FORBIDDEN);
    assert_eq!(guest.get(&url("/buckets/private/object?key=a.txt")).await.0, StatusCode::FORBIDDEN);
    assert_eq!(guest.get(&url("/buckets/Media/objects")).await.0, StatusCode::FORBIDDEN, "names are exact");
    assert_eq!(guest.post(&url("/buckets"), json!({ "name": "mine" })).await.0, StatusCode::FORBIDDEN);
    assert_eq!(guest.call(Method::DELETE, &url("/buckets/media"), None).await.0, StatusCode::FORBIDDEN);
    assert_eq!(guest.get(&url("/keys")).await.0, StatusCode::FORBIDDEN);
    let key = json!({ "name": "shop", "bucket": "media" });
    assert_eq!(guest.post(&url("/keys"), key).await.0, StatusCode::FORBIDDEN);
    assert_eq!(guest.post(&url("/test"), json!({})).await.0, StatusCode::FORBIDDEN);
    assert_eq!(guest.post(&url("/actions"), json!({ "action": "restart" })).await.0, StatusCode::FORBIDDEN);
    assert_eq!(guest.put(&url("/owner"), json!({ "environment_id": null })).await.0, StatusCode::FORBIDDEN);
    assert_eq!(guest.get(&url("/grants")).await.0, StatusCode::FORBIDDEN);
    assert_eq!(guest.post(&url("/grants"), ask("dev@example.com", "media", "read")).await.0, StatusCode::FORBIDDEN);
    let revoke_read = url(&format!("/grants/{read_id}"));
    assert_eq!(guest.call(Method::DELETE, &revoke_read, None).await.0, StatusCode::FORBIDDEN);
    let elsewhere = format!("/api/v1/object-stores/{other_store}");
    assert_eq!(guest.get(&elsewhere).await.0, StatusCode::FORBIDDEN);
    assert_eq!(guest.get(&format!("{elsewhere}/buckets/media/objects")).await.0, StatusCode::FORBIDDEN);

    // The list of grants of the store; a grant is revoked on its own store only.
    let (s, listed) = admin.get(&url("/grants")).await;
    assert_eq!((s, listed["grants"].as_array().unwrap().len()), (StatusCode::OK, 1), "{listed}");
    let wrong_store = format!("{elsewhere}/grants/{read_id}");
    assert_eq!(admin.call(Method::DELETE, &wrong_store, None).await.0, StatusCode::NOT_FOUND);
    assert_eq!(guest.get(&url("/buckets/media/objects")).await.0, StatusCode::OK);

    // Revoked: refused with the next request.
    assert_eq!(admin.call(Method::DELETE, &revoke_read, None).await.0, StatusCode::NO_CONTENT);
    assert_closed(&guest, &h.base, store).await;
    let (_, listed) = admin.get(&url("/grants")).await;
    assert_eq!(listed["grants"].as_array().unwrap().len(), 0, "{listed}");
    let (_, listed) = admin.get(&url("/grants?all=true")).await;
    assert_eq!(listed["grants"][0]["status"], "revoked", "{listed}");

    // readwrite, named by user id, with an end: upload and delete too.
    let rw = json!({ "user_id": guest_account.id, "bucket": "media", "access": "readwrite", "lifetime_minutes": 60 });
    let (s, rw) = admin.post(&url("/grants"), rw).await;
    assert_eq!(s, StatusCode::CREATED, "{rw}");
    assert_eq!((&rw["never_expires"], rw["expires_at"].is_i64()), (&json!(false), true), "{rw}");
    assert_eq!(put_object(&guest, &h.base, &url("/buckets/media/object?key=b.txt")).await, StatusCode::CREATED);
    assert_eq!(guest.get(&url("/buckets/media/object?key=b.txt")).await.0, StatusCode::OK);
    let (s, _) = guest.call(Method::DELETE, &url("/buckets/media/object?key=b.txt"), None).await;
    assert_eq!(s, StatusCode::NO_CONTENT);
    assert_eq!(put_object(&guest, &h.base, &url("/buckets/private/object?key=b.txt")).await, StatusCode::FORBIDDEN);
    assert_eq!(guest.call(Method::DELETE, &url("/buckets/media"), None).await.0, StatusCode::FORBIDDEN);

    // An API key acts as the account that holds the grant.
    let (s, key) = guest.post("/api/v1/api-keys", json!({ "name": "script" })).await;
    assert!(s.is_success(), "{key}");
    let mut with_key = Client::anonymous(&h.base);
    with_key.bearer = key["secret"].as_str().map(str::to_owned);
    assert!(with_key.bearer.is_some(), "{key}");
    assert_eq!(with_key.get(&url("/buckets/media/objects")).await.0, StatusCode::OK);
    assert_eq!(with_key.get(&url("/buckets/private/objects")).await.0, StatusCode::FORBIDDEN);

    // Expired: refused with the next request, by the clock, without a revoke.
    sqlx::query("UPDATE bucket_grants SET expires_at = $1 WHERE id = $2")
        .bind(h.app.state.clock.now().0)
        .bind(rw["id"].as_str().unwrap())
        .execute(h.app.state.store.pool())
        .await
        .unwrap();
    assert_closed(&guest, &h.base, store).await;
    assert_eq!(with_key.get(&url("/buckets/media/objects")).await.0, StatusCode::FORBIDDEN);
    let (_, listed) = admin.get(&url("/grants?all=true")).await;
    let statuses: Vec<&str> =
        listed["grants"].as_array().unwrap().iter().filter_map(|g| g["status"].as_str()).collect();
    assert_eq!(statuses, ["expired", "revoked"], "newest first: {listed}");

    // The audit log: who was given what, and what was done through which grant.
    let (_, audit) = admin.get("/api/v1/audit?limit=500").await;
    let events = audit["events"].as_array().or_else(|| audit.as_array()).unwrap();
    let of = |action: &str| events.iter().filter(|e| e["action"] == action).collect::<Vec<_>>();
    let ok = |e: &&serde_json::Value| e["outcome"] == "success";
    let created: Vec<_> = of("storage.grant.create").into_iter().filter(ok).collect();
    assert_eq!(created.len(), 2, "{created:?}");
    assert!(created.iter().any(|e| e["details"]["grant_id"] == read["id"]
        && e["details"]["user"] == "guest@example.com"
        && e["details"]["bucket"] == "media"
        && e["details"]["access"] == "read"));
    let revoked: Vec<_> = of("storage.grant.revoke").into_iter().filter(ok).collect();
    assert_eq!(revoked.len(), 1, "{revoked:?}");
    assert_eq!(revoked[0]["details"]["grant_id"], read["id"]);
    let denied_manage = of("storage.manage").into_iter().filter(|e| e["outcome"] == "denied").count();
    assert!(denied_manage >= 4, "refused attempts to grant are audited: {denied_manage}");

    // A grant that was allowed but could not be issued is on record too,
    // with the person and the bucket that were named.
    let failed: Vec<_> = of("storage.grant.create").into_iter().filter(|e| e["outcome"] == "failure").collect();
    assert_eq!(failed.len(), 5, "{failed:?}");
    assert!(failed.iter().any(|e| e["details"]["user"] == "nobody@example.com"
        && e["details"]["bucket"] == "media"
        && e["details"]["error"] == "not_found (404)"));
    assert!(failed.iter().any(|e| e["details"]["bucket"] == "nope"));
    let through = |action: &str, grant: &serde_json::Value| {
        of(action).into_iter().filter(|e| e["details"]["grant_id"] == grant["id"]).count()
    };
    assert_eq!(through("storage.object.download", &read), 1, "the download through the read grant");
    assert_eq!(through("storage.object.download", &rw), 1);
    assert_eq!(through("storage.object.upload", &rw), 1);
    assert_eq!(through("storage.object.delete", &rw), 1);
    let by_role = of("storage.object.upload").into_iter().filter(|e| e["details"]["grant_id"].is_null()).count();
    assert_eq!(by_role, 2, "the uploads of the administrator name no grant");
    assert!(
        of("storage.data").iter().any(|e| e["outcome"] == "denied"
            && e["details"]["bucket"] == "private"
            && e["details"]["access"] == "readwrite"),
        "a refused request names the bucket and the access it needed"
    );
}

/// D-108: the grants of a store go with it, so a store that is connected
/// again later does not come back with the access it had.
#[tokio::test]
async fn removing_a_store_removes_its_bucket_grants() {
    use forge_core::storage::BucketGrantRepository;
    let h = start().await;
    let mut admin = Client::login(&h.base, ADMIN_EMAIL, PASSWORD).await;
    step_up(&mut admin).await;
    let endpoint = start_fake_s3().await;
    let connect = json!({"name": "Cloud", "provider": "s3", "endpoint": endpoint, "region": "us-east-1",
                         "path_style": true, "access_key": "AKTEST", "secret_key": "secret-1"});
    let (s, store) = admin.post("/api/v1/object-stores/connect", connect).await;
    assert_eq!(s, StatusCode::CREATED, "{store}");
    let base = format!("/api/v1/object-stores/{}", store["id"].as_str().unwrap());
    let guest = h.app.state.identity.create_account(None, Some("guest@example.com"), "Guest", PASSWORD).await.unwrap();
    assert_eq!(admin.post(&format!("{base}/buckets"), json!({ "name": "media" })).await.0, StatusCode::CREATED);
    let (s, g) = admin.post(&format!("{base}/grants"), json!({ "user": "guest@example.com", "bucket": "media" })).await;
    assert_eq!(s, StatusCode::CREATED, "a connected store takes grants too: {g}");
    let now = h.app.state.clock.now();
    assert_eq!(h.app.state.store.list_live_bucket_grants(guest.id, now).await.unwrap().len(), 1);
    let (s, r) = admin.post(&format!("{base}/actions"), json!({ "action": "disconnect" })).await;
    assert_eq!(s, StatusCode::OK, "{r}");
    assert!(h.app.state.store.list_live_bucket_grants(guest.id, now).await.unwrap().is_empty());
    let guest = Client::login(&h.base, "guest@example.com", PASSWORD).await;
    let (_, mine) = guest.get("/api/v1/object-stores/grants/mine").await;
    assert_eq!(mine["grants"].as_array().unwrap().len(), 0, "{mine}");
}

/// D-108: the grants of a bucket end with the bucket. Deleting it revokes
/// them, audited with the reason, and a bucket that later gets the same name
/// is not open to the old holder, also when the old bucket was deleted
/// outside Forge.
#[tokio::test]
async fn deleting_a_bucket_revokes_its_grants_and_a_new_bucket_of_that_name_starts_closed() {
    let h = start().await;
    let mut admin = Client::login(&h.base, ADMIN_EMAIL, PASSWORD).await;
    step_up(&mut admin).await;
    let s3 = start_fake_s3().await;
    let store = managed_store_on(&h, &s3, node(&h, "store-1", "linux").await).await;
    let url = |rest: &str| format!("/api/v1/object-stores/{store}{rest}");
    h.app.state.identity.create_account(None, Some("guest@example.com"), "Guest", PASSWORD).await.unwrap();
    let guest = Client::login(&h.base, "guest@example.com", PASSWORD).await;
    let grant = |bucket: &str| json!({ "user": "guest@example.com", "bucket": bucket, "access": "readwrite" });

    for bucket in ["media", "logs"] {
        assert_eq!(admin.post(&url("/buckets"), json!({ "name": bucket })).await.0, StatusCode::CREATED);
        assert_eq!(admin.post(&url("/grants"), grant(bucket)).await.0, StatusCode::CREATED);
    }

    // The holder's API key works like their session (and stops like it).
    let (_, key) = guest.post("/api/v1/api-keys", json!({ "name": "script" })).await;
    let mut with_key = Client::anonymous(&h.base);
    with_key.bearer = key["secret"].as_str().map(str::to_owned);
    assert_eq!(guest.get(&url("/buckets/media/objects")).await.0, StatusCode::OK);
    assert_eq!(with_key.get(&url("/buckets/media/objects")).await.0, StatusCode::OK);
    assert_eq!(put_object(&with_key, &h.base, &url("/buckets/logs/object?key=a.txt")).await, StatusCode::CREATED);
    let (s, _) = with_key.call(Method::DELETE, &url("/buckets/logs/object?key=a.txt"), None).await;
    assert_eq!(s, StatusCode::NO_CONTENT);
    assert_eq!(with_key.get(&url("/keys")).await.0, StatusCode::FORBIDDEN, "a key opens no more than the grant");

    // Deleted through Forge: the grant is revoked with the bucket.
    assert_eq!(admin.call(Method::DELETE, &url("/buckets/media"), None).await.0, StatusCode::NO_CONTENT);
    let (_, listed) = admin.get(&url("/grants")).await;
    let live: Vec<&str> = listed["grants"].as_array().unwrap().iter().filter_map(|g| g["bucket"].as_str()).collect();
    assert_eq!(live, ["logs"], "the other bucket keeps its grant: {listed}");
    let (_, mine) = guest.get("/api/v1/object-stores/grants/mine").await;
    assert_eq!(mine["grants"].as_array().unwrap().len(), 1, "{mine}");

    // The same name again is a new bucket: closed to the old holder.
    assert_eq!(admin.post(&url("/buckets"), json!({ "name": "media" })).await.0, StatusCode::CREATED);
    assert_eq!(guest.get(&url("/buckets/media/objects")).await.0, StatusCode::FORBIDDEN);
    assert_eq!(with_key.get(&url("/buckets/media/objects")).await.0, StatusCode::FORBIDDEN);
    assert_eq!(put_object(&guest, &h.base, &url("/buckets/media/object?key=a.txt")).await, StatusCode::FORBIDDEN);
    let (_, b) = guest.get(&url("/buckets")).await;
    assert_eq!(b.as_array().unwrap().iter().map(|x| &x["name"]).collect::<Vec<_>>(), [&json!("logs")], "{b}");

    // Granting again is a new decision, and nothing stands in its way.
    assert_eq!(admin.post(&url("/grants"), grant("media")).await.0, StatusCode::CREATED);
    assert_eq!(guest.get(&url("/buckets/media/objects")).await.0, StatusCode::OK);

    // Deleted outside Forge (at the store itself): the grant is still live
    // until a bucket of that name is created through Forge again.
    let gone = reqwest::Client::new()
        .delete(format!("{s3}/logs"))
        .header("authorization", "AWS4-HMAC-SHA256 Credential=AKTEST/x")
        .send()
        .await
        .unwrap();
    assert!(gone.status().is_success(), "{}", gone.status());
    assert_eq!(admin.post(&url("/buckets"), json!({ "name": "logs" })).await.0, StatusCode::CREATED);
    assert_eq!(guest.get(&url("/buckets/logs/objects")).await.0, StatusCode::FORBIDDEN);

    // Creating a bucket that is already there revokes nothing.
    let (s, _) = admin.post(&url("/buckets"), json!({ "name": "media" })).await;
    assert!(s == StatusCode::CREATED || s == StatusCode::CONFLICT, "{s}");
    assert_eq!(guest.get(&url("/buckets/media/objects")).await.0, StatusCode::OK);

    let (_, audit) = admin.get("/api/v1/audit?limit=500").await;
    let events = audit["events"].as_array().or_else(|| audit.as_array()).unwrap();
    let reasons: Vec<(&str, &str)> = events
        .iter()
        .filter(|e| e["action"] == "storage.grant.revoke")
        .filter_map(|e| Some((e["details"]["bucket"].as_str()?, e["details"]["reason"].as_str()?)))
        .collect();
    assert_eq!(reasons.len(), 2, "{reasons:?}");
    assert!(reasons.contains(&("media", "bucket deleted")), "{reasons:?}");
    assert!(reasons.contains(&("logs", "bucket recreated")), "{reasons:?}");
    assert!(
        events.iter().any(|e| e["action"] == "storage.grant.revoke" && e["details"]["user"] == "guest@example.com")
    );
}

// ------------------------------------------------------------------ D-170

/// Keys live in RustFS: Forge writes policies there, reads them back, shows
/// drift when they change in RustFS, and reconciles to RustFS's state.
#[tokio::test]
async fn keys_and_prefix_grants_are_kept_in_rustfs() {
    let h = start().await;
    let mut admin = Client::login(&h.base, ADMIN_EMAIL, PASSWORD).await;
    let srv = node(&h, "iam-1", "linux").await;
    let (endpoint, fake) = start_fake_s3_with().await;
    let id = managed_store_on(&h, &endpoint, srv).await;
    let base = format!("/api/v1/object-stores/{id}/iam");
    let grants = json!([
        {"bucket": "media", "prefix": "uploads/", "access": "readwrite"},
        {"bucket": "media", "prefix": "public/", "access": "read"}
    ]);
    let (s, _) = admin.post(&format!("{base}/keys"), json!({"name": "shop", "grants": grants})).await;
    assert_eq!(s, StatusCode::FORBIDDEN, "IAM needs MFA");
    step_up(&mut admin).await;

    for bad in [
        json!([]),
        json!([{"bucket": "media", "prefix": "/x/", "access": "read"}]),
        json!([{"bucket": "media", "prefix": "a/*", "access": "read"}]),
    ] {
        let (s, r) = admin.post(&format!("{base}/keys"), json!({"name": "shop", "grants": bad})).await;
        assert_eq!(s, StatusCode::BAD_REQUEST, "{r}");
    }

    let (s, k) = admin.post(&format!("{base}/keys"), json!({"name": "shop", "grants": grants})).await;
    assert_eq!(s, StatusCode::CREATED, "{k}");
    assert!(!k.to_string().contains("svcsecret"), "the secret stays in the vault");
    let ak = k["access_key"].as_str().unwrap().to_owned();

    // The policy RustFS holds: objects under the prefix, listing limited to it.
    let policy = fake.accounts.lock().unwrap()[&ak].1.to_string();
    assert!(policy.contains("arn:aws:s3:::media/uploads/*") && policy.contains("arn:aws:s3:::media/public/*"));
    assert!(policy.contains(r#""s3:prefix":["uploads/*"]"#), "{policy}");
    assert!(!policy.contains("arn:aws:s3:::media/*"), "{policy}");

    let (s, v) = admin.get(&base).await;
    assert_eq!(s, StatusCode::OK, "{v}");
    assert_eq!(v["reachable"], true);
    assert_eq!(v["source"], "rustfs");
    let key = |v: &serde_json::Value, ak: &str| {
        v["keys"].as_array().unwrap().iter().find(|k| k["access_key"] == ak).cloned().unwrap()
    };
    assert_eq!(key(&v, &ak)["drift"], "in_sync");
    assert_eq!(key(&v, &ak)["grants"].as_array().unwrap().len(), 2);
    assert!(v.to_string().find("AKTEST").is_none(), "the root key is not an app key");

    // Changed directly in RustFS: a wider policy, and a key Forge never made.
    fake.accounts.lock().unwrap().get_mut(&ak).unwrap().1 = json!({"Version": "2012-10-17", "Statement": [
        {"Effect": "Allow", "Action": ["s3:*"], "Resource": ["arn:aws:s3:::*"]}]});
    fake.accounts
        .lock()
        .unwrap()
        .insert("OUTSIDE1".into(), ("cli".into(), json!({"Version": "2012-10-17", "Statement": []})));
    let (_, v) = admin.get(&base).await;
    assert_eq!(key(&v, &ak)["drift"], "policy_changed");
    assert_eq!(key(&v, &ak)["custom_policy"], true);
    assert_eq!(key(&v, "OUTSIDE1")["drift"], "not_in_forge");

    // Editing writes RustFS; it works for keys Forge did not create too.
    let one = json!([{"bucket": "logs", "prefix": "", "access": "read"}]);
    let (s, r) = admin.put(&format!("{base}/keys/OUTSIDE1"), json!({"grants": one})).await;
    assert_eq!(s, StatusCode::OK, "{r}");
    assert!(fake.accounts.lock().unwrap()["OUTSIDE1"].1.to_string().contains("arn:aws:s3:::logs/*"));
    let (s, _) = admin.put(&format!("{base}/keys/AKTEST"), json!({"grants": one})).await;
    assert_eq!(s, StatusCode::CONFLICT, "never the root key");
    let (s, _) = admin.put(&format!("{base}/keys/NOSUCH1"), json!({"grants": one})).await;
    assert_eq!(s, StatusCode::NOT_FOUND);

    // Reconcile accepts RustFS: the changed policy is recorded as custom.
    fake.accounts.lock().unwrap().remove("OUTSIDE1");
    let (s, r) = admin.post(&format!("{base}/reconcile"), json!({})).await;
    assert_eq!(s, StatusCode::OK, "{r}");
    assert_eq!((r["updated"].as_u64(), r["removed"].as_u64()), (Some(1), Some(1)), "{r}");
    let (_, v) = admin.get(&base).await;
    assert!(v["keys"].as_array().unwrap().iter().all(|k| k["drift"] == "in_sync"), "{v}");

    // Developers cannot see or change IAM.
    let ws = format!("workspace:{}", h.default_org);
    let dev = user_with_role(&h, &admin, "dev-iam@example.com", "developer", &ws).await;
    assert_eq!(dev.get(&base).await.0, StatusCode::FORBIDDEN);
    assert_eq!(dev.post(&format!("{base}/reconcile"), json!({})).await.0, StatusCode::FORBIDDEN);

    let (_, audit) = admin.get("/api/v1/audit?limit=200").await;
    let text = audit.to_string();

    for a in ["storage.key.create", "storage.key.policy", "storage.iam.reconcile"] {
        assert!(text.contains(a), "{a} audited");
    }

    assert!(!text.contains("svcsecret"));
}

/// RustFS down: the list comes from Forge's records, read-only.
#[tokio::test]
async fn an_unreachable_rustfs_shows_the_cached_keys_read_only() {
    let h = start().await;
    let mut admin = Client::login(&h.base, ADMIN_EMAIL, PASSWORD).await;
    step_up(&mut admin).await;
    let srv = node(&h, "iam-2", "linux").await;
    let (endpoint, _fake) = start_fake_s3_with().await;
    let id = managed_store_on(&h, &endpoint, srv).await;
    let base = format!("/api/v1/object-stores/{id}/iam");
    let grants = json!([{"bucket": "media", "prefix": "", "access": "read"}]);
    let (s, _) = admin.post(&format!("{base}/keys"), json!({"name": "shop", "grants": grants})).await;
    assert_eq!(s, StatusCode::CREATED);

    // Point the store at a closed port.
    let st = &h.app.state.store;
    let mut store = forge_core::storage::ObjectStoreRepository::get_object_store(st, id).await.unwrap().unwrap();
    store.endpoint = "http://127.0.0.1:1".into();
    forge_core::storage::ObjectStoreRepository::put_object_store(st, &store).await.unwrap();
    let (s, v) = admin.get(&base).await;
    assert_eq!(s, StatusCode::OK, "{v}");
    assert_eq!(v["reachable"], false);
    assert_eq!(v["source"], "cache");
    assert_eq!(v["keys"][0]["drift"], "unknown");
    assert_eq!(v["keys"][0]["grants"][0]["bucket"], "media");
    let (s, r) = admin.post(&format!("{base}/keys"), json!({"name": "two", "grants": grants})).await;
    assert_eq!(s, StatusCode::SERVICE_UNAVAILABLE, "{r}");
    assert_eq!(r["title"], "store_unreachable");
}

/// Settings after install: validated, audited, and a failed recreate rolls
/// the record back with the container.
#[tokio::test]
async fn settings_recreate_the_container_and_roll_back_on_failure() {
    let h = start().await;
    let mut admin = Client::login(&h.base, ADMIN_EMAIL, PASSWORD).await;
    let srv = node(&h, "set-1", "linux").await;
    let endpoint = start_fake_s3().await;
    let id = managed_store_with(&h, &endpoint, srv, "docker").await;
    let st = &h.app.state.store;
    let mut store = forge_core::storage::ObjectStoreRepository::get_object_store(st, id).await.unwrap().unwrap();
    store.s3_port = 9000;
    store.console_port = Some(9001);
    forge_core::storage::ObjectStoreRepository::put_object_store(st, &store).await.unwrap();
    let url = format!("/api/v1/object-stores/{id}/settings");
    let (s, _) = admin.get(&url).await;
    assert_eq!(s, StatusCode::FORBIDDEN, "settings need MFA");
    step_up(&mut admin).await;
    let (s, v) = admin.get(&url).await;
    assert_eq!(s, StatusCode::OK, "{v}");
    assert_eq!(v["editable"], true);
    assert_eq!(v["docker"]["image"], "rustfs/rustfs");
    let body = |env: serde_json::Value| {
        json!({
            "version": "1.0.6", "s3_port": 9100, "console_port": null, "region": "eu-west-1",
            "docker": {"image": "rustfs/rustfs", "restart": "always", "memory_mb": 2048, "cpu_millis": 1500, "env": env}
        })
    };
    let (s, r) = admin.put(&url, body(json!({"RUSTFS_SECRET_KEY": "x"}))).await;
    assert_eq!(s, StatusCode::UNPROCESSABLE_ENTITY, "credentials cannot be overridden: {r}");
    let (s, r) = admin.put(&url, body(json!({"RUSTFS_SERVER_DOMAINS": "s3.example.test"}))).await;
    assert_eq!(s, StatusCode::ACCEPTED, "{r}");
    assert_eq!(r["jobs"][0]["payload"]["action"], "reconfigure");
    assert_eq!(r["jobs"][0]["payload"]["previous"]["s3_port"], 9000);
    let op =
        answer_with(&h, srv, false, "running", "RustFS did not answer; rolled back to the previous container").await;
    assert!(op.contains("\"op\":\"reconfigure\"") && op.contains("\"memory_mb\":2048"), "{op}");
    assert!(op.contains("RUSTFS_SERVER_DOMAINS") && !op.contains("svcsecret"), "{op}");

    // The record follows the node back to the old settings.
    let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(20);

    loop {
        let s = forge_core::storage::ObjectStoreRepository::get_object_store(st, id).await.unwrap().unwrap();
        if s.s3_port == 9000 && s.version == "latest" {
            assert_eq!(s.console_port, Some(9001));
            break;
        }

        assert!(tokio::time::Instant::now() < deadline, "not rolled back: {s:?}");
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    }

    let (_, v) = admin.get(&url).await;
    assert_eq!(v["docker"]["memory_mb"], serde_json::Value::Null, "{v}");
    let (_, audit) = admin.get("/api/v1/audit?limit=100").await;
    assert!(audit.to_string().contains("storage.settings.update"));
}

/// Plays the node's Docker for container `list` and `inspect` commands.
fn fake_docker(
    h: &crate::common::Harness,
    server: forge_core::ServerId,
    list: serde_json::Value,
    inspect: serde_json::Value,
) -> tokio::task::JoinHandle<()> {
    use forge_proto::messages::{CommandBody, CommandResult, command_body::Command};
    use prost::Message;
    let hub = h.app.state.hub.clone();

    tokio::spawn(async move {
        for _ in 0..400 {
            for cmd in hub.take(server, 4) {
                let body = CommandBody::decode(cmd.body.as_slice()).unwrap();
                let Some(Command::ContainerAction(a)) = body.command else { continue };
                let out = if a.op == "list" { list.clone() } else { inspect.clone() };
                hub.resolve(
                    server,
                    CommandResult {
                        operation_id: body.operation_id,
                        ok: true,
                        output: out.to_string(),
                        ..Default::default()
                    },
                );
            }

            tokio::time::sleep(std::time::Duration::from_millis(25)).await;
        }
    })
}

/// Any RustFS container is found and adopted as it is: name, image, ports
/// and volumes come from the node; the credential is read there.
#[tokio::test]
async fn any_rustfs_container_is_discovered_and_adopted() {
    let h = start().await;
    let mut admin = Client::login(&h.base, ADMIN_EMAIL, PASSWORD).await;
    let srv = node(&h, "found-1", "linux").await;
    let list = json!([
        {"id": "abc123", "name": "my-rustfs", "image": "rustfs/rustfs:1.0.6", "state": "running", "status": "Up",
         "ports": ["0.0.0.0:19000->9000/tcp"], "labels": {}},
        {"id": "def456", "name": "web", "image": "nginx:1", "state": "running", "status": "Up", "ports": [], "labels": {}}
    ]);
    let inspect = json!({"image": "rustfs/rustfs:1.0.6",
        "ports": {"9000/tcp": [{"HostIp": "0.0.0.0", "HostPort": "19000"}]},
        "mounts": [{"source": "/var/lib/docker/volumes/rustfs-data/_data", "destination": "/data", "mode": "z"}],
        "networks": ["bridge"], "env_keys": ["RUSTFS_ACCESS_KEY", "RUSTFS_SECRET_KEY"]});
    let docker = fake_docker(&h, srv, list, inspect);

    // Reads need server.manage; nodes the caller may not manage are skipped.
    let ws = format!("workspace:{}", h.default_org);
    let guest = user_with_role(&h, &admin, "guest-found@example.com", "auditor", &ws).await;
    let (s, r) = guest.get("/api/v1/object-stores/discover").await;
    assert!(s == StatusCode::OK && r["containers"].as_array().unwrap().is_empty() || s == StatusCode::FORBIDDEN, "{r}");
    let (s, r) = admin.get(&format!("/api/v1/object-stores/discover?server_id={srv}")).await;
    assert_eq!(s, StatusCode::OK, "{r}");
    assert_eq!(r["nodes"][0]["docker"], "available", "{r}");
    assert_eq!(r["containers"].as_array().unwrap().len(), 1, "only RustFS: {r}");
    assert_eq!(r["containers"][0]["kind"], "foreign");
    assert_eq!(r["containers"][0]["version"], "1.0.6");
    assert_eq!(r["containers"][0]["mounts"][0]["name"], "rustfs-data");

    let adopt = json!({"name": "Found", "server_id": srv, "container": "my-rustfs"});
    let (s, _) = admin.post("/api/v1/object-stores/adopt", adopt.clone()).await;
    assert_eq!(s, StatusCode::FORBIDDEN, "adoption needs MFA");
    step_up(&mut admin).await;
    let (s, r) =
        admin.post("/api/v1/object-stores/adopt", json!({"name": "X", "server_id": srv, "container": "nope"})).await;
    assert_eq!(s, StatusCode::NOT_FOUND, "{r}");
    let elsewhere = json!({"name": "Found", "server_id": srv, "container": "my-rustfs", "endpoint_host": "192.0.2.10"});
    let (s, r) = admin.post("/api/v1/object-stores/adopt", elsewhere).await;
    assert_eq!(s, StatusCode::BAD_REQUEST, "root-signed calls go to the node only: {r}");
    let (s, r) = admin.post("/api/v1/object-stores/adopt", adopt.clone()).await;
    assert_eq!(s, StatusCode::ACCEPTED, "{r}");
    assert_eq!(r["store"]["s3_port"], 19000);
    assert_eq!(r["store"]["version"], "1.0.6");
    assert_eq!(r["store"]["members"][0]["data_paths"], json!(["/data"]));
    assert_eq!(r["job"]["payload"]["action"], "adopt");
    docker.abort();
    let op = answer(&h, srv, true, "running").await;
    assert!(op.contains("\"op\":\"adopt\"") && op.contains("\"container_name\":\"my-rustfs\""), "{op}");
    assert!(op.contains("\"adopted\":true") && op.contains("rustfs-data"), "{op}");
    let id = r["store"]["id"].as_str().unwrap();

    // An adopted container is never reinstalled and its data never deleted.
    let (s, _) = admin.post(&format!("/api/v1/object-stores/{id}/actions"), json!({"action": "install"})).await;
    assert!(s == StatusCode::CONFLICT, "{s}");
    let (_, v) = admin.get(&format!("/api/v1/object-stores/{id}/settings")).await;
    assert_eq!(v["editable"], false, "an adopted container is not recreated: {v}");
    let (_, audit) = admin.get("/api/v1/audit?limit=100").await;
    assert!(audit.to_string().contains("storage.adopt"));
}
