<#
  UI Automation driver for the Arc Deck UWP window. Windows PowerShell 5.1.

  Why this exists: coordinate clicking on this window is unreliable. The window
  belongs to ApplicationFrameHost and sits at DPI 120, and empirical clicks at
  the visually-correct spot did not change the NavigationView selection. UIA
  addresses elements by name instead, so neither DPI nor window chrome matters.

  Usage:
    powershell.exe -File Uia.ps1 -List
    powershell.exe -File Uia.ps1 -Invoke "Sessions"
    powershell.exe -File Uia.ps1 -Invoke "Sessions" -Out shot.png
    powershell.exe -File Uia.ps1 -SetText "搜索会话" "hello" -Out shot.png
    powershell.exe -File Uia.ps1 -SetTextFirstEdit "新标题"

  -SetTextFirstEdit exists because text boxes created in code -- such as the one
  inside the rename ContentDialog -- have no Name, so they cannot be addressed by
  name. It writes to the first Edit control in the tree. Use it when a modal dialog
  is up and there is exactly one input.
#>
param(
  [switch]$List,
  [string]$Invoke,
  [string[]]$SetText,
  [string]$SetTextFirstEdit,
  [string]$Out,
  [int]$Depth = 6
)

Add-Type -AssemblyName UIAutomationClient
Add-Type -AssemblyName UIAutomationTypes
Add-Type -AssemblyName System.Drawing
Add-Type -AssemblyName System.Windows.Forms

$AE = [System.Windows.Automation.AutomationElement]
$TS = [System.Windows.Automation.TreeScope]
$CT = [System.Windows.Automation.ControlType]

# The UWP window is owned by ApplicationFrameHost, so match on the title.
$root = $AE::RootElement
$cond = New-Object System.Windows.Automation.PropertyCondition($AE::NameProperty, "Arc Deck")
$win = $root.FindFirst($TS::Children, $cond)
if ($null -eq $win) { Write-Error "Arc Deck window not found"; exit 1 }

function Walk($el, $d) {
  if ($d -gt $Depth) { return }
  $kids = $el.FindAll($TS::Children, [System.Windows.Automation.Condition]::TrueCondition)
  foreach ($k in $kids) {
    $n = $k.Current.Name
    $t = $k.Current.ControlType.ProgrammaticName -replace 'ControlType\.', ''
    $aid = $k.Current.AutomationId
    if ($n -or $aid) { "{0}{1} [{2}] id={3}" -f ('  ' * $d), $n, $t, $aid }
    Walk $k ($d + 1)
  }
}

function FindByName($name) {
  # Search each ListItem's own subtree first: NavigationView items are ListItems
  # whose Name lives on the item itself, and a top-level Descendants query on the
  # window is both slow and ambiguous here.
  $liCond = New-Object System.Windows.Automation.PropertyCondition($AE::ControlTypeProperty, $CT::ListItem)
  foreach ($li in $win.FindAll($TS::Descendants, $liCond)) {
    if ($li.Current.Name -eq $name) { return $li }
  }
  $c = New-Object System.Windows.Automation.PropertyCondition($AE::NameProperty, $name)
  return $win.FindFirst($TS::Descendants, $c)
}

if ($List) { Walk $win 0; if (-not $Out) { exit 0 } }

# Text inside a ListItem is usually not the element carrying Invoke or
# SelectionItem; the row is. Walk up until a pattern answers.
function Get-Actionable($el) {
  $cur = $el
  for ($i = 0; $i -lt 6 -and $null -ne $cur; $i++) {
    foreach ($pat in @([System.Windows.Automation.InvokePattern]::Pattern,
                       [System.Windows.Automation.SelectionItemPattern]::Pattern)) {
      $o = $null
      if ($cur.TryGetCurrentPattern($pat, [ref]$o)) { return ,@($cur, $pat, $o) }
    }
    $cur = [System.Windows.Automation.TreeWalker]::ControlViewWalker.GetParent($cur)
  }
  return $null
}

if ($Invoke) {
  $el = FindByName $Invoke
  if ($null -eq $el) {
    # fall back to a name CONTAINS match (list rows carry long descriptions)
    $all = $win.FindAll($TS::Descendants, [System.Windows.Automation.Condition]::TrueCondition)
    foreach ($k in $all) { if ($k.Current.Name -like "*$Invoke*") { $el = $k; break } }
  }
  if ($null -eq $el) { Write-Error "element not found: $Invoke"; exit 2 }
  $act = Get-Actionable $el
  if ($null -eq $act) { Write-Error "no invoke/select pattern near: $Invoke"; exit 3 }
  $target = $act[0]
  if ($act[1] -eq [System.Windows.Automation.InvokePattern]::Pattern) {
    $act[2].Invoke()
  } else {
    $act[2].Select()
  }
  "invoked: $Invoke (on [$($target.Current.ControlType.ProgrammaticName)] '$($target.Current.Name)')"
  Start-Sleep -Milliseconds 1500
}

if ($SetText -and $SetText.Count -ge 2) {
  $el = FindByName $SetText[0]
  if ($null -eq $el) { Write-Error "element not found: $($SetText[0])"; exit 2 }
  $p = $el.GetCurrentPattern([System.Windows.Automation.ValuePattern]::Pattern)
  $p.SetValue($SetText[1])
  "set '$($SetText[0])' = '$($SetText[1])'"
  Start-Sleep -Milliseconds 1200
}

if ($SetTextFirstEdit) {
  # 对话框里代码创建的 TextBox 没有 Name，只能按类型找。
  $editCond = New-Object System.Windows.Automation.PropertyCondition(
    $AE::ControlTypeProperty, $CT::Edit)
  $edit = $win.FindFirst($TS::Descendants, $editCond)
  if ($null -eq $edit) { Write-Error "no Edit control found"; exit 2 }

  $p = $edit.GetCurrentPattern([System.Windows.Automation.ValuePattern]::Pattern)
  $p.SetValue($SetTextFirstEdit)
  "set first Edit = '$SetTextFirstEdit'"
  Start-Sleep -Milliseconds 1200
}

if ($Out) {
  $b = [System.Windows.Forms.SystemInformation]::VirtualScreen
  $bmp = New-Object System.Drawing.Bitmap($b.Width, $b.Height)
  $g = [System.Drawing.Graphics]::FromImage($bmp)
  $g.CopyFromScreen($b.X, $b.Y, 0, 0, $bmp.Size)
  $bmp.Save($Out, [System.Drawing.Imaging.ImageFormat]::Png)
  $g.Dispose(); $bmp.Dispose()
  "saved $Out"
}
