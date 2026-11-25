//! dockerd's TCP listener with mutual TLS (D-125), run by the root helper.
//!
//! Files, all in `/etc/docker/forge-tls` (0700, root):
//! * `key.pem` (0600): dockerd's key, made here and never sent anywhere;
//! * `ca.pem`: the node's Docker CA; dockerd accepts only clients it signed;
//! * `cert.pem`: dockerd's certificate, signed by that CA;
//! * `execstart`: dockerd's command line before Forge changed it.
//!
//! The listener is added with a systemd drop-in that repeats dockerd's own
//! command line and appends `-H tcp://… --tlsverify …`, so the local socket
//! and every other flag stay as they are. The drop-in is checked by
//! restarting dockerd and asking it `/_ping` on its socket; if that fails,
//! the drop-in is removed and dockerd restarted again.

use std::path::Path;
use std::time::Duration;

use forge_catalog::docker_tls::{DockerTlsOp, Prepared};
use serde_json::{Value, json};

const DIR: &str = "/etc/docker/forge-tls";
const DROP_IN_DIR: &str = "/etc/systemd/system/docker.service.d";
const DROP_IN: &str = "/etc/systemd/system/docker.service.d/forge-tls.conf";
const MARK: &str = "# Managed by Orbyn Forge (D-125): Docker Engine API with mutual TLS";

fn file(name: &str) -> String {
    format!("{DIR}/{name}")
}

/// Runs one operation.
pub async fn run(op: &DockerTlsOp) -> Result<Value, String> {
    op.validate()?;

    match op {
        DockerTlsOp::Prepare {} => prepare().await,
        DockerTlsOp::Apply { ca_pem, cert_pem, listen, port } => {
            apply(ca_pem, cert_pem, &listen.to_string(), *port).await
        }
        DockerTlsOp::Remove {} => remove().await,
        DockerTlsOp::Status {} => Ok(json!({ "installed": installed() })),
    }
}

/// `listen:port` from the drop-in, when Forge's listener is installed.
fn installed() -> Option<String> {
    let text = std::fs::read_to_string(DROP_IN).ok()?;
    text.contains(MARK).then_some(())?;

    text.split_whitespace().find_map(|w| w.strip_prefix("tcp://").map(str::to_owned))
}

async fn sh(program: &str, args: &[&str], timeout: Duration) -> Result<String, String> {
    let out = tokio::time::timeout(timeout, tokio::process::Command::new(program).args(args).output())
        .await
        .map_err(|_| format!("{program} timed out"))?
        .map_err(|e| format!("{program}: {e}"))?;

    if out.status.success() {
        Ok(String::from_utf8_lossy(&out.stdout).into_owned())
    } else {
        Err(format!("{program} {}: {}", args.join(" "), String::from_utf8_lossy(&out.stderr).trim()))
    }
}

fn mkdir_private() -> Result<(), String> {
    use std::os::unix::fs::{DirBuilderExt, PermissionsExt};
    std::fs::DirBuilder::new().recursive(true).mode(0o700).create(DIR).map_err(|e| format!("{DIR}: {e}"))?;

    std::fs::set_permissions(DIR, std::fs::Permissions::from_mode(0o700)).map_err(|e| format!("{DIR}: {e}"))
}

fn write(path: &str, data: &str, mode: u32) -> Result<(), String> {
    use std::io::Write;
    use std::os::unix::fs::OpenOptionsExt;
    let tmp = format!("{path}.tmp");
    let mut f = std::fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .mode(mode)
        .open(&tmp)
        .map_err(|e| format!("{tmp}: {e}"))?;
    f.write_all(data.as_bytes()).map_err(|e| format!("{tmp}: {e}"))?;
    f.sync_all().map_err(|e| format!("{tmp}: {e}"))?;

    std::fs::rename(&tmp, path).map_err(|e| format!("{path}: {e}"))
}

async fn prepare() -> Result<Value, String> {
    let info = crate::docker::api("GET", "/info", None, Duration::from_secs(15))
        .await?
        .ok_or("Docker is not answering on its socket")?;
    let key_path = file("key.pem");
    let csr_pem = tokio::task::spawn_blocking(move || -> Result<String, String> {
        mkdir_private()?;
        // The key is made once and kept: a repeated prepare (a retried job)
        // signs the same key.
        let key_pem = match std::fs::read_to_string(&key_path) {
            Ok(k) => k,
            Err(_) => {
                let key = rcgen::KeyPair::generate().map_err(|e| e.to_string())?;
                let pem = key.serialize_pem();
                write(&key_path, &pem, 0o600)?;
                pem
            }
        };
        let key = rcgen::KeyPair::from_pem(&key_pem).map_err(|e| e.to_string())?;
        let params = rcgen::CertificateParams::new(Vec::<String>::new()).map_err(|e| e.to_string())?;
        params.serialize_request(&key).and_then(|c| c.pem()).map_err(|e| e.to_string())
    })
    .await
    .map_err(|e| e.to_string())??;
    let prepared = Prepared {
        csr_pem,
        addresses: addresses().await,
        docker_version: info["ServerVersion"].as_str().unwrap_or_default().to_owned(),
        live_restore: info["LiveRestoreEnabled"].as_bool().unwrap_or(false),
        running_containers: info["ContainersRunning"].as_u64().unwrap_or(0),
        installed: installed(),
    };

    serde_json::to_value(prepared).map_err(|e| e.to_string())
}

/// The node's global unicast addresses (`ip -j addr`).
async fn addresses() -> Vec<std::net::IpAddr> {
    let Ok(out) = sh("ip", &["-j", "addr", "show"], Duration::from_secs(5)).await else {
        return Vec::new();
    };

    let v: Value = serde_json::from_str(&out).unwrap_or(Value::Null);

    v.as_array()
        .into_iter()
        .flatten()
        .flat_map(|i| i["addr_info"].as_array().cloned().unwrap_or_default())
        .filter(|a| a["scope"] == "global")
        .filter_map(|a| a["local"].as_str().and_then(|s| s.parse().ok()))
        .collect()
}

/// dockerd's command line without Forge's drop-in: saved the first time,
/// so re-applying never stacks listeners.
async fn base_command() -> Result<String, String> {
    if let Ok(saved) = std::fs::read_to_string(file("execstart"))
        && !saved.trim().is_empty()
    {
        return Ok(saved.trim().to_owned());
    }

    let shown =
        sh("systemctl", &["show", "docker.service", "-p", "ExecStart", "--value"], Duration::from_secs(10)).await?;
    let argv = parse_exec_start(&shown).ok_or("cannot read dockerd's command line from systemd")?;
    if argv.contains("tcp://") {
        return Err("dockerd already listens on TCP outside Forge; remove that listener first".into());
    }

    write(&file("execstart"), &argv, 0o600)?;

    Ok(argv)
}

/// The `argv[]=` part of `systemctl show -p ExecStart --value`.
fn parse_exec_start(shown: &str) -> Option<String> {
    let start = shown.find("argv[]=")? + "argv[]=".len();
    let rest = &shown[start..];
    let end = rest.find(" ;").unwrap_or(rest.len());
    let argv = rest[..end].trim();

    (argv.contains("dockerd") && !argv.contains('\n')).then(|| argv.to_owned())
}

fn drop_in(base: &str, listen: &str, port: u16) -> String {
    format!(
        "{MARK}\n[Service]\nExecStart=\nExecStart={base} -H tcp://{listen}:{port} --tlsverify --tlscacert={ca} --tlscert={cert} --tlskey={key}\n",
        ca = file("ca.pem"),
        cert = file("cert.pem"),
        key = file("key.pem"),
    )
}

async fn restart_and_wait() -> Result<(), String> {
    sh("systemctl", &["daemon-reload"], Duration::from_secs(30)).await?;
    sh("systemctl", &["restart", "docker.service"], Duration::from_secs(120)).await?;
    let deadline = tokio::time::Instant::now() + Duration::from_secs(60);

    loop {
        if crate::docker::api("GET", "/_ping", None, Duration::from_secs(5)).await.is_ok()
            && sh("systemctl", &["is-active", "docker.service"], Duration::from_secs(5)).await.is_ok()
        {
            return Ok(());
        }

        if tokio::time::Instant::now() >= deadline {
            let log = sh("journalctl", &["-u", "docker.service", "-n", "15", "--no-pager"], Duration::from_secs(10))
                .await
                .unwrap_or_default();
            return Err(format!("dockerd did not come back: {}", log.trim()));
        }

        tokio::time::sleep(Duration::from_millis(500)).await;
    }
}

async fn apply(ca_pem: &str, cert_pem: &str, listen: &str, port: u16) -> Result<Value, String> {
    if !Path::new(&file("key.pem")).exists() {
        return Err("no dockerd key on this node: prepare first".into());
    }

    let base = base_command().await?;
    let unit = drop_in(&base, listen, port);
    let previous = std::fs::read_to_string(DROP_IN).ok();
    if previous.as_deref() == Some(unit.as_str())
        && std::fs::read_to_string(file("ca.pem")).ok().as_deref() == Some(ca_pem)
        && std::fs::read_to_string(file("cert.pem")).ok().as_deref() == Some(cert_pem)
    {
        return Ok(json!({ "listening": format!("{listen}:{port}"), "restarted": false }));
    }

    let old_ca = std::fs::read_to_string(file("ca.pem")).ok();
    let old_cert = std::fs::read_to_string(file("cert.pem")).ok();
    mkdir_private()?;
    write(&file("ca.pem"), ca_pem, 0o644)?;
    write(&file("cert.pem"), cert_pem, 0o644)?;
    std::fs::create_dir_all(DROP_IN_DIR).map_err(|e| format!("{DROP_IN_DIR}: {e}"))?;
    write(DROP_IN, &unit, 0o644)?;

    if let Err(e) = restart_and_wait().await {
        // Put back what was there, so Docker keeps running as before.
        match &previous {
            Some(p) => write(DROP_IN, p, 0o644)?,
            None => {
                let _ = std::fs::remove_file(DROP_IN);
            }
        }

        if let (Some(ca), Some(cert)) = (&old_ca, &old_cert) {
            write(&file("ca.pem"), ca, 0o644)?;
            write(&file("cert.pem"), cert, 0o644)?;
        }

        let back = restart_and_wait().await;

        return Err(match back {
            Ok(()) => format!("{e} (rolled back; Docker runs as before)"),
            Err(b) => format!("{e}; rolling back also failed: {b}"),
        });
    }

    Ok(json!({ "listening": format!("{listen}:{port}"), "restarted": true }))
}

async fn remove() -> Result<Value, String> {
    let had = Path::new(DROP_IN).exists();
    if had {
        std::fs::remove_file(DROP_IN).map_err(|e| format!("{DROP_IN}: {e}"))?;
        restart_and_wait().await?;
    }

    // The key and certificates are useless without the listener; the CA is
    // dropped on the control plane, so nothing it signed opens anything.
    let _ = std::fs::remove_dir_all(DIR);

    Ok(json!({ "removed": had }))
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    #[test]
    fn exec_start_is_read_from_systemd() {
        let shown = "{ path=/usr/bin/dockerd ; argv[]=/usr/bin/dockerd -H fd:// --containerd=/run/containerd/containerd.sock ; ignore_errors=no ; start_time=[n/a] ; stop_time=[n/a] ; pid=0 ; code=(null) ; status=0/0 }";
        assert_eq!(
            parse_exec_start(shown).unwrap(),
            "/usr/bin/dockerd -H fd:// --containerd=/run/containerd/containerd.sock"
        );
        assert!(parse_exec_start("{ path=/bin/true ; argv[]=/bin/true ; }").is_none());
    }

    #[test]
    fn the_drop_in_keeps_the_socket_and_adds_tls() {
        let unit = drop_in("/usr/bin/dockerd -H fd://", "10.0.0.5", 2376);
        assert!(unit.starts_with(MARK));
        assert!(unit.contains("ExecStart=\nExecStart=/usr/bin/dockerd -H fd:// -H tcp://10.0.0.5:2376 --tlsverify"));
        assert!(unit.contains("--tlscacert=/etc/docker/forge-tls/ca.pem"));
    }
}
