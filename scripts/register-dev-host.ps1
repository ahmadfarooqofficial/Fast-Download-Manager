<#
.SYNOPSIS
    Registers the FDM Native Messaging Host for local development and testing.

.DESCRIPTION
    Creates the JSON host manifest pointing to target\release\fdm-host.exe and writes
    the HKCU registry keys for Chrome, Edge, and Brave so unpacked extensions can communicate
    with the host without requiring full administrator installation.

.PARAMETER ExtensionId
    Optional Chrome extension ID to allow. Defaults to the development key's ID.
#>

[CmdletBinding()]
param(
    [string]$ExtensionId = "enipidhffjdkkkmmohnnehfdigdnmfeo",
    [string]$HostPath
)

$ErrorActionPreference = 'Stop'

$RepoRoot = Split-Path -Parent $PSScriptRoot
if (-not $HostPath) {
    $HostPath = Join-Path $RepoRoot "target\release\fdm-host.exe"
}

if (-not (Test-Path -LiteralPath $HostPath)) {
    Write-Host "Host binary not found at: $HostPath" -ForegroundColor Yellow
    Write-Host "Building release binary..." -ForegroundColor Gray
    cargo build --release -p fdm-host
}

$ManifestDir = Join-Path $env:LOCALAPPDATA "FDM\manifests"
New-Item -ItemType Directory -Path $ManifestDir -Force | Out-Null
$ManifestFile = Join-Path $ManifestDir "com.fdm.native_host.json"

$allowedOrigins = @(
    "chrome-extension://$ExtensionId/"
)
if ($ExtensionId -ne "enipidhffjdkkkmmohnnehfdigdnmfeo") {
    $allowedOrigins += "chrome-extension://enipidhffjdkkkmmohnnehfdigdnmfeo/"
}

$manifest = @{
    name = "com.fdm.native_host"
    description = "FDM Download Host"
    path = (Resolve-Path $HostPath).Path
    type = "stdio"
    allowed_origins = $allowedOrigins
}

$json = $manifest | ConvertTo-Json -Depth 4
[System.IO.File]::WriteAllText($ManifestFile, $json)
Write-Host "Created manifest: $ManifestFile" -ForegroundColor Green

$regKeys = @(
    "HKCU:\Software\Google\Chrome\NativeMessagingHosts\com.fdm.native_host",
    "HKCU:\Software\Microsoft\Edge\NativeMessagingHosts\com.fdm.native_host",
    "HKCU:\Software\BraveSoftware\Brave-Browser\NativeMessagingHosts\com.fdm.native_host"
)

foreach ($key in $regKeys) {
    $parent = Split-Path -Parent $key
    if (-not (Test-Path $parent)) {
        New-Item -Path $parent -Force | Out-Null
    }
    New-Item -Path $key -Force | Out-Null
    Set-ItemProperty -Path $key -Name "(Default)" -Value $ManifestFile
    Write-Host "Registered: $key -> $ManifestFile" -ForegroundColor Green
}

Write-Host "`nFDM Native Messaging Host successfully registered for development!" -ForegroundColor Cyan
