Add-Type -AssemblyName System.Windows.Forms, System.Drawing
# As browsers copy an image: "PNG" plus a bitmap.
$bytes = [IO.File]::ReadAllBytes((Join-Path $PSScriptRoot 'clip.png'))
$img = [System.Drawing.Image]::FromStream((New-Object IO.MemoryStream(, $bytes)))
$do = New-Object System.Windows.Forms.DataObject
$do.SetData('PNG', $false, (New-Object IO.MemoryStream(, $bytes)))
$do.SetImage($img)
[System.Windows.Forms.Clipboard]::SetDataObject($do, $true)
