<#
  Capture the Saiku Deck window into a PNG.

  Method: PrintWindow with PW_RENDERFULLCONTENT.

  Why not a screen capture: this machine sometimes runs a full-screen game that
  refuses to give up the foreground. SetForegroundWindow is ignored, SW_MINIMIZE on
  the blocker does not help, and SetWindowPos reports success while the window keeps
  its old size. Screen capture therefore grabs the game (silently -- you get a
  picture of the wrong app and may conclude the UI regressed), and size/position
  tricks cannot be relied on.

  PrintWindow asks the window to draw itself into a bitmap. It needs neither focus
  nor Z-order, and it needs no resizing, so the result is deterministic.

  PW_RENDERFULLCONTENT (0x2) is required for UWP/XAML windows: the flagless call
  produces an empty or stale bitmap because the compositor content lives on a
  different surface.

  Usage:
    powershell.exe -File Print.ps1 -Out shot.png
    powershell.exe -File Print.ps1 -Out shot.png -Scale 2
#>
param(
  [Parameter(Mandatory=$true)][string]$Out,
  [int]$Scale = 1
)

Add-Type -AssemblyName System.Drawing

# -ReferencedAssemblies is required: Add-Type compiles the snippet against a default
# reference set that does NOT include System.Drawing, so without this the C# fails
# with "type or namespace 'Bitmap' could not be found" even though the assembly is
# loaded in the PowerShell session.
Add-Type -ReferencedAssemblies System.Drawing -TypeDefinition @"
using System;
using System.Drawing;
using System.Runtime.InteropServices;
public class Pw {
  [DllImport("user32.dll")] public static extern bool PrintWindow(IntPtr h, IntPtr hdc, uint flags);
  [DllImport("user32.dll")] public static extern bool GetWindowRect(IntPtr h, out RECT r);
  [DllImport("user32.dll")] public static extern bool IsWindowVisible(IntPtr h);
  [DllImport("user32.dll", CharSet=CharSet.Unicode)] public static extern int GetWindowTextW(IntPtr h, System.Text.StringBuilder s, int m);
  [StructLayout(LayoutKind.Sequential)] public struct RECT { public int L,T,R,B; }

  public const uint PW_RENDERFULLCONTENT = 0x00000002;

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

  /// Returns the size actually captured, or an empty string on failure.
  public static string Grab(IntPtr h, string path) {
    RECT r; GetWindowRect(h, out r);
    int w = r.R - r.L, ht = r.B - r.T;
    if (w <= 0 || ht <= 0) return "BAD_RECT " + w + "x" + ht;

    using (var bmp = new Bitmap(w, ht))
    using (var g = Graphics.FromImage(bmp)) {
      IntPtr hdc = g.GetHdc();
      bool ok = PrintWindow(h, hdc, PW_RENDERFULLCONTENT);
      g.ReleaseHdc(hdc);
      if (!ok) return "PRINTWINDOW_FAILED";
      bmp.Save(path, System.Drawing.Imaging.ImageFormat.Png);
    }
    return w + "x" + ht;
  }
}
"@

$h = [Pw]::Find()
if ($h -eq [IntPtr]::Zero) { Write-Error "Saiku Deck window not found"; exit 1 }
"window: [$([Pw]::Title($h))] visible=$([Pw]::IsWindowVisible($h))"

$size = [Pw]::Grab($h, $Out)
if ($size -notmatch '^\d+x\d+$') { Write-Error "capture failed: $size"; exit 2 }
"saved $Out ($size)"

if ($Scale -gt 1) {
  # Nearest-neighbour magnify for pixel-level inspection of glyphs and hairlines.
  $src = [System.Drawing.Image]::FromFile($Out)
  $bw = $src.Width * $Scale; $bh = $src.Height * $Scale
  $big = New-Object System.Drawing.Bitmap($bw, $bh)
  $g = [System.Drawing.Graphics]::FromImage($big)
  $g.InterpolationMode = [System.Drawing.Drawing2D.InterpolationMode]::NearestNeighbor
  $g.PixelOffsetMode = [System.Drawing.Drawing2D.PixelOffsetMode]::Half
  $g.DrawImage($src, 0, 0, $bw, $bh)
  $g.Dispose()
  $scaled = [System.IO.Path]::ChangeExtension($Out, $null) + "-${Scale}x.png"
  $big.Save($scaled, [System.Drawing.Imaging.ImageFormat]::Png)
  $big.Dispose(); $src.Dispose()
  "saved $scaled (${bw}x${bh})"
}
