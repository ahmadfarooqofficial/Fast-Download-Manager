$toolsDir = Join-Path $env:LOCALAPPDATA 'FDM\tools'
$ytdlp = Join-Path $toolsDir 'yt-dlp.exe'
$deno = Join-Path $toolsDir 'deno.exe'

$args = @(
    '--js-runtimes', "deno:$deno",
    '-g',
    '--get-filename',
    '-o', '%(title)s.%(ext)s',
    '-f', 'b/best',
    'https://www.youtube.com/watch?v=dQw4w9WgXcQ'
)

& $ytdlp $args
