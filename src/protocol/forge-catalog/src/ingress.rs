//! Cluster ingress profiles: the Nginx layout (standalone, active-standby or
//! active-active) on a cluster's edge nodes.
//!
//! Shared by the control plane (validation, upstream resolution) and the node
//! helper (rendering the global Nginx config and keepalived). Addresses and
//! internal names come from the Forge cluster network (see `fabric`); there is
//! no container orchestrator.

use serde::{Deserialize, Serialize};

pub const GLOBAL_CONF: &str = "/etc/nginx/conf.d/forge-global.conf";
pub const KEEPALIVED_CONF: &str = "/etc/keepalived/keepalived.conf";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum IngressMode {
    #[default]
    Standalone,
    ActiveStandby,
    ActiveActive,
}

/// How clients find the edges.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum Failover {
    /// keepalived VRRP moves virtual IPs between edges (same L2 network).
    Vrrp,
    /// DNS records list (or are swapped to) the healthy edges. The default:
    /// it works for edges on public IPs in different networks.
    #[default]
    Dns,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum EdgeRole {
    #[default]
    Primary,
    Backup,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Edge {
    pub server_id: String,
    #[serde(default)]
    pub role: EdgeRole,
    /// The address clients reach this edge on (published in DNS).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub public_ip: Option<String>,
}

/// DNS records Forge keeps for the cluster's endpoints, on a PowerDNS node.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct IngressDns {
    /// PowerDNS node holding the zones.
    pub server_id: String,
    /// Record TTL in seconds; low values make failover quicker.
    #[serde(default = "dns_ttl")]
    pub ttl: u32,
    /// Health-check the edges and move records off failed ones.
    #[serde(default = "yes")]
    pub auto_failover: bool,
}

const fn dns_ttl() -> u32 {
    60
}
const fn yes() -> bool {
    true
}

/// Consecutive results needed to change an edge's health (no flapping).
pub const HEALTH_THRESHOLD: u32 = 3;

/// Health of one edge as seen by the control plane.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EdgeHealth {
    pub healthy: bool,
    /// Consecutive results that disagree with `healthy`.
    #[serde(default)]
    pub streak: u32,
    #[serde(default)]
    pub last_error: Option<String>,
    /// When `healthy` last changed (epoch ms).
    #[serde(default)]
    pub since: i64,
}

impl Default for EdgeHealth {
    fn default() -> Self {
        Self { healthy: true, streak: 0, last_error: None, since: 0 }
    }
}

impl EdgeHealth {
    /// Records one check; returns `true` when the edge's health flipped.
    pub fn observe(&mut self, ok: bool, error: Option<String>, now_ms: i64) -> bool {
        if !ok {
            self.last_error = error;
        }
        if ok == self.healthy {
            self.streak = 0;
            if ok {
                self.last_error = None;
            }
            return false;
        }
        self.streak += 1;
        if self.streak >= HEALTH_THRESHOLD {
            self.healthy = ok;
            self.streak = 0;
            self.since = now_ms;
            if ok {
                self.last_error = None;
            }
            return true;
        }
        false
    }
}

/// Global Nginx tuning, written once per edge (`http {}` level).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct NginxSettings {
    pub worker_connections: u32,
    pub keepalive_timeout_s: u32,
    pub proxy_read_timeout_s: u32,
    pub client_max_body_mb: u32,
    pub gzip: bool,
    pub server_tokens: bool,
    pub hsts: bool,
    /// Trust `X-Forwarded-For` / PROXY protocol from these CIDRs (an L4 balancer in front).
    pub real_ip_from: Vec<String>,
    /// Global requests/s per client IP (0 = off). Sites can add their own.
    pub rate_limit: u32,
    /// `access_log` format: `combined`, `json` or `off`.
    pub access_log: String,
    /// Free directives included at `http {}` level (checked with `nginx -t`).
    pub custom: String,
}

impl Default for NginxSettings {
    fn default() -> Self {
        Self {
            worker_connections: 4096,
            keepalive_timeout_s: 65,
            proxy_read_timeout_s: 300,
            client_max_body_mb: 25,
            gzip: true,
            server_tokens: false,
            hsts: true,
            real_ip_from: Vec::new(),
            rate_limit: 0,
            access_log: "combined".into(),
            custom: String::new(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Vrrp {
    /// Virtual IPs (`a.b.c.d` or `a.b.c.d/nn`). Active-standby uses one;
    /// active-active one per edge.
    pub vips: Vec<String>,
    /// Interface carrying the VIP (`eth0`).
    pub interface: String,
    /// VRRP router id base (1–255); one per VIP, counting up.
    pub router_id: u8,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct IngressSpec {
    pub mode: IngressMode,
    pub edges: Vec<Edge>,
    #[serde(default)]
    pub failover: Failover,
    #[serde(default)]
    pub vrrp: Option<Vrrp>,
    #[serde(default)]
    pub nginx: NginxSettings,
    /// Forge-managed DNS records for the endpoints (optional).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub dns: Option<IngressDns>,
}

/// A cluster's profile. Profiles saved before the orchestrator was removed
/// also carry `orchestrator` and `network`; those keys are ignored.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct ClusterProfile {
    #[serde(default)]
    pub ingress: Option<IngressSpec>,
}

pub(crate) fn valid_ipv4(s: &str) -> bool {
    let parts: Vec<&str> = s.split('.').collect();
    parts.len() == 4 && parts.iter().all(|p| !p.is_empty() && p.len() <= 3 && p.parse::<u8>().is_ok())
}

fn valid_cidr(s: &str, min: u8, max: u8) -> bool {
    let Some((ip, bits)) = s.split_once('/') else { return false };
    valid_ipv4(ip) && bits.parse::<u8>().is_ok_and(|b| (min..=max).contains(&b))
}

fn valid_vip(s: &str) -> bool {
    valid_ipv4(s) || valid_cidr(s, 8, 32)
}

fn valid_iface(s: &str) -> bool {
    !s.is_empty() && s.len() <= 15 && s.bytes().all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'-' | b'_'))
}

impl NginxSettings {
    pub fn validate(&self) -> Result<(), String> {
        if !(64..=1_000_000).contains(&self.worker_connections) {
            return Err("worker connections: 64–1000000".into());
        }
        if self.keepalive_timeout_s > 3600 || self.proxy_read_timeout_s == 0 || self.proxy_read_timeout_s > 86_400 {
            return Err("timeouts out of range".into());
        }
        if self.client_max_body_mb == 0 || self.client_max_body_mb > 10_240 {
            return Err("max body size: 1–10240 MB".into());
        }
        if let Some(c) = self.real_ip_from.iter().find(|c| !valid_ipv4(c) && !valid_cidr(c, 0, 32)) {
            return Err(format!("trusted proxy `{c}` is not an IPv4 address or CIDR"));
        }
        if !matches!(self.access_log.as_str(), "combined" | "json" | "off") {
            return Err("access log: combined, json or off".into());
        }
        crate::sites::check_custom_block(&self.custom)
    }

    /// `http {}` directives [`NginxSettings::render`] sets. Distributions set
    /// some of them in `nginx.conf` already (Debian: `gzip on`,
    /// `keepalive_timeout`), and nginx refuses duplicates.
    #[must_use]
    pub fn directives(&self) -> Vec<&'static str> {
        let rendered = self.render();
        [
            "keepalive_timeout",
            "proxy_read_timeout",
            "client_max_body_size",
            "server_tokens",
            "gzip",
            "gzip_proxied",
            "gzip_types",
            "access_log",
            "real_ip_header",
            "real_ip_recursive",
        ]
        .into_iter()
        .filter(|d| rendered.lines().any(|l| l.split_whitespace().next() == Some(d)))
        .collect()
    }

    /// `http {}`-level config shared by every site on the edge.
    #[must_use]
    pub fn render(&self) -> String {
        let mut o = String::from("# Managed by Orbyn Forge — cluster ingress profile. Edit it in the console.\n");
        o.push_str(&format!("keepalive_timeout {}s;\n", self.keepalive_timeout_s));
        o.push_str(&format!("proxy_read_timeout {}s;\n", self.proxy_read_timeout_s));
        o.push_str(&format!("client_max_body_size {}m;\n", self.client_max_body_mb));
        o.push_str(&format!("server_tokens {};\n", if self.server_tokens { "on" } else { "off" }));
        if self.gzip {
            o.push_str("gzip on;\ngzip_proxied any;\ngzip_types text/css application/javascript application/json image/svg+xml text/plain;\n");
        }
        if self.hsts {
            o.push_str("map $scheme $forge_hsts {\n    https \"max-age=31536000\";\n    default \"\";\n}\nadd_header Strict-Transport-Security $forge_hsts always;\n");
        }
        for c in &self.real_ip_from {
            o.push_str(&format!("set_real_ip_from {c};\n"));
        }
        if !self.real_ip_from.is_empty() {
            o.push_str("real_ip_header X-Forwarded-For;\nreal_ip_recursive on;\n");
        }
        if self.rate_limit > 0 {
            o.push_str(&format!("limit_req_zone $binary_remote_addr zone=forge_global:10m rate={}r/s;\nlimit_req zone=forge_global burst={} nodelay;\n", self.rate_limit, self.rate_limit * 2));
        }
        // Per-route throughput for the console (Ingress → Routing). Kept even
        // when the main access log is off; logrotate covers /var/log/nginx.
        o.push_str(TRAFFIC_LOG_CONF);
        match self.access_log.as_str() {
            // `access_log off` would cancel the traffic log too: point the
            // main log at /dev/null instead.
            "off" => o.push_str("access_log /dev/null;\n"),
            "json" => o.push_str(
                "log_format forge_json escape=json '{\"time\":\"$time_iso8601\",\"host\":\"$host\",\"remote\":\"$remote_addr\",\
                 \"request\":\"$request\",\"status\":$status,\"bytes\":$body_bytes_sent,\"upstream\":\"$upstream_addr\",\
                 \"rt\":$request_time}';\naccess_log /var/log/nginx/access.log forge_json;\n",
            ),
            _ => {}
        }
        if !self.custom.trim().is_empty() {
            o.push_str("# --- custom ---\n");
            o.push_str(self.custom.trim_end());
            o.push('\n');
        }
        o
    }
}

impl IngressSpec {
    pub fn validate(&self) -> Result<(), String> {
        self.nginx.validate()?;
        if self.edges.is_empty() {
            return Err("pick at least one edge node".into());
        }
        let mut seen = std::collections::HashSet::new();
        if self.edges.iter().any(|e| !seen.insert(&e.server_id)) {
            return Err("an edge node is listed twice".into());
        }
        match self.mode {
            IngressMode::Standalone if self.edges.len() != 1 => {
                return Err("standalone uses exactly one edge node".into());
            }
            IngressMode::ActiveStandby => {
                if self.edges.len() < 2 {
                    return Err("active-standby needs at least two edge nodes".into());
                }
                if self.edges.iter().filter(|e| e.role == EdgeRole::Primary).count() != 1 {
                    return Err("active-standby needs exactly one primary edge".into());
                }
            }
            IngressMode::ActiveActive if self.edges.len() < 2 => {
                return Err("active-active needs at least two edge nodes".into());
            }
            _ => {}
        }
        let bad_ip = |e: &&Edge| e.public_ip.as_deref().is_some_and(|i| i.parse::<std::net::IpAddr>().is_err());
        if let Some(e) = self.edges.iter().find(bad_ip) {
            return Err(format!("edge {}: invalid public IP", e.server_id));
        }
        if let Some(d) = &self.dns {
            if d.server_id.is_empty() {
                return Err("DNS: choose the PowerDNS node".into());
            }
            if !(30..=86_400).contains(&d.ttl) {
                return Err("DNS TTL: 30 s to 1 day".into());
            }
            if let Some(e) = self.edges.iter().find(|e| e.public_ip.is_none()) {
                return Err(format!("DNS: edge {} needs its public IP", e.server_id));
            }
        }
        if self.mode != IngressMode::Standalone && self.failover == Failover::Vrrp {
            let v = self.vrrp.as_ref().ok_or("VRRP failover needs a virtual IP and an interface")?;
            let need = if self.mode == IngressMode::ActiveStandby { 1 } else { self.edges.len() };
            if v.vips.len() != need {
                return Err(if need == 1 {
                    "active-standby uses exactly one virtual IP".into()
                } else {
                    format!("active-active with VRRP uses one virtual IP per edge ({need})")
                });
            }
            if let Some(ip) = v.vips.iter().find(|i| !valid_vip(i)) {
                return Err(format!("virtual IP `{ip}` is not a valid IPv4 address"));
            }
            if !valid_iface(&v.interface) {
                return Err("interface name is invalid".into());
            }
            if v.router_id == 0 || usize::from(v.router_id) + v.vips.len() > 256 {
                return Err("VRRP router id must be 1–255".into());
            }
        }
        Ok(())
    }

    /// Addresses DNS should list now. Standalone: its edge. Active-active:
    /// every healthy edge. Active-standby: the primary while healthy, else the
    /// first healthy backup. When nothing is healthy the records are left
    /// pointing at every edge (or the primary) rather than at nothing.
    #[must_use]
    pub fn dns_addresses(&self, healthy: impl Fn(&str) -> bool) -> Vec<String> {
        let ip = |e: &Edge| e.public_ip.clone();
        match self.mode {
            IngressMode::Standalone => self.edges.iter().take(1).filter_map(ip).collect(),
            IngressMode::ActiveActive => {
                let up: Vec<String> = self.edges.iter().filter(|e| healthy(&e.server_id)).filter_map(ip).collect();
                if up.is_empty() { self.edges.iter().filter_map(ip).collect() } else { up }
            }
            IngressMode::ActiveStandby => {
                let mut order: Vec<&Edge> = self.edges.iter().filter(|e| e.role == EdgeRole::Primary).collect();
                order.extend(self.edges.iter().filter(|e| e.role != EdgeRole::Primary));
                order
                    .iter()
                    .find(|e| healthy(&e.server_id))
                    .or_else(|| order.first())
                    .and_then(|e| ip(e))
                    .into_iter()
                    .collect()
            }
        }
    }

    /// Whether this profile runs keepalived.
    #[must_use]
    pub fn uses_vrrp(&self) -> bool {
        self.mode != IngressMode::Standalone && self.failover == Failover::Vrrp && self.vrrp.is_some()
    }

    /// keepalived config for edge `index`. `auth` is a per-cluster secret
    /// (first 8 characters are used, a VRRP limit).
    #[must_use]
    pub fn render_keepalived(&self, index: usize, auth: &str) -> Option<String> {
        let v = self.vrrp.as_ref().filter(|_| self.uses_vrrp())?;
        let edge = self.edges.get(index)?;
        let pass: String = auth.chars().filter(char::is_ascii_alphanumeric).take(8).collect();
        let mut o = String::from(
            "# Managed by Orbyn Forge — cluster ingress profile.\nglobal_defs {\n    enable_script_security\n    script_user root\n}\n\
             vrrp_script chk_nginx {\n    script \"/usr/bin/pgrep -x nginx\"\n    interval 2\n    fall 2\n    rise 2\n}\n",
        );
        for (n, vip) in v.vips.iter().enumerate() {
            // Active-standby: the primary owns the VIP. Active-active: edge n owns VIP n,
            // the others back it up with descending priority.
            let owner = match self.mode {
                IngressMode::ActiveStandby => edge.role == EdgeRole::Primary,
                _ => n == index,
            };
            let distance = if self.mode == IngressMode::ActiveStandby {
                usize::from(!owner) + index
            } else {
                (index + self.edges.len() - n) % self.edges.len()
            };
            let prio = 150usize.saturating_sub(distance * 10).max(1);
            let vid = usize::from(v.router_id) + n;
            o.push_str(&format!(
                "vrrp_instance FORGE_VIP_{n} {{\n    state {state}\n    interface {iface}\n    virtual_router_id {vid}\n    priority {prio}\n\
                 \x20   advert_int 1\n    authentication {{\n        auth_type PASS\n        auth_pass {pass}\n    }}\n\
                 \x20   virtual_ipaddress {{\n        {vip}\n    }}\n    track_script {{\n        chk_nginx\n    }}\n}}\n",
                state = if owner { "MASTER" } else { "BACKUP" },
                iface = v.interface,
            ));
        }
        Some(o)
    }
}

impl ClusterProfile {
    pub fn validate(&self) -> Result<(), String> {
        if let Some(i) = &self.ingress {
            i.validate()?;
        }
        Ok(())
    }
}

/// A typed upstream for an ingress endpoint, resolved to `host:port` by the
/// control plane before the site is rendered.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum Target {
    /// A native service on a node: the node's address.
    Node { server_id: String, port: u16 },
    /// A Forge service: every node it is placed on, at its cluster
    /// network address (expanded by the control plane from the placements).
    App { service_id: String, port: u16 },
    /// Any `host:port`.
    External { address: String },
}

impl Target {
    /// Resolves to `host:port`; `node_addr` maps a server id to its address.
    pub fn resolve(&self, node_addr: impl Fn(&str) -> Option<String>) -> Result<String, String> {
        match self {
            Self::Node { server_id, port } => {
                let host = node_addr(server_id).ok_or_else(|| format!("node {server_id} has no known address"))?;
                if *port == 0 {
                    return Err("port must be 1–65535".into());
                }
                Ok(if host.contains(':') { format!("[{host}]:{port}") } else { format!("{host}:{port}") })
            }
            Self::App { service_id, .. } => Err(format!("service {service_id} is resolved from its placements")),
            Self::External { address } => Ok(address.clone()),
        }
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    fn edges(n: usize) -> Vec<Edge> {
        (0..n)
            .map(|i| Edge {
                server_id: format!("s{i}"),
                role: if i == 0 { EdgeRole::Primary } else { EdgeRole::Backup },
                public_ip: Some(format!("203.0.113.{}", 10 + i)),
            })
            .collect()
    }

    fn spec(mode: IngressMode, n: usize, vips: &[&str]) -> IngressSpec {
        IngressSpec {
            mode,
            edges: edges(n),
            failover: Failover::Vrrp,
            vrrp: Some(Vrrp {
                vips: vips.iter().map(|s| (*s).to_owned()).collect(),
                interface: "eth0".into(),
                router_id: 51,
            }),
            nginx: NginxSettings::default(),
            dns: None,
        }
    }

    #[test]
    fn dns_follows_health_per_mode() {
        let mut s = spec(IngressMode::ActiveStandby, 3, &[]);
        s.failover = Failover::Dns;
        s.vrrp = None;
        assert_eq!(s.dns_addresses(|_| true), vec!["203.0.113.10"]);
        assert_eq!(s.dns_addresses(|id| id != "s0"), vec!["203.0.113.11"]);
        assert_eq!(s.dns_addresses(|_| false), vec!["203.0.113.10"], "nothing healthy: keep the primary");
        s.mode = IngressMode::ActiveActive;
        assert_eq!(s.dns_addresses(|id| id != "s1"), vec!["203.0.113.10", "203.0.113.12"]);
        assert_eq!(s.dns_addresses(|_| false).len(), 3);
        s.dns = Some(IngressDns { server_id: "dns1".into(), ttl: 60, auto_failover: true });
        s.validate().unwrap();
        s.edges[2].public_ip = None;
        assert!(s.validate().is_err(), "every edge needs a public IP for DNS");
    }

    #[test]
    fn health_needs_three_checks_in_a_row() {
        let mut h = EdgeHealth::default();
        assert!(!h.observe(false, Some("refused".into()), 1));
        assert!(!h.observe(false, None, 2));
        assert!(!h.observe(true, None, 3), "a success resets the streak");
        assert!(!h.observe(false, None, 4));
        assert!(!h.observe(false, None, 5));
        assert!(h.observe(false, None, 6));
        assert!(!h.healthy);
        assert_eq!(h.since, 6);
        assert!(!h.observe(true, None, 7));
        assert!(!h.observe(true, None, 8));
        assert!(h.observe(true, None, 9));
        assert!(h.healthy && h.last_error.is_none());
    }

    #[test]
    fn modes_are_validated() {
        assert!(spec(IngressMode::Standalone, 1, &[]).validate().is_ok());
        assert!(spec(IngressMode::Standalone, 2, &[]).validate().is_err());
        assert!(spec(IngressMode::ActiveStandby, 2, &["10.0.0.100"]).validate().is_ok());
        assert!(spec(IngressMode::ActiveStandby, 1, &["10.0.0.100"]).validate().is_err());
        assert!(spec(IngressMode::ActiveStandby, 2, &["10.0.0.100", "10.0.0.101"]).validate().is_err());
        assert!(spec(IngressMode::ActiveStandby, 2, &["10.0.0.300"]).validate().is_err());
        assert!(spec(IngressMode::ActiveActive, 2, &["10.0.0.100", "10.0.0.101"]).validate().is_ok());
        let mut dns = spec(IngressMode::ActiveActive, 3, &[]);
        dns.failover = Failover::Dns;
        dns.vrrp = None;
        assert!(dns.validate().is_ok());
        let mut two_primaries = spec(IngressMode::ActiveStandby, 2, &["10.0.0.100"]);
        two_primaries.edges[1].role = EdgeRole::Primary;
        assert!(two_primaries.validate().is_err());
    }

    #[test]
    fn keepalived_priorities() {
        let s = spec(IngressMode::ActiveStandby, 2, &["10.0.0.100/24"]);
        let a = s.render_keepalived(0, "s3cret!xyz").unwrap();
        let b = s.render_keepalived(1, "s3cret!xyz").unwrap();
        assert!(a.contains("state MASTER") && a.contains("priority 150") && a.contains("auth_pass s3cretxy"));
        assert!(b.contains("state BACKUP") && b.contains("priority 130"));
        assert!(a.contains("10.0.0.100/24") && a.contains("virtual_router_id 51"));

        let aa = spec(IngressMode::ActiveActive, 2, &["10.0.0.100", "10.0.0.101"]);
        let e0 = aa.render_keepalived(0, "k").unwrap();
        let e1 = aa.render_keepalived(1, "k").unwrap();
        assert_eq!(e0.matches("state MASTER").count(), 1);
        assert_eq!(e1.matches("state MASTER").count(), 1);
        assert!(e0.contains("virtual_router_id 52"));
        assert!(spec(IngressMode::Standalone, 1, &[]).render_keepalived(0, "k").is_none());
    }

    #[test]
    fn nginx_global_render_and_filters() {
        let mut n = NginxSettings {
            real_ip_from: vec!["10.0.0.0/8".into()],
            rate_limit: 20,
            access_log: "json".into(),
            ..Default::default()
        };
        n.validate().unwrap();
        let c = n.render();
        assert!(c.contains("set_real_ip_from 10.0.0.0/8;") && c.contains("rate=20r/s") && c.contains("forge_json"));
        n.custom = "load_module x.so;".into();
        assert!(n.validate().is_err());
        n.custom = String::new();
        n.real_ip_from = vec!["nope; evil".into()];
        assert!(n.validate().is_err());
    }

    #[test]
    fn legacy_profiles_still_load() {
        let p: ClusterProfile = serde_json::from_value(serde_json::json!({
            "orchestrator": "swarm",
            "network": { "name": "forge-lab", "subnet": "10.42.1.0/24", "encrypted": false },
            "ingress": null
        }))
        .unwrap();
        assert_eq!(p, ClusterProfile::default());
        assert!(!serde_json::to_string(&p).unwrap().contains("orchestrator"));
    }

    #[test]
    fn targets() {
        let addr = |id: &str| (id == "n1").then(|| "192.168.1.5".to_owned());
        assert_eq!(Target::Node { server_id: "n1".into(), port: 8080 }.resolve(addr).unwrap(), "192.168.1.5:8080");
        assert!(Target::Node { server_id: "n2".into(), port: 8080 }.resolve(addr).is_err());
        assert!(Target::App { service_id: "a".into(), port: 80 }.resolve(addr).is_err());
    }
}

/// Marker for `nginx.conf` lines Forge took over.
pub const TAKEN_OVER: &str = "# forge-managed: ";

/// Where every edge logs one line per proxied request, for throughput.
pub const TRAFFIC_LOG: &str = "/var/log/nginx/forge-traffic.log";
/// The traffic log line, quoted for `log_format`. Each Forge site defines its
/// own format from it, so edges without a cluster profile log traffic too.
pub const TRAFFIC_LOG_FORMAT: &str = "'$msec $server_name $upstream_addr $request_length $bytes_sent \
     $upstream_bytes_sent $upstream_bytes_received us=$upstream_status rt=$upstream_response_time'";
/// The JSON access log line, quoted for `log_format … escape=json`.
pub const JSON_LOG_FORMAT: &str = "'{\"time\":\"$time_iso8601\",\"host\":\"$host\",\"remote\":\"$remote_addr\",\
     \"request\":\"$request\",\"status\":$status,\"bytes\":$body_bytes_sent,\"upstream\":\"$upstream_addr\",\
     \"rt\":$request_time}'";
const TRAFFIC_LOG_CONF: &str = "log_format forge_traffic '$msec $server_name $upstream_addr $request_length $bytes_sent \
     $upstream_bytes_sent $upstream_bytes_received us=$upstream_status rt=$upstream_response_time';\n\
     access_log /var/log/nginx/forge-traffic.log forge_traffic;\n";

/// Traffic through one upstream (bytes per second over the window).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct UpstreamTraffic {
    pub requests: u64,
    /// Edge → upstream (request bodies and headers).
    pub sent_bps: f64,
    /// Upstream → edge (responses).
    pub received_bps: f64,
    /// Attempts answered with a 5xx status (or none: connection failed).
    #[serde(default)]
    pub errors: u64,
    /// 95th percentile of the upstream's response time, ms (0: not logged).
    #[serde(default)]
    pub p95_ms: u64,
}

/// Traffic of one endpoint (by its first domain) on one edge.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct SiteTraffic {
    pub requests: u64,
    pub rps: f64,
    /// Clients → edge.
    pub in_bps: f64,
    /// Edge → clients.
    pub out_bps: f64,
    pub upstreams: std::collections::BTreeMap<String, UpstreamTraffic>,
}

/// A tagged list from the traffic log (`us=200,` `502`) without its tag and commas.
fn tagged<'a>(xs: &[&'a str], tag: &str) -> Vec<&'a str> {
    xs.iter().map(|x| x.strip_prefix(tag).unwrap_or(x).trim_matches(',')).collect()
}

/// Sums the traffic log lines newer than `now - window` per endpoint and
/// upstream. Lines: `$msec $server_name $upstream_addr $request_length
/// $bytes_sent $upstream_bytes_sent $upstream_bytes_received
/// us=$upstream_status rt=$upstream_response_time`; on retries the upstream
/// fields are lists (`a:1, b:2`), each attempt counted separately. Lines
/// written before the status and time were logged have no `us=`/`rt=` part.
#[must_use]
pub fn traffic(log: &str, now: f64, window: f64) -> std::collections::BTreeMap<String, SiteTraffic> {
    let mut out: std::collections::BTreeMap<String, SiteTraffic> = std::collections::BTreeMap::new();
    let mut times: std::collections::BTreeMap<(String, String), Vec<u64>> = std::collections::BTreeMap::new();
    let num = |s: &str| s.trim_matches(',').parse::<f64>().unwrap_or(0.0);
    for line in log.lines() {
        let all: Vec<&str> = line.split(' ').filter(|x| !x.is_empty()).collect();
        let us_at = all.iter().position(|x| x.starts_with("us="));
        let rt_at = all.iter().position(|x| x.starts_with("rt="));
        let (f, statuses, rts): (&[&str], Vec<&str>, Vec<&str>) = match (us_at, rt_at) {
            (Some(u), Some(r)) if u < r => (&all[..u], tagged(&all[u..r], "us="), tagged(&all[r..], "rt=")),
            _ => (&all[..], Vec::new(), Vec::new()),
        };
        if f.len() < 7 {
            continue;
        }
        let t = num(f[0]);
        if t < now - window || t > now + 5.0 {
            continue;
        }
        let site = out.entry(f[1].to_owned()).or_default();
        site.requests += 1;
        // The three upstream fields are comma-separated lists of equal length;
        // the request fields are the last two before them.
        let rest = &f[2..];
        let n = (rest.len() - 2) / 3;
        if n == 0 || rest.len() != 2 + 3 * n {
            continue;
        }
        let (addrs, tail) = rest.split_at(n);
        site.in_bps += num(tail[0]);
        site.out_bps += num(tail[1]);
        for (k, a) in addrs.iter().enumerate() {
            let a = a.trim_matches(',');
            if a == "-" || a.is_empty() {
                continue;
            }
            let u = site.upstreams.entry(a.to_owned()).or_default();
            u.requests += 1;
            u.sent_bps += num(tail[2 + k]);
            u.received_bps += num(tail[2 + n + k]);
            if let Some(st) = statuses.get(k) {
                // `502`, or `-` when no answer came (connection failed).
                if st.starts_with('5') || *st == "-" {
                    u.errors += 1;
                }
            }
            if let Some(ms) = rts.get(k).and_then(|t| t.parse::<f64>().ok()) {
                times.entry((f[1].to_owned(), a.to_owned())).or_default().push((ms * 1000.0).round() as u64);
            }
        }
    }
    for ((site, addr), mut ts) in times {
        ts.sort_unstable();
        let at = (ts.len() * 95).div_ceil(100).saturating_sub(1);
        if let Some(u) = out.get_mut(&site).and_then(|s| s.upstreams.get_mut(&addr)) {
            u.p95_ms = ts.get(at).copied().unwrap_or(0);
        }
    }
    for s in out.values_mut() {
        s.rps = s.requests as f64 / window;
        s.in_bps /= window;
        s.out_bps /= window;
        for u in s.upstreams.values_mut() {
            u.sent_bps /= window;
            u.received_bps /= window;
        }
    }
    out
}

/// Comments out `names` directly inside the `http {}` block of `nginx.conf`
/// (they are set in Forge's global config instead). Returns the new text.
#[must_use]
pub fn take_over(conf: &str, names: &[&str]) -> String {
    let mut depth = 0i32;
    let mut in_http = false;
    let mut out = String::with_capacity(conf.len() + 128);
    for line in conf.split_inclusive('\n') {
        let t = line.trim_start();
        let first = t.split(|c: char| c.is_whitespace() || c == ';').next().unwrap_or("");
        let code = t.split('#').next().unwrap_or("");
        if depth == 0 && first == "http" {
            in_http = true;
        }
        if in_http && depth == 1 && !t.starts_with('#') && names.contains(&first) {
            let indent = &line[..line.len() - t.len()];
            out.push_str(indent);
            out.push_str(TAKEN_OVER);
            out.push_str(t);
        } else {
            out.push_str(line);
        }
        depth += code.matches('{').count() as i32 - code.matches('}').count() as i32;
        if in_http && depth == 0 && code.contains('}') {
            in_http = false;
        }
    }
    out
}

/// Undoes [`take_over`].
#[must_use]
pub fn give_back(conf: &str) -> String {
    conf.split_inclusive('\n').map(|l| l.replacen(TAKEN_OVER, "", 1)).collect()
}

#[cfg(test)]
mod take_over_tests {
    use super::*;

    const DEBIAN: &str = "user www-data;\nevents {\n\tworker_connections 768;\n}\nhttp {\n\tsendfile on;\n\tkeepalive_timeout 65;\n\tgzip on;\n\t# gzip_proxied any;\n\tserver {\n\t\tgzip off;\n\t}\n\tinclude /etc/nginx/conf.d/*.conf;\n}\n";

    #[test]
    fn comments_only_top_level_http_duplicates_and_restores() {
        let n = NginxSettings::default();
        let names = n.directives();
        assert!(names.contains(&"gzip") && names.contains(&"keepalive_timeout"));
        let t = take_over(DEBIAN, &names);
        assert!(t.contains("\t# forge-managed: gzip on;\n"));
        assert!(t.contains("\t# forge-managed: keepalive_timeout 65;\n"));
        assert!(t.contains("\t\tgzip off;\n"), "nested blocks untouched");
        assert!(t.contains("\tsendfile on;\n") && t.contains("\t# gzip_proxied any;\n"));
        assert_eq!(take_over(&t, &names), t, "idempotent");
        assert_eq!(give_back(&t), DEBIAN);
    }
}

/// Node-side ingress operations (run by the root helper).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case", deny_unknown_fields)]
pub enum IngressOp {
    /// Writes the global Nginx config (and keepalived when `keepalived` is
    /// set), runs `nginx -t`, reloads; restores the previous files on failure.
    Apply {
        nginx: Box<NginxSettings>,
        #[serde(default)]
        keepalived: Option<Box<KeepalivedNode>>,
    },
    /// Removes the global config and stops keepalived.
    Remove,
    /// Nginx / keepalived state and the VIPs held by this node.
    Status {
        #[serde(default)]
        vips: Vec<String>,
    },
}

/// What one edge needs to render its keepalived config.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct KeepalivedNode {
    pub spec: IngressSpec,
    pub index: usize,
    /// VRRP PASS authentication (sent in clear by VRRP itself; not a secret
    /// of any value beyond keeping other routers out of the group).
    pub auth: String,
}

impl IngressOp {
    pub fn validate(&self) -> Result<(), String> {
        match self {
            Self::Apply { nginx, keepalived } => {
                nginx.validate()?;
                if let Some(k) = keepalived {
                    k.spec.validate()?;
                    if k.index >= k.spec.edges.len() || !k.spec.uses_vrrp() {
                        return Err("keepalived: invalid edge index or no VRRP in this profile".into());
                    }
                }
                Ok(())
            }
            Self::Remove => Ok(()),
            Self::Status { vips } => {
                if vips.len() > 32 || vips.iter().any(|v| !valid_vip(v)) {
                    return Err("invalid virtual IPs".into());
                }
                Ok(())
            }
        }
    }

    /// Ops that only read state.
    #[must_use]
    pub const fn is_read(&self) -> bool {
        matches!(self, Self::Status { .. })
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod traffic_tests {

    #[test]
    fn traffic_counts_errors_and_latency_per_upstream() {
        let log = "100.0 shop a:1 10 20 30 40 us=200 rt=0.010
100.5 shop a:1 10 20 30 40 us=502 rt=0.200
101.0 shop a:1, b:2 10 20 30, 31 40, 41 us=502, 200 rt=0.300, 0.020
101.5 shop b:2 10 20 30 40 us=- rt=-
102.0 shop b:2 10 20 30 40
";
        let t = super::traffic(log, 110.0, 60.0);
        let shop = &t["shop"];
        assert_eq!(shop.requests, 5);
        let (a, b) = (&shop.upstreams["a:1"], &shop.upstreams["b:2"]);
        assert_eq!((a.requests, a.errors, a.p95_ms), (3, 2, 300));
        // A failed connection (`-`) counts as an error; an old line counts as a request only.
        assert_eq!((b.requests, b.errors, b.p95_ms), (3, 1, 20));
    }

    use super::*;

    #[test]
    fn traffic_rates_per_site_and_upstream() {
        let log = "\
1000.000 py.lab.local 10.64.0.16:8101 400 2400 420 2300
1010.500 py.lab.local 10.64.0.17:8101 600 3600 610 3500
1015.000 php.lab.local 10.64.0.16:8102 100 1000 120 900
1020.000 py.lab.local 10.64.0.16:8101, 10.64.0.17:8101 200 1200 0, 210 0, 1100
1030.000 py.lab.local - 300 150 - -
900.000 py.lab.local 10.64.0.16:8101 99999 99999 99999 99999
garbage line
";
        let t = traffic(log, 1040.0, 60.0);
        let py = &t["py.lab.local"];
        assert_eq!(py.requests, 4, "old lines are outside the window");
        assert!((py.in_bps - (400.0 + 600.0 + 200.0 + 300.0) / 60.0).abs() < 1e-9);
        assert!((py.out_bps - (2400.0 + 3600.0 + 1200.0 + 150.0) / 60.0).abs() < 1e-9);
        let a = &py.upstreams["10.64.0.16:8101"];
        assert_eq!(a.requests, 2);
        assert!((a.received_bps - (2300.0 + 0.0) / 60.0).abs() < 1e-9);
        let b = &py.upstreams["10.64.0.17:8101"];
        assert_eq!(b.requests, 2, "the retry counts for the second upstream");
        assert!((b.received_bps - (3500.0 + 1100.0) / 60.0).abs() < 1e-9);
        assert_eq!(t["php.lab.local"].requests, 1);
    }

    #[test]
    fn global_config_keeps_the_traffic_log_when_the_access_log_is_off() {
        let n = NginxSettings { access_log: "off".into(), ..NginxSettings::default() };
        let r = n.render();
        assert!(r.contains("access_log /var/log/nginx/forge-traffic.log forge_traffic;"));
        assert!(!r.contains("access_log off"));
    }
}
