# Example rDownloader post-processing script (Windows).
#
# Copy this file into your scripts directory (Settings -> Post-processing, by default
# `scripts\` next to the database), then select it as the script of a package or category.
# PowerShell scripts are run as: powershell -NoProfile -ExecutionPolicy Bypass -File <script>
#
# Positional arguments (SABnzbd-compatible):
#   1 final directory   2 package name   3 clean package name   4 (empty)
#   5 category          6 (empty)        7 status: 0 ok, 1 download, 2 unpack, 3 par2
param(
    [string]$FinalDir = $env:RD_FINAL_DIR,
    [string]$PackageName = $env:RD_PACKAGE_NAME,
    [string]$CleanName = $env:RD_CLEAN_NAME,
    [string]$Unused4,
    [string]$Category = $env:RD_CATEGORY,
    [string]$Unused6,
    [string]$Status = $env:RD_STATUS
)

$ErrorActionPreference = 'Stop'

# The scripts directory is writable and travels with the configuration, so the log lands
# next to the script rather than in the package folder.
$logFile = Join-Path $env:RD_SCRIPT_DIR 'completed.log'
$timestamp = Get-Date -Format s

if ($Status -eq '0') {
    Add-Content -Path $logFile -Value "$timestamp OK $PackageName [$Category] -> $FinalDir"
    Write-Output "logged completion of $CleanName"
} else {
    Add-Content -Path $logFile -Value "$timestamp FAILED($Status) $PackageName [$Category]"
    Write-Output "package $PackageName finished with status $Status"
}
