# 05 · One node, many runtimes, one domain each

**Who:** an agency that hosts several small customer apps, written in
different languages, on one worker.

```text
lab-a :80 Nginx ─┬─ py.s05.lab.local  ──► lab-c :8001  Python
                 ├─ net.s05.lab.local ──► lab-c :8002  .NET 8
                 └─ php.s05.lab.local ──► lab-c :8003  PHP 8.3
```

| Cluster mapping | |
| --- | --- |
| Cluster | `lab` |
| lab-a | edge |
| lab-c | worker with Python, .NET and PHP |
| Environment | `s05` |

Prerequisite: [00-common-setup](00-common-setup.md). The app code is the
same as in `../MOP-cli-test-run.md` Part 3. Each app runs as its own user.

## 1. Runtimes ✅

```powershell
$NS = New-Ns 's05'
Install-Pkg 'python' 'system' @{} @($C)
Install-Pkg 'dotnet' '8.0' @{ flavor = 'aspnetcore' } @($C)
Install-Pkg 'php' '8.3' @{ extensions = 'mbstring,xml,curl' } @($C)
Install-Pkg 'nginx' 'stable' @{} @($A)
```

## 2. Three apps 🟡 create · ✅ deploy

All three bind `0.0.0.0` so the edge on lab-a can reach them.

```powershell
$apps = @(
  @{ name='py';  port=8001; dep=@{package='python';version='system';params=@{}};
     cmd='python3 -m http.server 8001 --bind 0.0.0.0 --directory /srv/forge/apps/s05/py/site' },
  @{ name='net'; port=8002; dep=@{package='dotnet';version='8.0';params=@{flavor='aspnetcore'}};
     cmd='dotnet /srv/forge/apps/s05/net/out/dot.dll --urls http://0.0.0.0:8002' },
  @{ name='php'; port=8003; dep=@{package='php';version='8.3';params=@{}};
     cmd='php -S 0.0.0.0:8003 -t /srv/forge/apps/s05/php/site' }
)
$ids = @{}
foreach ($a in $apps) {
  $ids[$a.name] = New-App @{ environment_id=$NS; name=$a.name; runtime='systemd'; command=$a.cmd; working_dir='';
                            listen_port=$a.port; restart=$true; env=@(); dependencies=@($a.dep) }
  Set-Placement $ids[$a.name] @($C)
  forge services deploy $a.name --environment s05 --wait --yes
}
```

Upload the code for each app into its app root (`forge nodes files roots lab-c`):
- **py:** `site/index.html`
- **net:** `out/` from `dotnet publish` (see scenario 02)
- **php:** `site/index.php`

Then restart each app with `forge services action <app> restart --node lab-c --yes`.

> If `php` fails with `php: not found`, the php package installed only
> php-fpm. Log it as a finding.

## 3. Three Sites, one per domain ✅

```powershell
foreach ($a in $apps) {
  J @{ name="s05-$($a.name)"; domains=@("$($a.name).s05.lab.local"); upstreams=@(); tls='off'; edges=@($A);
       targets=@(@{ kind='app'; service_id=$ids[$a.name]; port=$a.port }) } "site-$($a.name).json" | Out-Null
  forge sites create --file "site-$($a.name).json"
  forge sites apply "s05-$($a.name)"
}
```

## 4. Verify

```powershell
foreach ($n in 'py','net','php') { "$n " + (curl.exe -s -o NUL -w "%{http_code}" -H "Host: $n.s05.lab.local" http://10.50.0.10/) }
forge nodes exec lab-c -- ps -o user=,cmd= -C python3,dotnet,php     # three different users s05-py, s05-net, s05-php
```

- [ ] All three return 200 on their own domain
- [ ] Each process runs as its own user, never root
- [ ] One app can't read another's folder: `forge nodes exec lab-c -- sudo -u s05-py ls /srv/forge/apps/s05/php`, expect Permission denied
- [ ] Change an env var on `py` (UI **Configuration** or `services.update`) and redeploy; the others are unaffected
- [ ] Memory limit: set `memory_mb: 64` on `py` and redeploy, then check `systemctl show forge-app-s05-py -p MemoryMax`
- [ ] The UI lists 3 apps and 3 Sites; the node page shows the 3 runtimes as installed packages

## 5. Clean up

```powershell
J @{ action = 'remove' } rm.json | Out-Null
foreach ($a in $apps) {
  forge sites remove "s05-$($a.name)" --yes; forge sites delete "s05-$($a.name)" --yes
  forge api call services.action -p id=$($ids[$a.name]) -p server_id=$C --body '@rm.json' --yes
  forge services delete $a.name --environment s05 --yes
}
```
