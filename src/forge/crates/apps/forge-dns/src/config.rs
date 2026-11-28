//! Command line (`--file`, `--listen`, `--upstream`, `--ttl`, each also
//! `FORGE_DNS_*`) and the upstream default from `/etc/resolv.conf`.

use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::path::PathBuf;

use clap::Parser;

/// systemd-resolved's stub, skipped unless it is the only nameserver.
const RESOLVED_STUB: IpAddr = IpAddr::V4(Ipv4Addr::new(127, 0, 0, 53));

/// Where the system's nameservers are listed.
pub const RESOLV_CONF: &str = "/etc/resolv.conf";

/// `forge-dns` flags.
#[derive(Debug, Parser)]
#[command(name = "forge-dns", version, about = "Orbyn Forge cluster DNS")]
pub struct Args {
    /// The `names.json` record file.
    #[arg(long, env = "FORGE_DNS_FILE")]
    pub file: PathBuf,
    /// Address to serve UDP and TCP on; repeatable (env: comma-separated).
    #[arg(long, env = "FORGE_DNS_LISTEN", value_delimiter = ',', required = true)]
    pub listen: Vec<SocketAddr>,
    /// Forwarder for names outside the zones, tried in order; repeatable
    /// (env: comma-separated). Default: the nameservers in /etc/resolv.conf.
    #[arg(long, env = "FORGE_DNS_UPSTREAM", value_delimiter = ',')]
    pub upstream: Vec<SocketAddr>,
    /// TTL in seconds for every answer; default the file's `ttl`.
    #[arg(long, env = "FORGE_DNS_TTL")]
    pub ttl: Option<u32>,
}

/// The nameservers of a `resolv.conf` text, port 53, minus the `listen`
/// addresses and minus `127.0.0.53` unless it is the only one.
#[must_use]
pub fn default_upstreams(resolv_conf: &str, listen: &[SocketAddr]) -> Vec<SocketAddr> {
    let all: Vec<SocketAddr> = resolv_conf
        .lines()
        .filter_map(|line| {
            let mut words = line.split_whitespace();

            if words.next() != Some("nameserver") {
                return None;
            }

            // Drop an IPv6 zone index (`fe80::1%eth0`).
            let ip = words.next()?.split('%').next()?;

            ip.parse::<IpAddr>().ok().map(|ip| SocketAddr::new(ip, 53))
        })
        .filter(|a| !listen.contains(a))
        .collect();

    if all.len() == 1 {
        return all;
    }

    all.into_iter().filter(|a| a.ip() != RESOLVED_STUB).collect()
}

#[cfg(test)]
#[allow(clippy::unwrap_used)] // Reason: test assertions.
mod tests {
    use super::*;

    fn addr(s: &str) -> SocketAddr {
        s.parse().unwrap()
    }

    #[test]
    fn upstreams_skip_listen_and_the_stub_unless_alone() {
        let conf = "# comment\nnameserver 127.0.0.53\nnameserver 192.0.2.1\nnameserver 10.64.0.1\nsearch c1.local\n";

        assert_eq!(default_upstreams(conf, &[addr("10.64.0.1:53")]), vec![addr("192.0.2.1:53")]);
        assert_eq!(default_upstreams("nameserver 127.0.0.53\n", &[]), vec![addr("127.0.0.53:53")]);
        assert_eq!(default_upstreams("nameserver fe80::1%eth0\n", &[]), vec![addr("[fe80::1]:53")]);
    }
}
