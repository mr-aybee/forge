//! In-process server tests: a real UDP/TCP server on loopback, a fake
//! upstream, and a client built from `hickory-proto` messages.

#![allow(clippy::unwrap_used)] // Reason: test assertions.

use std::net::{Ipv4Addr, SocketAddr};
use std::path::Path;
use std::sync::Arc;
use std::time::{Duration, Instant};

use hickory_proto::op::{Edns, Message, Query, ResponseCode};
use hickory_proto::rr::rdata::A;
use hickory_proto::rr::{Name, RData, Record, RecordType};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream, UdpSocket};

use crate::handler::Handler;
use crate::watch::{Current, Watcher};

const DEADLINE: Duration = Duration::from_secs(5);
const FAST_POLL: Duration = Duration::from_millis(20);

fn names(addrs: &[&str]) -> String {
    let a: Vec<String> = addrs
        .iter()
        .map(|ip| format!(r#"{{ "name": "api.orders.prod.c1.local", "type": "A", "value": "{ip}" }}"#))
        .collect();

    format!(
        r#"{{ "version": 1, "generated_at_ms": 1764115200000, "zones": ["c1.local", "64.10.in-addr.arpa"], "ttl": 5,
            "records": [ {},
                {{ "name": "_http._tcp.api.orders.prod.c1.local", "type": "SRV", "priority": 0, "weight": 0, "port": 8080, "target": "api.orders.prod.c1.local" }},
                {{ "name": "2.10.64.10.in-addr.arpa", "type": "PTR", "value": "api-1.orders.prod.c1.local" }} ] }}"#,
        a.join(", ")
    )
}

/// Writes `text` the way the agent does: a temporary file, then a rename.
fn write_atomic(path: &Path, text: &str) {
    let tmp = path.with_extension("json.tmp");
    std::fs::write(&tmp, text).unwrap();
    std::fs::rename(&tmp, path).unwrap();
}

/// A UDP socket and a TCP listener on the same free port. The UDP port the
/// OS picks can already be taken for TCP by another process, so retry.
async fn bind_pair() -> (UdpSocket, TcpListener, SocketAddr) {
    for _ in 0..50 {
        let udp = UdpSocket::bind("127.0.0.1:0").await.unwrap();
        let addr = udp.local_addr().unwrap();

        match TcpListener::bind(addr).await {
            Ok(tcp) => return (udp, tcp, addr),
            Err(e) if e.kind() == std::io::ErrorKind::AddrInUse => continue,
            Err(e) => panic!("bind tcp {addr}: {e}"),
        }
    }

    panic!("no port free for both UDP and TCP after 50 tries")
}

/// A started server: its UDP and TCP address (same port).
struct Running {
    addr: SocketAddr,
    current: Arc<Current>,
}

async fn start(file: &Path, upstreams: Vec<SocketAddr>) -> Running {
    let current = Arc::new(Current::default());
    let watcher = Watcher { path: file.to_owned(), ttl: None, current: current.clone(), interval: FAST_POLL };
    tokio::spawn(watcher.run());

    let (udp, tcp, addr) = bind_pair().await;
    let mut server = hickory_server::Server::new(Handler { current: current.clone(), upstreams: upstreams.into() });
    server.register_socket(udp);
    server.register_listener(tcp, Duration::from_secs(5), 32);

    tokio::spawn(async move { server.block_until_done().await });

    Running { addr, current }
}

fn request(name: &str, qtype: RecordType, edns: Option<u16>) -> Message {
    let mut m = Message::query();
    m.metadata.id = 0x4242;
    m.metadata.recursion_desired = true;
    m.add_query(Query::query(Name::from_ascii(name).unwrap(), qtype));

    if let Some(size) = edns {
        let mut e = Edns::new();
        e.set_max_payload(size);
        m.set_edns(e);
    }

    m
}

async fn query_udp(server: SocketAddr, msg: &Message) -> Message {
    let socket = UdpSocket::bind("127.0.0.1:0").await.unwrap();
    socket.send_to(&msg.to_vec().unwrap(), server).await.unwrap();

    let mut buf = vec![0u8; 65_535];
    let (n, _) = tokio::time::timeout(DEADLINE, socket.recv_from(&mut buf)).await.unwrap().unwrap();

    Message::from_vec(&buf[..n]).unwrap()
}

async fn query(server: SocketAddr, name: &str, qtype: RecordType) -> Message {
    query_udp(server, &request(name, qtype, None)).await
}

async fn query_tcp(server: SocketAddr, msg: &Message) -> Message {
    let bytes = msg.to_vec().unwrap();
    let mut stream = TcpStream::connect(server).await.unwrap();
    stream.write_all(&u16::try_from(bytes.len()).unwrap().to_be_bytes()).await.unwrap();
    stream.write_all(&bytes).await.unwrap();

    let len = stream.read_u16().await.unwrap();
    let mut buf = vec![0u8; usize::from(len)];
    stream.read_exact(&mut buf).await.unwrap();

    Message::from_vec(&buf).unwrap()
}

fn a_values(m: &Message) -> Vec<Ipv4Addr> {
    m.answers
        .iter()
        .filter_map(|r| match &r.data {
            RData::A(A(ip)) => Some(*ip),
            _ => None,
        })
        .collect()
}

/// Polls `check` until it holds or the deadline passes.
async fn eventually<F, Fut>(mut check: F)
where
    F: FnMut() -> Fut,
    Fut: std::future::Future<Output = bool>,
{
    let until = Instant::now() + DEADLINE;

    while !check().await {
        assert!(Instant::now() < until, "condition not met before the deadline");
        tokio::task::yield_now().await;
        tokio::time::sleep(FAST_POLL).await;
    }
}

async fn loaded(dir: &tempfile::TempDir, text: &str, upstreams: Vec<SocketAddr>) -> Running {
    let file = dir.path().join("names.json");
    write_atomic(&file, text);

    let running = start(&file, upstreams).await;
    let current = running.current.clone();
    eventually(|| {
        let c = current.clone();
        async move { c.get().is_some() }
    })
    .await;

    running
}

/// A fake upstream that answers every query with one A record.
async fn fake_upstream(answer: Ipv4Addr) -> SocketAddr {
    let socket = UdpSocket::bind("127.0.0.1:0").await.unwrap();
    let addr = socket.local_addr().unwrap();

    tokio::spawn(async move {
        let mut buf = vec![0u8; 4096];

        while let Ok((n, peer)) = socket.recv_from(&mut buf).await {
            let Ok(req) = Message::from_vec(&buf[..n]) else { continue };
            let mut reply = Message::response(req.metadata.id, req.metadata.op_code);
            reply.metadata.recursion_available = true;
            reply.add_queries(req.queries.clone());

            if let Some(q) = req.queries.first() {
                reply.add_answer(Record::from_rdata(q.name().clone(), 60, RData::A(A(answer))));
            }

            let _ = socket.send_to(&reply.to_vec().unwrap(), peer).await;
        }
    });

    addr
}

#[tokio::test]
async fn a_records_rotate_round_robin() {
    let dir = tempfile::tempdir().unwrap();
    let s = loaded(&dir, &names(&["10.64.10.2", "10.64.12.4", "10.64.14.6"]), vec![]).await;

    let first = query(s.addr, "api.orders.prod.c1.local.", RecordType::A).await;
    let second = query(s.addr, "API.orders.prod.c1.local.", RecordType::A).await;

    assert_eq!(first.metadata.response_code, ResponseCode::NoError);
    assert!(first.metadata.authoritative);
    assert_eq!(a_values(&first).len(), 3);
    assert_ne!(a_values(&first)[0], a_values(&second)[0]);
    assert_eq!(a_values(&first)[1], a_values(&second)[0]);
    assert!(first.answers.iter().all(|r| r.ttl == 5));

    // SRV, PTR and AAAA (known name, no records) come from the same map.
    let srv = query(s.addr, "_http._tcp.api.orders.prod.c1.local.", RecordType::SRV).await;
    assert_eq!(srv.answers.len(), 1);

    let ptr = query(s.addr, "2.10.64.10.in-addr.arpa.", RecordType::PTR).await;
    assert_eq!(ptr.answers.len(), 1);

    let aaaa = query(s.addr, "api.orders.prod.c1.local.", RecordType::AAAA).await;
    assert_eq!(aaaa.metadata.response_code, ResponseCode::NoError);
    assert!(aaaa.answers.is_empty());

    // TCP answers too.
    let tcp = query_tcp(s.addr, &request("api.orders.prod.c1.local.", RecordType::A, None)).await;
    assert_eq!(a_values(&tcp).len(), 3);
}

#[tokio::test]
async fn unknown_name_in_zone_is_nxdomain_with_soa() {
    let dir = tempfile::tempdir().unwrap();
    let s = loaded(&dir, &names(&["10.64.10.2"]), vec![]).await;

    let reply = query(s.addr, "missing.prod.c1.local.", RecordType::A).await;

    assert_eq!(reply.metadata.response_code, ResponseCode::NXDomain);
    assert!(reply.answers.is_empty());
    assert_eq!(reply.authorities.len(), 1);
    assert_eq!(reply.authorities[0].record_type(), RecordType::SOA);
    assert_eq!(reply.authorities[0].name, Name::from_ascii("c1.local.").unwrap());
}

#[tokio::test]
async fn names_outside_the_zones_are_forwarded() {
    let dir = tempfile::tempdir().unwrap();

    // A dead first upstream (bound, never answering) exercises the fall-over.
    let dead = UdpSocket::bind("127.0.0.1:0").await.unwrap();
    let live = fake_upstream(Ipv4Addr::new(192, 0, 2, 9)).await;
    let s = loaded(&dir, &names(&["10.64.10.2"]), vec![dead.local_addr().unwrap(), live]).await;

    let reply = query(s.addr, "www.example.org.", RecordType::A).await;

    assert_eq!(reply.metadata.response_code, ResponseCode::NoError);
    assert!(!reply.metadata.authoritative);
    assert!(reply.metadata.recursion_available);
    assert_eq!(a_values(&reply), vec![Ipv4Addr::new(192, 0, 2, 9)]);
}

#[tokio::test]
async fn a_changed_file_is_swapped_in() {
    let dir = tempfile::tempdir().unwrap();
    let s = loaded(&dir, &names(&["10.64.10.2"]), vec![]).await;

    write_atomic(&dir.path().join("names.json"), &names(&["10.64.99.9", "10.64.99.8"]));

    eventually(|| async {
        let r = query(s.addr, "api.orders.prod.c1.local.", RecordType::A).await;
        let mut got = a_values(&r);
        got.sort();

        got == vec![Ipv4Addr::new(10, 64, 99, 8), Ipv4Addr::new(10, 64, 99, 9)]
    })
    .await;
}

#[tokio::test]
async fn an_invalid_file_keeps_the_previous_map() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("names.json");
    let s = loaded(&dir, &names(&["10.64.10.2"]), vec![]).await;
    let before = s.current.get().unwrap();

    write_atomic(&file, "{ not json");

    // Wait for the watcher to see the new file (it reads it, fails, keeps the map),
    // then make sure the old answers are still served.
    let marker = names(&["10.64.10.2"]).replace("\"ttl\": 5", "\"ttl\": 7");
    eventually(|| async { Arc::ptr_eq(&before, &s.current.get().unwrap()) }).await;

    let reply = query(s.addr, "api.orders.prod.c1.local.", RecordType::A).await;
    assert_eq!(a_values(&reply), vec![Ipv4Addr::new(10, 64, 10, 2)]);

    // A valid file afterwards is picked up again.
    write_atomic(&file, &marker);
    eventually(|| async {
        let r = query(s.addr, "api.orders.prod.c1.local.", RecordType::A).await;

        r.answers.first().is_some_and(|a| a.ttl == 7)
    })
    .await;
}

#[tokio::test]
async fn no_valid_file_refuses_and_udp_respects_the_payload_size() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("names.json");
    write_atomic(&file, "[]");

    let s = start(&file, vec![]).await;
    let refused = query(s.addr, "api.orders.prod.c1.local.", RecordType::A).await;
    assert_eq!(refused.metadata.response_code, ResponseCode::Refused);

    // 60 addresses (~1 KB of answers) truncate at 512 bytes, fit with EDNS 4096.
    let many: Vec<String> = (1..=60).map(|i| format!("10.64.20.{i}")).collect();
    let refs: Vec<&str> = many.iter().map(String::as_str).collect();
    write_atomic(&file, &names(&refs));

    let current = s.current.clone();
    eventually(|| {
        let c = current.clone();
        async move { c.get().is_some() }
    })
    .await;

    let small = query_udp(s.addr, &request("api.orders.prod.c1.local.", RecordType::A, None)).await;
    assert!(small.metadata.truncation);
    assert!(small.answers.len() < 60);

    let large = query_udp(s.addr, &request("api.orders.prod.c1.local.", RecordType::A, Some(4096))).await;
    assert!(!large.metadata.truncation);
    assert_eq!(large.answers.len(), 60);
}
