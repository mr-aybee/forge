//! The node spool against the real analytics worker listener (D-142):
//! mTLS, trust rules, file-id dedupe across a worker restart, back-pressure
//! and end to end from spool to store.
#![allow(clippy::unwrap_used, clippy::expect_used)] // reason: tests

use std::net::SocketAddr;
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use forge_agent::spool::{Limits, Spool, SystemFreeDisk, app_log_row};
use forge_agent::spool_upload::{self, MakeSink, Outcome, Sink, Target, WorkerSink};
use forge_analytics_proto::config::{Endpoint, StoreSelection, WorkerConfig};
use forge_analytics_proto::tuning::Tuning;
use forge_analytics_proto::{Reply, Request};
use forge_analytics_worker::memory::MemoryStore;
use forge_analytics_worker::wiring::Worker;
use forge_core::analytics::{AnalyticsMaps, AppKey, KeyService, MinuteBucket, NodeCert, NodeSample, Placement};
use forge_core::spool::{ContainerLine, FileName, Labels, Row, Stream};
use forge_core::{ServerId, ServiceId, Timestamp};
use forge_pki::{AgentCsr, CertificateAuthority};
use rustls::ClientConfig;
use tokio::sync::watch;

const KEY: &str = "ingest-key-1";

fn tuning() -> Tuning {
    Tuning {
        node_metrics_days: 7,
        top_processes_days: 7,
        node_rollup_days: 365,
        app_logs_days: 30,
        cpu_quota_vcpu: 0.0,
        memory_budget_mb: 1024,
        search_max_concurrent: 8,
        search_timeout_secs: 20,
        search_max_lookback_days: 0,
        search_threads: 0,
        search_memory_mb: 0,
        ingest_max_queued_mb: 1,
        ingest_max_batch_rows: 20_000,
        ingest_flush_interval_ms: 10,
        min_free_disk_mb: 0,
    }
}

fn now() -> Timestamp {
    Timestamp(forge_agent::node_analytics::now_ms())
}

/// Agent identities per node, issued once: `(TLS config, serial)`.
#[derive(Default)]
struct Ids(std::sync::Mutex<std::collections::HashMap<ServerId, (Arc<ClientConfig>, String)>>);

impl Ids {
    fn get(&self, ca: &CertificateAuthority, node: ServerId) -> (Arc<ClientConfig>, String) {
        self.0.lock().unwrap().entry(node).or_insert_with(|| Fixture::identity(ca, node)).clone()
    }
}

/// A worker with an in-memory store and its agent listener on a free port.
struct Fixture {
    ca: Arc<CertificateAuthority>,
    ids: Ids,
    store: Arc<MemoryStore>,
    worker: Arc<Worker>,
    addr: SocketAddr,
    stop: watch::Sender<bool>,
}

impl Fixture {
    async fn start(ca: Arc<CertificateAuthority>, state: &Path, maps: AnalyticsMaps, max_uploads: usize) -> Self {
        let store = Arc::new(MemoryStore::default());
        let config = WorkerConfig {
            store: StoreSelection::Memory,
            endpoint: Endpoint::Loopback,
            tuning: tuning(),
            agent_listener: None,
        };
        let worker = Arc::new(Worker::with_store(&config, store.clone(), None, Arc::new(forge_core::SystemClock)));
        assert!(worker.prepare().await);
        let cert = ca.issue_server_certificate(&["localhost".into()], now()).unwrap();
        let tls = worker
            .listener_tls(
                ca.cert_pem(),
                &forge_analytics_proto::config::AgentTls { cert_pem: cert.cert_pem, key_pem: cert.key_pem },
            )
            .unwrap();
        let uploads = worker.uploads(state, max_uploads).unwrap();
        let tcp = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = tcp.local_addr().unwrap();
        let (stop, rx) = watch::channel(false);
        tokio::spawn(forge_analytics_worker::ingest::upload::serve(tcp, tls, uploads, rx));
        let fx = Self { ca, ids: Ids::default(), store, worker, addr, stop };
        fx.push(maps).await;
        fx
    }

    /// Pushes `maps` with the current certificate of every active node
    /// (each node's identity in [`Ids`]), as forge-server does.
    async fn push(&self, mut maps: AnalyticsMaps) {
        maps.node_certs = maps
            .active_nodes
            .iter()
            .map(|n| NodeCert { server_id: *n, serial: self.ids.get(&self.ca, *n).1 })
            .collect();
        assert_eq!(self.worker.handle(Request::PushMaps(maps), Vec::new()).await, Ok(Reply::Ok));
    }

    fn url(&self) -> String {
        format!("https://localhost:{}", self.addr.port())
    }

    /// A new agent identity of `node` signed by `ca`, with its serial.
    fn identity(ca: &CertificateAuthority, node: ServerId) -> (Arc<ClientConfig>, String) {
        let csr = AgentCsr::generate("n").unwrap();
        let cert =
            ca.issue_agent_certificate(&csr.csr_pem, &node.to_string(), now(), Duration::from_secs(3_600)).unwrap();
        (forge_agent::tls::mutual(ca.cert_pem(), &cert.cert_pem, &csr.key_pem).unwrap(), cert.serial_hex)
    }

    /// `node`'s current identity.
    fn tls(&self, node: ServerId) -> Arc<ClientConfig> {
        self.ids.get(&self.ca, node).0
    }

    fn sink(&self, node: ServerId) -> WorkerSink {
        WorkerSink::new(&self.url(), self.tls(node)).unwrap()
    }
}

fn maps(node: ServerId, svc: ServiceId) -> AnalyticsMaps {
    AnalyticsMaps {
        active_nodes: vec![node],
        key_services: vec![KeyService {
            token_hash: forge_secrets::crypto::sha256_hex(KEY.as_bytes()),
            service_id: svc,
        }],
        app_keys: vec![AppKey { server_id: node, app: "prod/api".into(), service_id: svc }],
        placements: vec![Placement { service_id: svc, server_id: node, names: vec!["n1".into()] }],
        node_certs: vec![],
    }
}

fn spool(dir: &Path, node: ServerId) -> Spool {
    let limits = Limits { seal_bytes: 64 * 1024, ..Limits::default() };
    Spool::open(dir, node.to_string().parse().unwrap(), limits, Arc::new(SystemFreeDisk), now().0).unwrap().0
}

fn key_hash() -> String {
    forge_secrets::crypto::sha256_hex(KEY.as_bytes())
}

fn container_row(i: usize, app: &str) -> Row {
    Row::ContainerLog(ContainerLine {
        ts: 1_000 + i64::try_from(i).unwrap(),
        stream: Stream::Stdout,
        cid: "0123456789ab".into(),
        name: "web".into(),
        image: "img".into(),
        labels: Labels { forge_app: app.into(), ..Labels::default() },
        line: format!("line {i}"),
    })
}

/// One sealed file holding `rows`; returns its stem and bytes.
fn sealed(s: &Spool, rows: &[Row]) -> (String, Vec<u8>) {
    for r in rows {
        s.append(r, now().0).unwrap();
    }
    s.seal(now().0).unwrap();
    let f = s.oldest().unwrap();
    (f.stem.clone(), std::fs::read(&f.path).unwrap())
}

async fn wait_for(what: &str, mut f: impl FnMut() -> bool) {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(15);
    while !f() {
        assert!(tokio::time::Instant::now() < deadline, "timed out waiting for {what}");
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}

#[tokio::test]
async fn end_to_end_rows_sent_equal_rows_stored_and_the_spool_empties() {
    let ca = Arc::new(CertificateAuthority::generate("test ca", now()).unwrap());
    let (node, svc) = (ServerId::new(), ServiceId::new());
    let state = tempfile::tempdir().unwrap();
    let fx = Fixture::start(ca.clone(), state.path(), maps(node, svc), 4).await;
    let dir = tempfile::tempdir().unwrap();
    let s = Arc::new(spool(dir.path(), node));
    // 3 files: CLEF bodies, container lines, and minute rows.
    let clef = "{\"@t\":\"2024-01-01T00:00:00Z\",\"@mt\":\"order {Id}\",\"Id\":1}\n".repeat(50);
    for _ in 0..20 {
        s.append(&app_log_row(&key_hash(), &clef, now().0), now().0).unwrap();
    }
    s.seal(now().0).unwrap();
    for i in 0..300 {
        s.append(&container_row(i, "prod/api"), now().0).unwrap();
    }
    s.seal(now().0).unwrap();
    let mut b = MinuteBucket::new(0);
    b.add(&NodeSample { cpu: 5.0, ..NodeSample::default() });
    s.append(&Row::NodeMinute(b.close()), now().0).unwrap();
    s.seal(now().0).unwrap();
    let files = s.status(now().0).files;
    assert!(files >= 3, "sealed by size and on demand");

    let (target_tx, target) = watch::channel(Some(Target { url: fx.url(), generation: 0 }));
    let (stop_tx, stop) = watch::channel(false);
    let tls = fx.tls(node);
    let make: MakeSink =
        Box::new(move |t: &Target| Ok(Box::new(WorkerSink::new(&t.url, tls.clone())?) as Box<dyn Sink>));
    let uploader = tokio::spawn(spool_upload::run(s.clone(), target, make, stop));
    wait_for("every file acknowledged", || s.status(now().0).files == 0).await;
    assert_eq!(fx.store.counts(), [20 * 50 + 300, 1, 0, 0], "rows sent equal rows stored");
    let logs = fx.store.logs();
    assert!(logs.iter().all(|e| e.server_id == Some(node)));
    assert!(logs.iter().all(|e| e.service_id == Some(svc)));
    let st = s.status(now().0);
    assert_eq!((st.files, st.bytes, st.uploaded_files, st.dropped_files), (0, 0, files, 0));
    assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 0, "no files left");
    stop_tx.send_replace(true);
    drop(target_tx);
    uploader.await.unwrap();
    fx.stop.send_replace(true);
}

#[tokio::test]
async fn unknown_revoked_and_foreign_nodes_are_refused() {
    let ca = Arc::new(CertificateAuthority::generate("test ca", now()).unwrap());
    let (node, svc) = (ServerId::new(), ServiceId::new());
    let state = tempfile::tempdir().unwrap();
    let fx = Fixture::start(ca.clone(), state.path(), maps(node, svc), 4).await;
    let dir = tempfile::tempdir().unwrap();

    // A certificate from another CA never completes the handshake.
    let other_ca = CertificateAuthority::generate("other", now()).unwrap();
    let csr = AgentCsr::generate("n").unwrap();
    let cert =
        other_ca.issue_agent_certificate(&csr.csr_pem, &node.to_string(), now(), Duration::from_secs(60)).unwrap();
    let mut forged = forge_agent::spool_upload::WorkerSink::new(
        &fx.url(),
        forge_agent::tls::mutual(ca.cert_pem(), &cert.cert_pem, &csr.key_pem).unwrap(),
    )
    .unwrap();
    let (stem, body) = sealed(&spool(dir.path(), node), &[container_row(0, "prod/api")]);
    assert!(matches!(forged.upload(&stem, body.clone()).await, Outcome::Retry(_)));

    // A node forge-server does not list as active (unknown or revoked).
    let stranger = ServerId::new();
    let sdir = tempfile::tempdir().unwrap();
    let (s_stem, s_body) = sealed(&spool(sdir.path(), stranger), &[container_row(0, "prod/api")]);
    match fx.sink(stranger).upload(&s_stem, s_body).await {
        Outcome::Retry(r) => assert!(r.contains("403"), "{r}"),
        other => panic!("{other:?}"),
    }
    // Revoking the node: it leaves the active list.
    let mut revoked = maps(node, svc);
    revoked.active_nodes.clear();
    fx.push(revoked).await;
    match fx.sink(node).upload(&stem, body.clone()).await {
        Outcome::Retry(r) => assert!(r.contains("403"), "{r}"),
        other => panic!("{other:?}"),
    }
    fx.push(maps(node, svc)).await;

    // A certificate of the node that is not its current one (replaced at
    // renewal, so revoked) is refused, although it chains to the CA.
    let (replaced, _) = Fixture::identity(&ca, node);
    match WorkerSink::new(&fx.url(), replaced).unwrap().upload(&stem, body.clone()).await {
        Outcome::Retry(r) => assert!(r.contains("403"), "{r}"),
        other => panic!("{other:?}"),
    }

    // A node cannot upload a file named for another node.
    let other = ServerId::new();
    let mut both = maps(node, svc);
    both.active_nodes.push(other);
    fx.push(both).await;
    match fx.sink(other).upload(&stem, body.clone()).await {
        Outcome::Retry(r) => assert!(r.contains("403"), "{r}"),
        other => panic!("{other:?}"),
    }
    // A node cannot write for a service placed elsewhere: its lines are
    // stored for the node, unattributed, and its app keys are not taken.
    let odir = tempfile::tempdir().unwrap();
    let rows = [container_row(0, "prod/api"), app_log_row(&key_hash(), "{\"@mt\":\"x\"}", 1)];
    let (o_stem, o_body) = sealed(&spool(odir.path(), other), &rows);
    assert_eq!(fx.sink(other).upload(&o_stem, o_body).await, Outcome::Stored);
    let logs = fx.store.logs();
    assert_eq!(logs.len(), 1, "the foreign key's event is not stored");
    assert_eq!((logs[0].server_id, logs[0].service_id), (Some(other), None));
    fx.stop.send_replace(true);
}

#[tokio::test]
async fn a_file_sent_again_is_stored_once_even_after_a_worker_restart() {
    let ca = Arc::new(CertificateAuthority::generate("test ca", now()).unwrap());
    let (node, svc) = (ServerId::new(), ServiceId::new());
    let state = tempfile::tempdir().unwrap();
    let dir = tempfile::tempdir().unwrap();
    let s = spool(dir.path(), node);
    let (stem, body) = sealed(&s, &(0..10).map(|i| container_row(i, "prod/api")).collect::<Vec<_>>());
    assert!(FileName::parse(&stem).is_some());
    let fx = Fixture::start(ca.clone(), state.path(), maps(node, svc), 4).await;
    assert_eq!(fx.sink(node).upload(&stem, body.clone()).await, Outcome::Stored);
    // The ack was lost: the agent sends the file again.
    assert_eq!(fx.sink(node).upload(&stem, body.clone()).await, Outcome::Stored);
    assert_eq!(fx.store.counts()[0], 10);
    fx.stop.send_replace(true);
    drop(fx);
    // A new worker process with the same state folder and a fresh store:
    // the id log remembers the file, so nothing is written.
    let fx = Fixture::start(ca, state.path(), maps(node, svc), 4).await;
    assert_eq!(fx.sink(node).upload(&stem, body).await, Outcome::Stored);
    assert_eq!(fx.store.counts()[0], 0, "not stored a second time");
    fx.stop.send_replace(true);
}

#[tokio::test]
async fn bad_and_oversized_files_are_rejected_for_good() {
    let ca = Arc::new(CertificateAuthority::generate("test ca", now()).unwrap());
    let (node, svc) = (ServerId::new(), ServiceId::new());
    let state = tempfile::tempdir().unwrap();
    let fx = Fixture::start(ca.clone(), state.path(), maps(node, svc), 4).await;
    let stem = FileName::new(node.to_string().parse().unwrap()).stem();
    assert!(matches!(fx.sink(node).upload(&stem, b"not zstd".to_vec()).await, Outcome::Reject(_)));
    let big = vec![0u8; usize::try_from(forge_core::spool::MAX_FILE_BYTES).unwrap() + 1];
    assert!(matches!(fx.sink(node).upload(&stem, big).await, Outcome::Reject(_)));
    fx.stop.send_replace(true);
}

#[tokio::test]
async fn over_the_import_limit_the_worker_answers_429_at_once_and_the_file_waits() {
    let ca = Arc::new(CertificateAuthority::generate("test ca", now()).unwrap());
    let (node, svc) = (ServerId::new(), ServiceId::new());
    let state = tempfile::tempdir().unwrap();
    let fx = Fixture::start(ca.clone(), state.path(), maps(node, svc), 1).await;
    let gate = fx.store.hold_writes();
    let dir = tempfile::tempdir().unwrap();
    let s = spool(dir.path(), node);
    let (first, body) = sealed(&s, &[container_row(0, "prod/api")]);
    let mut sink = fx.sink(node);
    let held = tokio::spawn(async move { sink.upload(&first, body).await });
    wait_for("the first import to reach the store", || fx.store.writes_started() == 1).await;
    s.ack(&s.oldest().unwrap().stem).unwrap();
    let (second, body2) = sealed(&s, &[container_row(1, "prod/api")]);
    let started = std::time::Instant::now();
    match fx.sink(node).upload(&second, body2.clone()).await {
        Outcome::Retry(r) => assert!(r.contains("429"), "{r}"),
        other => panic!("{other:?}"),
    }
    assert!(started.elapsed() < Duration::from_secs(5), "refused at once, not held");
    gate.release();
    assert_eq!(held.await.unwrap(), Outcome::Stored);
    assert_eq!(fx.sink(node).upload(&second, body2).await, Outcome::Stored, "sent again later, stored");
    assert_eq!(fx.store.counts()[0], 2);
    fx.stop.send_replace(true);
}

/// The DER certificate the worker's listener presents to `node`'s agent.
async fn served_certificate(fx: &Fixture, node: ServerId) -> Vec<u8> {
    let tcp = tokio::net::TcpStream::connect(fx.addr).await.unwrap();
    let name = rustls_pki_types::ServerName::try_from("localhost").unwrap();
    let tls = tokio_rustls::TlsConnector::from(fx.tls(node)).connect(name, tcp).await.unwrap();
    tls.get_ref().1.peer_certificates().unwrap()[0].as_ref().to_vec()
}

fn der(pem: &str) -> Vec<u8> {
    use rustls_pki_types::pem::PemObject;
    rustls_pki_types::CertificateDer::from_pem_slice(pem.as_bytes()).unwrap().as_ref().to_vec()
}

/// forge-server sends the running worker a new listener certificate before
/// the current one expires (D-145): new connections get it at once, with no
/// restart, and uploads go on. A pair that does not load is refused and the
/// current certificate stays.
#[tokio::test]
async fn a_running_worker_takes_a_renewed_listener_certificate() {
    let ca = Arc::new(CertificateAuthority::generate("test ca", now()).unwrap());
    let (node, svc) = (ServerId::new(), ServiceId::new());
    let state = tempfile::tempdir().unwrap();
    let fx = Fixture::start(ca.clone(), state.path(), maps(node, svc), 4).await;
    let first = served_certificate(&fx, node).await;
    let renewed = ca.issue_server_certificate(&["localhost".into()], now()).unwrap();
    assert_ne!(der(&renewed.cert_pem), first);
    let set = |cert_pem: String, key_pem: String| {
        Request::SetAgentTls(forge_analytics_proto::config::AgentTls { cert_pem, key_pem })
    };
    let bad = fx.worker.handle(set(renewed.cert_pem.clone(), "not a key".into()), Vec::new()).await;
    assert!(bad.is_err(), "{bad:?}");
    assert_eq!(served_certificate(&fx, node).await, first, "a bad pair leaves the current one");
    // A certificate that has expired is refused as well.
    let old = ca.issue_server_certificate(&["localhost".into()], Timestamp(now().0 - 100 * 24 * 3_600_000)).unwrap();
    assert!(fx.worker.handle(set(old.cert_pem, old.key_pem), Vec::new()).await.is_err());
    assert_eq!(served_certificate(&fx, node).await, first);
    let ok = fx.worker.handle(set(renewed.cert_pem.clone(), renewed.key_pem.clone()), Vec::new()).await;
    assert_eq!(ok, Ok(Reply::Ok));
    assert_eq!(served_certificate(&fx, node).await, der(&renewed.cert_pem), "new connections get the new one");
    let dir = tempfile::tempdir().unwrap();
    let (stem, body) = sealed(&spool(dir.path(), node), &[container_row(0, "prod/api")]);
    assert_eq!(fx.sink(node).upload(&stem, body).await, Outcome::Stored);
    fx.stop.send_replace(true);
}
