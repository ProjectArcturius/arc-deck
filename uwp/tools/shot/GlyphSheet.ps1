<#
  Dump a range of icon-font codepoints as a labelled contact sheet.

  Why: codepoints are NOT shared between "Segoe MDL2 Assets" (Windows 10 icon font)
  and "Segoe Fluent Icons" (Windows 11). The same value can be a folder in one and
  something unrelated in the other -- E8B7 is a tabbed folder in Fluent but not in
  MDL2. That makes `{ThemeResource SymbolThemeFontFamily}` unsafe to override on its
  own: every hardcoded glyph in the app silently re-points at a different picture.

  Use this to find the right codepoint in a given font instead of guessing.

  Usage:
    powershell.exe -File GlyphSheet.ps1 -Font "Segoe MDL2 Assets" -From E8A0 -To E8FF -Out sheet.png
#>
param(
  [string]$Font = "Segoe MDL2 Assets",
  [string]$From = "E8A0",
  [string]$To = "E8FF",
  [Parameter(Mandatory=$true)][string]$Out,
  [int]$Cell = 56,
  [int]$Cols = 8
)

Add-Type -AssemblyName System.Drawing

$lo = [Convert]::ToInt32($From, 16)
$hi = [Convert]::ToInt32($To, 16)
$codes = $lo..$hi
$rows = [int][math]::Ceiling($codes.Count / $Cols)
$labH = 14
$cw = $Cell + 4
$ch = $Cell + $labH + 4

$bmp = New-Object System.Drawing.Bitmap(($Cols * $cw + 8), ($rows * $ch + 34))
$g = [System.Drawing.Graphics]::FromImage($bmp)
$g.Clear([System.Drawing.Color]::FromArgb(255, 20, 20, 20))
$g.TextRenderingHint = [System.Drawing.Text.TextRenderingHint]::AntiAliasGridFit

$title = New-Object System.Drawing.Font("Segoe UI", 12)
$lab = New-Object System.Drawing.Font("Consolas", 7)
$f = New-Object System.Drawing.Font($Font, ($Cell * 0.62), [System.Drawing.FontStyle]::Regular, [System.Drawing.GraphicsUnit]::Pixel)
$fmt = [System.Drawing.StringFormat]::GenericTypographic
$fmt.Alignment = [System.Drawing.StringAlignment]::Center
$fmt.LineAlignment = [System.Drawing.StringAlignment]::Center

$g.DrawString("$Font   $From..$To", $title, [System.Drawing.Brushes]::White, 6, 4)

$i = 0
foreach ($cp in $codes) {
  $cx = 4 + ($i % $Cols) * $cw
  $cy = 30 + [int][math]::Floor($i / $Cols) * $ch
  $rect = New-Object System.Drawing.RectangleF($cx, $cy, $Cell, $Cell)
  $g.DrawString([char]$cp, $f, [System.Drawing.Brushes]::White, $rect, $fmt)
  $g.DrawString(("{0:X4}" -f $cp), $lab, [System.Drawing.Brushes]::Gray, ($cx + 2), ($cy + $Cell + 1))
  $i++
}

$bmp.Save($Out, [System.Drawing.Imaging.ImageFormat]::Png)
$g.Dispose(); $bmp.Dispose(); $f.Dispose(); $title.Dispose(); $lab.Dispose()
"saved $Out  ($($bmp.Width) x $($bmp.Height), $($codes.Count) glyphs, font '$Font')"
