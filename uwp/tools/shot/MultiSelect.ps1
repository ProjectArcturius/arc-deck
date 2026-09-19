<#
  Exercise multi-selection on the session list via UI Automation.

  Multi-select is the last unverified interaction: SelectionMode is Extended, so
  Ctrl/Shift-click should select several rows and the batch commands (pin, delete)
  should then act on all of them. Driving that by synthetic key-plus-click is
  fragile, so this uses the automation SelectionItemPattern instead -- which is
  also what a screen reader would use, so it exercises the same contract.

  Usage:
    powershell.exe -File MultiSelect.ps1 -Count 2
    powershell.exe -File MultiSelect.ps1 -Count 2 -Clear
#>
param(
  [int]$Count = 2,
  [switch]$Clear
)

Add-Type -AssemblyName UIAutomationClient
Add-Type -AssemblyName UIAutomationTypes

$AE = [System.Windows.Automation.AutomationElement]
$TS = [System.Windows.Automation.TreeScope]
$CT = [System.Windows.Automation.ControlType]

function Find-All($root, $type) {
  $cond = New-Object System.Windows.Automation.PropertyCondition($AE::ControlTypeProperty, $type)
  return $root.FindAll($TS::Descendants, $cond)
}

$win = $AE::RootElement.FindFirst($TS::Children,
  (New-Object System.Windows.Automation.PropertyCondition($AE::NameProperty, "Arc Deck")))
if ($null -eq $win) { Write-Error "Arc Deck window not found"; exit 1 }

# 按 AutomationId 找 SessionList。找不到就按类型找第一个 List —— 依赖 Id 太脆，
# 而这个页面上只有一个真正承载会话的 List。
$list = $null
$byId = New-Object System.Windows.Automation.PropertyCondition($AE::AutomationIdProperty, "SessionList")
$list = $win.FindFirst($TS::Descendants, $byId)

if ($null -eq $list) {
  "AutomationId=SessionList 未找到，回退到按类型找 List"
  foreach ($l in (Find-All $win $CT::List)) {
    $r = $l.Current.BoundingRectangle
    "  候选 List id=$($l.Current.AutomationId) at $([int]$r.X),$([int]$r.Y) $([int]$r.Width)x$([int]$r.Height)"
    if ($null -eq $list) { $list = $l }
  }
}

if ($null -eq $list) { Write-Error "no List control found"; exit 2 }
"list: id=$($list.Current.AutomationId) name=[$($list.Current.Name)]"
"list supports SelectionPattern: " + $list.TryGetCurrentPattern([System.Windows.Automation.SelectionPattern]::Pattern, [ref]$null)

$items = Find-All $list $CT::ListItem
"list items: $($items.Count)"

if ($Clear) {
  $sp = $null
  if ($list.TryGetCurrentPattern([System.Windows.Automation.SelectionPattern]::Pattern, [ref]$sp)) {
    # CanSelectMultiple 决定是否有必要多选；False 说明 SelectionMode 没生效。
    "CanSelectMultiple = $($sp.Current.CanSelectMultiple)"
  }
  $list.SetFocus()
  Start-Sleep -Milliseconds 200
}

$n = 0
$done = 0
foreach ($it in $items) {
  if ($n -ge $Count) { break }
  $r = $it.Current.BoundingRectangle
  $title = ""
  foreach ($t in (Find-All $it $CT::Text)) { $title = $t.Current.Name; break }

  $pattern = $null
  if ($it.TryGetCurrentPattern([System.Windows.Automation.SelectionItemPattern]::Pattern, [ref]$pattern)) {
    if ($Clear) { $pattern.AddToSelection() } else { $pattern.Select() }
    "  [$done] selected: $title  at $([int]$r.X),$([int]$r.Y)"
    $done++
  } else {
    "  [$n] no SelectionItemPattern: $title"
  }
  $n++
}

"SILENT_MARKER selected_count=$done"
