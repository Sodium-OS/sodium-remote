# A full-screen window that scrolls text at the timer's rate (about 64 Hz): the
# 1080p frame-rate check needs a screen that changes faster than a console scrolls.
Add-Type -AssemblyName System.Windows.Forms, System.Drawing
$f = New-Object System.Windows.Forms.Form
$f.Text = 'SRHD anim'
$f.FormBorderStyle = 'None'
$f.WindowState = 'Maximized'
$f.BackColor = [System.Drawing.Color]::White
$f.GetType().GetProperty('DoubleBuffered', [Reflection.BindingFlags]'NonPublic,Instance').SetValue($f, $true)
$font = New-Object System.Drawing.Font('Consolas', 12)
$script:off = 0
$script:paints = 0
$sw = [Diagnostics.Stopwatch]::StartNew()
$lines = 0..63 | ForEach-Object { ('{0,4}  Sodium Remote frame-rate check: the quick brown fox jumps over the lazy dog. 0123456789 ' -f $_) * 2 }
$f.Add_Paint({
    param($s, $e)
    $g = $e.Graphics
    $script:paints++
    if ($sw.ElapsedMilliseconds -ge 2000) { [IO.File]::WriteAllText((Join-Path $PSScriptRoot 'anim-fps.txt'), ('{0:F1} paints/s' -f ($script:paints * 1000.0 / $sw.ElapsedMilliseconds))); $script:paints = 0; $sw.Restart() }
    $y0 = - ($script:off % 20)
    $first = [int][Math]::Floor($script:off / 20)
    for ($i = 0; $i -lt 56; $i++) {
        $g.DrawString($lines[($first + $i) % 64], $font, [System.Drawing.Brushes]::Black, 8, $y0 + $i * 20)
    }
    $g.FillRectangle([System.Drawing.Brushes]::SteelBlue, ($script:off * 7) % 1800, 980, 120, 80)
})
$t = New-Object System.Windows.Forms.Timer
$t.Interval = 1
$t.Add_Tick({ $script:off += 3; $f.Invalidate() })
$t.Start()
[System.Windows.Forms.Application]::Run($f)
