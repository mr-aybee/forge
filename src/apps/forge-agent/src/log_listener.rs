//! The node's local log listener (D-142): a Seq-compatible
//! `POST /api/events/raw` that appends request bodies to the spool.
//!
//! It binds `127.0.0.1:5341`, the Docker bridge gateway (for containers,
//! which reach it as `forge-node` through `--add-host
//! forge-node:host-gateway`) and an optional extra address from the node
//! settings (a mesh address, once one exists). It never binds a wildcard
//! address, so the port is not reachable from the network.
//!
//! A request carries an ingest key in `X-Seq-ApiKey`. Its SHA-256 is
//! compared in constant time with the hashes the server sent for the
//! services placed on this node. The listener checks the key and the body
//! size and appends the body unparsed; the worker parses it. It answers
//! 429 while the spool is full or its disk is below the floor, and on
//! addresses other than loopback a per-key rate cap applies by default.

use std::collections::HashMap;
use std::convert::Infallible;
use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::sync::{Arc, Mutex, PoisonError, RwLock};
use std::time::{Duration, Instant};

use bytes::Bytes;
use forge_proto::messages::{IngestKey, ListenerTuning};
use http::{Method, Request, Response, StatusCode};
use http_body_util::{BodyExt, Full, Limited};
use hyper::body::Incoming;
use hyper_util::rt::{TokioIo, TokioTimer};
use tokio::net::TcpListener;
use tokio::sync::{Semaphore, watch};

use crate::spool::{Spool, SpoolError};

/// The listener's port (Seq's).
pub const PORT: u16 = 5341;
/// The path Seq clients post to.
pub const PATH: &str = "/api/events/raw";
/// The header carrying the ingest key.
pub const KEY_HEADER: &str = "x-seq-apikey";
const MIB: u64 = 1024 * 1024;
/// Connections served at once over every address.
const MAX_CONNECTIONS: usize = 256;
/// Most rate buckets kept (one per key).
const MAX_BUCKETS: usize = forge_proto::messages::MAX_INGEST_KEYS;
const HEADER_TIMEOUT: Duration = Duration::from_secs(10);

/// Listener limits; `0` in [`ListenerTuning`] keeps a default.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Limits {
    pub max_body_bytes: u64,
    /// Requests per second per key (burst twice that).
    pub rate_per_sec: u32,
    pub rate_on_loopback: bool,
}

impl Default for Limits {
    fn default() -> Self {
        Self { max_body_bytes: MIB, rate_per_sec: 50, rate_on_loopback: false }
    }
}

impl Limits {
    #[must_use]
    pub fn from_tuning(t: &ListenerTuning) -> Self {
        let d = Self::default();
        Self {
            max_body_bytes: if t.max_body_bytes == 0 {
                d.max_body_bytes
            } else {
                t.max_body_bytes.clamp(1024, 16 * MIB)
            },
            rate_per_sec: if t.rate_per_sec == 0 { d.rate_per_sec } else { t.rate_per_sec.min(100_000) },
            rate_on_loopback: t.rate_on_loopback,
        }
    }
}

/// A token bucket.
#[derive(Debug, Clone, Copy)]
struct Bucket {
    tokens: f64,
    at: Instant,
}

/// The keys and limits the listener checks, updated from heartbeats.
#[derive(Debug, Default)]
pub struct Gate {
    keys: RwLock<Vec<[u8; 32]>>,
    limits: RwLock<Limits>,
    buckets: Mutex<HashMap<[u8; 32], Bucket>>,
}

/// Why a request is refused.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Refusal {
    Unauthorized,
    RateLimited,
}

impl Gate {
    /// Replaces the accepted keys (hashes of 32 bytes; others are ignored).
    pub fn set_keys(&self, keys: &[IngestKey]) {
        let v: Vec<[u8; 32]> = keys
            .iter()
            .take(forge_proto::messages::MAX_INGEST_KEYS)
            .filter_map(|k| <[u8; 32]>::try_from(k.hash.as_slice()).ok())
            .collect();

        // Buckets of keys that are gone are forgotten.
        self.buckets.lock().unwrap_or_else(PoisonError::into_inner).retain(|h, _| v.contains(h));
        *self.keys.write().unwrap_or_else(PoisonError::into_inner) = v;
    }

    pub fn set_limits(&self, l: Limits) {
        *self.limits.write().unwrap_or_else(PoisonError::into_inner) = l;
    }

    #[must_use]
    pub fn limits(&self) -> Limits {
        *self.limits.read().unwrap_or_else(PoisonError::into_inner)
    }

    /// The key's hash when it is accepted here. Every stored hash is
    /// compared, in constant time.
    #[must_use]
    pub fn check_key(&self, key: &str) -> Option<[u8; 32]> {
        let h = forge_secrets::crypto::sha256(key.as_bytes());
        let keys = self.keys.read().unwrap_or_else(PoisonError::into_inner);
        let mut found = false;

        for k in keys.iter() {
            found |= forge_secrets::crypto::constant_time_eq(k, &h);
        }

        found.then_some(h)
    }

    /// Takes one token of `key`'s bucket.
    fn take(&self, key: [u8; 32], rate: u32, now: Instant) -> bool {
        let rate = f64::from(rate.max(1));
        let burst = rate * 2.0;
        let mut buckets = self.buckets.lock().unwrap_or_else(PoisonError::into_inner);
        if !buckets.contains_key(&key) && buckets.len() >= MAX_BUCKETS {
            buckets.clear();
        }

        let b = buckets.entry(key).or_insert(Bucket { tokens: burst, at: now });
        b.tokens = (b.tokens + now.duration_since(b.at).as_secs_f64() * rate).min(burst);
        b.at = now;

        if b.tokens >= 1.0 {
            b.tokens -= 1.0;
            true
        } else {
            false
        }
    }

    /// Checks a request's key and, where it applies, its rate.
    pub fn admit(&self, key: Option<&str>, local: IpAddr, now: Instant) -> Result<[u8; 32], Refusal> {
        let hash = key.and_then(|k| self.check_key(k)).ok_or(Refusal::Unauthorized)?;
        let l = self.limits();
        if (!local.is_loopback() || l.rate_on_loopback) && !self.take(hash, l.rate_per_sec, now) {
            return Err(Refusal::RateLimited);
        }

        Ok(hash)
    }
}

/// The addresses to listen on: loopback, the Docker bridge gateway and an
/// extra (mesh) address. Wildcard, multicast and public addresses other
/// than an explicit extra one are never included.
#[must_use]
pub fn addresses(gateway: Option<IpAddr>, extra: Option<IpAddr>) -> Vec<SocketAddr> {
    let mut v = vec![SocketAddr::from((Ipv4Addr::LOCALHOST, PORT))];
    let usable = |ip: &IpAddr| !ip.is_unspecified() && !ip.is_multicast() && !ip.is_loopback();

    let private = |ip: &IpAddr| match ip {
        IpAddr::V4(v4) => v4.is_private(),
        IpAddr::V6(_) => false,
    };

    for ip in [gateway.filter(|g| usable(g) && private(g)), extra.filter(usable)].into_iter().flatten() {
        let a = SocketAddr::new(ip, PORT);
        if !v.contains(&a) {
            v.push(a);
        }
    }

    v
}

fn reply(status: StatusCode, body: &'static str) -> Response<Full<Bytes>> {
    let mut r = Response::new(Full::new(Bytes::from_static(body.as_bytes())));
    *r.status_mut() = status;
    r.headers_mut().insert(http::header::CONTENT_TYPE, http::HeaderValue::from_static("application/json"));

    r
}

fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| i64::try_from(d.as_millis()).unwrap_or(i64::MAX))
}

/// Answers one request.
pub async fn handle(spool: &Arc<Spool>, gate: &Gate, local: IpAddr, req: Request<Incoming>) -> Response<Full<Bytes>> {
    if req.uri().path() != PATH {
        return reply(StatusCode::NOT_FOUND, r#"{"Error":"not found"}"#);
    }

    if req.method() != Method::POST {
        return reply(StatusCode::METHOD_NOT_ALLOWED, r#"{"Error":"POST only"}"#);
    }

    let key = req.headers().get(KEY_HEADER).and_then(|v| v.to_str().ok());
    let hash = match gate.admit(key, local, Instant::now()) {
        Ok(h) => h,
        Err(Refusal::Unauthorized) => {
            return reply(StatusCode::UNAUTHORIZED, r#"{"Error":"missing or unknown API key"}"#);
        }
        Err(Refusal::RateLimited) => {
            spool.count_refused();
            return reply(StatusCode::TOO_MANY_REQUESTS, r#"{"Error":"rate limited"}"#);
        }
    };

    if spool.accepting().is_err() {
        spool.count_refused();
        return reply(StatusCode::TOO_MANY_REQUESTS, r#"{"Error":"the node's log spool is full"}"#);
    }

    let cap = usize::try_from(gate.limits().max_body_bytes).unwrap_or(usize::MAX);

    if req
        .headers()
        .get(http::header::CONTENT_LENGTH)
        .and_then(|v| v.to_str().ok()?.parse::<usize>().ok())
        .is_some_and(|n| n > cap)
    {
        return reply(StatusCode::PAYLOAD_TOO_LARGE, r#"{"Error":"body too large"}"#);
    }

    let body = match Limited::new(req.into_body(), cap).collect().await {
        Ok(b) => b.to_bytes(),
        Err(_) => return reply(StatusCode::PAYLOAD_TOO_LARGE, r#"{"Error":"body too large"}"#),
    };

    let Ok(text) = String::from_utf8(body.to_vec()) else {
        return reply(StatusCode::BAD_REQUEST, r#"{"Error":"the body is not UTF-8"}"#);
    };

    if text.trim().is_empty() {
        return reply(StatusCode::CREATED, r#"{"MinimumLevelAccepted":null}"#);
    }

    let (spool2, key_hex) = (spool.clone(), hex::encode(hash));
    let r = tokio::task::spawn_blocking(move || spool2.append_app_log(&key_hex, &text, now_ms())).await;
    match r {
        Ok(Ok(())) => reply(StatusCode::CREATED, r#"{"MinimumLevelAccepted":null}"#),
        Ok(Err(SpoolError::Full | SpoolError::DiskLow)) => {
            spool.count_refused();
            reply(StatusCode::TOO_MANY_REQUESTS, r#"{"Error":"the node's log spool is full"}"#)
        }
        Ok(Err(SpoolError::Io(e))) => {
            tracing::warn!(error = %e, "appending to the spool failed");
            reply(StatusCode::SERVICE_UNAVAILABLE, r#"{"Error":"the node could not store the events"}"#)
        }
        Err(_) => reply(StatusCode::SERVICE_UNAVAILABLE, r#"{"Error":"the node could not store the events"}"#),
    }
}

/// Serves one bound address until `stop`.
async fn serve(
    listener: TcpListener,
    spool: Arc<Spool>,
    gate: Arc<Gate>,
    slots: Arc<Semaphore>,
    mut stop: watch::Receiver<bool>,
) {
    loop {
        let (tcp, _) = tokio::select! {
            r = listener.accept() => match r {
                Ok(v) => v,
                Err(e) => {
                    tracing::debug!(error = %e, "log listener accept failed");
                    tokio::time::sleep(Duration::from_millis(100)).await;
                    continue;
                }
            },
            _ = stop.changed() => return,
        };

        // Over the connection limit the new connection is closed at once.
        let Ok(permit) = slots.clone().try_acquire_owned() else { continue };
        let local = tcp.local_addr().map_or(IpAddr::V4(Ipv4Addr::UNSPECIFIED), |a| a.ip());
        let (spool, gate) = (spool.clone(), gate.clone());
        tokio::spawn(async move {
            let svc = hyper::service::service_fn(move |req| {
                let (spool, gate) = (spool.clone(), gate.clone());
                async move { Ok::<_, Infallible>(handle(&spool, &gate, local, req).await) }
            });
            let conn = hyper::server::conn::http1::Builder::new()
                .timer(TokioTimer::new())
                .header_read_timeout(HEADER_TIMEOUT)
                .serve_connection(TokioIo::new(tcp), svc);
            let _ = tokio::time::timeout(Duration::from_secs(120), conn).await;
            drop(permit);
        });
    }
}

/// First wait before binding an address that failed again.
const RETRY_FIRST: Duration = Duration::from_secs(60);
/// Longest wait between binds of an address that keeps failing.
const RETRY_MAX: Duration = Duration::from_secs(15 * 60);

/// Addresses that failed to bind. A transient failure (the port is taken)
/// is retried with back-off: the first failure is a warning, later ones are
/// debug lines. An address that is not on this host at all
/// (`EADDRNOTAVAIL`) is not retried while it stays wanted: a node that runs
/// in a container and uses the host's Docker socket is told the host's
/// bridge gateway, which is not an address inside the container. A new
/// gateway is a new address and is tried again.
#[derive(Debug, Default)]
struct Retries(HashMap<SocketAddr, (u32, Instant)>, std::collections::HashSet<SocketAddr>);

/// How one failed bind is logged.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum BindLog {
    /// The first failure of an address that exists: a warning.
    Warn,
    /// A later failure: a debug line.
    Debug,
    /// The address is not on this host: one info line, then no retries.
    NotHere,
}

impl Retries {
    /// Whether `a` may be bound at `now`.
    fn due(&self, a: &SocketAddr, now: Instant) -> bool {
        !self.1.contains(a) && self.0.get(a).is_none_or(|(_, next)| now >= *next)
    }

    /// Records a failed bind of `a` with `e`; how to log it.
    fn on_error(&mut self, a: SocketAddr, e: &std::io::Error, now: Instant) -> BindLog {
        if e.kind() == std::io::ErrorKind::AddrNotAvailable {
            self.0.remove(&a);
            self.1.insert(a);

            return BindLog::NotHere;
        }

        if self.failed(a, now) { BindLog::Warn } else { BindLog::Debug }
    }

    /// Records a failed bind; true for the first failure (log it loudly).
    fn failed(&mut self, a: SocketAddr, now: Instant) -> bool {
        let n = self.0.get(&a).map_or(0, |(n, _)| *n).saturating_add(1);
        let wait = RETRY_FIRST.saturating_mul(1 << (n - 1).min(4)).min(RETRY_MAX);
        self.0.insert(a, (n, now + wait));

        n == 1
    }

    /// Forgets `a` (bound, or no longer wanted).
    fn clear(&mut self, a: &SocketAddr) {
        self.0.remove(a);
        self.1.remove(a);
    }

    /// Forgets every address not in `want`.
    fn keep(&mut self, want: &[SocketAddr]) {
        self.0.retain(|a, _| want.contains(a));
        self.1.retain(|a| want.contains(a));
    }

    /// When the next failed address is due.
    fn next(&self) -> Option<Instant> {
        self.0.values().map(|(_, at)| *at).min()
    }
}

/// Keeps one accept loop per wanted address; rebinds when the set changes
/// (the gateway appears with Docker) and retries addresses that failed to
/// bind, waiting longer after each failure ([`Retries`]).
pub async fn run(
    spool: Arc<Spool>,
    gate: Arc<Gate>,
    mut wanted: watch::Receiver<Vec<SocketAddr>>,
    mut shutdown: watch::Receiver<bool>,
) {
    let slots = Arc::new(Semaphore::new(MAX_CONNECTIONS));
    let mut bound: HashMap<SocketAddr, watch::Sender<bool>> = HashMap::new();
    let mut retries = Retries::default();

    loop {
        let want = wanted.borrow_and_update().clone();
        bound.retain(|a, stop| {
            let keep = want.contains(a);
            if !keep {
                let _ = stop.send(true);
                tracing::info!(addr = %a, "log listener stopped");
            }

            keep
        });
        retries.keep(&want);
        let now = Instant::now();

        for a in want.iter().filter(|a| !a.ip().is_unspecified()) {
            if bound.contains_key(a) || !retries.due(a, now) {
                continue;
            }

            match TcpListener::bind(a).await {
                Ok(l) => {
                    let (tx, rx) = watch::channel(false);
                    tokio::spawn(serve(l, spool.clone(), gate.clone(), slots.clone(), rx));
                    bound.insert(*a, tx);
                    retries.clear(a);
                    tracing::info!(addr = %a, "log listener listening");
                }
                Err(e) => match retries.on_error(*a, &e, now) {
                    BindLog::NotHere => tracing::info!(
                        addr = %a,
                        "the Docker bridge gateway is not an address of this host (a node in a container?); \
                         containers here cannot send logs to forge-node:5341; not retrying until the gateway changes"
                    ),
                    BindLog::Warn => {
                        tracing::warn!(addr = %a, error = %e, "log listener cannot bind; retrying with back-off");
                    }
                    BindLog::Debug => tracing::debug!(addr = %a, error = %e, "log listener still cannot bind"),
                },
            }
        }

        let wake = retries.next().map_or(RETRY_MAX, |at| at.saturating_duration_since(Instant::now()));
        tokio::select! {
            r = wanted.changed() => if r.is_err() { break },
            _ = shutdown.changed() => break,
            () = tokio::time::sleep(wake) => {}
        }
    }

    for stop in bound.values() {
        let _ = stop.send(true);
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)] // reason: tests
mod tests {
    use super::*;

    fn gate(keys: &[&str]) -> Gate {
        let g = Gate::default();
        g.set_keys(
            &keys
                .iter()
                .map(|k| IngestKey {
                    hash: forge_secrets::crypto::sha256(k.as_bytes()).to_vec(),
                    service_id: "s".into(),
                })
                .collect::<Vec<_>>(),
        );

        g
    }

    async fn post(addr: SocketAddr, key: Option<&str>, body: Vec<u8>) -> StatusCode {
        let tcp = tokio::net::TcpStream::connect(addr).await.unwrap();
        let (mut send, conn) = hyper::client::conn::http1::handshake(TokioIo::new(tcp)).await.unwrap();
        tokio::spawn(conn);
        let mut req = Request::post(PATH).header(http::header::HOST, "forge-node");

        if let Some(k) = key {
            req = req.header("X-Seq-ApiKey", k);
        }

        send.send_request(req.body(Full::new(Bytes::from(body))).unwrap()).await.unwrap().status()
    }

    #[tokio::test]
    async fn requests_are_spooled_and_refused_at_the_caps() {
        let dir = tempfile::tempdir().unwrap();
        let limits = crate::spool::Limits { max_bytes: 16 * 1024 * 1024, ..crate::spool::Limits::default() };
        let (spool, _) =
            Spool::open(dir.path(), uuid::Uuid::nil(), limits, Arc::new(crate::spool::SystemFreeDisk), 0).unwrap();
        let spool = Arc::new(spool);
        let g = Arc::new(gate(&["good"]));
        g.set_limits(Limits { max_body_bytes: 1024, ..Limits::default() });
        let l = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = l.local_addr().unwrap();
        let (_stop, rx) = watch::channel(false);
        tokio::spawn(serve(l, spool.clone(), g.clone(), Arc::new(Semaphore::new(8)), rx));
        let clef = br#"{"@t":"2024-01-01T00:00:00Z","@mt":"hi"}"#.to_vec();
        assert_eq!(post(addr, None, clef.clone()).await, StatusCode::UNAUTHORIZED);
        assert_eq!(post(addr, Some("bad"), clef.clone()).await, StatusCode::UNAUTHORIZED);
        assert_eq!(post(addr, Some("good"), clef.clone()).await, StatusCode::CREATED);
        assert!(spool.buffered() > clef.len(), "appended unparsed");
        assert_eq!(post(addr, Some("good"), vec![b'x'; 2048]).await, StatusCode::PAYLOAD_TOO_LARGE);
        assert_eq!(post(addr, Some("good"), vec![0xff, 0xfe]).await, StatusCode::BAD_REQUEST);

        // At the spool's cap the app gets 429 and retries later.
        spool.set_limits(crate::spool::Limits { max_bytes: 1, ..limits });
        assert_eq!(post(addr, Some("good"), clef).await, StatusCode::TOO_MANY_REQUESTS);
        assert_eq!(spool.status(0).refused_requests, 1);
    }

    #[test]
    fn a_failing_address_is_retried_with_back_off_and_warned_about_once() {
        let mut r = Retries::default();
        let a: SocketAddr = "172.17.0.1:5341".parse().unwrap();
        let t = Instant::now();
        assert!(r.due(&a, t));
        assert!(r.failed(a, t), "the first failure is a warning");
        assert!(!r.due(&a, t + Duration::from_secs(59)) && r.due(&a, t + RETRY_FIRST));
        let mut at = t + RETRY_FIRST;
        let mut waits = Vec::new();

        for _ in 0..6 {
            assert!(!r.failed(a, at), "later failures are quiet");
            let next = r.next().unwrap();
            waits.push((next - at).as_secs());
            at = next;
        }

        assert_eq!(waits, [120, 240, 480, 900, 900, 900], "doubling, capped at 15 minutes");
        r.clear(&a);
        assert!(r.due(&a, t) && r.failed(a, t), "after a bind the next failure warns again");
    }

    #[test]
    fn an_address_not_on_this_host_is_not_retried_until_it_changes() {
        let mut r = Retries::default();
        let gw: SocketAddr = "172.17.0.1:5341".parse().unwrap();
        let t = Instant::now();
        let absent = std::io::Error::from(std::io::ErrorKind::AddrNotAvailable);
        assert_eq!(r.on_error(gw, &absent, t), BindLog::NotHere);
        assert!(!r.due(&gw, t + Duration::from_secs(24 * 3600)), "never retried while wanted");
        assert_eq!(r.next(), None, "no timer for it");
        let busy = std::io::Error::from(std::io::ErrorKind::AddrInUse);
        let lo: SocketAddr = "127.0.0.1:5341".parse().unwrap();
        assert_eq!(r.on_error(lo, &busy, t), BindLog::Warn, "a taken port is transient");
        assert_eq!(r.on_error(lo, &busy, t + RETRY_FIRST), BindLog::Debug);

        // Docker reports another gateway: the old address is forgotten, the new one tried.
        let other: SocketAddr = "172.18.0.1:5341".parse().unwrap();
        r.keep(&[lo, other]);
        assert!(r.due(&other, t));
        r.keep(&[lo, gw]);
        assert!(r.due(&gw, t), "wanted again later: tried again");
    }

    /// An address that does not exist on the host (a containerised node's
    /// Docker bridge) does not keep loopback from serving.
    #[tokio::test]
    async fn loopback_serves_while_another_address_cannot_bind() {
        let dir = tempfile::tempdir().unwrap();
        let (spool, _) = Spool::open(
            dir.path(),
            uuid::Uuid::nil(),
            crate::spool::Limits::default(),
            Arc::new(crate::spool::SystemFreeDisk),
            0,
        )
        .unwrap();
        let port = std::net::TcpListener::bind("127.0.0.1:0").unwrap().local_addr().unwrap().port();
        let lo = SocketAddr::from((Ipv4Addr::LOCALHOST, port));

        // TEST-NET-1: never assigned to this host.
        let missing = SocketAddr::from((Ipv4Addr::new(192, 0, 2, 1), port));
        let (_want_tx, want) = watch::channel(vec![missing, lo]);
        let (stop_tx, stop) = watch::channel(false);
        let task = tokio::spawn(run(Arc::new(spool), Arc::new(gate(&["good"])), want, stop));
        let deadline = tokio::time::Instant::now() + Duration::from_secs(5);

        loop {
            if tokio::net::TcpStream::connect(lo).await.is_ok() {
                break;
            }

            assert!(tokio::time::Instant::now() < deadline, "loopback never listened");
            tokio::task::yield_now().await;
        }

        let clef = br#"{"@mt":"hi"}"#.to_vec();
        assert_eq!(post(lo, Some("good"), clef).await, StatusCode::CREATED);
        stop_tx.send(true).unwrap();
        task.await.unwrap();
    }

    #[test]
    fn never_binds_a_wildcard_or_public_address() {
        let lo = SocketAddr::from((Ipv4Addr::LOCALHOST, PORT));
        assert_eq!(addresses(None, None), vec![lo]);
        assert_eq!(addresses(Some(IpAddr::V4(Ipv4Addr::UNSPECIFIED)), Some("::".parse().unwrap())), vec![lo]);
        assert_eq!(addresses(Some("8.8.8.8".parse().unwrap()), None), vec![lo], "the gateway must be private");
        let gw: IpAddr = "172.17.0.1".parse().unwrap();
        assert_eq!(addresses(Some(gw), None), vec![lo, SocketAddr::new(gw, PORT)]);
        let mesh: IpAddr = "100.64.0.7".parse().unwrap();
        let all = addresses(Some(gw), Some(mesh));
        assert_eq!(all.len(), 3);
        assert!(all.iter().all(|a| !a.ip().is_unspecified()));
    }

    #[test]
    fn keys_are_checked_by_hash_and_rated_off_loopback() {
        let g = gate(&["good"]);
        let (lo, gw): (IpAddr, IpAddr) = ("127.0.0.1".parse().unwrap(), "172.17.0.1".parse().unwrap());
        let t = Instant::now();
        assert_eq!(g.admit(None, lo, t), Err(Refusal::Unauthorized));
        assert_eq!(g.admit(Some("bad"), lo, t), Err(Refusal::Unauthorized));
        assert!(g.admit(Some("good"), lo, t).is_ok());
        g.set_limits(Limits { rate_per_sec: 2, ..Limits::default() });

        for _ in 0..100 {
            assert!(g.admit(Some("good"), lo, t).is_ok(), "no cap on loopback by default");
        }

        let ok = (0..10).filter(|_| g.admit(Some("good"), gw, t).is_ok()).count();
        assert_eq!(ok, 4, "burst of twice the rate");
        assert!(g.admit(Some("good"), gw, t + Duration::from_secs(1)).is_ok(), "refills over time");
        g.set_keys(&[]);
        assert_eq!(g.admit(Some("good"), lo, t), Err(Refusal::Unauthorized), "a revoked key stops working");
    }
}
