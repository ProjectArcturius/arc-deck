<#
  Walk the visual tree of the Saiku Deck window and print each element's type, name
  and actual size.

  Why: when a layout problem resists the usual fixes, the only reliable move is to
  see who is actually measuring what. Specifically, a ScrollViewer anywhere in the
  ancestor chain measures its child with INFINITE height, which makes star-sized
  Grid rows grow to fit content and pushes later rows out of the window -- and you
  cannot tell that from the XAML alone.

  Usage:
    powershell.exe -File VisualTree.ps1 -Filter ScrollViewer
    powershell.exe -File VisualTree.ps1 -Filter ScrollViewer|MessageList|Grid -Max 30
#>
param(
  [string]$Filter,
  [int]$Max = 200
)

Add-Type -AssemblyName UIAutomationClient
Add-Type -AssemblyName UIAutomationTypes

$AE = [System.Windows.Automation.AutomationElement]
$TS = [System.Windows.Automation.TreeScope]

$win = $AE::RootElement.FindFirst($TS::Children,
  (New-Object System.Windows.Automation.PropertyCondition($AE::NameProperty, "Saiku Deck")))
if ($null -eq $win) { Write-Error "Saiku Deck window not found"; exit 1 }

# Walk the RAW view: the control view that Uia.ps1 uses hides layout-only elements
# such as ScrollViewer, which is exactly what we need to see here.
$walker = [System.Windows.Automation.TreeWalker]::RawViewWalker

$shown = 0
function Walk($el, $depth) {
  if ($null -eq $el -or $depth -gt 30 -or $script:shown -ge $Max) { return }

  $type = $el.Current.LocalizedControlType
  $name = $el.Current.Name
  $aid = $el.Current.AutomationId
  $cls = $el.Current.ClassName
  $r = $el.Current.BoundingRectangle

  $label = "$cls"
  $q = [char]34
  if ($name) { $label += " $q$name$q" }
  if ($aid) { $label += " #$aid" }

  $text = "{0}{1}  {2}x{3} @{4},{5}" -f ('  ' * $depth), $label, `
    [int]$r.Width, [int]$r.Height, [int]$r.X, [int]$r.Y

  $match = $true
  if ($Filter) { $match = ($label -match $Filter) -or ($type -match $Filter) }

  if ($match) { $text; $script:shown++ }

  $child = $walker.GetFirstChild($el)
  while ($null -ne $child) {
    Walk $child ($depth + 1)
    $child = $walker.GetNextSibling($child)
  }
}

Walk $win 0
"shown: $shown"
