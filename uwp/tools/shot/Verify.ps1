<#
  One-shot verification: maximise the window, capture it, and for a named element
  report both its automation rectangle and whether that region of the capture
  actually contains rendered pixels.

  Why this exists: doing it in separate steps produced a string of wrong conclusions.
  `PrintWindow` returns a bitmap the size of the window's *current* rectangle, and the
  window gets resized between steps (snap shortcuts, the shell restoring it), so a
  rectangle read in one step and applied to an image from another step measures the
  wrong place -- and "empty region" then reads as "the control did not render".

  Doing capture and measurement together removes the mismatch.

  Usage:
    powershell.exe -File Verify.ps1 -Element "新会话"
    powershell.exe -File Verify.ps1 -Element "新会话|刷新|增加应用栏"
#>
param(
  [Parameter(Mandatory=$true)][string]$Element,
  [string]$Out = "$env:TEMP\verify-shot.png",
  [int]$Threshold = 60
)

Add-Type -AssemblyName System.Drawing
Add-Type -AssemblyName System.Windows.Forms
Add-Type -AssemblyName UIAutomationClient
Add-Type -AssemblyName UIAutomationTypes

Add-Type -ReferencedAssemblies System.Drawing -TypeDefinition @"
using System;
using System.Drawing;
using System.Runtime.InteropServices;
public class Vw {
  [DllImport("user32.dll")] public static extern bool PrintWindow(IntPtr h, IntPtr hdc, uint flags);
  [DllImport("user32.dll")] public static extern bool GetWindowRect(IntPtr h, out RECT r);
  [DllImport("user32.dll", CharSet=CharSet.Unicode)] public static extern int GetWindowTextW(IntPtr h, System.Text.StringBuilder s, int m);
  [StructLayout(LayoutKind.Sequential)] public struct RECT { public int L,T,R,B; }
  public const uint PW_RENDERFULLCONTENT = 0x00000002;

  public static IntPtr Find() {
    foreach (var p in System.Diagnostics.Process.GetProcesses()) {
      if (p.ProcessName != "ApplicationFrameHost") continue;
      var hh = p.MainWindowHandle; if (hh == IntPtr.Zero) continue;
      var t = new System.Text.StringBuilder(512); GetWindowTextW(hh, t, 512);
      if (t.ToString().Contains("Arc Deck")) return hh;
    }
    return IntPtr.Zero;
  }

  public static string WindowRect(IntPtr h) {
    RECT r; GetWindowRect(h, out r);
    return r.L + "," + r.T + "," + (r.R - r.L) + "," + (r.B - r.T);
  }

  public static string Grab(IntPtr h, string path) {
    RECT r; GetWindowRect(h, out r);
    int w = r.R - r.L, ht = r.B - r.T;
    if (w <= 0 || ht <= 0) return "BAD_RECT";
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

$h = [Vw]::Find()
if ($h -eq [IntPtr]::Zero) { Write-Error "Arc Deck window not found"; exit 1 }

$wr = ([Vw]::WindowRect($h) -split ',') | ForEach-Object { [int]$_ }
$winX = $wr[0]; $winY = $wr[1]
"window rect: $winX,$winY $($wr[2])x$($wr[3])"

# Capture first, then query rectangles -- the window may shift in between, so the
# rects are converted into image space using the SAME window rect we just captured.
$sizeText = [Vw]::Grab($h, $Out)
if ($sizeText -notmatch '^(\d+)x(\d+)$') { Write-Error "capture failed: $sizeText"; exit 2 }
$imgW = [int]$Matches[1]; $imgH = [int]$Matches[2]
"captured:   $Out ($sizeText)"
""

$AE = [System.Windows.Automation.AutomationElement]
$TS = [System.Windows.Automation.TreeScope]
$win = $AE::RootElement.FindFirst($TS::Children,
  (New-Object System.Windows.Automation.PropertyCondition($AE::NameProperty, "Arc Deck")))
$all = $win.FindAll($TS::Descendants, [System.Windows.Automation.Condition]::TrueCondition)

$bmp = New-Object System.Drawing.Bitmap($Out)

foreach ($e in $all) {
  $name = $e.Current.Name
  if ([string]::IsNullOrEmpty($name) -or $name -notmatch $Element) { continue }

  $r = $e.Current.BoundingRectangle
  # Screen -> image space.
  $x = [int]$r.X - $winX
  $y = [int]$r.Y - $winY
  $w = [int]$r.Width
  $hh = [int]$r.Height

  if ($x -lt 0 -or $y -lt 0 -or $x + $w -gt $imgW -or $y + $hh -gt $imgH) {
    "  {0,-20} screen {1},{2} {3}x{4}  -> image {5},{6}  OUTSIDE CAPTURE" -f `
      $name, [int]$r.X, [int]$r.Y, $w, $hh, $x, $y
    continue
  }

  $lit = 0; $max = 0; $n = 0
  for ($yy = $y; $yy -lt ($y + $hh); $yy++) {
    for ($xx = $x; $xx -lt ($x + $w); $xx++) {
      $c = $bmp.GetPixel($xx, $yy)
      $v = [int](($c.R + $c.G + $c.B) / 3)
      if ($v -gt $max) { $max = $v }
      if ($v -ge $Threshold) { $lit++ }
      $n++
    }
  }

  $pct = if ($n -gt 0) { [math]::Round(100.0 * $lit / $n, 2) } else { 0 }
  $verdict = if ($lit -eq 0) { "NOT RENDERED" } else { "rendered" }
  "  {0,-20} image {1},{2} {3}x{4}  lit={5} ({6}%)  max={7}  {8}" -f `
    $name, $x, $y, $w, $hh, $lit, $pct, $max, $verdict
}

$bmp.Dispose()
