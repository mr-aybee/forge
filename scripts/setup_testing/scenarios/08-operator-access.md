# 08 · Operator access: tunnels, SFTP, terminal, logs

**Who:** day-2 operations. An operator must reach databases, files and logs
on nodes without SSH keys or open firewall ports.

| Cluster mapping | |
| --- | --- |
| Cluster | `lab` |
| lab-b | Docker host running a Postgres container |
| lab-c | native app with files |
| Environment | `s08` |

Prerequisite: [00-common-setup](00-common-setup.md). Docker on lab-b
(scenario 02 or `Install-Pkg 'docker' 'stable' @{} @($B)`).

## 1. Postgres on lab-b, loopback only ✅

```powershell
$NS = New-Ns 's08'
@'
services:
  db:
    image: postgres:16
    environment: { POSTGRES_PASSWORD: "${PGPASS}" }
    ports: ["127.0.0.1:5432:5432"]
    volumes: ["pg:/var/lib/postgresql/data"]
volumes: { pg: {} }
'@ | Set-Content pg.yml
'PGPASS=<choose-one>' | Set-Content pg.env
forge stacks create pg --environment s08 --node lab-b -f pg.yml --env-file pg.env
forge stacks deploy pg --wait --yes; Remove-Item pg.env
```

## 2. Tunnel ✅

```powershell
forge tunnel ports --node lab-b --set 5432
forge tunnel open -n lab-b -p 5432 -l 15432       # leave running
# second window:
psql "host=127.0.0.1 port=15432 user=postgres"   # or any SQL client on 127.0.0.1:15432
forge tunnel list
forge tunnel close --id <id>
```

- [ ] Connects through the tunnel; port 5432 is **not** reachable from Windows directly (`Test-NetConnection 10.50.0.11 -Port 5432` fails)
- [ ] A raw tunnel to another port (`forge tunnel open -n lab-b -p 22`) opens for an administrator (`network.tunnel.any`) and is refused for someone without that permission
- [ ] The tunnel appears in the UI under **Settings → Tunnels** and can be closed there
- [ ] ❌ `forge tunnel open -n lab-b -s pg` (by service name) is not implemented; note whether you need it

## 3. SFTP access to one app folder ✅

```powershell
forge nodes files sftp --help
forge nodes files sftp grant lab-c --help        # check the flags: root, user, expiry (Never behind a setting)
forge nodes files sftp list lab-c
```

- [ ] Grant SFTP on an app root, connect with WinSCP or `sftp`, and confirm you can't go above the app folder
- [ ] Edit, reset the password, and revoke; after revoke the login fails
- [ ] **Never** expiry is only offered when the setting is on

## 4. Terminal and exec ✅

```powershell
forge nodes logins lab-c
forge nodes terminal lab-c                       # interactive; exit with Ctrl-D
forge nodes exec lab-c -- uptime
forge sessions list                              # open sessions; end one with forge sessions ...
```

- [ ] Every exec and terminal session appears in `forge audit`
- [ ] An exec without MFA is refused (exit `5`)

## 5. Logs ✅ / 🟡

```powershell
forge stacks logs pg db --node lab-b --tail 50
# native app logs: UI Logs tab (follow), or (after the CLI fix) forge services logs <app> --node lab-c --follow
forge nodes command lab-c service_status --name nginx    # ⚠ F-02 until fixed
```

## 6. Clean up

```powershell
forge tunnel ports --node lab-b --clear
forge stacks down pg --volumes --yes; forge stacks delete pg --yes
# revoke any SFTP grants: forge nodes files sftp revoke ...
```
