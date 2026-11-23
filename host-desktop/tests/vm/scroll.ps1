$host.UI.RawUI.WindowTitle = 'SRHD scroller'
$line = 'Sodium Remote scrolling test: the quick brown fox jumps over the lazy dog. 0123456789 '
$i = 0
while ($true) {
    $i++
    Write-Host ('{0,8}  {1}{1}' -f $i, $line)
}
