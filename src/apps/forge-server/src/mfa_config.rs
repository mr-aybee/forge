//! MFA as the operator's choice (DECISIONS.md D-064): `forge-server mfa
//! status|on|off` edits `[security] require_mfa` in `forge.toml` in place,
//! and the server records a change of the setting at its next start.

use std::path::Path;

/// File in the data directory with the last setting the server ran with.
const STATE_FILE: &str = "mfa-setting";

/// `[security] require_mfa` as `forge.toml` sets it, with the deprecated
/// top-level `dev_skip_mfa = true` meaning "off". Missing means required.
pub fn read(text: &str) -> anyhow::Result<bool> {
    let v: toml::Value = toml::from_str(text)?;
    let dev_skip = v.get("dev_skip_mfa").and_then(toml::Value::as_bool).unwrap_or(false);
    let require = v.get("security").and_then(|s| s.get("require_mfa")).and_then(toml::Value::as_bool).unwrap_or(true);
    Ok(require && !dev_skip)
}

fn key_of(line: &str) -> Option<&str> {
    let t = line.trim_start();
    if t.starts_with('#') || t.starts_with('[') {
        return None;
    }
    let (k, _) = t.split_once('=')?;
    Some(k.trim().trim_matches('"'))
}

fn header_of(line: &str) -> Option<&str> {
    let t = line.trim();
    let inner = t.strip_prefix('[')?;
    let inner = inner.split('#').next().unwrap_or(inner).trim_end();
    let inner = inner.strip_suffix(']')?;
    Some(inner.trim_matches(|c| c == '[' || c == ']').trim())
}

/// Sets `[security] require_mfa` and turns the deprecated `dev_skip_mfa`
/// off, keeping every other line (comments included) as it is.
pub fn set(text: &str, required: bool) -> anyhow::Result<String> {
    let mut out: Vec<String> = Vec::new();
    let mut table: Option<String> = None;
    let mut done = false;
    let mut security_header_at: Option<usize> = None;
    for line in text.lines() {
        if let Some(h) = header_of(line) {
            table = Some(h.to_owned());
            out.push(line.to_owned());
            if h == "security" && security_header_at.is_none() {
                security_header_at = Some(out.len());
            }
            continue;
        }
        match (table.as_deref(), key_of(line)) {
            (None, Some("dev_skip_mfa")) => out.push("dev_skip_mfa = false".into()),
            (Some("security"), Some("require_mfa")) if !done => {
                out.push(format!("require_mfa = {required}"));
                done = true;
            }
            _ => out.push(line.to_owned()),
        }
    }
    if !done {
        match security_header_at {
            Some(i) => out.insert(i, format!("require_mfa = {required}")),
            None => {
                if out.last().is_some_and(|l| !l.trim().is_empty()) {
                    out.push(String::new());
                }
                out.push("[security]".into());
                out.push(format!("require_mfa = {required}"));
            }
        }
    }
    let mut s = out.join("\n");
    s.push('\n');
    if read(&s)? != required {
        anyhow::bail!("could not set [security] require_mfa in this file; edit it by hand");
    }
    Ok(s)
}

/// `[security] allow_elevated_tokens` as `forge.toml` sets it (D-097).
/// Missing means allowed.
pub fn read_elevated(text: &str) -> anyhow::Result<bool> {
    let v: toml::Value = toml::from_str(text)?;
    Ok(v.get("security").and_then(|s| s.get("allow_elevated_tokens")).and_then(toml::Value::as_bool).unwrap_or(true))
}

/// Sets `[security] allow_elevated_tokens`, keeping every other line.
pub fn set_elevated(text: &str, allowed: bool) -> anyhow::Result<String> {
    let line = format!("allow_elevated_tokens = {allowed}");
    let mut out: Vec<String> = Vec::new();
    let mut table: Option<String> = None;
    let mut done = false;
    let mut security_header_at: Option<usize> = None;
    for l in text.lines() {
        if let Some(h) = header_of(l) {
            table = Some(h.to_owned());
            out.push(l.to_owned());
            if h == "security" && security_header_at.is_none() {
                security_header_at = Some(out.len());
            }
            continue;
        }
        if !done && table.as_deref() == Some("security") && key_of(l) == Some("allow_elevated_tokens") {
            out.push(line.clone());
            done = true;
        } else {
            out.push(l.to_owned());
        }
    }
    if !done {
        match security_header_at {
            Some(i) => out.insert(i, line),
            None => {
                if out.last().is_some_and(|l| !l.trim().is_empty()) {
                    out.push(String::new());
                }
                out.push("[security]".into());
                out.push(line);
            }
        }
    }
    let mut s = out.join("\n");
    s.push('\n');
    if read_elevated(&s)? != allowed {
        anyhow::bail!("could not set [security] allow_elevated_tokens in this file; edit it by hand");
    }
    Ok(s)
}

/// Replaces `path` with `content` atomically, keeping its mode and owner.
pub fn write_atomic(path: &Path, content: &str) -> anyhow::Result<()> {
    let meta = std::fs::metadata(path)?;
    let tmp = path.with_extension("toml.forge-tmp");
    std::fs::write(&tmp, content)?;
    let result = (|| -> anyhow::Result<()> {
        std::fs::set_permissions(&tmp, meta.permissions())?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::MetadataExt;
            std::os::unix::fs::chown(&tmp, Some(meta.uid()), Some(meta.gid()))?;
        }
        std::fs::rename(&tmp, path)?;
        Ok(())
    })();
    if result.is_err() {
        let _ = std::fs::remove_file(&tmp);
    }
    result
}

/// The setting this server last ran with and the one it runs with now, if
/// they differ; records the new one. `None` on the first start.
pub fn changed_since_last_start(data_dir: &Path, required: bool) -> Option<(Option<bool>, bool)> {
    let file = data_dir.join(STATE_FILE);
    let old = std::fs::read_to_string(&file).ok().map(|s| s.trim() == "required");
    let now = if required { "required" } else { "off" };
    if old == Some(required) {
        return None;
    }
    if let Err(e) = std::fs::write(&file, format!("{now}\n")) {
        tracing::warn!(error = %e, "could not record the MFA setting");
    }
    Some((old, required))
}

#[cfg(test)]
mod tests {
    use super::*;

    const FILE: &str =
        "# Orbyn Forge\ndev_skip_mfa = true\npublic_url = \"http://x\" # the URL\n\n[update]\nenabled = true\n";

    #[test]
    fn sets_the_key_and_keeps_comments() {
        assert!(!read(FILE).unwrap_or(true));
        let on = set(FILE, true).unwrap_or_default();
        assert!(read(&on).unwrap_or(false));
        assert!(on.contains("# Orbyn Forge") && on.contains("# the URL") && on.contains("[update]"));
        assert!(on.contains("dev_skip_mfa = false"));
        assert!(on.ends_with("[security]\nrequire_mfa = true\n"), "{on}");
        let off = set(&on, false).unwrap_or_default();
        assert!(!read(&off).unwrap_or(true));
        assert_eq!(off.matches("require_mfa").count(), 1, "{off}");
        let again = set(&off, true).unwrap_or_default();
        assert_eq!(again, on, "on → off → on is stable");
    }

    #[test]
    fn inserts_into_an_existing_security_table() {
        let f = "[security]\nlogin_challenge = true\n\n[update]\nenabled = true\n";
        let s = set(f, false).unwrap_or_default();
        assert!(s.starts_with("[security]\nrequire_mfa = false\nlogin_challenge = true\n"), "{s}");
        assert!(read("").unwrap_or(false), "missing means required");
    }

    #[test]
    fn remembers_the_last_setting() {
        let dir = tempfile::tempdir().unwrap_or_else(|_| unreachable!());
        assert_eq!(changed_since_last_start(dir.path(), true), Some((None, true)));
        assert_eq!(changed_since_last_start(dir.path(), true), None);
        assert_eq!(changed_since_last_start(dir.path(), false), Some((Some(true), false)));
    }

    #[test]
    fn sets_the_elevated_token_policy() {
        assert!(read_elevated(FILE).unwrap_or(false), "missing means allowed");
        let off = set_elevated(FILE, false).unwrap_or_default();
        assert!(!read_elevated(&off).unwrap_or(true));
        assert!(off.contains("# the URL") && off.ends_with("[security]\nallow_elevated_tokens = false\n"), "{off}");
        let on = set_elevated(&off, true).unwrap_or_default();
        assert_eq!(on.matches("allow_elevated_tokens").count(), 1, "{on}");
        assert!(read(&on).unwrap_or(true) == read(FILE).unwrap_or(true), "require_mfa untouched");
    }
}
