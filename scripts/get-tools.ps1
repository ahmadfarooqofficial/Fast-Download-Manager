$ErrorActionPreference = 'Stop'

$toolsDir = Join-Path $env:LOCALAPPDATA 'FDM\tools'
$stagingToolsDir = Join-Path $PSScriptRoot '..\target\installer-staging\tools'
$releaseToolsDir = Join-Path $PSScriptRoot '..\target\release\tools'

New-Item -ItemType Directory -Force -Path $toolsDir | Out-Null
New-Item -ItemType Directory -Force -Path $stagingToolsDir | Out-Null
New-Item -ItemType Directory -Force -Path $releaseToolsDir | Out-Null

[Net.ServicePointManager]::SecurityProtocol = [Net.SecurityProtocolType]::Tls12

# 1. yt-dlp.exe
$ytdlpPath = Join-Path $toolsDir 'yt-dlp.exe'
if (-not (Test-Path $ytdlpPath)) {
    Write-Host "Downloading yt-dlp.exe..."
    curl.exe -L "https://github.com/yt-dlp/yt-dlp/releases/latest/download/yt-dlp.exe" -o "$ytdlpPath"
}
Copy-Item $ytdlpPath (Join-Path $stagingToolsDir 'yt-dlp.exe') -Force
Copy-Item $ytdlpPath (Join-Path $releaseToolsDir 'yt-dlp.exe') -Force

# 2. Deno (JavaScript runtime required by YouTube extractor)
$denoPath = Join-Path $toolsDir 'deno.exe'
if (-not (Test-Path $denoPath)) {
    Write-Host "Downloading deno.exe..."
    $denoZip = Join-Path $env:TEMP 'deno.zip'
    curl.exe -L "https://github.com/denoland/deno/releases/latest/download/deno-x86_64-pc-windows-msvc.zip" -o "$denoZip"
    Expand-Archive -Path $denoZip -DestinationPath $toolsDir -Force
    Remove-Item $denoZip -Force -ErrorAction SilentlyContinue
}

# 3. FFmpeg (Audio/Video muxing engine)
$ffmpegPath = Join-Path $toolsDir 'ffmpeg.exe'
if (-not (Test-Path $ffmpegPath)) {
    Write-Host "Downloading ffmpeg.exe..."
    $ffZip = Join-Path $env:TEMP 'ffmpeg-release.zip'
    $ffExtract = Join-Path $env:TEMP 'ffmpeg-unzip'
    curl.exe -L "https://github.com/yt-dlp/FFmpeg-Builds/releases/download/latest/ffmpeg-master-latest-win64-gpl.zip" -o "$ffZip"
    Expand-Archive -Path $ffZip -DestinationPath $ffExtract -Force
    $ffFound = Get-ChildItem -Path $ffExtract -Filter 'ffmpeg.exe' -Recurse | Select-Object -First 1
    if ($ffFound) {
        Copy-Item $ffFound.FullName $ffmpegPath -Force
    }
    Remove-Item $ffZip -Force -ErrorAction SilentlyContinue
    Remove-Item $ffExtract -Recurse -Force -ErrorAction SilentlyContinue
}
if (Test-Path $ffmpegPath) {
    Copy-Item $ffmpegPath (Join-Path $stagingToolsDir 'ffmpeg.exe') -Force
    Copy-Item $ffmpegPath (Join-Path $releaseToolsDir 'ffmpeg.exe') -Force
}

Write-Host "Tools verified successfully in $toolsDir, $stagingToolsDir, and $releaseToolsDir"
