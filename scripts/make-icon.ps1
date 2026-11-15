# Draws assets/winproc-gui.ico (16-256 px): a dark rounded tile with a bar chart.
# Usage: powershell -File scripts/make-icon.ps1   (needs Windows PowerShell for System.Drawing)
Add-Type -AssemblyName System.Drawing
$sizes = 16, 24, 32, 48, 64, 256
$pngs = @()
foreach ($s in $sizes) {
    $bmp = New-Object Drawing.Bitmap $s, $s
    $g = [Drawing.Graphics]::FromImage($bmp)
    $g.SmoothingMode = 'AntiAlias'
    $r = [single]($s * 0.18)
    $path = New-Object Drawing.Drawing2D.GraphicsPath
    $w = [single]($s - 1)
    $path.AddArc(0, 0, $r * 2, $r * 2, 180, 90)
    $path.AddArc($w - $r * 2, 0, $r * 2, $r * 2, 270, 90)
    $path.AddArc($w - $r * 2, $w - $r * 2, $r * 2, $r * 2, 0, 90)
    $path.AddArc(0, $w - $r * 2, $r * 2, $r * 2, 90, 90)
    $path.CloseFigure()
    $g.FillPath((New-Object Drawing.SolidBrush ([Drawing.Color]::FromArgb(255, 30, 33, 40))), $path)
    $colors = @([Drawing.Color]::FromArgb(255, 76, 154, 255), [Drawing.Color]::FromArgb(255, 63, 191, 127), [Drawing.Color]::FromArgb(255, 255, 176, 32))
    $heights = 0.45, 0.7, 0.3
    $pad = $s * 0.2; $gap = $s * 0.06
    $bw = ($s - 2 * $pad - 2 * $gap) / 3
    for ($i = 0; $i -lt 3; $i++) {
        $h = ($s - 2 * $pad) * $heights[$i]
        $x = $pad + $i * ($bw + $gap)
        $g.FillRectangle((New-Object Drawing.SolidBrush $colors[$i]), [single]$x, [single]($s - $pad - $h), [single]$bw, [single]$h)
    }
    $g.Dispose()
    $ms = New-Object IO.MemoryStream
    $bmp.Save($ms, [Drawing.Imaging.ImageFormat]::Png)
    $pngs += , $ms.ToArray()
    $bmp.Dispose()
}
$out = New-Object IO.MemoryStream
$bw = New-Object IO.BinaryWriter $out
$bw.Write([uint16]0); $bw.Write([uint16]1); $bw.Write([uint16]$sizes.Count)
$offset = 6 + 16 * $sizes.Count
for ($i = 0; $i -lt $sizes.Count; $i++) {
    $s = $sizes[$i]; $d = if ($s -ge 256) { 0 } else { $s }
    $bw.Write([byte]$d); $bw.Write([byte]$d); $bw.Write([byte]0); $bw.Write([byte]0)
    $bw.Write([uint16]1); $bw.Write([uint16]32)
    $bw.Write([uint32]$pngs[$i].Length); $bw.Write([uint32]$offset)
    $offset += $pngs[$i].Length
}
foreach ($p in $pngs) { $bw.Write($p) }
[IO.File]::WriteAllBytes("$PSScriptRoot\..\assets\winproc-gui.ico", $out.ToArray())
