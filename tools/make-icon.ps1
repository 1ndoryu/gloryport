# Genera assets/gloryport.ico (entradas BMP clásicas, compatibles con
# CreateIconFromResourceEx) y assets/gloryport.png (para el README).
# Uso: pwsh tools/make-icon.ps1

Add-Type -AssemblyName System.Drawing

$root = Split-Path -Parent $PSScriptRoot
$icoPath = Join-Path $root 'assets\gloryport.ico'
$pngPath = Join-Path $root 'assets\gloryport.png'

function New-IconBitmap([int]$size) {
    $bmp = New-Object System.Drawing.Bitmap -ArgumentList (
        [int]$size, [int]$size, [System.Drawing.Imaging.PixelFormat]::Format32bppArgb)
    $g = [System.Drawing.Graphics]::FromImage($bmp)
    $g.SmoothingMode = [System.Drawing.Drawing2D.SmoothingMode]::AntiAlias
    $g.Clear([System.Drawing.Color]::Transparent)

    # Fondo: cuadrado redondeado con degradado azul oscuro.
    $pad = [Math]::Max(1, [int]($size * 0.06))
    $rect = New-Object System.Drawing.RectangleF -ArgumentList (
        [float]$pad, [float]$pad, [float]($size - 2 * $pad), [float]($size - 2 * $pad))
    $radius = [float]($size * 0.24)
    $path = New-Object System.Drawing.Drawing2D.GraphicsPath
    $d = 2 * $radius
    $path.AddArc($rect.X, $rect.Y, $d, $d, 180, 90)
    $path.AddArc($rect.Right - $d, $rect.Y, $d, $d, 270, 90)
    $path.AddArc($rect.Right - $d, $rect.Bottom - $d, $d, $d, 0, 90)
    $path.AddArc($rect.X, $rect.Bottom - $d, $d, $d, 90, 90)
    $path.CloseFigure()
    $brush = New-Object System.Drawing.Drawing2D.LinearGradientBrush -ArgumentList (
        $rect,
        [System.Drawing.Color]::FromArgb(255, 15, 23, 42),
        [System.Drawing.Color]::FromArgb(255, 30, 58, 138),
        [float]45)
    $g.FillPath($brush, $path)

    # Letra "G" en cian.
    $fontSize = [float]($size * 0.58)
    $font = New-Object System.Drawing.Font -ArgumentList (
        'Segoe UI', [float]$fontSize, [System.Drawing.FontStyle]::Bold,
        [System.Drawing.GraphicsUnit]::Pixel)
    $sf = New-Object System.Drawing.StringFormat
    $sf.Alignment = [System.Drawing.StringAlignment]::Center
    $sf.LineAlignment = [System.Drawing.StringAlignment]::Center
    $g.DrawString('G', $font, [System.Drawing.Brushes]::Cyan, $rect, $sf)

    # Punto verde (puerto activo) abajo a la derecha.
    $dot = [float]($size * 0.16)
    $g.FillEllipse([System.Drawing.Brushes]::Lime,
        $rect.Right - $dot * 1.5, $rect.Bottom - $dot * 1.5, $dot, $dot)

    $g.Dispose()
    return $bmp
}

function Get-BmpIconBytes([System.Drawing.Bitmap]$bmp) {
    # Convierte el bitmap en la sección DIB clásica de un ICO:
    # BITMAPINFOHEADER (40) + píxeles BGRA bottom-up + máscara AND (0 = opaco).
    $w = $bmp.Width
    $h = $bmp.Height
    $xorStride = $w * 4
    $andRowBytes = [Math]::Ceiling($w / 8.0)
    $andStride = ([int]$andRowBytes + 3) -band (-bnot 3)
    $sizeImage = $xorStride * $h + $andStride * $h

    $rect = New-Object System.Drawing.Rectangle -ArgumentList (0, 0, $w, $h)
    $data = $bmp.LockBits($rect, [System.Drawing.Imaging.ImageLockMode]::ReadOnly, [System.Drawing.Imaging.PixelFormat]::Format32bppArgb)
    try {
        $pixels = New-Object byte[] ($xorStride * $h)
        [System.Runtime.InteropServices.Marshal]::Copy($data.Scan0, $pixels, 0, $pixels.Length)
    } finally {
        $bmp.UnlockBits($data)
    }

    $ms = New-Object System.IO.MemoryStream
    $bw = New-Object System.IO.BinaryWriter($ms)
    # BITMAPINFOHEADER
    $bw.Write([int]40); $bw.Write([int]$w); $bw.Write([int]($h * 2))
    $bw.Write([int16]1); $bw.Write([int16]32); $bw.Write([int]0)
    $bw.Write([int]$sizeImage); $bw.Write([int]0); $bw.Write([int]0)
    $bw.Write([int]0); $bw.Write([int]0)
    # Píxeles bottom-up (BGRA ya en el orden del bitmap)
    for ($row = $h - 1; $row -ge 0; $row--) {
        $bw.Write($pixels, $row * $xorStride, $xorStride)
    }
    # Máscara AND: todo 0 (totalmente opaco)
    $andRow = New-Object byte[] $andStride
    for ($row = 0; $row -lt $h; $row++) {
        $bw.Write($andRow)
    }
    $bw.Flush()
    $bytes = $ms.ToArray()
    $bw.Dispose(); $ms.Dispose()
    # La coma evita que PowerShell desenvuelva el byte[] en el pipeline.
    return , $bytes
}

$sizes = 16, 24, 32, 48, 64
$images = @()
foreach ($s in $sizes) {
    $bmp = New-IconBitmap $s
    $images += , @{ Size = $s; Data = Get-BmpIconBytes $bmp }
    $bmp.Dispose()
}

# Ensambla el contenedor ICO.
$ms = New-Object System.IO.MemoryStream
$bw = New-Object System.IO.BinaryWriter($ms)
$bw.Write([int16]0); $bw.Write([int16]1); $bw.Write([int16]$images.Count)
$offset = 6 + 16 * $images.Count
foreach ($img in $images) {
    $s = $img.Size
    $bytes = $img.Data
    $bw.Write([byte]($s -band 0xFF)); $bw.Write([byte]($s -band 0xFF))
    $bw.Write([byte]0); $bw.Write([byte]0)
    $bw.Write([int16]1); $bw.Write([int16]32)
    $bw.Write([int]$bytes.Length); $bw.Write([int]$offset)
    $offset += $bytes.Length
}
foreach ($img in $images) {
    $bw.Write($img.Data)
}
$bw.Flush()
[System.IO.File]::WriteAllBytes($icoPath, $ms.ToArray())
$bw.Dispose(); $ms.Dispose()

# PNG 256 para el README.
$png = New-IconBitmap 256
$png.Save($pngPath, [System.Drawing.Imaging.ImageFormat]::Png)
$png.Dispose()

Write-Host "OK: $icoPath ($((Get-Item $icoPath).Length) bytes) y $pngPath"
