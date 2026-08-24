$ErrorActionPreference = 'Stop'

$toolsDir = Join-Path $env:LOCALAPPDATA 'FDM\tools'
$stagingToolsDir = Join-Path $PSScriptRoot '..\target\installer-staging\tools'

New-Item -ItemType Directory -Force -Path $toolsDir | Out-Null
New-Item -ItemType Directory -Force -Path $stagingToolsDir | Out-Null

[Net.ServicePointManager]::SecurityProtocol = [Net.SecurityProtocolType]::Tls12

# 1. yt-dlp.exe
$ytdlpPath = Join-Path $toolsDir 'yt-dlp.exe'
if (-not (Test-Path $ytdlpPath)) {
    Write-Host "Downloading yt-dlp.exe..."
    Invoke-WebRequest -Uri 'https://github.com/yt-dlp/yt-dlp/releases/latest/download/yt-dlp.exe' -OutFile $ytdlpPath -UseBasicParsing
}
Copy-Item $ytdlpPath (Join-Path $stagingToolsDir 'yt-dlp.exe') -Force

# 2. Deno (JavaScript runtime required by YouTube extractor)
$denoPath = Join-Path $toolsDir 'deno.exe'
if (-not (Test-Path $denoPath)) {
    Write-Host "Downloading deno.exe..."
    $denoZip = Join-Path $env:TEMP 'deno.zip'
    Invoke-WebRequest -Uri 'https://github.com/denoland/deno/releases/latest/download/deno-x86_64-pc-windows-msvc.zip' -OutFile $denoZip -UseBasicParsing
    Expand-Archive -Path $denoZip -DestinationPath $toolsDir -Force
    Remove-Item $denoZip -Force -ErrorAction SilentlyContinue
}
if (Test-Path $denoPath) {
    Copy-Item $denoPath (Join-Path $stagingToolsDir 'deno.exe') -Force
}

Write-Host "Tools verified successfully in $toolsDir and $stagingToolsDir"
