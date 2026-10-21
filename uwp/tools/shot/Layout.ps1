<#
  Saiku Deck 布局量测 —— 每次都用**同一个坐标空间**报数字。

  为什么要重写这一份（U13 的教训）：

    老 Geometry.ps1 把 GetWindowRect 和 UIA 的 BoundingRectangle 混在一起比较，
    而两者**坐标空间不同**：

      · UIA 的 BoundingRectangle 报的是**物理像素**（屏幕绝对坐标）
      · GetWindowRect 在**未声明 DPI 感知**的进程里被系统虚拟化，
        报的是「按 96 DPI 换算过」的值

    125% 缩放下两者正好差 1.25 倍。于是同一扇窗被量出 1550x830 与
    1938x1038 两个尺寸，据此推出的「输入区落在窗口外」是**坐标空间的产物**，
    不是布局缺陷 —— 全屏截图里输入框和「发送」都在屏幕上、也点得到。

    所以本脚本第一件事是 SetProcessDpiAwarenessContext，把本进程变成
    per-monitor DPI aware，让 GetWindowRect 与 UIA 落在同一空间。

  用法：
    powershell.exe -File Layout.ps1
#>
[CmdletBinding()]
param(
  # 打印每个元素相对窗口的偏移，便于判断「谁会掉到窗口外」
  [switch]$Relative
)

Add-Type -AssemblyName UIAutomationClient
Add-Type -AssemblyName UIAutomationTypes

Add-Type -TypeDefinition @"
using System;
using System.Runtime.InteropServices;
using System.Text;

public class LayoutProbe {
  [DllImport("user32.dll")] public static extern bool GetWindowRect(IntPtr h, out RECT r);
  [DllImport("dwmapi.dll")] public static extern int DwmGetWindowAttribute(IntPtr h, int attr, out RECT r, int size);
  [DllImport("user32.dll")] public static extern uint GetDpiForWindow(IntPtr h);
  [DllImport("user32.dll", CharSet=CharSet.Unicode)] public static extern int GetWindowTextW(IntPtr h, StringBuilder s, int m);
  [DllImport("user32.dll")] public static extern IntPtr GetForegroundWindow();

  // -4 = DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2
  [DllImport("user32.dll")] public static extern bool SetProcessDpiAwarenessContext(IntPtr value);

  [StructLayout(LayoutKind.Sequential)] public struct RECT { public int L,T,R,B; }

  public static string MakeDpiAware() {
    // 失败也不致命：下面会把「是否感知 DPI」一并报出来，读数字的人能判断可信度。
    bool ok = false;
    try { ok = SetProcessDpiAwarenessContext(new IntPtr(-4)); } catch { }
    return ok ? "per-monitor-v2" : "FAILED(数字可能被虚拟化)";
  }

  public static IntPtr FindArcDeck() {
    foreach (var p in System.Diagnostics.Process.GetProcesses()) {
      if (p.ProcessName != "ApplicationFrameHost") continue;
      var hh = p.MainWindowHandle; if (hh == IntPtr.Zero) continue;
      var t = new StringBuilder(512); GetWindowTextW(hh, t, 512);
      if (t.ToString().Contains("Saiku Deck")) return hh;
    }
    return IntPtr.Zero;
  }

  public static string Describe(IntPtr h) {
    if (h == IntPtr.Zero) return "NO_WINDOW";
    RECT wr; GetWindowRect(h, out wr);
    RECT dw; int hr = DwmGetWindowAttribute(h, 9, out dw, Marshal.SizeOf(typeof(RECT))); // 9 = DWMWA_EXTENDED_FRAME_BOUNDS
    uint dpi = GetDpiForWindow(h);
    var sb = new StringBuilder();
    sb.Append("GetWindowRect        ").Append(wr.L).Append(',').Append(wr.T).Append("  ")
      .Append(wr.R-wr.L).Append('x').Append(wr.B-wr.T).Append('\n');
    sb.Append("DWM extended frame   ");
    if (hr == 0) sb.Append(dw.L).Append(',').Append(dw.T).Append("  ").Append(dw.R-dw.L).Append('x').Append(dw.B-dw.T);
    else sb.Append("unavailable hr=0x").Append(hr.ToString("X8"));
    sb.Append('\n');
    sb.Append("dpi=").Append(dpi).Append("  scale=").Append((dpi/96.0).ToString("F2"));
    return sb.ToString();
  }
}
"@

"DPI 感知: $([LayoutProbe]::MakeDpiAware())"
""

$h = [LayoutProbe]::FindArcDeck()
if ($h -eq [IntPtr]::Zero) { Write-Error "找不到 Saiku Deck 窗口"; exit 1 }

[LayoutProbe]::Describe($h)

$AE = [System.Windows.Automation.AutomationElement]
$TS = [System.Windows.Automation.TreeScope]
$win = $AE::RootElement.FindFirst($TS::Children,
  (New-Object System.Windows.Automation.PropertyCondition($AE::NameProperty, "Saiku Deck")))
if ($null -eq $win) { Write-Error "UIA 找不到 Saiku Deck 窗口"; exit 1 }

$winRect = $win.Current.BoundingRectangle
""
"UIA 窗口元素           $([int]$winRect.X),$([int]$winRect.Y)  $([int]$winRect.Width)x$([int]$winRect.Height)"
""

function Show($el, $what) {
  if ($null -eq $el) { "  {0,-26} NOT FOUND" -f $what; return }
  $r = $el.Current.BoundingRectangle
  $line = "  {0,-26} {1},{2}  {3}x{4}" -f $what, [int]$r.X, [int]$r.Y, [int]$r.Width, [int]$r.Height
  if ($Relative) {
    $line += "   相对窗口 x={0} y={1} 底={2}" -f `
      [int]($r.X - $winRect.X), [int]($r.Y - $winRect.Y), [int]($r.Bottom - $winRect.Y)
  }
  $line
}

"UIA 元素:"
$idCond = { param($id) New-Object System.Windows.Automation.PropertyCondition($AE::AutomationIdProperty, $id) }
Show ($win.FindFirst($TS::Descendants, (& $idCond "Nav")))            "NavigationView"
Show ($win.FindFirst($TS::Descendants, (& $idCond "ContentFrame")))  "ContentFrame"
Show ($win.FindFirst($TS::Descendants, (& $idCond "MessageList")))   "MessageList"
Show ($win.FindFirst($TS::Descendants, (& $idCond "Composer")))      "Composer"

$btnCond = New-Object System.Windows.Automation.PropertyCondition(
  $AE::ControlTypeProperty, [System.Windows.Automation.ControlType]::Button)
foreach ($b in $win.FindAll($TS::Descendants, $btnCond)) {
  Show $b "Button [$($b.Current.Name)]"
}

# 结论行 —— 本脚本存在的意义就是这一行不再需要人肉换算。
""
$composer = $win.FindFirst($TS::Descendants, (& $idCond "Composer"))
if ($null -ne $composer) {
  $c = $composer.Current.BoundingRectangle
  $inside = ($c.Bottom -le $winRect.Bottom + 1) -and ($c.Top -ge $winRect.Top - 1)
  "结论: Composer 底边 $([int]$c.Bottom) / 窗口底边 $([int]$winRect.Bottom) → " +
    $(if ($inside) { "在窗口内 ✓" } else { "**落在窗口外** ✗" })
}
