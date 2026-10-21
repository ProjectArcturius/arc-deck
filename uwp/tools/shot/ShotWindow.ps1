<#
  Screenshot the Saiku Deck window by its own rectangle, not the whole screen.

  Why this exists: a full-screen capture grabs whatever is on top, and this machine
  sometimes runs a full-screen game that refuses to give up the foreground --
  SetForegroundWindow is ignored and even SW_MINIMIZE on the blocker does not help.
  Worse, that failure is silent: you get a picture of the game and might conclude
  the app regressed.

  So instead of competing for the foreground:
    1. put the Saiku Deck window at the top of the Z order with HWND_TOPMOST
    2. capture only its rectangle
    3. restore the previous Z order

  The window does not need focus to be captured this way, and the result is a
  tighter image (no desktop wallpaper, no taskbar).

  Usage:
    powershell.exe -File ShotWindow.ps1 -Out shot.png
    powershell.exe -File ShotWindow.ps1 -Max -Out shot.png
#>
param(
  [Parameter(Mandatory=$true)][string]$Out,
  [switch]$Max,
  [int]$SettleMs = 900,
  [int]$Pad = 0
)

Add-Type -TypeDefinition @"
using System;
using System.Runtime.InteropServices;
public class Win {
  [DllImport("user32.dll")] public static extern bool SetWindowPos(IntPtr h, IntPtr after, int x, int y, int cx, int cy, uint flags);
  [DllImport("user32.dll")] public static extern bool ShowWindow(IntPtr h, int c);
  [DllImport("user32.dll")] public static extern bool IsIconic(IntPtr h);
  [DllImport("user32.dll")] public static extern bool GetWindowRect(IntPtr h, out RECT r);
  [DllImport("user32.dll")] public static extern IntPtr GetForegroundWindow();
  [DllImport("user32.dll", CharSet=CharSet.Unicode)] public static extern int GetWindowTextW(IntPtr h, System.Text.StringBuilder s, int m);
  [StructLayout(LayoutKind.Sequential)] public struct RECT { public int L,T,R,B; }

  public static readonly IntPtr HWND_TOPMOST = new IntPtr(-1);
  public static readonly IntPtr HWND_NOTOPMOST = new IntPtr(-2);
  public const uint SWP_NOMOVE = 0x0002;
  public const uint SWP_NOSIZE = 0x0004;
  public const uint SWP_NOACTIVATE = 0x0010;

  public static IntPtr Find() {
    foreach (var p in System.Diagnostics.Process.GetProcesses()) {
      if (p.ProcessName != "ApplicationFrameHost") continue;
      var hh = p.MainWindowHandle; if (hh == IntPtr.Zero) continue;
      var t = new System.Text.StringBuilder(512); GetWindowTextW(hh, t, 512);
      if (t.ToString().Contains("Saiku Deck")) return hh;
    }
    return IntPtr.Zero;
  }

  public static string Title(IntPtr h) {
    var t = new System.Text.StringBuilder(512); GetWindowTextW(h, t, 512);
    return t.ToString();
  }

  public static string Rect(IntPtr h) {
    RECT r; GetWindowRect(h, out r);
    return r.L + "," + r.T + "," + (r.R - r.L) + "," + (r.B - r.T);
  }

  public static bool Raise(IntPtr h) {
    return SetWindowPos(h, HWND_TOPMOST, 0, 0, 0, 0, SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE);
  }

  /// Explicit geometry, used instead of SW_MAXIMIZE.
  public static bool Fill(IntPtr h, int x, int y, int cx, int cy) {
    return SetWindowPos(h, HWND_TOPMOST, x, y, cx, cy, SWP_NOACTIVATE);
  }

  public static bool Lower(IntPtr h) {
    return SetWindowPos(h, HWND_NOTOPMOST, 0, 0, 0, 0, SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE);
  }
}
"@

$h = [Win]::Find()
if ($h -eq [IntPtr]::Zero) { Write-Error "Saiku Deck window not found"; exit 1 }

Add-Type -AssemblyName System.Drawing
Add-Type -AssemblyName System.Windows.Forms
$screen = [System.Windows.Forms.SystemInformation]::VirtualScreen

# SW_MAXIMIZE is unreliable here: the window belongs to ApplicationFrameHost and a
# full-screen game can leave the request unimplemented, so it stays at its old size.
# Sizing it explicitly always works.
if ($Max) {
  $ok = [Win]::Fill($h, $screen.X, $screen.Y, $screen.Width, $screen.Height)
  "Fill($($screen.X),$($screen.Y),$($screen.Width),$($screen.Height)) -> $ok"
  Start-Sleep -Milliseconds 800
} elseif ([Win]::IsIconic($h)) {
  [Win]::ShowWindow($h, 9) | Out-Null
  Start-Sleep -Milliseconds 500
}

# Bring forward in Z order WITHOUT activating: the game keeps focus, we still get pixels.
[Win]::Raise($h) | Out-Null
Start-Sleep -Milliseconds $SettleMs

$rect = ([Win]::Rect($h) -split ',') | ForEach-Object { [int]$_ }
$x = $rect[0]; $y = $rect[1]; $w = $rect[2]; $bh = $rect[3]
"window rect: $x,$y ${w}x${bh}  title=[$([Win]::Title($h))]"

# Clamp to the visible desktop: a maximised window reports -8/-8 by chrome overhang.
$sx = [math]::Max($x - $Pad, $screen.X)
$sy = [math]::Max($y - $Pad, $screen.Y)
$sw = [math]::Min($w + 2 * $Pad, $screen.Width  - ($sx - $screen.X))
$sh = [math]::Min($bh + 2 * $Pad, $screen.Height - ($sy - $screen.Y))

$bmp = New-Object System.Drawing.Bitmap($sw, $sh)
$g = [System.Drawing.Graphics]::FromImage($bmp)
$g.CopyFromScreen($sx, $sy, 0, 0, (New-Object System.Drawing.Size($sw, $sh)))
$bmp.Save($Out, [System.Drawing.Imaging.ImageFormat]::Png)
$g.Dispose(); $bmp.Dispose()

# Put the Z order back so we do not leave the app floating over everything.
[Win]::Lower($h) | Out-Null

"saved $Out (${sw}x${sh})"
