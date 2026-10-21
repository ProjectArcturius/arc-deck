param(
  [int]$TargetPid = 0,
  [string]$Click = "",
  [switch]$List
)

Add-Type -AssemblyName UIAutomationClient
Add-Type -AssemblyName UIAutomationTypes

$root = [System.Windows.Automation.AutomationElement]::RootElement
$cond = New-Object System.Windows.Automation.PropertyCondition(
    [System.Windows.Automation.AutomationElement]::ControlTypeProperty,
    [System.Windows.Automation.ControlType]::Window)
$windows = $root.FindAll([System.Windows.Automation.TreeScope]::Children, $cond)

$target = $null
foreach ($w in $windows) {
    if ($TargetPid -ne 0 -and $w.Current.ProcessId -eq $TargetPid) { $target = $w; break }
    if ($TargetPid -eq 0 -and $w.Current.Name -like "*Saiku Deck*") { $target = $w; break }
}
if ($target -eq $null) {
    Write-Output "未找到窗口（pid=$TargetPid）"
    foreach ($w in $windows) { Write-Output ("  window: " + $w.Current.ProcessId + " | " + $w.Current.Name) }
    exit 1
}

Write-Output ("窗口: " + $target.Current.Name + " pid=" + $target.Current.ProcessId)

function Dump($element, $depth) {
    if ($depth -gt 7) { return }
    $walker = [System.Windows.Automation.TreeWalker]::ControlViewWalker
    $child = $walker.GetFirstChild($element)
    while ($child -ne $null) {
        $c = $child.Current
        Write-Output ("  " * $depth + $c.ControlType.ProgrammaticName + " | " + $c.ClassName + " | " + $c.Name)
        Dump $child ($depth + 1)
        $child = $walker.GetNextSibling($child)
    }
}

if ($List) {
    Dump $target 1
    exit 0
}

if ($Click -ne "") {
    $nameCond = New-Object System.Windows.Automation.PropertyCondition(
        [System.Windows.Automation.AutomationElement]::NameProperty, $Click)
    $element = $target.FindFirst([System.Windows.Automation.TreeScope]::Descendants, $nameCond)
    if ($element -eq $null) { Write-Output "找不到元素: $Click"; exit 2 }
    try {
        $pattern = $element.GetCurrentPattern([System.Windows.Automation.InvokePattern]::Pattern)
        $pattern.Invoke()
        Write-Output "已点击: $Click"
    } catch {
        try {
            $pattern = $element.GetCurrentPattern([System.Windows.Automation.SelectionItemPattern]::Pattern)
            $pattern.Select()
            Write-Output "已选择: $Click"
        } catch {
            Write-Output ("点击失败: " + $_.Exception.Message)
            exit 3
        }
    }
    Start-Sleep -Milliseconds 800

    $textCond = New-Object System.Windows.Automation.PropertyCondition(
        [System.Windows.Automation.AutomationElement]::ControlTypeProperty,
        [System.Windows.Automation.ControlType]::Text)
    $texts = $target.FindAll([System.Windows.Automation.TreeScope]::Descendants, $textCond)
    foreach ($t in $texts) {
        $n = $t.Current.Name
        if ($n -ne "") { Write-Output ("text: " + $n) }
    }
}
