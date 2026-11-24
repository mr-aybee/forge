---
title: Platforms
description: The operating systems and architectures each Forge binary is built for, and the outbound connections the control plane makes.
sidebar:
  order: 9
---

This page lists where each Forge component runs and what the release
publishes for it.

## Components

| Component | Linux | Windows | macOS |
| --- | --- | --- | --- |
| `forge-server` | `x86_64`, `aarch64`; static musl | source only, not verified | not supported |
| `forge-analytics-worker` | `x86_64`, `aarch64`; DuckDB, embedded chDB or ClickHouse | source only, not verified | not supported |
| `forge-plugin-runner` | `x86_64`, `aarch64` | source only, not verified | not supported |
| `forge-agent` + `forge-agent-helper` | `x86_64`, `aarch64`; systemd distributions | source only, not verified (no helper) | not supported |
| `forge` CLI ([install](/docs/reference/cli/#forge)) | `x86_64`, `aarch64`; static musl | `x86_64` (MSVC) | no release binary; build it from source |

Releases publish the control plane and the agent for Linux only. The control
plane runs on Debian 12+, Ubuntu 22.04+, RHEL, Rocky and Alma 8+ with systemd.

:::note
The Windows control plane and the Windows agent are not in releases yet. The
Windows code paths still exist in the source, so you can build
`forge-server.exe` and `forge-agent.exe` yourself
(`cargo build --release --target x86_64-pc-windows-msvc`), but these builds
are not verified. On Windows, releases publish only the `forge` CLI.
:::

Release builds use thin LTO, 16 codegen units and `panic = "abort"`. Linux
binaries are static musl builds with no runtime library dependencies. The
server needs the system CA bundle (`ca-certificates` on Debian, Ubuntu and
RHEL-likes) for outbound HTTPS.

## Release assets

| Asset | Contents |
| --- | --- |
| `forge-<version>-x86_64-linux.tar.gz`, `forge-<version>-aarch64-linux.tar.gz` | Linux package: `install.sh`, `forge-server`, `forge-analytics-worker`, `forge-plugin-runner`, `dist/` (agent binaries, `SHA256SUMS`, `MANIFEST.json`), `forge.toml.example`, `README.txt`, `LICENSE.txt` |
| `get-forge.sh` | Download, verify and install a Linux package |
| `get-forge.ps1` | The Windows bootstrap installer. It needs a Windows package, which releases do not publish yet |
| `forge-server-<target>`, `forge-analytics-worker-<target>`, `forge-plugin-runner-<target>` | Single binaries for the self-updater (Linux targets) |
| `forge-agent-linux-<arch>`, `forge-agent-helper-linux-<arch>` | Agent binaries for the self-updater |
| `forge-cli-<target>[.exe]` | The `forge` CLI, including `forge-cli-x86_64-pc-windows-msvc.exe` |
| `SHA256SUMS`, `SHA256SUMS.sig` | SHA-256 of every asset, and its Ed25519 signature |
| `LICENSE.txt` | The Apache License 2.0 (a copy of the repository's `LICENSE`) |

`<target>` is `x86_64-unknown-linux-musl` or `aarch64-unknown-linux-musl`.
The CLI also has `x86_64-pc-windows-msvc`.

## Outbound connections from the server

| Destination | Used for |
| --- | --- |
| `github.com`, `api.github.com` | Release checks and updates. Optional; see [Update Forge](/docs/guides/update-forge/) |
| `endoflife.date` | Release lines (LTS, latest, end of life) on the Updates page, refreshed daily. Optional |
| Your OIDC issuer | Single sign-on, if configured |
