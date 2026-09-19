<#
  Click + screenshot helper. Run with Windows PowerShell 5.1, NOT pwsh.

  Why this exists: Arc Deck is a UWP app, so its window belongs to
  ApplicationFrameHost, and this machine runs at DPI 120 (125%). The window
  rect is in physical pixels, the window content is laid out in 96-DPI logical
  pixels, and the saved screenshot is physical pixels again. Guessing a click
  coordinate by hand across those three spaces always lands off target.

  Usage:
    powershell.exe -File Ui.ps1 -Click 56,83 -Out shot.png
    powershell.exe -File Ui.ps1 -Max -Out shot.png
    powershell.exe -File Ui.ps1 -Info

  -Click takes SCREENSHOT-PREVIEW coordinates: the coordinates you read off the
  image after the window width is scaled to -PreviewWidth (default 1066).
  The script converts back to screen pixels itself.

  NOTE: the Add-Type C# below must stay pure ASCII. Windows PowerShell 5.1 reads
  a BOM-less .ps1 as the ANSI code page, so a non-ASCII character in a C# string
  literal or comment gets mangled and the here-string fails to compile.
#>
param(
  [int[]]$Click,
  [string]$Out,
  [switch]$Max,
  [switch]$Info,
  [int]$PreviewWidth = 1066
)

Add-Type -TypeDefinition @"
using System;
using System.Runtime.InteropServices;
public class Ui {
  [DllImport("user32.dll")] public static extern bool SetCursorPos(int x, int y);
  [DllImport("user32.dll")] public static extern void mouse_event(uint f, uint x, uint y, uint d, IntPtr e);
  [DllImport("user32.dll")] public static extern bool SetForegroundWindow(IntPtr h);
  [DllImport("user32.dll")] public static extern bool ShowWindow(IntPtr h, int c);
  [DllImport("user32.dll")] public static extern IntPtr GetForegroundWindow();
  [DllImport("user32.dll", CharSet=CharSet.Unicode)] public static extern int GetWindowTextW(IntPtr h, System.Text.StringBuilder s, int m);
  [DllImport("user32.dll")] public static extern bool GetWindowRect(IntPtr h, out RECT r);
  [DllImport("user32.dll")] public static extern uint GetDpiForWindow(IntPtr h);
  [StructLayout(LayoutKind.Sequential)] public struct RECT { public int L,T,R,B; }

  public static IntPtr Find() {
    foreach (var p in System.Diagnostics.Process.GetProcesses()) {
      if (p.ProcessName != "ApplicationFrameHost") continue;
      var hh = p.MainWindowHandle; if (hh == IntPtr.Zero) continue;
      var t = new System.Text.StringBuilder(512); GetWindowTextW(hh, t, 512);
      if (t.ToString().Contains("Arc Deck")) return hh;
    }
    return IntPtr.Zero;
  }

  public static string Info() {
    IntPtr h = Find(); if (h == IntPtr.Zero) return "NO_WINDOW";
    RECT r; GetWindowRect(h, out r);
    var ft = new System.Text.StringBuilder(512); GetWindowTextW(GetForegroundWindow(), ft, 512);
    return "dpi=" + GetDpiForWindow(h) + " rect=" + r.L + "," + r.T + "," + r.R + "," + r.B
         + " size=" + (r.R - r.L) + "x" + (r.B - r.T) + " fg=[" + ft.ToString() + "]";
  }

  public static string Max() {
    IntPtr h = Find(); if (h == IntPtr.Zero) return "NO_WINDOW";
    ShowWindow(h, 3);
    System.Threading.Thread.Sleep(900);
    SetForegroundWindow(h);
    System.Threading.Thread.Sleep(400);
    return Info();
  }

  // px,py are screenshot-preview coords (window width == previewW)
  public static string ClickAt(int px, int py, int previewW) {
    IntPtr h = Find(); if (h == IntPtr.Zero) return "NO_WINDOW";
    RECT r; GetWindowRect(h, out r);
    SetForegroundWindow(h);
    System.Threading.Thread.Sleep(400);
    double scale = (double)(r.R - r.L) / previewW;
    int x = r.L + (int)Math.Round(px * scale);
    int y = r.T + (int)Math.Round(py * scale);
    SetCursorPos(x, y);
    System.Threading.Thread.Sleep(250);
    mouse_event(0x0002, 0, 0, 0, IntPtr.Zero);
    System.Threading.Thread.Sleep(70);
    mouse_event(0x0004, 0, 0, 0, IntPtr.Zero);
    return "clicked preview(" + px + "," + py + ") -> screen(" + x + "," + y + ") scale=" + scale.ToString("F3");
  }
}
"@

if ($Info) { [Ui]::Info(); exit }
if ($Max)  { [Ui]::Max() | Out-Null; Start-Sleep -Milliseconds 600 }

if ($Click -and $Click.Count -ge 2) {
  [Ui]::ClickAt($Click[0], $Click[1], $PreviewWidth)
  Start-Sleep -Milliseconds 1500
}

if ($Out) {
  Add-Type -AssemblyName System.Drawing
  Add-Type -AssemblyName System.Windows.Forms
  $b = [System.Windows.Forms.SystemInformation]::VirtualScreen
  $bmp = New-Object System.Drawing.Bitmap($b.Width, $b.Height)
  $g = [System.Drawing.Graphics]::FromImage($bmp)
  $g.CopyFromScreen($b.X, $b.Y, 0, 0, $bmp.Size)
  $bmp.Save($Out, [System.Drawing.Imaging.ImageFormat]::Png)
  $g.Dispose(); $bmp.Dispose()
  $out2 = [Ui]::Info()
  "saved $Out ($($b.Width)x$($b.Height) virtual)  $out2"
}
