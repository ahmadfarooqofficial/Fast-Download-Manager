$toolsDir = Join-Path $env:LOCALAPPDATA 'FDM\tools'
$ytdlp = Join-Path $toolsDir 'yt-dlp.exe'
$deno = Join-Path $toolsDir 'deno.exe'
$url = 'https://www.youtube.com/watch?v=WK8TRdY1Dis'

Write-Host "Testing height<=1080 resolution..."
& $ytdlp --no-playlist --no-warnings --js-runtimes "deno:$deno" -g --get-filename -o "%(title)s.%(ext)s" -f "bestvideo[height<=1080][ext=mp4]/bestvideo[height<=1080]/best[height<=1080]/bestvideo[ext=mp4]/bestvideo/best" $url
