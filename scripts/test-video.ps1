$toolsDir = Join-Path $env:LOCALAPPDATA 'FDM\tools'
$ytdlp = Join-Path $toolsDir 'yt-dlp.exe'
$deno = Join-Path $toolsDir 'deno.exe'
$url = 'https://www.youtube.com/watch?v=WK8TRdY1Dis'

$dest = Join-Path $env:USERPROFILE "Downloads\FDM\Video\test_p.%(ext)s"
& $ytdlp --newline --progress-template "download:FDM_PROG:%(progress.downloaded_bytes)s:%(progress.total_bytes)s:%(progress._speed_str)s:%(progress._eta_str)s" --no-playlist --no-warnings --js-runtimes "deno:$deno" --ffmpeg-location "$toolsDir" -N 16 -f "bestvideo[height<=720][ext=mp4]+bestaudio[ext=m4a]/bestvideo+bestaudio/best" -o $dest $url
