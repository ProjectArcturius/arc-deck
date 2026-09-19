<#
  Measure the window rectangle and the key UIA rectangles in ONE process, so the
  numbers share a coordinate space and a moment in time.

  Why: every wrong conclusion in this investigation came from mixing measurements
  taken at different times or from different sources -- the shell resizes the window
  between steps, so a rect read in one step and compared against an image from
  another step measures the wrong place. Printing them together removes that.

  Usage:
    powershell.exe -File Geometry.ps1
#>
Add-Type -AssemblyName UIAutomationClient
Add-Type -AssemblyName UIAutomationTypes

Add-Type -TypeDefinition @"
using System;
using System.Runtime.InteropServices;
using System.Text;
public class Geo {
  [DllImport("user32.dll")] public static extern bool GetWindowRect(IntPtr h, out RECT r);
  [DllImport("user32.dll")] public static extern uint GetDpiForWindow(IntPtr h);
  [DllImport("user32.dll")] public static extern IntPtr GetForegroundWindow();
  [DllImport("user32.dll", CharSet=CharSet.Unicode)] public static extern int GetWindowTextW(IntPtr h, StringBuilder s, int m);
  [StructLayout(LayoutKind.Sequential)] public struct RECT { public int L,T,R,B; }

  public static IntPtr Find() {
    foreach (var p in System.Diagnostics.Process.GetProcesses()) {
      if (p.ProcessName != "ApplicationFrameHost") continue;
      var hh = p.MainWindowHandle; if (hh == IntPtr.Zero) continue;
      var t = new StringBuilder(512); GetWindowTextW(hh, t, 512);
      if (t.ToString().Contains("Arc Deck")) return hh;
    }
    return IntPtr.Zero;
  }

  public static string WindowInfo() {
    var h = Find(); if (h == IntPtr.Zero) return "NO_WINDOW";
    RECT r; GetWindowRect(h, out r);
    uint dpi = GetDpiForWindow(h);
    return "window rect=" + r.L + "," + r.T + " size=" + (r.R-r.L) + "x" + (r.B-r.T)
         + " dpi=" + dpi + " scale=" + (dpi / 96.0).ToString("F2");
  }
}
"@

[Geo]::WindowInfo()

$AE = [System.Windows.Automation.AutomationElement]
$TS = [System.Windows.Automation.TreeScope]
$win = $AE::RootElement.FindFirst($TS::Children,
  (New-Object System.Windows.Automation.PropertyCondition($AE::NameProperty, "Arc Deck")))
if ($null -eq $win) { Write-Error "Arc Deck window not found"; exit 1 }

function Show($el, $what) {
  if ($null -eq $el) { "  {0,-22} NOT FOUND" -f $what; return }
  $r = $el.Current.BoundingRectangle
  "  {0,-22} {1},{2} {3}x{4}" -f $what, [int]$r.X, [int]$r.Y, [int]$r.Width, [int]$r.Height
}

"UIA rectangles:"
Show $win "window element"

$idCond = { param($id) New-Object System.Windows.Automation.PropertyCondition($AE::AutomationIdProperty, $id) }
Show ($win.FindFirst($TS::Descendants, (& $idCond "Nav"))) "NavigationView"
Show ($win.FindFirst($TS::Descendants, (& $idCond "ContentFrame"))) "ContentFrame"
Show ($win.FindFirst($TS::Descendants, (& $idCond "MessageList"))) "MessageList"

# The composer has no AutomationId; it is the last Edit in tree order.
$editCond = New-Object System.Windows.Automation.PropertyCondition(
  $AE::ControlTypeProperty, [System.Windows.Automation.ControlType]::Edit)
$edits = $win.FindAll($TS::Descendants, $editCond)
"  Edit controls: $($edits.Count)"
foreach ($e in $edits) { Show $e "Edit [$($e.Current.Name)]" }
