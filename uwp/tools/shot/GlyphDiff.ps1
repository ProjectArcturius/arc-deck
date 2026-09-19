<#
  Compare the two Windows icon fonts glyph by glyph.

  Why: on this machine `SymbolThemeFontFamily` (the platform key that selects the
  icon font) resolves to "Segoe Fluent Icons" -- the Windows 11 icon set. Windows 10
  used "Segoe MDL2 Assets". To restore the older look we need to know, per glyph,
  whether the two fonts actually differ and by how much. Eyeballing a screenshot is
  not good enough at 14-20px, so this renders each glyph in both fonts at large
  size and reports the fraction of pixels that differ.

  A near-zero diff means the two fonts share that glyph (safe to switch).
  A large diff means the glyph was redesigned in the Fluent icon set.

  Usage:
    powershell.exe -File GlyphDiff.ps1
    powershell.exe -File GlyphDiff.ps1 -Sheet sheet.png   # also write a visual grid
#>
param(
  [string]$Sheet,
  [int]$Size = 128
)

Add-Type -AssemblyName System.Drawing

$MDL2 = "Segoe MDL2 Assets"
$FLUENT = "Segoe Fluent Icons"

# Codepoints the app itself uses, plus the ones WinUI 2 picks internally for the
# NavigationView (hamburger E700, settings E713, back E72B, chevron E70D).
$glyphs = [ordered]@{
  "E700 GlobalNavButton (hamburger)" = 0xE700
  "E713 Setting"                     = 0xE713
  "E72B Back"                        = 0xE72B
  "E72C Refresh"                     = 0xE72C
  "E734 FavoriteStar"                = 0xE734
  "E735 FavoriteStarFill"            = 0xE735
  "E70D ChevronDown"                 = 0xE70D
  "E76C ChevronRight"                = 0xE76C
  "E8B7 Folder"                      = 0xE8B7
  "E8C8 Copy"                        = 0xE8C8
  "E756 (tool card header)"          = 0xE756
  "E721 Search"                      = 0xE721
  "E74D Delete"                      = 0xE74D
  "E8A5 Document"                    = 0xE8A5
  "E10F Home"                        = 0xE10F
  "E8BD (comment/chat)"              = 0xE8BD
}

function Render-Glyph([string]$fontName, [int]$cp, [int]$size) {
  $bmp = New-Object System.Drawing.Bitmap($size, $size)
  $g = [System.Drawing.Graphics]::FromImage($bmp)
  $g.Clear([System.Drawing.Color]::Black)
  $g.TextRenderingHint = [System.Drawing.Text.TextRenderingHint]::AntiAliasGridFit
  $fmt = [System.Drawing.StringFormat]::GenericTypographic
  $fmt.Alignment = [System.Drawing.StringAlignment]::Center
  $fmt.LineAlignment = [System.Drawing.StringAlignment]::Center
  $f = New-Object System.Drawing.Font($fontName, ($size * 0.62), [System.Drawing.FontStyle]::Regular, [System.Drawing.GraphicsUnit]::Pixel)
  $g.DrawString([char]$cp, $f, [System.Drawing.Brushes]::White, (New-Object System.Drawing.RectangleF(0,0,$size,$size)), $fmt)
  $f.Dispose(); $g.Dispose()
  return $bmp
}

function Diff-Ratio($a, $b) {
  $n = 0; $d = 0
  for ($y = 0; $y -lt $a.Height; $y++) {
    for ($x = 0; $x -lt $a.Width; $x++) {
      $pa = $a.GetPixel($x, $y); $pb = $b.GetPixel($x, $y)
      $va = [int](($pa.R + $pa.G + $pa.B) / 3)
      $vb = [int](($pb.R + $pb.G + $pb.B) / 3)
      if ($va -gt 24 -or $vb -gt 24) { $n++ }
      if ([math]::Abs($va - $vb) -gt 40) { $d++ }
    }
  }
  if ($n -eq 0) { return -1.0 }
  return [double]$d / $n
}

"{0,-34} {1,7} {2,7} {3,9}  {4}" -f "glyph", "cp", "ink", "diff%", "verdict"
"-" * 76

$rows = @()
foreach ($k in $glyphs.Keys) {
  $cp = $glyphs[$k]
  $b1 = Render-Glyph $MDL2 $cp $Size
  $b2 = Render-Glyph $FLUENT $cp $Size
  $r = Diff-Ratio $b1 $b2
  $ink1 = 0
  for ($y = 0; $y -lt $b1.Height; $y += 2) {
    for ($x = 0; $x -lt $b1.Width; $x += 2) {
      $p = $b1.GetPixel($x, $y); if ((($p.R + $p.G + $p.B) / 3) -gt 24) { $ink1++ }
    }
  }
  $verdict = if ($r -lt 0) { "MISSING in one font" }
             elseif ($r -lt 0.02) { "identical" }
             elseif ($r -lt 0.15) { "near-identical" }
             else { "REDESIGNED" }
  "{0,-34} {1,7:X4} {2,7} {3,8:F2}  {4}" -f $k, $cp, $ink1, ($r * 100), $verdict
  $rows += ,@($k, $cp, $r)
  $b1.Dispose(); $b2.Dispose()
}

if ($Sheet) {
  # NB: do not name the bitmap $Sheet -- the param is a path string.
  $cell = 96; $pad = 4
  $cols = 4
  $half = [int]($cell / 2)
  $rowH = $half + 22
  $n = $glyphs.Count
  $rowCount = [int][math]::Ceiling($n / $cols)
  $w = $cols * ($cell + $pad) + $pad
  $h = $rowCount * $rowH + 40
  $canvas = New-Object System.Drawing.Bitmap($w, $h)
  $sg = [System.Drawing.Graphics]::FromImage($canvas)
  $sg.Clear([System.Drawing.Color]::FromArgb(255, 24, 24, 24))
  $titleFont = New-Object System.Drawing.Font("Segoe UI", 11)
  $labelFont = New-Object System.Drawing.Font("Consolas", 7)
  $sg.DrawString("left = MDL2 (Win10)   right = Fluent (Win11)", $titleFont, [System.Drawing.Brushes]::White, 6, 6)

  $i = 0
  foreach ($k in $glyphs.Keys) {
    $cp = $glyphs[$k]
    $cx = $pad + ($i % $cols) * ($cell + $pad)
    $cy = 34 + [int][math]::Floor($i / $cols) * $rowH
    $b1 = Render-Glyph $MDL2 $cp $cell
    $b2 = Render-Glyph $FLUENT $cp $cell
    $r1 = New-Object System.Drawing.Rectangle($cx, $cy, ($half - 2), ($half - 2))
    $r2 = New-Object System.Drawing.Rectangle(($cx + $half), $cy, ($half - 2), ($half - 2))
    $sg.DrawImage($b1, $r1)
    $sg.DrawImage($b2, $r2)
    $sg.DrawString(("{0:X4}" -f $cp), $labelFont, [System.Drawing.Brushes]::Gray, $cx, ($cy + $half + 1))
    $b1.Dispose(); $b2.Dispose()
    $i++
  }
  $canvas.Save($Sheet, [System.Drawing.Imaging.ImageFormat]::Png)
  $sg.Dispose(); $canvas.Dispose()
  "saved $Sheet"
}
