# Pack extension into CRX using Chrome if available.
#
# A CRX is signed, and its extension ID is derived from the signing key. The
# installer registers the extension under one fixed ID (see ChromeExtensionId in
# installer\fdm.iss), so the same key has to be used every time: if
# extension.pem exists beside the extension folder it is used, otherwise Chrome
# generates a fresh key and writes it there for the next build to pick up.
# The key is deliberately gitignored (*.pem); on CI it arrives via a secret.
$chromeCandidates = @(
    "$env:ProgramFiles\Google\Chrome\Application\chrome.exe",
    "${env:ProgramFiles(x86)}\Google\Chrome\Application\chrome.exe",
    "$env:LOCALAPPDATA\Google\Chrome\Application\chrome.exe"
)

$chrome = $chromeCandidates | Where-Object { Test-Path $_ } | Select-Object -First 1

if ($chrome) {
    Write-Host "Found Chrome at $chrome"
    $extDir = (Resolve-Path "$PSScriptRoot\..\extension").Path
    $keyFile = Join-Path (Split-Path -Parent $extDir) 'extension.pem'
    if (Test-Path $keyFile) {
        Write-Host "Signing with the existing key at $keyFile"
        & $chrome --pack-extension="$extDir" --pack-extension-key="$keyFile" --no-message-box
    } else {
        Write-Host "No extension.pem found; Chrome will generate a new key (and a new extension ID)."
        & $chrome --pack-extension="$extDir" --no-message-box
    }
    Write-Host "Chrome packaging command finished."
} else {
    Write-Host "Chrome not found at standard paths."
}
