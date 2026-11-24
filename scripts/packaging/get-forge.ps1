<#
.SYNOPSIS
  Downloads an Orbyn Forge release for Windows, verifies it and runs its installer.

.EXAMPLE
  Invoke-WebRequest https://github.com/orbyntech/forge/releases/latest/download/get-forge.ps1 -OutFile get-forge.ps1
  .\get-forge.ps1                                   # latest stable release
  .\get-forge.ps1 -Version v0.0.1-rc-01             # one release, also a pre-release
  .\get-forge.ps1 -File .\forge-0.0.1-x86_64-windows.zip
  .\get-forge.ps1 -PublicUrl http://forge.example.com:3880   # other options go to install.ps1

.DESCRIPTION
  Nothing is installed unless SHA256SUMS carries a valid Ed25519 signature
  from the Orbyn Forge release key (pinned below) and the package's SHA-256
  is listed in it. Windows has no built-in Ed25519 verifier, so the signature
  is checked with openssl.exe: from PATH or from Git for Windows. Without it
  the script stops, unless -SkipSignatureCheck is given (then only the
  checksum, downloaded over HTTPS from GitHub, is checked).

  -File installs from a downloaded package; SHA256SUMS and SHA256SUMS.sig
  must be in the same folder. -DownloadOnly verifies and unpacks into the
  current folder without installing. Run in PowerShell as Administrator.

  Environment (testing and mirrors): FORGE_RELEASES_URL (default
  https://github.com/orbyntech/forge/releases) and
  FORGE_RELEASE_PUBLIC_KEY (another base64url release key).
#>
param(
  [string]$Version = "latest",
  [string]$File = "",
  [switch]$DownloadOnly,
  [switch]$SkipSignatureCheck,
  [Parameter(ValueFromRemainingArguments = $true)]
  [string[]]$InstallArgs = @()
)
$ErrorActionPreference = 'Stop'
$ProgressPreference = 'SilentlyContinue'   # Invoke-WebRequest is slow with a progress bar
[Net.ServicePointManager]::SecurityProtocol = [Net.ServicePointManager]::SecurityProtocol -bor [Net.SecurityProtocolType]::Tls12

# Must equal RELEASE_PUBLIC_KEY in src/protocol/forge-sdk/src/release.rs
# (the release workflow checks this).
$PinnedKey = "6yf7l7jBEw6u446vqWAgt7P7bqEuaY5tpbYnWPHBmbg"
$Base = if ($env:FORGE_RELEASES_URL) { $env:FORGE_RELEASES_URL.TrimEnd('/') } else { "https://github.com/orbyntech/forge/releases" }
$Key = if ($env:FORGE_RELEASE_PUBLIC_KEY) { $env:FORGE_RELEASE_PUBLIC_KEY } else { $PinnedKey }

function Fail([string]$m) { Write-Host ""; Write-Host "  x $m" -ForegroundColor Red; Write-Host ""; exit 1 }
function Say([string]$l, [string]$d) { Write-Host ("  + {0,-22} {1}" -f $l, $d) }

if ($Key -ne $PinnedKey) { Write-Host "  ! FORGE_RELEASE_PUBLIC_KEY overrides the Orbyn Forge release key" -ForegroundColor Yellow }
if (-not $DownloadOnly) {
  $p = [Security.Principal.WindowsPrincipal][Security.Principal.WindowsIdentity]::GetCurrent()
  if (-not $p.IsInRole([Security.Principal.WindowsBuiltInRole]::Administrator)) { Fail "Run PowerShell as Administrator." }
}
if ($Version -eq "latest") { $tag = "latest" }
elseif ($Version -match '^v?[0-9]+\.[0-9]+\.[0-9]+(-rc-[0-9]+)?$') { $tag = if ($Version.StartsWith("v")) { $Version } else { "v$Version" } }
else { Fail "-Version must look like v1.2.3, v1.2.3-rc-01 or latest (got $Version)" }

$work = Join-Path ([IO.Path]::GetTempPath()) ("forge-" + [Guid]::NewGuid().ToString("N"))
New-Item -ItemType Directory -Path $work | Out-Null
try {
  # ---------------------------------------------------------------- fetch
  if ($File) {
    if (-not (Test-Path $File)) { Fail "$File not found." }
    $src = Split-Path -Parent (Resolve-Path $File)
    $pkg = Split-Path -Leaf $File
    foreach ($f in "SHA256SUMS", "SHA256SUMS.sig") {
      if (-not (Test-Path (Join-Path $src $f))) { Fail "$f is not next to $pkg. Download it from the same release (or use -Version)." }
      Copy-Item (Join-Path $src $f) $work
    }
    Copy-Item (Resolve-Path $File) $work
    Say "Package" $File
  } else {
    $url = if ($tag -eq "latest") { "$Base/latest/download" } else { "$Base/download/$tag" }
    function Fetch([string]$n) { Invoke-WebRequest -UseBasicParsing -Uri "$url/$n" -OutFile (Join-Path $work $n) }
    try { Fetch "SHA256SUMS" } catch {
      if ($tag -eq "latest") { Fail "No stable release found at $Base. For a pre-release, pass its tag: -Version vX.Y.Z-rc-NN." }
      Fail "Release $tag not found at $Base (check the tag on the releases page)."
    }
    try { Fetch "SHA256SUMS.sig" } catch { Fail "SHA256SUMS.sig is missing from the release; it is not installed." }
    $pkg = (Get-Content (Join-Path $work "SHA256SUMS") | ForEach-Object { ($_ -split '\s+')[1] -replace '^\*', '' } |
      Where-Object { $_ -match '^forge-[0-9][A-Za-z0-9.+_-]*-x86_64-windows\.zip$' } | Select-Object -First 1)
    if (-not $pkg) { Fail "The release has no Windows package." }
    Say "Downloading" $pkg
    Fetch $pkg
  }

  # ---------------------------------------------------------------- verify
  $sums = Join-Path $work "SHA256SUMS"
  if ($SkipSignatureCheck) {
    Write-Host "  ! Signature not checked (-SkipSignatureCheck); only the checksum is." -ForegroundColor Yellow
  } else {
    $openssl = (Get-Command openssl -ErrorAction SilentlyContinue).Source
    foreach ($c in "$env:ProgramFiles\Git\usr\bin\openssl.exe", "$env:ProgramFiles\Git\mingw64\bin\openssl.exe") {
      if (-not $openssl -and (Test-Path $c)) { $openssl = $c }
    }
    if (-not $openssl) { Fail "openssl.exe is needed to check the release signature. Install Git for Windows (it includes OpenSSL), or pass -SkipSignatureCheck to rely on the checksum only." }
    $k = $Key.Replace('-', '+').Replace('_', '/'); while ($k.Length % 4) { $k += "=" }
    $raw = [Convert]::FromBase64String($k)
    if ($raw.Length -ne 32) { Fail "The release key is not an Ed25519 key." }
    [byte[]]$prefix = 0x30, 0x2a, 0x30, 0x05, 0x06, 0x03, 0x2b, 0x65, 0x70, 0x03, 0x21, 0x00
    [IO.File]::WriteAllBytes((Join-Path $work "pub.der"), $prefix + $raw)
    $sig = [Convert]::FromBase64String(((Get-Content -Raw (Join-Path $work "SHA256SUMS.sig")) -replace '\s', ''))
    [IO.File]::WriteAllBytes((Join-Path $work "sig.bin"), $sig)
    & $openssl pkeyutl -verify -pubin -keyform DER -inkey (Join-Path $work "pub.der") -rawin -in $sums -sigfile (Join-Path $work "sig.bin") | Out-Null
    if ($LASTEXITCODE -ne 0) { Fail "SHA256SUMS does not carry a valid Orbyn Forge signature. Nothing was installed." }
    Say "Signature" "SHA256SUMS signed by the Orbyn Forge release key"
  }
  $want = (Get-Content $sums | Where-Object { (($_ -split '\s+')[1] -replace '^\*', '') -eq $pkg } | ForEach-Object { ($_ -split '\s+')[0] } | Select-Object -First 1)
  if (-not $want) { Fail "$pkg is not listed in SHA256SUMS." }
  $got = (Get-FileHash -Algorithm SHA256 (Join-Path $work $pkg)).Hash.ToLowerInvariant()
  if ($got -ne $want.ToLowerInvariant()) { Fail "$pkg does not match SHA256SUMS (damaged or tampered download). Nothing was installed." }
  Say "Checksum" "$pkg matches SHA256SUMS"
  if ($pkg -notmatch '^forge-.*-x86_64-windows\.zip$') { Fail "$pkg is not a Forge Windows package." }

  # ---------------------------------------------------------------- unpack, install
  $name = $pkg -replace '\.zip$', ''
  $dest = if ($DownloadOnly) { (Get-Location).Path } else { $work }
  Expand-Archive -Path (Join-Path $work $pkg) -DestinationPath $dest -Force
  $installer = Join-Path (Join-Path $dest $name) "install.ps1"
  if (-not (Test-Path $installer)) { Fail "$pkg has no install.ps1." }
  if ($DownloadOnly) {
    Say "Unpacked" (Join-Path $dest $name)
    Write-Host ""; Write-Host "  Install with: cd '$(Join-Path $dest $name)'; .\install.ps1"; Write-Host ""
    exit 0
  }
  & $installer @InstallArgs
  exit $LASTEXITCODE
} finally {
  Remove-Item -Recurse -Force $work -ErrorAction SilentlyContinue
}
