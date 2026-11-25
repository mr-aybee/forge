//! Reads one agent spool file (D-142) into typed rows, applying the trust
//! rules: the node is the one of the client certificate, whatever the file
//! says; an application's ingest key only counts for a service placed on
//! that node; a container's `forge.app` label only names a service placed
//! there. Application bodies are parsed by `forge_core::logs::parse_batch`
//! and container lines by `forge_core::logs::container_event`, exactly as
//! the older paths did, so a line is stored the same way whichever way it
//! came.
//!
//! The file is decompressed as a stream with a cap on its decompressed
//! size; a row that does not parse is skipped and counted, so one bad row
//! never blocks a node's spool.

use std::io::{BufRead, BufReader, Read};

use forge_core::analytics::{ContainerMinute, NodeMinute, NodeProcessMinute};
use forge_core::logs::{ContainerContext, IngestContext, LogEvent, LogStream, container_event, parse_batch};
use forge_core::spool::{self, FileHeader, MAX_RAW_BYTES, Row, VERSION};
use forge_core::{ServerId, ServiceId, Timestamp};
use forge_proto::messages::{ContainerLabels, MAX_CONTAINER_FIELD_BYTES, MAX_CONTAINER_LINE_BYTES};

use super::attribution::{attribute, clip};
use crate::maps::Indexed;

/// Longest row read (a body of the agent's largest allowed request, escaped).
const MAX_ROW_BYTES: usize = 40 * 1024 * 1024;

/// Why a file is refused for good.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum FileError {
    #[error("the file is not readable zstd: {0}")]
    Decode(String),
    #[error("bad header: {0}")]
    Header(String),
    #[error("the file is larger than {MAX_RAW_BYTES} bytes once decompressed")]
    TooLarge,
}

/// The rows of one file, ready to store.
#[derive(Debug, Default)]
pub struct Imported {
    pub logs: Vec<LogEvent>,
    pub minutes: Vec<NodeMinute>,
    pub processes: Vec<NodeProcessMinute>,
    pub containers: Vec<ContainerMinute>,
    /// Rows skipped: unreadable, or an application key not valid here.
    pub skipped: u64,
}

impl Imported {
    #[must_use]
    pub fn rows(&self) -> u64 {
        (self.logs.len() + self.minutes.len() + self.processes.len() + self.containers.len()) as u64
    }
}

fn labels(l: &spool::Labels) -> ContainerLabels {
    ContainerLabels {
        forge_app: l.forge_app.clone(),
        forge_stack: l.forge_stack.clone(),
        compose_project: l.compose_project.clone(),
        compose_service: l.compose_service.clone(),
    }
}

/// The service `key` (hex SHA-256) writes for, when it is placed on `node`.
fn key_service(maps: &Indexed, key: &str, node: ServerId) -> Option<ServiceId> {
    let service = *maps.keys.get(key)?;
    maps.placements.get(&service)?.iter().any(|p| p.server_id == node).then_some(service)
}

/// Reads `bytes` (one sealed file) sent by `node`.
pub fn read(bytes: &[u8], node: ServerId, now: Timestamp, maps: &Indexed) -> Result<Imported, FileError> {
    let decoder = zstd::stream::read::Decoder::new(bytes).map_err(|e| FileError::Decode(e.to_string()))?;
    let mut reader = BufReader::new(decoder.take(MAX_RAW_BYTES + 1));
    let mut total: u64 = 0;
    let mut line = String::new();
    let mut next = |line: &mut String| -> Result<bool, FileError> {
        line.clear();
        let n =
            (&mut reader).take(MAX_ROW_BYTES as u64).read_line(line).map_err(|e| FileError::Decode(e.to_string()))?;
        total += n as u64;

        if total > MAX_RAW_BYTES {
            return Err(FileError::TooLarge);
        }

        Ok(n > 0)
    };

    if !next(&mut line)? {
        return Err(FileError::Header("the file is empty".into()));
    }

    let header: FileHeader = serde_json::from_str(&line).map_err(|e| FileError::Header(e.to_string()))?;
    if header.v != VERSION {
        return Err(FileError::Header(format!("version {} is not supported", header.v)));
    }

    let mut out = Imported::default();
    let services = maps.apps.get(&node);

    while next(&mut line)? {
        if line.trim().is_empty() {
            continue;
        }

        let Ok(row) = serde_json::from_str::<Row>(&line) else {
            out.skipped += 1;
            continue;
        };

        match row {
            Row::Log(l) => {
                let Some(service) = key_service(maps, &l.key, node) else {
                    out.skipped += 1;
                    continue;
                };

                let cx =
                    IngestContext { service_id: service, server_id: Some(node), node_ts: Some(Timestamp(l.ts)), now };

                match parse_batch(&l.body, cx) {
                    Ok(events) => out.logs.extend(events),
                    Err(_) => out.skipped += 1,
                }
            }
            Row::ContainerLog(c) => {
                let a = attribute(Some(&labels(&c.labels)), services);
                let cx = ContainerContext {
                    service_id: a.service_id,
                    server_id: node,
                    container_id: clip(&c.cid, 64).to_owned(),
                    container_name: clip(&c.name, MAX_CONTAINER_FIELD_BYTES).to_owned(),
                    stack: a.stack,
                    now,
                };

                let stream = match c.stream {
                    spool::Stream::Stdout => LogStream::Stdout,
                    spool::Stream::Stderr => LogStream::Stderr,
                };

                out.logs.push(container_event(&cx, stream, c.ts, clip(&c.line, MAX_CONTAINER_LINE_BYTES)));
            }
            Row::NodeMinute(m) => out.minutes.push(NodeMinute::from_row(node, &m)),
            Row::TopProcess(p) => out.processes.push(NodeProcessMinute {
                server_id: node,
                minute: Timestamp(p.minute),
                pid: p.pid,
                name: clip(&p.name, 256).to_owned(),
                user: clip(&p.user, 256).to_owned(),
                cpu: if p.cpu.is_finite() { p.cpu.max(0.0) } else { 0.0 },
                mem: p.mem,
                cmd: clip(&p.cmd, 1024).to_owned(),
            }),
            Row::ContainerMinute(s) => {
                if s.name.is_empty() {
                    out.skipped += 1;
                    continue;
                }

                let a = attribute(Some(&labels(&s.labels)), services);
                out.containers.push(ContainerMinute {
                    server_id: node,
                    minute: Timestamp(s.minute),
                    container_name: clip(&s.name, MAX_CONTAINER_FIELD_BYTES).to_owned(),
                    container_id: clip(&s.cid, 64).to_owned(),
                    image: clip(&s.image, MAX_CONTAINER_FIELD_BYTES).to_owned(),
                    service_id: a.service_id,
                    stack: a.stack,
                    cpu: if s.cpu.is_finite() { s.cpu.max(0.0) } else { 0.0 },
                    mem_used: s.mem_used,
                    mem_limit: s.mem_limit,
                    rx: s.rx,
                    tx: s.tx,
                    block_read: s.block_read,
                    block_write: s.block_write,
                    restarts: s.restarts,
                });
            }
        }
    }

    Ok(out)
}

#[cfg(test)]
#[allow(clippy::unwrap_used)] // reason: tests
pub(crate) mod tests {
    use super::*;
    use forge_core::analytics::{AnalyticsMaps, AppKey, KeyService, Placement};
    use forge_core::spool::{AppLog, BodyFormat, ContainerLine, Counts, Labels};
    use std::io::Write;

    /// A file as an agent writes it.
    pub(crate) fn file(node: &str, rows: &[Row]) -> Vec<u8> {
        let mut raw = Vec::new();
        let header = FileHeader { v: VERSION, node: node.into(), created: 1, counts: Counts::default() };
        serde_json::to_writer(&mut raw, &header).unwrap();
        raw.push(b'\n');

        for r in rows {
            serde_json::to_writer(&mut raw, r).unwrap();
            raw.push(b'\n');
        }

        let mut enc = zstd::stream::write::Encoder::new(Vec::new(), 1).unwrap();
        enc.write_all(&raw).unwrap();

        enc.finish().unwrap()
    }

    pub(crate) fn app(key: &str, body: &str) -> Row {
        Row::Log(AppLog {
            key: forge_secrets::crypto::sha256_hex(key.as_bytes()),
            ts: 5,
            fmt: BodyFormat::Raw,
            body: body.into(),
        })
    }

    pub(crate) fn container(app: &str, line: &str) -> Row {
        Row::ContainerLog(ContainerLine {
            ts: 7,
            stream: spool::Stream::Stderr,
            cid: "0123456789ab".into(),
            name: "web".into(),
            image: "img".into(),
            labels: Labels { forge_app: app.into(), ..Labels::default() },
            line: line.into(),
        })
    }

    /// Maps where `svc` is placed on `node` and `key` writes for it.
    pub(crate) fn maps(node: ServerId, svc: ServiceId, key: &str) -> std::sync::Arc<Indexed> {
        let m = crate::maps::Maps::default();
        m.replace(AnalyticsMaps {
            active_nodes: vec![node],
            key_services: vec![KeyService {
                token_hash: forge_secrets::crypto::sha256_hex(key.as_bytes()),
                service_id: svc,
            }],
            app_keys: vec![AppKey { server_id: node, app: "prod/api".into(), service_id: svc }],
            placements: vec![Placement { service_id: svc, server_id: node, names: vec!["n1".into()] }],
            node_certs: vec![],
        })
        .unwrap();

        m.get()
    }

    #[test]
    fn rows_are_read_with_the_trust_rules() {
        let (node, other, svc) = (ServerId::new(), ServerId::new(), ServiceId::new());
        let m = maps(node, svc, "k1");
        let rows = vec![
            app("k1", "{\"@t\":\"2024-01-01T00:00:00Z\",\"@mt\":\"hi {N}\",\"N\":1}\n{\"@mt\":\"two\"}"),
            app("unknown", "{\"@mt\":\"x\"}"),
            container("prod/api", "{\"level\":\"error\",\"msg\":\"boom\"}"),
            container("prod/other", "plain"),
        ];

        // The file claims another node; the certificate's node wins.
        let got = read(&file(&other.to_string(), &rows), node, Timestamp(9), &m).unwrap();
        assert_eq!(got.logs.len(), 4);
        assert_eq!(got.skipped, 1, "the unknown key is not stored");
        assert!(got.logs.iter().all(|e| e.server_id == Some(node)));
        assert_eq!(got.logs[0].service_id, Some(svc));
        let expected = container_event(
            &ContainerContext {
                service_id: Some(svc),
                server_id: node,
                container_id: "0123456789ab".into(),
                container_name: "web".into(),
                stack: None,
                now: Timestamp(9),
            },
            LogStream::Stderr,
            7,
            "{\"level\":\"error\",\"msg\":\"boom\"}",
        );
        assert_eq!((got.logs[2].level, &got.logs[2].message), (expected.level, &expected.message));
        assert_eq!(got.logs[3].service_id, None, "a service not placed here is not attributed");

        // The same key from a node where its service is not placed is refused.
        let elsewhere = read(&file("x", &rows[..1]), other, Timestamp(9), &m).unwrap();
        assert_eq!((elsewhere.logs.len(), elsewhere.skipped), (0, 1));
    }

    #[test]
    fn bad_files_are_refused_and_bad_rows_skipped() {
        let (node, svc) = (ServerId::new(), ServiceId::new());
        let m = maps(node, svc, "k");
        assert!(matches!(read(b"not zstd", node, Timestamp(0), &m), Err(FileError::Decode(_))));
        let mut enc = zstd::stream::write::Encoder::new(Vec::new(), 1).unwrap();
        enc.write_all(b"{\"v\":9,\"node\":\"x\",\"created\":0,\"counts\":{}}\n").unwrap();
        assert!(matches!(read(&enc.finish().unwrap(), node, Timestamp(0), &m), Err(FileError::Header(_))));
        let mut raw = file("x", &[app("k", "{\"@mt\":\"ok\"}")]);
        let mut extra = zstd::stream::write::Encoder::new(Vec::new(), 1).unwrap();
        extra.write_all(b"{\"k\":\"nope\"}\nnot json\n").unwrap();
        raw.extend(extra.finish().unwrap());
        let got = read(&raw, node, Timestamp(0), &m).unwrap();
        assert_eq!((got.rows(), got.skipped), (1, 2), "concatenated frames are read; bad rows skipped");
    }
}
