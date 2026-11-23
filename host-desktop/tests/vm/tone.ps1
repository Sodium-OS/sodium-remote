$p = New-Object System.Media.SoundPlayer (Join-Path $PSScriptRoot 'tone.wav')
$p.PlaySync()
