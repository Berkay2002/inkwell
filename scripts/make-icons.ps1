# Draws Inkwell's icons from the ink-drop mark in the title bar (src/components/Chrome.tsx, accent #2e4b8f):
#   - the app icon: src-tauri/icons/icon.ico (16..256) and the PNG set tauri-build and the bundler read
#   - the PDF document icon Explorer shows for Inkwell's .pdf registration: src-tauri/icons/pdf-document.ico
# Run from the repo root:  pwsh -File scripts/make-icons.ps1
param([string]$Dir = "src-tauri/icons")
Add-Type -AssemblyName System.Drawing
$PtF = [System.Drawing.PointF]
$accent = [System.Drawing.Color]::FromArgb(46, 75, 143)
$accentLight = [System.Drawing.Color]::FromArgb(58, 94, 176)
$tint = [System.Drawing.Color]::FromArgb(221, 229, 243)

# The mark's path in its own 24-unit box: M12 2.5 c3.6 4.3 6.5 7.5 6.5 11 a6.5 6.5 0 0 1 -13 0 c0 -3.5 2.9 -6.7 6.5 -11 Z
function DropPath {
  $p = New-Object System.Drawing.Drawing2D.GraphicsPath
  $p.AddBezier($PtF::new(12, 2.5), $PtF::new(15.6, 6.8), $PtF::new(18.5, 10), $PtF::new(18.5, 13.5))
  $p.AddArc([System.Drawing.RectangleF]::new(5.5, 7, 13, 13), 0, 180)
  $p.AddBezier($PtF::new(5.5, 13.5), $PtF::new(5.5, 10), $PtF::new(8.4, 6.8), $PtF::new(12, 2.5))
  $p.CloseFigure()
  $p
}

# Draws the drop with its box centre at (cx, cy) and `h` px tall (the mark is 17.5 units tall, centred at y 11.25).
function FillDrop($g, $brush, [double]$cx, [double]$cy, [double]$h) {
  $k = $h / 17.5
  $m = New-Object System.Drawing.Drawing2D.Matrix
  $m.Translate([single]$cx, [single]$cy)
  $m.Scale([single]$k, [single]$k)
  $m.Translate(-12, -11.25)
  $path = DropPath
  $path.Transform($m)
  $g.FillPath($brush, $path)
}

function Canvas([int]$s) {
  $bmp = New-Object System.Drawing.Bitmap $s, $s, ([System.Drawing.Imaging.PixelFormat]::Format32bppArgb)
  $g = [System.Drawing.Graphics]::FromImage($bmp)
  $g.SmoothingMode = 'AntiAlias'; $g.TextRenderingHint = 'AntiAliasGridFit'; $g.Clear([System.Drawing.Color]::Transparent)
  $g.ScaleTransform($s / 256.0, $s / 256.0)
  @{ Bmp = $bmp; G = $g }
}

function RoundedRect([double]$x, [double]$y, [double]$w, [double]$h, [double]$r) {
  $p = New-Object System.Drawing.Drawing2D.GraphicsPath
  $d = 2 * $r
  $p.AddArc($x, $y, $d, $d, 180, 90); $p.AddArc($x + $w - $d, $y, $d, $d, 270, 90)
  $p.AddArc($x + $w - $d, $y + $h - $d, $d, $d, 0, 90); $p.AddArc($x, $y + $h - $d, $d, $d, 90, 90)
  $p.CloseFigure()
  $p
}

function Png($c) {
  $c.G.Dispose()
  $ms = New-Object System.IO.MemoryStream
  $c.Bmp.Save($ms, [System.Drawing.Imaging.ImageFormat]::Png)
  $c.Bmp.Dispose()
  , $ms.ToArray()
}

# App icon: a white drop on an accent tile.
function AppIcon([int]$s) {
  $c = Canvas $s
  $g = $c.G
  $tile = RoundedRect 8 8 240 240 56
  $grad = New-Object System.Drawing.Drawing2D.LinearGradientBrush ([System.Drawing.PointF]::new(0, 8)), ([System.Drawing.PointF]::new(0, 248)), $accentLight, $accent
  $g.FillPath($grad, $tile)
  FillDrop $g ([System.Drawing.Brushes]::White) 128 128 150
  Png $c
}

# PDF document icon: a page with a folded corner, the drop on it, a PDF label.
function PdfIcon([int]$s) {
  $c = Canvas $s
  $g = $c.G
  $page = [System.Drawing.PointF[]]@($PtF::new(44, 20), $PtF::new(160, 20), $PtF::new(212, 72), $PtF::new(212, 236), $PtF::new(44, 236))
  $fold = [System.Drawing.PointF[]]@($PtF::new(160, 20), $PtF::new(160, 72), $PtF::new(212, 72))
  $g.FillPolygon([System.Drawing.Brushes]::White, $page)
  $g.FillPolygon((New-Object System.Drawing.SolidBrush $tint), $fold)
  $pen = New-Object System.Drawing.Pen $accent, 8
  $pen.LineJoin = 'Round'
  $g.DrawPolygon($pen, $page)
  $g.DrawPolygon($pen, $fold)
  FillDrop $g (New-Object System.Drawing.SolidBrush $accent) 128 132 104
  $font = New-Object System.Drawing.Font 'Segoe UI', 26, ([System.Drawing.FontStyle]::Bold), ([System.Drawing.GraphicsUnit]::Pixel)
  $fmt = New-Object System.Drawing.StringFormat; $fmt.Alignment = 'Center'
  $g.DrawString('PDF', $font, (New-Object System.Drawing.SolidBrush $accent), [System.Drawing.RectangleF]::new(44, 200, 168, 30), $fmt)
  Png $c
}

function WriteIco($draw, [int[]]$sizes, [string]$path) {
  $frames = $sizes | ForEach-Object { , (& $draw $_) }
  $ms = New-Object System.IO.MemoryStream
  $bw = New-Object System.IO.BinaryWriter $ms
  $bw.Write([uint16]0); $bw.Write([uint16]1); $bw.Write([uint16]$sizes.Count)
  $offset = 6 + 16 * $sizes.Count
  for ($i = 0; $i -lt $sizes.Count; $i++) {
    $dim = if ($sizes[$i] -ge 256) { 0 } else { $sizes[$i] }
    $bw.Write([byte]$dim); $bw.Write([byte]$dim); $bw.Write([byte]0); $bw.Write([byte]0)
    $bw.Write([uint16]1); $bw.Write([uint16]32); $bw.Write([uint32]$frames[$i].Length); $bw.Write([uint32]$offset)
    $offset += $frames[$i].Length
  }
  foreach ($f in $frames) { $bw.Write($f) }
  $bw.Flush()
  [System.IO.File]::WriteAllBytes($path, $ms.ToArray())
}

$root = Join-Path (Get-Location) $Dir
New-Item -ItemType Directory -Force -Path $root | Out-Null
WriteIco ${function:AppIcon} @(16, 20, 24, 32, 40, 48, 64, 128, 256) (Join-Path $root 'icon.ico')
WriteIco ${function:PdfIcon} @(16, 20, 24, 32, 40, 48, 64, 128, 256) (Join-Path $root 'pdf-document.ico')
# the PNGs tauri-build (window icon), the bundler and the Store/tile sizes read
$png = [ordered]@{
  '32x32.png' = 32; '128x128.png' = 128; '128x128@2x.png' = 256; 'icon.png' = 512; 'StoreLogo.png' = 50
  'Square30x30Logo.png' = 30; 'Square44x44Logo.png' = 44; 'Square71x71Logo.png' = 71; 'Square89x89Logo.png' = 89
  'Square107x107Logo.png' = 107; 'Square142x142Logo.png' = 142; 'Square150x150Logo.png' = 150
  'Square284x284Logo.png' = 284; 'Square310x310Logo.png' = 310
}
foreach ($k in $png.Keys) { [System.IO.File]::WriteAllBytes((Join-Path $root $k), (AppIcon $png[$k])) }
"wrote icon.ico, pdf-document.ico and $($png.Count) PNGs in $Dir"
