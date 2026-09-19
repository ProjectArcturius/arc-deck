<#
  Report whether a rectangle of a screenshot contains anything visible.

  Why: "I cannot see the button in the picture" is a weak claim -- the region may be
  subtly non-empty, or the crop may be aimed at the wrong place. This reports the
  luminance range and the count of pixels above a threshold, so "empty" becomes a
  measurement. Useful for distinguishing "the control did not render" from
  "the control rendered but is nearly black on black".

  Coordinates are in the screenshot's own pixel space.

  Usage:
    powershell.exe -File Region.ps1 -Shot shot.png -Rect 1150,20,400,80
    powershell.exe -File Region.ps1 -Shot shot.png -Rect 1150,20,400,80 -Threshold 60
#>
param(
  [Parameter(Mandatory=$true)][string]$Shot,
  [Parameter(Mandatory=$true)][string]$Rect,
  [int]$Threshold = 60
)

Add-Type -AssemblyName System.Drawing

$p = $Rect -split ',' | ForEach-Object { [int]$_ }
$x = $p[0]; $y = $p[1]; $w = $p[2]; $h = $p[3]

$src = [System.Drawing.Image]::FromFile($Shot)
"image: $($src.Width) x $($src.Height)"
"region: $x,$y ${w}x${h}"

if ($x -lt 0) { $x = 0 }
if ($y -lt 0) { $y = 0 }
if ($x + $w -gt $src.Width)  { $w = $src.Width  - $x }
if ($y + $h -gt $src.Height) { $h = $src.Height - $y }
if ($w -le 0 -or $h -le 0) { Write-Error "region is empty after clamping"; exit 1 }

$bmp = New-Object System.Drawing.Bitmap($src.Width, $src.Height)
$g = [System.Drawing.Graphics]::FromImage($bmp)
$g.DrawImage($src, 0, 0, $src.Width, $src.Height)
$g.Dispose()

$min = 255; $max = 0; $sum = 0.0; $n = 0; $lit = 0
$samples = @{}

for ($yy = $y; $yy -lt ($y + $h); $yy++) {
  for ($xx = $x; $xx -lt ($x + $w); $xx++) {
    $c = $bmp.GetPixel($xx, $yy)
    $v = [int](($c.R + $c.G + $c.B) / 3)
    if ($v -lt $min) { $min = $v }
    if ($v -gt $max) { $max = $v }
    $sum += $v; $n++
    if ($v -ge $Threshold) { $lit++ }

    $key = "{0:X2}{1:X2}{2:X2}" -f $c.R, $c.G, $c.B
    if ($samples.ContainsKey($key)) { $samples[$key]++ } else { $samples[$key] = 1 }
  }
}

"pixels:        $n"
"luminance:     min=$min max=$max mean=$([math]::Round($sum / $n, 1))"
"above $Threshold`:     $lit  ($([math]::Round(100.0 * $lit / $n, 2))%)"
"verdict:       " + $(if ($lit -eq 0) { "EMPTY -- nothing rendered in this region" }
                       elseif ($lit -lt ($n * 0.002)) { "NEARLY EMPTY -- only a few lit pixels" }
                       else { "HAS CONTENT" })

""
"top colours:"
$samples.GetEnumerator() | Sort-Object Value -Descending | Select-Object -First 6 |
  ForEach-Object { "  #{0}  {1} px  ({2}%)" -f $_.Key, $_.Value, [math]::Round(100.0 * $_.Value / $n, 2) }

$bmp.Dispose(); $src.Dispose()
