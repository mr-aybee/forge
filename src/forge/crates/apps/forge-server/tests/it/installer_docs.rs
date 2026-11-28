//! Fails when an option of the server installers (`scripts/packaging/install.sh`
//! and `install.ps1`) is missing from `reference/cli/server-installer.md`.
#![allow(clippy::unwrap_used)]

use std::path::PathBuf;

fn repo() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../../../..")
}

fn read(rel: &str) -> String {
    std::fs::read_to_string(repo().join(rel)).unwrap_or_else(|e| panic!("{rel}: {e}"))
}

/// `--flag)` cases of the shell installer's argument loop.
fn shell_flags(script: &str) -> Vec<String> {
    script
        .lines()
        .map(str::trim)
        .filter(|l| l.starts_with("--") && l.contains(')'))
        .filter_map(|l| l.split(')').next())
        .flat_map(|cases| cases.split('|').map(|c| c.trim().to_owned()).collect::<Vec<_>>())
        .filter(|c| {
            c.starts_with("--") && c.len() > 2 && c[2..].chars().all(|ch| ch.is_ascii_alphanumeric() || ch == '-')
        })
        .collect()
}

/// `[type]$Name` parameters of the PowerShell installer's `param(...)` block.
fn ps_params(script: &str) -> Vec<String> {
    let Some(start) = script.find("param(") else { return Vec::new() };
    let block = &script[start..script[start..].find("\n)").map_or(script.len(), |e| start + e)];

    block
        .split('$')
        .skip(1)
        .filter_map(|rest| {
            let name: String = rest.chars().take_while(|c| c.is_ascii_alphanumeric()).collect();
            (!name.is_empty()).then(|| format!("-{name}"))
        })
        .filter(|p| p != "-env")
        .collect()
}

#[test]
fn every_server_installer_option_is_documented() {
    let doc = read("docs/src/content/docs/reference/cli/server-installer.md");
    let mut options = shell_flags(&read("scripts/packaging/install.sh"));
    options.extend(ps_params(&read("scripts/packaging/install.ps1")));
    assert!(options.iter().any(|o| o == "--mfa"), "parsed the shell flags: {options:?}");
    let missing: Vec<&String> = options.iter().filter(|o| !doc.contains(o.as_str())).collect();
    assert!(missing.is_empty(), "undocumented in reference/cli/server-installer.md: {missing:?}");
}
