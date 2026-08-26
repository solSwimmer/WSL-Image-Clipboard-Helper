param(
    [string]$FilePath
)

# Deprecated. Use the Rust build. Only reads an existing .png under a \temp\ folder.

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

if (-not (Test-Path -LiteralPath $full)) {
    exit 1
}

Add-Type -AssemblyName System.Windows.Forms
Add-Type -AssemblyName System.Drawing

$img = $null
$bytes = $null
$pngStream = $null
try {
    $img = [System.Drawing.Image]::FromFile($full)
    $bytes = [System.IO.File]::ReadAllBytes($full)
    $pngStream = New-Object System.IO.MemoryStream(,$bytes)
} catch {
    exit 1
}

try {
    $data = New-Object System.Windows.Forms.DataObject

    $data.SetData([System.Windows.Forms.DataFormats]::Bitmap, $true, $img)
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
    if ($img -ne $null) {
        $img.Dispose()
    }
}
