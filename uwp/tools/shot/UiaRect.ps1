<#
  Dump the Saiku Deck automation tree WITH screen rectangles.

  Why: Uia.ps1 -List prints names and control types, which tells you an element
  exists but not where it is -- so you cannot tell "the command bar rendered but is
  squeezed to the right edge" from "the command bar did not render". Rectangles
  settle that, and they are also what you need to aim a click.

  Usage:
    powershell.exe -File UiaRect.ps1
    powershell.exe -File UiaRect.ps1 -Filter "新会话|刷新|全选|MoreButton"
    powershell.exe -File UiaRect.ps1 -Depth 8
#>
param(
  [string]$Filter,
  [int]$Depth = 8
)

Add-Type -AssemblyName UIAutomationClient
Add-Type -AssemblyName UIAutomationTypes

$AE = [System.Windows.Automation.AutomationElement]
$TS = [System.Windows.Automation.TreeScope]

$win = $AE::RootElement.FindFirst($TS::Children,
  (New-Object System.Windows.Automation.PropertyCondition($AE::NameProperty, "Saiku Deck")))
if ($null -eq $win) { Write-Error "Saiku Deck window not found"; exit 1 }

$all = $win.FindAll($TS::Descendants, [System.Windows.Automation.Condition]::TrueCondition)

"{0,-42} {1,-16} {2}" -f "name", "type", "rect (x,y wxh)"
"-" * 88

foreach ($e in $all) {
  $name = $e.Current.Name
  $type = ($e.Current.ControlType.ProgrammaticName -replace 'ControlType\.', '')
  $aid = $e.Current.AutomationId
  $r = $e.Current.BoundingRectangle

  $label = $name
  if ([string]::IsNullOrEmpty($label)) { $label = "id=$aid" }
  if ([string]::IsNullOrEmpty($label)) { $label = "(unnamed)" }

  if ($Filter) {
    $hay = "$name|$aid|$type"
    if ($hay -notmatch $Filter) { continue }
  }

  # IsOffscreen distinguishes "in the tree but not painted" from "painted".
  # An element can report a real rectangle and still be offscreen -- that is exactly
  # the case which makes a screenshot look like the control is missing.
  $state = @()
  if ($e.Current.IsOffscreen) { $state += "offscreen" }
  if (-not $e.Current.IsEnabled) { $state += "disabled" }
  $stateText = if ($state.Count -eq 0) { "visible" } else { $state -join "," }

  $size = if ($r.IsEmpty) { "(none)" } else { "{0}x{1}" -f [int]$r.Width, [int]$r.Height }
  $pos  = if ($r.IsEmpty) { "-" } else { "{0},{1}" -f [int]$r.X, [int]$r.Y }

  "{0,-38} {1,-14} {2,-20} {3}" -f $label, $type, "$pos $size", $stateText
}
