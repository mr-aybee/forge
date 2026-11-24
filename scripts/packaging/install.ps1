<#
.SYNOPSIS
  Installs (or upgrades) the Orbyn Forge control plane on Windows from this package.

.EXAMPLE
  .\install.ps1

.EXAMPLE
  .\install.ps1 -PublicUrl https://forge.example.com

  The console answers on any name or IP of this machine (port 3880) and nodes
  use the address they reach it on. -PublicUrl optionally pins the address
  links use.

.DESCRIPTION
  Binaries go to "C:\Program Files\Orbyn Forge", configuration and data to
  "C:\ProgramData\Orbyn Forge". Forge runs at startup as SYSTEM (scheduled
  task "Orbyn Forge", restarted on failure). Re-running replaces the
  binaries and keeps configuration and data. Nothing else needs to be
  installed. Run in PowerShell as Administrator.

  On a first install the administrator (-AdminUsername, default "admin")
  gets a generated password. It is printed once and saved to
  "C:\ProgramData\Orbyn Forge\data\initial-admin-password" (SYSTEM and
  Administrators only); Forge deletes that file when the administrator
  changes the password. Set FORGE_ADMIN_PASSWORD to choose the password
  instead (nothing is saved then). -AdminEmail is optional: an email can be
  added later in the console.

  Databases: SQLite in the data folder by default. -DbUrl uses PostgreSQL
  (server address without database and password, e.g.
  postgres://forge@db1:5432; password in FORGE_DATABASE_PASSWORD) with
  -DbName (default forge_op; created when missing, reused when it exists).
  -Analytics off|clickhouse|duckdb chooses the analytics database (the
  embedded engine is Linux only; DuckDB on Windows is not yet verified, so
  the default stays off here); -AnalyticsUrl and FORGE_ANALYTICS_PASSWORD
  point at its server.

  -Uninstall stops and removes the startup task and the binaries and keeps
  configuration and data; add -Purge to delete "C:\ProgramData\Orbyn Forge"
  too. Before changing anything, the installer checks the agent binaries
  against dist\SHA256SUMS; get-forge.ps1 checks the package's signature
  before it runs this script.
#>
param(
  [string]$PublicUrl = "",
  [string]$AdminUsername = "admin",
  [string]$AdminEmail = "",
  [string]$AgentHost = "",
  [string]$InstallDir = "$env:ProgramFiles\Orbyn Forge",
  [string]$DataRoot = "$env:ProgramData\Orbyn Forge",
  [string]$DbUrl = "",
  [string]$DbName = "forge_op",
  [switch]$AllowSharedDb,
  [ValidateSet("off", "clickhouse", "duckdb")]
  [string]$Analytics = "off",
  [string]$AnalyticsUrl = "",
  [string]$AnalyticsName = "",
  [switch]$Uninstall,
  [switch]$Purge
)
$ErrorActionPreference = 'Stop'

# ------------------------------------------------------------------ output
function Step([string]$label, [string]$detail) {
  Write-Host "  " -NoNewline; Write-Host ([char]0x2714) -ForegroundColor Green -NoNewline
  Write-Host (" {0,-22} " -f $label) -NoNewline; Write-Host $detail -ForegroundColor DarkGray
}
function Section([string]$title) { Write-Host ""; Write-Host "  $title" -ForegroundColor White }
function Row([string]$label, [string]$value, [string]$hint = "") {
  Write-Host ("    {0,-13} " -f $label) -NoNewline; Write-Host $value -NoNewline
  if ($hint) { Write-Host "  $hint" -ForegroundColor DarkGray } else { Write-Host "" }
}
function Fail([string]$message) {
  Write-Host ""; Write-Host "  $([char]0x2716) $message" -ForegroundColor Red; Write-Host ""; exit 1
}

# ------------------------------------------------------------------ checks
$principal = [Security.Principal.WindowsPrincipal][Security.Principal.WindowsIdentity]::GetCurrent()
if (-not $principal.IsInRole([Security.Principal.WindowsBuiltInRole]::Administrator)) { Fail "Run PowerShell as Administrator." }
$task = "Orbyn Forge"

if ($Uninstall) {
  if (Get-ScheduledTask -TaskName $task -ErrorAction SilentlyContinue) {
    Stop-ScheduledTask -TaskName $task -ErrorAction SilentlyContinue
    Unregister-ScheduledTask -TaskName $task -Confirm:$false
  }
  Get-Process forge-server, forge-analytics-worker, forge-plugin-runner -ErrorAction SilentlyContinue | Stop-Process -Force -ErrorAction SilentlyContinue
  Start-Sleep -Seconds 1
  Step "Startup task" "'$task' removed"
  if (Test-Path $InstallDir) { Remove-Item -Recurse -Force $InstallDir }
  Step "Binaries" "$InstallDir removed"
  if ($Purge) {
    if (Test-Path $DataRoot) { Remove-Item -Recurse -Force $DataRoot }
    Step "Configuration, data" "$DataRoot removed"
  } else {
    Step "Kept" "$DataRoot (configuration and data; -Purge deletes it)"
  }
  Get-NetFirewallRule -DisplayName "Orbyn Forge *" -ErrorAction SilentlyContinue | Remove-NetFirewallRule
  Step "Firewall" "rules 'Orbyn Forge <port>' removed"
  Write-Host ""
  exit 0
}
if ($Purge) { Fail "-Purge goes with -Uninstall." }

$here = Split-Path -Parent $MyInvocation.MyCommand.Path
if (-not (Test-Path "$here\forge-server.exe")) { Fail "Run this from the extracted package (forge-server.exe not found)." }
# The package must be whole before anything changes.
foreach ($f in "forge-server.exe", "forge-plugin-runner.exe", "forge-analytics-worker.exe", "dist\SHA256SUMS", "dist\MANIFEST.json") {
  if (-not (Test-Path (Join-Path $here $f))) { Fail "The package is incomplete: $f is missing. Download it again." }
}
foreach ($line in Get-Content (Join-Path $here "dist\SHA256SUMS")) {
  $parts = $line -split '\s+'
  if ($parts.Count -lt 2) { continue }
  $f = Join-Path (Join-Path $here "dist") ($parts[1] -replace '^\*', '')
  if (-not (Test-Path $f) -or (Get-FileHash -Algorithm SHA256 $f).Hash.ToLowerInvariant() -ne $parts[0].ToLowerInvariant()) {
    Fail "The agent binaries in $here\dist do not match dist\SHA256SUMS. Download the package again."
  }
}
$newVersion = (& "$here\forge-server.exe" --version).Split(' ')[1]
$oldVersion = if (Test-Path "$InstallDir\forge-server.exe") { (& "$InstallDir\forge-server.exe" --version).Split(' ')[1] } else { "" }

$hostName = [System.Net.Dns]::GetHostEntry($env:COMPUTERNAME).HostName
# -PublicUrl is optional: without it nothing is pinned and the server uses the
# address each request came in on (D-167).
$PublicUrl = $PublicUrl.Trim().TrimEnd('/')
if ($PublicUrl) {
  if ($PublicUrl -notmatch '^https?://') { $PublicUrl = "http://$PublicUrl" }
  $u = $null
  if (-not [Uri]::TryCreate($PublicUrl, [UriKind]::Absolute, [ref]$u) -or -not $u.Host -or $u.Host -in @("0.0.0.0", "[::]", "::")) {
    Fail "-PublicUrl $PublicUrl is not a URL people and nodes can open. Use http(s)://<name or IP>[:port] (not 0.0.0.0, which is a listen address)."
  }
  if (-not $AgentHost) { $AgentHost = $u.Host }
}
# cookie_secure only for a pinned https:// address; otherwise Secure follows
# the request (HTTPS reported by a trusted proxy).
$secure = if ($PublicUrl.StartsWith("https://")) { "true" } else { "false" }
$data = Join-Path $DataRoot "data"
$conf = Join-Path $DataRoot "forge.toml"
$dot = [char]0x00B7

$mode = if ($oldVersion) { "Upgrade $oldVersion $([char]0x2192) $newVersion" } else { "New installation" }
Write-Host ""
Write-Host "  $([char]0x256D)$([char]0x2500) " -ForegroundColor Cyan -NoNewline; Write-Host "Orbyn Forge $dot control plane installer" -ForegroundColor White
Write-Host "  $([char]0x2502)  " -ForegroundColor Cyan -NoNewline; Write-Host "$mode  $dot  version $newVersion  $dot  host $hostName" -ForegroundColor DarkGray
Write-Host ("  " + [char]0x2570 + ([string][char]0x2500 * 60)) -ForegroundColor Cyan
Write-Host ""

# ------------------------------------------------------------------ install
# Stop a running instance so its files can be replaced.
if (Get-ScheduledTask -TaskName $task -ErrorAction SilentlyContinue) { Stop-ScheduledTask -TaskName $task -ErrorAction SilentlyContinue }
Get-Process forge-server, forge-analytics-worker, forge-plugin-runner -ErrorAction SilentlyContinue | Stop-Process -Force -ErrorAction SilentlyContinue
Start-Sleep -Seconds 1

New-Item -ItemType Directory -Force -Path $InstallDir, "$InstallDir\dist", $data | Out-Null
# Data and configuration: SYSTEM and Administrators only.
icacls $DataRoot /inheritance:r /grant:r "SYSTEM:(OI)(CI)F" "Administrators:(OI)(CI)F" | Out-Null
Step "Directories" "$InstallDir $dot $DataRoot"

Copy-Item -Force "$here\forge-server.exe", "$here\forge-plugin-runner.exe", "$here\forge-analytics-worker.exe" $InstallDir
Copy-Item -Force "$here\dist\*" "$InstallDir\dist"
$agents = @(Get-ChildItem "$InstallDir\dist" -Filter "forge-agent*").Count
Step "Binaries" "server, plugin runner, analytics worker, $agents agent binaries"

if (-not (Test-Path $conf)) {
  $dataToml = $data.Replace('\', '/')
  $distToml = "$InstallDir\dist".Replace('\', '/')
  $pinLines = ""; $agentNames = ""
  if ($PublicUrl) {
    $pinLines = "public_url = `"$PublicUrl`"`nagent_public_url = `"https://${AgentHost}:3843`"`n"
    if ($AgentHost -ne $hostName) { $agentNames = "`"$AgentHost`", " }
  }
  @"
# Orbyn Forge control plane (written by install.ps1; see forge.toml.example).
data_dir = "$dataToml"
api_listen = "0.0.0.0:3880"
${pinLines}cookie_secure = $secure
agent_listen = "0.0.0.0:3843"
agent_hostnames = [${agentNames}"$hostName", "localhost", "127.0.0.1"]
agent_dist_dir = "$distToml"

[update]
enabled = true
"@ | Set-Content -Encoding UTF8 -Path $conf
  foreach ($u in @($DbUrl, $AnalyticsUrl)) {
    if ($u -match '://[^/@]*:[^/@]*@') { Fail "Put database passwords in FORGE_DATABASE_PASSWORD or FORGE_ANALYTICS_PASSWORD, not in the URL." }
  }
  foreach ($n in @($DbName, $AnalyticsName)) {
    if ($n -and $n -notmatch '^[A-Za-z0-9_]+$') { Fail "Database names use only letters, digits and _." }
  }
  # Password files: SYSTEM and Administrators only, like the data folder.
  function Write-SecretFile([string]$path, [string]$value) {
    Set-Content -Encoding ASCII -NoNewline -Path $path -Value $value
    icacls $path /inheritance:r /grant:r "SYSTEM:F" "Administrators:F" | Out-Null
  }
  $db = "`n[database]`n"
  if ($DbUrl) {
    $db += "url = `"$DbUrl`"`nname = `"$DbName`"`n"
    if ($env:FORGE_DATABASE_PASSWORD) {
      $f = Join-Path $DataRoot "db-password"
      Write-SecretFile $f $env:FORGE_DATABASE_PASSWORD
      $db += "password_file = `"$($f.Replace('\', '/'))`"`n"
    }
  }
  $db += "`n[analytics]`nengine = `"$Analytics`"`n"
  # Written out: the code default stays 55132 so installs whose forge.toml
  # never named the port keep it on upgrade.
  $db += "worker_agent_listen = `"0.0.0.0:3832`"`n"
  if ($AnalyticsUrl) { $db += "url = `"$AnalyticsUrl`"`n" }
  if ($AnalyticsName) { $db += "name = `"$AnalyticsName`"`n" }
  if ($env:FORGE_ANALYTICS_PASSWORD) {
    $f = Join-Path $DataRoot "analytics-password"
    Write-SecretFile $f $env:FORGE_ANALYTICS_PASSWORD
    $db += "password_file = `"$($f.Replace('\', '/'))`"`n"
  }
  Add-Content -Encoding UTF8 -Path $conf -Value $db
  Step "Configuration" "$conf (new)"
  if ($DbUrl) {
    $checkArgs = @("--config", $conf, "db", "check", "--create")
    if ($AllowSharedDb) { $checkArgs += "--allow-shared" }
    $out = & "$InstallDir\forge-server.exe" @checkArgs 2>&1
    if ($LASTEXITCODE -ne 0) { $out | ForEach-Object { Write-Host "  $_" }; Fail "The database cannot be used (see above). Fix it and re-run the installer." }
    Step "Database" (($out | Select-String '^contents: ') -replace '^contents: ', '')
  }
} else { Step "Configuration" "$conf (kept)" }

# ------------------------------------------------------------------ administrator
$adminCreated = $false
$generated = $false
$passwordFile = Join-Path $data "initial-admin-password"
$dbConfigured = Select-String -Path $conf -Pattern '^(database_url|url\s*=\s*"postgres)' -Quiet
$newDb = -not (Test-Path (Join-Path $data "forge.db")) -and -not $dbConfigured
# A PostgreSQL database checked above: new unless it already holds Forge.
if ($DbUrl -and $out -and -not ($out -match 'contents: Forge database')) { $newDb = $true }
if ($newDb) {
  $pw = $env:FORGE_ADMIN_PASSWORD
  if ($pw) {
    if ($pw.Length -lt 12) { Fail "FORGE_ADMIN_PASSWORD must be at least 12 characters." }
  } else {
    # 24 characters from an unambiguous alphabet (no 0/O, 1/l/I): about 137 bits.
    $alphabet = "ABCDEFGHJKLMNPQRSTUVWXYZabcdefghijkmnopqrstuvwxyz23456789"
    # Rejection sampling over CSPRNG bytes (works in Windows PowerShell 5.1 too).
    $rng = [Security.Cryptography.RandomNumberGenerator]::Create()
    $limit = 256 - (256 % $alphabet.Length)
    $chars = New-Object System.Collections.Generic.List[char]
    $buf = New-Object byte[] 64
    while ($chars.Count -lt 24) {
      $rng.GetBytes($buf)
      foreach ($b in $buf) { if ($b -lt $limit -and $chars.Count -lt 24) { $chars.Add($alphabet[$b % $alphabet.Length]) } }
    }
    $rng.Dispose()
    $pw = -join $chars
    $generated = $true
    @(
      "# Orbyn Forge initial administrator. Forge deletes this file when the",
      "# administrator changes the password (console: Profile > Password & security).",
      "username: $AdminUsername",
      "password: $pw"
    ) | Set-Content -Encoding UTF8 -Path $passwordFile
  }
  $initArgs = @("--config", $conf, "init", "--username", $AdminUsername)
  if ($AdminEmail) { $initArgs += @("--email", $AdminEmail) }
  if ($generated) { $initArgs += "--generated-password" }
  # `init` validates everything before it creates the database. If it still
  # fails, remove what this run created so a re-run starts clean.
  $env:FORGE_ADMIN_PASSWORD = $pw
  $out = & "$InstallDir\forge-server.exe" @initArgs 2>&1
  $initExit = $LASTEXITCODE
  Remove-Item Env:FORGE_ADMIN_PASSWORD
  if ($initExit -ne 0) {
    Remove-Item -Force -ErrorAction SilentlyContinue (Join-Path $data "forge.db"), (Join-Path $data "forge.db-wal"), (Join-Path $data "forge.db-shm"), $passwordFile
    $out | ForEach-Object { Write-Host "      $_" -ForegroundColor DarkGray }
    Fail "Could not create the administrator. Nothing was kept; fix the problem and run the installer again."
  }
  $adminCreated = $true
  $who = if ($AdminEmail) { "$AdminUsername <$AdminEmail>" } else { $AdminUsername }
  Step "Administrator" "$who (platform admin)"
} else { Step "Administrator" "existing accounts kept" }

# ------------------------------------------------------------------ service
$action = New-ScheduledTaskAction -Execute "$InstallDir\forge-server.exe" -Argument "--config `"$conf`" serve" -WorkingDirectory $InstallDir
$trigger = New-ScheduledTaskTrigger -AtStartup
$settings = New-ScheduledTaskSettingsSet -AllowStartIfOnBatteries -DontStopIfGoingOnBatteries -ExecutionTimeLimit ([TimeSpan]::Zero) `
  -RestartCount 999 -RestartInterval (New-TimeSpan -Minutes 1) -StartWhenAvailable
$sys = New-ScheduledTaskPrincipal -UserId "SYSTEM" -LogonType ServiceAccount -RunLevel Highest
Register-ScheduledTask -TaskName $task -Action $action -Trigger $trigger -Settings $settings -Principal $sys -Force | Out-Null
Start-ScheduledTask -TaskName $task
Step "Startup task" "'$task' (SYSTEM, at boot, restarts on failure)"

# Ports from the configuration (a re-run keeps the operator's ports).
function Conf-Port([string]$table, [string]$key, [int]$default) {
  $cur = ""
  foreach ($line in Get-Content $conf) {
    if ($line -match '^\s*\[(.+)\]') { $cur = $Matches[1].Trim(); continue }
    if ($cur -eq $table -and $line -match ('^\s*' + $key + '\s*=\s*"[^"]*:(\d+)"')) { return [int]$Matches[1] }
  }
  return $default
}
$apiPort = Conf-Port "" "api_listen" 3880
$agentPort = Conf-Port "" "agent_listen" 3843
$workerPort = Conf-Port "analytics" "worker_agent_listen" 55132  # code default when forge.toml names none
foreach ($p in $apiPort, $agentPort, $workerPort) {
  $name = "Orbyn Forge $p"
  if (-not (Get-NetFirewallRule -DisplayName $name -ErrorAction SilentlyContinue)) {
    New-NetFirewallRule -DisplayName $name -Direction Inbound -Protocol TCP -LocalPort $p -Action Allow | Out-Null
  }
}
Step "Firewall" "inbound TCP $apiPort (console), $agentPort (agents) and $workerPort (analytics uploads) allowed"

$ok = $false
for ($i = 0; $i -lt 60; $i++) {
  try { Invoke-WebRequest -UseBasicParsing -TimeoutSec 2 "http://127.0.0.1:$apiPort/api/health" | Out-Null; $ok = $true; break } catch { Start-Sleep 1 }
}
if (-not $ok) { Fail "forge-server did not start. Run it by hand to see why: & '$InstallDir\forge-server.exe' --config '$conf' serve" }
Step "Health check" "responding after ${i}s"

# ------------------------------------------------------------------ summary
# Report what the configuration says (a re-run keeps the existing file).
$cfg = Get-Content $conf
function ConfValue($key) {
  $m = $cfg | Select-String "^$key = `"(.*)`"" | Select-Object -First 1
  if ($m) { $m.Matches[0].Groups[1].Value } else { "" }
}
$PublicUrl = ConfValue "public_url"
$agentUrl = ConfValue "agent_public_url"
$secure = if ($PublicUrl.StartsWith("https://")) { "true" } else { "false" }

Write-Host ""
if ($oldVersion) { Write-Host "  Orbyn Forge upgraded to $newVersion and running." -ForegroundColor Green }
else { Write-Host "  Orbyn Forge $newVersion is installed and running." -ForegroundColor Green }

Section "ACCESS"
if ($PublicUrl) {
  Row "Console" $PublicUrl "(pinned)"
  $firstUrl = $PublicUrl
} else {
  $ips = @(Get-NetIPAddress -AddressFamily IPv4 -ErrorAction SilentlyContinue |
    Where-Object { $_.IPAddress -notlike "127.*" -and $_.IPAddress -notlike "169.254.*" } | ForEach-Object { $_.IPAddress })
  foreach ($ip in $ips) { Row "Console" "http://${ip}:3880" }
  Row "Console" "http://${hostName}:3880"
  $firstUrl = if ($ips.Count) { "http://$($ips[0]):3880" } else { "http://${hostName}:3880" }
  Write-Host "    $dot Any name or IP of this machine works; pin one later in Settings or with -PublicUrl." -ForegroundColor DarkGray
}
if ($agentUrl) { Row "Agents" $agentUrl "(nodes connect to TCP $agentPort and $workerPort)" }
else { Row "Agents" "nodes connect to TCP $agentPort and $workerPort on the address they use" }

Section "RESTRICT ACCESS"
Write-Host "    $dot Every interface listens. Narrow who may connect, for example:" -ForegroundColor DarkGray
Write-Host "      New-NetFirewallRule -DisplayName 'Forge agents' -Direction Inbound -Protocol TCP -LocalPort $agentPort -RemoteAddress <node subnet> -Action Allow"
Write-Host "      [security.access] allow = [`"10.0.0.0/8`"] in $conf"

if ($adminCreated) {
  Section "ADMINISTRATOR"
  Row "Username" $AdminUsername
  if ($AdminEmail) { Row "Email" $AdminEmail }
  if ($generated) {
    Write-Host ("    {0,-13} " -f "Password") -NoNewline; Write-Host $pw -ForegroundColor Yellow
    Row "Saved to" $passwordFile "(Administrators only, deleted when you change it)"
  } else { Row "Password" "the value of FORGE_ADMIN_PASSWORD" }
  Write-Host "    $dot Sign in, then in Profile add your email, change the password and set up MFA." -ForegroundColor DarkGray
}

Section "INSTALLED"
Row "Binaries" $InstallDir
Row "Agents" "$InstallDir\dist"
Row "Config" $conf
Row "Data" $data "(database, keys, internal CA, backups)"
Row "Startup" "scheduled task '$task'" "(SYSTEM)"

Section "OPERATE"
Row "Status" "Get-ScheduledTask -TaskName '$task' | Get-ScheduledTaskInfo"
Row "Restart" "Stop-ScheduledTask '$task'; Start-ScheduledTask '$task'"
Row "Uninstall" ".\install.ps1 -Uninstall" "(from the package folder; -Purge also deletes data)"

Section "NEXT STEPS"
Write-Host "    1. Open $firstUrl and sign in$(if ($adminCreated) { " as $AdminUsername" })."
if ($generated) { Write-Host "       The password is also in $passwordFile." }
Write-Host "    2. Allow TCP $agentPort and $workerPort from your nodes to this server."
Write-Host "    3. Add nodes: Overview > Add node."
if ($secure -eq "false") {
  Write-Host ""
  Write-Host "  $([char]0x25B2) " -ForegroundColor Yellow -NoNewline
  Write-Host ("{0,-22} " -f "Production") -NoNewline
  Write-Host "Put a TLS proxy (Caddy, Nginx, IIS) in front and pin its https:// address."
}
Write-Host ""
$pw = $null
