//! Reachability checks executed on the node itself, so results reflect the
//! node's own network path to its peers (not the control plane's).

use std::time::{Duration, Instant};

use serde_json::{Value, json};

pub const MAX_TARGETS: usize = 32;
const TCP_DEFAULT_PORT: u16 = 22;
const TIMEOUT: Duration = Duration::from_secs(2);

/// Splits `host`, `host:port` or `[v6]:port` and validates the host.
pub fn parse_target(t: &str) -> Result<(String, Option<u16>), String> {
    let (host, port) = if let Some(rest) = t.strip_prefix('[') {
        let (h, p) = rest.split_once(']').ok_or("bad IPv6 target")?;
        (h, p.strip_prefix(':'))
    } else if t.matches(':').count() == 1 {
        let (h, p) = t.split_once(':').ok_or("bad target")?;
        (h, Some(p))
    } else {
        (t, None)
    };

    let port = port.map(|p| p.parse::<u16>().map_err(|_| format!("bad port in `{t}`"))).transpose()?;

    if host.is_empty()
        || host.len() > 253
        || host.starts_with('-')
        || !host.bytes().all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'-' | b':' | b'_'))
    {
        return Err(format!("invalid target `{t}`"));
    }

    Ok((host.to_owned(), port))
}

#[derive(Debug, Default, PartialEq)]
struct PingStats {
    sent: u32,
    received: u32,
    avg_ms: Option<f64>,
}

fn leading_num(s: &str) -> Option<u32> {
    let d: String = s.trim().chars().take_while(char::is_ascii_digit).collect();
    d.parse().ok()
}

/// Parses iputils, busybox and Windows `ping` summaries.
fn parse_ping(out: &str) -> Option<PingStats> {
    let mut st = PingStats::default();

    for line in out.lines() {
        let l = line.trim();
        if l.contains("packets transmitted") {
            // "4 packets transmitted, 4 received, 0% packet loss"
            let mut parts = l.split(',');
            st.sent = leading_num(parts.next()?)?;
            st.received = leading_num(parts.next()?)?;
        } else if l.starts_with("Packets: Sent") {
            // Windows: "Packets: Sent = 4, Received = 4, Lost = 0 (0% loss),"
            let v: Vec<u32> = l.split('=').skip(1).filter_map(leading_num).collect();
            st.sent = *v.first()?;
            st.received = *v.get(1)?;
        } else if l.contains("min/avg/max") {
            // "rtt min/avg/max/mdev = 0.041/0.052/0.061/0.007 ms"
            st.avg_ms = l.split(" = ").nth(1).and_then(|s| s.split('/').nth(1)).and_then(|v| v.trim().parse().ok());
        } else if let Some(avg) = l.split("Average = ").nth(1) {
            st.avg_ms = avg.trim().trim_end_matches("ms").parse().ok();
        }
    }

    (st.sent > 0).then_some(st)
}

async fn icmp(host: &str, count: u32) -> Option<PingStats> {
    let mut cmd = tokio::process::Command::new("ping");

    if cfg!(windows) {
        cmd.args(["-n", &count.to_string(), "-w", "2000", host]);
    } else {
        cmd.args(["-c", &count.to_string(), "-i", "0.2", "-W", "2", "-q", host]);
    }

    cmd.env("LC_ALL", "C").kill_on_drop(true);
    let out = tokio::time::timeout(Duration::from_secs(u64::from(count) * 3 + 3), cmd.output()).await.ok()?.ok()?;

    parse_ping(&String::from_utf8_lossy(&out.stdout))
}

async fn tcp(host: &str, port: u16, count: u32) -> PingStats {
    let mut st = PingStats { sent: count, ..PingStats::default() };
    let mut total = 0.0;

    for _ in 0..count {
        let t0 = Instant::now();

        if let Ok(Ok(_)) = tokio::time::timeout(TIMEOUT, tokio::net::TcpStream::connect((host, port))).await {
            st.received += 1;
            total += t0.elapsed().as_secs_f64() * 1000.0;
        }
    }

    if st.received > 0 {
        st.avg_ms = Some(total / f64::from(st.received));
    }

    st
}

async fn one(target: String, count: u32) -> Value {
    let (host, port) = match parse_target(&target) {
        Ok(v) => v,
        Err(e) => return json!({ "target": target, "reachable": false, "error": e }),
    };

    let t0 = Instant::now();

    let (method, st) = match icmp(&host, count).await {
        // No ping binary, no permission, or ICMP filtered: try TCP.
        Some(st) if st.received > 0 => ("icmp", st),
        _ => ("tcp", tcp(&host, port.unwrap_or(TCP_DEFAULT_PORT), count).await),
    };

    let lost = st.sent - st.received.min(st.sent);
    let loss = if st.sent == 0 { 100.0 } else { f64::from(lost) * 100.0 / f64::from(st.sent) };

    json!({
        "target": target,
        "method": method,
        "reachable": st.received > 0,
        "sent": st.sent,
        "received": st.received,
        "loss_percent": (loss * 10.0).round() / 10.0,
        "latency_ms": st.avg_ms.map(|v| (v * 100.0).round() / 100.0),
        "elapsed_ms": u64::try_from(t0.elapsed().as_millis()).unwrap_or(u64::MAX),
    })
}

/// Probes every target concurrently.
pub async fn run(targets: &[String], count: u32) -> Result<Value, String> {
    if targets.is_empty() || targets.len() > MAX_TARGETS {
        return Err(format!("between 1 and {MAX_TARGETS} targets"));
    }

    let count = count.clamp(1, 10);
    let tasks: Vec<_> = targets.iter().map(|t| tokio::spawn(one(t.clone(), count))).collect();
    let mut results = Vec::with_capacity(tasks.len());

    for t in tasks {
        results.push(t.await.unwrap_or_else(|e| json!({ "reachable": false, "error": e.to_string() })));
    }

    Ok(json!({ "results": results }))
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    #[test]
    fn parses_linux_and_windows() {
        let l = "--- x ping statistics ---\n4 packets transmitted, 3 received, 25% packet loss, time 3003ms\nrtt min/avg/max/mdev = 0.041/0.052/0.061/0.007 ms\n";
        assert_eq!(parse_ping(l), Some(PingStats { sent: 4, received: 3, avg_ms: Some(0.052) }));
        let w = "    Packets: Sent = 4, Received = 4, Lost = 0 (0% loss),\n    Minimum = 1ms, Maximum = 2ms, Average = 1ms\n";
        assert_eq!(parse_ping(w), Some(PingStats { sent: 4, received: 4, avg_ms: Some(1.0) }));
        assert_eq!(parse_ping("ping: socket: Operation not permitted"), None);
    }

    #[test]
    fn validates_targets() {
        assert_eq!(parse_target("node-2:22").unwrap(), ("node-2".into(), Some(22)));
        assert_eq!(parse_target("10.0.0.1").unwrap(), ("10.0.0.1".into(), None));
        assert_eq!(parse_target("[fd00::1]:22").unwrap(), ("fd00::1".into(), Some(22)));
        assert!(parse_target("-c 100 x").is_err());
        assert!(parse_target("a;b").is_err());
    }

    #[tokio::test]
    async fn tcp_fallback_measures_a_local_listener() {
        let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = l.local_addr().unwrap().port();
        tokio::spawn(async move { while l.accept().await.is_ok() {} });
        let st = tcp("127.0.0.1", port, 2).await;
        assert_eq!((st.sent, st.received), (2, 2));
    }
}
