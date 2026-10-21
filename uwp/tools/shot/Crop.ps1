<#
  Crop a UIA element's bounding box out of a full-screen screenshot and magnify it.

  Why: judging a 20px icon by eye in a 1920x1080 screenshot is guesswork. UIA gives
  the element's exact screen rectangle, and magnifying that crop makes the glyph
  shape unmistakable (filled MDL2 shape vs thin Fluent outline).

  Usage:
    powershell.exe -File Crop.ps1 -Element "Projects/arc-deck" -Shot shot.png -Out crop.png
    powershell.exe -File Crop.ps1 -Element "Saiku Deck 骨架与 IME 验证" -Shot shot.png -Out crop.png -Pad 8 -Zoom 8
#>
param(
  [Parameter(Mandatory=$true)][string]$Element,
  [Parameter(Mandatory=$true)][string]$Shot,
  [Parameter(Mandatory=$true)][string]$Out,
  [int]$Pad = 6,
  [int]$Zoom = 6
)

Add-Type -AssemblyName UIAutomationClient
Add-Type -AssemblyName UIAutomationTypes
Add-Type -AssemblyName System.Drawing

$AE = [System.Windows.Automation.AutomationElement]
$TS = [System.Windows.Automation.TreeScope]

$win = $AE::RootElement.FindFirst($TS::Children,
  (New-Object System.Windows.Automation.PropertyCondition($AE::NameProperty, "Saiku Deck")))
if ($null -eq $win) { Write-Error "Saiku Deck window not found"; exit 1 }

$all = $win.FindAll($TS::Descendants, [System.Windows.Automation.Condition]::TrueCondition)
$hit = $null
foreach ($e in $all) {
  if ($e.Current.Name -eq $Element) { $hit = $e; break }
}
if ($null -eq $hit) {
  foreach ($e in $all) { if ($e.Current.Name -like "*$Element*") { $hit = $e; break } }
}
if ($null -eq $hit) { Write-Error "element not found: $Element"; exit 2 }

$r = $hit.Current.BoundingRectangle
"element: $($hit.Current.Name)  [{0}]  rect={1},{2} {3}x{4}" -f `
  $hit.Current.ControlType.ProgrammaticName, $r.X, $r.Y, $r.Width, $r.Height

$src = [System.Drawing.Image]::FromFile($Shot)
$x = [int][math]::Max(0, $r.X - $Pad)
$y = [int][math]::Max(0, $r.Y - $Pad)
$w = [int][math]::Min($src.Width - $x, $r.Width + 2 * $Pad)
$h = [int][math]::Min($src.Height - $y, $r.Height + 2 * $Pad)
if ($w -le 0 -or $h -le 0) { Write-Error "element is off-screen"; exit 3 }

$crop = New-Object System.Drawing.Bitmap($w, $h)
$cg = [System.Drawing.Graphics]::FromImage($crop)
$cg.DrawImage($src, (New-Object System.Drawing.Rectangle(0, 0, $w, $h)),
              (New-Object System.Drawing.Rectangle($x, $y, $w, $h)),
              [System.Drawing.GraphicsUnit]::Pixel)
$cg.Dispose()

$zw = $w * $Zoom; $zh = $h * $Zoom
$big = New-Object System.Drawing.Bitmap($zw, $zh)
$bg = [System.Drawing.Graphics]::FromImage($big)
# NearestNeighbor: we want to see the actual pixels, not a smoothed guess.
$bg.InterpolationMode = [System.Drawing.Drawing2D.InterpolationMode]::NearestNeighbor
$bg.PixelOffsetMode = [System.Drawing.Drawing2D.PixelOffsetMode]::Half
$bg.DrawImage($crop, 0, 0, $zw, $zh)
$bg.Dispose()
$big.Save($Out, [System.Drawing.Imaging.ImageFormat]::Png)
$big.Dispose(); $crop.Dispose(); $src.Dispose()
"saved $Out ($zw x $zh, zoom $Zoom)"
