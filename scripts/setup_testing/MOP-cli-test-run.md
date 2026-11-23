# MOP: Forge CLI test run

Step-by-step commands to test Forge end to end from the `forge` CLI on your
Windows PC. Each step says what the command does, and whether the console UI
can do the same.

Lab layout used here:

| VM | IP | Role in this test |
| --- | --- | --- |
| lab-a | 10.50.0.10 | Forge server **and** agent (worker) |
| lab-b | 10.50.0.11 | Agent: Docker plus a Seq container |
| lab-c | 10.50.0.12 | Agent: Python, .NET and PHP native services |

Legend: ✅ works from the CLI · 🟡 API only (`forge api call`) or UI only ·
❌ not implemented yet.

Run everything in **PowerShell**. Each JSON body goes into a small file, so
you don't have to fight PowerShell's quoting.

---

## Part 0: CLI on Windows

```powershell
cd <repo-root>
cargo build --release -p forge-cli                    # builds target\release\forge.exe
$env:Path = "<repo-root>\target\release;$env:Path"   # or add it to your user PATH once
$env:FORGE_ALLOW_HTTP = "1"     # the lab server is plain http (lab only: tokens go unencrypted)

forge contexts add lab --server http://10.50.0.10:8080
forge auth login               # admin password: scripts\setup_testing\.lab\forge-admin.env
forge auth whoami
forge version                  # client and server versions
```

Make a scratch folder for the JSON files:

```powershell
New-Item -ItemType Directory -Force C:\forge-test | Out-Null
cd C:\forge-test
```

Write-actions such as exec, package installs and file writes need an
**MFA-verified** session. If a command exits with code `5`, sign in again
with `forge auth login` and complete MFA.

---

## Part 1: Set up the environment (cluster and nodes)

### 1.1 Cluster ✅

The `lab` cluster already exists if you followed the main MOP. To create one
from the CLI instead:

```powershell
'{"name":"lab","description":"Hyper-V lab","abbr":"LAB"}' | Set-Content cluster.json
forge clusters create --file cluster.json
forge clusters list
forge clusters get lab
```

### 1.2 Add a node with an enrollment token ✅ (CLI · UI: **Nodes → Add node**)

This is the method you used through the UI. From the CLI:

```powershell
forge nodes enroll-token --cluster lab --hostname lab-b --ttl-minutes 60
```

It prints a one-time token. On the node (as a sudo user):

```sh
read -rs FORGE_ENROLL_TOKEN && export FORGE_ENROLL_TOKEN     # paste the token, press Enter
curl -fsSL 'http://10.50.0.10:8080/install.sh?terminal_account=forge-ops' \
  | sudo --preserve-env=FORGE_ENROLL_TOKEN sh
```

The agent installs, enrolls and shows up as **Online**.

### 1.3 Add a node over SSH with a password or key 🟡 (UI wizard · API; no typed CLI command)

The server connects to the node over SSH and installs the agent itself.

**UI:** **Nodes → Add node → SSH**: enter the host, confirm the host key,
choose password or private key, and pick the cluster.

**CLI (through the API):**

```powershell
# 1. Read the node's SSH host key and compare it with the node itself
'{"host":"10.50.0.12","port":22}' | Set-Content probe.json
forge api call nodes.probe --body '@probe.json' --yes
```

```powershell
# 2. Start the bootstrap job (fill in the fingerprint from step 1 and the cluster id)
@'
{
  "host": "10.50.0.12", "port": 22, "username": "<ssh-user>",
  "host_key_fingerprint": "SHA256:<from-probe>",
  "auth": { "type": "password", "password": "<ssh-password>" },
  "sudo_password": null,
  "cluster_id": "<cluster-id>",
  "keep_credential": false,
  "hostname": "lab-c",
  "terminal_account": "forge-ops", "terminal_account_sudo": false
}
'@ | Set-Content onboard.json
forge api call nodes.onboard --body '@onboard.json' --yes
forge jobs watch <job-id>
Remove-Item onboard.json        # it holds a password
```

To log in with a key, use
`"auth": {"type":"private_key","pem":"-----BEGIN OPENSSH PRIVATE KEY-----\n...","passphrase":null}`.
With `keep_credential: false`, the credential is used once and then dropped.

### 1.4 Check the nodes ✅

```powershell
forge nodes list --cluster lab   # ⚠ F-01: returns an empty table, see Findings; use `forge nodes list`
forge nodes get lab-a            # ✅ works, shows cluster_id, facts, agent version
forge nodes health lab-c
forge nodes metrics lab-b
forge nodes connectivity lab-c
forge nodes command lab-c service_status --name forge-agent   # ⚠ F-02: prints the queued job, not the status; then run: forge jobs watch <id>
forge nodes exec lab-c -- uname -a        # needs MFA
forge nodes terminal lab-c                # interactive shell (UI: node → Terminal)
```

Keep the node ids at hand, because the package and app bodies need ids, not names:

```powershell
forge -o json nodes list | ConvertFrom-Json | % result | ft id, hostname
$A = '<lab-a-id>'; $B = '<lab-b-id>'; $C = '<lab-c-id>'
```

### 1.5 Environment for the test apps ✅

```powershell
forge projects create lab --description "Lab test apps"
forge environments create lab-test --cluster lab --project lab
forge -o json environments list | ConvertFrom-Json | % result | ft id, name
$NS = '<lab-test-id>'
```

---

## Part 2: Install runtimes (App Store packages)

Runtimes are **App Store packages** (`forge packages`), not `forge plugins`.
Plugins are signed WASM extensions, which is something else.

| Runtime | Package id | Versions | Parameters |
| --- | --- | --- | --- |
| Python | `python` | `system` | — |
| .NET | `dotnet` | `8.0`, `9.0`, `10.0` | `flavor`: `aspnetcore` (default), `runtime`, `sdk` |
| PHP-FPM | `php` | `8.4`, `8.3`, `8.2`, `8.1`, `7.4` | `extensions` (comma list) |
| Docker | `docker` | `stable` | — (Docker CE and the compose plugin) |

```powershell
forge packages catalog                 # everything you can install
forge packages releases python         # versions (if the command takes a package id)
```

### 2.1 lab-c: Python, .NET SDK, PHP ✅ (UI: **App Store → package → Install**)

```powershell
"{`"package`":`"python`",`"version`":`"system`",`"params`":{},`"server_ids`":[`"$C`"]}" | Set-Content py.json
"{`"package`":`"dotnet`",`"version`":`"8.0`",`"params`":{`"flavor`":`"sdk`"},`"server_ids`":[`"$C`"]}" | Set-Content dotnet.json
"{`"package`":`"php`",`"version`":`"8.3`",`"params`":{`"extensions`":`"mbstring,xml,curl`"},`"server_ids`":[`"$C`"]}" | Set-Content php.json

forge packages install --file py.json
forge packages install --file dotnet.json
forge packages install --file php.json
forge packages installed
```

We use the .NET `sdk` flavor so the test app can be built on the node. In
production, `aspnetcore` (runtime only) is enough.

### 2.2 lab-b: Docker ✅

```powershell
"{`"package`":`"docker`",`"version`":`"stable`",`"params`":{},`"server_ids`":[`"$B`"]}" | Set-Content docker.json
forge packages install --file docker.json
forge packages installed
forge nodes exec lab-b -- docker version
```

### 2.3 Manage installed packages ✅

```powershell
forge packages action <installation-id> status --yes
forge packages action <installation-id> restart --yes
forge packages updates                     # updates available
forge packages action <installation-id> uninstall --yes
```

The CLI has no per-node filter (⚠ E-03). To list packages for one node:

```powershell
forge -o json packages installed | ConvertFrom-Json | % result | ? server_id -eq $B | ft   # filter on your PC
forge api request GET "/api/v1/installations?server_id=$B"                              # 🟡 filter on the server
```

---

## Part 3: Native services on lab-c (systemd)

How Forge lays out a native app on the node:

| What | Path on the node |
| --- | --- |
| Code | `/srv/forge/apps/<environment>/<app>/` |
| Env file | `/etc/forge/apps/<environment>/<app>.env` (generated, protected) |
| systemd unit | `forge-app-<environment>-<app>.service` |
| Runs as | its own system user `<environment>-<app>` (never root) |

Status per action:

| Action | CLI | UI |
| --- | --- | --- |
| Create app | 🟡 `forge api call services.create` | ✅ **Services → New** |
| Place on a node | 🟡 `forge api call services.place` | ✅ |
| Deploy | ✅ `forge services deploy` | ✅ |
| Start, stop, restart, status | ✅ `forge services action` | ✅ |
| Modify (command, env, port) | 🟡 `forge api call services.update` | ✅ **Configuration** tab |
| Logs | 🟡 `services.action` `logs` (or `nodes exec journalctl`) | ✅ **Logs** tab, with follow |
| Health checks | ✅ view (`forge services health`) · 🟡 edit (`app_health.update`) | ✅ form + YAML |
| Delete | ✅ `forge services delete` (after `remove`) | ✅ |
| Upload code | 🟡 `forge nodes files upload` / `nodes exec` (no app-code step) | ✅ file manager |

### 3.1 Python service

**1. Create the app** (this also creates its directory and user on first deploy):

```powershell
@"
{
  "environment_id": "$NS", "name": "pyhello", "runtime": "systemd",
  "command": "python3 /srv/forge/apps/lab-test/pyhello/app.py",
  "working_dir": "", "listen_port": 8001, "restart": true,
  "env": [ { "key": "GREETING", "value": "hello from python" } ],
  "dependencies": [ { "package": "python", "version": "system", "params": {} } ]
}
"@ | Set-Content pyhello.json
forge api call services.create --body '@pyhello.json' --yes     # note the "id"
$PY = '<pyhello-app-id>'
```

**2. Upload the code:**

```powershell
@'
import os
from http.server import BaseHTTPRequestHandler, HTTPServer
class H(BaseHTTPRequestHandler):
    def do_GET(self):
        self.send_response(200); self.end_headers()
        self.wfile.write(os.environ.get("GREETING", "hi").encode())
HTTPServer(("127.0.0.1", 8001), H).serve_forever()
'@ | Set-Content app.py
forge nodes files mkdir lab-c /srv/forge/apps/lab-test/pyhello
forge nodes files upload lab-c .\app.py /srv/forge/apps/lab-test/pyhello/app.py
forge nodes files chmod lab-c /srv/forge/apps/lab-test/pyhello/app.py 644
```

**3. Place it on lab-c and deploy:**

```powershell
"[{`"server_id`":`"$C`",`"role`":`"active`"}]" | Set-Content place.json
forge api call services.place -p id=$PY --body '@place.json' --yes
forge services deploy pyhello --environment lab-test --wait --yes
```

**4. Check, manage and read logs:**

```powershell
forge services list --environment lab-test
forge services show pyhello
forge services action pyhello status  --node lab-c
forge services action pyhello restart --node lab-c --yes
forge services health pyhello
forge nodes exec lab-c -- curl -s http://127.0.0.1:8001      # "hello from python"

'{"action":"logs","tail":200}' | Set-Content logs.json
forge api call services.action -p id=$PY -p server_id=$C --body '@logs.json' --yes
forge nodes exec lab-c -- journalctl -u forge-app-lab-test-pyhello -n 100 --no-pager
```

**5. Modify it** (change the env var). `PUT` replaces the whole spec:

```powershell
(Get-Content pyhello.json -Raw) -replace 'hello from python','hello v2' | Set-Content pyhello.json
forge api call services.update -p id=$PY --body '@pyhello.json' --yes
forge services deploy pyhello --environment lab-test --wait --yes
forge nodes exec lab-c -- curl -s http://127.0.0.1:8001      # "hello v2"
```

### 3.2 .NET service

Build a minimal web app on lab-c with the SDK installed in 2.1:

```powershell
forge nodes exec lab-c --shell -- 'sudo mkdir -p /srv/forge/apps/lab-test/dothello && cd /tmp && dotnet new web -o dothello --force && cd dothello && dotnet publish -c Release -o /tmp/dothello-out && sudo cp -r /tmp/dothello-out/. /srv/forge/apps/lab-test/dothello/'
```

> Whether `sudo` works there depends on the terminal account (`forge nodes
> logins lab-c`). Without sudo, build on Windows (`dotnet publish -c Release
> -r linux-x64 --self-contained false`), zip it, then use `forge nodes files
> upload` followed by `forge nodes files extract`.

```powershell
@"
{
  "environment_id": "$NS", "name": "dothello", "runtime": "systemd",
  "command": "dotnet /srv/forge/apps/lab-test/dothello/dothello.dll --urls http://127.0.0.1:8002",
  "working_dir": "", "listen_port": 8002, "restart": true, "env": [],
  "dependencies": [ { "package": "dotnet", "version": "8.0", "params": { "flavor": "sdk" } } ]
}
"@ | Set-Content dothello.json
forge api call services.create --body '@dothello.json' --yes
$DN = '<dothello-app-id>'
forge api call services.place -p id=$DN --body '@place.json' --yes
forge services deploy dothello --environment lab-test --wait --yes
forge nodes exec lab-c -- curl -s http://127.0.0.1:8002      # "Hello World!"
```

### 3.3 PHP service

The simplest test is PHP's built-in server as a native service. A real
deployment would use php-fpm behind a site.

```powershell
'<?php echo "hello from php " . PHP_VERSION;' | Set-Content index.php
forge nodes files mkdir lab-c /srv/forge/apps/lab-test/phphello
forge nodes files upload lab-c .\index.php /srv/forge/apps/lab-test/phphello/index.php
@"
{
  "environment_id": "$NS", "name": "phphello", "runtime": "systemd",
  "command": "php -S 127.0.0.1:8003 -t /srv/forge/apps/lab-test/phphello",
  "working_dir": "", "listen_port": 8003, "restart": true, "env": [],
  "dependencies": [ { "package": "php", "version": "8.3", "params": {} } ]
}
"@ | Set-Content phphello.json
forge api call services.create --body '@phphello.json' --yes
$PH = '<phphello-app-id>'
forge api call services.place -p id=$PH --body '@place.json' --yes
forge services deploy phphello --environment lab-test --wait --yes
forge nodes exec lab-c -- curl -s http://127.0.0.1:8003
```

> If deploy fails with `php: not found`, the php package installed only
> php-fpm and not the CLI. Note that as a finding.

### 3.4 Env files and unit files

| File | CLI | UI | Notes |
| --- | --- | --- | --- |
| App env `/etc/forge/apps/<ns>/<app>.env` | ❌ not as a file | ❌ not as a file | `/etc/forge` is protected. Edit env through the app definition: `services.update` (🟡) or the **Configuration** tab (✅). Secret values are never shown back. |
| Unit `/etc/systemd/system/forge-app-<ns>-<app>.service` | ✅ read · ✅ write (admin, MFA) | ✅ file editor | Forge **regenerates the unit on every deploy**, so manual edits are lost. Change the app spec instead. |

```powershell
forge nodes files cat  lab-c /etc/systemd/system/forge-app-lab-test-pyhello.service
forge nodes files stat lab-c /etc/systemd/system/forge-app-lab-test-pyhello.service   # shows the version
forge nodes files cat  lab-c /etc/forge/apps/lab-test/pyhello.env                     # expected: refused (protected)
```

**Not implemented yet ❌:** a dedicated "edit env file" or "edit unit file"
command or editor for apps, such as:

```powershell
forge services env pyhello --set GREETING=hi     # ❌ does not exist
forge services unit pyhello --edit               # ❌ does not exist
```

### 3.5 Remove and delete ✅ / 🟡

```powershell
'{"action":"remove"}' | Set-Content remove.json
forge api call services.action -p id=$PY -p server_id=$C --body '@remove.json' --yes   # 🟡 takes it off the node
forge services delete pyhello --environment lab-test --yes                                       # ✅
```

---

## Part 4: Containers on lab-b (Seq)

Seq is not in the App Store catalog, so run it as a **Compose stack** ✅ or
as a **single container** 🟡 (UI: **Containers → New**).

### 4.1 Create and deploy a stack ✅

```powershell
@'
services:
  seq:
    image: datalust/seq:latest
    environment:
      ACCEPT_EULA: "Y"
      SEQ_FIRSTRUN_ADMINPASSWORD: ${SEQ_PASSWORD}
    ports: ["127.0.0.1:5341:80"]
    volumes: ["seqdata:/data"]
    restart: unless-stopped
volumes:
  seqdata: {}
'@ | Set-Content seq.yml
'SEQ_PASSWORD=<choose-a-password>' | Set-Content seq.env

forge stacks create seq --environment lab-test --node lab-b -f seq.yml --env-file seq.env --dry-run
forge stacks create seq --environment lab-test --node lab-b -f seq.yml --env-file seq.env
forge stacks deploy seq --wait --yes
Remove-Item seq.env
```

It shows up in `forge stacks list`, in `forge services list` (runtime
`compose-service`, read-only there), and in the UI under **Containers** for
the node.

### 4.2 Logs, restart, update, roll back ✅

```powershell
forge stacks show seq
forge stacks logs seq seq --node lab-b --tail 200
forge stacks service seq restart --node lab-b --services seq
forge stacks service seq pull    --node lab-b --services seq

# Update: edit seq.yml (for example pin a tag), save a new revision, deploy it
(Get-Content seq.yml -Raw) -replace 'seq:latest','seq:2024.3' | Set-Content seq.yml
forge stacks update seq -f seq.yml
forge stacks deploy seq --wait --yes
forge stacks revisions seq
forge stacks rollback seq --yes
```

### 4.3 Any single container 🟡 (API · UI)

```powershell
forge nodes exec lab-b -- docker ps          # find the container id
'{"op":"logs","id":"<container-id>","tail":200}' | Set-Content clogs.json
forge api call containers.run -p id=lab-b --body '@clogs.json' --yes
```

The UI shows the same under **node → Containers**, with live follow and
start, stop, restart and remove.

### 4.4 Take down ✅

```powershell
forge stacks down seq --yes            # add --volumes to also delete the data
forge stacks delete seq --yes
```

---

## Part 5: Tunnel access (reach Seq from Windows)

A tunnel forwards a local port on your PC to `127.0.0.1:<port>` **on the
node**, like `ssh -L`.

| Action | CLI | UI |
| --- | --- | --- |
| Allow ports | ✅ `forge tunnel ports` | ✅ Settings → Tunnels |
| Open a tunnel | ✅ `forge tunnel open` | ❌ |
| List, close | ✅ | ✅ |

```powershell
forge tunnel ports --node lab-b --set 5341      # an empty list means no tunnels allowed
forge tunnel open -n lab-b -p 5341 -l 15341     # runs until Ctrl-C
```

Browse to <http://127.0.0.1:15341> for the Seq UI. In a second window:

```powershell
forge tunnel list
forge tunnel close --id <tunnel-id>
```

Same for the native apps: `forge tunnel ports --node lab-c --set 8001,8002,8003`,
then `forge tunnel open -n lab-c -p 8001 -l 18001`.

**Not implemented yet ❌:** `--service`/`-s` (tunnel by service name instead
of port), for example `forge tunnel open -n lab-b -s seq`. Only `--port` works.

---

## Part 5b: React app on lab-b and lab-c behind an Nginx load balancer

Goal: build the React portal from `<path-to-your-react-app>`, serve the
static build on **lab-b and lab-c**, and load-balance it through **Nginx on
lab-a** (the edge).

```text
browser ──► lab-a :80  Nginx (Forge Site "xcweb", round_robin)
                 ├──► lab-b :8080  xcweb (native service)
                 └──► lab-c :8080  xcweb (native service)
```

| Piece | How | Status |
| --- | --- | --- |
| Build the app | `npm run build` on Windows | ✅ (outside Forge) |
| Static web server per node | Native service: `python3 -m http.server` (Python installed by deploy) | 🟡 create/place via API · ✅ deploy |
| Upload the build | `forge nodes files upload` + `extract` | ✅ |
| Nginx on the edge | App Store package `nginx` | ✅ |
| Load balancer | Forge **Site**: `edges: [lab-a]`, target `kind: app` → every node of the app | ✅ `forge sites create/preview/apply` |
| Static hosting as a Forge feature (“serve this folder”, SPA fallback) | — | ❌ not implemented, see below |

### 5b.1 Build the React app (Windows)

```powershell
cd <path-to-your-react-app>
npm ci
npm run build                     # tsc + vite build + prerender → dist\
Compress-Archive -Path .\dist\* -DestinationPath C:\forge-test\xcweb.zip -Force
cd C:\forge-test
```

### 5b.2 Create the service and place it on lab-b and lab-c 🟡

The service must listen on `0.0.0.0`, not `127.0.0.1`, because Nginx on
lab-a connects to it over the network.

```powershell
@"
{
  "environment_id": "$NS", "name": "xcweb", "runtime": "systemd",
  "command": "python3 -m http.server 8080 --bind 0.0.0.0 --directory /srv/forge/apps/lab-test/xcweb/site",
  "working_dir": "", "listen_port": 8080, "restart": true, "env": [],
  "dependencies": [ { "package": "python", "version": "system", "params": {} } ]
}
"@ | Set-Content xcweb.json
forge api call services.create --body '@xcweb.json' --yes
$XW = '<xcweb-app-id>'

"[{`"server_id`":`"$B`",`"role`":`"active`"},{`"server_id`":`"$C`",`"role`":`"active`"}]" | Set-Content place-bc.json
forge api call services.place -p id=$XW --body '@place-bc.json' --yes
```

### 5b.3 Deploy first: this creates the app folder and user ✅

The folder `/srv/forge/apps/lab-test/xcweb` and the user `lab-test-xcweb`
are created on the first **deploy**, not on create or place (⚠ F-03). Before
that, uploads fail with `` `forge` was not found `` (⚠ F-04).

```powershell
forge services deploy xcweb --environment lab-test --wait --yes      # installs Python on lab-b if missing
forge services show xcweb                                          # two instances: lab-b, lab-c
```

The service starts with an empty `site` folder and answers 404 until the
next step.

### 5b.4 Upload the build into the app root ✅

Each deployed app gets its own **app root**. It covers only
`/srv/forge/apps/<ns>/<app>`, and every operation runs as the app's user,
never root, so files get the right owner and can't land outside the app.
Paths are relative to that root.

App roots are **not created automatically** (⚠ F-05). Out of the box,
`forge nodes files roots lab-b` lists only `node`. Approve one root per node
first. There's no typed CLI command, so use the console (node → **Files** →
roots) or the API 🟡. The API defaults to `read_only: true`, so pass `false`:

```powershell
@"
{ "kind": "service", "label": "xcweb", "path": "/srv/forge/apps/lab-test/xcweb",
  "read_only": false, "service_id": "$XW" }
"@ | Set-Content root-xcweb.json
$RB = (forge -o json api call files.add_root -p id=$B --body '@root-xcweb.json' --yes | ConvertFrom-Json).result.id
$RC = (forge -o json api call files.add_root -p id=$C --body '@root-xcweb.json' --yes | ConvertFrom-Json).result.id
"$RB  $RC"                                       # two fr_... ids

forge nodes files roots lab-b                    # now lists node + xcweb (spec.kind "app" = runs as the app user) ✅
forge nodes files roots lab-c

foreach ($p in @(@('lab-b',$RB), @('lab-c',$RC))) {
  $n = $p[0]; $r = $p[1]
  forge nodes files upload  $n .\xcweb.zip /xcweb.zip --root $r     # ✅ runs as lab-test-xcweb, confined: full
  forge nodes files extract $n /xcweb.zip /site --root $r            # creates /site; it must NOT exist (no mkdir first)
  forge nodes files ls      $n /site --root $r                       # index.html, assets/
}
# ✅ Test run: 143 entries extracted on both nodes, owned by lab-test-xcweb.
# If /site already exists (even empty), delete it first:
#   forge nodes files rm $n /site --root $r --permanent --yes
```

> `forge nodes exec lab-b -- ls /srv/forge/apps/lab-test/xcweb` gives
> **Permission denied**. That's expected: exec runs as the terminal login
> (`forge-ops`), and each app folder is private to the app's user. Use
> `forge nodes files ... --root $RB`, or `sudo` if the login has it.
>
> Run `add_root` only once per node. Running it again creates a duplicate
> root (⚠ F-06). `extract` refuses an existing folder (⚠ F-07), so don't
> `mkdir` the destination first.
>
> The whole-node root (`/srv/forge/apps/...` without `--root`) also works
> after the deploy, but runs as root, so prefer the app root.

### 5b.4b Test each node ✅

```powershell
forge nodes exec lab-a -- curl -sI http://10.50.0.11:8080/  # HTTP/1.0 200 from lab-b
forge nodes exec lab-a -- curl -sI http://10.50.0.12:8080/  # HTTP/1.0 200 from lab-c
```

### 5b.5 Nginx on lab-a (the edge) ✅

```powershell
"{`"package`":`"nginx`",`"version`":`"stable`",`"params`":{},`"server_ids`":[`"$A`"]}" | Set-Content nginx.json
forge packages install --file nginx.json
forge packages installed
```

lab-a's Forge server listens on `:8080`, and Nginx on `:80`, so they don't
clash.

### 5b.6 Create the load-balanced Site ✅

The `app` target expands to every node `xcweb` runs on (lab-b and lab-c),
and is resolved again on every apply.

```powershell
@"
{
  "name": "xcweb",
  "domains": ["xcweb.lab.local"],
  "upstreams": [],
  "balance": "round_robin",
  "tls": "off",
  "edges": ["$A"],
  "targets": [ { "kind": "app", "service_id": "$XW", "port": 8080 } ]
}
"@ | Set-Content site-xcweb.json

forge sites preview --file site-xcweb.json      # shows the generated nginx config: upstream with both nodes
forge sites create  --file site-xcweb.json
forge sites apply   xcweb          # ⚠ F-09: no --yes flag here
forge sites get     xcweb                       # edge_status for lab-a
forge clusters routing lab                      # ⚠ F-11: empty for sites with explicit "edges" (no cluster_id)
```

To use another method, set `"balance": "least_conn"` or `"ip_hash"` (sticky
per client IP), then run `forge sites update xcweb --file site-xcweb.json` and
`forge sites apply xcweb`.

> If `create` refuses explicit `edges` and asks for an ingress profile, set
> up **Cluster → Ingress → Profile** (Standalone, edge = lab-a) in the UI,
> then use `"cluster_id": "<cluster-id>"` instead of `edges`. Log that as a
> finding: the ingress profile has no typed CLI command.

### 5b.7 Test from Windows (✅ test run: Nginx on lab-a served the React index.html)

No DNS is needed. Send the host name in the header:

```powershell
curl.exe -s -H "Host: xcweb.lab.local" http://10.50.0.10/ | Select-Object -First 5
```

To use a browser, add `10.50.0.10  xcweb.lab.local` to
`C:\Windows\System32\drivers\etc\hosts` (as admin) and open
<http://xcweb.lab.local>.

To see which node served each request:

```powershell
1..6 | % { curl.exe -s -o NUL -H "Host: xcweb.lab.local" http://10.50.0.10/ }
# ⚠ F-12: /var/log/nginx/forge-traffic.log doesn't exist for sites with explicit "edges".
# Check each node's service log instead: GET / lines from 10.50.0.10 on both nodes.
'{"action":"logs","tail":10}' | Set-Content logs.json
forge api call services.action -p id=$XW -p server_id=$B --body '@logs.json' --yes
forge api call services.action -p id=$XW -p server_id=$C --body '@logs.json' --yes
```

### 5b.8 Fail-over test

```powershell
forge services action xcweb stop --node lab-b --yes
1..4 | % { curl.exe -s -o NUL -w "%{http_code}`n" -H "Host: xcweb.lab.local" http://10.50.0.10/ }   # all 200 (lab-c only)
forge services action xcweb start --node lab-b --yes
```

### 5b.9 Update the app (new build) ✅

```powershell
cd <path-to-your-react-app>; npm run build
Compress-Archive -Path .\dist\* -DestinationPath C:\forge-test\xcweb.zip -Force; cd C:\forge-test
foreach ($p in @(@('lab-b',$RB), @('lab-c',$RC))) {
  $n = $p[0]; $r = $p[1]
  forge nodes files upload  $n .\xcweb.zip /xcweb.zip --root $r --overwrite   # check the flag name with --help
  forge nodes files extract $n /xcweb.zip /site-new --root $r                 # extract beside the live site
  forge nodes files rm      $n /site --root $r --permanent --yes
  forge nodes files mv      $n /site-new /site --root $r                      # swap in (brief 404 window)
}
# a static server needs no restart
```

### 5b.10 Clean up

```powershell
forge sites remove xcweb --yes; forge sites delete xcweb --yes
'{"action":"remove"}' | Set-Content remove.json
forge api call services.action -p id=$XW -p server_id=$B --body '@remove.json' --yes
forge api call services.action -p id=$XW -p server_id=$C --body '@remove.json' --yes
forge services delete xcweb --environment lab-test --yes
```

### Not implemented yet ❌

| Feature | Example of what doesn't exist | Use instead |
| --- | --- | --- |
| Static site hosting (serve a folder, SPA fallback to `index.html`, cache headers) | `forge services create xcweb --static ./dist --nodes lab-b,lab-c` | A native service with `python3 -m http.server`. It has **no SPA fallback**: deep links such as `/about` return 404 unless the prerender produced `about/index.html` |
| Upload a build to every node of an app at once | `forge services push xcweb ./dist` | `forge nodes files upload` + `extract`, per node |
| Typed CLI for the cluster ingress profile | `forge clusters ingress set --edge lab-a --layout standalone` | the UI (**Cluster → Ingress → Profile**) |
| Open the site from a browser without editing `hosts` | automatic `xcweb.lab.local` name for Sites | the `hosts` file or `curl -H Host:` |

---

## Part 6: Not implemented yet (summary)

| Feature | Example of what doesn't exist | Use instead |
| --- | --- | --- |
| Typed CLI create/edit/place for apps | `forge services create --file app.json` | `forge api call services.create/update/place`, or the UI |
| Typed CLI app logs | `forge services logs pyhello --node lab-c --follow` | `services.action` `logs`, `nodes exec journalctl`, the UI **Logs** tab |
| Edit an app's env file as a file | `forge services env pyhello --edit` | the app spec (`env`) |
| Persistent unit-file edits | `forge services unit pyhello --edit` | the app spec (it regenerates on deploy) |
| SSH onboarding as a CLI command | `forge nodes onboard --host … --password` | `forge api call nodes.onboard`, or the UI wizard |
| Tunnel by service name | `forge tunnel open -n lab-b -s seq` | `-p <port>` |
| Open a tunnel from the UI | — | the CLI |
| Seq in the App Store | `forge packages install seq` | a Compose stack |
| Per-node package list in the CLI | `forge packages installed --node lab-c` | `forge api request GET /api/v1/installations?server_id=…` |
| Code upload/deploy for native apps | `forge services push ./src` | `forge nodes files upload` + `extract` |

## Findings from the test run

Logged while going through this MOP. Fix them after the full run.

| # | Step | Command | Expected | Actual | Type |
| --- | --- | --- | --- | --- | --- |
| F-01 | 1.4 | `forge nodes list --cluster lab` | The 3 lab nodes | Empty table (header only). `forge nodes get lab-a` shows the node is `online` with a `cluster_id`, so the `--cluster` filter matches nothing: either it doesn't resolve the cluster name `lab`, or it compares against the wrong field | Bug |
| F-02 | 1.4 | `forge nodes command lab-c service_status --name forge-agent` | The unit's status (active/inactive, since when) | Prints the raw job object just after queuing it (`"status": "queued"`, `"state": {}`) and exits; the actual result is never shown. It should wait for the job (up to ~25 s, like `forge services action`) and print the result, with `--no-wait` to return the job id only. Workaround: `forge jobs watch <id>` | Bug |
| E-03 | 2.3, all lists | `forge packages installed` (and every `list` command) | Every filter the API supports is a CLI flag | `packages installed` has no filter even though the API supports `?server_id=`. Generalise it: for every list command, expose each query filter of its API endpoint as a flag (`--node`, `--package`, `--status`, `--environment`, `--cluster`, …). Flags take names as well as ids, as the other commands do. Add a parity check (like `forge parity` / the docs check) that fails CI when an endpoint's query parameter has no matching CLI flag, so new filters are never missed. Document each flag in the CLI reference | Enhancement |
| E-04 | 2.3, all lists | `forge packages installed`, `audit`, `jobs list`, … | Long output is paged and can be fetched in pages | Everything prints in one go. Add (1) **page flags** on every list: `--limit <n>` (default e.g. 50), `--page <n>` or `--after <cursor>`, `--all` to fetch everything, and a footer such as `showing 1–50 of 312 · next: --page 2`, using the API's limit/cursor parameters where they exist (and adding them to endpoints that lack them); (2) an **interactive pager** like `git log` / `journalctl`: on a terminal, long output opens in `less` (Linux/macOS; `$PAGER` respected), or in a built-in pager on Windows; `--no-pager` and pipes/`-o json` print plainly | Enhancement |
| F-03 | 5b.3 | `services.create` + `services.place`, then upload | The app's folder and user exist as soon as the app is placed on a node, so code can be uploaded before the first deploy | Folder `/srv/forge/apps/<ns>/<app>`, the user and the app root are created only on the first **deploy**. Uploading first fails. Provision them on place (or with a `prepare` action), and show the app root in the app's details | Bug / design |
| F-04 | 5b.3 | `forge nodes files mkdir lab-b /srv/forge/apps/lab-test/xcweb/site` | Creates missing parents (`-p`), or a clear error | No `--parents` option. The error `` `forge` was not found [not_found] `` names one path segment and reads as if the `forge` command is missing. The message should say "parent folder /srv/forge does not exist; use --parents". `upload` says "the destination folder does not exist" without naming it | Bug (UX) |
| F-05 | 5b.4 | `forge nodes files roots lab-b` after deploying `xcweb` | An app root for `lab-test/xcweb` is listed, since the app is deployed there | Only the built-in `node` root. App roots must be approved by hand per node (`POST /servers/{id}/files/roots`, `file.admin`), it defaults to `read_only: true`, and the CLI has no typed command (parity.json: "admin setting in the console"). Forge should create a writable app root for each native app on deploy (or on place, see F-03), remove it when the app leaves the node, and the CLI should get `forge nodes files roots add/remove` | Bug / design |
| F-06 | 5b.4 | `forge api call files.add_root` run twice for the same app and node | Second call refused (`409`, the root exists) or returns the existing root | Creates a second root with the same `service_id` and `base` (seen: `fr_ff3c…` and `fr_50a6…` on lab-b). Add a uniqueness check per node + kind + base (+ service). **UI symptom:** the node **Files** root dropdown lists `xcweb (/srv/forge/apps/lab-test/…)` twice. The labels are identical and cut off, so you can't tell the entries apart, and there's no remove option there. Fix: refuse duplicates (server); in the dropdown show the label plus the full path in a tooltip, and group app roots under **Services**; offer **Remove root** to admins; migrate or clean up existing duplicates | Bug |
| F-07 | 5b.4 | `forge nodes files extract lab-b /xcweb.zip /site --root …` when `/site` exists | Option to extract into an existing folder (`--into`/`--overwrite`), as a deploy needs | `error: the destination already exists; choose a new folder [conflict]`. Extract only creates a new folder, so updating a site needs extract-to-new + rm + mv, with a short 404 window. Add `--overwrite`, or an atomic "replace folder" (extract to temp, swap). **Confirmed in the test run:** after deleting the empty `/site` by hand, `extract` worked on lab-b and lab-c (143 entries, 13.6 MB, as `lab-test-xcweb`, `confined: full`, nothing skipped) ✅. So extract itself works; only an existing destination blocks it | Enhancement |
| F-08 | 5b.4 | `forge nodes files upload lab-b .\xcweb.zip /xcweb.zip --root …` run a second time | Refused **before** the transfer (target exists, use `--overwrite`), or the temp file removed on failure | A full-size orphan `.forge-upload-<hash>.part` (9.3 MB, mode 600) stays in the app folder after the second upload. The agent must delete the `.part` file on every failure path, check for conflicts before streaming, and sweep stale `.part` files | Bug |
| F-09 | 5b.6 | `forge sites apply xcweb --yes` | `--yes` accepted like every other changing command (`sites remove`, `sites delete`, `apps deploy` all take it) | `error: unexpected argument '--yes' found`. `apply` pushes config to edges but has no `--yes`/prompt. Add `--yes` (and a confirmation prompt) for consistency, and accept `--yes` as a no-op on every command so scripts never break | Bug (UX) |
| F-10 | 5b.6 | `forge sites preview --file site-xcweb.json` | The rendered Nginx config, readable | Prints JSON with the config as one escaped string (`
`), and shows **both** an `http` and an `https` variant although `tls` is `off`; the `https` variant references Let's Encrypt files that don't exist. Print only the variant that will be used, as plain text (`-o json` for the full object). Preview content itself is correct ✅: `upstream forge_xcweb` with 10.50.0.11:8080 and .12:8080 | Enhancement |
| F-11 | 5b.6 | `forge clusters routing lab` after creating site `xcweb` with explicit `edges` | The site in the routing table (domain → lab-a → xcweb → .11/.12) | `{"endpoints": []}`. Routing only covers sites with `cluster_id` (ingress profile). Sites with explicit `edges` are invisible there. Show them too (they are on nodes of the cluster), or say so in the output. **UI too:** **Cluster → lab → Ingress** is empty, because it lists only sites with `cluster_id` = this cluster (`Endpoints.tsx:115`); the site appears only on the workspace-level **Ingress** page (`/ingress`). The site is stored and live (Nginx serves it, `forge sites get` shows it), so nothing is lost. Fix: show edge-based sites in the cluster that owns their edge nodes (cluster Ingress page and `clusters routing`), or label them "not linked to a cluster" with a link; offer "link to cluster" | Bug / design |
| F-12 | 5b.7 | `tail /var/log/nginx/forge-traffic.log` on lab-a | Per-request log with the chosen upstream, for every site | File doesn't exist: the traffic log format is set up by the cluster ingress profile, so sites with explicit `edges` get no traffic log and no **Watch traffic** view. Enable the Forge log format for every site Forge renders (in the site's own config), so edge-based sites are observable too. Related to F-11 | Bug |
| E-05 | 5b | app code uploads | Uploads scoped to the app: `forge services files xcweb upload ./dist` (no root id lookup), to every node of the app at once; a **service account** whose access profile allows writing only that app's root, for CI deploys | The app root already exists (D-083: confined to the app folder, runs as the app user), but you look up its id per node with `forge nodes files roots` and upload per node. Add an app-level files command that resolves the root on every placed node, and access-profile scoping of `file.write` to one app's root so a CI token can deploy files and nothing else | Enhancement |
| E-06 | 5b.3 | `forge services show xcweb` | A readable summary: name, runtime, environment; one line per instance with the **node name**, status and state; and for native services the unit, folder, run-as user and app root | Raw JSON (see E-02). Instances show only `server_id` uuids. Container-only fields (`image`, `entrypoint`, `volumes`, `ports`, `labels`, `hostname`, `user`, `extra_hosts`) are printed empty for a `service`, although the server refuses them for that runtime. The unit `forge-app-<ns>-<app>`, the folder `/srv/forge/apps/<ns>/<app>`, the user `<ns>-<app>` and the app root aren't shown. Result seen: both instances `deployed`/`active`, so the deploy works ✅ | Enhancement |
| F-13 | update | `forge system check` with MFA required but no authenticator enrolled | A clear message before asking for a code: "MFA is required for this action but your account has no authenticator; set one up in Settings → Security, or an admin can run `forge-server mfa off` on a lab" | Prompts "One-time code from your authenticator app", accepts any input, then fails with `set up an authenticator app in Settings → Security first [bad_request]`. Check enrollment first (the server knows) and fail fast; same in the UI: say why **Check now** shows nothing instead of reporting 0 updates. **Also:** the error says "Settings → Security", but there is no Security page; the setup is the **Multi-factor authentication** panel low on **Settings**. Fix the message (and link `/settings#mfa`), or give MFA its own **Security** section | Bug (UX) |
| F-14 | update | UI **Updates** page | A **Forge server** section on the Updates page: current version, latest on the feed, feed URL, last check, its own **Check now** and **Update now**, and why it can't update (no MFA / no permission / feed unreachable / signature key missing) | The page covers only App Store packages and agents. The server update exists only as a header badge (`UpdateBadge.tsx`), hidden unless an update was found **and** the session may update (`can_update`). The page's **Check now** checks packages only. With MFA required but not enrolled, nothing explains why no server update shows. Agents show "up to date" because they're compared with the control plane, which is fine but should be stated ("agents follow the server version; update the server first") | Bug (UX) |
| E-07 | lab | `forge users delete|disable <user>` | Disable or delete an account from the CLI (and revoke its keys) | `forge users` has only `list`, `create`, `reset-passkeys`. **There is no API route for it either**: `/users` has only list/create and `/users/{id}/api-keys` (revoke). An account can't be disabled or deleted anywhere except by revoking its keys; the console's member page should be checked for the same gap. Add `PATCH /users/{id} {disabled}` and `DELETE /users/{id}` (delete only when the account owns nothing), then `forge users disable|enable|delete --yes`, all audited; disabling must also end live sessions and tunnels | Bug |
| E-01 | all | any table / `get` output | Coloured output for readability | Plain text only. Add colour in `forge-cli/src/out.rs`: status words (green `online`/`succeeded`, yellow `pending`/`running`, red `offline`/`failed`), bold headers, red `error:` prefix, coloured JSON for `get`/`show`. Only when stdout is a terminal; off for pipes, `-o json`, `NO_COLOR`; `--color auto\|always\|never` flag. No new dependency (`anstream`/`anstyle` already come with clap). About half a day | Enhancement |
| E-02 | all | `forge nodes get lab-a`, `nodes command`, `apps show`, `apps action`, … | Readable output for every command; choose JSON or XML with `--output` | `-o/--output` exists but only has `table` and `json`. In the default `table` mode, commands without defined columns (every `get`/`show` and most actions) print raw pretty-printed JSON. `-o json` prints one compact line. Fix in `forge-cli/src/out.rs`: (1) a generic key/value view for objects, with nested objects such as `facts` as indented sections and epoch-ms timestamps shown as dates; (2) pretty `-o json` on a terminal, compact when piped; (3) new formats `-o xml` and `-o yaml` from the same data. About 1.5 days, together with E-01 | Enhancement |

### Fix status (local, uncommitted; not yet verified on the lab)

| # | Status | What to use now |
| --- | --- | --- |
| F-01 | ✅ fixed | `forge nodes list --cluster lab` resolves the name |
| F-02 | ✅ fixed | `forge nodes command …` waits (default 25 s, `--timeout-secs`), `--no-wait` for the job id |
| F-03 | ✅ fixed | Placing an app on a node runs a `prepare` step: folder, user and root exist before deploy. Also `forge services action <app> prepare --node <n>` |
| F-04 | ✅ fixed | `forge nodes files mkdir … --parents`; errors name the full missing path |
| F-05 | ✅ fixed | The app root is created automatically on place/deploy and removed on remove/delete |
| F-06 | ✅ fixed | A duplicate root gets `409`; existing duplicates are hidden from the list; UI dropdown grouped, full path on hover, **Remove root**; `forge nodes files roots add/remove` |
| F-07 | ✅ fixed | `forge nodes files extract … --replace` (atomic swap on Linux) |
| F-08 | ✅ fixed | Failed uploads delete the `.part` file; an existing target is refused before streaming; stale `.part` files are swept after 24 h |
| F-09 | ✅ fixed | `forge sites apply xcweb --yes` (also `sites renew`, `packages policy`) |
| F-10 | ✅ fixed | `forge sites preview` prints only the config in use, as text; the API `https` is `null` when TLS is off |
| F-11 | ✅ fixed | Explicit-edge sites appear in `forge clusters routing` (SOURCE column) and **Cluster → Ingress** (badge, **Link to cluster**) |
| F-12 | ✅ fixed (needs new agent on the edges) | Every site writes `/var/log/nginx/forge-traffic.log` |
| E-01 | ✅ done | Colours on a terminal; `--color auto\|always\|never`, `NO_COLOR` |
| E-02 | ✅ done | Readable key/value output everywhere; `-o json\|yaml\|xml` |
| E-03 | ✅ done | API filters as flags (`packages installed --node/--package`, …) and a CI check for missing flags |
| E-04 | ✅ mostly | `--limit/--page/--all`, footer, pager (`--no-pager`). Not yet on `tunnel list`, `plugins list`, `dns list`, `jobs steps`, `stacks revisions`; many endpoints still page in the CLI, not on the server |
| E-05 | 🟡 partial | `forge services files <app> ls\|upload\|extract\|mkdir` on every node of the app. **Open:** service account limited to one app's files (needs a decision: a new `file.app_write` permission is recommended) |
| E-06 | ✅ done | `forge services show` summary with node names, unit, folder, user |
| new | ✅ done | `forge services create/update --file`, `apps place --node`, `apps logs --node [--follow]`, `apps remove --node` |

Still open on the server: paging parameters on most list endpoints;
`?cluster_id=` on `/servers` and `?package=` on `/installations` (the CLI
filters on its side for now).

## Clean up

```powershell
forge stacks down seq --volumes --yes; forge stacks delete seq --yes
# for each app: remove from the node (3.5), then forge services delete <app> --yes
forge tunnel ports --node lab-b --clear; forge tunnel ports --node lab-c --clear
Remove-Item C:\forge-test -Recurse -Force
forge contexts remove lab
Remove-Item Env:FORGE_ALLOW_HTTP
```

Write down every failure (command, output, exit code) and send it back so it
can be turned into an issue.
