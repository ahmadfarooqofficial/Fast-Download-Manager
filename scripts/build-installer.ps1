<#
.SYNOPSIS
    Builds FDM-Setup-<version>.exe -- the single file an end user downloads.

.DESCRIPTION
    Does everything, in order, and installs its own missing tools:

      1. Ensures Inno Setup and the Rust toolchain are present (winget).
      2. Ensures the brand rasters exist (runs the Node rasteriser if not).
      3. cargo build --release
      4. Stages the payload into target\installer-staging
      5. Compiles installer\fdm.iss into installer\output\FDM-Setup-<ver>.exe

    Refuses to produce a build that is missing the desktop app or the browser
    bridge unless you pass -AllowPartial. A "setup file" that silently installs
    only half the product is worse than a build failure.

.PARAMETER Version
    Version stamped into the installer and the filename. Defaults to the
    workspace version in Cargo.toml.

.PARAMETER ExtensionId
    Chrome Web Store extension ID. Until the extension is published, leave this
    unset -- the installer then skips browser registration rather than writing a
    registry key that makes Chrome log an error on every launch.

.PARAMETER AllowPartial
    Build even though fdm-desktop.exe / fdm-host.exe do not exist yet.
    Intended for Phase 1-2 development only.

.PARAMETER SignThumbprint
    SHA1 thumbprint of a code-signing certificate in the Windows certificate
    store. Signs the payload binaries, the uninstaller and the finished setup, so
    Windows shows the publisher name instead of "Publisher: Unknown".

.PARAMETER SelfSign
    Sign with a local development certificate for "Ahmad Farooq" instead.
    Shows the name on THIS machine only -- see scripts\sign.ps1 for why that is
    worse than useless for distribution.

.PARAMETER SkipBuild
    Reuse whatever is already in target\release.

.EXAMPLE
    powershell -ExecutionPolicy Bypass -File scripts\build-installer.ps1 -AllowPartial

.EXAMPLE
    powershell -ExecutionPolicy Bypass -File scripts\build-installer.ps1 -SignThumbprint A1B2C3...
#>

[CmdletBinding()]
param(
    [string]$Version,
    [string]$ExtensionId,
    [string]$SignThumbprint,
    [switch]$SelfSign,
    [switch]$AllowPartial,
    [switch]$SkipBuild
)

$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest

# --------------------------------------------------------------------- helpers

function Write-Step { param([string]$Message) Write-Host "`n==> $Message" -ForegroundColor Red }
function Write-Info { param([string]$Message) Write-Host "    $Message" -ForegroundColor Gray }
function Write-Ok   { param([string]$Message) Write-Host "    OK  $Message" -ForegroundColor Green }
function Write-Warn { param([string]$Message) Write-Host "    !   $Message" -ForegroundColor Yellow }

function Fail {
    param([string]$Message, [string[]]$Hints = @())
    Write-Host "`nBUILD FAILED: $Message" -ForegroundColor Red
    foreach ($h in $Hints) { Write-Host "  - $h" -ForegroundColor Yellow }
    exit 1
}

# winget's "already installed" path exits 43 (APPINSTALLER_CLI_ERROR_UPDATE_NOT_APPLICABLE)
# and -1978335189 (no applicable upgrade). Neither is an error for our purposes.
function Install-IfMissing {
    param(
        [Parameter(Mandatory)][string]$WingetId,
        [Parameter(Mandatory)][string]$FriendlyName,
        [Parameter(Mandatory)][scriptblock]$Probe
    )

    if (& $Probe) { Write-Ok "$FriendlyName already present"; return }

    Write-Info "$FriendlyName is missing -- installing via winget..."
    if (-not (Get-Command winget -ErrorAction SilentlyContinue)) {
        Fail "$FriendlyName is missing and winget is not available." @(
            'Install App Installer from the Microsoft Store, then re-run this script.'
        )
    }

    winget install --id $WingetId --silent --accept-source-agreements `
        --accept-package-agreements --disable-interactivity 2>&1 | Out-Null
    $code = $LASTEXITCODE

    if (($code -ne 0) -and ($code -ne 43) -and ($code -ne -1978335189)) {
        Fail "winget could not install $FriendlyName (exit $code)."
    }

    if (-not (& $Probe)) {
        Fail "$FriendlyName installed but still cannot be found." @(
            'It may need a new shell for PATH changes to apply. Re-run this script.'
        )
    }
    Write-Ok "$FriendlyName installed"
}

function Find-Iscc {
    $candidates = @(
        "${env:ProgramFiles(x86)}\Inno Setup 6\ISCC.exe",
        "$env:ProgramFiles\Inno Setup 6\ISCC.exe",
        "$env:LOCALAPPDATA\Programs\Inno Setup 6\ISCC.exe"
    )
    foreach ($c in $candidates) { if (Test-Path -LiteralPath $c) { return $c } }
    $onPath = Get-Command ISCC.exe -ErrorAction SilentlyContinue
    if ($onPath) { return $onPath.Source }
    return $null
}

function Find-Cargo {
    $local = Join-Path $env:USERPROFILE '.cargo\bin\cargo.exe'
    if (Test-Path -LiteralPath $local) { return $local }
    $onPath = Get-Command cargo -ErrorAction SilentlyContinue
    if ($onPath) { return $onPath.Source }
    return $null
}

# Newest Windows SDK signtool. Version-sorted, not lexically -- 10.0.9 must not
# beat 10.0.26100.
function Find-SignToolForIscc {
    $root = "${env:ProgramFiles(x86)}\Windows Kits\10\bin"
    if (Test-Path -LiteralPath $root) {
        $hit = Get-ChildItem -LiteralPath $root -Directory -ErrorAction SilentlyContinue |
            Where-Object { $_.Name -match '^10\.\d+\.\d+\.\d+$' } |
            Sort-Object { [version]$_.Name } -Descending |
            ForEach-Object { Join-Path $_.FullName 'x64\signtool.exe' } |
            Where-Object { Test-Path -LiteralPath $_ } |
            Select-Object -First 1
        if ($hit) { return $hit }
    }
    $onPath = Get-Command signtool.exe -ErrorAction SilentlyContinue
    if ($onPath) { return $onPath.Source }
    Fail 'signtool.exe not found. Install the Windows SDK: winget install Microsoft.WindowsSDK.10.0.26100'
}

# ------------------------------------------------------------------------ paths

$RepoRoot   = Split-Path -Parent $PSScriptRoot
$InstallDir = Join-Path $RepoRoot 'installer'
$Staging    = Join-Path $RepoRoot 'target\installer-staging'
$Release    = Join-Path $RepoRoot 'target\release'
$OutputDir  = Join-Path $InstallDir 'output'
$IssFile    = Join-Path $InstallDir 'fdm.iss'

if (-not (Test-Path -LiteralPath $IssFile)) {
    Fail "installer\fdm.iss not found. Run this script from a full checkout."
}

Write-Host ''
Write-Host '  FDM - Fast Download Manager :: installer build' -ForegroundColor White
Write-Host '  ---------------------------------------------' -ForegroundColor DarkGray

# ---------------------------------------------------------------- 0. version

if (-not $Version) {
    $cargoToml = Get-Content (Join-Path $RepoRoot 'Cargo.toml') -Raw
    # [workspace.package] version = "x.y.z"
    $m = [regex]::Match($cargoToml, '(?ms)\[workspace\.package\].*?^\s*version\s*=\s*"([^"]+)"')
    if (-not $m.Success) { Fail 'Could not read version from Cargo.toml. Pass -Version explicitly.' }
    $Version = $m.Groups[1].Value
}
Write-Info "Version: $Version"

# ------------------------------------------------------------- 1. build tools

Write-Step 'Checking build tools'

Install-IfMissing -WingetId 'Rustlang.Rustup' -FriendlyName 'Rust toolchain' -Probe { [bool](Find-Cargo) }
$Cargo = Find-Cargo

# rustc cannot link on Windows without MSVC's link.exe. Cargo cannot even
# `check` without it, because dependency build scripts are native executables.
$msvc = Get-ChildItem "${env:ProgramFiles(x86)}\Microsoft Visual Studio\*\*\VC\Tools\MSVC\*\bin\Hostx64\x64\link.exe" -ErrorAction SilentlyContinue
if (-not $msvc) {
    Write-Info 'MSVC linker missing -- installing Visual Studio Build Tools (this is a large download)...'
    winget install --id Microsoft.VisualStudio.2022.BuildTools --accept-source-agreements `
        --accept-package-agreements --disable-interactivity `
        --override '--wait --quiet --norestart --nocache --add Microsoft.VisualStudio.Workload.VCTools --includeRecommended' 2>&1 | Out-Null
    Write-Ok 'Build Tools install attempted'
} else {
    Write-Ok "MSVC linker present ($($msvc[0].Directory.Parent.Parent.Parent.Name))"
}

Install-IfMissing -WingetId 'JRSoftware.InnoSetup' -FriendlyName 'Inno Setup 6' -Probe { [bool](Find-Iscc) }
$Iscc = Find-Iscc
Write-Info "ISCC: $Iscc"

# ------------------------------------------------------------ 2. brand rasters

Write-Step 'Checking installer artwork'

$requiredAssets = @('fdm.ico', 'wizard-large.bmp', 'wizard-small.bmp') |
    ForEach-Object { Join-Path $InstallDir "assets\$_" }
$missingAssets = $requiredAssets | Where-Object { -not (Test-Path -LiteralPath $_) }

if ($missingAssets) {
    Write-Info 'Artwork missing -- rasterising from brand\logo\*.svg...'
    if (-not (Get-Command node -ErrorAction SilentlyContinue)) {
        Fail 'Installer artwork is missing and Node.js is not installed.' @(
            'winget install OpenJS.NodeJS.LTS',
            'then: cd scripts; npm install; npm run rasterize'
        )
    }
    Push-Location (Join-Path $RepoRoot 'scripts')
    try {
        if (-not (Test-Path 'node_modules')) { npm install --silent --no-fund --no-audit }
        node rasterize-logo.mjs | Out-Null
    } finally { Pop-Location }

    $stillMissing = $requiredAssets | Where-Object { -not (Test-Path -LiteralPath $_) }
    if ($stillMissing) { Fail "Rasteriser did not produce: $($stillMissing -join ', ')" }
    Write-Ok 'Artwork generated'
} else {
    Write-Ok 'Artwork present'
}

# --------------------------------------------------------------- 3. cargo build

if ($SkipBuild) {
    Write-Step 'Skipping cargo build (-SkipBuild)'
} else {
    Write-Step 'Building release binaries'
    & $Cargo build --release --workspace
    if ($LASTEXITCODE -ne 0) { Fail "cargo build failed (exit $LASTEXITCODE)." }
    Write-Ok 'cargo build --release'
}

# ------------------------------------------------------------------ 4. staging

Write-Step 'Staging installer payload'

if (Test-Path -LiteralPath $Staging) { Remove-Item -LiteralPath $Staging -Recurse -Force }
New-Item -ItemType Directory -Path $Staging -Force | Out-Null

$included = [System.Collections.Generic.List[string]]::new()
$absent   = [System.Collections.Generic.List[string]]::new()

function Stage-Binary {
    param([string]$Name, [switch]$Required)
    $src = Join-Path $Release $Name
    if (Test-Path -LiteralPath $src) {
        Copy-Item -LiteralPath $src -Destination (Join-Path $Staging $Name) -Force
        $size = [math]::Round((Get-Item -LiteralPath $src).Length / 1MB, 2)
        $script:included.Add("$Name ($size MB)")
    } elseif ($Required) {
        Fail "$Name was not produced by the release build." @(
            'Check the cargo build output above.'
        )
    } else {
        $script:absent.Add($Name)
    }
}

Stage-Binary 'fdm.exe' -Required     # engine + CLI: the installer has no reason to exist without it
Stage-Binary 'fdm-desktop.exe'       # Phase 3
Stage-Binary 'fdm-host.exe'          # Phase 2
Stage-Binary 'WebView2Loader.dll'

# The unpacked extension is copied wholesale, minus anything a browser must not see.
$extSrc = Join-Path $RepoRoot 'extension'
if (Test-Path -LiteralPath (Join-Path $extSrc 'manifest.json')) {

    # extension\styles\tokens.css must be a copy of the brand tokens, because an
    # unpacked extension cannot reference a file outside its own directory. Refresh
    # it here so the two can never ship out of sync: the brand file is the source
    # of truth, and a stale copy would silently give the popup and the desktop app
    # different colours.
    $tokensSrc = Join-Path $RepoRoot 'brand\tokens\fdm-tokens.css'
    $tokensDst = Join-Path $extSrc 'styles\tokens.css'
    if (Test-Path -LiteralPath $tokensSrc) {
        $stale = -not (Test-Path -LiteralPath $tokensDst) -or
                 (Get-FileHash -LiteralPath $tokensSrc).Hash -ne
                 (Get-FileHash -LiteralPath $tokensDst).Hash
        if ($stale) {
            New-Item -ItemType Directory -Path (Split-Path -Parent $tokensDst) -Force | Out-Null
            Copy-Item -LiteralPath $tokensSrc -Destination $tokensDst -Force
            Write-Warn 'extension\styles\tokens.css was stale; refreshed from brand\tokens\fdm-tokens.css'
        }
    } else {
        Write-Warn "brand\tokens\fdm-tokens.css is missing; shipping extension\styles\tokens.css as-is"
    }

    $extDst = Join-Path $Staging 'extension'
    New-Item -ItemType Directory -Path $extDst -Force | Out-Null
    Copy-Item -Path (Join-Path $extSrc '*') -Destination $extDst -Recurse -Force `
        -Exclude @('node_modules', '*.map', '*.ts', 'tsconfig.json', 'package*.json')
    $count = (Get-ChildItem -LiteralPath $extDst -Recurse -File).Count
    $included.Add("extension\ ($count files)")
} else {
    $absent.Add('extension\manifest.json')
}

foreach ($i in $included) { Write-Ok $i }
foreach ($a in $absent)   { Write-Warn "not built yet: $a" }

# ------------------------------------------------------- 5. completeness gate

$blocking = @($absent | Where-Object { $_ -in @('fdm-desktop.exe', 'fdm-host.exe', 'extension\manifest.json') })

if ($blocking.Count -gt 0 -and -not $AllowPartial) {
    Fail "This build is missing $($blocking.Count) shipping component(s): $($blocking -join ', ')" @(
        'These are Phase 2-3 deliverables. To build a development installer with',
        'just the engine and CLI, re-run with -AllowPartial.',
        'Do NOT publish a -AllowPartial build to end users.'
    )
}

# ------------------------------------------------------------ 5b. sign payload
# The staged binaries must be signed BEFORE ISCC runs, because Setup embeds them.
# Signing the finished installer does not retroactively sign its contents.

$SignScript = Join-Path $PSScriptRoot 'sign.ps1'
$Signing = $SignThumbprint -or $SelfSign

if ($Signing) {
    Write-Step 'Signing payload binaries'
    $payload = @(Get-ChildItem -LiteralPath $Staging -Filter '*.exe' -File | Select-Object -ExpandProperty FullName)
    if ($payload) {
        if ($SignThumbprint) {
            & $SignScript -Thumbprint $SignThumbprint -Files $payload
        } else {
            & $SignScript -SelfSigned -Files $payload
        }
    }
}

# ------------------------------------------------------------ 6. compile setup

Write-Step 'Compiling installer'

New-Item -ItemType Directory -Path $OutputDir -Force | Out-Null

$isccArgs = @(
    "/DAppVersion=$Version",
    "/DStagingDir=$Staging",
    "/Q"
)
if ($ExtensionId) {
    $isccArgs += "/DChromeExtensionId=$ExtensionId"
    Write-Info "Chrome extension ID: $ExtensionId"
} else {
    Write-Warn 'No -ExtensionId given: the installer will NOT register the browser extension.'
    Write-Warn 'That is correct until the extension is published to the Chrome Web Store.'
}

# Hand ISCC a signing command so it can sign the uninstaller. unins000.exe is
# generated on the user's machine at install time, so signtool cannot reach it
# afterwards -- Inno has to embed a pre-signed copy, which is what SignedUninstaller
# plus this SignTool command arranges.
if ($Signing) {
    $st = Find-SignToolForIscc
    if ($SelfSign) {
        $devCert = Get-ChildItem Cert:\CurrentUser\My |
            Where-Object {
                $_.Subject -eq 'CN=Ahmad Farooq' -and $_.NotAfter -gt (Get-Date) -and
                $_.EnhancedKeyUsageList.ObjectId -contains '1.3.6.1.5.5.7.3.3'
            } | Sort-Object NotAfter -Descending | Select-Object -First 1
        if (-not $devCert) { Fail 'Expected sign.ps1 to have created a dev certificate, but none was found.' }
        $thumb = $devCert.Thumbprint
    } else {
        $thumb = $SignThumbprint -replace '\s', ''
    }
    # Inno substitutes $f with the target file name and $q with a double quote.
    # $q is load-bearing: signtool lives under "Program Files (x86)", so the
    # command needs quoting, but a literal " inside this argument gets mangled
    # by PowerShell's native-argument escaping and ISCC then reads the tail as a
    # second script filename ("You may not specify more than one script
    # filename"). $q sidesteps quoting on both sides.
    $signCmd = '$q' + $st + '$q sign /fd SHA256 /tr http://timestamp.digicert.com' +
               ' /td SHA256 /sha1 ' + $thumb + ' $f'
    $isccArgs += "/Sfdmsign=$signCmd"
    $isccArgs += '/DSignToolName=fdmsign'
    Write-Info 'Uninstaller will be signed by Inno.'
}

$isccArgs += $IssFile

& $Iscc @isccArgs
if ($LASTEXITCODE -ne 0) { Fail "ISCC failed (exit $LASTEXITCODE)." }

$setup = Join-Path $OutputDir "FDM-Setup-$Version.exe"
if (-not (Test-Path -LiteralPath $setup)) { Fail 'ISCC reported success but produced no output file.' }

if ($Signing) {
    Write-Step 'Signing the setup file'
    if ($SignThumbprint) {
        & $SignScript -Thumbprint $SignThumbprint -Files @($setup)
    } else {
        & $SignScript -SelfSigned -Files @($setup)
    }
}

$setupMb = [math]::Round((Get-Item -LiteralPath $setup).Length / 1MB, 2)

Write-Host ''
Write-Host "  Setup file: $setup" -ForegroundColor Green
Write-Host "  Size:       $setupMb MB" -ForegroundColor Gray
Write-Host ''

if ($blocking.Count -gt 0) {
    Write-Host '  DEVELOPMENT BUILD -- incomplete, do not publish.' -ForegroundColor Yellow
    Write-Host ''
}

if ($Signing) {
    if ($SelfSign) {
        Write-Host '  Signed with a DEVELOPMENT certificate -- trusted on this machine only.' -ForegroundColor Yellow
    } else {
        Write-Host '  Signed and timestamped. Windows will show the publisher name.' -ForegroundColor Green
    }
    Write-Host ''
} else {
    Write-Host '  UNSIGNED: Windows will show "Publisher: Unknown" on the UAC prompt.' -ForegroundColor Yellow
    Write-Host '  Fix with a real certificate:  scripts\build-installer.ps1 -SignThumbprint <hex>' -ForegroundColor DarkGray
    Write-Host '  Or preview it locally:        scripts\sign.ps1 -SelfSigned -TrustSelfSigned' -ForegroundColor DarkGray
    Write-Host ''
}
