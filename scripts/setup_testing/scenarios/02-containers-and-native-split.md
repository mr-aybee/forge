# 02 · Containers on one node, native apps on the other

**Who:** a team that runs third-party tools as containers (Seq for logs) on
one server, and its own code as native systemd services on another.

```text
lab-b  Docker ── stack "seq" (datalust/seq)        127.0.0.1:5341
lab-c  systemd ── api  (Python)   :8001  ──logs──► Seq on lab-b :5341
                  dot  (.NET 8)   :8002
```

| Cluster mapping | |
| --- | --- |
| Cluster | `lab` |
| lab-b | container host (Docker) |
| lab-c | native host (Python, .NET) |
| Environment | `s02` |

Prerequisite: [00-common-setup](00-common-setup.md).

## 1. Environment and runtimes ✅

```powershell
$NS = New-Ns 's02'
Install-Pkg 'docker' 'stable' @{} @($B)
Install-Pkg 'python' 'system' @{} @($C)
Install-Pkg 'dotnet' '8.0' @{ flavor = 'sdk' } @($C)
forge packages installed          # ⚠ E-03: no --node filter yet
forge nodes exec lab-b -- docker version
```

## 2. Seq as a Compose stack on lab-b ✅

Seq must accept log events from lab-c, so it listens on lab-b's LAN address.
The UI stays on loopback and is reached through a tunnel.

```powershell
@'
services:
  seq:
    image: datalust/seq:latest
    environment:
      ACCEPT_EULA: "Y"
      SEQ_FIRSTRUN_ADMINPASSWORD: ${SEQ_PASSWORD}
    ports:
      - "127.0.0.1:5380:80"       # UI, tunnel only
      - "10.50.0.11:5341:5341" # ingestion from lab-c
    volumes: ["seqdata:/data"]
    restart: unless-stopped
volumes: { seqdata: {} }
'@ | Set-Content seq.yml
'SEQ_PASSWORD=<choose-one>' | Set-Content seq.env
forge stacks create seq --environment s02 --node lab-b -f seq.yml --env-file seq.env
forge stacks deploy seq --wait --yes
Remove-Item seq.env
forge stacks show seq
forge stacks logs seq seq --node lab-b --tail 50
```

## 3. Python API on lab-c that logs to Seq 🟡 create · ✅ deploy

```powershell
@'
import json, os, urllib.request
from http.server import BaseHTTPRequestHandler, HTTPServer
SEQ = os.environ["SEQ_URL"]
def log(msg):
    body = json.dumps({"@t": __import__("datetime").datetime.utcnow().isoformat()+"Z", "@mt": msg}).encode()
    req = urllib.request.Request(SEQ + "/ingest/clef", body, {"Content-Type": "application/vnd.serilog.clef"})
    try: urllib.request.urlopen(req, timeout=2)
    except Exception as e: print("seq:", e, flush=True)
class H(BaseHTTPRequestHandler):
    def do_GET(self):
        log("GET {Path}".replace("{Path}", self.path))
        self.send_response(200); self.end_headers(); self.wfile.write(b"api ok")
HTTPServer(("127.0.0.1", 8001), H).serve_forever()
'@ | Set-Content app.py

$API = New-App @{
  environment_id = $NS; name = 'api'; runtime = 'systemd'
  command = 'python3 /srv/forge/apps/s02/api/app.py'; working_dir = ''; listen_port = 8001; restart = $true
  env = @(@{ key = 'SEQ_URL'; value = 'http://10.50.0.11:5341' })
  dependencies = @(@{ package = 'python'; version = 'system'; params = @{} })
}
Set-Placement $API @($C)
forge services deploy api --environment s02 --wait --yes          # creates folder + user
```

Upload the code into the app root, then restart:

```powershell
forge nodes files roots lab-c                                # app root for s02/api (auto after ⚠ F-05 fix)
$RAPI = '<root-id>'
forge nodes files upload lab-c .\app.py /app.py --root $RAPI
forge services action api restart --node lab-c --yes
forge nodes exec lab-c -- curl -s http://127.0.0.1:8001/    # api ok
```

## 4. .NET service on lab-c 🟡 create · ✅ deploy

```powershell
dotnet new web -o dot --force; dotnet publish dot -c Release -o dot-out    # on Windows (needs .NET 8 SDK)
Compress-Archive -Path .\dot-out\* -DestinationPath dot.zip -Force

$DOT = New-App @{
  environment_id = $NS; name = 'dot'; runtime = 'systemd'
  command = 'dotnet /srv/forge/apps/s02/dot/out/dot.dll --urls http://127.0.0.1:8002'
  working_dir = ''; listen_port = 8002; restart = $true; env = @()
  dependencies = @(@{ package = 'dotnet'; version = '8.0'; params = @{ flavor = 'aspnetcore' } })
}
Set-Placement $DOT @($C)
forge services deploy dot --environment s02 --wait --yes
$RDOT = '<root-id>'                                          # from forge nodes files roots lab-c
forge nodes files upload  lab-c .\dot.zip /dot.zip --root $RDOT
forge nodes files extract lab-c /dot.zip /out --root $RDOT
forge services action dot restart --node lab-c --yes
forge nodes exec lab-c -- curl -s http://127.0.0.1:8002/    # Hello World!
```

## 5. Verify

```powershell
1..5 | % { forge nodes exec lab-c -- curl -s http://127.0.0.1:8001/ping$_ }
forge tunnel ports --node lab-b --set 5380
forge tunnel open -n lab-b -p 5380 -l 15380                 # browse http://127.0.0.1:15380, sign in, see 5 "GET /pingN" events
```

- [ ] `forge services list --environment s02` shows `api` and `dot` (systemd) and `seq` (compose-service)
- [ ] The UI node page for lab-b lists the Seq container; for lab-c, the two services
- [ ] Seq shows the events from lab-c
- [ ] `forge stacks service seq restart --node lab-b --services seq` works, and the events are still there afterwards (volume)
- [ ] Logs: `forge stacks logs seq seq --node lab-b`, plus the service logs (UI **Logs** tab or `services.action logs`)

## 6. Clean up

```powershell
forge tunnel ports --node lab-b --clear
forge stacks down seq --volumes --yes; forge stacks delete seq --yes
J @{ action = 'remove' } rm.json | Out-Null
foreach ($id in $API,$DOT) { forge api call services.action -p id=$id -p server_id=$C --body '@rm.json' --yes }
forge services delete api --environment s02 --yes; forge services delete dot --environment s02 --yes
```
