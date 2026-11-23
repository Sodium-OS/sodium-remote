# Runs Sodium Remote in the logged-on session (session 1) through a scheduled task, and a screenshot watcher beside it:
# every "remote-script: shot NAME" line in the app's log becomes <this folder>\shots\NAME.png.
# usage (over SSH): powershell -File run-app.ps1 [-AppArgs "--lan-only"] [-Lang en_US] [-Theme dark] [-Name app] [-Script file] [-Data dir]
param([string]$AppArgs = "", [string]$Lang = "en_US", [string]$Theme = "dark", [string]$Name = "app", [string]$Script = "", [string]$Data = "")
$root = $PSScriptRoot   # the guest's integration folder (run-*.sh copy this script there)
New-Item -ItemType Directory -Force "$root\out", "$root\shots" | Out-Null
Get-Process SodiumRemote -ErrorAction SilentlyContinue | Where-Object { $_.Path -like "$root\*" } | Stop-Process -Force
Get-CimInstance Win32_Process -Filter "Name='powershell.exe'" | Where-Object { $_.CommandLine -like "*shots.ps1*" } | ForEach-Object { Stop-Process -Id $_.ProcessId -Force }
Start-Sleep -Seconds 2
$dataArg = if ($Data) { "--data $Data" } else { "" }
$scriptArg = if ($Script) { "--script $Script" } else { "" }
$log = "$root\out\$Name.log"
Remove-Item $log -ErrorAction SilentlyContinue
$cmd = "set SODIUMOS_UI_LANGUAGE=$Lang&& set SODIUMOS_STATE=$root\state\$Theme&& set QT_FORCE_STDERR_LOGGING=1&& set SODIUM_REMOTE_HOST_DEBUG=1&& set SODIUM_REMOTE_TEST=1&& `"$root\SodiumRemote\SodiumRemote.exe`" $dataArg $scriptArg $AppArgs > `"$log`" 2>&1"
$a = New-ScheduledTaskAction -Execute "cmd.exe" -Argument "/c $cmd"
$t = New-ScheduledTaskTrigger -Once -At (Get-Date).AddMinutes(5)
$p = New-ScheduledTaskPrincipal -UserId ([Security.Principal.WindowsIdentity]::GetCurrent().Name) -LogonType Interactive -RunLevel Limited
if ($Script) {
    $w = New-ScheduledTaskAction -Execute "cmd.exe" -Argument "/c powershell -NoProfile -File $root\shots.ps1 -Log `"$log`" > $root\out\shots-watcher.txt 2>&1"
    Register-ScheduledTask -TaskName "RT-INT-shots" -Action $w -Trigger $t -Principal $p -Force | Out-Null
    Start-ScheduledTask -TaskName "RT-INT-shots"
}
Register-ScheduledTask -TaskName "RT-INT-app" -Action $a -Trigger $t -Principal $p -Force | Out-Null
Start-ScheduledTask -TaskName "RT-INT-app"
