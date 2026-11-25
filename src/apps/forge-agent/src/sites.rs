//! Nginx sites and PowerDNS (root, inside `forge-agent-helper`).

use std::path::Path;
use std::time::Duration;

use forge_catalog::SiteSpec;
use forge_catalog::sites::{ACME_ROOT, CACHE_DIR, CUSTOM_DIR, HTPASSWD_DIR, LOG_DIR, SITES_DIR, TLS_DIR, Tls};
use http_body_util::{BodyExt, Full};
use hyper::Request;
use hyper::body::Bytes;
use hyper_util::rt::TokioIo;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use tokio::process::Command;

pub(crate) async fn run(cmd: &str, args: &[&str]) -> Result<String, String> {
    let out = Command::new(cmd).args(args).output().await.map_err(|e| format!("{cmd}: {e}"))?;
    let text = format!("{}{}", String::from_utf8_lossy(&out.stdout), String::from_utf8_lossy(&out.stderr));
    if out.status.success() { Ok(text) } else { Err(text.trim().to_owned()) }
}

pub(crate) fn nginx_bin() -> Result<&'static str, String> {
    ["/usr/sbin/nginx", "/usr/bin/nginx"]
        .into_iter()
        .find(|p| Path::new(p).exists())
        .ok_or_else(|| "Nginx is not installed on this node (App Store → Nginx)".to_owned())
}

pub(crate) async fn reload(nginx: &str) -> Result<(), String> {
    if Path::new("/run/systemd/system").is_dir() {
        run("systemctl", &["reload", "nginx"]).await.map(|_| ())
    } else if run("pgrep", &["-x", "nginx"]).await.is_ok() {
        run(nginx, &["-s", "reload"]).await.map(|_| ())
    } else {
        run(nginx, &[]).await.map(|_| ())
    }
}

/// The group nginx's workers run as (`www-data` on Debian/Ubuntu, `nginx`
/// on RHEL-likes), from `/etc/group`.
#[cfg(unix)]
fn nginx_group() -> Option<u32> {
    let groups = std::fs::read_to_string("/etc/group").ok()?;
    ["www-data", "nginx"].iter().find_map(|name| {
        groups.lines().find_map(|l| {
            let mut f = l.split(':');
            (f.next() == Some(name)).then(|| f.nth(1)?.parse().ok()).flatten()
        })
    })
}

/// Writes a file; certificates and keys under [`TLS_DIR`] are root-only
/// (0600); `htpasswd` files are 0640, readable by nginx's worker group only.
fn write_file(path: &str, content: &str) -> Result<(), String> {
    #[cfg(unix)]
    if path.starts_with(HTPASSWD_DIR) {
        use std::io::Write;
        use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
        let mut f = std::fs::OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .mode(0o640)
            .open(path)
            .map_err(|e| format!("{path}: {e}"))?;
        f.set_permissions(std::fs::Permissions::from_mode(0o640)).map_err(|e| format!("{path}: {e}"))?;

        // Without nginx's group the workers could not read the file.
        let gid = nginx_group().ok_or("neither the www-data nor the nginx group exists; is Nginx installed?")?;
        std::os::unix::fs::fchown(&f, Some(0), Some(gid)).map_err(|e| format!("{path}: {e}"))?;

        return f.write_all(content.as_bytes()).map_err(|e| format!("{path}: {e}"));
    }
    #[cfg(unix)]
    if path.starts_with(TLS_DIR) {
        use std::io::Write;
        use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
        let mut f = std::fs::OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .mode(0o600)
            .open(path)
            .map_err(|e| format!("{path}: {e}"))?;

        // An existing file keeps its mode on open: tighten it explicitly.
        f.set_permissions(std::fs::Permissions::from_mode(0o600)).map_err(|e| format!("{path}: {e}"))?;

        return f.write_all(content.as_bytes()).map_err(|e| format!("{path}: {e}"));
    }
    std::fs::write(path, content).map_err(|e| format!("{path}: {e}"))
}

/// The nginx binary and the directories a swap writes into.
struct Nginx<'a> {
    bin: &'a str,
    dirs: &'a [&'a str],
    /// Reload after a passing test (off in tests).
    reload: bool,
}

/// Writes `files`, runs `nginx -t`, reloads. Restores the previous files when
/// the test fails, so a bad custom configuration never takes the edge down.
async fn swap(files: &[(String, Option<String>)]) -> Result<String, String> {
    let dirs = [SITES_DIR, CUSTOM_DIR, TLS_DIR, HTPASSWD_DIR, LOG_DIR, CACHE_DIR];
    swap_with(&Nginx { bin: nginx_bin()?, dirs: &dirs, reload: true }, files).await
}

async fn swap_with(nginx: &Nginx<'_>, files: &[(String, Option<String>)]) -> Result<String, String> {
    for d in nginx.dirs {
        std::fs::create_dir_all(d).map_err(|e| e.to_string())?;
    }

    #[cfg(unix)]
    if nginx.dirs.contains(&TLS_DIR) {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(TLS_DIR, std::fs::Permissions::from_mode(0o700)).map_err(|e| e.to_string())?;
    }
    let previous: Vec<(String, Option<String>)> =
        files.iter().map(|(p, _)| (p.clone(), std::fs::read_to_string(p).ok())).collect();
    let apply = |set: &[(String, Option<String>)]| -> Result<(), String> {
        for (p, content) in set {
            match content {
                Some(c) => write_file(p, c)?,
                None => {
                    let _ = std::fs::remove_file(p);
                }
            }
        }

        Ok(())
    };
    apply(files)?;

    if let Err(e) = run(nginx.bin, &["-t"]).await {
        apply(&previous)?;
        return Err(format!("nginx -t failed; previous configuration restored:\n{e}"));
    }

    if nginx.reload {
        reload(nginx.bin).await?;
    }

    Ok("nginx configuration test passed; reloaded".into())
}

/// The last lines (at most 1000 and 512 KiB) of one of the endpoint's own
/// log files, and the file's size. Only the paths derived from the
/// endpoint's validated name are read.
async fn tail(path: &str) -> Result<Value, String> {
    use tokio::io::{AsyncReadExt, AsyncSeekExt};
    const MAX: u64 = 512 * 1024;

    let mut f = match tokio::fs::File::open(path).await {
        Ok(f) => f,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            return Ok(json!({ "path": path, "size": 0, "log": "", "missing": true }));
        }
        Err(e) => return Err(format!("{path}: {e}")),
    };

    let size = f.metadata().await.map_err(|e| format!("{path}: {e}"))?.len();
    f.seek(std::io::SeekFrom::Start(size.saturating_sub(MAX))).await.map_err(|e| format!("{path}: {e}"))?;
    let mut buf = Vec::new();
    f.take(MAX).read_to_end(&mut buf).await.map_err(|e| format!("{path}: {e}"))?;
    let text = String::from_utf8_lossy(&buf);
    let lines: Vec<&str> = text.lines().collect();

    // A cut-off first line is dropped when the read started mid-file.
    let skip = usize::from(size > MAX && lines.len() > 1);
    let from = lines.len().saturating_sub(1000).max(skip);

    Ok(json!({ "path": path, "size": size, "log": lines[from..].join("\n") }))
}

/// Empties the endpoint's cache directory (nginx keeps serving; entries are
/// fetched again from the upstreams).
async fn clear_cache(dir: &str) -> Result<Value, String> {
    let mut removed = 0u64;

    let mut entries = match tokio::fs::read_dir(dir).await {
        Ok(e) => e,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(json!({ "state": "cleared", "removed": 0 })),
        Err(e) => return Err(format!("{dir}: {e}")),
    };

    while let Some(e) = entries.next_entry().await.map_err(|e| format!("{dir}: {e}"))? {
        let p = e.path();
        let kind = e.file_type().await.map_err(|e| format!("{}: {e}", p.display()))?;
        if kind.is_dir() {
            tokio::fs::remove_dir_all(&p).await.map_err(|e| format!("{}: {e}", p.display()))?;
        } else {
            tokio::fs::remove_file(&p).await.map_err(|e| format!("{}: {e}", p.display()))?;
        }

        removed += 1;
    }

    Ok(json!({ "state": "cleared", "removed": removed }))
}

/// Issues (or, with `force`, renews) the Let's Encrypt certificate over HTTP-01.
async fn issue(spec: &SiteSpec, force: bool) -> Result<String, String> {
    if !Path::new("/usr/bin/certbot").exists() {
        return Err("TLS needs Certbot on this node (App Store → Certbot)".into());
    }

    let name = cert_name(spec);
    let mut args = vec![
        "certonly",
        "--webroot",
        "-w",
        ACME_ROOT,
        "--non-interactive",
        "--agree-tos",
        "--cert-name",
        &name,
        "-m",
        &spec.acme_email,
    ];

    if force {
        args.push("--force-renewal");
    }

    for d in &spec.domains {
        args.push("-d");
        args.push(d);
    }

    let out =
        run("certbot", &args).await.map_err(|e| format!("certbot failed (is DNS pointing at this node?):\n{e}"))?;

    Ok(out.lines().rev().take(4).collect::<Vec<_>>().join("\n"))
}

fn cert_name(spec: &SiteSpec) -> String {
    spec.domains.first().map(|d| d.trim_start_matches("*.").to_owned()).unwrap_or_default()
}

/// Expiry of the Let's Encrypt certificate, from `certbot certificates`.
async fn certificate(spec: &SiteSpec) -> Value {
    let name = cert_name(spec);
    match run("certbot", &["certificates", "--cert-name", &name]).await {
        Ok(out) => match parse_expiry(&out) {
            Some((expiry, days)) => json!({ "expiry": expiry, "days_left": days }),
            None => json!({ "missing": true }),
        },
        Err(e) => json!({ "error": e.lines().last().unwrap_or_default() }),
    }
}

/// `Expiry Date: 2027-02-08 12:00:00+00:00 (VALID: 89 days)` → (date, 89);
/// `(INVALID: EXPIRED)` → (date, -1).
pub(crate) fn parse_expiry(out: &str) -> Option<(String, i64)> {
    let line = out.lines().find(|l| l.trim_start().starts_with("Expiry Date:"))?;
    let rest = line.trim_start().trim_start_matches("Expiry Date:").trim();
    let (date, tail) = rest.split_once(" (").unwrap_or((rest, ""));
    let days =
        if tail.contains("EXPIRED") { -1 } else { tail.split_whitespace().find_map(|w| w.parse::<i64>().ok())? };

    Some((date.trim().to_owned(), days))
}

async fn site(action: &str, spec: &SiteSpec) -> Result<Value, String> {
    spec.validate()?;

    match action {
        "apply" => {
            std::fs::create_dir_all(format!("{ACME_ROOT}/.well-known/acme-challenge")).map_err(|e| e.to_string())?;
            let custom = (spec.custom_path(), Some(spec.render_custom()));
            let htpasswd = (spec.htpasswd_path(), spec.access.htpasswd());

            if spec.tls == Tls::Uploaded {
                let m = spec.material.as_ref().ok_or("the uploaded certificate was not sent with the command")?;
                let (crt, key) = spec.cert_files();
                let log = swap(&[
                    (crt, Some(m.chain_pem.clone())),
                    (key, Some(m.key_pem.clone())),
                    htpasswd,
                    (spec.site_path(), Some(spec.render(true))),
                    custom,
                ])
                .await?;

                return Ok(json!({ "state": "applied", "https": true, "log": log }));
            }

            let has_cert = Path::new(&format!("{}/fullchain.pem", spec.cert_dir())).exists();
            let tls = spec.tls == Tls::Acme;

            // HTTP first (serves the ACME challenge), then the certificate, then HTTPS.
            let mut log =
                swap(&[htpasswd, (spec.site_path(), Some(spec.render(tls && has_cert))), custom.clone()]).await?;

            if tls && !has_cert {
                if !Path::new("/usr/bin/certbot").exists() {
                    return Err("TLS needs Certbot on this node (App Store → Certbot)".into());
                }

                log.push_str(&format!("\n{}", issue(spec, false).await?));
                log.push_str(&format!("\n{}", swap(&[(spec.site_path(), Some(spec.render(true))), custom]).await?));
            }

            Ok(json!({ "state": "applied", "https": tls, "log": log }))
        }
        "remove" => {
            let (crt, key) = spec.cert_files();
            let mut files = vec![(spec.site_path(), None), (spec.custom_path(), None), (spec.htpasswd_path(), None)];

            if spec.tls == Tls::Uploaded {
                files.extend([(crt, None), (key, None)]);
            }

            let log = swap(&files).await?;
            Ok(json!({ "state": "removed", "log": log }))
        }
        "renew" => {
            if spec.tls != Tls::Acme {
                return Err("only Let's Encrypt certificates are renewed by Forge".into());
            }

            let mut log = issue(spec, true).await?;
            let nginx = nginx_bin()?;
            run(nginx, &["-t"]).await.map_err(|e| format!("nginx -t failed after renewal:\n{e}"))?;
            reload(nginx).await?;
            log.push_str("\nnginx reloaded");
            Ok(json!({ "state": "renewed", "certificate": certificate(spec).await, "log": log }))
        }
        "status" => Ok(json!({
            "certificate": if spec.tls == Tls::Acme { certificate(spec).await } else { Value::Null },
            "state": if Path::new(&spec.site_path()).exists() { "applied" } else { "missing" },
            "config": std::fs::read_to_string(spec.site_path()).ok(),
        })),
        "clear_cache" => clear_cache(&spec.cache_path()).await,
        "access_log" => tail(&spec.access_log_path()).await,
        "error_log" => tail(&spec.error_log_path()).await,
        other => Err(format!("unknown site action `{other}`")),
    }
}

// ---------------------------------------------------------------- PowerDNS

/// DNS operations against the node's local PowerDNS API.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case", deny_unknown_fields)]
pub enum DnsOp {
    Zones,
    CreateZone { zone: String, nameservers: Vec<String> },
    DeleteZone { zone: String },
    Records { zone: String },
    SetRecords { zone: String, name: String, rtype: String, ttl: u32, contents: Vec<String> },
    DeleteRecords { zone: String, name: String, rtype: String },
}

const RTYPES: &[&str] = &["A", "AAAA", "CNAME", "TXT", "MX", "NS", "SRV", "CAA"];

fn fqdn_ok(n: &str) -> bool {
    n.ends_with('.')
        && n.len() <= 254
        && n.trim_end_matches('.').split('.').all(|l| {
            !l.is_empty()
                && l.len() <= 63
                && l.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_' || b == b'*')
        })
}

impl DnsOp {
    /// Checks the zone and record names before the operation runs.
    pub fn validate(&self) -> Result<(), String> {
        let zone_ok = |z: &str| if fqdn_ok(z) { Ok(()) } else { Err(format!("invalid zone `{z}` (use example.com.)")) };

        match self {
            Self::Zones => Ok(()),
            Self::CreateZone { zone, nameservers } => {
                zone_ok(zone)?;

                if nameservers.iter().any(|n| !fqdn_ok(n)) {
                    return Err("name servers must be fully qualified (ns1.example.com.)".into());
                }

                Ok(())
            }
            Self::DeleteZone { zone } | Self::Records { zone } => zone_ok(zone),
            Self::SetRecords { zone, name, rtype, ttl, contents } => {
                zone_ok(zone)?;

                if !fqdn_ok(name) || !name.ends_with(zone.as_str()) {
                    return Err(format!("`{name}` must be a fully qualified name inside {zone}"));
                }

                if !RTYPES.contains(&rtype.as_str()) {
                    return Err(format!("record type {rtype} is not supported"));
                }

                if *ttl < 30 || *ttl > 604_800 || contents.is_empty() || contents.len() > 50 {
                    return Err("ttl 30–604800 and 1–50 values".into());
                }

                if contents.iter().any(|c| c.is_empty() || c.len() > 1024 || c.contains(['\n', '\r', '\0'])) {
                    return Err("record values must be single lines".into());
                }

                Ok(())
            }
            Self::DeleteRecords { zone, name, rtype } => {
                zone_ok(zone)?;
                if fqdn_ok(name) && RTYPES.contains(&rtype.as_str()) { Ok(()) } else { Err("invalid record".into()) }
            }
        }
    }
}

fn pdns_env() -> Result<(String, String), String> {
    let text = std::fs::read_to_string("/etc/forge/pkgs/powerdns.env")
        .map_err(|_| "PowerDNS is not installed on this node (App Store → PowerDNS)".to_owned())?;
    let get = |k: &str| text.lines().find_map(|l| l.strip_prefix(&format!("{k}=")).map(str::to_owned));
    let url = get("FORGE_PKG_POWERDNS_API_URL").ok_or("PowerDNS API URL missing")?;
    let key = get("FORGE_PKG_POWERDNS_API_KEY").ok_or("PowerDNS API key missing")?;

    Ok((url.trim_start_matches("http://").to_owned(), key))
}

async fn pdns(method: &str, path: &str, body: Option<Value>) -> Result<Value, String> {
    let (addr, key) = pdns_env()?;
    let run = async {
        let stream = tokio::net::TcpStream::connect(&addr).await.map_err(|e| format!("PowerDNS API ({addr}): {e}"))?;
        let (mut sender, conn) =
            hyper::client::conn::http1::handshake(TokioIo::new(stream)).await.map_err(|e| e.to_string())?;
        tokio::spawn(conn);
        let req = Request::builder()
            .method(method)
            .uri(format!("http://{addr}/api/v1/servers/localhost{path}"))
            .header("Host", addr.as_str())
            .header("X-API-Key", key)
            .header("Content-Type", "application/json")
            .body(Full::new(body.map_or_else(Bytes::new, |b| Bytes::from(b.to_string()))))
            .map_err(|e| e.to_string())?;
        let resp = sender.send_request(req).await.map_err(|e| e.to_string())?;
        let status = resp.status().as_u16();
        let bytes = resp.into_body().collect().await.map_err(|e| e.to_string())?.to_bytes();
        let v: Value = serde_json::from_slice(&bytes).unwrap_or(Value::Null);

        if (200..300).contains(&status) {
            Ok(v)
        } else {
            Err(format!("PowerDNS: {} (HTTP {status})", v["error"].as_str().unwrap_or("request failed")))
        }
    };

    tokio::time::timeout(Duration::from_secs(20), run).await.map_err(|_| "PowerDNS did not respond".to_owned())?
}

async fn dns(op: &DnsOp) -> Result<Value, String> {
    op.validate()?;

    match op {
        DnsOp::Zones => {
            let v = pdns("GET", "/zones", None).await?;
            Ok(Value::Array(
                v.as_array()
                    .map(|a| {
                        a.iter()
                            .map(|z| json!({ "name": z["name"], "serial": z["serial"], "kind": z["kind"] }))
                            .collect()
                    })
                    .unwrap_or_default(),
            ))
        }
        DnsOp::CreateZone { zone, nameservers } => {
            pdns("POST", "/zones", Some(json!({ "name": zone, "kind": "Native", "nameservers": nameservers }))).await?;
            Ok(json!({ "zone": zone, "done": "created" }))
        }
        DnsOp::DeleteZone { zone } => {
            pdns("DELETE", &format!("/zones/{zone}"), None).await?;
            Ok(json!({ "zone": zone, "done": "deleted" }))
        }
        DnsOp::Records { zone } => {
            let v = pdns("GET", &format!("/zones/{zone}"), None).await?;
            let rr: Vec<Value> = v["rrsets"]
                .as_array()
                .map(|a| {
                    a.iter()
                        .map(|r| json!({ "name": r["name"], "type": r["type"], "ttl": r["ttl"], "contents": r["records"].as_array().map(|x| x.iter().map(|c| c["content"].clone()).collect::<Vec<_>>()) }))
                        .collect()
                })
                .unwrap_or_default();
            Ok(json!({ "zone": zone, "records": rr }))
        }
        DnsOp::SetRecords { zone, name, rtype, ttl, contents } => {
            let records: Vec<Value> = contents.iter().map(|c| json!({ "content": c, "disabled": false })).collect();
            pdns("PATCH", &format!("/zones/{zone}"), Some(json!({ "rrsets": [{ "name": name, "type": rtype, "ttl": ttl, "changetype": "REPLACE", "records": records }] }))).await?;
            Ok(json!({ "name": name, "type": rtype, "done": "set" }))
        }
        DnsOp::DeleteRecords { zone, name, rtype } => {
            pdns(
                "PATCH",
                &format!("/zones/{zone}"),
                Some(json!({ "rrsets": [{ "name": name, "type": rtype, "changetype": "DELETE" }] })),
            )
            .await?;
            Ok(json!({ "name": name, "type": rtype, "done": "deleted" }))
        }
    }
}

/// Entry points for the helper.
pub async fn site_action(action: &str, spec: &SiteSpec) -> Result<Value, String> {
    site(action, spec).await
}

/// Runs one DNS operation for the helper.
pub async fn dns_action(op: &DnsOp) -> Result<Value, String> {
    dns(op).await
}

#[cfg(test)]
#[allow(clippy::unwrap_used)] // reason: tests
mod tests {
    use super::DnsOp;

    /// A fake `nginx` whose `-t` fails when any written file contains `BAD`.
    #[cfg(unix)]
    fn fake_nginx(dir: &std::path::Path) -> String {
        use std::os::unix::fs::PermissionsExt;
        let bin = dir.join("nginx");
        let script = format!(
            "#!/bin/sh\nif grep -rq BAD {}; then echo 'emerg: BAD' >&2; exit 1; fi\nexit 0\n",
            dir.join("conf").display()
        );
        std::fs::write(&bin, script).unwrap();
        std::fs::set_permissions(&bin, std::fs::Permissions::from_mode(0o755)).unwrap();

        bin.display().to_string()
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn swap_keeps_a_passing_config_and_restores_a_failing_one() {
        let tmp = tempfile::tempdir().unwrap();
        let bin = fake_nginx(tmp.path());
        let conf = tmp.path().join("conf");
        let conf_s = conf.display().to_string();
        let nginx = super::Nginx { bin: &bin, dirs: &[conf_s.as_str()], reload: false };
        let site = conf.join("site.conf").display().to_string();
        let custom = conf.join("custom.conf").display().to_string();
        super::swap_with(&nginx, &[(site.clone(), Some("good 1".into())), (custom.clone(), Some("c1".into()))])
            .await
            .unwrap();
        assert_eq!(std::fs::read_to_string(&site).unwrap(), "good 1");

        // A failing test restores both files as they were.
        let err =
            super::swap_with(&nginx, &[(site.clone(), Some("good 2".into())), (custom.clone(), Some("BAD".into()))])
                .await
                .unwrap_err();
        assert!(err.contains("previous configuration restored") && err.contains("BAD"), "{err}");
        assert_eq!(std::fs::read_to_string(&site).unwrap(), "good 1");
        assert_eq!(std::fs::read_to_string(&custom).unwrap(), "c1");

        // A new file that fails is removed again; a removal that passes stays removed.
        let extra = conf.join("extra.conf").display().to_string();
        assert!(super::swap_with(&nginx, &[(extra.clone(), Some("BAD".into()))]).await.is_err());
        assert!(!std::path::Path::new(&extra).exists());
        super::swap_with(&nginx, &[(custom.clone(), None)]).await.unwrap();
        assert!(!std::path::Path::new(&custom).exists());
    }

    #[tokio::test]
    async fn tails_only_the_last_lines_and_reports_missing_logs() {
        let tmp = tempfile::tempdir().unwrap();
        let p = tmp.path().join("x.log");
        let body: String = (0..1500).map(|i| format!("line {i}\n")).collect();
        std::fs::write(&p, body).unwrap();
        let v = super::tail(&p.display().to_string()).await.unwrap();
        let log = v["log"].as_str().unwrap();
        assert_eq!(log.lines().count(), 1000);
        assert!(log.starts_with("line 500") && log.ends_with("line 1499"));
        let missing = super::tail(&tmp.path().join("nope.log").display().to_string()).await.unwrap();
        assert_eq!(missing["missing"], true);
        let cache = tmp.path().join("cache");
        std::fs::create_dir_all(cache.join("a/b")).unwrap();
        std::fs::write(cache.join("f"), "x").unwrap();
        let r = super::clear_cache(&cache.display().to_string()).await.unwrap();
        assert_eq!(r["removed"], 2);
        assert!(cache.exists() && std::fs::read_dir(&cache).unwrap().next().is_none());
    }

    #[test]
    fn reads_certbot_expiry() {
        let out = "Found the following certs:\n  Certificate Name: shop.example.com\n    Domains: shop.example.com\n    Expiry Date: 2027-02-08 12:00:00+00:00 (VALID: 89 days)\n";
        assert_eq!(super::parse_expiry(out), Some(("2027-02-08 12:00:00+00:00".into(), 89)));
        let gone = "    Expiry Date: 2026-01-01 00:00:00+00:00 (INVALID: EXPIRED)\n";
        assert_eq!(super::parse_expiry(gone).map(|x| x.1), Some(-1));
        assert_eq!(super::parse_expiry("No certificates found."), None);
    }

    #[test]
    fn dns_ops_are_validated() {
        let set = |name: &str, t: &str, c: &str| DnsOp::SetRecords {
            zone: "example.com.".into(),
            name: name.into(),
            rtype: t.into(),
            ttl: 300,
            contents: vec![c.into()],
        };
        assert!(set("shop.example.com.", "A", "203.0.113.10").validate().is_ok());
        assert!(set("shop.other.com.", "A", "203.0.113.10").validate().is_err(), "outside the zone");
        assert!(set("shop.example.com.", "PTR", "x").validate().is_err());
        assert!(set("shop.example.com.", "TXT", "a\nb").validate().is_err());
        assert!(
            DnsOp::CreateZone { zone: "example.com".into(), nameservers: vec![] }.validate().is_err(),
            "trailing dot"
        );
    }
}
