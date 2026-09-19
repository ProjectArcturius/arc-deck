<#
  Compare two screenshots and report WHERE they differ.

  Why: this is the objective control experiment for the appearance overrides. Build
  with the override, screenshot; build without it, screenshot; then this tells you
  which screen regions actually changed and by how much. Eyeballing two 1920x1080
  dark screenshots side by side is exactly how you end up "fixing" something that
  never took effect.

  Images are normalised to the smaller of the two (the window is resizable, so the
  captures may not be the same size).

  Usage:
    powershell.exe -File ImgDiff.ps1 -A a.png -B b.png
    powershell.exe -File ImgDiff.ps1 -A a.png -B b.png -Grid 16 -Out diff.png
#>
param(
  [Parameter(Mandatory=$true)][string]$A,
  [Parameter(Mandatory=$true)][string]$B,
  [int]$Grid = 12,
  [int]$Threshold = 30,
  [string]$Out
)

Add-Type -AssemblyName System.Drawing

$ia = [System.Drawing.Image]::FromFile($A)
$ib = [System.Drawing.Image]::FromFile($B)
"$A  $($ia.Width)x$($ia.Height)"
"$B  $($ib.Width)x$($ib.Height)"

$w = [int][math]::Min($ia.Width, $ib.Width)
$h = [int][math]::Min($ia.Height, $ib.Height)
"comparing common region: ${w}x${h}"

$ba = New-Object System.Drawing.Bitmap($w, $h)
$bb = New-Object System.Drawing.Bitmap($w, $h)
$g = [System.Drawing.Graphics]::FromImage($ba); $g.DrawImage($ia, 0, 0, $w, $h); $g.Dispose()
$g = [System.Drawing.Graphics]::FromImage($bb); $g.DrawImage($ib, 0, 0, $w, $h); $g.Dispose()

# Bucket the image into a coarse grid; per cell report the mean absolute difference.
$cw = [int][math]::Ceiling($w / $Grid)
$ch = [int][math]::Ceiling($h / $Grid)
$cells = @()
$totalDiff = 0; $totalPix = 0

for ($gy = 0; $gy -lt $Grid; $gy++) {
  $row = @()
  for ($gx = 0; $gx -lt $Grid; $gx++) {
    $x0 = $gx * $cw; $y0 = $gy * $ch
    $x1 = [int][math]::Min($x0 + $cw, $w); $y1 = [int][math]::Min($y0 + $ch, $h)
    $sum = 0; $n = 0
    for ($y = $y0; $y -lt $y1; $y++) {
      for ($x = $x0; $x -lt $x1; $x++) {
        $pa = $ba.GetPixel($x, $y); $pb = $bb.GetPixel($x, $y)
        $sum += [math]::Abs(([int]$pa.R + $pa.G + $pa.B) - ([int]$pb.R + $pb.G + $pb.B)) / 3
        $n++
      }
    }
    $mean = if ($n -gt 0) { $sum / $n } else { 0 }
    $row += $mean
    $totalDiff += $sum; $totalPix += $n
  }
  $cells += ,$row
}

"overall mean absolute difference: {0:F2} / 255" -f ($totalDiff / $totalPix)
""
"per-cell mean diff (rows top->bottom, cols left->right; '.' <1  ':' <3  '+' <8  '*' <20  '#' >=20)"
""

$hdr = "      "
for ($gx = 0; $gx -lt $Grid; $gx++) { $hdr += "{0,5}" -f $gx }
$hdr

for ($gy = 0; $gy -lt $Grid; $gy++) {
  $line = "{0,4}: " -f $gy
  foreach ($m in $cells[$gy]) {
    $ch2 = if ($m -lt 1) { "." } elseif ($m -lt 3) { ":" } elseif ($m -lt 8) { "+" } elseif ($m -lt 20) { "*" } else { "#" }
    $line += "{0,4}{1}" -f $ch2, ""
  }
  $line
}

""
"cell size: ${cw} x ${ch} px"

if ($Out) {
  # Amplify the difference so the changed regions are visible.
  $d = New-Object System.Drawing.Bitmap($w, $h)
  for ($y = 0; $y -lt $h; $y++) {
    for ($x = 0; $x -lt $w; $x++) {
      $pa = $ba.GetPixel($x, $y); $pb = $bb.GetPixel($x, $y)
      $v = [math]::Min(255, 6 * ([math]::Abs($pa.R - $pb.R) + [math]::Abs($pa.G - $pb.G) + [math]::Abs($pa.B - $pb.B)) / 3)
      $d.SetPixel($x, $y, [System.Drawing.Color]::FromArgb(255, [int]$v, [int]$v, [int]$v))
    }
  }
  $d.Save($Out, [System.Drawing.Imaging.ImageFormat]::Png)
  $d.Dispose()
  "saved amplified diff: $Out"
}

$ba.Dispose(); $bb.Dispose(); $ia.Dispose(); $ib.Dispose()
