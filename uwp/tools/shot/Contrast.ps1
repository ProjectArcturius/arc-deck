<#
  Contrast check for the Saiku Deck UI, with the region list kept in a file.

  Why a file: passing a semicolon-separated region list through
  `pwsh -Command` -> `powershell.exe -File` mangles it (the outer shell re-parses the
  separators), and the resulting error is a confusing "cannot convert to Int32".
  Keeping the list on disk removes every quoting layer.

  Why this exists at all: "the text is hard to read" is not actionable, but a WCAG
  contrast ratio is. Anything under about 2:1 is the "invisible text" failure.

  Region file format, one per line, '#' starts a comment:
      label,x,y,w,h

  Usage:
    powershell.exe -File Contrast.ps1 -Shot shot.png
    powershell.exe -File Contrast.ps1 -Shot shot.png -File other-regions.txt
#>
param(
  [Parameter(Mandatory=$true)][string]$Shot,
  [string]$File = "contrast-regions.txt"
)

Add-Type -AssemblyName System.Drawing

if (-not (Test-Path $File)) { Write-Error "region file not found: $File"; exit 1 }

$regions = @()
foreach ($line in (Get-Content $File)) {
  $t = $line.Trim()
  if ($t -eq "" -or $t.StartsWith("#")) { continue }
  $f = $t -split ','
  if ($f.Count -lt 5) { continue }
  $regions += ,@($f[0].Trim(), [int]$f[1], [int]$f[2], [int]$f[3], [int]$f[4])
}
"regions: $($regions.Count)"

function Luminance($c) {
  $f = {
    param($v)
    $s = $v / 255.0
    if ($s -le 0.03928) { return $s / 12.92 }
    return [math]::Pow((($s + 0.055) / 1.055), 2.4)
  }
  return 0.2126 * (& $f $c.R) + 0.7152 * (& $f $c.G) + 0.0722 * (& $f $c.B)
}

function Ratio($a, $b) {
  $l1 = Luminance $a; $l2 = Luminance $b
  if ($l1 -lt $l2) { $t = $l1; $l1 = $l2; $l2 = $t }
  return ($l1 + 0.05) / ($l2 + 0.05)
}

$src = [System.Drawing.Image]::FromFile($Shot)
$bmp = New-Object System.Drawing.Bitmap($src.Width, $src.Height)
$g = [System.Drawing.Graphics]::FromImage($bmp)
$g.DrawImage($src, 0, 0, $src.Width, $src.Height)
$g.Dispose()

"image: $($src.Width) x $($src.Height)"
""
"{0,-20} {1,-9} {2,-9} {3,7}  {4}" -f "region", "bg", "text", "ratio", "verdict"
"-" * 70

foreach ($r in $regions) {
  $label = $r[0]; $x = $r[1]; $y = $r[2]; $w = $r[3]; $h = $r[4]

  if ($x -lt 0) { $x = 0 }
  if ($y -lt 0) { $y = 0 }
  if ($x + $w -gt $bmp.Width)  { $w = $bmp.Width - $x }
  if ($y + $h -gt $bmp.Height) { $h = $bmp.Height - $y }
  if ($w -le 0 -or $h -le 0) { "  $label : region empty after clamping"; continue }

  $hist = @{}
  $n = 0
  for ($yy = $y; $yy -lt ($y + $h); $yy++) {
    for ($xx = $x; $xx -lt ($x + $w); $xx++) {
      $c = $bmp.GetPixel($xx, $yy)
      $key = "$($c.R),$($c.G),$($c.B)"
      if ($hist.ContainsKey($key)) { $hist[$key]++ } else { $hist[$key] = 1 }
      $n++
    }
  }
  if ($n -eq 0) { continue }

  # Background := most common colour. Text := the colour whose luminance is farthest
  # from it, among colours covering a non-trivial share of pixels.
  $bgKey = ($hist.GetEnumerator() | Sort-Object Value -Descending | Select-Object -First 1).Key
  $bp = $bgKey -split ',' | ForEach-Object { [int]$_ }
  $bg = [System.Drawing.Color]::FromArgb(255, $bp[0], $bp[1], $bp[2])
  $bgLum = ($bg.R + $bg.G + $bg.B) / 3

  $best = $bg; $bestDelta = -1
  foreach ($kv in $hist.GetEnumerator()) {
    if ($kv.Value -lt [math]::Max(3, $n * 0.002)) { continue }
    $pp = $kv.Key -split ',' | ForEach-Object { [int]$_ }
    $c = [System.Drawing.Color]::FromArgb(255, $pp[0], $pp[1], $pp[2])
    $d = [math]::Abs((($c.R + $c.G + $c.B) / 3) - $bgLum)
    if ($d -gt $bestDelta) { $bestDelta = $d; $best = $c }
  }

  $ratio = Ratio $bg $best
  $verdict = if ($ratio -ge 4.5) { "AA ok" }
             elseif ($ratio -ge 3) { "AA large only" }
             elseif ($ratio -ge 2) { "WEAK" }
             else { "FAIL - effectively invisible" }

  "{0,-20} #{1:X2}{2:X2}{3:X2} #{4:X2}{5:X2}{6:X2} {7,7:F2}  {8}" -f `
    $label, $bg.R, $bg.G, $bg.B, $best.R, $best.G, $best.B, $ratio, $verdict
}

$bmp.Dispose(); $src.Dispose()
