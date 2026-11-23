//! The control protocol between forge-server and `forge-analytics-worker`
//! (D-141): length-prefixed frames carrying typed JSON messages and opaque
//! payloads, the worker's configuration, and the analytics settings
//! contract both sides share.
//!
//! The transport is a Unix socket (mode 0600) in the server's data
//! directory, or loopback TCP where Unix sockets are unavailable. Every
//! connection starts with [`messages::Request::Hello`] and the secret
//! forge-server handed the worker on its stdin.

pub mod config;
pub mod frame;
pub mod messages;
pub mod socket_path;
pub mod tuning;
pub mod views;

pub use frame::{FrameError, Limits, REQUEST_LIMITS, RESPONSE_LIMITS, read_message, write_message};
pub use messages::{PROTOCOL, Reply, Request, Response, WorkerError};

#[cfg(test)]
#[allow(clippy::unwrap_used)] // reason: tests
mod tests {
    use super::*;
    use forge_core::analytics::{AnalyticsMaps, AppKey, Placement};
    use forge_core::logs::{LogFilter, LogPage, LogQuery, SearchLimits};
    use forge_core::{ServerId, ServiceId, Timestamp};

    async fn round_trip<T>(msg: &T, blob: &[u8], limits: Limits) -> (T, Vec<u8>)
    where
        T: serde::Serialize + serde::de::DeserializeOwned,
    {
        let (mut a, mut b) = tokio::io::duplex(1 << 20);
        write_message(&mut a, msg, blob).await.unwrap();
        read_message(&mut b, limits).await.unwrap()
    }

    fn query() -> LogQuery {
        LogQuery {
            services: vec![ServiceId::new()],
            instances: vec![(ServiceId::new(), ServerId::new())],
            open_nodes: vec![],
            nodes: vec![ServerId::new()],
            container: Some("web".into()),
            stack: None,
            source: None,
            levels: vec![forge_core::logs::LogLevel::Error],
            filter: LogFilter::parse(r#"boom Name="x y""#),
            from: Some(Timestamp(1)),
            to: Some(Timestamp(2)),
            page: LogPage::Older(None),
            limit: 100,
            limits: SearchLimits::default(),
        }
    }

    #[tokio::test]
    async fn every_request_and_answer_round_trips() {
        let server = ServerId::new();
        let maps = AnalyticsMaps {
            active_nodes: vec![server],
            key_services: vec![],
            app_keys: vec![AppKey { server_id: server, app: "prod/api".into(), service_id: ServiceId::new() }],
            placements: vec![Placement { service_id: ServiceId::new(), server_id: server, names: vec!["n1".into()] }],
            node_certs: vec![forge_core::analytics::NodeCert { server_id: server, serial: "0a1b".into() }],
        };
        let requests = vec![
            Request::Hello { secret: "s".into(), protocol: PROTOCOL },
            Request::Ping,
            Request::PushMaps(maps),
            Request::Report,
            Request::RestartEngine,
            Request::SetAgentTls(crate::config::AgentTls { cert_pem: "c".into(), key_pem: "k".into() }),
            Request::IngestLogs { service: ServiceId::new(), now: Timestamp(5) },
            Request::Telemetry { server, now: Timestamp(5), metrics_len: 3 },
            Request::QueryLogs { query: query(), now: Timestamp(9) },
            Request::LogHistogram { query: query(), now: Timestamp(9) },
            Request::ContainerHistory { server, container: Some("db".into()), from: 1, to: 2, limit: 3 },
        ];
        for r in requests {
            let (back, blob) = round_trip(&r, b"abc\n\0", REQUEST_LIMITS).await;
            assert_eq!(back, r);
            assert_eq!(blob, b"abc\n\0");
        }
        let answers: Vec<Response> = vec![
            Ok(Reply::Ok),
            Ok(Reply::TelemetryAck { ack: 7 }),
            Ok(Reply::Ingested { count: 3 }),
            Err(WorkerError::Busy),
            Err(WorkerError::Unavailable { reason: "down".into() }),
            Err(WorkerError::ProtocolMismatch { expected: 1, got: 2 }),
        ];
        for a in answers {
            let (back, blob) = round_trip(&a, &[], RESPONSE_LIMITS).await;
            assert_eq!(back, a);
            assert!(blob.is_empty());
        }
    }

    #[tokio::test]
    async fn oversized_frames_are_refused_before_reading_them() {
        let (mut a, mut b) = tokio::io::duplex(1 << 16);
        write_message(&mut a, &Request::Ping, &[0u8; 10]).await.unwrap();
        let tiny = Limits { header: 1024, blob: 4 };
        let e = read_message::<_, Request>(&mut b, tiny).await.unwrap_err();
        assert!(matches!(e, FrameError::TooLarge { what: "blob", len: 10, max: 4 }), "{e}");
        drop(a);
        let (a, mut b) = tokio::io::duplex(64);
        drop(a);
        assert!(matches!(read_message::<_, Request>(&mut b, REQUEST_LIMITS).await, Err(FrameError::Closed)));
    }
}
