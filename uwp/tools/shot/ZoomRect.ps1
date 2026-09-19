<#
  Crop a raw rectangle out of a screenshot and magnify it with nearest-neighbour.

  Companion to Crop.ps1: that one uses UIA to locate an element, which does not help
  when the glyph sits in the row's leading gutter and UIA only exposes the text. This
  takes an explicit pixel rectangle instead.

  Usage:
    powershell.exe -File ZoomRect.ps1 -Shot in.png -Out out.png -Rect 500,170,120,50 -Zoom 8
#>
param(
  [Parameter(Mandatory=$true)][string]$Shot,
  [Parameter(Mandatory=$true)][string]$Out,
  [Parameter(Mandatory=$true)][string]$Rect,   # x,y,w,h in screenshot pixels
  [int]$Zoom = 6
)

Add-Type -AssemblyName System.Drawing

$p = $Rect -split ',' | ForEach-Object { [int]$_ }
$x = $p[0]; $y = $p[1]; $w = $p[2]; $h = $p[3]

$src = [System.Drawing.Image]::FromFile($Shot)
"source: $($src.Width) x $($src.Height)"

# Clamp to the image so an overshooting rect still produces something inspectable.
if ($x -lt 0) { $x = 0 }
if ($y -lt 0) { $y = 0 }
if ($x + $w -gt $src.Width)  { $w = $src.Width - $x }
if ($y + $h -gt $src.Height) { $h = $src.Height - $y }
if ($w -le 0 -or $h -le 0) { Write-Error "rect is empty after clamping"; exit 1 }

$crop = New-Object System.Drawing.Bitmap($w, $h)
$cg = [System.Drawing.Graphics]::FromImage($crop)
$cg.DrawImage($src, (New-Object System.Drawing.Rectangle(0, 0, $w, $h)),
              (New-Object System.Drawing.Rectangle($x, $y, $w, $h)),
              [System.Drawing.GraphicsUnit]::Pixel)
$cg.Dispose()

$zw = $w * $Zoom; $zh = $h * $Zoom
$big = New-Object System.Drawing.Bitmap($zw, $zh)
$bg = [System.Drawing.Graphics]::FromImage($big)
$bg.InterpolationMode = [System.Drawing.Drawing2D.InterpolationMode]::NearestNeighbor
$bg.PixelOffsetMode = [System.Drawing.Drawing2D.PixelOffsetMode]::Half
$bg.DrawImage($crop, 0, 0, $zw, $zh)
$bg.Dispose()
$big.Save($Out, [System.Drawing.Imaging.ImageFormat]::Png)
$big.Dispose(); $crop.Dispose(); $src.Dispose()
"saved $Out ($zw x $zh, zoom $Zoom, from $x,$y ${w}x${h})"
