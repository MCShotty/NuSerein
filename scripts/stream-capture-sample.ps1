#requires -Version 5.1
<#
.SYNOPSIS
Owner-operated game/frame-time and GPU-engine sample during one explicit capture scenario.
.DESCRIPTION
Observes only the supplied game/capture PIDs; never starts a game, changes Serein,
starts a Discord session, selects a source, or stops either observed process.
PresentMon 2.x must be supplied by its exact executable path. Only the PresentMon
process and temporary files started by this invocation are retired on failure.
Use the same scene/settings and one warmup followed by five alternating before/after pairs.
.EXAMPLE
powershell -NoProfile -File scripts/stream-capture-sample.ps1 -OwnerControlled `
  -PresentMon C:\Tools\PresentMon.exe -GameProcessId 1234 -CaptureProcessId 5678 `
  -Scenario full-sharing -Revision 30d3437f -Label '1440p60 SDR H264, same scene'
#>
[CmdletBinding()]
param(
    [switch] $OwnerControlled,
    [Parameter(Mandatory = $true)] [string] $PresentMon,
    [Parameter(Mandatory = $true)] [ValidateRange(1, 2147483647)] [int] $GameProcessId,
    [ValidateRange(0, 2147483647)] [int] $CaptureProcessId = 0,
    [Parameter(Mandatory = $true)] [ValidateSet('no-share', 'capture-only', 'readback', 'encode', 'full-sharing')] [string] $Scenario,
    [Parameter(Mandatory = $true)] [ValidatePattern('^[0-9a-fA-F]{7,40}$')] [string] $Revision,
    [Parameter(Mandatory = $true)] [ValidateLength(1, 256)] [string] $Label,
    [ValidateRange(0, 60)] [int] $WarmupSeconds = 10,
    [ValidateRange(5, 120)] [int] $SampleSeconds = 30,
    [ValidateLength(1, 64)] [string] $SwapChain,
    [switch] $SkipGpuCounters
)
$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest
if (-not $OwnerControlled) { throw 'Pass -OwnerControlled only for your own game and explicitly selected capture source.' }
if ($Scenario -ne 'no-share' -and $CaptureProcessId -eq 0) { throw 'Supply the exact capture/probe PID for this scenario.' }
if ($GameProcessId -eq $CaptureProcessId) { throw 'Game and capture must be different processes.' }
$presentMonPath = (Resolve-Path -LiteralPath $PresentMon).ProviderPath
if (-not [IO.File]::Exists($presentMonPath) -or [IO.Path]::GetExtension($presentMonPath) -ne '.exe') {
    throw 'PresentMon must be the exact prebuilt PresentMon 2.x executable path.'
}
$game = Get-Process -Id $GameProcessId
$capture = $(if ($CaptureProcessId -ne 0) { Get-Process -Id $CaptureProcessId } else { $null })
$gameStart = $game.StartTime.ToUniversalTime()
$captureStart = $(if ($null -ne $capture) { $capture.StartTime.ToUniversalTime() } else { $null })
function Assert-ObservedProcesses {
    $game.Refresh()
    if ($game.HasExited -or $game.StartTime.ToUniversalTime() -ne $gameStart) { throw 'The selected game exited or its PID changed; discard this run.' }
    if ($null -ne $capture) {
        $capture.Refresh()
        if ($capture.HasExited -or $capture.StartTime.ToUniversalTime() -ne $captureStart) { throw 'The selected capture exited or its PID changed; discard this run.' }
    }
}
# Warmup is intentionally outside the measured PresentMon window.
$warmup = [Diagnostics.Stopwatch]::StartNew()
while ($warmup.Elapsed.TotalSeconds -lt $WarmupSeconds) {
    Assert-ObservedProcesses
    Start-Sleep -Milliseconds 250
}
$tag = [Guid]::NewGuid().ToString('N')
$csvPath = Join-Path ([IO.Path]::GetTempPath()) "serein-capture-$tag.csv"
$outPath = Join-Path ([IO.Path]::GetTempPath()) "serein-capture-$tag.stdout"
$errorPath = Join-Path ([IO.Path]::GetTempPath()) "serein-capture-$tag.stderr"
$presentProcess = $null
$gpu = @{}
$gpuFailure = $(if ($SkipGpuCounters) { 'Disabled explicitly; GPU activity unmeasured.' } else { $null })
$ticks = 0
$memory = @{}
$startedUtc = [DateTime]::UtcNow
try {
    Assert-ObservedProcesses
    $gameCpu = $game.TotalProcessorTime.TotalSeconds
    $captureCpu = $(if ($null -ne $capture) { $capture.TotalProcessorTime.TotalSeconds } else { 0 })
    # The temporary path contains no quote. Arguments are data passed directly to PresentMon.
    $arguments = @('--process_id', [string]$GameProcessId, '--output_file', ('"' + $csvPath + '"'),
        '--timed', [string]$SampleSeconds, '--terminate_after_timed', '--no_console_stats',
        '--session_name', "SereinCapture_$tag")
    $presentProcess = Start-Process -FilePath $presentMonPath -ArgumentList $arguments `
        -RedirectStandardOutput $outPath -RedirectStandardError $errorPath -PassThru -NoNewWindow
    $clock = [Diagnostics.Stopwatch]::StartNew()
    while (-not $presentProcess.HasExited) {
        Assert-ObservedProcesses
        if ($clock.Elapsed.TotalSeconds -gt ($SampleSeconds + 15)) { throw 'PresentMon did not finish in time; discard this run.' }
        foreach ($path in @($outPath, $errorPath)) {
            if ([IO.File]::Exists($path) -and (Get-Item -LiteralPath $path).Length -gt 65536) { throw 'PresentMon diagnostic output exceeded its limit.' }
        }
        if ([IO.File]::Exists($csvPath) -and (Get-Item -LiteralPath $csvPath).Length -gt 33554432) { throw 'PresentMon CSV exceeded 32 MiB.' }
        foreach ($process in @($game, $capture)) {
            if ($null -eq $process) { continue }
            $key = [string]$process.Id
            if (-not $memory.ContainsKey($key)) { $memory[$key] = @{ working_set_peak = 0L; private_peak = 0L } }
            $memory[$key].working_set_peak = [Math]::Max($memory[$key].working_set_peak, $process.WorkingSet64)
            $memory[$key].private_peak = [Math]::Max($memory[$key].private_peak, $process.PrivateMemorySize64)
        }
        if ($null -eq $gpuFailure) {
            try {
                $engines = Get-CimInstance -ClassName Win32_PerfFormattedData_GPUPerformanceCounters_GPUEngine -OperationTimeoutSec 2
                foreach ($engine in $engines) {
                    if ($engine.Name -notmatch '^pid_(\d+)_(.*)$') { continue }
                    $engineProcessId = [int]$Matches[1]
                    $engineName = $Matches[2]
                    if ($engineProcessId -ne $GameProcessId -and $engineProcessId -ne $CaptureProcessId) { continue }
                    $key = [string]$engineProcessId + ':' + $engineName
                    if (-not $gpu.ContainsKey($key)) {
                        if ($gpu.Count -ge 128) { throw 'More than 128 GPU engine contexts; activity cannot be represented by this bounded sample.' }
                        $gpu[$key] = @{ sum = 0.0; maximum = 0.0 }
                    }
                    $value = [double]$engine.UtilizationPercentage
                    if ([double]::IsNaN($value) -or [double]::IsInfinity($value) -or $value -lt 0) { throw 'Invalid GPU counter value.' }
                    $gpu[$key].sum += $value
                    $gpu[$key].maximum = [Math]::Max($gpu[$key].maximum, $value)
                }
            } catch { $gpuFailure = $_.Exception.Message; $gpu.Clear() }
        }
        $ticks++
        Start-Sleep -Milliseconds 1000
        $presentProcess.Refresh()
    }
    $clock.Stop()
    Assert-ObservedProcesses
    if ($presentProcess.ExitCode -ne 0) {
        $details = ''
        if ([IO.File]::Exists($errorPath)) {
            $diagnosticsReader = New-Object IO.StreamReader($errorPath)
            try {
                $diagnosticBuffer = New-Object char[] 4096
                $diagnosticCount = $diagnosticsReader.Read($diagnosticBuffer, 0, $diagnosticBuffer.Length)
                if ($diagnosticCount -gt 0) { $details = -join $diagnosticBuffer[0..($diagnosticCount - 1)] }
            } finally { $diagnosticsReader.Dispose() }
        }
        throw "PresentMon failed ($($presentProcess.ExitCode)). Verify its 2.x command-line support. $details"
    }
    if (-not [IO.File]::Exists($csvPath)) { throw 'PresentMon produced no CSV.' }
    if ((Get-Item -LiteralPath $csvPath).Length -gt 33554432) { throw 'PresentMon CSV exceeded 32 MiB.' }
    $frames = New-Object 'Collections.Generic.List[double]'
    $chains = New-Object 'Collections.Generic.HashSet[string]'
    $reader = New-Object IO.StreamReader($csvPath)
    $field = $null
    try {
        $header = $reader.ReadLine()
        if ($null -eq $header -or $header.Length -gt 8192) { throw 'Missing/oversized PresentMon CSV header.' }
        $lines = 0
        while ($null -ne ($line = $reader.ReadLine())) {
            if ($line.Length -gt 8192 -or ++$lines -gt 500000) { throw 'PresentMon CSV exceeds the bounded frame sample.' }
            $row = ($header + "`n" + $line) | ConvertFrom-Csv
            if ($row.PSObject.Properties.Name -notcontains 'ProcessID' -or [int]$row.ProcessID -ne $GameProcessId) { throw 'PresentMon CSV does not identify the selected game PID.' }
            if ($row.PSObject.Properties.Name -notcontains 'SwapChainAddress') { throw 'PresentMon CSV is missing swap-chain identity.' }
            $chain = [string]$row.SwapChainAddress
            if ($chain.Length -gt 64) { throw 'Invalid swap-chain identity.' }
            $null = $chains.Add($chain)
            if ($chains.Count -gt 128) { throw 'Too many swap chains in the bounded sample.' }
            if ($SwapChain -and $chain -ne $SwapChain) { continue }
            if ($null -eq $field) {
                if ($row.PSObject.Properties.Name -contains 'MsBetweenPresents') { $field = 'MsBetweenPresents' }
                else { throw 'PresentMon CSV is missing MsBetweenPresents; use the console application with its documented default metrics.' }
            }
            $value = 0.0
            if (-not [double]::TryParse([string]$row.$field, [Globalization.NumberStyles]::Float,
                [Globalization.CultureInfo]::InvariantCulture, [ref]$value)) { throw 'Invalid frame interval.' }
            if ($value -gt 0 -and -not [double]::IsNaN($value) -and -not [double]::IsInfinity($value)) { $frames.Add($value) }
        }
    } finally { $reader.Dispose() }
    if (-not $SwapChain -and $chains.Count -ne 1) { throw "Multiple/no game swap chains observed. Repeat with -SwapChain using one of: $($chains -join ', ')" }
    if ($frames.Count -lt 100) { throw 'Fewer than 100 valid frames; discard this run.' }
    $frames.Sort()
    $sum = 0.0
    foreach ($value in $frames) { $sum += $value }
    $slowCount = [Math]::Max(1, [int][Math]::Ceiling($frames.Count / 100.0))
    $slowSum = 0.0
    for ($index = $frames.Count - $slowCount; $index -lt $frames.Count; $index++) { $slowSum += $frames[$index] }
    $processes = @()
    foreach ($process in @($game, $capture)) {
        if ($null -eq $process) { continue }
        $process.Refresh()
        $initialCpu = $(if ($process.Id -eq $GameProcessId) { $gameCpu } else { $captureCpu })
        $processes += [ordered]@{
            id = $process.Id
            cpu_percent_one_core = 100 * ($process.TotalProcessorTime.TotalSeconds - $initialCpu) / $clock.Elapsed.TotalSeconds
            working_set_settled_bytes = $process.WorkingSet64
            private_settled_bytes = $process.PrivateMemorySize64
            sampled_peaks = $memory[[string]$process.Id]
        }
    }
    $gpuSummary = @()
    foreach ($entry in $gpu.GetEnumerator() | Sort-Object Key) {
        $gpuSummary += [ordered]@{ process_and_engine = $entry.Key; mean_utilization_percent = $entry.Value.sum / [Math]::Max(1, $ticks); peak_utilization_percent = $entry.Value.maximum }
    }
    $captureHash = $null
    $captureIdentityUnavailable = $null
    if ($null -ne $capture) {
        try { $captureHash = (Get-FileHash -LiteralPath $capture.MainModule.FileName -Algorithm SHA256).Hash }
        catch { $captureIdentityUnavailable = $_.Exception.Message }
    }
    [ordered]@{
        scenario = $Scenario; revision_label = $Revision; label = $Label; started_utc = $startedUtc.ToString('o')
        capture_executable_sha256 = $captureHash; capture_identity_unavailable = $captureIdentityUnavailable
        warmup_seconds = $WarmupSeconds; requested_sample_seconds = $SampleSeconds; observer_elapsed_seconds = $clock.Elapsed.TotalSeconds
        presentmon_sha256 = (Get-FileHash -LiteralPath $presentMonPath -Algorithm SHA256).Hash
        frame_interval_field = $field; frame_count = $frames.Count; swap_chain = $(if ($SwapChain) { $SwapChain } else { @($chains)[0] })
        average_present_fps = 1000 * $frames.Count / $sum
        one_percent_low_present_fps = 1000 * $slowCount / $slowSum
        frame_interval_p95_ms = $frames[[int][Math]::Ceiling(0.95 * $frames.Count) - 1]
        frame_interval_p99_ms = $frames[[int][Math]::Ceiling(0.99 * $frames.Count) - 1]
        processes = $processes; gpu_engine_samples = $ticks; gpu_unavailable = $gpuFailure; gpu_engines = $gpuSummary
        method = 'PresentMon game present intervals; 1 Hz observer CPU/memory/GPU counters. GPU contexts stay separate. Does not measure physical display latency or receiver delivery.'
    } | ConvertTo-Json -Depth 6
} finally {
    if ($null -ne $presentProcess) {
        if (-not $presentProcess.HasExited) { $presentProcess.Kill(); $null = $presentProcess.WaitForExit(5000) }
        $presentProcess.Dispose()
    }
    # These random files belong solely to this invocation; neither observed process is stopped.
    foreach ($path in @($csvPath, $outPath, $errorPath)) { if ([IO.File]::Exists($path)) { Remove-Item -LiteralPath $path } }
    $game.Dispose()
    if ($null -ne $capture) { $capture.Dispose() }
}
