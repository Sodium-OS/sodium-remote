# Stops this test's helper scripts (only ours: matched by script name), and the host with -Host.
param([switch]$Host_, [string[]]$Names = @('scroll.ps1', 'anim.ps1', 'tone.ps1'))
foreach ($n in $Names) {
    Get-CimInstance Win32_Process | Where-Object { $_.CommandLine -like "*\host-desktop\$n*" } |
        ForEach-Object { Stop-Process -Id $_.ProcessId -Force; "stopped $n ($($_.ProcessId))" }
}
if ($Host_) { Get-Process sodium-remote-host -ErrorAction SilentlyContinue | Stop-Process -Force; 'stopped host' }
