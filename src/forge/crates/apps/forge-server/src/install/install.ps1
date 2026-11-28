# Orbyn Forge node installer for Windows (D-079). Installs forge-agent as the
# "ForgeAgent" Windows service and enrolls this machine. Safe to re-run: a
# second run upgrades the agent and keeps the enrollment.
#
#   $env:FORGE_ENROLL_TOKEN = '<token>'; irm https://<forge-host>/install.ps1 | iex
#
# Or download, read, then run it:
#
#   irm https://<forge-host>/install.ps1 -OutFile install.ps1
#   powershell -ExecutionPolicy Bypass -File .\install.ps1
#
# Remove the agent: run it with -Uninstall (keeps the enrollment data) or
# -Uninstall -Purge (removes everything), or set $env:FORGE_UNINSTALL=1 and
# $env:FORGE_PURGE=1 when piping into iex.
#
# It changes only: the ForgeAgent service, C:\Program Files\OrbynForge and
# C:\ProgramData\OrbynForge (enrollment, logs, WireGuard keys, package
# settings, file store), owned by Administrators and readable by SYSTEM and
# Administrators only. No firewall
# rules (the agent only connects out), no execution policy, no PATH or other
# environment changes. It removes $env:FORGE_ENROLL_TOKEN, FORGE_UNINSTALL and
# FORGE_PURGE from this session when it ends, also after an error.

$ErrorActionPreference = 'Stop'
Set-StrictMode -Version 3.0

$ForgeUrl = '{{FORGE_URL}}'
$AgentUrl = '{{AGENT_URL}}'
$AgentSha = '{{AGENT_SHA}}'
$ServiceName = 'ForgeAgent'
$InstallDir = Join-Path $env:ProgramFiles 'OrbynForge'
$Exe = Join-Path $InstallDir 'forge-agent.exe'
$DataRoot = Join-Path $env:ProgramData 'OrbynForge'
$StateDir = Join-Path $DataRoot 'agent'

$Uninstall = ($args -contains '-Uninstall') -or ($env:FORGE_UNINSTALL -eq '1')
$Purge = ($args -contains '-Purge') -or ($env:FORGE_PURGE -eq '1')
$AllowHttp = ($args -contains '-AllowHttp') -or ($env:FORGE_ALLOW_HTTP -eq '1')

# Every step is printed and also reported to Forge (authenticated by the
# enrollment token in a header, never a URL), so the onboarding console shows
# the install. Reporting is best effort and never fails the install.
function Send-Report([string]$Text) {
    if (-not $env:FORGE_ENROLL_TOKEN) { return }
    try {
        Invoke-RestMethod -Method Post -Uri "$ForgeUrl/api/v1/enroll/progress" -TimeoutSec 5 `
            -Headers @{ Authorization = "Bearer $env:FORGE_ENROLL_TOKEN" } -ContentType 'text/plain' -Body $Text | Out-Null
    } catch { }
}
function Say([string]$Text) { Write-Host "forge-install: $Text"; Send-Report $Text }
function Step([string]$Text) { Write-Host "`$ $Text"; Send-Report "`$ $Text" }
function Fail([string]$Text) {
    Write-Host "forge-install: error: $Text" -ForegroundColor Red
    Send-Report "error: $Text"
    # `throw`, not `exit`: under `irm | iex`, exit would close the window.
    throw "forge-install: $Text"
}

# SYSTEM and Administrators: the only accounts the data folder grants access to.
$LockedSids = @('S-1-5-18', 'S-1-5-32-544')
# Owners accepted on existing data: those two and the Administrator running this.
$TrustedOwners = $LockedSids + [Security.Principal.WindowsIdentity]::GetCurrent().User.Value

# Whether $Path is already locked by this installer: not a link, owned by a
# trusted account, no inherited access, and access for SYSTEM and Administrators only.
function Test-Locked([string]$Path) {
    $item = Get-Item -LiteralPath $Path -Force
    if ($item.Attributes -band [IO.FileAttributes]::ReparsePoint) { return $false }
    $acl = Get-Acl -LiteralPath $Path
    if ($TrustedOwners -notcontains $acl.GetOwner([Security.Principal.SecurityIdentifier]).Value) { return $false }
    if (-not $acl.AreAccessRulesProtected) { return $false }
    foreach ($r in $acl.GetAccessRules($true, $true, [Security.Principal.SecurityIdentifier])) {
        if ($LockedSids -notcontains $r.IdentityReference.Value) { return $false }
    }
    return $true
}

# The first item under $Path (itself included) that someone else could have
# planted: a link (junction, symlink, mount point) or an item owned by another
# account. Links are never followed. Returns $null when there is none.
function Find-Untrusted([string]$Path) {
    $item = Get-Item -LiteralPath $Path -Force
    if ($item.Attributes -band [IO.FileAttributes]::ReparsePoint) { return "$Path is a link (reparse point)" }
    $owner = (Get-Acl -LiteralPath $Path).GetOwner([Security.Principal.SecurityIdentifier]).Value
    if ($TrustedOwners -notcontains $owner) { return "$Path is owned by $owner, not SYSTEM or Administrators" }
    if ($item.PSIsContainer) {
        foreach ($child in Get-ChildItem -LiteralPath $Path -Force) {
            $why = Find-Untrusted $child.FullName
            if ($why) { return $why }
        }
    }
    return $null
}

# Owned by Administrators, no inherited access, full control for SYSTEM and
# Administrators only. Any user may create folders in C:\ProgramData, so a
# folder that exists but is not locked yet is checked first: one holding a
# link or another account's files is refused, never taken over (its ACLs and
# links would survive and expose the agent's keys). Checked again after
# locking, for anything created in between.
function Protect-Folder([string]$Path) {
    if (Test-Path -LiteralPath $Path) {
        if (Test-Locked $Path) { return }
        $why = Find-Untrusted $Path
        if ($why) { Fail "$why; this installer did not create it. Inspect and remove $Path, then run the installer again" }
    } else {
        New-Item -ItemType Directory -Path $Path | Out-Null
    }
    Step "icacls $Path /setowner Administrators; Set-Acl: no inheritance, SYSTEM and Administrators full control only"
    & icacls.exe $Path /setowner '*S-1-5-32-544' /C /Q | Out-Null
    if ($LASTEXITCODE -ne 0) { Fail "could not take ownership of $Path" }
    # A new ACL, not icacls /grant:r: that would keep other accounts' entries.
    $acl = New-Object Security.AccessControl.DirectorySecurity
    $acl.SetAccessRuleProtection($true, $false)
    foreach ($sid in $LockedSids) {
        $acl.AddAccessRule((New-Object Security.AccessControl.FileSystemAccessRule(
            (New-Object Security.Principal.SecurityIdentifier($sid)), 'FullControl',
            'ContainerInherit, ObjectInherit', 'None', 'Allow')))
    }
    Set-Acl -LiteralPath $Path -AclObject $acl
    if (-not (Test-Locked $Path)) { Fail "could not restrict access to $Path" }
    $why = Find-Untrusted $Path
    if ($why) { Fail "$why; created while the installer ran. Inspect and remove $Path, then run the installer again" }
}

# Stops the ForgeAgent service: asks the SCM, waits up to 30 s, then ends
# forge-agent.exe from $InstallDir only. Failure actions are cleared first so
# the SCM does not restart the old binary; `service install` sets them again.
function Stop-Agent {
    $svc = Get-Service -Name $ServiceName -ErrorAction SilentlyContinue
    if (-not $svc -or $svc.Status -eq 'Stopped') { return }
    Step "sc.exe stop $ServiceName"
    & sc.exe stop $ServiceName | Out-Null
    $deadline = (Get-Date).AddSeconds(30)
    while ((Get-Date) -lt $deadline) {
        $svc.Refresh()
        if ($svc.Status -eq 'Stopped') { return }
        Start-Sleep -Milliseconds 500
    }
    Step "Stop-Process forge-agent  # did not stop within 30 s"
    & sc.exe failure $ServiceName reset= 0 actions= '""' | Out-Null
    Get-Process -Name 'forge-agent' -ErrorAction SilentlyContinue |
        Where-Object { $_.Path -and ($_.Path -ieq $Exe) } |
        Stop-Process -Force -ErrorAction SilentlyContinue
    $deadline = (Get-Date).AddSeconds(15)
    while ((Get-Date) -lt $deadline) {
        $svc.Refresh()
        if ($svc.Status -eq 'Stopped') { return }
        Start-Sleep -Milliseconds 500
    }
    Fail "the $ServiceName service did not stop"
}

try {

# TLS 1.2 for this process only (Windows PowerShell 5.1 may default to older).
[Net.ServicePointManager]::SecurityProtocol = [Net.ServicePointManager]::SecurityProtocol -bor [Net.SecurityProtocolType]::Tls12

Step 'check: Administrator, 64-bit, Windows 10 1809 / Server 2019 or later'
$principal = New-Object Security.Principal.WindowsPrincipal([Security.Principal.WindowsIdentity]::GetCurrent())
if (-not $principal.IsInRole([Security.Principal.WindowsBuiltInRole]::Administrator)) {
    Fail 'run PowerShell as Administrator'
}
if (-not [Environment]::Is64BitOperatingSystem -or $env:PROCESSOR_ARCHITECTURE -ne 'AMD64') {
    Fail 'this installer needs 64-bit Windows on x86_64'
}
$build = [Environment]::OSVersion.Version.Build
if ($build -lt 17763) { Fail "Windows 10 1809 or Windows Server 2019 or later is needed (this is build $build)" }

if ($Uninstall) {
    Say "removing forge-agent from $env:COMPUTERNAME"
    Stop-Agent
    if (Test-Path $Exe) {
        Step "forge-agent.exe service uninstall"
        & $Exe service uninstall
        if ($LASTEXITCODE -ne 0) { Fail 'could not remove the ForgeAgent service' }
    } elseif (Get-Service -Name $ServiceName -ErrorAction SilentlyContinue) {
        Step "sc.exe delete $ServiceName"
        & sc.exe delete $ServiceName | Out-Null
    }
    if (Test-Path $InstallDir) {
        Step "Remove-Item $InstallDir"
        Remove-Item -Recurse -Force $InstallDir
    }
    if ($Purge) {
        if (Test-Path $DataRoot) {
            Step "Remove-Item $DataRoot  # -Purge: identity, keys, logs and agent data"
            Remove-Item -Recurse -Force $DataRoot
        }
        Say 'forge-agent removed with its data; revoke the node in Forge as well'
    } else {
        Say "forge-agent removed; its enrollment stays in $StateDir (-Purge removes it)"
    }
    return
}

if (-not $env:FORGE_ENROLL_TOKEN -and -not (Test-Path (Join-Path $StateDir 'identity.json'))) {
    Fail 'set $env:FORGE_ENROLL_TOKEN to the token shown in Orbyn Forge'
}
if (-not $AgentSha) { Fail 'this Forge server has no Windows agent build' }
if (-not $ForgeUrl.StartsWith('https://') -and -not $AllowHttp) {
    Fail 'refusing a plain http:// server; set $env:FORGE_ALLOW_HTTP=1 only for a test lab'
}

Say "installing forge-agent on $env:COMPUTERNAME"
# Downloaded straight into the install folder, which only Administrators can
# write: a copy in %TEMP% could be swapped by any program this user runs
# between the checksum and the install, and would then run as LocalSystem.
New-Item -ItemType Directory -Force -Path $InstallDir | Out-Null
$staged = "$Exe.download"
try {
    Step "download $ForgeUrl/agent/download/windows/x86_64/forge-agent"
    $progressPreference = 'SilentlyContinue'
    Invoke-WebRequest -UseBasicParsing -Uri "$ForgeUrl/agent/download/windows/x86_64/forge-agent" -OutFile $staged
    $got = (Get-FileHash -LiteralPath $staged -Algorithm SHA256).Hash.ToLowerInvariant()
    if ($got -ne $AgentSha) { Fail "checksum mismatch for forge-agent.exe (expected $AgentSha, got $got)" }
    Say 'checksum verified'

    Stop-Agent  # upgrade: the running binary cannot be replaced
    Step "install $Exe"
    Move-Item -Force -LiteralPath $staged -Destination $Exe

    # Enrollment data (private key, identity) and the agent's own data
    # (WireGuard keys, package settings, file store): SYSTEM and Administrators only.
    Protect-Folder $DataRoot
    New-Item -ItemType Directory -Force -Path $StateDir | Out-Null

    if (Test-Path (Join-Path $StateDir 'identity.json')) {
        Say 'already enrolled; keeping this node''s identity'
    } else {
        Step "forge-agent.exe enroll --server $AgentUrl  # token from `$env:FORGE_ENROLL_TOKEN"
        & $Exe --state-dir $StateDir enroll --server $AgentUrl
        if ($LASTEXITCODE -ne 0) { Fail 'enrollment failed (is the token still valid?)' }
    }

    Step "forge-agent.exe service install  # ForgeAgent, automatic start, LocalSystem"
    & $Exe --state-dir $StateDir service install
    if ($LASTEXITCODE -ne 0) { Fail 'could not install or start the ForgeAgent service' }
    Say "the ForgeAgent service is running; logs: $StateDir\logs"
    Send-Report 'installer finished'
} catch {
    Send-Report 'installer failed'
    throw
} finally {
    Remove-Item -Force -LiteralPath $staged -ErrorAction SilentlyContinue
}

} finally {
    # The token is single use and the flags are one-shot: never leave them in
    # this session, whether the install worked or not.
    foreach ($v in 'FORGE_ENROLL_TOKEN', 'FORGE_UNINSTALL', 'FORGE_PURGE') {
        Remove-Item -Path "Env:$v" -ErrorAction SilentlyContinue
    }
}
