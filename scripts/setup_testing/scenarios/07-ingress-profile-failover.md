# 07 · Cluster ingress profile with two edges

**Who:** a production setup where the edge itself must not be a single point
of failure. Sites are linked to the cluster's ingress profile instead of
listing edges by hand.

```text
             ┌─ lab-a :80 Nginx (edge 1) ─┐
browser ─────┤                            ├──► lab-c :8080 web
             └─ lab-b :80 Nginx (edge 2) ─┘
```

| Cluster mapping | |
| --- | --- |
| Cluster | `lab`, with an ingress profile: **Active · active**, edges lab-a and lab-b |
| lab-c | worker |
| Environment | `s07` |

Prerequisite: [00-common-setup](00-common-setup.md). No DNS server is
needed: the test uses `Host:` headers against each edge IP.

## 1. Nginx on both edges ✅

```powershell
$NS = New-Ns 's07'
Install-Pkg 'nginx' 'stable' @{} @($A, $B)
```

## 2. Ingress profile 🟡 (UI only, no typed CLI)

In the UI, go to **Clusters → lab → Ingress → Profile**:
1. **Layout:** Active · active.
2. **Edge nodes:** lab-a (public IP `10.50.0.10`) and lab-b (`10.50.0.11`).
3. **Save & apply** (MFA).

```powershell
forge clusters live lab        # both edges: nginx running
forge api operations --tag clusters   # is there a profile endpoint? Note it for a typed CLI command
```

## 3. App and cluster-linked Site ✅

```powershell
$WEB = New-App @{
  environment_id=$NS; name='web'; runtime='systemd'
  command='python3 -m http.server 8080 --bind 0.0.0.0 --directory /srv/forge/apps/s07/web'
  working_dir=''; listen_port=8080; restart=$true; env=@()
  dependencies=@(@{ package='python'; version='system'; params=@{} })
}
Set-Placement $WEB @($C)
forge services deploy web --environment s07 --wait --yes

J @{ name='s07-web'; domains=@('s07.lab.local'); upstreams=@(); tls='off'; cluster_id=$LAB;
     targets=@(@{ kind='app'; service_id=$WEB; port=8080 }) } site.json | Out-Null
forge sites create --file site.json
forge sites apply  s07-web
```

## 4. Verify

```powershell
foreach ($e in '10.50.0.10','10.50.0.11') { "$e " + (curl.exe -s -o NUL -w "%{http_code}" -H "Host: s07.lab.local" "http://$e/") }
forge clusters routing lab     # s07.lab.local → lab-a, lab-b → web → lab-c
forge nodes exec lab-b -- tail -n 3 /var/log/nginx/forge-traffic.log
```

- [ ] Both edges return 200
- [ ] `forge clusters routing lab` and **Cluster → lab → Ingress** list the site
- [ ] **Watch traffic** in the UI shows requests from both edges
- [ ] **Edge fail-over:** stop Nginx on lab-b (`forge packages action <nginx-inst-on-lab-b> stop --yes`). The profile's health check takes lab-b out after about 45 s (UI **Health** tab, audit entry). lab-a still serves. Start it again; lab-b comes back
- [ ] Change the profile's Nginx tuning (for example the body size), **Save & apply**, then `nginx -t` passes on both edges and the site still works
- [ ] Remove lab-b from the profile, then lab-b's Nginx no longer serves `s07.lab.local`

## 5. Clean up

```powershell
forge sites remove s07-web --yes; forge sites delete s07-web --yes
J @{ action = 'remove' } rm.json | Out-Null
forge api call services.action -p id=$WEB -p server_id=$C --body '@rm.json' --yes
forge services delete web --environment s07 --yes
```

Remove lab-b from the ingress profile in the UI, or delete the profile if no
other scenario needs it.
