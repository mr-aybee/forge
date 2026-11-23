# Local release feed (Hyper-V test lab)

Publishes local builds of Forge as GitHub-style releases from your Windows
machine, so lab VMs install and **self-update** Forge the same way customers
will from GitHub, with no copying to VMs by hand.

```
 Windows (dev machine)                          lab-a (Forge server VM)
 publish.py → releases/<tag>/                  install.sh from the feed, then
 feed_server.py on 10.50.0.1:8000  ◄──────── [update] feed_url → "Update to …"
```

| File | What it does |
| --- | --- |
| `feed_server.py` | Serves `releases/` with GitHub's release API and download URLs. Accepts any personal access token (or none). Python standard library only, about 20 MB of RAM. Reads disk per request, so it never needs a restart. |
| `publish.py` | Picks a newer version (`<Cargo version>-dev.<yyyyMMdd.HHmmss>`), builds natively on Linux or in WSL on Windows, packages, signs and publishes. |
| `build_release.sh` | The build half of `publish.py`: same asset names, tarball, `SHA256SUMS` and `.sig` as `.github/workflows/release.yml`. |
| `releases/` | Published releases. **Git-ignored.** |
| `.keys/` | Local dev signing key (`release.key`, `release.pub`). **Git-ignored. Never use it for real releases.** |

## VM lab automation

`vm_lab.py` rebuilds the three lab VMs end to end: checkpoint restore, Docker
on lab-c, Forge install on lab-a from this feed, and node enrollment. See
`docs/src/content/docs/development/hyperv-lab.md`.

```powershell
py scripts\setup_testing\vm_lab.py init       # once: .lab/vm-lab.env (git-ignored), add the VM password
py scripts\setup_testing\vm_lab.py status
py scripts\setup_testing\vm_lab.py rebuild --publish
```

## One-time setup

1. WSL build tooling, if not done yet:
   `wsl -d Ubuntu-24.04 -u root -- bash scripts/lab/setup-wsl.sh <wsl-user>`
2. The ForgeLab switch with the host on `10.50.0.1`. See the Hyper-V lab notes.
3. Allow the VMs to reach the feed (admin PowerShell):
   ```powershell
   New-NetFirewallRule -DisplayName "Forge local feed" -Direction Inbound -Protocol TCP -LocalPort 8000 -InterfaceAlias "vEthernet (ForgeLab)" -Action Allow
   ```

## Publish and serve

```powershell
python scripts/setup_testing/publish.py                      # build + publish a new release
python scripts/setup_testing/publish.py --ui --notes "…"       # rebuild the console too
python scripts/setup_testing/publish.py --prune 5             # keep the newest 5
python scripts/setup_testing/publish.py --full                # real release optimisation (slow); default is a quick lab build
py scripts\setup_testing\feed_server.py                  # serve on 10.50.0.1:8000
```

The first publish creates the dev signing key and prints its **public key**.

`build_release.sh` builds, packages and signs in a WSL-local temp directory,
then copies the finished release onto `releases/` once (via
`releases/.staging-<tag>`, renamed into place). The repository on `/mnt/d` is
a slow 9P mount, so only that last copy touches it.

### Faster linking (local WSL builds)

`build_release.sh` and `scripts/lab/build.sh` link with LLD when it is
installed (`setup-wsl.sh` installs it): the same program, a much faster link
step. They set `CARGO_TARGET_X86_64_UNKNOWN_LINUX_MUSL_RUSTFLAGS="-C link-arg=-fuse-ld=lld"`
unless you set it yourself. CI and the GitHub release workflow keep the default
linker. On an older WSL setup, install it once:

```powershell
wsl -d Ubuntu-24.04 -u root -- apt-get install -y lld
```

### Running `build_release.sh` by hand

From PowerShell, pass the repository as a Windows path:

```powershell
wsl -d Ubuntu-24.04 --cd <repo-root> -- env FORGE_QUICK=1 bash scripts/setup_testing/build_release.sh <version>
```

From Git Bash, set `MSYS_NO_PATHCONV=1` first. Without it, MSYS rewrites
`--cd` and any `CARGO_TARGET_DIR=/home/...` value into Windows paths and the
build fails or writes to the wrong place:

```bash
MSYS_NO_PATHCONV=1 wsl -d Ubuntu-24.04 --cd '<repo-root>' -- env FORGE_QUICK=1 bash scripts/setup_testing/build_release.sh <version>
```

`--cd /mnt/d/...` fails. Pass the Windows path, `<repo-root>`, instead.

## First install on the Forge server VM

```bash
FEED=http://10.50.0.1:8000
TAG=$(curl -fsS -H "Authorization: token anything" $FEED/repos/mr-aybee/forge/releases/latest | jq -r .tag_name)
curl -fLO -H "Authorization: token anything" "$FEED/mr-aybee/forge/releases/download/$TAG/forge-${TAG#v}-x86_64-linux.tar.gz"
tar xzf forge-*-x86_64-linux.tar.gz && cd forge-*-x86_64-linux
sudo ./install.sh --public-url http://10.50.0.10:8080
```

Then point the updater at the feed and trust the dev key, in
`/etc/forge/forge.toml`:

```toml
[update]
enabled = true
feed_url = "http://10.50.0.1:8000/repos/mr-aybee/forge/releases/latest"
public_key = "<contents of scripts/setup_testing/.keys/release.pub>"
```

`sudo systemctl restart forge-server`.

## Update loop

1. Change code, then run `publish.py`.
2. In the console, check for updates (or wait for the 6-hourly check), then
   **Update now** (MFA). Forge downloads from the feed, verifies the dev
   signature, dry-runs, backs up the database, swaps, restarts, and rolls back
   after three failed starts.

New nodes get the agent from the updated release.

## Notes

- Versions come from `FORGE_BUILD_VERSION`, compiled into `forge-server` and
  `forge-agent`. Only those crates rebuild per publish; `Cargo.toml` stays
  untouched.
- `--require-token` makes the feed answer 404 without an `Authorization`
  header, like a private GitHub repository. Forge's updater sends no token,
  so keep it off when testing self-update.
- `--bind 0.0.0.0` serves on every interface. Keep the default for the lab.
