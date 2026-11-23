# Pair B test helpers for host-desktop: tasks that run in the interactive session
# (session 1) as the test account, unelevated, as a person's own programs would.
$dir = $PSScriptRoot   # the guest folder these helpers were copied to
$ps = 'powershell.exe'
$psa = '-NoProfile -ExecutionPolicy Bypass'

function T($name, $exe, $arg) {
    $a = New-ScheduledTaskAction -Execute $exe -Argument $arg -WorkingDirectory $dir
    $p = New-ScheduledTaskPrincipal -UserId ([Security.Principal.WindowsIdentity]::GetCurrent().Name) -LogonType Interactive -RunLevel Limited
    $s = New-ScheduledTaskSettingsSet -AllowStartIfOnBatteries -DontStopIfGoingOnBatteries `
        -ExecutionTimeLimit (New-TimeSpan -Hours 2) -MultipleInstances IgnoreNew
    Register-ScheduledTask -TaskName "SRHD-$name" -Action $a -Principal $p -Settings $s -Force | Out-Null
    "SRHD-$name"
}

T host    'cmd.exe' '/c start "" /min cmd /c run.cmd'
T list    'cmd.exe' '/c sodium-remote-host.exe --list > list.txt 2>&1'
T scroll  $ps "$psa -File $dir\scroll.ps1"
T anim    $ps "$psa -WindowStyle Hidden -File $dir\anim.ps1"
T notepad 'cmd.exe' '/c start "" /max notepad.exe'
T tone    $ps "$psa -WindowStyle Hidden -File $dir\tone.ps1"
T cliptext $ps "$psa -WindowStyle Hidden -File $dir\cliptext.ps1"
T clippng $ps "$psa -WindowStyle Hidden -File $dir\clippng.ps1"
T clipget $ps "$psa -WindowStyle Hidden -File $dir\clipget.ps1"
T uac     $ps "$psa -WindowStyle Hidden -Command Start-Process notepad.exe -Verb RunAs"
T lock    'rundll32.exe' 'user32.dll,LockWorkStation'
