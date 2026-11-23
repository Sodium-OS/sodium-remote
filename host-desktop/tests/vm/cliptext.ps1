Add-Type -AssemblyName System.Windows.Forms
# "from Windows: " + U+00DC U+65E5 U+672C (escaped so the file stays ASCII for PowerShell 5.1)
$t = 'from Windows: ' + [char]0x00DC + [char]0x65E5 + [char]0x672C
[System.Windows.Forms.Clipboard]::SetText($t)
