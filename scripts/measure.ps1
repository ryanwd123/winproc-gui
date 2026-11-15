# Measures a release build for PERFORMANCE.md: size, startup time, memory and idle CPU.
# Usage: pwsh scripts/measure.ps1 [-Exe path\to\winproc-gui.exe] [-Runs 7] [-IdleSeconds 20]
# Settings go to a throwaway APPDATA so your real settings and window position are not touched.
# Idle CPU is measured twice: with GPU sampling on (the default) and off.
param(
    [string]$Exe = "$PSScriptRoot\..\target\release\winproc-gui.exe",
    [int]$Runs = 7,
    [int]$IdleSeconds = 20
)

$tmp = Join-Path ([IO.Path]::GetTempPath()) "winproc-gui-measure"
Remove-Item -Recurse -Force $tmp -ErrorAction SilentlyContinue
New-Item -ItemType Directory -Force "$tmp\winproc-gui" | Out-Null
$env:APPDATA = $tmp
$ini = "$tmp\winproc-gui\settings.ini"

function Stop-App($p) {
    $null = $p.CloseMainWindow()
    $null = $p.WaitForExit(5000)
    if (!$p.HasExited) { $p.Kill() }
}

function Measure-Idle([bool]$gpu) {
    Set-Content $ini "[settings]`r`ngpu = $($gpu.ToString().ToLower())`r`n"
    $p = Start-Process $Exe -PassThru
    $null = $p.WaitForInputIdle(10000)
    Start-Sleep 5
    $p.Refresh()
    $cpu0 = $p.TotalProcessorTime
    Start-Sleep $IdleSeconds
    $p.Refresh()
    $cpu = ($p.TotalProcessorTime - $cpu0).TotalMilliseconds
    $r = [pscustomobject]@{
        Cpu = $cpu; Private = $p.PrivateMemorySize64 / 1MB; WorkingSet = $p.WorkingSet64 / 1MB
        Threads = $p.Threads.Count; Handles = $p.HandleCount
    }
    Stop-App $p
    $r
}

# Startup: process launch until the main window exists.
$times = @()
for ($i = 0; $i -lt $Runs; $i++) {
    $sw = [Diagnostics.Stopwatch]::StartNew()
    $p = Start-Process $Exe -PassThru
    $null = $p.WaitForInputIdle(10000)
    while ($p.MainWindowHandle -eq 0 -and $sw.ElapsedMilliseconds -lt 10000) {
        Start-Sleep -Milliseconds 1
        $p.Refresh()
    }
    $times += $sw.Elapsed.TotalMilliseconds
    Stop-App $p
}

$on = Measure-Idle $true
$off = Measure-Idle $false
$s = $times | Sort-Object
$pct = { param($ms) 100 * $ms / ($IdleSeconds * 1000) }
"Binary size:     {0:N0} bytes" -f (Get-Item $Exe).Length
"Startup (ms):    median {0:N1}, min {1:N1}, max {2:N1} over $Runs runs" -f $s[[int]($Runs / 2)], $s[0], $s[-1]
"Idle CPU, GPU on:  {0:N0} ms over $IdleSeconds s ({1:N2}% of one core)" -f $on.Cpu, (& $pct $on.Cpu)
"Idle CPU, GPU off: {0:N0} ms over $IdleSeconds s ({1:N2}% of one core)" -f $off.Cpu, (& $pct $off.Cpu)
"Memory (MB):     private {0:N1}, working set {1:N1} (GPU on)" -f $on.Private, $on.WorkingSet
"Threads:         $($on.Threads), handles: $($on.Handles)"
