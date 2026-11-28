# 00 · Common setup

Run this once per PowerShell window before any scenario.

## Prerequisites

- The lab from `../MOP-fresh-lab.md`: the server is on lab-a, and lab-a, lab-b
  and lab-c are enrolled in cluster `lab` and **Online**.
- `forge.exe` is built (`cargo build --release -p forge-cli`) and on your PATH.

## 1. Sign in

```powershell
$env:Path = "<repo-root>\src\forge\target\release;$env:Path"
$env:FORGE_ALLOW_HTTP = "1"                      # lab only: plain http
forge contexts add lab --server http://10.50.0.10:3880   # once; "exists" is fine
forge contexts use lab
forge auth login                                 # admin password: ..\.lab\forge-admin.env
forge auth whoami
```

Exit code `5` on a command means MFA step-up: run `forge auth login` again.

## 2. Variables used by every scenario

```powershell
New-Item -ItemType Directory -Force C:\forge-test | Out-Null; cd C:\forge-test
$nodes = (forge -o json nodes list | ConvertFrom-Json).result
$A = ($nodes | ? hostname -eq 'lab-a').id
$B = ($nodes | ? hostname -eq 'lab-b').id
$C = ($nodes | ? hostname -eq 'lab-c').id
$LAB = ((forge -o json clusters list | ConvertFrom-Json).result | ? name -eq 'lab').id
"A=$A`nB=$B`nC=$C`nLAB=$LAB"
```

Each scenario creates its own environment, `sNN` (for example `s01`), so
scenarios don't collide:

```powershell
function New-Ns($name) {
  forge projects create lab --description "Lab scenarios" 2>$null
  forge environments create $name --cluster lab --project lab 2>$null
  ((forge -o json environments list --all | ConvertFrom-Json).result | ? name -eq $name).id
}
```

## 3. Helpers

```powershell
# Write JSON to a file and return the path
# (-InputObject keeps one-element arrays as arrays; piping would unwrap them)
function J($obj, $file) { ConvertTo-Json -InputObject $obj -Depth 10 | Set-Content $file; $file }

# Create an app from a hashtable, return its id
function New-App($spec) {
  J $spec app.json | Out-Null
  (forge -o json api call services.create --body '@app.json' --yes | ConvertFrom-Json).result.id
}

# Place an app on nodes (ids)
function Set-Placement($appId, [string[]]$serverIds) {
  J @($serverIds | % { @{ server_id = $_; role = 'active' } }) place.json | Out-Null
  forge api call services.place -p id=$appId --body '@place.json' --yes
}

# Install an App Store package on nodes
function Install-Pkg($pkg, $ver, $params, [string[]]$serverIds) {
  J @{ package = $pkg; version = $ver; params = $params; server_ids = $serverIds } pkg.json | Out-Null
  forge packages install --file pkg.json
}
```

### With a build that includes the fixes (recommended)

The helpers above work on any build. With a new release, prefer the typed
commands:

| Instead of | Use |
| --- | --- |
| `New-App` (`services.create`) | `forge services create --file app.json` |
| `Set-Placement` | `forge services place <app> --node lab-b --node lab-c` |
| `services.action … logs` | `forge services logs <app> --node lab-c --tail 200 [--follow]` |
| `services.action … remove` | `forge services remove <app> --node lab-c` |
| `files.add_root` + root-id lookups | Nothing: the app root is created on place. Then `forge services files <app> upload .\site.zip /site.zip` and `forge services files <app> extract /site.zip /site` act on every node of the app |
| `rm /site` before re-extracting | `forge nodes files extract … --replace` |
| `mkdir` of each parent | `forge nodes files mkdir … --parents` |

Check exact flags with `forge services --help` and `forge services files --help`.

## 4. Reset between scenarios

Each scenario ends with its own **Clean up** section. To check that nothing
is left over:

```powershell
forge services list; forge stacks list; forge sites list; forge packages installed
```
