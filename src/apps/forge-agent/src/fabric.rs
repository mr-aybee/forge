//! Forge network fabric on the node: the cluster's WireGuard mesh (root,
//! inside `forge-agent-helper`).
//!
//! The private key is generated here with `wg genkey`, stored readable by root
//! only, and never sent anywhere: [`FabricOp::Key`] returns the public key,
//! and [`FabricOp::Apply`] splices the private key into the config locally.
//! Every operation is idempotent so the control plane can simply retry.

use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::Duration;

use forge_catalog::fabric::{FabricOp, MeshConfig, valid_key};
#[cfg(not(unix))]
use forge_proto::messages::RefusalCode;
use serde_json::{Value, json};
use tokio::io::AsyncWriteExt;
use tokio::process::Command;

use crate::executor::CommandError;

#[cfg(unix)]
mod network;

const T: Duration = Duration::from_secs(120);

async fn run(cmd: &str, args: &[&str]) -> Result<String, String> {
    let out = tokio::time::timeout(crate::recipes::command_timeout(cmd, T), Command::new(cmd).args(args).output())
        .await
        .map_err(|_| format!("{cmd}: timed out"))?
        .map_err(|e| format!("{cmd}: {e}"))?;
    let text = format!("{}{}", String::from_utf8_lossy(&out.stdout), String::from_utf8_lossy(&out.stderr));
    if out.status.success() { Ok(text) } else { Err(format!("{cmd} {}: {}", args.join(" "), text.trim())) }
}

/// Run with `input` on stdin (used for `wg pubkey`, so the key is never an argument).
async fn run_stdin(cmd: &str, args: &[&str], input: &str) -> Result<String, String> {
    let mut child = Command::new(cmd)
        .args(args)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| format!("{cmd}: {e}"))?;

    if let Some(mut stdin) = child.stdin.take() {
        stdin.write_all(input.as_bytes()).await.map_err(|e| format!("{cmd}: {e}"))?;
    }

    let out = tokio::time::timeout(T, child.wait_with_output())
        .await
        .map_err(|_| format!("{cmd}: timed out"))?
        .map_err(|e| format!("{cmd}: {e}"))?;

    if out.status.success() {
        Ok(String::from_utf8_lossy(&out.stdout).trim().to_owned())
    } else {
        Err(format!("{cmd}: {}", String::from_utf8_lossy(&out.stderr).trim()))
    }
}

fn dir() -> PathBuf {
    if cfg!(windows) {
        crate::config::windows_data_dir().join("wireguard")
    } else {
        PathBuf::from("/etc/forge/wireguard")
    }
}

fn wg() -> String {
    if cfg!(windows) {
        let p = r"C:\Program Files\WireGuard\wg.exe";
        if Path::new(p).exists() { p.to_owned() } else { "wg".to_owned() }
    } else {
        "wg".to_owned()
    }
}

#[cfg(windows)]
const WIREGUARD_EXE: &str = r"C:\Program Files\WireGuard\wireguard.exe";

/// Write a file only root (or SYSTEM / Administrators) can read.
fn write_private(path: &Path, content: &str) -> Result<(), String> {
    if let Some(d) = path.parent() {
        std::fs::create_dir_all(d).map_err(|e| format!("{}: {e}", d.display()))?;
    }
    #[cfg(unix)]
    {
        use std::io::Write;
        use std::os::unix::fs::OpenOptionsExt;
        let tmp = path.with_extension("tmp");
        let mut f = std::fs::OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .mode(0o600)
            .open(&tmp)
            .map_err(|e| format!("{}: {e}", tmp.display()))?;
        f.write_all(content.as_bytes()).map_err(|e| format!("{}: {e}", tmp.display()))?;
        f.sync_all().map_err(|e| format!("{}: {e}", tmp.display()))?;
        std::fs::rename(&tmp, path).map_err(|e| format!("{}: {e}", path.display()))
    }
    #[cfg(not(unix))]
    {
        // %ProgramData%\OrbynForge is created by install.ps1, owned by
        // Administrators, for SYSTEM and Administrators only (D-079).
        std::fs::write(path, content).map_err(|e| format!("{}: {e}", path.display()))
    }
}

async fn installed() -> bool {
    #[cfg(windows)]
    {
        Path::new(WIREGUARD_EXE).exists()
    }
    #[cfg(not(windows))]
    {
        run("wg", &["--version"]).await.is_ok()
    }
}

async fn install() -> Result<Value, String> {
    if installed().await {
        return Ok(json!({ "installed": true, "changed": false }));
    }

    #[cfg(windows)]
    {
        run(
            "winget",
            &[
                "install",
                "--id",
                "WireGuard.WireGuard",
                "-e",
                "--silent",
                "--accept-package-agreements",
                "--accept-source-agreements",
            ],
        )
        .await
        .map_err(|e| format!("installing WireGuard for Windows with winget failed: {e}"))?;
    }
    #[cfg(not(windows))]
    {
        let attempts: [(&str, &[&str]); 5] = [
            ("apt-get", &["-o", crate::recipes::APT_LOCK_TIMEOUT, "install", "-y", "wireguard-tools"]),
            ("dnf", &["install", "-y", "wireguard-tools"]),
            ("yum", &["install", "-y", "wireguard-tools"]),
            ("zypper", &["--non-interactive", "install", "wireguard-tools"]),
            ("apk", &["add", "wireguard-tools"]),
        ];
        let mut last = String::from("no supported package manager (apt, dnf, yum, zypper, apk) found");
        let mut done = false;
        for (pm, args) in attempts {
            if run("sh", &["-c", &format!("command -v {pm}")]).await.is_err() {
                continue;
            }

            if pm == "apt-get" {
                let _ = run("apt-get", &["-o", crate::recipes::APT_LOCK_TIMEOUT, "update"]).await;
            }

            match run(pm, args).await {
                Ok(_) => {
                    done = true;
                    break;
                }
                Err(e) => last = e,
            }

            // RHEL 8 and some rebuilds ship wireguard-tools in EPEL only.
            if matches!(pm, "dnf" | "yum") && run(pm, &["install", "-y", "epel-release"]).await.is_ok() {
                match run(pm, args).await {
                    Ok(_) => {
                        done = true;
                        break;
                    }
                    Err(e) => last = e,
                }
            }
        }
        if !done {
            return Err(format!(
                "could not install wireguard-tools: {last} (on RHEL-like nodes it may need the EPEL repository)"
            ));
        }
    }
    if !installed().await {
        return Err("WireGuard was installed but `wg` is still not available".into());
    }

    Ok(json!({ "installed": true, "changed": true }))
}

fn key_path(iface: &str) -> PathBuf {
    dir().join(format!("{iface}.key"))
}

async fn private_key(iface: &str) -> Result<String, String> {
    let p = key_path(iface);
    if let Ok(k) = std::fs::read_to_string(&p) {
        let k = k.trim().to_owned();
        if k.len() == 44 {
            return Ok(k);
        }
    }

    let k = run(&wg(), &["genkey"]).await?.trim().to_owned();
    write_private(&p, &k)?;

    Ok(k)
}

async fn key(iface: &str) -> Result<Value, String> {
    if !installed().await {
        return Err("WireGuard is not installed yet".into());
    }

    let private = private_key(iface).await?;
    let public = run_stdin(&wg(), &["pubkey"], &private).await?;
    valid_key(&public)?;

    Ok(json!({ "public_key": public }))
}

fn conf_path(iface: &str) -> PathBuf {
    if cfg!(windows) {
        dir().join(format!("{iface}.conf"))
    } else {
        PathBuf::from(format!("/etc/wireguard/{iface}.conf"))
    }
}

async fn apply(c: &MeshConfig) -> Result<Value, String> {
    if !installed().await {
        return Err("WireGuard is not installed yet".into());
    }
    let private = private_key(&c.iface).await?;
    let text = c.render(&private);
    let path = conf_path(&c.iface);
    let unchanged = std::fs::read_to_string(&path).is_ok_and(|old| old == text);
    write_private(&path, &text)?;
    let path_s = path.to_string_lossy().into_owned();

    #[cfg(windows)]
    {
        let _ = &path_s;
        let running = run("sc.exe", &["query", &format!("WireGuardTunnel${}", c.iface)])
            .await
            .is_ok_and(|o| o.contains("RUNNING"));
        if !(running && unchanged) {
            if running {
                let _ = run(WIREGUARD_EXE, &["/uninstalltunnelservice", &c.iface]).await;
                tokio::time::sleep(Duration::from_secs(2)).await;
            }

            run(WIREGUARD_EXE, &["/installtunnelservice", &path_s]).await?;
        }
        if c.forward {
            return Err("this node was chosen as a relay, but relaying needs a Linux node".into());
        }
        Ok(json!({ "iface": c.iface, "changed": !(running && unchanged), "peers": c.peers.len() }))
    }

    #[cfg(not(windows))]
    {
        let up = run("ip", &["link", "show", &c.iface]).await.is_ok();
        if !up {
            run("wg-quick", &["up", &c.iface]).await?;
        } else if !unchanged {
            // Live update without dropping the tunnel: peers via syncconf,
            // address and routes via ip.
            let stripped = run("wg-quick", &["strip", &c.iface]).await?;
            let tmp = dir().join(format!("{}.sync", c.iface));
            write_private(&tmp, &stripped)?;
            let r = run(&wg(), &["syncconf", &c.iface, &tmp.to_string_lossy()]).await;
            let _ = std::fs::remove_file(&tmp);
            r?;
            run("ip", &["address", "replace", &c.address, "dev", &c.iface]).await?;
        }
        // Routes for every peer's addresses (wg-quick adds them only on `up`).
        for p in &c.peers {
            for net in &p.allowed_ips {
                run("ip", &["route", "replace", &net.to_string(), "dev", &c.iface]).await?;
            }
        }
        if Path::new("/run/systemd/system").is_dir() {
            let _ = run("systemctl", &["enable", &format!("wg-quick@{}", c.iface)]).await;
        }
        open_firewalld(c.port).await;
        if c.forward {
            run("sysctl", &["-w", "net.ipv4.ip_forward=1"]).await?;
            write_private(Path::new("/etc/sysctl.d/90-forge-fabric.conf"), "net.ipv4.ip_forward = 1\n")?;
        }
        let _ = path_s;
        Ok(json!({ "iface": c.iface, "changed": !up || !unchanged, "peers": c.peers.len(), "forward": c.forward }))
    }
}

/// Opens the WireGuard port in firewalld when it runs (RHEL-likes enable it by
/// default and drop the tunnel's UDP traffic otherwise). Idempotent; other
/// firewalls (ufw, nftables) are left to the operator.
#[cfg(not(windows))]
async fn open_firewalld(port: u16) {
    if run("firewall-cmd", &["--state"]).await.is_err() {
        return;
    }

    let rule = format!("{port}/udp");
    if run("firewall-cmd", &["--query-port", &rule]).await.is_ok() {
        return;
    }

    let _ = run("firewall-cmd", &["--permanent", "--add-port", &rule]).await;
    let _ = run("firewall-cmd", &["--add-port", &rule]).await;
}

/// `wg show <iface> dump`: the interface line, then one line per peer:
/// public-key, preshared, endpoint, allowed-ips, latest-handshake, rx, tx, keepalive.
async fn status(iface: &str) -> Result<Value, String> {
    if !installed().await {
        return Ok(json!({ "installed": false, "up": false, "peers": [] }));
    }

    let Ok(out) = run(&wg(), &["show", iface, "dump"]).await else {
        return Ok(json!({ "installed": true, "up": false, "peers": [] }));
    };

    let mut lines = out.lines();
    let iface_line = lines.next().unwrap_or_default();
    let public = iface_line.split('\t').nth(1).unwrap_or_default();
    let peers: Vec<Value> = lines
        .filter_map(|l| {
            let f: Vec<&str> = l.split('\t').collect();
            (f.len() >= 7).then(|| {
                json!({
                    "public_key": f[0],
                    "endpoint": (f[2] != "(none)").then_some(f[2]),
                    "latest_handshake": f[4].parse::<u64>().unwrap_or(0),
                    "rx": f[5].parse::<u64>().unwrap_or(0),
                    "tx": f[6].parse::<u64>().unwrap_or(0),
                })
            })
        })
        .collect();

    Ok(json!({ "installed": true, "up": true, "public_key": public, "peers": peers }))
}

async fn remove(iface: &str) -> Result<Value, String> {
    #[cfg(windows)]
    {
        let _ = run(WIREGUARD_EXE, &["/uninstalltunnelservice", iface]).await;
    }
    #[cfg(not(windows))]
    {
        if run("ip", &["link", "show", iface]).await.is_ok() {
            let _ = run("wg-quick", &["down", iface]).await;
        }
        if Path::new("/run/systemd/system").is_dir() {
            let _ = run("systemctl", &["disable", &format!("wg-quick@{iface}")]).await;
        }
    }
    for p in [conf_path(iface), key_path(iface)] {
        match std::fs::remove_file(&p) {
            Err(e) if e.kind() != std::io::ErrorKind::NotFound => return Err(format!("{}: {e}", p.display())),
            _ => {}
        }
    }

    Ok(json!({ "removed": iface }))
}

/// Runs one fabric operation. Refusals the control plane tells apart (an
/// old or missing Docker for the container network) keep their code.
pub async fn run_op(op: &FabricOp) -> Result<Value, CommandError> {
    op.validate()?;

    match op {
        FabricOp::Install => Ok(install().await?),
        FabricOp::Key { iface } => Ok(key(iface).await?),
        FabricOp::Apply { config } => {
            let mut out = apply(config).await?;
            out["hosts"] = json!(write_hosts(&config.iface, &config.hosts)?);

            Ok(out)
        }
        FabricOp::Status { iface } => Ok(status(iface).await?),
        FabricOp::Remove { iface } => {
            // The container network goes first: with containers still on
            // it, the fabric stays and the control plane reports the node.
            network_remove(iface).await?;

            let out = remove(iface).await?;
            write_hosts(iface, &[])?;

            Ok(out)
        }
        FabricOp::NetworkEnsure { iface, subnet, mtu } => network_ensure(iface, subnet, *mtu).await,
        FabricOp::NetworkRemove { iface } => Ok(network_remove(iface).await?),
    }
}

#[cfg(unix)]
async fn network_ensure(iface: &str, subnet: &forge_catalog::fabric::Cidr, mtu: u16) -> Result<Value, CommandError> {
    network::ensure(iface, subnet, mtu).await
}

#[cfg(not(unix))]
async fn network_ensure(_: &str, _: &forge_catalog::fabric::Cidr, _: u16) -> Result<Value, CommandError> {
    Err(CommandError::refused(RefusalCode::Unsupported, "container networks need a Linux node"))
}

#[cfg(unix)]
async fn network_remove(iface: &str) -> Result<Value, String> {
    network::remove(iface).await
}

/// Windows nodes never get a container network, so there is nothing to remove.
#[cfg(not(unix))]
async fn network_remove(_: &str) -> Result<Value, String> {
    Ok(json!({ "removed": false }))
}

/// Rewrites this fabric's block in the hosts file (cluster names → mesh
/// addresses), the same safe way as the fleet's names (D-084). Returns the
/// number of names.
fn write_hosts(iface: &str, hosts: &[forge_catalog::fabric::HostEntry]) -> Result<usize, String> {
    let entries: Vec<forge_catalog::names::NameEntry> = hosts
        .iter()
        .map(|h| forge_catalog::names::NameEntry { name: h.name.clone(), ip: std::net::IpAddr::V4(h.ip) })
        .collect();
    crate::host_names::write_block(iface, &entries)?;

    Ok(hosts.len())
}
