---
title: Update Forge
description: Update the Forge control plane to a newer release from the console, the forge CLI or the installer, and what happens when an update fails.
sidebar:
  order: 3
---

This guide updates the control plane (`forge-server`, the analytics worker,
the plugin runner and the agent binaries it hands to nodes) to a newer
release. To bring the agents on your nodes up to that version afterwards, see
[Update agents](/docs/guides/update-agents/).

## Before you begin

- You need to be a **Platform admin**. **Update now** asks for an MFA code.
- Read the [upgrade notes](/docs/reference/upgrade-notes/) for every release
  between yours and the new one. Some updates migrate data.
- The server needs HTTPS to `github.com` and `api.github.com`, or a mirror
  set in `[update] feed_url`.
- Self-update replaces files in the install folder (`/opt/forge` on Linux).
  It needs free space for the new binaries and the `*.prev` copies it keeps.

## Update from the console

1. Platform administrators see **Update to x.y.z** at the top right of the
   console when a newer release is out. Forge checks every six hours; select
   **Check again** to check now.
2. Select it to read the release notes, then select **Update now** and enter
   your MFA code.
3. The console waits while the server restarts and reloads on the new
   version.

**Administration → Forge updates** lists every release newer than the one
running, newest first, so you can also pick one that is not the latest.
Forge only upgrades; older versions are never offered. A pre-release
(`X.Y.Z-rc-NN`) is offered only to installs that run a pre-release.

## Update from the CLI

```sh
forge system releases            # the releases you can update to
forge system apply --version <version>   # update, then roll the agents
```

By default the agents follow: once the new server serves, it rolls every
outdated Linux agent to the new version in canary batches. Add
`--agents none` to skip that. See
[`forge system apply`](/docs/reference/cli/system/#forge-system-apply).

## Update with the installer

Run the installer of the newer release, as for a new install. It replaces
the binaries and keeps the configuration and data:

```sh
curl -fsSLO https://github.com/orbyntech/forge/releases/download/<tag>/get-forge.sh
sudo sh get-forge.sh --version <tag> -- --yes
```

See [Installation](/docs/getting-started/installation/) for Windows and for
packages you downloaded yourself.

## What an update does

1. Downloads this platform's files and checks each against `SHA256SUMS`,
   whose Ed25519 signature must match the release key built into Forge.
2. Runs the new version against your configuration and checks that it knows
   every database migration already applied (`forge-server check-migrations`).
3. Backs up the SQLite database to `<data_dir>/backups/forge-<old>-<time>.db`.
4. Installs the new binaries and agent binaries, keeping the current ones as
   `*.prev`. The agent folder's `MANIFEST.json` names the version nodes are
   offered.
5. Restarts in place from the installed path (for example
   `/opt/forge/forge-server`), never from a `*.prev` copy.

## Verify

- **Administration → Forge updates** shows the new version.
- The audit log has `system.update` and `system.update.completed`.
- On Linux: `/opt/forge/forge-server --version`.

## Troubleshooting

| Symptom | Cause | Fix |
| --- | --- | --- |
| The update status shows **failed**, and the server runs the old version | The new version failed to start three times, so Forge restored the `*.prev` binaries | Read the reason in the status and `journalctl -u forge-server`; fix it and update again |
| **failed**: the server restarted into the old version | The service restarted the old binary | Restart the `forge-server` service to start the installed version |
| **failed**: cannot determine where forge-server is installed | The server was started through a relative path | Restart it through the service; Forge never guesses a binary from `PATH` |
| No update is offered | The newer release is a pre-release, or `[update] enabled = false`, or the server cannot reach GitHub | Install the pre-release with the installer; check `[update]` and outbound HTTPS |

## Turn updates off, or use a mirror

```toml
[update]
enabled = true
feed_url = "https://api.github.com/repos/orbyntech/forge/releases/latest"
```

`enabled = false` stops the checks. See
[Configuration](/docs/reference/configuration/).

## Next steps

- [Update agents](/docs/guides/update-agents/)
- [Upgrade notes](/docs/reference/upgrade-notes/)
