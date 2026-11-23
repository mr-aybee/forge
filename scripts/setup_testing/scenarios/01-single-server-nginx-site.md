# 01 · One server hosts everything

**Who:** a small team with one VM. The Forge server, the agent, the website
and Nginx all run on the same machine.

```text
browser ──► lab-a :80 Nginx (Site "s01-web") ──► lab-a :8081 web (native service)
            lab-a :8080 Forge server (same machine)
```

| Cluster mapping | |
| --- | --- |
| Cluster | `lab` |
| Nodes used | lab-a (server + agent + edge + worker) |
| Environment | `s01` |

Prerequisite: [00-common-setup](00-common-setup.md).

## 1. Environment

```powershell
$NS = New-Ns 's01'
```

## 2. Nginx on lab-a ✅

```powershell
Install-Pkg 'nginx' 'stable' @{} @($A)
forge packages installed
forge nodes exec lab-a -- systemctl is-active nginx       # active
```

Check: the Forge server keeps `:8080`, and Nginx takes `:80`/`:443`.

## 3. The website as a native service 🟡 create · ✅ deploy

Use any static build. The test uses the React portal:

```powershell
cd <path-to-your-react-app>; npm ci; npm run build
Compress-Archive -Path .\dist\* -DestinationPath C:\forge-test\site.zip -Force; cd C:\forge-test

$WEB = New-App @{
  environment_id = $NS; name = 'web'; runtime = 'systemd'
  command = 'python3 -m http.server 8081 --bind 127.0.0.1 --directory /srv/forge/apps/s01/web/site'
  working_dir = ''; listen_port = 8081; restart = $true; env = @()
  dependencies = @(@{ package = 'python'; version = 'system'; params = @{} })
}
Set-Placement $WEB @($A)
forge services deploy web --environment s01 --wait --yes
```

Nginx is on the same machine, so the app binds to `127.0.0.1` only and is not
reachable from the network directly.

## 4. Upload the site ✅

```powershell
forge nodes files roots lab-a          # after the fixes an app root for s01/web exists automatically (⚠ F-05)
# today: approve it once
J @{ kind='service'; label='s01-web'; path='/srv/forge/apps/s01/web'; read_only=$false; service_id=$WEB } root.json | Out-Null
$R = (forge -o json api call files.add_root -p id=$A --body '@root.json' --yes | ConvertFrom-Json).result.id

forge nodes files upload  lab-a .\site.zip /site.zip --root $R
forge nodes files extract lab-a /site.zip /site --root $R
forge nodes exec lab-a -- curl -sI http://127.0.0.1:8081/   # 200
```

## 5. Site on the same node ✅

```powershell
J @{
  name = 's01-web'; domains = @('s01.lab.local'); upstreams = @('127.0.0.1:8081')
  balance = 'round_robin'; tls = 'off'; edges = @($A)
} site.json | Out-Null
forge sites preview --file site.json
forge sites create  --file site.json
forge sites apply   s01-web
```

## 6. Verify

```powershell
curl.exe -s -o NUL -w "%{http_code}`n" -H "Host: s01.lab.local" http://10.50.0.10/   # 200
curl.exe -s -o NUL -w "%{http_code}`n" http://10.50.0.10:8080/                       # Forge console still up
```

- [ ] Site returns 200
- [ ] Forge console still reachable on :8080
- [ ] Site visible in the UI under **Ingress** (⚠ F-11: workspace-level page only)
- [ ] `forge services show web` shows the instance on lab-a as `deployed`/`active`
- [ ] Restart the Forge server (`forge nodes exec lab-a -- sudo systemctl restart forge-server`): the site keeps serving

## 7. Clean up

```powershell
forge sites remove s01-web --yes; forge sites delete s01-web --yes
J @{ action = 'remove' } rm.json | Out-Null
forge api call services.action -p id=$WEB -p server_id=$A --body '@rm.json' --yes
forge services delete web --environment s01 --yes
```
