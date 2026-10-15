# Generates RustWinUtil's icon: a walnut shield carrying a gilt wrench, on a
# transparent ground, drawn at every size Windows asks for.
#
#   powershell -ExecutionPolicy Bypass -File tools\make-icon.ps1
#
# Writes two files into assets\:
#   rustwinutil.ico   multi-size icon, embedded into the exe by build.rs (MSVC builds)
#   icon-64.rgba      raw 64x64 RGBA pixels, set as the window/taskbar icon at runtime

Add-Type -AssemblyName System.Drawing

$root   = Split-Path -Parent (Split-Path -Parent $MyInvocation.MyCommand.Path)
$assets = Join-Path $root 'assets'
New-Item -ItemType Directory -Force $assets | Out-Null
$outIco  = Join-Path $assets 'rustwinutil.ico'
$outRgba = Join-Path $assets 'icon-64.rgba'
$sizes   = @(16, 20, 24, 32, 40, 48, 64, 128, 256)

$walnut     = [System.Drawing.Color]::FromArgb(255, 107, 79, 53)
$walnutDeep = [System.Drawing.Color]::FromArgb(255, 74, 53, 36)
$cream      = [System.Drawing.Color]::FromArgb(255, 244, 238, 226)
$gilt       = [System.Drawing.Color]::FromArgb(255, 199, 154, 74)

function P([float]$x, [float]$y) { New-Object System.Drawing.PointF($x, $y) }

function New-IconBitmap([int]$size) {
    $bmp = New-Object System.Drawing.Bitmap($size, $size, [System.Drawing.Imaging.PixelFormat]::Format32bppArgb)
    $g   = [System.Drawing.Graphics]::FromImage($bmp)
    $g.SmoothingMode = [System.Drawing.Drawing2D.SmoothingMode]::AntiAlias

    # Authored on a 48x48 grid and scaled to the target.
    $k = $size / 48.0
    $g.ScaleTransform($k, $k)

    # Shield: flat top, straight sides, a curved point at the bottom.
    $shield = New-Object 'System.Drawing.Drawing2D.GraphicsPath'
    $shield.AddLine((P 6 6), (P 42 6))
    $shield.AddLine((P 42 6), (P 42 22))
    $shield.AddBezier((P 42 22), (P 42 34), (P 34 41), (P 24 45.5))
    $shield.AddBezier((P 24 45.5), (P 14 41), (P 6 34), (P 6 22))
    $shield.CloseFigure()
    $g.FillPath((New-Object System.Drawing.SolidBrush($walnutDeep)), $shield)

    # Inner face, inset, in the lighter walnut.
    $face = New-Object 'System.Drawing.Drawing2D.GraphicsPath'
    $face.AddLine((P 9.5 9.5), (P 38.5 9.5))
    $face.AddLine((P 38.5 9.5), (P 38.5 22))
    $face.AddBezier((P 38.5 22), (P 38.5 32), (P 32 38), (P 24 41.6))
    $face.AddBezier((P 24 41.6), (P 16 38), (P 9.5 32), (P 9.5 22))
    $face.CloseFigure()
    $g.FillPath((New-Object System.Drawing.SolidBrush($walnut)), $face)

    # Wrench, corner to corner: a thick handle with a round open-jawed head.
    $g.TranslateTransform(24, 24)
    $g.RotateTransform(-45)
    $giltBrush = New-Object System.Drawing.SolidBrush($gilt)
    $g.FillRectangle($giltBrush, -2.6, -2.0, 5.2, 16.5)                # handle
    $g.FillEllipse($giltBrush, -2.6, 12.0, 5.2, 5.2)                   # handle's round end
    $g.FillEllipse($giltBrush, -7.2, -14.0, 14.4, 14.4)                # head
    $g.FillRectangle((New-Object System.Drawing.SolidBrush($walnut)), -2.4, -15.5, 4.8, 8.0)  # open jaw
    $g.ResetTransform()

    # A cream rim along the shield's top edge, like the bands on the launcher's mark.
    $g.ScaleTransform($k, $k)
    $g.FillRectangle((New-Object System.Drawing.SolidBrush($cream)), 6, 6, 36, 1.8)

    $g.Dispose()
    return $bmp
}

function Get-PngBytes($bmp) {
    $ms = New-Object System.IO.MemoryStream
    $bmp.Save($ms, [System.Drawing.Imaging.ImageFormat]::Png)
    [byte[]]$bytes = $ms.ToArray()
    return ,$bytes
}

$images = New-Object System.Collections.ArrayList
foreach ($s in $sizes) {
    $bmp = New-IconBitmap $s
    [void]$images.Add([byte[]](Get-PngBytes $bmp))
    if ($s -eq 64) {
        # GDI+ stores BGRA; the window icon wants RGBA, row-major from the top.
        $rect = New-Object System.Drawing.Rectangle(0, 0, 64, 64)
        $data = $bmp.LockBits($rect, [System.Drawing.Imaging.ImageLockMode]::ReadOnly,
                              [System.Drawing.Imaging.PixelFormat]::Format32bppArgb)
        $raw = New-Object byte[] (64 * 64 * 4)
        [System.Runtime.InteropServices.Marshal]::Copy($data.Scan0, $raw, 0, $raw.Length)
        $bmp.UnlockBits($data)
        for ($i = 0; $i -lt $raw.Length; $i += 4) {
            $b = $raw[$i]; $raw[$i] = $raw[$i + 2]; $raw[$i + 2] = $b
        }
        [System.IO.File]::WriteAllBytes($outRgba, $raw)
    }
    $bmp.Dispose()
}

# ICO container: 6-byte header, then one 16-byte directory entry per image.
$fs = [System.IO.File]::Create($outIco)
$bw = New-Object System.IO.BinaryWriter($fs)
$bw.Write([UInt16]0); $bw.Write([UInt16]1); $bw.Write([UInt16]$sizes.Count)
$offset = 6 + (16 * $sizes.Count)
for ($i = 0; $i -lt $sizes.Count; $i++) {
    $s = $sizes[$i]
    $dim = 0
    if ($s -lt 256) { $dim = $s }    # 0 means 256 in the ICO directory
    $bw.Write([Byte]$dim); $bw.Write([Byte]$dim); $bw.Write([Byte]0); $bw.Write([Byte]0)
    $bw.Write([UInt16]1); $bw.Write([UInt16]32)
    $bw.Write([UInt32]$images[$i].Length); $bw.Write([UInt32]$offset)
    $offset += $images[$i].Length
}
foreach ($img in $images) { $bw.Write([byte[]]$img, 0, $img.Length) }
$bw.Flush(); $bw.Close(); $fs.Close()
Write-Output "Wrote $outIco and $outRgba"
