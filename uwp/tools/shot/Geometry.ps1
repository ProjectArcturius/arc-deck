<#
  ⚠️ 本脚本的**窗口尺寸不可用** —— 改用 Layout.ps1，或者只想看 UIA 元素时再用本脚本。

  它把 GetWindowRect 与 UIA 的 BoundingRectangle 放在一起报，看起来是「同一个
  坐标空间」，实际**不是**：本进程没有声明 DPI 感知，所以 GetWindowRect 返回的
  是被系统按 96 DPI 虚拟化过的值，而 UIA 报的是物理像素。125% 缩放下两者差
  恰好 1.25 倍 —— 据此得出的「窗口 1550x830」是假的（真实是 1938x1038），
  并直接导致 U13 那条「输入区落在窗口外」的错误结论。

  Layout.ps1 第一件事是 SetProcessDpiAwarenessContext，并会把是否拿到 DPI
  感知打出来；窗口尺寸也从 DwmGetWindowAttribute 取（UWP 的 GetWindowRect
  会把不可见的 resize 边框算进去）。**量坐标一律用那个。**

  本脚本保留的原因：它的 UIA 元素列表（Nav / ContentFrame / MessageList / Edit）
  仍然好用，那几个数字本身是物理像素、是对的。

  ----------------- 以下是原始说明 -----------------

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
      if (t.ToString().Contains("Saiku Deck")) return hh;
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
  (New-Object System.Windows.Automation.PropertyCondition($AE::NameProperty, "Saiku Deck")))
if ($null -eq $win) { Write-Error "Saiku Deck window not found"; exit 1 }

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
