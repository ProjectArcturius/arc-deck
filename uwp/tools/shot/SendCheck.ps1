<#
  对话页发送通路的验收 —— 从输入到界面的**整条**路径。

  为什么不能只调一下按钮就算了：这条链上有四段，任何一段断掉的表现都是
  「点了发送没反应」——
    1. 会话列表能进得去（导航真的换了页）
    2. 输入框拿得到文本（ValuePattern 设进去了没有）
    3. 消息进了 MessageList（列表里多了一条，且那条的正文就是刚发的）
    4. 发送后输入框被清空、列表摘要跟着更新

  所以这里逐步断言并把每一步的结果打出来，而不是只报「成功/失败」。

  用法：
    powershell.exe -File SendCheck.ps1 -Session "Arc Deck 骨架与 IME 验证" -Text "验收消息"
#>
[CmdletBinding()]
param(
  [string]$Session = "Arc Deck 骨架与 IME 验证",
  [string]$Text = "发送通路验收",
  [string]$Out
)

Add-Type -AssemblyName UIAutomationClient
Add-Type -AssemblyName UIAutomationTypes

$AE = [System.Windows.Automation.AutomationElement]
$TS = [System.Windows.Automation.TreeScope]
$CT = [System.Windows.Automation.ControlType]

function Find-Window {
  $win = $AE::RootElement.FindFirst($TS::Children,
    (New-Object System.Windows.Automation.PropertyCondition($AE::NameProperty, "Arc Deck")))
  if ($null -eq $win) { throw "找不到 Arc Deck 窗口 —— 应用没在跑？" }
  return $win
}

function Find-ByName($root, $name, $type) {
  $conds = @(New-Object System.Windows.Automation.PropertyCondition($AE::NameProperty, $name))
  if ($type) { $conds += New-Object System.Windows.Automation.PropertyCondition($AE::ControlTypeProperty, $type) }
  $cond = if ($conds.Count -eq 1) { $conds[0] } else { New-Object System.Windows.Automation.AndCondition($conds) }
  return $root.FindFirst($TS::Descendants, $cond)
}

function Find-ByNameLike($root, $name) {
  foreach ($k in $root.FindAll($TS::Descendants, [System.Windows.Automation.Condition]::TrueCondition)) {
    if ($k.Current.Name -like "*$name*") { return $k }
  }
  return $null
}

# 会话行本身通常不是承载 Invoke/Selection 的那个元素。
# **实测**：直接对 ListItem 取 InvokePattern 会报 "Unsupported Pattern" ——
# ListView 的行走的是 SelectionItemPattern。所以往上找第一个能应答的元素，
# 沿用 Uia.ps1 里已经验证过的做法。
function Find-Actionable($el) {
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

function Activate($el, $what) {
  $act = Find-Actionable $el
  if ($null -eq $act) { throw "「$what」附近既没有 Invoke 也没有 Select 模式" }
  if ($act[1] -eq [System.Windows.Automation.InvokePattern]::Pattern) { $act[2].Invoke() }
  else { $act[2].Select() }
}

function Find-Composer($root) {
  foreach ($e in $root.FindAll($TS::Descendants,
      (New-Object System.Windows.Automation.PropertyCondition($AE::ControlTypeProperty, $CT::Edit)))) {
    if ($e.Current.Name -like "*输入消息*") { return $e }
  }
  return $null
}

function Count-Messages($root) {
  $list = $root.FindFirst($TS::Descendants,
    (New-Object System.Windows.Automation.PropertyCondition($AE::AutomationIdProperty, "MessageList")))
  if ($null -eq $list) { return -1 }
  return $list.FindAll($TS::Descendants,
    (New-Object System.Windows.Automation.PropertyCondition($AE::ControlTypeProperty, $CT::ListItem))).Count
}

$ok = $true

# ── 0. 回到会话列表 ───────────────────────────────────────────
# 上一次运行会停在对话页，而对话页里**没有**会话列表 —— 不先退回去，
# 下面那步会以「找不到会话」失败，看起来像导航坏了。
$win = Find-Window
if ($null -ne (Find-Composer $win)) {
  $back = Find-ByName $win "返回会话列表" $CT::Button
  if ($null -ne $back) {
    Activate $back "返回会话列表"
    Start-Sleep -Milliseconds 1000
    Write-Host "  ✓ 已返回会话列表"
  }
}
$win = Find-Window

# ── 1. 打开会话 ───────────────────────────────────────────────
$item = Find-ByName $win $Session $null
if ($null -eq $item) { $item = Find-ByNameLike $win $Session }
if ($null -eq $item) { throw "会话列表里找不到「$Session」" }

Activate $item $Session
Start-Sleep -Milliseconds 1200
$win = Find-Window

$composer = Find-Composer $win
if ($null -eq $composer) { throw "没有进入对话页（找不到输入框）" }
Write-Host "✓ 已进入会话：$Session"

# ── 2. 往输入框写文本 ─────────────────────────────────────────
$value = $composer.GetCurrentPattern([System.Windows.Automation.ValuePattern]::Pattern)
$value.SetValue($Text)
Start-Sleep -Milliseconds 300

$readback = (Find-Composer (Find-Window)).GetCurrentPattern(
  [System.Windows.Automation.ValuePattern]::Pattern).Current.Value
if ($readback -ne $Text) { throw "输入框回读不一致：'$readback' ≠ '$Text'" }
Write-Host "✓ 输入框已写入：$Text"

# ── 3. 发送前的条数 ───────────────────────────────────────────
$before = Count-Messages $win
Write-Host "  发送前消息条数：$before"

# ── 4. 点「发送」 ─────────────────────────────────────────────
$send = Find-ByName $win "发送" $CT::Button
if ($null -eq $send) { throw "找不到「发送」按钮" }
Activate $send "发送"
Start-Sleep -Milliseconds 900

$win = Find-Window
$after = Count-Messages $win
Write-Host "  发送后消息条数：$after"

# ── 5. 断言：列表多了一条，且正文就是刚发的 ───────────────────
if ($after -gt $before) {
  Write-Host "✓ MessageList 多出一条（$before → $after）"
} else {
  Write-Host "✗ MessageList 没有变化（$before → $after）—— 发送没有落到列表上"
  $ok = $false
}

# 正文在 ListItem 的**子树**里。
#
# **实测**：ListItem 自身的 Name 是数据对象的类型名（`ArcDeck.ChatMessage`），
# 不是正文 —— 拿它去断言「文本进去了没」会得到假阴性（第一版就是这么错的）。
$list = $win.FindFirst($TS::Descendants,
  (New-Object System.Windows.Automation.PropertyCondition($AE::AutomationIdProperty, "MessageList")))
$items = $list.FindAll($TS::Descendants,
  (New-Object System.Windows.Automation.PropertyCondition($AE::ControlTypeProperty, $CT::ListItem)))
$inLast = @()
foreach ($t in $items[$items.Count - 1].FindAll($TS::Descendants,
    (New-Object System.Windows.Automation.PropertyCondition($AE::ControlTypeProperty, $CT::Text)))) {
  $inLast += $t.Current.Name
}
$lastText = $inLast -join " | "
Write-Host "  最后一条的正文：$lastText"
if ($lastText -like "*$Text*") {
  Write-Host "✓ 最后一条正是刚发出的文本"
} else {
  Write-Host "✗ 最后一条里没有刚发出的文本"
  $ok = $false
}

# ── 6. 输入框应被清空 ─────────────────────────────────────────
$composer2 = Find-Composer (Find-Window)
if ($null -ne $composer2) {
  $left = $composer2.GetCurrentPattern([System.Windows.Automation.ValuePattern]::Pattern).Current.Value
  if ([string]::IsNullOrEmpty($left)) { Write-Host "✓ 发送后输入框已清空" }
  else { Write-Host "✗ 发送后输入框里还留着：'$left'"; $ok = $false }
}

# ── 7. 会话列表的摘要是否跟着变 ───────────────────────────────
# 对应 AGENTS.md 里那个曾经「无法验证」的项：发完之后列表明细要更新。
$backBtn = Find-ByName (Find-Window) "返回会话列表" $CT::Button
if ($null -ne $backBtn) {
  Activate $backBtn "返回会话列表"
  Start-Sleep -Milliseconds 1000
  if ($null -ne (Find-ByNameLike (Find-Window) $Text)) {
    Write-Host "✓ 会话列表里出现了含「$Text」的条目（摘要已更新）"
  } else {
    Write-Host "✗ 会话列表里没找到「$Text」—— 摘要没跟着更新"
    $ok = $false
  }
}

if ($Out) {
  Add-Type -AssemblyName System.Drawing
  $r = (Find-Window).Current.BoundingRectangle
  $bmp = New-Object System.Drawing.Bitmap([int]$r.Width, [int]$r.Height)
  $g = [System.Drawing.Graphics]::FromImage($bmp)
  $g.CopyFromScreen([int]$r.X, [int]$r.Y, 0, 0, $bmp.Size)
  $bmp.Save($Out, [System.Drawing.Imaging.ImageFormat]::Png)
  $g.Dispose(); $bmp.Dispose()
  Write-Host "已截图：$Out"
}

Write-Host ""
Write-Host $(if ($ok) { "结论：发送通路成立 ✓" } else { "结论：发送通路有断点 ✗" })
exit $(if ($ok) { 0 } else { 1 })
