---
title: Hyper-V test lab (VMs)
description: "Three Debian VMs on Hyper-V for end-to-end tests: a Forge server, two nodes and Docker, rebuilt from the local release feed with one command."
sidebar:
  order: 4
---

The default test lab is three small **Debian 13 VMs** on the developer's
Windows machine (Hyper-V). Forge is installed on them the way customers
install it: from a release package, here from the
[local release feed](/docs/development/releases/#testing-releases-locally).
No containers run on the Windows machine. Docker runs **inside lab-c** for
container and App Store tests.

| VM (Hyper-V name) | Address | Role |
| --- | --- | --- |
| `lab-a` (LAB-A) | 192.0.2.10 | Forge server, console at `http://192.0.2.10:3880` |
| `lab-b` (LAB-B) | 192.0.2.11 | Node |
| `lab-c` (LAB-C) | 192.0.2.12 | Node and **Docker Engine** |
| Windows host | 192.0.2.1 | Gateway (NAT), release feed on `:8000` |

All VMs: Debian 13, 1–2 vCPU, dynamic memory, SSH, a sudo-capable user. The
switch, NAT and VM setup are in `scripts/setup_testing/MOP.md`.

## Credentials

Nothing secret is committed. Two git-ignored files in
`scripts/setup_testing/.lab/` hold the logins:

| File | Holds | Written by |
| --- | --- | --- |
| `vm-lab.env` | VM SSH user and password (also used for sudo), addresses, Hyper-V VM names, checkpoint name, feed URL | `vm_lab.py init` (template), then the developer |
| `forge-admin.env` | The lab's **fixed** Forge console login (`FORGE_ADMIN_USER`, `FORGE_ADMIN_PASSWORD`) | `vm_lab.py install-server` on the first install, then kept |

**The console login stays the same across rebuilds.** When
`forge-admin.env` exists, `install-server` passes its password to the
installer over stdin (`FORGE_ADMIN_PASSWORD`; never on a command line), so a
fresh install creates `admin` with that same password and nothing is
generated. Only when the file is missing does the installer generate a
password, which is then saved there. Never delete or overwrite
`forge-admin.env` during a rebuild. If someone changes the password in the
console, update the file to match.

Ask the developer for the VM password if `vm-lab.env` is missing. Scripts
read these files; never print their values, put them in commits, docs, issue
comments or chat.

## One command: rebuild everything

From the repository root on Windows, with the feed server running
(`py scripts\setup_testing\feed_server.py`):

```powershell
py scripts\setup_testing\vm_lab.py rebuild             # newest release on the feed
py scripts\setup_testing\vm_lab.py rebuild --publish   # build + publish this tree first
py scripts\setup_testing\vm_lab.py rebuild --publish --ui
```

`--publish` builds the release in WSL with
`scripts/setup_testing/build_release.sh`. It packages and signs in a
WSL-local temp directory and copies the result onto the repository once.
It links with LLD when installed (`scripts/lab/setup-wsl.sh` installs it),
which makes the link step much faster; CI and the GitHub release workflow keep
the default linker.
To run it by hand from Git Bash, set `MSYS_NO_PATHCONV=1` and pass the
repository to `wsl --cd` as a Windows path; `--cd /mnt/d/...` fails:

```bash
MSYS_NO_PATHCONV=1 wsl -d Ubuntu-24.04 --cd '<repo-root>' -- env FORGE_QUICK=1 bash scripts/setup_testing/build_release.sh <version>
```

`rebuild` runs, in order:

1. **reset**: restores the Hyper-V checkpoint (`FORGE_LAB_CHECKPOINT`,
   default `clean`) on all three VMs and waits for SSH. Needs Hyper-V rights:
   an admin shell, or membership of the *Hyper-V Administrators* group.
   `--no-reset` skips it (upgrade in place).
2. **prepare**: checks SSH and sudo everywhere, that lab-a reaches the feed,
   and installs Docker Engine on lab-c if missing.
3. **install-server**: downloads the release package on lab-a, runs
   `install.sh`, points `[update]` at the local feed with the dev key, checks
   `/api/version` matches the release, and saves the generated admin login to
   `.lab/forge-admin.env`.
4. **add-nodes**: signs in to the API, creates a one-time enrollment token per
   node and runs the node installer (`/install.sh`) over SSH, then waits until
   each node is online.
5. **status**.

## Individual commands

| Goal | Command |
| --- | --- |
| VMs, memory, disk, services, Forge version, nodes | `vm_lab.py status` |
| Restore the checkpoint | `vm_lab.py reset` |
| SSH/sudo check, Docker on lab-c | `vm_lab.py prepare` |
| Install or upgrade Forge on lab-a | `vm_lab.py install-server [--tag vX.Y.Z-dev…]` |
| Enroll lab-b and lab-c | `vm_lab.py add-nodes` |
| Run a command | `vm_lab.py run lab-b -- uname -a` |
| Run as root | `vm_lab.py sudo lab-c -- docker ps` |

To test a **self-update** instead of a reinstall: run `publish.py`, then in
the console check for updates and click **Update now**.

## How the scripts reach the VMs

- **SSH** uses Windows OpenSSH with `SSH_ASKPASS`, so no key setup is needed
  and the password never appears on a command line. Host keys go to
  `.lab/known_hosts`, so checkpoint restores do not touch your own
  `~/.ssh/known_hosts`.
- **sudo** reads the password from stdin (`sudo -S`).
- The **console API** is used with the admin login from `forge-admin.env`.
  The script does not change the password. If you change it in the console,
  update that file.

## Notes

- **Memory:** lab-c runs Docker workloads, so give it a higher dynamic
  memory maximum (2 GB or more) than the 1 GB the base layout uses.
- **Checkpoints:** after a successful `prepare`, a checkpoint such as
  `lab-base` (with Docker already on lab-c) makes later rebuilds faster. Set
  `FORGE_LAB_CHECKPOINT=lab-base` to use it.
- **Lab-only settings stay in the lab:** the dev release key and the plain
  HTTP feed are for these VMs only.
- The container lab in [Linux test lab (WSL)](/docs/development/linux-lab/) is
  not in use for now.
