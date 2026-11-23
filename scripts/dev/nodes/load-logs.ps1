<#
.SYNOPSIS
    Load test for Forge's Seq-compatible log ingest (/api/events/raw).

.DESCRIPTION
    Starts many parallel senders. Each posts batches of CLEF events with a log
    ingest key, then the script prints throughput, latency percentiles and the
    status codes the server answered. Every event carries a RunId property, so
    one run can be found in Operations -> Logs with:  RunId="<id>"

    Works in Windows PowerShell 5.1 and PowerShell 7 (Windows, Linux, macOS).

    Create the ingest key in the console: open the service's page ->
    Log ingest keys -> New key. Pass it in $env:FORGE_LOG_KEY, or let the
    script ask for it (input is hidden). Do not put it on the command line.

.EXAMPLE
    # 50 senders x 20 requests x 100 events = 100,000 events
    .\load-logs.ps1

.EXAMPLE
    # Many small requests: 200 senders, 10 events each, for 60 seconds
    $env:FORGE_LOG_KEY = Read-Host 'Key'
    .\load-logs.ps1 -Senders 200 -EventsPerRequest 10 -DurationSeconds 60

.EXAMPLE
    # Attribute the events to a node of the service (node filter in Logs)
    .\load-logs.ps1 -MachineName dev-node-1
#>
[CmdletBinding()]
param(
    # Forge server base URL, without a path.
    [string]$ForgeUrl = 'http://127.0.0.1:55130',
    # Ingest key (flk_...). Default: $env:FORGE_LOG_KEY, else a hidden prompt.
    [string]$ApiKey = $env:FORGE_LOG_KEY,
    # Parallel senders (connections).
    [ValidateRange(1, 2000)][int]$Senders = 50,
    # Requests per sender. Ignored when -DurationSeconds is set.
    [ValidateRange(1, 1000000)][int]$RequestsPerSender = 20,
    # Run for this long instead of a fixed number of requests (0: off).
    [ValidateRange(0, 86400)][int]$DurationSeconds = 0,
    # Events in each request (Forge accepts at most 10,000).
    [ValidateRange(1, 10000)][int]$EventsPerRequest = 100,
    # Size of the filler property on each event, in bytes.
    [ValidateRange(0, 32000)][int]$PayloadBytes = 120,
    # MachineName property on each event. Use a node's hostname (dev-node-1)
    # to have Forge attribute the events to that node.
    [string]$MachineName = [Environment]::MachineName,
    # Per-request timeout.
    [ValidateRange(1, 600)][int]$TimeoutSeconds = 60
)

Set-StrictMode -Version 3
$ErrorActionPreference = 'Stop'

if (-not $ApiKey) {
    $secure = Read-Host 'Log ingest key (flk_...)' -AsSecureString
    $ApiKey = [Runtime.InteropServices.Marshal]::PtrToStringBSTR([Runtime.InteropServices.Marshal]::SecureStringToBSTR($secure))
}
if (-not $ApiKey.StartsWith('flk_')) { throw 'The ingest key starts with flk_. Create one on the service page under Log ingest keys.' }

Add-Type -AssemblyName System.Net.Http
# .NET Framework (PowerShell 5.1) allows 2 connections per host by default.
[System.Net.ServicePointManager]::DefaultConnectionLimit = [Math]::Max(100, $Senders * 2)
[System.Net.ServicePointManager]::Expect100Continue = $false

$url = $ForgeUrl.TrimEnd('/') + '/api/events/raw?clef'
$runId = '{0:yyyyMMdd-HHmmss}-{1}' -f (Get-Date), (Get-Random -Maximum 9999)
$client = [System.Net.Http.HttpClient]::new()
$client.Timeout = [TimeSpan]::FromSeconds($TimeoutSeconds)
$client.DefaultRequestHeaders.Add('X-Seq-ApiKey', $ApiKey)

function ConvertTo-JsonString([string]$s) {
    '"' + ($s -replace '\\', '\\' -replace '"', '\"' -replace "`r", '\r' -replace "`n", '\n' -replace "`t", '\t') + '"'
}

# One request first: a wrong key or URL stops here instead of 1,000 times.
$probe = '{"@t":"' + [DateTime]::UtcNow.ToString('o') + '","@mt":"Load test {RunId} starting","RunId":' + (ConvertTo-JsonString $runId) + '}'
try {
    $content = [System.Net.Http.StringContent]::new($probe, [Text.Encoding]::UTF8, 'application/vnd.serilog.clef')
    $r = $client.PostAsync($url, $content).GetAwaiter().GetResult()
} catch {
    throw "Cannot reach $url : $($_.Exception.GetBaseException().Message)"
}
switch ([int]$r.StatusCode) {
    201 { }
    401 { throw 'The server refused the key (401). Check that it is an active ingest key.' }
    503 { throw 'Log analytics is not available on the server (503).' }
    default { throw "Unexpected answer $([int]$r.StatusCode): $($r.Content.ReadAsStringAsync().GetAwaiter().GetResult())" }
}

$mode = if ($DurationSeconds -gt 0) { "for $DurationSeconds s" } else { "$RequestsPerSender requests each" }
Write-Host ''
Write-Host "Forge log ingest load test  RunId=$runId" -ForegroundColor Cyan
Write-Host "  target   $url"
Write-Host "  senders  $Senders ($mode), $EventsPerRequest events per request, ~$PayloadBytes B payload"
Write-Host "  node     MachineName=$MachineName"
Write-Host ''

# Shared between runspaces: progress counters and the stop time.
$shared = [hashtable]::Synchronized(@{ Requests = 0; Events = 0; Failed = 0 })
$deadline = if ($DurationSeconds -gt 0) { [DateTime]::UtcNow.AddSeconds($DurationSeconds) } else { [DateTime]::MaxValue }
$jsonRun = ConvertTo-JsonString $runId
$jsonMachine = ConvertTo-JsonString $MachineName
$filler = ConvertTo-JsonString ('x' * $PayloadBytes)

$worker = {
    param($client, $url, $shared, $sender, $requests, $deadline, $perRequest, $jsonRun, $jsonMachine, $filler)
    $results = [System.Collections.Generic.List[object]]::new()
    $sb = [System.Text.StringBuilder]::new($perRequest * 256)
    $levels = @('Information') * 90 + @('Warning') * 8 + @('Error') * 2
    $rnd = [Random]::new($sender * 7919 + [Environment]::TickCount)
    $req = 0
    while (($req -lt $requests) -and ([DateTime]::UtcNow -lt $deadline)) {
        $req++
        [void]$sb.Clear()
        $t = [DateTime]::UtcNow.ToString('o')
        for ($i = 0; $i -lt $perRequest; $i++) {
            $lvl = $levels[$rnd.Next($levels.Count)]
            [void]$sb.Append('{"@t":"').Append($t).Append('","@l":"').Append($lvl)
            [void]$sb.Append('","@mt":"Load test {RunId} sender {Sender} request {Request} event {Index}","RunId":').Append($jsonRun)
            [void]$sb.Append(',"Sender":').Append($sender).Append(',"Request":').Append($req).Append(',"Index":').Append($i)
            [void]$sb.Append(',"MachineName":').Append($jsonMachine).Append(',"Payload":').Append($filler).Append("}`n")
        }
        $content = [System.Net.Http.StringContent]::new($sb.ToString(), [Text.Encoding]::UTF8, 'application/vnd.serilog.clef')
        $sw = [System.Diagnostics.Stopwatch]::StartNew()
        $code = 0; $err = $null
        try {
            $resp = $client.PostAsync($url, $content).GetAwaiter().GetResult()
            $code = [int]$resp.StatusCode
            if ($code -ne 201) { $err = $resp.Content.ReadAsStringAsync().GetAwaiter().GetResult() }
            $resp.Dispose()
        } catch {
            $err = $_.Exception.GetBaseException().Message
        } finally {
            $content.Dispose()
        }
        $sw.Stop()
        $results.Add([pscustomobject]@{ Ms = $sw.Elapsed.TotalMilliseconds; Code = $code; Error = $err })
        [System.Threading.Monitor]::Enter($shared.SyncRoot)
        try {
            $shared.Requests++
            if ($code -eq 201) { $shared.Events += $perRequest } else { $shared.Failed++ }
        } finally { [System.Threading.Monitor]::Exit($shared.SyncRoot) }
    }
    , $results
}

$pool = [RunspaceFactory]::CreateRunspacePool(1, $Senders)
$pool.Open()
$jobs = foreach ($s in 1..$Senders) {
    $ps = [PowerShell]::Create()
    $ps.RunspacePool = $pool
    [void]$ps.AddScript($worker).AddArgument($client).AddArgument($url).AddArgument($shared).AddArgument($s).
        AddArgument($(if ($DurationSeconds -gt 0) { [int]::MaxValue } else { $RequestsPerSender })).AddArgument($deadline).
        AddArgument($EventsPerRequest).AddArgument($jsonRun).AddArgument($jsonMachine).AddArgument($filler)
    [pscustomobject]@{ PS = $ps; Handle = $ps.BeginInvoke() }
}

$clock = [System.Diagnostics.Stopwatch]::StartNew()
$planned = if ($DurationSeconds -gt 0) { 0 } else { $Senders * $RequestsPerSender }
try {
    while (@($jobs | Where-Object { -not $_.Handle.IsCompleted }).Count -gt 0) {
        Start-Sleep -Milliseconds 500
        $secs = [Math]::Max($clock.Elapsed.TotalSeconds, 0.001)
        $status = '{0:N0} requests, {1:N0} events stored, {2:N0} failed, {3:N0} ev/s' -f $shared.Requests, $shared.Events, $shared.Failed, ($shared.Events / $secs)
        $pct = if ($planned) { [Math]::Min(100, [int](100 * $shared.Requests / $planned)) } else { [Math]::Min(100, [int](100 * $secs / $DurationSeconds)) }
        Write-Progress -Activity "Load test $runId" -Status $status -PercentComplete $pct
    }
    $all = [System.Collections.Generic.List[object]]::new()
    foreach ($j in $jobs) { foreach ($r in $j.PS.EndInvoke($j.Handle)) { $all.AddRange([object[]]$r) } }
} finally {
    $clock.Stop()
    Write-Progress -Activity "Load test $runId" -Completed
    foreach ($j in $jobs) { $j.PS.Dispose() }
    $pool.Close(); $pool.Dispose()
    $client.Dispose()
}

# ------------------------------------------------------------------ summary
function Get-Percentile([double[]]$sorted, [double]$p) {
    if ($sorted.Count -eq 0) { return 0 }
    $sorted[[Math]::Min($sorted.Count - 1, [int][Math]::Floor($p * $sorted.Count))]
}
$secs = [Math]::Max($clock.Elapsed.TotalSeconds, 0.001)
$ok = @($all | Where-Object Code -eq 201)
$lat = [double[]]($all | ForEach-Object Ms | Sort-Object)
$sent = $all.Count * $EventsPerRequest
$stored = $ok.Count * $EventsPerRequest

Write-Host ''
Write-Host "Result  RunId=$runId" -ForegroundColor Cyan
'  {0,-18} {1:N1} s' -f 'duration', $secs
'  {0,-18} {1:N0} requests, {2:N0} events' -f 'sent', $all.Count, $sent
'  {0,-18} {1:N0} events ({2:P1})' -f 'stored (201)', $stored, $(if ($sent) { $stored / $sent } else { 0 })
'  {0,-18} {1:N0} events/s, {2:N0} requests/s' -f 'throughput', ($stored / $secs), ($all.Count / $secs)
'  {0,-18} p50 {1:N0} ms   p95 {2:N0} ms   p99 {3:N0} ms   max {4:N0} ms' -f 'latency', (Get-Percentile $lat 0.50), (Get-Percentile $lat 0.95), (Get-Percentile $lat 0.99), (Get-Percentile $lat 1.0)
Write-Host '  status codes'
$all | Group-Object Code | Sort-Object Name | ForEach-Object {
    $label = if ($_.Name -eq '0') { 'no answer' } else { $_.Name }
    $color = if ($_.Name -eq '201') { 'Green' } else { 'Yellow' }
    Write-Host ('    {0,-10} {1,8:N0}' -f $label, $_.Count) -ForegroundColor $color
}
$errs = @($all | Where-Object { $_.Error } | Group-Object Error | Sort-Object Count -Descending | Select-Object -First 5)
if ($errs.Count) {
    Write-Host '  most common errors' -ForegroundColor Yellow
    foreach ($e in $errs) { '    {0,6:N0} x {1}' -f $e.Count, ($e.Name.Substring(0, [Math]::Min(160, $e.Name.Length))) }
}
Write-Host ''
Write-Host "Find this run in Operations -> Logs with:  RunId=`"$runId`"" -ForegroundColor DarkGray
