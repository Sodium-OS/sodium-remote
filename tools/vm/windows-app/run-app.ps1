# Runs Sodium Remote in the logged-on session (session 1) through a scheduled task; its log goes to <this folder>\out\app.log.
# usage (over SSH): powershell -File run-app.ps1 [-AppArgs "--lan-only"] [-Lang en_US] [-Theme dark] [-Name app]
param([string]$AppArgs = "", [string]$Lang = "en_US", [string]$Theme = "dark", [string]$Name = "app", [string]$Data = (Join-Path $PSScriptRoot "data"))
$root = $PSScriptRoot   # the guest's windows-app folder (redeploy.sh fills it)
New-Item -ItemType Directory -Force "$root\out" | Out-Null
Get-Process SodiumRemote -ErrorAction SilentlyContinue | Stop-Process -Force
$cmd = "set SODIUMOS_UI_LANGUAGE=$Lang&& set SODIUMOS_STATE=$root\state\$Theme&& set QT_FORCE_STDERR_LOGGING=1&& set SODIUM_REMOTE_HOST_DEBUG=1&& `"$root\SodiumRemote\SodiumRemote.exe`" --data $Data $AppArgs > `"$root\out\$Name.log`" 2>&1"
$a = New-ScheduledTaskAction -Execute "cmd.exe" -Argument "/c $cmd"
$t = New-ScheduledTaskTrigger -Once -At (Get-Date).AddMinutes(5)
$p = New-ScheduledTaskPrincipal -UserId ([Security.Principal.WindowsIdentity]::GetCurrent().Name) -LogonType Interactive -RunLevel Limited
Register-ScheduledTask -TaskName "RT-APP-app" -Action $a -Trigger $t -Principal $p -Force | Out-Null
Start-ScheduledTask -TaskName "RT-APP-app"
