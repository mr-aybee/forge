# MOP: fresh Forge lab from a clean checkpoint

Method of procedure to bring the three-VM lab from its `clean` Hyper-V
checkpoint to a running Forge server on lab-a, served from your own local
release feed, so you can onboard the nodes yourself through the console.

For building the VMs themselves, see [MOP.md](MOP.md). For the automation
behind these steps, see [README.md](README.md) and `vm_lab.py`.

| Host              | Role                             | Address                                  |
| ----------------- | -------------------------------- | ---------------------------------------- |
| Windows (this PC) | Builds releases, serves the feed | `10.50.0.1`                         |
| lab-a             | Forge server                     | `10.50.0.10` (console on `:3880`) |
| lab-b             | Node                             | `10.50.0.11`                        |
| lab-c             | Node, with Docker                | `10.50.0.12`                        |

Secrets stay in the git-ignored `scripts/setup_testing/.lab/`:
`vm-lab.env` (VM user and password) and `forge-admin.env` (the fixed Forge
admin login). Never paste them into chats, issues or commits.

Time: about 40–60 minutes the first time after a WSL rebuild (the first build
starts from an empty cache), about 15 minutes after that.

---

## 0. Before you begin

1. Open **PowerShell** (not Git Bash) in the repository:

   ```powershell
   cd <repo-root>
   git pull
   ```
2. Check the lab is at the clean checkpoint and running:

   ```powershell
   py scripts\setup_testing\vm_lab.py status
   ```

   **Expect:** lab-a, lab-b and lab-c `Running`, SSH reachable.
   To go back to clean first, use an **Administrator** PowerShell:
   ```powershell
   py scripts\setup_testing\vm_lab.py reset
   ```
3. Once per PC (already done on this one), allow the VMs to reach the feed.
   **Administrator** PowerShell:

   ```powershell
   New-NetFirewallRule -DisplayName "Forge local feed" -Direction Inbound -Protocol TCP -LocalPort 8000 -InterfaceAlias "vEthernet (ForgeLab)" -Action Allow
   ```
4. Once per WSL install (already done after the rebuild), the build tools:

   ```powershell
   wsl -d Ubuntu-24.04 -u root --cd <repo-root> -- bash scripts/lab/setup-wsl.sh <wsl-user>
   ```

> **Note:** `py` may print "Could not find platform independent libraries".
> That warning is harmless here.

## 1. Build and publish a release

```powershell
python scripts/setup_testing/publish.py --ui --notes "Fresh lab from develop"
```

- `-Ui` rebuilds the web console (`src/console`) so the release has the current UI.
- The build runs in WSL as a quick lab build, linked with LLD for speed
  (installed by `setup-wsl.sh`). Add `-Full` for a fully optimised build (much
  slower; not needed for testing).
- The first publish ever creates the dev signing key in `.keys/` and prints
  its public key. Later publishes reuse it.

**Expect:** it ends with `==> Published v<version>` and lists the files in
`scripts\setup_testing\releases\v<version>\`. Note the tag.

## 2. Start the release feed

Open a **second PowerShell window** and leave it running for the whole session:

```powershell
cd <repo-root>
py scripts\setup_testing\feed_server.py
```

**Expect:** it says it serves on `10.50.0.1:8000`. It reads from disk per
request, so new publishes need no restart.

## 3. Prepare the VMs

Back in the first window:

```powershell
py scripts\setup_testing\vm_lab.py prepare
```

**Expect:**

- `SSH and sudo work` for all three VMs.
- `lab-a: release feed http://10.50.0.1:8000 → 200`.
- `lab-c: installed Docker Engine` (or `already running`).

It does not install Forge on any node.

## 4. Install the Forge server on lab-a

```powershell
py scripts\setup_testing\vm_lab.py install-server
```

(Add `--tag v<version>` to install a specific release; the default is the
newest on the feed.)

What it does:

- downloads the release on lab-a from your feed and runs `install.sh` with
  `--public-url http://10.50.0.10:3880` and MFA off (lab setting);
- reuses the fixed admin login from `.lab/forge-admin.env` (if the file is
  missing, the installer generates one and the script saves it there);
- points the server's updater at your feed and trusts the dev key, so
  **Update now** works from the console later.

**Expect:** `server <version> answering at http://10.50.0.10:3880`.

### 4b. Same install by hand (what users run)

This is what `install-server` does, with plain shell commands. Use it to test
the install the way a user would. A real install downloads from GitHub; in
the home lab the feed on `10.50.0.1:8000` stands in for GitHub.

1. From PowerShell, open a shell on the server VM:
   ```powershell
   ssh <vm-user>@10.50.0.10
   ```
2. On lab-a, download and unpack the newest release from the lab feed:
   ```bash
   FEED=http://10.50.0.1:8000
   TAG=$(curl -fsS "$FEED/repos/orbyntech/forge/releases/latest" | sed -n 's/.*"tag_name": *"\([^"]*\)".*/\1/p')
   VER=${TAG#v}
   cd /tmp && rm -rf forge-pkg && mkdir forge-pkg && cd forge-pkg
   curl -fLO "$FEED/orbyntech/forge/releases/download/$TAG/forge-$VER-x86_64-linux.tar.gz"
   tar xzf "forge-$VER-x86_64-linux.tar.gz" && cd "forge-$VER-x86_64-linux"
   ```
   (A real user replaces the feed with
   `https://github.com/orbyntech/forge/releases/latest/download/forge-<version>-x86_64-linux.tar.gz`.)
3. Install the server. Run the installer without options; on the terminal it
   asks the questions and shows a review screen:
   ```bash
   sudo ./install.sh
   ```
   Lab answers: public URL `http://10.50.0.10:3880`, environment
   `development`, admin user `admin`, the fixed admin password from
   `.lab/forge-admin.env` (typed hidden, twice), MFA `n`, elevated API tokens
   `y`, install the agent here `y` (cluster `default`, terminal account
   `forge-ops`), then `Y` on **Install with these settings?**. lab-a then
   enrolls itself as a node; the summary's **AGENT** section says
   `enrolled in cluster default as lab-a`.

   Alternative without questions (scripts, or to repeat a run exactly). Any
   setting option, or `--yes`, turns the questions off:
   ```bash
   printf 'Admin password: '; stty -echo; read -r FORGE_ADMIN_PASSWORD; stty echo; echo
   export FORGE_ADMIN_PASSWORD
   sudo --preserve-env=FORGE_ADMIN_PASSWORD ./install.sh --yes \
     --public-url http://10.50.0.10:3880 \
     --admin-username admin \
     --environment development --mfa off --allow-elevated-tokens yes \
     --install-agent --cluster default --terminal-account forge-ops
   unset FORGE_ADMIN_PASSWORD
   ```
   Leave out `--install-agent …` to keep workloads off the server. Leave out
   the password lines (or press Enter at the password question) to let the
   installer generate one; it prints it once and saves it to
   `/var/lib/forge/initial-admin-password` (root only). A production install
   uses `--mfa required` (the default), `--allow-elevated-tokens no` and an
   `https://` public URL. If the agent step fails, the server install still
   succeeds and the summary prints the commands to retry it by hand.
4. Lab only: point the updater at the feed and trust the dev signing key, so
   **Update now** works. Copy the one line in
   `scripts\setup_testing\.keys\release.pub` from Windows, then on lab-a:
   ```bash
   KEY='<contents of release.pub>'
   sudo sed -i '/^\[update\]/,$d' /etc/forge/forge.toml
   printf '[update]\nenabled = true\nfeed_url = "%s"\npublic_key = "%s"\n' \
     "http://10.50.0.1:8000/repos/orbyntech/forge/releases/latest" "$KEY" \
     | sudo tee -a /etc/forge/forge.toml >/dev/null
   sudo systemctl restart forge-server
   ```
5. Check it:
   ```bash
   systemctl is-active forge-server
   curl -fsS http://127.0.0.1:3880/api/version
   ```
   **Expect:** `active`, and the version you downloaded.

## 5. Verify the server

1. Open **http://10.50.0.10:3880** in your browser and sign in with the
   login from `.lab/forge-admin.env`.
2. Check the file permissions on lab-a are owner-only:
   ```powershell
   py scripts\setup_testing\vm_lab.py sudo lab-a "stat -c '%a %n' /var/lib/forge/forge.db* /var/lib/forge/pki /var/lib/forge/backups /var/lib/forge/updates"
   ```

   **Expect:** `600` for the `forge.db` files, `700` for the folders.

## 6. Onboard the nodes yourself (console)

1. **Infrastructure → Clusters → New cluster**, name `lab`.
2. **Infrastructure → Nodes → Add node**. Choose cluster `lab`, Linux. Copy
   the enrollment command it shows (it contains a short-lived token — don't
   share it).
3. Run it on the node. From PowerShell:
   ```powershell
   ssh <vm-user>@10.50.0.11
   ```

   (`<vm-user>` and its password are in `.lab/vm-lab.env`.) Then paste the
   enrollment command (it uses `sudo`).
4. **Expect:** within about 30 seconds lab-b shows **online** under **Nodes**.
5. Repeat steps 2–4 for lab-c (`10.50.0.12`), and optionally for lab-a
   (`10.50.0.10`) if you want the server host to be a node too (for example
   as the ingress edge).
6. Open **Infrastructure → Clusters → lab → Network** and apply the cluster
   network if the console asks.

### 6b. Install the agent by hand (what users run)

The **Add node** page shows this command with your server's address filled
in. On the node, it looks like this (lab server `10.50.0.10:3880`, default
terminal account `forge-ops`):

```bash
printf 'Enrollment token: '; stty -echo; read -r FORGE_ENROLL_TOKEN; stty echo; echo
export FORGE_ENROLL_TOKEN
curl -fsSL 'http://10.50.0.10:3880/install.sh?terminal_account=forge-ops' \
  | sudo --preserve-env=FORGE_ENROLL_TOKEN sh
unset FORGE_ENROLL_TOKEN
```

- Paste the token from the **Add node** page at the prompt. It is single-use
  and expires; it never appears on the command line or in the shell history.
- The installer creates the unprivileged `forge-agent` user, the signed root
  helper and the `forge-ops` terminal account, then enrolls the node.
- As `root`, leave out `sudo --preserve-env=FORGE_ENROLL_TOKEN`.
- A token can also come from the CLI instead of the page:
  `forge nodes enroll-token --cluster lab --hostname lab-b`.

Check on the node:

```bash
systemctl is-active forge-agent
sudo journalctl -u forge-agent -n 20 --no-pager
```

**Expect:** `active`, and the node **online** under **Nodes** within about 30
seconds.

> **Tip:** Instead of steps 2–5, `py scripts\setup_testing\vm_lab.py add-nodes`
> enrolls lab-b and lab-c automatically. Use it when you don't need to test
> onboarding by hand.

## 7. Test and leave comments

Your test list is `scripts\setup_testing\.lab\LAB-TESTS.md` (git-ignored; it
holds the admin login).

1. Work through each **To-do visit** in order.
2. Tick `[x]` when it passes.
3. Under `Comment:` write what you did, what you expected, what happened, and
   a screenshot path if you have one.
4. Copy the entries you commented on and paste them into the chat with
   lead-a. Each comment is routed to the agent that owns it.

## 8. Update loop (after code changes)

1. Pull the new code, then publish: `python scripts/setup_testing/publish.py --ui`
   (feed window still running).
2. In the console: **Settings → Updates → Check for updates → Update now**.
   The server downloads from your feed, verifies the signature, backs up the
   database, restarts into the new version, and rolls back on failure.
3. **Updates → Update agents** updates the nodes (choose a batch size of 1 to
   watch them one by one).

## Troubleshooting

| Symptom                                                        | Cause                                                                           | Fix                                                                                         |
| -------------------------------------------------------------- | ------------------------------------------------------------------------------- | ------------------------------------------------------------------------------------------- |
| `publish.py` fails with `no Rust toolchain in this WSL`   | WSL was rebuilt without the build tools                                         | Run step 0.4                                                                                |
| `publish.py` very slow on the first run                     | Empty WSL build cache after a rebuild                                           | Normal; later runs are much faster                                                          |
| `prepare` shows `release feed … → unreachable`           | Feed not running, or the firewall rule is missing                               | Start step 2; add the rule in step 0.3                                                      |
| `install-server` says the feed is not reachable from Windows | `feed_server.py` not running                                                  | Start it (step 2)                                                                           |
| `install-server` says `no dev release key`                 | Never published on this PC                                                      | Run step 1 once                                                                             |
| `reset` fails with an access error                           | Not an Administrator PowerShell                                                 | Re-run it as Administrator                                                                  |
| A node never comes online                                      | Wrong cluster, expired token, or no route to `10.50.0.10:3880` / `:3843` | Create a new enrollment command; check `curl -k https://10.50.0.10:3843` from the node |
| Console asks for MFA codes                                     | The server was installed without the lab MFA setting                            | Re-run step 4 with the script (it sets MFA off), or `sudo forge-server mfa off` on lab-a  |

## Start over

1. **Administrator** PowerShell: `py scripts\setup_testing\vm_lab.py reset`.
2. Continue from step 3 (the feed and published releases survive a reset).
