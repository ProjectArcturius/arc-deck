<#
  Match a cropped glyph from a screenshot against reference renderings of the same
  codepoint in both icon fonts.

  Why: "the override took effect" is a claim that must be proven, not eyeballed.
  Pixel differencing whole screenshots fails because the app window drifts between
  captures. So instead: crop just the glyph, then score it against the MDL2 and
  Fluent renderings under translation and scale, and report which matches better.

  The score is mean absolute luminance difference over the union of inked pixels,
  minimised over a small search window so a few pixels of misalignment do not matter.

  Usage:
    powershell.exe -File MatchGlyph.ps1 -Crop crop.png -Codepoint E8B7
#>
param(
  [Parameter(Mandatory=$true)][string]$Crop,
  [Parameter(Mandatory=$true)][string]$Codepoint,   # hex, e.g. E8B7
  [int]$Search = 6,
  [double]$ScaleLo = 0.75,
  [double]$ScaleHi = 1.35
)

Add-Type -AssemblyName System.Drawing

$cp = [Convert]::ToInt32($Codepoint, 16)
$MDL2 = "Segoe MDL2 Assets"
$FLUENT = "Segoe Fluent Icons"

function Load-Gray([string]$path) {
  $img = [System.Drawing.Image]::FromFile($path)
  $bmp = New-Object System.Drawing.Bitmap($img.Width, $img.Height)
  $g = [System.Drawing.Graphics]::FromImage($bmp)
  $g.DrawImage($img, 0, 0, $img.Width, $img.Height)
  $g.Dispose(); $img.Dispose()
  return $bmp
}

function Gray($bmp, $x, $y) {
  $p = $bmp.GetPixel($x, $y)
  return [double](($p.R + $p.G + $p.B) / 3.0)
}

# Reference glyph: render at a known size on black, return bitmap + ink bbox.
function Render-Ref([string]$fontName, [int]$cp, [int]$px) {
  $pad = 8
  $bmp = New-Object System.Drawing.Bitmap(($px + 2 * $pad), ($px + 2 * $pad))
  $g = [System.Drawing.Graphics]::FromImage($bmp)
  $g.Clear([System.Drawing.Color]::Black)
  $g.TextRenderingHint = [System.Drawing.Text.TextRenderingHint]::AntiAliasGridFit
  $fmt = [System.Drawing.StringFormat]::GenericTypographic
  $fmt.Alignment = [System.Drawing.StringAlignment]::Center
  $fmt.LineAlignment = [System.Drawing.StringAlignment]::Center
  $f = New-Object System.Drawing.Font($fontName, $px, [System.Drawing.FontStyle]::Regular, [System.Drawing.GraphicsUnit]::Pixel)
  $g.DrawString([char]$cp, $f, [System.Drawing.Brushes]::White,
                (New-Object System.Drawing.RectangleF(0, 0, $bmp.Width, $bmp.Height)), $fmt)
  $f.Dispose(); $g.Dispose()
  return $bmp
}

$cropBmp = Load-Gray $Crop
"crop: $($cropBmp.Width) x $($cropBmp.Height)"

# Ink bbox of the crop.
$minX = $cropBmp.Width; $minY = $cropBmp.Height; $maxX = -1; $maxY = -1
for ($y = 0; $y -lt $cropBmp.Height; $y++) {
  for ($x = 0; $x -lt $cropBmp.Width; $x++) {
    if ((Gray $cropBmp $x $y) -gt 40) {
      if ($x -lt $minX) { $minX = $x }; if ($x -gt $maxX) { $maxX = $x }
      if ($y -lt $minY) { $minY = $y }; if ($y -gt $maxY) { $maxY = $y }
    }
  }
}
if ($maxX -lt 0) { Write-Error "crop has no ink above threshold 40 (raise contrast or widen the crop)"; exit 2 }
$bw = $maxX - $minX + 1; $bh = $maxY - $minY + 1
"ink bbox: $bw x $bh at ($minX,$minY)"
$target = [math]::Max($bw, $bh)

function Score-Ref($ref, [int]$scalePx) {
  # Rescale reference ink to the crop's ink size, then compare on a common canvas
  # over a translation search.
  $best = [double]::MaxValue
  foreach ($px in @($scalePx - 2, $scalePx - 1, $scalePx, $scalePx + 1, $scalePx + 2)) {
    if ($px -lt 8) { continue }
    $r = Render-Ref $ref.font $cp $px
    # ink bbox of reference
    $rMinX = $r.Width; $rMinY = $r.Height; $rMaxX = -1; $rMaxY = -1
    for ($y = 0; $y -lt $r.Height; $y++) {
      for ($x = 0; $x -lt $r.Width; $x++) {
        if ((Gray $r $x $y) -gt 40) {
          if ($x -lt $rMinX) { $rMinX = $x }; if ($x -gt $rMaxX) { $rMaxX = $x }
          if ($y -lt $rMinY) { $rMinY = $y }; if ($y -gt $rMaxY) { $rMaxY = $y }
        }
      }
    }
    if ($rMaxX -lt 0) { continue }
    $rw = $rMaxX - $rMinX + 1; $rh = $rMaxY - $rMinY + 1
    $scale = [double]$target / [math]::Max($rw, $rh)

    # Sum over union of ink in either image, with a ±Search translation sweep.
    for ($dy = -$Search; $dy -le $Search; $dy += 2) {
      for ($dx = -$Search; $dx -le $Search; $dx += 2) {
        $sum = 0.0; $n = 0
        for ($y = 0; $y -lt $bh; $y++) {
          for ($x = 0; $x -lt $bw; $x++) {
            $cv = Gray $cropBmp ($minX + $x) ($minY + $y)
            # sample reference at the scaled position
            $rx = [int][math]::Round($rMinX + ($x + $dx) / $scale)
            $ry = [int][math]::Round($rMinY + ($y + $dy) / $scale)
            $rv = 0.0
            if ($rx -ge 0 -and $rx -lt $r.Width -and $ry -ge 0 -and $ry -lt $r.Height) {
              $rv = Gray $r $rx $ry
            }
            if ($cv -gt 40 -or $rv -gt 40) { $sum += [math]::Abs($cv - $rv); $n++ }
          }
        }
        if ($n -gt 0) {
          $m = $sum / $n
          if ($m -lt $best) { $best = $m }
        }
      }
    }
    $r.Dispose()
  }
  return $best
}

$mdl2 = [pscustomobject]@{ font = $MDL2 }
$fluent = [pscustomobject]@{ font = $FLUENT }

# Start near the crop's own ink size; the sweep inside covers scale error.
$guess = [int][math]::Max(16, $target)
$sM = Score-Ref $mdl2 $guess
$sF = Score-Ref $fluent $guess

""
"score vs Segoe MDL2 Assets (Win10) : {0:F2}" -f $sM
"score vs Segoe Fluent Icons (Win11): {0:F2}" -f $sF
""
$win = if ($sM -lt $sF) { "MDL2  (Windows 10 icon set)" } else { "Fluent (Windows 11 icon set)" }
$margin = [math]::Abs($sM - $sF)
"verdict: {0}   (margin {1:F2}{2})" -f $win, $margin, $(if ($margin -lt 4) { " -- TOO CLOSE TO CALL" } else { "" })

$cropBmp.Dispose()
