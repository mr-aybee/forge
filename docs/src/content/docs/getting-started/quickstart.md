---
title: Quickstart
description: Install Forge on one Linux server, sign in and see your first node in about ten minutes.
sidebar:
  order: 2
---

This walkthrough installs Forge on one Linux server and manages that same
server as your first node. At the end you have the console open, a node
online and a command run on it. For Windows, unattended installs and
production layouts, see [Installation](/docs/getting-started/installation/).

## Before you begin

- A Linux server with systemd, x86_64 or arm64: Debian, Ubuntu, RHEL, Rocky
  or Alma. A small virtual machine is enough.
- A user that can run `sudo`.
- `curl`, `tar` and `gzip`. Minimal RHEL, Rocky and Alma images can lack
  `tar` and `gzip`: install them with `sudo dnf install -y tar gzip`.
- From your computer you can reach the server on TCP port 3880.

## 1. Download Forge

On the server, download the bootstrap script of the latest release:

```sh
curl -fsSLO https://github.com/orbyntech/forge/releases/latest/download/get-forge.sh
```

For a release candidate, use its tag instead of `latest`:
`https://github.com/orbyntech/forge/releases/download/<tag>/get-forge.sh`.

## 2. Run the installer

```sh
sudo sh get-forge.sh                    # or: sudo sh get-forge.sh --version <tag>
```

The script downloads the package for this machine, checks its signature and
checksum, and starts the installer. The installer asks a few questions. Press **Enter** to take the default in
brackets. For this walkthrough, take the defaults except where the table
says otherwise:

| Question | Answer for this walkthrough |
| --- | --- |
| Environment | **Enter** (`development`). |
| Operational database | **Enter** (`sqlite`, built in). |
| Analytics database for node history | **Enter** (`duckdb`). |
| Administrator user name | **Enter** (`admin`). |
| Administrator password | **Enter** to generate one, or type at least 12 characters. |
| Also run workloads on this server? | **`y`**, so this server becomes your first node. |
| Cluster for this server | **Enter** (`default`). |
| Terminal account on this server | **Enter** (`forge-ops`). |

The installer then shows a **REVIEW** of your answers. Answer **Enter**
(`y`) to **Install with these settings?**.

## 3. Read the summary

The installer reports each step as it goes, then prints a summary like this:

```text
  Orbyn Forge <version> is installed and running.

  ACCESS
    Console       http://192.0.2.10:3880
    Console       http://10.0.0.5:3880
    Console       http://<hostname>:3880
    · Any name or IP of this machine works; pin one later in Settings or with --public-url.
    Agents        nodes connect to TCP 3843 and 3832 on the address they use
    Analytics     duckdb, uploads on port 3832

  ADMINISTRATOR
    Username      admin
    Password      <generated password>
    Saved to      /var/lib/forge/initial-admin-password (root only, deleted when you change it)

  RESTRICT ACCESS
    · Every interface listens. Narrow who may connect, for example:
      ufw allow from <node subnet> to any port 3843 proto tcp
      [security.access] allow = ["10.0.0.0/8"] in /etc/forge/forge.toml

  AGENT
    This server   enrolled in cluster default as <hostname> (terminal account forge-ops)
```

The installer asks no address. The console answers on every **Console**
line, and on any other name or IP of the server.

Copy the generated password now: the summary shows it only once. If it
scrolled away, read it again with:

```sh
sudo cat /var/lib/forge/initial-admin-password
```

## 4. Sign in

1. Open a **Console** address from the summary that your computer can reach.
2. Sign in as `admin` with the password.
3. Open **Settings → Password & security** and choose your own password. Forge then
   deletes the saved password file.
4. Optional: in **Settings → General**, add your email so you can sign in
   with it.

:::tip
Set up MFA now in **Settings → Password & security**. See [Enable MFA](/docs/guides/mfa/).
:::

## 5. Run a command on your first node

1. Open **Overview**. Your server is listed as a node with status **online**.
2. Open the node's row menu and select **Ping**.
3. The jobs drawer opens and shows the job reach **succeeded**.

Forge reached the node through the agent, which keeps an outbound,
mutually authenticated connection to the control plane. Your node needs no
inbound port.

## Troubleshooting

| Symptom | Cause | Fix |
| --- | --- | --- |
| The console does not open | Port 3880 is blocked between you and the server | Open TCP 3880 in your cloud firewall or security group. The installer only opens the host firewall. |
| The node shows **offline** | The agent cannot reach TCP 3843 | Run `systemctl status forge-agent` on the node. Check that port 3843 is open. |
| The summary says the agent was not enrolled | The node installer failed | The summary prints the commands to retry by hand. The server install is fine. |
| `install.sh` did not ask any questions | You passed options, `--yes`, or ran it without a terminal | Run `sudo sh get-forge.sh` with no options in an interactive shell. |

## Next steps

- [Add nodes](/docs/guides/add-nodes/): manage more Linux and Windows machines.
- [Navigate the console](/docs/guides/navigate-the-console/).
- [Install an app from the App Store](/docs/guides/app-store/).
- [Roles and access](/docs/guides/roles-and-access/): invite your team.
- [Installation](/docs/getting-started/installation/): Windows, production and updates.
