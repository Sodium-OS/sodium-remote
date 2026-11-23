Add-Type -AssemblyName System.Windows.Forms, System.Drawing
$d = $PSScriptRoot
Remove-Item "$d\clip-got.*", "$d\clip-formats.txt" -ErrorAction SilentlyContinue
$o = [System.Windows.Forms.Clipboard]::GetDataObject()
[IO.File]::WriteAllText("$d\clip-formats.txt", ($o.GetFormats() -join ','))
if ([System.Windows.Forms.Clipboard]::ContainsText()) {
    [IO.File]::WriteAllText("$d\clip-got.txt", [System.Windows.Forms.Clipboard]::GetText(), [Text.Encoding]::UTF8)
}
$i = [System.Windows.Forms.Clipboard]::GetImage()
if ($i) { $i.Save("$d\clip-got.png", [System.Drawing.Imaging.ImageFormat]::Png) }
