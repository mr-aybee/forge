<#
.SYNOPSIS
    Load test for the dev portals behind the Forge ingress (Nginx edge).

.DESCRIPTION
    Brute-forces both portals through the ingress: many parallel senders, each
    alternating between the portals (so every portal gets the same number of
    requests) and calling a random endpoint each time (every page, the JSON API,
    health, and POST /notes, which writes to PostgreSQL on the database node).
    Reads the X-Served-By header of each answer, so you see how the edge
    spreads the load over the nodes. Prints throughput, latency percentiles,
    status codes, and the split per endpoint, per portal and per node.

    Works in Windows PowerShell 5.1 and PowerShell 7 (Windows, Linux, macOS).

.EXAMPLE
    # Every endpoint of both portals: 50 senders x 40 requests
    .\load-portals.ps1

.EXAMPLE
    # 200 senders hammering everything for 2 minutes
    .\load-portals.ps1 -Senders 200 -DurationSeconds 120

.EXAMPLE
    # Only database traffic: list and save notes
    .\load-portals.ps1 -Endpoints 'GET /notes','POST /notes' -Senders 50 -DurationSeconds 60

.EXAMPLE
    # Only the .NET portal
    .\load-portals.ps1 -Portals http://net.127.0.0.1.nip.io:8088
#>
[CmdletBinding()]
param(
    # Portal base URLs (through the ingress). Every sender alternates over all
    # of them, so each portal gets the same share of the requests.
    [string[]]$Portals = @('http://py.127.0.0.1.nip.io:8088', 'http://net.127.0.0.1.nip.io:8088'),
    # Endpoints to call, as 'METHOD /path'; each request picks one at random.
    # POST /notes saves a note (a write to PostgreSQL).
    [string[]]$Endpoints = @('GET /', 'GET /?format=json', 'GET /notes', 'POST /notes', 'GET /health', 'GET /about'),
    # Parallel senders (connections).
    [ValidateRange(1, 2000)][int]$Senders = 50,
    # Requests per sender. Ignored when -DurationSeconds is set.
    [ValidateRange(1, 1000000)][int]$RequestsPerSender = 40,
    # Run for this long instead of a fixed number of requests (0: off).
    [ValidateRange(0, 86400)][int]$DurationSeconds = 0,
    # Per-request timeout.
    [ValidateRange(1, 600)][int]$TimeoutSeconds = 30
)

Set-StrictMode -Version 3
$ErrorActionPreference = 'Stop'

Add-Type -AssemblyName System.Net.Http
# .NET Framework (PowerShell 5.1) allows 2 connections per host by default.
[System.Net.ServicePointManager]::DefaultConnectionLimit = [Math]::Max(100, $Senders * 2)
[System.Net.ServicePointManager]::Expect100Continue = $false

$runId = '{0:yyyyMMdd-HHmmss}-{1}' -f (Get-Date), (Get-Random -Maximum 9999)
$handler = [System.Net.Http.HttpClientHandler]::new()
$handler.AllowAutoRedirect = $false   # POST /notes answers 303/302: count that, not the redirect
$client = [System.Net.Http.HttpClient]::new($handler)
$client.Timeout = [TimeSpan]::FromSeconds($TimeoutSeconds)
$Portals = @($Portals | ForEach-Object { $_.TrimEnd('/') })
$Endpoints = @($Endpoints | ForEach-Object {
    $m, $pth = ($_.Trim() -split '\s+', 2)
    if (-not $pth) { $pth = $m; $m = 'GET' }
    if (-not $pth.StartsWith('/')) { $pth = "/$pth" }
    $m = $m.ToUpperInvariant()
    if ($m -notin 'GET', 'POST') { throw "Endpoint '$_': use GET or POST" }
    "$m $pth"
})

# One request per portal first: a wrong URL or DNS name stops here.
foreach ($p in $Portals) {
    try {
        $r = $client.GetAsync("$p/health").GetAwaiter().GetResult()
        $node = if ($r.Headers.Contains('X-Served-By')) { @($r.Headers.GetValues('X-Served-By'))[0] } else { '?' }
        Write-Host ("  reachable  {0}  ({1}, served by {2})" -f $p, [int]$r.StatusCode, $node) -ForegroundColor DarkGray
        $r.Dispose()
    } catch {
        throw "Cannot reach $p : $($_.Exception.GetBaseException().Message). Is Tailscale up, and does the name resolve (nip.io needs internet DNS)?"
    }
}

$what = $Endpoints -join ', '
$mode = if ($DurationSeconds -gt 0) { "for $DurationSeconds s" } else { "$RequestsPerSender requests each" }
Write-Host ''
Write-Host "Forge ingress load test  RunId=$runId" -ForegroundColor Cyan
Write-Host "  portals  $($Portals -join ', ')"
Write-Host "  random   $what"
Write-Host "  senders  $Senders ($mode)"
Write-Host ''

$shared = [hashtable]::Synchronized(@{ Requests = 0; Failed = 0 })
$deadline = if ($DurationSeconds -gt 0) { [DateTime]::UtcNow.AddSeconds($DurationSeconds) } else { [DateTime]::MaxValue }

$worker = {
    param($client, $portals, $endpoints, $runId, $shared, $sender, $requests, $deadline)
    $results = [System.Collections.Generic.List[object]]::new()
    $rnd = [Random]::new($sender * 7919 + [Environment]::TickCount)
    $req = 0
    while (($req -lt $requests) -and ([DateTime]::UtcNow -lt $deadline)) {
        $req++
        $portal = $portals[($sender + $req) % $portals.Count]   # alternate: equal load per portal
        $endpoint = $endpoints[$rnd.Next($endpoints.Count)]
        $method, $path = $endpoint -split ' ', 2
        $sw = [System.Diagnostics.Stopwatch]::StartNew()
        $code = 0; $node = ''; $err = $null
        try {
            if ($method -eq 'POST') {
                $form = [System.Collections.Generic.Dictionary[string, string]]::new()
                $form['body'] = "load $runId sender $sender request $req"
                $content = [System.Net.Http.FormUrlEncodedContent]::new($form)
                $resp = $client.PostAsync("$portal$path", $content).GetAwaiter().GetResult()
                $content.Dispose()
            } else {
                $resp = $client.GetAsync("$portal$path").GetAwaiter().GetResult()
            }
            [void]$resp.Content.ReadAsByteArrayAsync().GetAwaiter().GetResult()   # read the whole body, like a browser
            $code = [int]$resp.StatusCode
            if ($resp.Headers.Contains('X-Served-By')) { $node = @($resp.Headers.GetValues('X-Served-By'))[0] }
            if ($code -ge 400) { $err = "HTTP $code from $endpoint on $portal" }
            $resp.Dispose()
        } catch {
            $err = $_.Exception.GetBaseException().Message
        }
        $sw.Stop()
        $results.Add([pscustomobject]@{ Portal = $portal; Endpoint = $endpoint; Node = $node; Ms = $sw.Elapsed.TotalMilliseconds; Code = $code; Error = $err })
        [System.Threading.Monitor]::Enter($shared.SyncRoot)
        try {
            $shared.Requests++
            if ($code -eq 0 -or $code -ge 400) { $shared.Failed++ }
        } finally { [System.Threading.Monitor]::Exit($shared.SyncRoot) }
    }
    , $results
}

$pool = [RunspaceFactory]::CreateRunspacePool(1, $Senders)
$pool.Open()
$jobs = foreach ($s in 1..$Senders) {
    $ps = [PowerShell]::Create()
    $ps.RunspacePool = $pool
    [void]$ps.AddScript($worker).AddArgument($client).AddArgument($Portals).AddArgument($Endpoints).
        AddArgument($runId).AddArgument($shared).AddArgument($s).
        AddArgument($(if ($DurationSeconds -gt 0) { [int]::MaxValue } else { $RequestsPerSender })).AddArgument($deadline)
    [pscustomobject]@{ PS = $ps; Handle = $ps.BeginInvoke() }
}

$clock = [System.Diagnostics.Stopwatch]::StartNew()
$planned = if ($DurationSeconds -gt 0) { 0 } else { $Senders * $RequestsPerSender }
try {
    while (@($jobs | Where-Object { -not $_.Handle.IsCompleted }).Count -gt 0) {
        Start-Sleep -Milliseconds 500
        $secs = [Math]::Max($clock.Elapsed.TotalSeconds, 0.001)
        $status = '{0:N0} requests, {1:N0} failed, {2:N0} req/s' -f $shared.Requests, $shared.Failed, ($shared.Requests / $secs)
        $pct = if ($planned) { [Math]::Min(100, [int](100 * $shared.Requests / $planned)) } else { [Math]::Min(100, [int](100 * $secs / $DurationSeconds)) }
        Write-Progress -Activity "Ingress load test $runId" -Status $status -PercentComplete $pct
    }
    $all = [System.Collections.Generic.List[object]]::new()
    foreach ($j in $jobs) { foreach ($r in $j.PS.EndInvoke($j.Handle)) { $all.AddRange([object[]]$r) } }
} finally {
    $clock.Stop()
    Write-Progress -Activity "Ingress load test $runId" -Completed
    foreach ($j in $jobs) { $j.PS.Dispose() }
    $pool.Close(); $pool.Dispose()
    $client.Dispose()
}

# ------------------------------------------------------------------ summary
function Get-Percentile([double[]]$sorted, [double]$p) {
    if ($sorted.Count -eq 0) { return 0 }
    $sorted[[Math]::Min($sorted.Count - 1, [int][Math]::Floor($p * $sorted.Count))]
}
function Format-Latency($rows) {
    $l = [double[]]($rows | ForEach-Object Ms | Sort-Object)
    'p50 {0,5:N0}  p95 {1,5:N0}  p99 {2,5:N0}  max {3,6:N0} ms' -f (Get-Percentile $l 0.50), (Get-Percentile $l 0.95), (Get-Percentile $l 0.99), (Get-Percentile $l 1.0)
}
$secs = [Math]::Max($clock.Elapsed.TotalSeconds, 0.001)
$good = @($all | Where-Object { $_.Code -gt 0 -and $_.Code -lt 400 })

Write-Host ''
Write-Host "Result  RunId=$runId" -ForegroundColor Cyan
'  {0,-12} {1:N1} s' -f 'duration', $secs
'  {0,-12} {1:N0} requests, {2:N0} ok ({3:P1})' -f 'requests', $all.Count, $good.Count, $(if ($all.Count) { $good.Count / $all.Count } else { 0 })
'  {0,-12} {1:N0} requests/s' -f 'throughput', ($all.Count / $secs)
'  {0,-12} {1}' -f 'latency', (Format-Latency $all)

Write-Host ''
Write-Host '  per endpoint' -ForegroundColor Cyan
foreach ($eg in ($all | Group-Object Endpoint | Sort-Object Name)) {
    $okN = @($eg.Group | Where-Object { $_.Code -gt 0 -and $_.Code -lt 400 }).Count
    Write-Host ('    {0,-20} {1,7:N0}  ok {2,6:P1}  {3}' -f $eg.Name, $eg.Count, ($okN / [Math]::Max(1, $eg.Count)), (Format-Latency $eg.Group))
}

Write-Host ''
Write-Host '  per portal and node (load balancing)' -ForegroundColor Cyan
foreach ($pg in ($all | Group-Object Portal | Sort-Object Name)) {
    $host_ = ([Uri]$pg.Name).Host
    Write-Host ('  {0}  {1:N0} requests' -f $host_, $pg.Count)
    foreach ($ng in ($pg.Group | Group-Object Node | Sort-Object Name)) {
        $label = if ($ng.Name) { $ng.Name } else { '(no answer)' }
        $share = $ng.Count / [Math]::Max(1, $pg.Count)
        $bar = '#' * [int][Math]::Round($share * 30)
        Write-Host ('    {0,-14} {1,7:N0}  {2,6:P1}  {3,-30}  {4}' -f $label, $ng.Count, $share, $bar, (Format-Latency $ng.Group))
    }
}

Write-Host ''
Write-Host '  status codes'
$all | Group-Object Code | Sort-Object Name | ForEach-Object {
    $label = if ($_.Name -eq '0') { 'no answer' } else { $_.Name }
    $color = if ([int]$_.Name -gt 0 -and [int]$_.Name -lt 400) { 'Green' } else { 'Yellow' }
    Write-Host ('    {0,-10} {1,8:N0}' -f $label, $_.Count) -ForegroundColor $color
}
$errs = @($all | Where-Object { $_.Error } | Group-Object Error | Sort-Object Count -Descending | Select-Object -First 5)
if ($errs.Count) {
    Write-Host '  most common errors' -ForegroundColor Yellow
    foreach ($e in $errs) { '    {0,6:N0} x {1}' -f $e.Count, ($e.Name.Substring(0, [Math]::Min(160, $e.Name.Length))) }
}
if ($Endpoints -contains 'POST /notes') {
    Write-Host ''
    Write-Host "Notes from this run start with 'load $runId' (Notes page of either portal)." -ForegroundColor DarkGray
}
