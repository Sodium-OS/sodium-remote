#!/usr/bin/env bash
# SPDX-License-Identifier: AGPL-3.0-only
# SPDX-FileCopyrightText: 2026 Ning Bao
# Runs windows.ps1 in the guest's interactive session (a scheduled task) with the given arguments and prints its output.
#   winq.sh [-Move x,y,w,h] [-Min min|restore]      (VM lock held)
here=$(cd "$(dirname "$0")" && pwd)
# shellcheck disable=SC1091
source "$here/../config.sh"
WIN=${WIN:-$here/../win.sh}
$WIN scp-to "$here/windows.ps1" "$GF/integration/windows.ps1" >/dev/null
$WIN ssh "Remove-Item $GD\\integration\\out\\windows.txt -ErrorAction SilentlyContinue; \$a = New-ScheduledTaskAction -Execute powershell.exe -Argument '-NoProfile -Command \"& $GD\\integration\\windows.ps1 $* *> $GD\\integration\\out\\windows.txt\"'; \$p = New-ScheduledTaskPrincipal -UserId \$(\$env:COMPUTERNAME + '\\' + \$env:USERNAME) -LogonType Interactive -RunLevel Limited; Register-ScheduledTask -TaskName RT-INT-win -Action \$a -Principal \$p -Force | Out-Null; Start-ScheduledTask -TaskName RT-INT-win; Start-Sleep 5" >/dev/null
$WIN ssh "Get-Content $GD\\integration\\out\\windows.txt"
