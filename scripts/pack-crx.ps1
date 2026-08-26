# Pack extension into CRX using Chrome if available
$chromeCandidates = @(
    "$env:ProgramFiles\Google\Chrome\Application\chrome.exe",
    "${env:ProgramFiles(x86)}\Google\Chrome\Application\chrome.exe",
    "$env:LOCALAPPDATA\Google\Chrome\Application\chrome.exe"
)

$chrome = $chromeCandidates | Where-Object { Test-Path $_ } | Select-Object -First 1

if ($chrome) {
    Write-Host "Found Chrome at $chrome"
    $extDir = (Resolve-Path "$PSScriptRoot\..\extension").Path
    $destCrx = (Resolve-Path "$PSScriptRoot\..\extension.crx" -ErrorAction SilentlyContinue)
    & $chrome --pack-extension="$extDir" --no-message-box
    Write-Host "Chrome packaging command finished."
} else {
    Write-Host "Chrome not found at standard paths."
}
