$toolsDir = Join-Path $env:LOCALAPPDATA 'FDM\tools'
$ytdlp = Join-Path $toolsDir 'yt-dlp.exe'
$deno = Join-Path $toolsDir 'deno.exe'
$url = 'https://www.youtube.com/watch?v=WK8TRdY1Dis'

Write-Host "Testing download with combined video + audio..."
$dest = Join-Path $env:USERPROFILE "Downloads\FDM\Video\test_video.mp4"
& $ytdlp -N 16 --no-playlist --no-warnings --js-runtimes "deno:$deno" -f "bestvideo[height<=1080][ext=mp4]+bestaudio[ext=m4a]/bestvideo+bestaudio/best" -o $dest $url
Write-Host "Done! File size:" (Get-Item $dest).Length
