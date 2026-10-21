<#
  Right-click an element found by name, via UI Automation.

  Why UIA rather than coordinates: element rectangles move whenever the window is
  resized by the shell, and a stale coordinate silently clicks the wrong thing.
  UIA gives the current rectangle, and we click its centre.

  Why a real mouse event rather than the automation Invoke pattern: the point of the
  test is to exercise the *right-click path* -- which is what ContextFlyout and the
  app's own RightTapped handling key off. Invoke would bypass exactly the code under
  test.

  Usage:
    powershell.exe -File RightClick.ps1 -Name "会话标题"
    powershell.exe -File RightClick.ps1 -Name "UWP 客户端骨架" -List
#>
param(
  [Parameter(Mandatory=$true)][string]$Name,
  [switch]$List
)

Add-Type -AssemblyName UIAutomationClient
Add-Type -AssemblyName UIAutomationTypes

Add-Type -TypeDefinition @"
using System;
using System.Runtime.InteropServices;
public class Rc {
  [DllImport("user32.dll")] public static extern bool SetCursorPos(int x, int y);
  [DllImport("user32.dll")] public static extern void mouse_event(uint f, uint x, uint y, uint d, IntPtr e);
  [DllImport("user32.dll")] public static extern IntPtr GetForegroundWindow();
  [DllImport("user32.dll", CharSet=CharSet.Unicode)] public static extern int GetWindowTextW(IntPtr h, System.Text.StringBuilder s, int m);

  public const uint RIGHTDOWN = 0x0008;
  public const uint RIGHTUP   = 0x0010;

  public static string RightClick(int x, int y) {
    SetCursorPos(x, y);
    System.Threading.Thread.Sleep(220);
    mouse_event(RIGHTDOWN, 0, 0, 0, IntPtr.Zero);
    System.Threading.Thread.Sleep(70);
    mouse_event(RIGHTUP, 0, 0, 0, IntPtr.Zero);
    var t = new System.Text.StringBuilder(512);
    GetWindowTextW(GetForegroundWindow(), t, 512);
    return "right-clicked " + x + "," + y + "  foreground=[" + t.ToString() + "]";
  }
}
"@

$AE = [System.Windows.Automation.AutomationElement]
$TS = [System.Windows.Automation.TreeScope]

$win = $AE::RootElement.FindFirst($TS::Children,
  (New-Object System.Windows.Automation.PropertyCondition($AE::NameProperty, "Saiku Deck")))
if ($null -eq $win) { Write-Error "Saiku Deck window not found"; exit 1 }

# Prefer a ListItem whose name matches; fall back to any matching element.
$liCond = New-Object System.Windows.Automation.PropertyCondition(
  $AE::ControlTypeProperty, [System.Windows.Automation.ControlType]::ListItem)

$target = $null
foreach ($li in $win.FindAll($TS::Descendants, $liCond)) {
  if ($li.Current.Name -eq $Name) { $target = $li; break }
}
if ($null -eq $target) {
  $nameCond = New-Object System.Windows.Automation.PropertyCondition($AE::NameProperty, $Name)
  $target = $win.FindFirst($TS::Descendants, $nameCond)
}
if ($null -eq $target) { Write-Error "element not found: $Name"; exit 2 }

$r = $target.Current.BoundingRectangle
if ($r.IsEmpty) { Write-Error "element has no rectangle (not laid out?)"; exit 3 }

$x = [int]($r.X + $r.Width / 2)
$y = [int]($r.Y + $r.Height / 2)

"target: [$($target.Current.Name)] $($target.Current.ControlType.ProgrammaticName) rect=$([int]$r.X),$([int]$r.Y) $([int]$r.Width)x$([int]$r.Height)"
[Rc]::RightClick($x, $y)

if ($List) {
  Start-Sleep -Milliseconds 1200
  "--- 菜单项 ---"
  foreach ($e in $win.FindAll($TS::Descendants, [System.Windows.Automation.Condition]::TrueCondition)) {
    $t = ($e.Current.ControlType.ProgrammaticName -replace 'ControlType\.', '')
    if ($t -eq "MenuItem" -or $t -eq "Menu") {
      $rr = $e.Current.BoundingRectangle
      "  {0,-22} {1,-10} {2}" -f $e.Current.Name, $t, $(if ($rr.IsEmpty) { "(none)" } else { "$([int]$rr.X),$([int]$rr.Y)" })
    }
  }
}
