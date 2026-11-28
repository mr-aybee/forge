# Orbyn Forge recipe runner for Windows (winget). Run by the agent service
# (LocalSystem) with a clean set of FORGE_* variables; the action is $env:FORGE_ACTION.
#
#   FORGE_PKG_ID, FORGE_PKG_VERSION  validated package and release line
#   FORGE_WINGET                     candidate ids, '|'-separated; {line}, {major}, {flavor} expanded
#   FORGE_WINGET_SERVICE             Windows service name ('' for runtimes); {line} expanded
#   FORGE_PKG_BUILD                  exact build to install or upgrade to (validated)
#   FORGE_P_*                        package parameters
#
# Output: progress lines, then the result as the last line (a state such as
# `running`, or JSON for `versions`).
$ErrorActionPreference = 'Stop'
$ProgressPreference = 'SilentlyContinue'
[Console]::OutputEncoding = [System.Text.Encoding]::UTF8

$Id = $env:FORGE_PKG_ID
$Line = $env:FORGE_PKG_VERSION
$Build = $env:FORGE_PKG_BUILD
$Action = $env:FORGE_ACTION
$Major = ($Line -split '\.')[0]
$Flavor = switch ($env:FORGE_P_FLAVOR) { 'sdk' { 'SDK' } 'runtime' { 'Runtime' } default { 'AspNetCore' } }
# Builds on this release line start with "<line>." (or "<line>-" for EDB's PostgreSQL builds).
$Prefixes = @("$Line.", "$Line-")
# The agent's data folder (D-079); never the generic %ProgramData%\Forge.
$PkgEnvDir = Join-Path $env:ProgramData 'OrbynForge\pkgs'
$PkgEnv = Join-Path $PkgEnvDir "$Id.env"
# Written before D-079 under the generic folder: moved on first use.
$LegacyPkgEnv = Join-Path $env:ProgramData "Forge\pkgs\$Id.env"
if ((Test-Path -LiteralPath $LegacyPkgEnv) -and -not (Test-Path -LiteralPath $PkgEnv)) {
  New-Item -ItemType Directory -Force -Path $PkgEnvDir | Out-Null
  Move-Item -LiteralPath $LegacyPkgEnv -Destination $PkgEnv
}
foreach ($d in (Join-Path $env:ProgramData 'Forge\pkgs'), (Join-Path $env:ProgramData 'Forge')) {
  if ((Test-Path -LiteralPath $d) -and -not (Get-ChildItem -LiteralPath $d -Force -ErrorAction SilentlyContinue)) {
    Remove-Item -LiteralPath $d -Force -ErrorAction SilentlyContinue
  }
}

function Log([string]$m) { Write-Output "forge: $m" }
function Die([string]$m) { [Console]::Error.WriteLine("forge: error: $m"); exit 1 }

function Expand([string]$t) { $t.Replace('{line}', $Line).Replace('{major}', $Major).Replace('{flavor}', $Flavor) }

# winget.exe for LocalSystem: the App Installer package path (no user alias).
function Find-Winget {
  $cmd = Get-Command winget.exe -ErrorAction SilentlyContinue
  if ($cmd) { return $cmd.Source }
  $dir = Get-ChildItem "$env:ProgramFiles\WindowsApps" -Directory -Filter 'Microsoft.DesktopAppInstaller_*_x64__8wekyb3d8bbwe' -ErrorAction SilentlyContinue |
    Sort-Object Name -Descending | Select-Object -First 1
  if ($dir -and (Test-Path (Join-Path $dir.FullName 'winget.exe'))) { return (Join-Path $dir.FullName 'winget.exe') }
  Die 'winget is not available on this node (Windows Server 2025 and Windows 11 include it; on Server 2022 install App Installer first)'
}
$Winget = Find-Winget
$Common = @('--accept-source-agreements', '--disable-interactivity')

function WG { param([string[]]$Args2) & $Winget @Args2 @Common 2>&1 | ForEach-Object { "$_" } }

# Versions an id offers on this line, newest first.
function Line-Versions([string]$wid) {
  WG @('show', '--id', $wid, '--exact', '--versions') |
    Where-Object { $v = $_.Trim(); $Prefixes | Where-Object { $v.StartsWith($_) } } |
    ForEach-Object { $_.Trim() }
}

# The first candidate id with builds on this line.
function Resolve-Id {
  foreach ($t in ($env:FORGE_WINGET -split '\|')) {
    $wid = Expand $t
    $vs = @(Line-Versions $wid)
    if ($vs.Count -gt 0) { return @{ Id = $wid; Versions = $vs } }
  }
  Die "no winget package offers $Id $Line"
}

# Installed version of an id ('' if not installed): the Version column of `winget list`.
function Installed([string]$wid) {
  $rows = WG @('list', '--id', $wid, '--exact')
  $head = $rows | Where-Object { $_ -match '^Name\s+Id\s+Version' } | Select-Object -First 1
  if (-not $head) { return '' }
  $vcol = $head.IndexOf('Version')
  $acol = $head.IndexOf('Available'); if ($acol -lt 0) { $acol = $head.IndexOf('Source') }
  $row = $rows | Where-Object { $_ -match [regex]::Escape($wid) } | Select-Object -Last 1
  if (-not $row -or $row.Length -le $vcol) { return '' }
  $end = if ($acol -gt $vcol -and $row.Length -gt $acol) { $acol } else { $row.Length }
  return $row.Substring($vcol, $end - $vcol).Trim()
}

function Service-Name { if ($env:FORGE_WINGET_SERVICE) { Expand $env:FORGE_WINGET_SERVICE } else { '' } }

function Status {
  $svc = Service-Name
  if ($svc) {
    $s = Get-Service -Name $svc -ErrorAction SilentlyContinue
    if (-not $s) { return 'not-installed' }
    if ($s.Status -eq 'Running') { 'running' } else { 'stopped' }
  } else {
    $r = Resolve-Id
    if (Installed $r.Id) { 'running' } else { 'not-installed' }
  }
}

# Package env file: readable by SYSTEM and Administrators only.
function Pkg-Env([string]$k, [string]$v) {
  New-Item -ItemType Directory -Force -Path $PkgEnvDir | Out-Null
  $key = 'FORGE_PKG_' + ($Id.ToUpper() -replace '-', '_') + '_' + $k
  $lines = @(); if (Test-Path $PkgEnv) { $lines = @(Get-Content $PkgEnv | Where-Object { -not $_.StartsWith("$key=") }) }
  $lines += "$key=$v"
  Set-Content -Path $PkgEnv -Value $lines -Encoding UTF8
  # By SID (SYSTEM, Administrators), so it works on any Windows language.
  icacls $PkgEnv /inheritance:r /grant:r '*S-1-5-18:F' '*S-1-5-32-544:F' | Out-Null
}
function Pkg-Env-Get([string]$k) {
  if (-not (Test-Path $PkgEnv)) { return '' }
  $key = 'FORGE_PKG_' + ($Id.ToUpper() -replace '-', '_') + '_' + $k
  $l = Get-Content $PkgEnv | Where-Object { $_.StartsWith("$key=") } | Select-Object -Last 1
  if ($l) { $l.Substring($key.Length + 1) } else { '' }
}
function New-Secret {
  $b = New-Object byte[] 24; [Security.Cryptography.RandomNumberGenerator]::Create().GetBytes($b)
  ([Convert]::ToBase64String($b) -replace '[/+=]', '').Substring(0, 24)
}

# A temporary installer option file (secrets) removed after winget runs.
$script:OptFile = $null

# Package-specific installer arguments (silent, with Forge's settings).
# Secrets never go on the command line, which every process list shows:
# they go into an option file readable by SYSTEM and Administrators only.
function Override([string]$wid) {
  switch ($Id) {
    'postgresql' {
      $pw = Pkg-Env-Get 'PASSWORD'; if (-not $pw) { $pw = New-Secret; Pkg-Env 'PASSWORD' $pw }
      $script:OptFile = Join-Path ([IO.Path]::GetTempPath()) ('forge-pg-' + [guid]::NewGuid().ToString('N') + '.ini')
      # Created and locked down first, written second.
      New-Item -ItemType File -Force -Path $script:OptFile | Out-Null
      icacls $script:OptFile /inheritance:r /grant:r '*S-1-5-18:F' '*S-1-5-32-544:F' | Out-Null
      if ($LASTEXITCODE -ne 0) { Die 'could not protect the installer option file' }
      Set-Content -Path $script:OptFile -Value "superpassword=$pw" -Encoding ASCII
      return "--mode unattended --unattendedmodeui none --optionfile `"$($script:OptFile)`" --serverport $($env:FORGE_P_PORT) --disable-components stackbuilder"
    }
    default { return '' }
  }
}

function After-Install {
  switch ($Id) {
    'postgresql' {
      $data = Join-Path $env:ProgramFiles "PostgreSQL\$Line\data"
      $conf = Join-Path $data 'postgresql.conf'
      if (Test-Path $conf) {
        $addr = if ($env:FORGE_P_BIND -eq 'all') { '*' } else { 'localhost' }
        (Get-Content $conf) -replace "^#?listen_addresses = '[^']*'", "listen_addresses = '$addr'" | Set-Content $conf -Encoding UTF8
        if ($env:FORGE_P_BIND -eq 'all' -and -not (Select-String -Path (Join-Path $data 'pg_hba.conf') -Pattern 'forge-managed' -Quiet)) {
          Add-Content (Join-Path $data 'pg_hba.conf') "# forge-managed`r`nhost all all 0.0.0.0/0 scram-sha-256`r`nhost all all ::/0 scram-sha-256"
        }
        Restart-Service -Name (Service-Name)
      }
      $rule = "Forge PostgreSQL $Line"
      Get-NetFirewallRule -DisplayName $rule -ErrorAction SilentlyContinue | Remove-NetFirewallRule
      if ($env:FORGE_P_BIND -eq 'all') {
        New-NetFirewallRule -DisplayName $rule -Direction Inbound -Protocol TCP -LocalPort $env:FORGE_P_PORT -Action Allow | Out-Null
      }
      Pkg-Env 'HOST' '127.0.0.1'; Pkg-Env 'PORT' $env:FORGE_P_PORT; Pkg-Env 'USER' 'postgres'
      Pkg-Env 'BIN' (Join-Path $env:ProgramFiles "PostgreSQL\$Line\bin")
    }
    'nodejs' { Pkg-Env 'BIN' (Join-Path $env:ProgramFiles 'nodejs\node.exe') }
    'java' {
      $jdk = Get-ChildItem (Join-Path $env:ProgramFiles 'Eclipse Adoptium') -Directory -Filter "jdk-$Line*" -ErrorAction SilentlyContinue |
        Sort-Object Name -Descending | Select-Object -First 1
      if ($jdk) { Pkg-Env 'HOME' $jdk.FullName; Pkg-Env 'BIN' (Join-Path $jdk.FullName 'bin\java.exe') }
    }
    'dotnet' { Pkg-Env 'BIN' (Join-Path $env:ProgramFiles 'dotnet\dotnet.exe') }
  }
}

function Running-Version {
  try {
    switch ($Id) {
      'postgresql' { & (Join-Path $env:ProgramFiles "PostgreSQL\$Line\bin\postgres.exe") --version 2>$null | ForEach-Object { ($_ -split ' ')[-1] } }
      'nodejs' { (& (Join-Path $env:ProgramFiles 'nodejs\node.exe') --version 2>$null).TrimStart('v') }
      default { '' }
    }
  } catch { '' }
}

function Json-Str([string]$s) { '"' + (($s -replace '[^A-Za-z0-9.+~:_ ()/-]', '') -replace '^(.{0,120}).*$', '$1') + '"' }

function Versions {
  $r = Resolve-Id
  $inst = Installed $r.Id
  '{"manager":"winget","package":' + (Json-Str $r.Id) + ',"installed":' + (Json-Str $inst) +
    ',"candidate":' + (Json-Str $r.Versions[0]) + ',"running":' + (Json-Str $(if ($inst) { Running-Version } else { '' })) + '}'
}

function Winget-Install([string]$verb) {
  $r = Resolve-Id
  $target = if ($Build) { $Build } else { $r.Versions[0] }
  if ($r.Versions -notcontains $target) { Die "$($r.Id) $target is not offered by winget" }
  $args2 = @($verb, '--id', $r.Id, '--exact', '--version', $target, '--silent', '--accept-package-agreements')
  if ($verb -eq 'install') { $args2 += @('--scope', 'machine') }
  $o = Override $r.Id
  if ($o) { $args2 += @('--override', $o) }
  Log "winget $verb $($r.Id) $target"
  try {
    WG $args2 | ForEach-Object { if ($_.Trim()) { Log $_.Trim() } }
    $code = $LASTEXITCODE
  } finally {
    if ($script:OptFile) { Remove-Item -Force -LiteralPath $script:OptFile -ErrorAction SilentlyContinue; $script:OptFile = $null }
  }
  if ($code -ne 0 -and $code -ne -1978335189) {  # 0x8A15002B: already installed / no newer
    Die "winget $verb failed (exit $code)"
  }
  $now = Installed $r.Id
  if ($now -ne $target) { Die "$($r.Id) is at '$now' after $verb, not $target" }
}

switch ($Action) {
  'install' { Winget-Install 'install'; After-Install; Log "installed $Id $Line"; Status }
  'upgrade' {
    $r = Resolve-Id
    if (-not (Installed $r.Id)) { Die "$($r.Id) is not installed" }
    Winget-Install 'upgrade'
    $svc = Service-Name; if ($svc) { Restart-Service -Name $svc }
    Log "state: $(Status)"
    Versions
  }
  'uninstall' {
    $r = Resolve-Id
    WG @('uninstall', '--id', $r.Id, '--exact', '--silent') | ForEach-Object { if ($_.Trim()) { Log $_.Trim() } }
    Remove-Item -Force $PkgEnv -ErrorAction SilentlyContinue
    Log "uninstalled $Id"; 'not-installed'
  }
  'start' { $s = Service-Name; if ($s) { Start-Service -Name $s }; Status }
  'stop' { $s = Service-Name; if ($s) { Stop-Service -Name $s }; Status }
  'restart' { $s = Service-Name; if ($s) { Restart-Service -Name $s }; Status }
  'status' { Status }
  'versions' { Versions }
  default { Die "unknown action $Action" }
}
