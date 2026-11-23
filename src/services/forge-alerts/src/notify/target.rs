//! Where a notification may be sent (#11, D-110).
//!
//! A channel's URL is chosen by a person and called by the control plane, so
//! it must not reach what only the control plane can reach. These rules are
//! applied twice: to the URL when a channel is saved, and to every address
//! the host name resolves to when a message is sent (the transport connects
//! only to an address that passed, so a name cannot change its answer
//! between the check and the connection).

use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};

use forge_catalog::health::{canonical, forbidden_address, forbidden_name, numeric_host, transition_address};
use forge_core::notifications::MAX_URL;

/// What the operator allows beyond public HTTPS (`[alerts]` in the server's
/// configuration; all off by default).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct TargetRules {
    /// `http://` targets. Messages then travel unencrypted.
    pub allow_http: bool,
    /// Private networks: RFC 1918, carrier-grade NAT, IPv6 unique local.
    pub allow_private: bool,
    /// The control plane's own loopback interface (tests and labs).
    pub allow_loopback: bool,
}

/// Why a target is refused.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum TargetError {
    #[error("the URL is not valid: write https://host[:port]/path")]
    Malformed,
    #[error("the URL is longer than {MAX_URL} bytes")]
    TooLong,
    #[error("only https:// targets are allowed (an operator can allow http:// with [alerts] webhook_allow_http)")]
    Scheme,
    #[error("the URL must not carry a user name, a password or a fragment")]
    Credentials,
    #[error("write the IP address in its normal form, or use a host name")]
    NumericHost,
    #[error("link-local, metadata, multicast and reserved addresses are never called")]
    Forbidden,
    #[error("loopback targets are not allowed")]
    Loopback,
    #[error("private network targets are not allowed (an operator can allow them with [alerts] webhook_allow_private)")]
    Private,
}

/// A target that passed the rules.
#[derive(Clone, PartialEq, Eq)]
pub struct Target {
    pub url: url::Url,
    /// `scheme://host[:port]`: what people may see of it.
    pub display: String,
}

impl std::fmt::Debug for Target {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // The path and query can carry a token.
        f.debug_struct("Target").field("display", &self.display).finish_non_exhaustive()
    }
}

fn private_v4(a: Ipv4Addr) -> bool {
    let o = a.octets();
    a.is_private()
        // 100.64.0.0/10: carrier-grade NAT (also used by overlay networks).
        || (o[0] == 100 && (o[1] & 0xc0) == 64)
        // 198.18.0.0/15: benchmarking between private networks.
        || (o[0] == 198 && (o[1] & 0xfe) == 18)
}

/// Never routable to a real receiver: "this network", protocol assignments,
/// documentation and reserved space.
fn reserved_v4(a: Ipv4Addr) -> bool {
    let o = a.octets();
    o[0] == 0
        || o[0] >= 240
        || (o[0] == 192 && o[1] == 0 && o[2] == 0)
        || (o[0] == 192 && o[1] == 0 && o[2] == 2)
        || (o[0] == 198 && o[1] == 51 && o[2] == 100)
        || (o[0] == 203 && o[1] == 0 && o[2] == 113)
}

fn private_v6(a: Ipv6Addr) -> bool {
    let s = a.segments()[0];
    // fc00::/7 unique local, fec0::/10 site-local (deprecated).
    (s & 0xfe00) == 0xfc00 || (s & 0xffc0) == 0xfec0
}

/// Checks one address. The address is first reduced to the IPv4 address it
/// embeds, if any, so `::ffff:10.0.0.1` is judged as `10.0.0.1`.
pub fn check_address(ip: IpAddr, rules: TargetRules) -> Result<(), TargetError> {
    // 6to4, Teredo and translation prefixes name an IPv4 destination that a
    // relay reaches for us: refused whole, whatever they embed.
    if transition_address(ip) {
        return Err(TargetError::Forbidden);
    }
    let ip = canonical(ip);
    if forbidden_address(ip) {
        return Err(TargetError::Forbidden);
    }
    if ip.is_loopback() {
        return if rules.allow_loopback { Ok(()) } else { Err(TargetError::Loopback) };
    }
    let (private, reserved) = match ip {
        IpAddr::V4(a) => (private_v4(a), reserved_v4(a)),
        // 2001:db8::/32 is documentation space.
        IpAddr::V6(a) => (private_v6(a), a.segments()[0] == 0x2001 && a.segments()[1] == 0x0db8),
    };
    if reserved {
        return Err(TargetError::Forbidden);
    }
    if private && !rules.allow_private {
        return Err(TargetError::Private);
    }
    Ok(())
}

/// Whether the transport may connect to `ip`.
#[must_use]
pub fn address_allowed(ip: IpAddr, rules: TargetRules) -> bool {
    check_address(ip, rules).is_ok()
}

/// Parses and checks a channel URL. Host names are checked by name here and
/// by address when a message is sent.
pub fn parse_target(raw: &str, rules: TargetRules) -> Result<Target, TargetError> {
    let raw = raw.trim();
    if raw.len() > MAX_URL {
        return Err(TargetError::TooLong);
    }
    let url = url::Url::parse(raw).map_err(|_| TargetError::Malformed)?;
    match url.scheme() {
        "https" => {}
        "http" if rules.allow_http => {}
        _ => return Err(TargetError::Scheme),
    }
    if !url.username().is_empty() || url.password().is_some() || url.fragment().is_some() {
        return Err(TargetError::Credentials);
    }
    let host = match url.host().ok_or(TargetError::Malformed)? {
        url::Host::Ipv4(a) => {
            check_address(IpAddr::V4(a), rules)?;
            a.to_string()
        }
        url::Host::Ipv6(a) => {
            check_address(IpAddr::V6(a), rules)?;
            format!("[{a}]")
        }
        url::Host::Domain(d) => {
            let name = d.trim_end_matches('.').to_ascii_lowercase();
            if name.is_empty() {
                return Err(TargetError::Malformed);
            }
            if numeric_host(&name) {
                return Err(TargetError::NumericHost);
            }
            if forbidden_name(&name) {
                return Err(TargetError::Forbidden);
            }
            if (name == "localhost" || name.ends_with(".localhost")) && !rules.allow_loopback {
                return Err(TargetError::Loopback);
            }
            name
        }
    };
    let display = match url.port() {
        Some(p) => format!("{}://{host}:{p}", url.scheme()),
        None => format!("{}://{host}", url.scheme()),
    };
    Ok(Target { url, display })
}

/// Checks the SMTP server of an email channel with the same rules as a
/// webhook host, and returns it normalized (lower case; an IPv6 address
/// without brackets). Like a URL's host, a name is checked by name here and
/// by every address it resolves to when a message is sent.
pub fn parse_smtp_host(raw: &str, port: u16, rules: TargetRules) -> Result<String, TargetError> {
    let raw = raw.trim();
    // Only what a host name or an address literal is made of: nothing that
    // would add a path, a user or a port to the URL checked below.
    if raw.is_empty()
        || raw.len() > forge_core::notifications::MAX_SMTP_HOST
        || !raw.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | ':' | '[' | ']'))
    {
        return Err(TargetError::Malformed);
    }
    let bracketed = if raw.contains(':') && !raw.starts_with('[') { format!("[{raw}]") } else { raw.to_owned() };
    let target = parse_target(&format!("https://{bracketed}:{port}/"), rules)?;
    let host = match target.url.host() {
        Some(url::Host::Ipv6(a)) => a.to_string(),
        Some(url::Host::Ipv4(a)) => a.to_string(),
        Some(url::Host::Domain(d)) => d.trim_end_matches('.').to_ascii_lowercase(),
        None => return Err(TargetError::Malformed),
    };
    Ok(host)
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]
    use super::*;

    #[test]
    fn smtp_hosts_follow_the_webhook_rules() {
        assert_eq!(parse_smtp_host(" SMTP.Example.com ", 587, STRICT).unwrap(), "smtp.example.com");
        assert_eq!(parse_smtp_host("2606:4700::1111", 465, STRICT).unwrap(), "2606:4700::1111");
        assert_eq!(parse_smtp_host("[2606:4700::1111]", 465, STRICT).unwrap(), "2606:4700::1111");
        for bad in ["", "smtp.example.com/x", "user@smtp.example.com", "smtp.example.com:25", "smtp example.com"] {
            assert!(parse_smtp_host(bad, 25, LAB).is_err(), "{bad}");
        }
        assert_eq!(parse_smtp_host("169.254.169.254", 25, LAB), Err(TargetError::Forbidden));
        assert_eq!(parse_smtp_host("metadata.google.internal", 25, LAB), Err(TargetError::Forbidden));
        assert_eq!(parse_smtp_host("127.0.0.1", 25, STRICT), Err(TargetError::Loopback));
        assert_eq!(parse_smtp_host("localhost", 25, STRICT), Err(TargetError::Loopback));
        assert_eq!(parse_smtp_host("10.0.0.5", 25, STRICT), Err(TargetError::Private));
        assert_eq!(parse_smtp_host("10.0.0.5", 25, ONPREM).unwrap(), "10.0.0.5");
        assert_eq!(parse_smtp_host("127.0.0.1", 2525, LAB).unwrap(), "127.0.0.1");
    }

    const STRICT: TargetRules = TargetRules { allow_http: false, allow_private: false, allow_loopback: false };
    const ONPREM: TargetRules = TargetRules { allow_http: true, allow_private: true, allow_loopback: false };
    const LAB: TargetRules = TargetRules { allow_http: true, allow_private: true, allow_loopback: true };

    fn ip(s: &str) -> IpAddr {
        s.parse().unwrap()
    }

    #[test]
    fn metadata_and_link_local_are_refused_under_every_rule_set() {
        for a in [
            "169.254.169.254",
            "169.254.0.1",
            "0.0.0.0",
            "224.0.0.1",
            "255.255.255.255",
            "fe80::1",
            "fd00:ec2::254",
            "::",
            "ff02::1",
            // The same addresses written as IPv6.
            "::ffff:169.254.169.254",
            "::a9fe:a9fe",
            "64:ff9b::a9fe:a9fe",
            // IPv6 transition prefixes, whatever IPv4 address they carry.
            "2002:a9fe:a9fe::1",
            "2002:0a00:0005::1",
            "2002:0101:0101::1",
            "2001:0:4136:e378:8000:63bf:3fff:fdd2",
            "64:ff9b:1::a9fe:a9fe",
            "64:ff9b:1::0a00:0005",
            "::ffff:0:a9fe:a9fe",
            "::ffff:0:7f00:1",
            // Reserved and documentation space.
            "0.1.2.3",
            "240.0.0.1",
            "192.0.2.10",
            "2001:db8::1",
        ] {
            for rules in [STRICT, ONPREM, LAB] {
                assert_eq!(check_address(ip(a), rules), Err(TargetError::Forbidden), "{a} {rules:?}");
            }
        }
    }

    #[test]
    fn loopback_and_private_ranges_need_their_setting() {
        for a in ["127.0.0.1", "127.8.9.10", "::1", "::ffff:127.0.0.1", "64:ff9b::7f00:1"] {
            assert_eq!(check_address(ip(a), STRICT), Err(TargetError::Loopback), "{a}");
            assert_eq!(check_address(ip(a), ONPREM), Err(TargetError::Loopback), "{a}: private does not mean loopback");
            assert!(address_allowed(ip(a), LAB), "{a}");
        }
        for a in [
            "10.0.0.5",
            "172.16.0.1",
            "172.31.255.254",
            "192.168.1.10",
            "100.64.0.1",
            "198.18.0.1",
            "fd12:3456::1",
            "fc00::1",
            "fec0::1",
            "::ffff:10.0.0.5",
        ] {
            assert_eq!(check_address(ip(a), STRICT), Err(TargetError::Private), "{a}");
            assert!(address_allowed(ip(a), ONPREM), "{a}");
        }
        for a in ["1.1.1.1", "172.32.0.1", "100.128.0.1", "198.20.0.1", "2606:4700::1111", "2001:4860:4860::8888"] {
            assert!(address_allowed(ip(a), STRICT), "{a} is public");
        }
    }

    #[test]
    fn urls_are_https_without_credentials() {
        let t = parse_target(" https://hooks.example.com/services/T0/B0/secret?x=1 ", STRICT).unwrap();
        assert_eq!(t.display, "https://hooks.example.com");
        assert_eq!(t.url.path(), "/services/T0/B0/secret");
        assert!(!format!("{t:?}").contains("secret"), "the path stays out of logs");
        assert_eq!(
            parse_target("https://Hooks.Example.com:8443/x", STRICT).unwrap().display,
            "https://hooks.example.com:8443"
        );
        assert_eq!(parse_target("http://hooks.example.com/x", STRICT), Err(TargetError::Scheme));
        assert_eq!(parse_target("http://hooks.example.com/x", ONPREM).unwrap().display, "http://hooks.example.com");
        for bad in ["ftp://example.com/", "file:///etc/passwd", "gopher://example.com/", "javascript:alert(1)"] {
            assert_eq!(parse_target(bad, LAB), Err(TargetError::Scheme), "{bad}");
        }
        for bad in ["", "hooks.example.com/x", "https://", "//example.com/x"] {
            assert_eq!(parse_target(bad, LAB), Err(TargetError::Malformed), "{bad}");
        }
        for bad in ["https://user@example.com/", "https://user:pw@example.com/", "https://example.com/#frag"] {
            assert_eq!(parse_target(bad, LAB), Err(TargetError::Credentials), "{bad}");
        }
        let long = format!("https://example.com/{}", "a".repeat(MAX_URL));
        assert_eq!(parse_target(&long, LAB), Err(TargetError::TooLong));
    }

    #[test]
    fn literal_addresses_in_any_spelling_follow_the_address_rules() {
        // The URL parser reads these as IPv4 addresses; none slips through as a name.
        for bad in [
            "https://169.254.169.254/latest/meta-data/",
            "https://2852039166/",
            "https://0xa9fea9fe/",
            "https://0251.0376.0251.0376/",
            "https://[::ffff:169.254.169.254]/",
            "https://[fe80::1]/",
            "https://[fd00:ec2::254]/",
            "https://[2002:a9fe:a9fe::1]/",
            "https://[64:ff9b:1::a9fe:a9fe]/",
        ] {
            assert_eq!(parse_target(bad, LAB), Err(TargetError::Forbidden), "{bad}");
        }
        for (bad, why) in [
            ("https://127.0.0.1/", TargetError::Loopback),
            ("https://2130706433/", TargetError::Loopback),
            ("https://0x7f.1/", TargetError::Loopback),
            ("https://[::1]/", TargetError::Loopback),
            ("https://localhost/", TargetError::Loopback),
            ("https://LOCALHOST./", TargetError::Loopback),
            ("https://app.localhost/", TargetError::Loopback),
            ("https://10.0.0.5/", TargetError::Private),
            ("https://192.168.1.10:9000/hook", TargetError::Private),
            ("https://[fd12:3456::1]/", TargetError::Private),
        ] {
            assert_eq!(parse_target(bad, STRICT), Err(why), "{bad}");
        }
        assert_eq!(parse_target("https://10.0.0.5/hook", ONPREM).unwrap().display, "https://10.0.0.5");
        assert_eq!(parse_target("http://127.0.0.1:8080/hook", LAB).unwrap().display, "http://127.0.0.1:8080");
        assert_eq!(parse_target("https://[::1]:8443/", LAB).unwrap().display, "https://[::1]:8443");
    }

    #[test]
    fn metadata_names_are_refused() {
        for bad in ["https://metadata.google.internal/", "https://metadata/", "https://instance-data/latest/"] {
            assert_eq!(parse_target(bad, LAB), Err(TargetError::Forbidden), "{bad}");
        }
    }
}
