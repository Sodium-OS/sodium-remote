# Runs on the Windows guest, in the logged-on session (started by a scheduled task, see vm-gallery-shots.sh).
# params: name, language, theme (dark|light), page, extra args. Writes <this folder>\out\<name>.png and .log
param([string]$Name, [string]$Lang, [string]$Theme, [string]$Page, [string]$Extra = "")
$root = $PSScriptRoot   # the guest folder the gallery was copied to
New-Item -ItemType Directory -Force "$root\out" | Out-Null
$env:SODIUMOS_UI_LANGUAGE = $Lang
$env:SODIUMOS_STATE = "$root\state\$Theme"
$env:QT_FORCE_STDERR_LOGGING = "1"
$env:QSG_INFO = "1"
Remove-Item Env:QT_OPENGL, Env:QSG_RHI_BACKEND -ErrorAction SilentlyContinue
$args = @("--page=$Page", "--grab=$root\out\$Name.png") + ($Extra -split " " | Where-Object { $_ })
$p = Start-Process -FilePath "$root\gallery\sodiumui-gallery.exe" -ArgumentList $args -RedirectStandardError "$root\out\$Name.log" -PassThru
if (-not $p.WaitForExit(60000)) { $p.Kill() }
"exit $($p.ExitCode)" | Out-File -Append "$root\out\$Name.log"
