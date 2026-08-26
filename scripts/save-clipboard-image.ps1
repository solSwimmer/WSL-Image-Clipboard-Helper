param(
    [string]$FilePath
)

# Deprecated. Use the Rust build. This script will only write a .png under a \temp\ folder.

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
if (-not (Test-Path -LiteralPath $dir)) {
    New-Item -ItemType Directory -Force -Path $dir | Out-Null
}

Add-Type -AssemblyName System.Windows.Forms
Add-Type -AssemblyName System.Drawing

if ([System.Windows.Forms.Clipboard]::ContainsImage()) {
    try {
        $image = [System.Windows.Forms.Clipboard]::GetImage()
        $image.Save($full, [System.Drawing.Imaging.ImageFormat]::Png)
        $image.Dispose()
    } catch {
        # Save failed, silently ignore
    }
}
