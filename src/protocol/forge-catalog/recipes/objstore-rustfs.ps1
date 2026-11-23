# RustFS object store instance on Windows (forge_catalog::storage). Runs in
# the agent service (LocalSystem) with validated FORGE_* variables.
# rustfs.exe is not a Windows service binary, so it runs as a SYSTEM
# scheduled task at startup that restarts on failure. Deletes data only on an
# explicit uninstall with delete-data, and only where the ownership marker matches.
$ErrorActionPreference = 'Stop'
$ProgressPreference = 'SilentlyContinue'
[Net.ServicePointManager]::SecurityProtocol = [Net.SecurityProtocolType]::Tls12

function Log($m) { Write-Output "forge: $m" }
function Die($m) { [Console]::Error.WriteLine("forge: error: $m"); exit 1 }

$Slug = $env:FORGE_OS_SLUG
$Rel = $env:FORGE_OS_VERSION
$S3Port = [int]$env:FORGE_OS_S3_PORT
$ConsolePort = $env:FORGE_OS_CONSOLE_PORT
$Data = $env:FORGE_OS_DATA
$Action = $env:FORGE_ACTION
$Unit = "forge-objstore-$Slug"
# The agent's data folder (D-079); never the generic %ProgramData%\Forge.
$Root = Join-Path $env:ProgramData "OrbynForge\objstore"
$Base = Join-Path $Root $Slug
$Runner = Join-Path $Base 'run.cmd'
# Installed before D-079 under the generic folder: kept in use until the
# next install moves it (the install rewrites the runner and the task).
$LegacyRoot = Join-Path $env:ProgramData "Forge\objstore"
$LegacyBase = Join-Path $LegacyRoot $Slug
if ($Action -ne 'install' -and -not (Test-Path -LiteralPath $Base) -and (Test-Path -LiteralPath $LegacyBase)) {
  $Base = $LegacyBase
  $Runner = Join-Path $Base 'run.cmd'
}
$Releases = 'https://github.com/rustfs/rustfs/releases'

if ($env:FORGE_OS_RUNTIME -ne 'native') { Die 'only the native runtime is available on Windows' }

function Lock-Down($path) {
  # Administrators and SYSTEM only: the runner holds the credential.
  icacls $path /inheritance:r /grant:r '*S-1-5-18:(OI)(CI)F' '*S-1-5-32-544:(OI)(CI)F' | Out-Null
}

function Test-Mount($path) {
  # A dedicated volume: the path is a volume root or a mounted folder.
  $item = Get-Item -LiteralPath $path -ErrorAction SilentlyContinue
  if (-not $item) { return $false }
  if ($item.FullName.TrimEnd('\').Length -le 2) { return $true }
  return [bool]($item.Attributes -band [IO.FileAttributes]::ReparsePoint)
}

function Check-Data {
  if ($env:FORGE_OS_REQUIRE_MOUNT -eq '1') {
    if (-not (Test-Path -LiteralPath $Data)) { Die "data path $Data does not exist; attach the volume first" }
    if (-not (Test-Mount $Data)) { Die "data path $Data is not a dedicated volume" }
  }
}

function Get-State {
  $t = Get-ScheduledTask -TaskName $Unit -ErrorAction SilentlyContinue
  if (-not $t) { return 'not-installed' }
  $p = Get-CimInstance Win32_Process -Filter "Name = 'rustfs.exe'" -ErrorAction SilentlyContinue |
    Where-Object { $_.ExecutablePath -and $_.ExecutablePath.StartsWith($Base, [StringComparison]::OrdinalIgnoreCase) }
  if ($p) { 'running' } else { 'stopped' }
}

function Wait-Health($seconds) {
  for ($i = 0; $i -lt $seconds; $i++) {
    try { Invoke-WebRequest -UseBasicParsing -TimeoutSec 3 "http://127.0.0.1:$S3Port/health" | Out-Null; return } catch { Start-Sleep 1 }
  }
  Die "RustFS did not answer on port $S3Port"
}

function Report {
  $state = Get-State
  $ver = ''
  if ($state -eq 'running') {
    try { $ver = (Invoke-RestMethod -TimeoutSec 5 "http://127.0.0.1:$S3Port/health").version } catch { }
  }
  if (-not $ver -and (Test-Path (Join-Path $Base 'release'))) { $ver = (Get-Content (Join-Path $Base 'release') -Raw).Trim() }
  $r = [ordered]@{ state = $state; version = "$ver"; dedicated = $null; capacity_bytes = $null; free_bytes = $null }
  if (Test-Path -LiteralPath $Data) {
    $r.dedicated = Test-Mount $Data
    try {
      $drive = (Get-Item -LiteralPath $Data).PSDrive.Name
      $v = Get-Volume -DriveLetter $drive -ErrorAction Stop
      $r.capacity_bytes = [uint64]$v.Size; $r.free_bytes = [uint64]$v.SizeRemaining
    } catch { }
  }
  Write-Output ("forge: objstore: " + ($r | ConvertTo-Json -Compress))
}

function Install-Binary {
  if ($Rel -eq 'latest') { $url = "$Releases/latest/download"; $suffix = 'latest' } else { $url = "$Releases/download/$Rel"; $suffix = "v$Rel" }
  $asset = "rustfs-windows-x86_64-$suffix.zip"
  $tmp = Join-Path ([IO.Path]::GetTempPath()) ("forge-objstore-" + [guid]::NewGuid().ToString('N'))
  New-Item -ItemType Directory -Path $tmp | Out-Null
  try {
    Log "downloading $asset"
    Invoke-WebRequest -UseBasicParsing "$url/$asset" -OutFile (Join-Path $tmp $asset)
    Invoke-WebRequest -UseBasicParsing "$url/SHA256SUMS" -OutFile (Join-Path $tmp 'SHA256SUMS')
    $line = Get-Content (Join-Path $tmp 'SHA256SUMS') | Where-Object { $_ -match "\s\*?$([regex]::Escape($asset))$" } | Select-Object -First 1
    if (-not $line) { Die "$asset is not listed in SHA256SUMS" }
    $want = ($line -split '\s+')[0].ToLowerInvariant()
    $got = (Get-FileHash -Algorithm SHA256 (Join-Path $tmp $asset)).Hash.ToLowerInvariant()
    if ($want -ne $got) { Die "checksum mismatch for $asset" }
    Log "checksum verified: $($got.Substring(0,16))..."
    Expand-Archive -Path (Join-Path $tmp $asset) -DestinationPath (Join-Path $tmp 'x') -Force
    $bin = Get-ChildItem -Recurse -File -Filter 'rustfs.exe' (Join-Path $tmp 'x') | Select-Object -First 1
    if (-not $bin) { Die "no rustfs.exe in $asset" }
    $ver = ''
    try { $ver = ((& $bin.FullName --version) -join ' ' | Select-String -Pattern '[0-9][0-9A-Za-z.+-]*').Matches[0].Value } catch { }
    if (-not $ver) { $ver = $Rel }
    $dir = Join-Path $Base $ver
    New-Item -ItemType Directory -Force -Path $dir | Out-Null
    Copy-Item -Force $bin.FullName (Join-Path $dir 'rustfs.exe')
    Set-Content -Path (Join-Path $Base 'release') -Value $ver -Encoding ASCII
    Log "installed RustFS $ver in $dir"
    return $dir
  } finally {
    Remove-Item -Recurse -Force $tmp -ErrorAction SilentlyContinue
  }
}

function Stop-Store {
  Stop-ScheduledTask -TaskName $Unit -ErrorAction SilentlyContinue
  Get-CimInstance Win32_Process -Filter "Name = 'rustfs.exe'" -ErrorAction SilentlyContinue |
    Where-Object { $_.ExecutablePath -and $_.ExecutablePath.StartsWith($Base, [StringComparison]::OrdinalIgnoreCase) } |
    ForEach-Object { Stop-Process -Id $_.ProcessId -Force -ErrorAction SilentlyContinue }
}

function Remove-EmptyLegacy {
  foreach ($d in $LegacyRoot, (Join-Path $env:ProgramData 'Forge')) {
    if ((Test-Path -LiteralPath $d) -and -not (Get-ChildItem -LiteralPath $d -Force -ErrorAction SilentlyContinue)) {
      Remove-Item -LiteralPath $d -Force -ErrorAction SilentlyContinue
    }
  }
}

function Move-Legacy {
  if ((Test-Path -LiteralPath $Base) -or -not (Test-Path -LiteralPath $LegacyBase)) { return }
  $new = $script:Base
  $script:Base = $LegacyBase
  Stop-Store
  $script:Base = $new
  New-Item -ItemType Directory -Force -Path $Root | Out-Null
  Move-Item -LiteralPath $LegacyBase -Destination $Base
  Log "moved $LegacyBase to $Base"
  Remove-EmptyLegacy
}

function Install-Store {
  Move-Legacy
  if ((Get-State) -eq 'not-installed') {
    foreach ($p in @($S3Port) + @($ConsolePort | Where-Object { $_ })) {
      if (Get-NetTCPConnection -State Listen -LocalPort ([int]$p) -ErrorAction SilentlyContinue) { Die "port $p is already in use on this node; choose another port" }
    }
  }
  Check-Data
  New-Item -ItemType Directory -Force -Path $Data, $Base | Out-Null
  Write-Marker
  Lock-Down $Base
  Stop-Store
  $dir = Install-Binary
  $lines = @(
    '@echo off',
    "set RUSTFS_ACCESS_KEY=$($env:FORGE_OS_ACCESS_KEY)",
    "set RUSTFS_SECRET_KEY=$($env:FORGE_OS_SECRET_KEY)",
    "set `"RUSTFS_VOLUMES=$Data`"",
    "set RUSTFS_ADDRESS=0.0.0.0:$S3Port",
    'set RUSTFS_CHECK_UPDATE=false'
  )
  if ($ConsolePort) { $lines += 'set RUSTFS_CONSOLE_ENABLE=true'; $lines += "set RUSTFS_CONSOLE_ADDRESS=0.0.0.0:$ConsolePort" }
  else { $lines += 'set RUSTFS_CONSOLE_ENABLE=false' }
  $lines += "`"$dir\rustfs.exe`" server"
  Set-Content -Path $Runner -Value $lines -Encoding ASCII
  Lock-Down $Runner
  $act = New-ScheduledTaskAction -Execute 'cmd.exe' -Argument "/c `"$Runner`""
  $trg = New-ScheduledTaskTrigger -AtStartup
  $set = New-ScheduledTaskSettingsSet -RestartCount 999 -RestartInterval (New-TimeSpan -Minutes 1) -ExecutionTimeLimit ([TimeSpan]::Zero) -AllowStartIfOnBatteries -DontStopIfGoingOnBatteries
  $who = New-ScheduledTaskPrincipal -UserId 'SYSTEM' -LogonType ServiceAccount -RunLevel Highest
  Register-ScheduledTask -TaskName $Unit -Action $act -Trigger $trg -Settings $set -Principal $who -Description "RustFS object store $Slug (Orbyn Forge)" -Force | Out-Null
  foreach ($p in @($S3Port) + @($ConsolePort | Where-Object { $_ })) {
    $name = "$Unit-$p"
    if (-not (Get-NetFirewallRule -Name $name -ErrorAction SilentlyContinue)) {
      New-NetFirewallRule -Name $name -DisplayName "Forge object store $Slug ($p)" -Direction Inbound -Protocol TCP -LocalPort $p -Action Allow | Out-Null
    }
  }
  Start-ScheduledTask -TaskName $Unit
  Wait-Health 90
  Log "RustFS $Slug answers on port $S3Port"
}

# Ownership marker: `.forge-objstore` holds the store id. Data is deleted only
# where it matches, never through a reparse point (symlink or junction), and
# never where it overlaps another store's data path.
$Marker = '.forge-objstore'
$Owner = "$env:FORGE_OS_OWNER"

function Write-Marker {
  if (-not $Owner -or -not (Test-Path -LiteralPath $Data -PathType Container)) { return }
  $m = Join-Path $Data $Marker
  if (Test-Path -LiteralPath $m) {
    $cur = (Get-Content -LiteralPath $m -TotalCount 1)
    if ($cur -ne $Owner) { Log "warning: $Data is marked as data of store $cur; marker kept" }
    return
  }
  $items = @(Get-ChildItem -LiteralPath $Data -Force -ErrorAction SilentlyContinue)
  if ($items.Count -eq 0 -or (Test-Path -LiteralPath (Join-Path $Data '.rustfs.sys'))) {
    Set-Content -LiteralPath $m -Value $Owner -Encoding ASCII
  } else {
    Log "warning: $Data holds other files; it is not marked, and deleting data skips it"
  }
}

function Test-Overlap($a, $b) {
  $a = $a.TrimEnd('\').ToLowerInvariant(); $b = $b.TrimEnd('\').ToLowerInvariant()
  return ($a -eq $b) -or $a.StartsWith("$b\") -or $b.StartsWith("$a\")
}

function Remove-OwnedData {
  if (-not (Test-Path -LiteralPath $Data)) { return }
  $item = Get-Item -LiteralPath $Data -Force
  for ($i = $item; $i; $i = $i.Parent) {
    if ($i.Attributes -band [IO.FileAttributes]::ReparsePoint) { Log "warning: skipped ${Data}: it or a parent is a symlink or junction"; return }
  }
  $real = $item.FullName.TrimEnd('\')
  if ($real -ne $Data.TrimEnd('\')) { Log "warning: skipped ${Data}: the path does not resolve to itself"; return }
  $m = Join-Path $real $Marker
  $cur = if (Test-Path -LiteralPath $m) { Get-Content -LiteralPath $m -TotalCount 1 } else { '' }
  if (-not $Owner -or $cur -ne $Owner) { Log "warning: skipped ${Data}: not owned by this store"; return }
  foreach ($o in ("$env:FORGE_OS_PROTECT" -split '\|')) {
    if ($o -and (Test-Overlap $real $o)) { Log "warning: skipped ${Data}: it overlaps $o of another store"; return }
  }
  $nested = Get-ChildItem -LiteralPath $real -Recurse -Force -Filter $Marker -ErrorAction SilentlyContinue | Where-Object { $_.DirectoryName.TrimEnd('\') -ne $real } | Select-Object -First 1
  if ($nested) { Log "warning: skipped ${Data}: it contains data of another store ($($nested.FullName))"; return }
  # rd /s removes junctions and symlinks inside the tree as links and never
  # follows them; Remove-Item -Recurse on PowerShell 5.1 can delete a
  # junction's target.
  & cmd.exe /d /c "rd /s /q `"$real`" >nul 2>nul"
  if (Test-Path -LiteralPath $real) { Log "warning: some files in $Data could not be deleted" }
  Log "deleted the data in $Data"
}

function Uninstall-Store {
  Stop-Store
  Unregister-ScheduledTask -TaskName $Unit -Confirm:$false -ErrorAction SilentlyContinue
  Get-NetFirewallRule -Name "$Unit-*" -ErrorAction SilentlyContinue | Remove-NetFirewallRule
  Remove-Item -Recurse -Force $Base -ErrorAction SilentlyContinue
  Remove-EmptyLegacy
  if ($env:FORGE_OS_DELETE_DATA -eq '1') {
    Remove-OwnedData
    Log "removed $Unit"
  } else {
    Log "removed $Unit; data kept in $Data"
  }
}

switch ($Action) {
  'install' { Install-Store }
  'uninstall' { Uninstall-Store }
  'start' { if ((Get-State) -eq 'not-installed') { Die "object store $Slug is not installed" }; Check-Data; Start-ScheduledTask -TaskName $Unit; Wait-Health 60 }
  'stop' { Stop-Store }
  'restart' { if ((Get-State) -eq 'not-installed') { Die "object store $Slug is not installed" }; Check-Data; Stop-Store; Start-ScheduledTask -TaskName $Unit; Wait-Health 60 }
  'status' { Write-Marker }
  default { Die "unknown action $Action" }
}
Report
exit 0
