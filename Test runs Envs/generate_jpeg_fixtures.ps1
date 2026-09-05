param(
    [string]$OutputDirectory = (Join-Path $PSScriptRoot "Materials")
)

Add-Type -AssemblyName System.Drawing
New-Item -ItemType Directory -Force -Path $OutputDirectory | Out-Null

function Save-Jpeg {
    param(
        [System.Drawing.Bitmap]$Bitmap,
        [string]$Name,
        [long]$Quality
    )

    $codec = [System.Drawing.Imaging.ImageCodecInfo]::GetImageEncoders() |
        Where-Object MimeType -eq "image/jpeg"
    $parameters = [System.Drawing.Imaging.EncoderParameters]::new(1)
    $parameters.Param[0] = [System.Drawing.Imaging.EncoderParameter]::new(
        [System.Drawing.Imaging.Encoder]::Quality,
        $Quality
    )
    try {
        $Bitmap.Save((Join-Path $OutputDirectory $Name), $codec, $parameters)
    }
    finally {
        $parameters.Dispose()
        $Bitmap.Dispose()
    }
}

$photo = [System.Drawing.Bitmap]::new(640, 360)
$graphics = [System.Drawing.Graphics]::FromImage($photo)
$graphics.SmoothingMode = [System.Drawing.Drawing2D.SmoothingMode]::AntiAlias
$sky = [System.Drawing.Drawing2D.LinearGradientBrush]::new(
    [System.Drawing.Rectangle]::new(0, 0, 640, 240),
    [System.Drawing.Color]::FromArgb(255, 50, 135, 205),
    [System.Drawing.Color]::FromArgb(255, 238, 189, 106),
    90.0
)
$graphics.FillRectangle($sky, 0, 0, 640, 240)
$graphics.FillEllipse([System.Drawing.Brushes]::LemonChiffon, 485, 42, 72, 72)
$farHills = [System.Drawing.Point[]]@(
    [System.Drawing.Point]::new(0, 230), [System.Drawing.Point]::new(100, 140),
    [System.Drawing.Point]::new(205, 220), [System.Drawing.Point]::new(325, 115),
    [System.Drawing.Point]::new(470, 225), [System.Drawing.Point]::new(570, 155),
    [System.Drawing.Point]::new(640, 210), [System.Drawing.Point]::new(640, 360),
    [System.Drawing.Point]::new(0, 360)
)
$graphics.FillPolygon([System.Drawing.Brushes]::DarkSlateGray, $farHills)
$nearHills = [System.Drawing.Point[]]@(
    [System.Drawing.Point]::new(0, 285), [System.Drawing.Point]::new(145, 205),
    [System.Drawing.Point]::new(290, 295), [System.Drawing.Point]::new(430, 190),
    [System.Drawing.Point]::new(640, 275), [System.Drawing.Point]::new(640, 360),
    [System.Drawing.Point]::new(0, 360)
)
$graphics.FillPolygon([System.Drawing.Brushes]::ForestGreen, $nearHills)
$graphics.FillRectangle([System.Drawing.Brushes]::MidnightBlue, 0, 292, 640, 68)
for ($index = 0; $index -lt 28; $index++) {
    $x = 8 + (($index * 83) % 620)
    $height = 16 + (($index * 29) % 58)
    $graphics.FillRectangle([System.Drawing.Brushes]::Black, $x, 292 - $height, 4, $height)
}
$sky.Dispose()
$graphics.Dispose()
Save-Jpeg $photo "JpegPhotoScene.jpg" 88

$dark = [System.Drawing.Bitmap]::new(640, 360)
for ($y = 0; $y -lt $dark.Height; $y++) {
    for ($x = 0; $x -lt $dark.Width; $x++) {
        $distance = [Math]::Sqrt([Math]::Pow($x - 390, 2) + [Math]::Pow($y - 155, 2))
        $glow = [Math]::Max(0, 46 - [int]($distance / 7))
        $red = [Math]::Min(255, 2 + $glow)
        $green = [Math]::Min(255, 3 + [int]($glow * 0.72))
        $blue = [Math]::Min(255, 5 + [int]($glow * 0.42))
        $dark.SetPixel($x, $y, [System.Drawing.Color]::FromArgb($red, $green, $blue))
    }
}
$graphics = [System.Drawing.Graphics]::FromImage($dark)
$graphics.FillRectangle([System.Drawing.Brushes]::Black, 0, 285, 640, 75)
$dimPen = [System.Drawing.Pen]::new([System.Drawing.Color]::FromArgb(36, 55, 68), 2)
$graphics.DrawRectangle($dimPen, 70, 95, 175, 150)
$graphics.DrawLine($dimPen, 70, 170, 245, 170)
$pointBrush = [System.Drawing.SolidBrush]::new([System.Drawing.Color]::FromArgb(75, 92, 105))
$graphics.FillEllipse($pointBrush, 482, 82, 5, 5)
$pointBrush.Dispose()
$dimPen.Dispose()
$graphics.Dispose()
Save-Jpeg $dark "JpegDarkDetails.jpg" 92

$gradient = [System.Drawing.Bitmap]::new(768, 256)
for ($y = 0; $y -lt $gradient.Height; $y++) {
    for ($x = 0; $x -lt $gradient.Width; $x++) {
        $red = [int](255 * $x / ($gradient.Width - 1))
        $green = [int](255 * $y / ($gradient.Height - 1))
        $blue = [int](255 * (1 - [Math]::Abs(($x / ($gradient.Width - 1)) - 0.5) * 2))
        $gradient.SetPixel($x, $y, [System.Drawing.Color]::FromArgb($red, $green, $blue))
    }
}
Save-Jpeg $gradient "JpegSmoothGradient.jpg" 96

$artifacts = [System.Drawing.Bitmap]::new(640, 360)
$graphics = [System.Drawing.Graphics]::FromImage($artifacts)
$graphics.Clear([System.Drawing.Color]::White)
$redBrush = [System.Drawing.SolidBrush]::new([System.Drawing.Color]::FromArgb(225, 20, 35))
$cyanBrush = [System.Drawing.SolidBrush]::new([System.Drawing.Color]::FromArgb(10, 175, 215))
for ($y = 0; $y -lt 360; $y += 16) {
    for ($x = 0; $x -lt 640; $x += 16) {
        $brush = if ((($x + $y) / 16) % 2 -eq 0) {
            $redBrush
        } else {
            $cyanBrush
        }
        $graphics.FillRectangle($brush, $x, $y, 16, 16)
    }
}
$graphics.FillRectangle([System.Drawing.Brushes]::Black, 70, 110, 500, 140)
$font = [System.Drawing.Font]::new("Consolas", 48, [System.Drawing.FontStyle]::Bold)
$graphics.DrawString("JPEG 8x8 STRESS", $font, [System.Drawing.Brushes]::White, 88, 142)
$font.Dispose()
$redBrush.Dispose()
$cyanBrush.Dispose()
$graphics.Dispose()
Save-Jpeg $artifacts "JpegCompressionArtifacts.jpg" 8

Write-Host "Generated JPEG fixtures in $OutputDirectory"