# Measures a release build for PERFORMANCE.md: size, startup time, memory and idle CPU.
# Usage: pwsh scripts/measure.ps1 [-Exe path\to\winproc-gui.exe] [-Runs 7] [-IdleSeconds 20]
# Settings go to a throwaway APPDATA so your real settings and window position are not touched.
#
# Idle CPU is measured three ways: main view with GPU sampling on (the default), main view
# with it off, and the watch view watching "svchost" (default columns, so GPU is still read
# for the GPU % column). It is counted in CPU cycles (QueryProcessCycleTime), which is exact;
# Windows' CPU *time* only moves in 15.6 ms ticks and is too coarse at this level.
param(
    [string]$Exe = "$PSScriptRoot\..\target\release\winproc-gui.exe",
    [int]$Runs = 7,
    [int]$IdleSeconds = 20
)

Add-Type @"
using System; using System.Runtime.InteropServices;
public static class Cycles {
    [DllImport("kernel32.dll")] public static extern bool QueryProcessCycleTime(IntPtr h, out ulong c);
    public static ulong Of(IntPtr h) { ulong c; QueryProcessCycleTime(h, out c); return c; }
}
"@

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

function Measure-Idle([bool]$gpu, [string]$view) {
    Set-Content $ini "[settings]`r`ngpu = $($gpu.ToString().ToLower())`r`n"
    Set-Content "$tmp\winproc-gui\state.ini" "[window]`r`nview = $view`r`n[watch]`r`nsearch = svchost`r`n"
    $p = Start-Process $Exe -PassThru
    $null = $p.WaitForInputIdle(10000)
    Start-Sleep 5
    $c0 = [Cycles]::Of($p.Handle)
    Start-Sleep $IdleSeconds
    $c = [Cycles]::Of($p.Handle) - $c0
    $p.Refresh()
    $r = [pscustomobject]@{
        Mcps = $c / 1e6 / $IdleSeconds; Private = $p.PrivateMemorySize64 / 1MB; WorkingSet = $p.WorkingSet64 / 1MB
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

$on = Measure-Idle $true "main"
$off = Measure-Idle $false "main"
$watch = Measure-Idle $true "watch"
$s = $times | Sort-Object
"Binary size:          {0:N0} bytes" -f (Get-Item $Exe).Length
"Startup (ms):         median {0:N1}, min {1:N1}, max {2:N1} over $Runs runs" -f $s[[int]($Runs / 2)], $s[0], $s[-1]
"Idle CPU, GPU on:     {0:N1} Mcycles/s" -f $on.Mcps
"Idle CPU, GPU off:    {0:N1} Mcycles/s" -f $off.Mcps
"Idle CPU, watch view: {0:N1} Mcycles/s" -f $watch.Mcps
"Memory (MB):          private {0:N1}, working set {1:N1} (GPU on)" -f $on.Private, $on.WorkingSet
"Threads:              $($on.Threads), handles: $($on.Handles)"
