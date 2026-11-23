//! `forge update`: signed self-update of the CLI (issue #5, D-076).
//!
//! 1. Reads the release feed (GitHub "latest release" document;
//!    `FORGE_UPDATE_FEED` points at a mirror or a test feed).
//! 2. Downloads `SHA256SUMS` and `SHA256SUMS.sig` and checks the Ed25519
//!    signature with the compiled-in release key (`FORGE_UPDATE_PUBLIC_KEY`
//!    replaces it for a private feed).
//! 3. Downloads `forge-cli-<target>` and checks its SHA-256 against the sums.
//! 4. Writes it next to the running binary, runs it once (`--version` must
//!    print the release's version), then swaps it in. The old binary stays as
//!    `forge.prev` (`forge.exe.prev`) until the next update.
//!
//! Nothing is replaced unless every check passes. Feeds and downloads must
//! use HTTPS (plain HTTP only for loopback addresses).

use std::path::{Path, PathBuf};
use std::time::Duration;

use serde_json::{Value, json};

use crate::Global;
use crate::cmds::confirm;
use crate::error::{CliError, Exit, Result};
use crate::out;

/// Largest file `forge update` downloads.
const MAX_DOWNLOAD: usize = 200 * 1024 * 1024;

/// The release asset name of this platform's CLI.
#[must_use]
pub fn asset_name() -> Option<&'static str> {
    if cfg!(all(target_os = "linux", target_arch = "x86_64")) {
        Some("forge-cli-x86_64-unknown-linux-musl")
    } else if cfg!(all(target_os = "linux", target_arch = "aarch64")) {
        Some("forge-cli-aarch64-unknown-linux-musl")
    } else if cfg!(all(windows, target_arch = "x86_64")) {
        Some("forge-cli-x86_64-pc-windows-msvc.exe")
    } else if cfg!(all(target_os = "macos", target_arch = "aarch64")) {
        Some("forge-cli-aarch64-apple-darwin")
    } else if cfg!(all(target_os = "macos", target_arch = "x86_64")) {
        Some("forge-cli-x86_64-apple-darwin")
    } else {
        None
    }
}

/// HTTPS, or HTTP to a loopback address (test feeds).
fn allowed_url(u: &str) -> Result<url::Url> {
    let url = url::Url::parse(u).map_err(|e| CliError::usage(format!("{u}: {e}")))?;
    let loopback = match url.host() {
        Some(url::Host::Domain(d)) => d.eq_ignore_ascii_case("localhost"),
        Some(url::Host::Ipv4(ip)) => ip.is_loopback(),
        Some(url::Host::Ipv6(ip)) => ip.is_loopback(),
        None => false,
    };
    match url.scheme() {
        "https" => Ok(url),
        "http" if loopback => Ok(url),
        _ => Err(CliError::usage(format!("refusing {u}: release downloads must use HTTPS"))),
    }
}

fn client() -> Result<reqwest::Client> {
    // GitHub serves assets through redirects to its storage host; no
    // credentials are ever sent, so following them is safe. Each hop must
    // stay on HTTPS (or loopback).
    let policy = reqwest::redirect::Policy::custom(|attempt| {
        if attempt.previous().len() >= 5 || allowed_url(attempt.url().as_str()).is_err() {
            attempt.stop()
        } else {
            attempt.follow()
        }
    });
    reqwest::Client::builder()
        .connect_timeout(Duration::from_secs(15))
        .timeout(Duration::from_secs(600))
        .redirect(policy)
        .user_agent(concat!("forge-cli/", env!("CARGO_PKG_VERSION")))
        .build()
        .map_err(|e| CliError::new(Exit::Transport, e.to_string()))
}

async fn fetch(c: &reqwest::Client, url: &str, accept: Option<&str>) -> Result<Vec<u8>> {
    allowed_url(url)?;
    let mut req = c.get(url);
    if let Some(a) = accept {
        req = req.header(reqwest::header::ACCEPT, a);
    }
    let mut resp = req.send().await.map_err(|e| CliError::new(Exit::Transport, format!("{url}: {e}")))?;
    if !resp.status().is_success() {
        return Err(CliError::new(Exit::Transport, format!("{url}: HTTP {}", resp.status())));
    }
    let mut body = Vec::new();
    while let Some(chunk) = resp.chunk().await.map_err(|e| CliError::new(Exit::Transport, format!("{url}: {e}")))? {
        if body.len() + chunk.len() > MAX_DOWNLOAD {
            return Err(CliError::new(Exit::Transport, format!("{url}: larger than {MAX_DOWNLOAD} bytes")));
        }
        body.extend_from_slice(&chunk);
    }
    Ok(body)
}

/// `(version, release page, assets)` from a GitHub release document.
fn parse_release(doc: &Value) -> Option<(String, String, std::collections::BTreeMap<String, String>)> {
    let version = doc.get("tag_name")?.as_str()?.trim_start_matches('v').to_owned();
    if !forge_catalog::versions::valid_build(&version) {
        return None;
    }
    let assets = doc
        .get("assets")?
        .as_array()?
        .iter()
        .filter_map(|a| {
            Some((a.get("name")?.as_str()?.to_owned(), a.get("browser_download_url")?.as_str()?.to_owned()))
        })
        .collect();
    Some((version, doc.get("html_url").and_then(Value::as_str).unwrap_or_default().to_owned(), assets))
}

fn exe_path() -> Result<PathBuf> {
    let p = std::env::current_exe().map_err(|e| CliError::new(Exit::General, format!("where is this binary? {e}")))?;
    // A symlink (Homebrew, /usr/local/bin) is replaced at its target.
    Ok(std::fs::canonicalize(&p).unwrap_or(p))
}

fn sibling(exe: &Path, suffix: &str) -> PathBuf {
    let name = exe.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| "forge".into());
    exe.with_file_name(format!("{name}{suffix}"))
}

fn denied(path: &Path, e: &std::io::Error) -> CliError {
    let hint = if e.kind() == std::io::ErrorKind::PermissionDenied {
        " (no permission: run it as the user that installed forge, e.g. with sudo, or reinstall)"
    } else {
        ""
    };
    CliError::new(Exit::General, format!("{}: {e}{hint}", path.display())).with_code("install_failed")
}

/// Writes, test-runs and swaps in the new binary. On any failure the
/// running binary stays as it was.
fn install(exe: &Path, bytes: &[u8], version: &str) -> Result<PathBuf> {
    let staged = sibling(exe, &format!(".new-{}", std::process::id()));
    let prev = sibling(exe, ".prev");
    std::fs::write(&staged, bytes).map_err(|e| denied(&staged, &e))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&staged, std::fs::Permissions::from_mode(0o755)).map_err(|e| denied(&staged, &e))?;
    }
    let ran = std::process::Command::new(&staged).arg("--version").output();
    let ok = ran.as_ref().is_ok_and(|o| o.status.success() && String::from_utf8_lossy(&o.stdout).contains(version));
    if !ok {
        let _ = std::fs::remove_file(&staged);
        return Err(CliError::new(
            Exit::General,
            format!("the downloaded forge {version} did not start on this machine; nothing was changed"),
        )
        .with_code("install_failed"));
    }
    // The previous update's leftover (Windows cannot delete a running binary).
    if prev.exists() {
        let _ = std::fs::remove_file(&prev);
    }
    std::fs::rename(exe, &prev).map_err(|e| {
        let _ = std::fs::remove_file(&staged);
        denied(exe, &e)
    })?;
    if let Err(e) = std::fs::rename(&staged, exe) {
        // Put the old binary back.
        let _ = std::fs::rename(&prev, exe);
        let _ = std::fs::remove_file(&staged);
        return Err(denied(exe, &e));
    }
    Ok(prev)
}

pub async fn run(g: &Global, check: bool, yes: bool, allow_downgrade: bool) -> Result<()> {
    let current = crate::VERSION;
    let feed = std::env::var("FORGE_UPDATE_FEED")
        .ok()
        .filter(|s| !s.trim().is_empty())
        .unwrap_or_else(|| forge_sdk::release::DEFAULT_FEED.to_owned());
    let key = std::env::var("FORGE_UPDATE_PUBLIC_KEY")
        .ok()
        .filter(|s| !s.trim().is_empty())
        .unwrap_or_else(|| forge_sdk::release::RELEASE_PUBLIC_KEY.to_owned());
    let c = client()?;
    let doc: Value = serde_json::from_slice(&fetch(&c, &feed, Some("application/vnd.github+json")).await?)
        .map_err(|e| CliError::new(Exit::Transport, format!("{feed}: not a release document ({e})")))?;
    let (latest, page, assets) =
        parse_release(&doc).ok_or_else(|| CliError::new(Exit::Transport, format!("{feed}: no usable release")))?;
    let order = forge_sdk::release::compare_release(&latest, current);
    let available = order == std::cmp::Ordering::Greater;
    if check {
        out::result(
            g.output,
            &json!({ "current": current, "latest": latest, "available": available, "url": page, "feed": feed }),
            None,
        );
        return Ok(());
    }
    if order == std::cmp::Ordering::Equal {
        out::result(g.output, &json!({ "current": current, "latest": latest, "updated": false }), None);
        if g.output == out::Format::Table {
            eprintln!("forge {current} is the latest release.");
        }
        return Ok(());
    }
    if order == std::cmp::Ordering::Less && !allow_downgrade {
        return Err(CliError::new(
            Exit::Conflict,
            format!("the feed's release ({latest}) is older than this forge ({current}); pass --allow-downgrade to install it"),
        )
        .with_code("downgrade"));
    }
    let name = asset_name().ok_or_else(|| CliError::usage("no forge release is built for this platform"))?;
    let (Some(sums_url), Some(sig_url), Some(bin_url)) =
        (assets.get("SHA256SUMS"), assets.get("SHA256SUMS.sig"), assets.get(name))
    else {
        return Err(
            CliError::new(Exit::NotFound, format!("release {latest} has no signed {name}")).with_code("no_asset")
        );
    };
    let exe = exe_path()?;
    confirm(g, yes, &format!("Update forge {current} → {latest} ({})?", exe.display()))?;

    let sums = fetch(&c, sums_url, None).await?;
    let sig = String::from_utf8_lossy(&fetch(&c, sig_url, None).await?).into_owned();
    forge_sdk::release::verify_signature(&key, &sums, &sig)
        .map_err(|e| CliError::new(Exit::General, format!("{e}; nothing was changed")).with_code("bad_signature"))?;
    let want = forge_sdk::release::parse_sums(&String::from_utf8_lossy(&sums))
        .get(name)
        .cloned()
        .ok_or_else(|| CliError::new(Exit::General, format!("the signed SHA256SUMS does not list {name}")))?;
    let bytes = fetch(&c, bin_url, None).await?;
    if forge_sdk::release::sha256_hex(&bytes) != want {
        return Err(CliError::new(
            Exit::General,
            format!("{name} does not match the signed SHA256SUMS; nothing was changed"),
        )
        .with_code("bad_checksum"));
    }
    let prev = install(&exe, &bytes, &latest)?;
    out::result(
        g.output,
        &json!({ "previous": current, "current": latest, "updated": true, "path": exe, "backup": prev }),
        None,
    );
    if g.output == out::Format::Table {
        eprintln!("Updated forge {current} → {latest}. The old binary is kept as {}.", prev.display());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn feeds_must_be_https_except_loopback() {
        assert!(allowed_url("https://api.github.com/x").is_ok());
        assert!(allowed_url("http://127.0.0.1:8000/release.json").is_ok());
        assert!(allowed_url("http://localhost/x").is_ok());
        assert!(allowed_url("http://[::1]/x").is_ok());
        assert!(allowed_url("http://example.com/x").is_err());
        assert!(allowed_url("http://127.0.0.1.example.com/x").is_err());
        assert!(allowed_url("file:///etc/passwd").is_err());
    }

    #[test]
    fn releases_parse_with_their_assets() {
        let doc = json!({
            "tag_name": "v0.2.0", "html_url": "https://h/r",
            "assets": [{ "name": "SHA256SUMS", "browser_download_url": "https://h/s" }]
        });
        let (v, page, assets) = parse_release(&doc).unwrap_or_default();
        assert_eq!((v.as_str(), page.as_str()), ("0.2.0", "https://h/r"));
        assert_eq!(assets["SHA256SUMS"], "https://h/s");
        assert!(parse_release(&json!({ "tag_name": "" })).is_none());
    }

    #[test]
    fn this_platform_has_an_asset_name() {
        assert!(asset_name().is_some_and(|n| n.starts_with("forge-cli-")));
    }
}
