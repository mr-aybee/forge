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
| `forge-server` | `x86_64`, `aarch64`; static musl | `x86_64` (MSVC) | not supported |
| `forge-analytics-worker` | `x86_64`, `aarch64`; DuckDB, embedded chDB or ClickHouse | `x86_64`; ClickHouse. DuckDB is built in but not yet verified, so the installer defaults to `off` | not supported |
| `forge-plugin-runner` | `x86_64`, `aarch64` | `x86_64` | not supported |
| `forge-agent` + `forge-agent-helper` | `x86_64`, `aarch64`; systemd distributions | `x86_64`; Windows Server 2022 and 2025 (no helper) | not supported |
| `forge` CLI ([install](/docs/reference/cli/#forge)) | `x86_64`, `aarch64`; static musl | `x86_64` (MSVC) | no release binary; build it from source |

The control plane runs on Debian 12+, Ubuntu 22.04+, RHEL, Rocky and
Alma 8+ with systemd, and on Windows Server 2019, 2022 and 2025 and
Windows 10 and 11.

Release builds use LTO, one codegen unit and `panic = "abort"`. Linux
binaries are static musl builds with no runtime library dependencies. The
server needs the system CA bundle (`ca-certificates` on Debian, Ubuntu and
RHEL-likes) for outbound HTTPS.

## Release assets

| Asset | Contents |
| --- | --- |
| `forge-<version>-x86_64-linux.tar.gz`, `forge-<version>-aarch64-linux.tar.gz` | Linux package: `install.sh`, `forge-server`, `forge-analytics-worker`, `forge-plugin-runner`, `dist/` (agent binaries, `SHA256SUMS`, `MANIFEST.json`), `forge.toml.example`, `README.txt`, `LICENSE.txt` |
| `forge-<version>-x86_64-windows.zip` | Windows package: the same with `install.ps1` and `.exe` files |
| `get-forge.sh`, `get-forge.ps1` | Download, verify and install |
| `forge-server-<target>[.exe]`, `forge-analytics-worker-<target>[.exe]`, `forge-plugin-runner-<target>[.exe]` | Single binaries for the self-updater |
| `forge-agent-linux-<arch>`, `forge-agent-helper-linux-<arch>`, `forge-agent-windows-x86_64.exe` | Agent binaries for the self-updater |
| `forge-cli-<target>[.exe]` | The `forge` CLI |
| `SHA256SUMS`, `SHA256SUMS.sig` | SHA-256 of every asset, and its Ed25519 signature |
| `LICENSE.txt` | The Apache License 2.0 (a copy of the repository's `LICENSE`) |

`<target>` is `x86_64-unknown-linux-musl`, `aarch64-unknown-linux-musl` or
`x86_64-pc-windows-msvc`.

## Outbound connections from the server

| Destination | Used for |
| --- | --- |
| `github.com`, `api.github.com` | Release checks and updates. Optional; see [Update Forge](/docs/guides/update-forge/) |
| `endoflife.date` | Release lines (LTS, latest, end of life) on the Updates page, refreshed daily. Optional |
| Your OIDC issuer | Single sign-on, if configured |
