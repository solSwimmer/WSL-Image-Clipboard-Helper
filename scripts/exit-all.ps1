param(
    [string]$TempDir,
    [switch]$Verbose
)

# Deprecated. Use the Rust build.
# Only kills processes whose executable lives under this install tree,
# and only deletes *.png files under a directory named "temp".

$scriptDir = Split-Path -Parent $MyInvocation.MyCommand.Path
$installRoot = [System.IO.Path]::GetFullPath((Split-Path -Parent $scriptDir))

if (-not $TempDir) {
    $TempDir = Join-Path $installRoot 'temp'
}

try {
    $TempDir = [System.IO.Path]::GetFullPath($TempDir)
} catch {
    Write-Error "Invalid TempDir"
    exit 1
}

$tempPrefix = $installRoot.TrimEnd('\') + '\'
if (-not ($TempDir.StartsWith($tempPrefix, [System.StringComparison]::OrdinalIgnoreCase)) -and
    -not ($TempDir.Equals($installRoot, [System.StringComparison]::OrdinalIgnoreCase))) {
    Write-Error "TempDir is outside the install directory; refusing to delete."
    exit 1
}

if ([System.IO.Path]::GetFileName($TempDir) -ne 'temp') {
    Write-Error "TempDir must be a folder named 'temp'"
    exit 1
}

$allProcs = Get-CimInstance Win32_Process -ErrorAction SilentlyContinue

function Under-Install([string]$exePath) {
    if (-not $exePath) { return $false }
    try {
        $full = [System.IO.Path]::GetFullPath($exePath)
    } catch {
        return $false
    }
    return $full.StartsWith($tempPrefix, [System.StringComparison]::OrdinalIgnoreCase) -or
           $full.Equals($installRoot, [System.StringComparison]::OrdinalIgnoreCase)
}

$ahkProcs = $allProcs | Where-Object {
    $_.ExecutablePath -and (Under-Install $_.ExecutablePath) -and (
        $_.ExecutablePath -like '*AutoHotkey*.exe*' -or
        $_.CommandLine -like '*wsl_clipboard.ahk*'
    )
}

foreach ($proc in $ahkProcs) {
    if ($Verbose) { Write-Host "Stopping AHK PID=$($proc.ProcessId)" }
    Stop-Process -Id $proc.ProcessId -Force -ErrorAction SilentlyContinue
}

$helperExeProcs = $allProcs | Where-Object {
    $_.Name -and $_.Name -like 'wsl_clipboard*.exe' -and (Under-Install $_.ExecutablePath)
}

foreach ($proc in $helperExeProcs) {
    if ($Verbose) { Write-Host "Stopping helper PID=$($proc.ProcessId)" }
    Stop-Process -Id $proc.ProcessId -Force -ErrorAction SilentlyContinue
}

$psProcs = $allProcs | Where-Object {
    $_.CommandLine -and (
        $_.CommandLine -like '*save-clipboard-image.ps1*' -or
        $_.CommandLine -like '*save-and-set-attachment.ps1*' -or
        $_.CommandLine -like '*set-clipboard-attachment.ps1*'
    ) -and (
        (Under-Install $_.ExecutablePath) -or ($_.CommandLine -like "*$scriptDir*")
    )
}

foreach ($proc in $psProcs) {
    if ($Verbose) { Write-Host "Stopping PowerShell helper PID=$($proc.ProcessId)" }
    Stop-Process -Id $proc.ProcessId -Force -ErrorAction SilentlyContinue
}

if (Test-Path -LiteralPath $TempDir) {
    Get-ChildItem -LiteralPath $TempDir -Filter '*.png' -File -ErrorAction SilentlyContinue |
        ForEach-Object { Remove-Item -LiteralPath $_.FullName -Force -ErrorAction SilentlyContinue }
}

exit 0
