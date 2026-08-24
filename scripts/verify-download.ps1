$ErrorActionPreference = 'Stop'

Write-Host "=== 1. Testing yt-dlp stream extraction ===" -ForegroundColor Cyan
$toolsDir = Join-Path $env:LOCALAPPDATA 'FDM\tools'
$ytdlp = Join-Path $toolsDir 'yt-dlp.exe'
$deno = Join-Path $toolsDir 'deno.exe'

$args = @(
    '--no-playlist',
    '--no-warnings',
    '--js-runtimes', "deno:$deno",
    '-g',
    '--get-filename',
    '-o', '%(title)s.%(ext)s',
    '-f', 'b/best',
    'https://www.youtube.com/watch?v=dQw4w9WgXcQ'
)

$output = & $ytdlp $args
if ($output.Count -lt 2) {
    Write-Error "yt-dlp failed to extract stream and filename. Output: $output"
}

$directUrl = $output[0]
$filename = $output[1]
Write-Host "Extracted Title: $filename" -ForegroundColor Green
Write-Host "Direct Stream URL: $($directUrl.Substring(0, [math]::Min(80, $directUrl.Length)))..." -ForegroundColor Green

Write-Host "`n=== 2. Testing FDM Core Engine Download ===" -ForegroundColor Cyan
$testOutDir = Join-Path $env:TEMP 'fdm_test_downloads'
New-Item -ItemType Directory -Force -Path $testOutDir | Out-Null
$testFile = Join-Path $testOutDir 'test_video.mp4'
if (Test-Path $testFile) { Remove-Item $testFile -Force }

$fdmCli = 'D:\Code\FDM\target\release\fdm.exe'
Write-Host "Running: $fdmCli download with 32 connections..." -ForegroundColor Gray

# Download the video stream with fdm get
& $fdmCli get --out $testFile $directUrl

if (Test-Path $testFile) {
    $size = (Get-Item $testFile).Length
    Write-Host "Downloaded file size: $size bytes ($([math]::Round($size / 1MB, 2)) MB)" -ForegroundColor Green
    if ($size -gt 0) {
        Write-Host "SUCCESS: FDM downloaded real video data!" -ForegroundColor Green
    } else {
        Write-Error "FAILURE: Downloaded file is 0 bytes."
    }
} else {
    Write-Error "FAILURE: Output file was not created."
}

Write-Host "`n=== 3. Verification Complete ===" -ForegroundColor Green
