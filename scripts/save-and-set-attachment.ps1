param(
    [string]$FilePath
)

# Deprecated. Use the Rust build. Writes only a .png under a \temp\ folder.

if (-not $FilePath) {
    exit 1
}

try {
    $full = [System.IO.Path]::GetFullPath($FilePath)
} catch {
    exit 1
}

if ($full -match '\.\.') { exit 1 }
if ($full -notmatch '(?i)\\temp\\[^\\/]+\.png$') { exit 1 }

$dir = Split-Path -Parent $full
if (-not $dir) {
    exit 1
}

try {
    if (-not (Test-Path -LiteralPath $dir)) {
        New-Item -ItemType Directory -Force -Path $dir | Out-Null
    }
} catch {
    exit 1
}

Add-Type -AssemblyName System.Windows.Forms
Add-Type -AssemblyName System.Drawing

if (-not [System.Windows.Forms.Clipboard]::ContainsImage()) {
    exit 1
}

$image = $null
$pngBuffer = $null
$pngStream = $null
try {
    $image = [System.Windows.Forms.Clipboard]::GetImage()
    if ($null -eq $image) {
        exit 1
    }

    $pngBuffer = New-Object System.IO.MemoryStream
    $image.Save($pngBuffer, [System.Drawing.Imaging.ImageFormat]::Png)
    $bytes = $pngBuffer.ToArray()
    [System.IO.File]::WriteAllBytes($full, $bytes)

    $pngStream = New-Object System.IO.MemoryStream(,$bytes)
    $data = New-Object System.Windows.Forms.DataObject

    $data.SetData([System.Windows.Forms.DataFormats]::Bitmap, $true, $image)
    $data.SetData("PNG", $true, $pngStream)

    $files = New-Object System.Collections.Specialized.StringCollection
    [void]$files.Add($full)
    $data.SetFileDropList($files)

    [System.Windows.Forms.Clipboard]::SetDataObject($data, $true)
    exit 0
} catch {
    exit 1
} finally {
    if ($pngStream -ne $null) {
        $pngStream.Dispose()
    }
    if ($pngBuffer -ne $null) {
        $pngBuffer.Dispose()
    }
    if ($image -ne $null) {
        $image.Dispose()
    }
}
