# Lists Sodium Remote's top-level windows (class, title, visible, minimised, rect, owner, topmost, tool/app-window styles).
# usage: powershell -File windows.ps1 [-Move x,y,w,h]   (-Move first moves and sizes the stream frame)
param([int[]]$Move = @(), [string]$Min = "", [string]$Close = "")
Add-Type @"
using System; using System.Text; using System.Collections.Generic; using System.Runtime.InteropServices;
public class W {
  public delegate bool P(IntPtr h, IntPtr l);
  [DllImport("user32.dll")] public static extern bool EnumWindows(P p, IntPtr l);
  [DllImport("user32.dll")] public static extern uint GetWindowThreadProcessId(IntPtr h, out uint pid);
  [DllImport("user32.dll")] public static extern int GetClassName(IntPtr h, StringBuilder s, int n);
  [DllImport("user32.dll")] public static extern int GetWindowText(IntPtr h, StringBuilder s, int n);
  [DllImport("user32.dll")] public static extern bool IsWindowVisible(IntPtr h);
  [DllImport("user32.dll")] public static extern bool IsIconic(IntPtr h);
  [DllImport("user32.dll")] public static extern bool GetWindowRect(IntPtr h, out RECT r);
  [DllImport("user32.dll")] public static extern IntPtr GetWindow(IntPtr h, uint c);
  [DllImport("user32.dll")] public static extern int GetWindowLong(IntPtr h, int i);
  [DllImport("user32.dll")] public static extern bool SetWindowPos(IntPtr h, IntPtr a, int x, int y, int w, int hh, uint f);
  [DllImport("user32.dll")] public static extern bool ShowWindow(IntPtr h, int c);
  [DllImport("user32.dll")] public static extern bool SetProcessDPIAware();
  [DllImport("user32.dll")] public static extern IntPtr GetForegroundWindow();
  [DllImport("user32.dll")] public static extern IntPtr PostMessage(IntPtr h, uint m, IntPtr w, IntPtr l);
  [StructLayout(LayoutKind.Sequential)] public struct RECT { public int l, t, r, b; }
  public static List<IntPtr> All(uint pid) { var v = new List<IntPtr>(); EnumWindows((h, l) => { uint p; GetWindowThreadProcessId(h, out p); if (p == pid) v.Add(h); return true; }, IntPtr.Zero); return v; }
}
"@
[W]::SetProcessDPIAware() | Out-Null
$pid1 = (Get-Process SodiumRemote | Select-Object -First 1).Id
function Dump {
  foreach ($h in [W]::All($pid1)) {
    $c = New-Object Text.StringBuilder 100; [W]::GetClassName($h, $c, 100) | Out-Null
    $t = New-Object Text.StringBuilder 100; [W]::GetWindowText($h, $t, 100) | Out-Null
    $r = New-Object W+RECT; [W]::GetWindowRect($h, [ref]$r) | Out-Null
    $ex = [W]::GetWindowLong($h, -20); $st = [W]::GetWindowLong($h, -16)
    $own = [W]::GetWindow($h, 4)
    if (-not [W]::IsWindowVisible($h) -and $c.ToString() -notlike "*Quick*" -and $c.ToString() -notlike "SodiumRemoteStream*") { continue }
    "{0,-34} vis={1} min={2} rect=({3},{4} {5}x{6}) owner={7} topmost={8} toolwin={9} appwin={10} sysmenu={11}" -f $c, [W]::IsWindowVisible($h), [W]::IsIconic($h), $r.l, $r.t, ($r.r-$r.l), ($r.b-$r.t), ($own -ne [IntPtr]::Zero), (($ex -band 0x8) -ne 0), (($ex -band 0x80) -ne 0), (($ex -band 0x40000) -ne 0), (($st -band 0x80000) -ne 0)
  }
}
$frame = [W]::All($pid1) | Where-Object { $s = New-Object Text.StringBuilder 100; [W]::GetClassName($_, $s, 100) | Out-Null; $s.ToString() -eq "SodiumRemoteStreamFrame" } | Select-Object -First 1
if ($Move.Count -eq 4 -and $frame) { [W]::SetWindowPos($frame, [IntPtr]::Zero, $Move[0], $Move[1], $Move[2], $Move[3], 0x14) | Out-Null; Start-Sleep -Milliseconds 700 }
if ($Min -eq "min" -and $frame) { [W]::ShowWindow($frame, 6) | Out-Null; Start-Sleep -Milliseconds 800 }
if ($Min -eq "restore" -and $frame) { [W]::ShowWindow($frame, 9) | Out-Null; Start-Sleep -Milliseconds 800 }
if ($Close -eq "syscmd" -and $frame) { [W]::PostMessage($frame, 0x112, [IntPtr]0xF060, [IntPtr]0) | Out-Null; Start-Sleep -Milliseconds 1500 }
Dump
$fg = [W]::GetForegroundWindow(); $c = New-Object Text.StringBuilder 100; [W]::GetClassName($fg, $c, 100) | Out-Null; "foreground: $c"
