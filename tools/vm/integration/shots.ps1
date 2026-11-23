# Screenshots of the interactive desktop on every "remote-script: shot NAME" line of the app's log (run in session 1).
param([string]$Log, [string]$Out = (Join-Path $PSScriptRoot "shots"), [int]$Seconds = 900)
Add-Type -AssemblyName System.Drawing, System.Windows.Forms
Add-Type @"
using System.Runtime.InteropServices;
public class Dpi { [DllImport("user32.dll")] public static extern bool SetProcessDPIAware(); }
"@
[Dpi]::SetProcessDPIAware() | Out-Null
$done = @{}
$end = (Get-Date).AddSeconds($Seconds)
while ((Get-Date) -lt $end) {
    if (Test-Path $Log) {
        $lines = Get-Content $Log -ErrorAction SilentlyContinue | Select-String "remote-script: shot (\S+)"
        foreach ($l in $lines) {
            $n = $l.Matches[0].Groups[1].Value
            if (-not $done[$n]) {
                $done[$n] = 1
                $b = [System.Windows.Forms.Screen]::PrimaryScreen.Bounds
                $bmp = New-Object System.Drawing.Bitmap $b.Width, $b.Height
                $g = [System.Drawing.Graphics]::FromImage($bmp)
                $g.CopyFromScreen($b.Location, [System.Drawing.Point]::Empty, $b.Size)
                $bmp.Save("$Out\$n.png", [System.Drawing.Imaging.ImageFormat]::Png)
                $g.Dispose(); $bmp.Dispose()
            }
        }
    }
    Start-Sleep -Milliseconds 100
}
