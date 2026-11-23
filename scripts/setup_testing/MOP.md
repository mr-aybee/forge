# MOP: Forge test lab on Hyper-V (DRAFT, to refine)

Method of procedure for a three-VM Debian lab on a Windows 11 host: one Forge
server, two nodes, installing and updating Forge from the local release feed
in this folder. Steps are in the order we ran them. **Notes** record what went
wrong the first time and why.

| VM | Role | vCPU | RAM startup / min / max | Disk (dynamic VHDX) | IP |
| --- | --- | --- | --- | --- | --- |
| LAB-A (`lab-a`) | Forge server | 2 | 1024 / 512 / 2048 MB | 20 GB | 10.50.0.10 |
| LAB-B (`lab-b`) | Node | 1 | 512 / 256 / 1024 MB | 12 GB | 10.50.0.11 |
| LAB-C (`lab-c`) | Node | 1 | 512 / 256 / 1024 MB | 12 GB | 10.50.0.12 |
| Windows host | Gateway, NAT, release feed | – | – | – | 10.50.0.1 |

Host used: i7-1185G7 (4 cores / 8 threads), 32 GB RAM, Windows 11.

---

## 0. Prerequisites

- Hyper-V enabled. Windows 11 **Home** does not include Hyper-V (use Pro, or
  an unsupported workaround).
- Debian 13 netinst ISO (amd64) from
  `https://cdimage.debian.org/debian-cd/current/amd64/iso-cd/`. Verify it
  against `SHA512SUMS`.
- Shut down WSL and Docker Desktop while running the lab if RAM is tight
  (`wsl --shutdown`).

> **Note:** The firmware "virtualization enabled" flag reads *False* when a
> hypervisor (WSL2/Docker) is already running. That is expected and not an error.

## 1. Lab network (host, admin PowerShell, once)

```powershell
New-VMSwitch -Name ForgeLab -SwitchType Internal
New-NetIPAddress -IPAddress 10.50.0.1 -PrefixLength 24 -InterfaceAlias "vEthernet (ForgeLab)"
New-NetNat -Name ForgeLabNAT -InternalIPInterfaceAddressPrefix 10.50.0.0/24
Set-NetConnectionProfile -InterfaceAlias "vEthernet (ForgeLab)" -NetworkCategory Private
```

Check: `Get-NetNat` shows `ForgeLabNAT 10.50.0.0/24 Active True`.

> **Note:** The parameter is `-InternalIPInterfaceAddressPrefix` (an earlier
> draft had it wrong). `AddressState: Tentative` right after creation, and
> `Invalid` on the *PersistentStore* copy, are both normal.
>
> **Note:** Do not use the Hyper-V *Default Switch* for the lab. Its DHCP
> addresses (172.x) change after host reboots, so agents lose the server.
> It is still useful temporarily (step 4).

## 2. Create the VMs

Hyper-V Manager → New → Virtual Machine, per VM:

- **Generation 2**.
- Startup memory per the table, **Use Dynamic Memory** on.
- Network: `ForgeLab`.
- New VHDX, size per the table (dynamically expanding: it only uses what is written).
- Install from the Debian ISO.

Then, before first boot:

```powershell
Set-VMFirmware LAB-A,LAB-B,LAB-C -SecureBootTemplate MicrosoftUEFICertificateAuthority
Set-VM LAB-A -ProcessorCount 2 -AutomaticCheckpointsEnabled $false
Set-VM LAB-B,LAB-C -ProcessorCount 1 -AutomaticCheckpointsEnabled $false
Set-VMMemory LAB-A -MinimumBytes 512MB -MaximumBytes 2GB
Set-VMMemory LAB-B,LAB-C -MinimumBytes 256MB -MaximumBytes 1GB
```

> **Note:** *"The signed image's hash is not allowed (DB)"* at boot means the
> Secure Boot template is still "Microsoft Windows". Set it to "Microsoft UEFI
> Certificate Authority" (above). Only if that still fails, turn Secure Boot off
> for the lab.
>
> **Note:** A **2 GB disk is too small**. Debian alone needs about 1.5–2 GB.
> Expand while the VM is off: `Get-VMHardDiskDrive LAB-B | Resize-VHD -SizeBytes 12GB`.
>
> **Note:** The Debian installer enters **low memory mode below about 780 MB**
> and asks which installer components to load. Install with **1024 MB
> startup**, then set 512 MB again afterwards. If you are already in the
> component list, select nothing and continue.

## 3. Install Debian

Choose **Install** (text mode). Answers:

| Screen | Answer |
| --- | --- |
| Network | DHCP fails on ForgeLab (no DHCP server): configure manually, `10.50.0.x/24`, gateway `10.50.0.1`, DNS `1.1.1.1 8.8.8.8` |
| Hostname | `lab-a` / `lab-b` / `lab-c`, domain empty |
| Root password | **Leave empty** (your user gets sudo) |
| Partitioning | Guided, entire disk, one partition (or manual: 512 MB EFI + ext4 `/`, no swap) |
| Scan extra media | No |
| Mirror | your country → `deb.debian.org`, proxy empty |
| Software | **only** SSH server + standard system utilities |

Eject the ISO before rebooting (Media → DVD Drive → Eject).

> **Note:** If you **set a root password**, Debian does not install `sudo`
> and your user is not an admin. Use `su -` (with the dash, otherwise
> `/usr/sbin` is not on PATH and `reboot` is "command not found"), then step 5.

## 4. Network inside the VMs (static IP)

`/etc/network/interfaces` (lab-a shown, use .11 / .12 on the nodes):

```
source /etc/network/interfaces.d/*
auto lo
iface lo inet loopback

allow-hotplug eth0
iface eth0 inet static
    address 10.50.0.10/24
    gateway 10.50.0.1
    dns-nameservers 1.1.1.1 8.8.8.8
```

DNS (as root). Debian 13's `dhcpcd` overwrites `resolv.conf`, and
`dns-nameservers` only works with the `resolvconf` package:

```bash
grep -q '^nohook resolv.conf' /etc/dhcpcd.conf 2>/dev/null || echo 'nohook resolv.conf' >> /etc/dhcpcd.conf
systemctl disable --now dhcpcd 2>/dev/null
printf 'nameserver 1.1.1.1\nnameserver 8.8.8.8\n' > /etc/resolv.conf
```

Check: `ip -br a` shows `10.50.0.x/24`, and `ping -c2 deb.debian.org` gets replies.

> **Note:** The Hyper-V console cannot paste with Ctrl+V. Use **Clipboard →
> Type clipboard text** (one command at a time, US keyboard layout), or get
> SSH working first: attach the VM to *Default Switch* temporarily, SSH to
> its 172.x address, write the config, `poweroff`, attach to `ForgeLab`, start.
> A quick alternative is a temporary address typed in the console
> (`ip addr add 10.50.0.11/24 dev eth0`), then paste over SSH.
>
> **Note:** When pasting a block that starts with `su -`, the password prompt
> swallows the rest of the block. Run `su -` alone, then paste the rest.
>
> **Note:** Only an `fe80::` address on eth0 means no IPv4: DHCP on a switch
> without DHCP, or the static config is not applied yet.
>
> **Note:** `resolv.conf` with only `# Generated by dhcpcd` comments means no
> DNS: `ping 1.1.1.1` works but `apt` fails with *Temporary failure
> resolving*. Fix with the DNS block above.
>
> **Note:** `ping 10.50.0.1` from a VM fails because Windows Firewall drops
> ICMP to the host. That is harmless as long as internet traffic passes the NAT.
>
> **Note:** After moving a VM to ForgeLab it took a short while before the
> host could reach it. Re-check before debugging further.

## 5. Base packages (each VM, as root)

```bash
apt update && apt full-upgrade -y
apt install -y sudo htop curl wget ca-certificates gnupg nano less git unzip \
  bash-completion tmux rsync jq lsof dnsutils net-tools iputils-ping hyperv-daemons
usermod -aG sudo <lab-user>
systemctl enable --now fstrim.timer
sed -i 's/^#\?SystemMaxUse=.*/SystemMaxUse=100M/' /etc/systemd/journald.conf
systemctl restart systemd-journald
apt autoremove -y && apt clean
```

Log out and back in, then check `sudo whoami` prints `root`.

`fstrim.timer` lets the dynamic VHDX shrink when files are deleted (Gen 2 SCSI
passes TRIM through). `hyperv-daemons` shows the VM's IP in Hyper-V Manager
and allows clean shutdown from the host.

## 6. SSH from Windows

```powershell
ssh-keygen -t ed25519
foreach ($ip in "10.50.0.10","10.50.0.11","10.50.0.12") {
  Get-Content $env:USERPROFILE\.ssh\id_ed25519.pub | ssh <lab-user>@$ip "mkdir -p ~/.ssh && chmod 700 ~/.ssh && cat >> ~/.ssh/authorized_keys && chmod 600 ~/.ssh/authorized_keys"
}
```

Optionally add short host names in `%USERPROFILE%\.ssh\config` (`forge-cp`, `forge-node1`, `forge-node2`).

## 7. Checkpoint "clean"

```powershell
Stop-VM LAB-A,LAB-B,LAB-C
Set-VMMemory LAB-B,LAB-C -StartupBytes 512MB
Checkpoint-VM LAB-A,LAB-B,LAB-C -SnapshotName "clean"
Start-VM LAB-A,LAB-B,LAB-C
```

## 8. Local release feed (Windows)

See `README.md` in this folder. Once:

```powershell
# WSL build tools (the first build is about 15 min, later ones are incremental)
wsl -d Ubuntu-24.04 -u root -- bash scripts/lab/setup-wsl.sh <wsl-user>
New-NetFirewallRule -DisplayName "Forge local feed" -Direction Inbound -Protocol TCP -LocalPort 8000 -InterfaceAlias "vEthernet (ForgeLab)" -Action Allow
```

Every time:

```powershell
python scripts/setup_testing/publish.py            # --ui if the console changed
py scripts\setup_testing\feed_server.py        # leave running
```

> **Note:** A fresh Ubuntu WSL had no Rust, so the first publish failed with
> `~/.cargo/env: No such file or directory`. `build_release.sh` now says so
> and points to `setup-wsl.sh`. Only build tools and Rust are needed (Docker
> is not).

## 9. Install Forge on lab-a from the feed

```bash
FEED=http://10.50.0.1:8000
TAG=$(curl -fsS -H "Authorization: token anything" $FEED/repos/mr-aybee/forge/releases/latest | jq -r .tag_name)
curl -fLO -H "Authorization: token anything" "$FEED/mr-aybee/forge/releases/download/$TAG/forge-${TAG#v}-x86_64-linux.tar.gz"
tar xzf "forge-${TAG#v}-x86_64-linux.tar.gz" && cd "forge-${TAG#v}-x86_64-linux"
sudo ./install.sh --public-url http://10.50.0.10:8080
```

The installer creates the administrator **`admin`** (no email needed) with a
**generated password**. It prints the password once in the summary and saves
it to `/var/lib/forge/initial-admin-password` (root only). Sign in as
`admin`, then in **Settings** add your email, change the password (the saved
file is deleted at that moment) and set up MFA. If the summary scrolled away:
`sudo cat /var/lib/forge/initial-admin-password`.

Then point the updater at the feed in `/etc/forge/forge.toml` and restart:

```toml
[update]
enabled = true
feed_url = "http://10.50.0.1:8000/repos/mr-aybee/forge/releases/latest"
public_key = "<scripts/setup_testing/.keys/release.pub>"
```

```bash
sudo systemctl restart forge-server
```

Open `http://10.50.0.10:8080` and sign in. Take checkpoint **"forge-installed"**.

> **Note (installer bug, fixed: `init` now validates before creating the database, and the installer generates the password):** With a
> password under 12 characters, the first run failed *after* `init` had
> created `forge.db`. The re-run then saw the database, treated the install
> as an upgrade, **skipped creating the administrator**, and started a server
> nobody could sign in to. Fixed: the installer re-prompts until the password
> is 12+ characters (and asks for it twice), and removes a database it just
> created if `init` fails.
>
> **Recovery on a server installed before the fix** (no users exist, so
> `init` is allowed):
> ```bash
> sudo systemctl stop forge-server
> read -rs -p "Admin password (12+): " P; echo
> sudo FORGE_ADMIN_PASSWORD="$P" runuser -u forge -- /opt/forge/forge-server --config /etc/forge/forge.toml init --username admin
> unset P
> sudo systemctl start forge-server
> ```
>
> **Note:** A typo in the feed port (`:80000`) breaks the download. Keep `FEED` on `:8000`.
>
> **Note:** Running the commands again from *inside* the extracted folder
> extracts a nested copy (`forge-…/forge-…/`). It is harmless. Run from `~`.

## 10. Add the nodes

Console → **Nodes → Add node**, for `10.50.0.11` and `10.50.0.12`:
*Assisted* with `<lab-user>` and SSH credentials (sudo-capable), or *Manual*
with the one-line installer run on the node. Agents connect to lab-a on TCP 7443.

## 11. Update loop

1. Change code, then run `publish.py`.
2. Console → check for updates → **Update now** (MFA).
3. Confirm the footer shows the new version (hover it for the commit).

---

## Open items / to refine

- [ ] Verify step 10 (assisted add) end to end and record the screens.
- [ ] Verify a full self-update from the feed (download → verify → swap → restart).
- [ ] Decide whether the lab wants a shorter update check than 6 h, or a "Check now" button in the console.
- [ ] Agents on existing nodes are not updated by a server update. Document the reinstall path, or plan agent self-update.
- [ ] Optional: script steps 1–2 (switch, NAT, VM creation) as `new-lab.ps1`.
- [ ] Pre-existing clippy `result_large_err` in `agent_hub/handlers.rs` (newer toolchain).
