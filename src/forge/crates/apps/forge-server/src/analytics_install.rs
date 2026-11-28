//! `forge-server analytics install`: puts the embedded engine's binary in
//! place. The analytics worker runs it (D-141); forge-server only knows
//! where it lives.

use std::path::Path;

use anyhow::Context;
use forge_analytics_proto::config::EmbeddedPaths;

use crate::config::ServerConfig;

/// The embedded engine's files under `<data_dir>/analytics`.
#[must_use]
pub fn paths(config: &ServerConfig) -> EmbeddedPaths {
    EmbeddedPaths::under(&config.data_dir.join("analytics"), config.analytics.embedded_binary.clone())
}

/// Copies the engine binary into place after checking its SHA-256. Blocking.
pub fn install(paths: &EmbeddedPaths, from: &Path, sha256: &str) -> anyhow::Result<()> {
    let bytes = std::fs::read(from).with_context(|| format!("reading {}", from.display()))?;
    let got = forge_secrets::crypto::sha256_hex(&bytes);
    if !forge_secrets::crypto::constant_time_eq(got.as_bytes(), sha256.trim().to_lowercase().as_bytes()) {
        anyhow::bail!("SHA-256 mismatch: the file is {got}");
    }

    let dir = paths.binary.parent().context("binary path has no directory")?;
    std::fs::create_dir_all(dir)?;
    let tmp = paths.binary.with_extension("new");
    std::fs::write(&tmp, &bytes)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&tmp, std::fs::Permissions::from_mode(0o750))?;
    }
    std::fs::rename(&tmp, &paths.binary)?;

    Ok(())
}

#[cfg(test)]
#[allow(clippy::unwrap_used)] // reason: tests
mod tests {
    use super::*;

    #[test]
    fn install_checks_the_hash() {
        let dir = tempfile::tempdir().unwrap();
        let c = ServerConfig { data_dir: dir.path().to_path_buf(), ..ServerConfig::default() };
        let p = paths(&c);
        let src = dir.path().join("ch");
        std::fs::write(&src, b"engine").unwrap();
        assert!(install(&p, &src, "00").is_err());
        install(&p, &src, &forge_secrets::crypto::sha256_hex(b"engine")).unwrap();
        assert_eq!(std::fs::read(&p.binary).unwrap(), b"engine");
    }
}
