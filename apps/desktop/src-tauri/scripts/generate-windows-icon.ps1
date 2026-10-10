param(
  [string]$Source = (Join-Path $PSScriptRoot '..\icons\icon.png'),
  [string]$Destination = (Join-Path $PSScriptRoot '..\icons\generated\icon.ico')
)

$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest
Add-Type -AssemblyName System.Drawing

$sourceImage = [System.Drawing.Image]::FromFile((Resolve-Path $Source))
$resized = [System.Drawing.Bitmap]::new(
  256,
  256,
  [System.Drawing.Imaging.PixelFormat]::Format32bppArgb
)
$graphics = [System.Drawing.Graphics]::FromImage($resized)
$pngStream = [System.IO.MemoryStream]::new()

try {
  $graphics.Clear([System.Drawing.Color]::Transparent)
  $graphics.CompositingQuality = [System.Drawing.Drawing2D.CompositingQuality]::HighQuality
  $graphics.InterpolationMode = [System.Drawing.Drawing2D.InterpolationMode]::HighQualityBicubic
  $graphics.PixelOffsetMode = [System.Drawing.Drawing2D.PixelOffsetMode]::HighQuality
  $graphics.SmoothingMode = [System.Drawing.Drawing2D.SmoothingMode]::HighQuality
  $graphics.DrawImage($sourceImage, 0, 0, 256, 256)
  $resized.Save($pngStream, [System.Drawing.Imaging.ImageFormat]::Png)
  $png = $pngStream.ToArray()

  $parent = Split-Path -Parent $Destination
  New-Item -ItemType Directory -Force -Path $parent | Out-Null
  $file = [System.IO.File]::Open(
    $Destination,
    [System.IO.FileMode]::Create,
    [System.IO.FileAccess]::Write,
    [System.IO.FileShare]::None
  )
  $writer = [System.IO.BinaryWriter]::new($file)
  try {
    # ICONDIR: reserved, image type, image count.
    $writer.Write([UInt16]0)
    $writer.Write([UInt16]1)
    $writer.Write([UInt16]1)

    # ICONDIRENTRY. A zero width/height byte represents 256 pixels.
    $writer.Write([Byte]0)
    $writer.Write([Byte]0)
    $writer.Write([Byte]0)
    $writer.Write([Byte]0)
    $writer.Write([UInt16]1)
    $writer.Write([UInt16]32)
    $writer.Write([UInt32]$png.Length)
    $writer.Write([UInt32]22)
    $writer.Write([Byte[]]$png)
  } finally {
    $writer.Dispose()
  }
} finally {
  $pngStream.Dispose()
  $graphics.Dispose()
  $resized.Dispose()
  $sourceImage.Dispose()
}
