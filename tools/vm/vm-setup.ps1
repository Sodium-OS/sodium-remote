# Runs once at the first logon of the test account (from the unattend ISO; create.sh stages vm-config.ps1 beside it).
# Static address on the bridge NIC, OpenSSH Server with key auth, firewall, power and update policy.
$ErrorActionPreference = 'Continue'
$here = Split-Path -Parent $MyInvocation.MyCommand.Path
. (Join-Path $here 'vm-config.ps1')   # $Cfg: Dir (working folder), Net (first three octets), Mac (bridge NIC)
$dir = $Cfg.Dir
$net = $Cfg.Net
$mac = $Cfg.Mac.ToUpper().Replace(':', '-')
New-Item -ItemType Directory -Force $dir | Out-Null
Start-Transcript -Path "$dir\setup.log" -Append

function Log($m) { Write-Host ("[{0}] {1}" -f (Get-Date -Format s), $m) }

# 1. Bridge NIC (MAC from vm-config.ps1): static <Net>.10/24, no gateway, Private profile.
$ad = $null
for ($i = 0; $i -lt 60 -and -not $ad; $i++) {
    $ad = Get-NetAdapter | Where-Object { $_.MacAddress -eq $mac }
    if (-not $ad) { Start-Sleep 5 }
}
if ($ad) {
    Log "bridge NIC: $($ad.Name)"
    Rename-NetAdapter -Name $ad.Name -NewName 'Bridge' -ErrorAction SilentlyContinue
    Set-NetIPInterface -InterfaceAlias 'Bridge' -Dhcp Disabled -ErrorAction SilentlyContinue
    Get-NetIPAddress -InterfaceAlias 'Bridge' -AddressFamily IPv4 -ErrorAction SilentlyContinue | Remove-NetIPAddress -Confirm:$false -ErrorAction SilentlyContinue
    New-NetIPAddress -InterfaceAlias 'Bridge' -IPAddress "$net.10" -PrefixLength 24 -ErrorAction SilentlyContinue | Out-Null
    Set-NetIPInterface -InterfaceAlias 'Bridge' -InterfaceMetric 5000
}
$na = Get-NetAdapter | Where-Object { $_.MacAddress -eq '52-54-00-12-34-56' }
if ($na) { Rename-NetAdapter -Name $na.Name -NewName 'Internet' -ErrorAction SilentlyContinue }

# The bridge has no gateway, so Windows files it under "Unidentified network" (Public).
# A startup task keeps it Private.
@'
for ($i = 0; $i -lt 120; $i++) {
    Get-NetConnectionProfile | Where-Object { $_.InterfaceAlias -eq 'Bridge' -and $_.NetworkCategory -ne 'Private' } |
        ForEach-Object { Set-NetConnectionProfile -InterfaceIndex $_.InterfaceIndex -NetworkCategory Private }
    Start-Sleep 5
}
'@ | Set-Content -Encoding ASCII "$dir\private-net.ps1"
$act = New-ScheduledTaskAction -Execute 'powershell.exe' -Argument "-NoProfile -ExecutionPolicy Bypass -File $dir\private-net.ps1"
Register-ScheduledTask -TaskName 'RemoteTestBridgePrivate' -Action $act -Trigger (New-ScheduledTaskTrigger -AtStartup) -User 'SYSTEM' -RunLevel Highest -Force | Out-Null
Start-ScheduledTask -TaskName 'RemoteTestBridgePrivate'

# 2. Firewall: ICMP, mDNS, and anything from the bridge subnet, on every profile.
New-NetFirewallRule -DisplayName 'Remote test ICMPv4 in' -Direction Inbound -Protocol ICMPv4 -IcmpType 8 -Action Allow -Profile Any | Out-Null
New-NetFirewallRule -DisplayName 'Remote test mDNS in' -Direction Inbound -Protocol UDP -LocalPort 5353 -Action Allow -Profile Any | Out-Null
New-NetFirewallRule -DisplayName 'Remote test bridge TCP in' -Direction Inbound -Protocol TCP -RemoteAddress "$net.0/24" -Action Allow -Profile Any | Out-Null
New-NetFirewallRule -DisplayName 'Remote test bridge UDP in' -Direction Inbound -Protocol UDP -RemoteAddress "$net.0/24" -Action Allow -Profile Any | Out-Null

# 3. Power, lock screen, updates.
powercfg /change monitor-timeout-ac 0
powercfg /change monitor-timeout-dc 0
powercfg /change standby-timeout-ac 0
powercfg /change standby-timeout-dc 0
powercfg /change hibernate-timeout-ac 0
powercfg /hibernate off
powercfg /setacvalueindex SCHEME_CURRENT SUB_NONE CONSOLELOCK 0
powercfg /setdcvalueindex SCHEME_CURRENT SUB_NONE CONSOLELOCK 0
powercfg /setactive SCHEME_CURRENT
function RegSet($path, $name, $value, $type = 'DWord') {
    New-Item -Path $path -Force | Out-Null
    New-ItemProperty -Path $path -Name $name -Value $value -PropertyType $type -Force | Out-Null
}
RegSet 'HKLM:\SOFTWARE\Policies\Microsoft\Windows\Personalization' NoLockScreen 1
RegSet 'HKLM:\SOFTWARE\Policies\Microsoft\Windows\Control Panel\Desktop' ScreenSaveActive '0' String
RegSet 'HKLM:\SOFTWARE\Policies\Microsoft\Windows\System' DisableLockWorkstation 0
Remove-ItemProperty -Path 'HKLM:\SOFTWARE\Microsoft\Windows\CurrentVersion\Policies\System' -Name InactivityTimeoutSecs -ErrorAction SilentlyContinue
# 4. OpenSSH Server (needs the Internet NIC for the optional feature). Updates are paused only after it.
Remove-Item 'HKLM:\SOFTWARE\Policies\Microsoft\Windows\WindowsUpdate' -Recurse -Force -ErrorAction SilentlyContinue
Remove-Item 'HKLM:\SOFTWARE\Microsoft\WindowsUpdate\UX\Settings' -Recurse -Force -ErrorAction SilentlyContinue
function Have-Sshd { [bool](Get-Service sshd -ErrorAction SilentlyContinue) }
# The optional-feature download goes through Windows Update and can hang: give it 8 minutes.
$job = Start-Job { Add-WindowsCapability -Online -Name 'OpenSSH.Server~~~~0.0.1.0' }
if (-not (Wait-Job $job -Timeout 480)) { Log 'capability install timed out'; Stop-Job $job }
Remove-Job $job -Force -ErrorAction SilentlyContinue
if (-not (Have-Sshd)) {
    # Fallback: Microsoft's own Win32-OpenSSH release.
    Log 'falling back to the PowerShell/Win32-OpenSSH release'
    $zip = "$dir\OpenSSH-Win64.zip"
    Invoke-WebRequest -UseBasicParsing 'https://github.com/PowerShell/Win32-OpenSSH/releases/latest/download/OpenSSH-Win64.zip' -OutFile $zip
    Expand-Archive $zip -DestinationPath 'C:\Program Files' -Force
    Rename-Item 'C:\Program Files\OpenSSH-Win64' 'C:\Program Files\OpenSSH' -ErrorAction SilentlyContinue
    & powershell.exe -NoProfile -ExecutionPolicy Bypass -File 'C:\Program Files\OpenSSH\install-sshd.ps1'
}
Set-Service sshd -StartupType Automatic
Start-Service sshd
Start-Sleep 3
New-Item -Path 'HKLM:\SOFTWARE\OpenSSH' -Force | Out-Null
New-ItemProperty -Path 'HKLM:\SOFTWARE\OpenSSH' -Name DefaultShell -Value 'C:\Windows\System32\WindowsPowerShell\v1.0\powershell.exe' -PropertyType String -Force | Out-Null
$key = (Get-Content (Join-Path $here 'test_vm.pub') -Raw).Trim()
$ak = "$env:ProgramData\ssh\administrators_authorized_keys"
New-Item -ItemType Directory -Force "$env:ProgramData\ssh" | Out-Null
[IO.File]::WriteAllText($ak, "$key`n")
icacls.exe $ak /inheritance:r /grant 'SYSTEM:F' /grant 'Administrators:F' | Out-Null
Restart-Service sshd
# Scripts may run (RemoteSigned), and `ver` works in the PowerShell SSH shell.
Set-ExecutionPolicy RemoteSigned -Scope LocalMachine -Force
Set-Content -Path "$PSHOME\profile.ps1" -Value 'function ver { cmd /c ver }' -Encoding ASCII
if (-not (Get-NetFirewallRule -Name 'OpenSSH-Server-In-TCP' -ErrorAction SilentlyContinue)) {
    New-NetFirewallRule -Name 'OpenSSH-Server-In-TCP' -DisplayName 'OpenSSH Server (sshd)' -Direction Inbound -Protocol TCP -LocalPort 22 -Action Allow -Profile Any | Out-Null
}

# Windows Update stays on, but never reboots a signed-in session and only notifies.
$au = 'HKLM:\SOFTWARE\Policies\Microsoft\Windows\WindowsUpdate\AU'
RegSet $au NoAutoRebootWithLoggedOnUsers 1
RegSet $au AUOptions 2
RegSet 'HKLM:\SOFTWARE\Policies\Microsoft\Windows\WindowsUpdate' SetActiveHours 1
RegSet 'HKLM:\SOFTWARE\Policies\Microsoft\Windows\WindowsUpdate' ActiveHoursStart 0
RegSet 'HKLM:\SOFTWARE\Policies\Microsoft\Windows\WindowsUpdate' ActiveHoursEnd 18
# Pause updates for 5 weeks (the Settings > Windows Update "Pause" values).
$ux = 'HKLM:\SOFTWARE\Microsoft\WindowsUpdate\UX\Settings'
$now = (Get-Date).ToUniversalTime(); $end = $now.AddDays(35).ToString('yyyy-MM-ddTHH:mm:ssZ')
RegSet $ux PauseUpdatesStartTime $now.ToString('yyyy-MM-ddTHH:mm:ssZ') String
RegSet $ux PauseUpdatesExpiryTime $end String
RegSet $ux PauseFeatureUpdatesStartTime $now.ToString('yyyy-MM-ddTHH:mm:ssZ') String
RegSet $ux PauseFeatureUpdatesEndTime $end String
RegSet $ux PauseQualityUpdatesStartTime $now.ToString('yyyy-MM-ddTHH:mm:ssZ') String
RegSet $ux PauseQualityUpdatesEndTime $end String
Remove-ItemProperty -Path 'HKLM:\SOFTWARE\Microsoft\Windows NT\CurrentVersion\Winlogon' -Name AutoLogonCount -ErrorAction SilentlyContinue

Set-Content "$dir\setup.done" (Get-Date -Format s)
Log 'setup done'
Stop-Transcript
